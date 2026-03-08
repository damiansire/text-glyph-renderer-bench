/**
 * renderer.js — WebGPU Render Pipeline for PoC 1C (Texture Atlas)
 *
 * Architecture:
 *   - ONE render pass per frame (single draw call for all visible glyphs).
 *   - Vertex data: instanced quads (6 verts × N glyph instances).
 *   - Instance data: GlyphInstance structs written to a GPUBuffer each frame.
 *   - Atlas: R8Unorm 2048×2048 (from atlas.js).
 *   - Bind Group 0: scene uniforms, atlas texture, sampler.
 *
 * GPU pipeline config:
 *   - Topology: triangle-list (2 triangles per glyph, 6 verts).
 *   - Blend: premultiplied alpha over background.
 *   - Format: bgra8unorm (swap-chain default on macOS).
 */

'use strict';

// GlyphInstance GPU layout:
//  [ screen_pos.x f32 ][ screen_pos.y f32 ]
//  [ atlas_uv.x f32   ][ atlas_uv.y f32   ][ atlas_uv.w f32 ][ atlas_uv.h f32 ]
//  [ color_rgba u32   ]
//  [ glyph_w f32      ][ glyph_h f32      ]
//  total: 9 × 4 = 36 bytes, padded to 40 for alignment

const INSTANCE_BYTE_STRIDE = 40;
const MAX_GLYPHS_PER_FRAME = 65536;

class GlyphRenderer {
    constructor() {
        this._device = null;
        this._pipeline = null;
        this._uniformBuf = null;
        this._instanceBuf = null;
        this._bindGroup = null;
        this._instanceData = new ArrayBuffer(INSTANCE_BYTE_STRIDE * MAX_GLYPHS_PER_FRAME);
        this._f32view = new Float32Array(this._instanceData);
        this._u32view = new Uint32Array(this._instanceData);
        this._atlasSize = 2048;
    }

    // ── Init ──────────────────────────────────────────────────────────────────

    async init(canvas, atlas) {
        const adapter = await navigator.gpu.requestAdapter({
            powerPreference: 'high-performance',
        });
        if (!adapter) throw new Error('No WebGPU adapter found');

        this._device = await adapter.requestDevice({
            label: 'text-engine-poc-1c',
        });
        this._atlas = atlas;  // TextureAtlas instance (already constructed with device)

        this._canvas = canvas;
        this._context = canvas.getContext('webgpu');
        const format = navigator.gpu.getPreferredCanvasFormat();
        this._format = format;

        this._context.configure({
            device: this._device,
            format,
            alphaMode: 'premultiplied',
        });

        await this._buildPipeline(format);
        return this._device;
    }

    async _buildPipeline(format) {
        const device = this._device;

        // Load WGSL shaders from same directory
        const [vertCode, fragCode] = await Promise.all([
            fetch('./src/shaders/glyph.vert.wgsl').then(r => r.text()),
            fetch('./src/shaders/glyph.frag.wgsl').then(r => r.text()),
        ]);

        const vertModule = device.createShaderModule({ label: 'glyph-vert', code: vertCode });
        const fragModule = device.createShaderModule({ label: 'glyph-frag', code: fragCode });

        // Bind group layout: uniform buffer + texture + sampler
        this._bgl = device.createBindGroupLayout({
            label: 'scene-bgl',
            entries: [
                { binding: 0, visibility: GPUShaderStage.VERTEX, buffer: { type: 'uniform' } },
                { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'unfilterable-float' } },
                { binding: 2, visibility: GPUShaderStage.FRAGMENT, sampler: { type: 'non-filtering' } },
            ],
        });

        this._pipeline = device.createRenderPipeline({
            label: 'glyph-pipeline',
            layout: device.createPipelineLayout({ bindGroupLayouts: [this._bgl] }),
            vertex: {
                module: vertModule,
                entryPoint: 'vs_main',
                buffers: [{
                    arrayStride: INSTANCE_BYTE_STRIDE,
                    stepMode: 'instance',
                    attributes: [
                        { shaderLocation: 0, offset: 0, format: 'float32x2' }, // screen_pos
                        { shaderLocation: 1, offset: 8, format: 'float32x4' }, // atlas_uv (u,v,w,h)
                        { shaderLocation: 2, offset: 24, format: 'uint32' }, // color_rgba
                        { shaderLocation: 3, offset: 28, format: 'float32x2' }, // glyph_wh
                    ],
                }],
            },
            fragment: {
                module: fragModule,
                entryPoint: 'fs_main',
                targets: [{
                    format,
                    blend: {
                        color: { srcFactor: 'one', dstFactor: 'one-minus-src-alpha', operation: 'add' },
                        alpha: { srcFactor: 'one', dstFactor: 'one-minus-src-alpha', operation: 'add' },
                    },
                }],
            },
            primitive: { topology: 'triangle-list', cullMode: 'none' },
        });
    }

    _buildBuffers() {
        const device = this._device;

        // Uniform buffer: viewport_size (vec2f), atlas_size (f32), pad (f32)
        this._uniformBuf = device.createBuffer({
            label: 'uniforms',
            size: 16,
            usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST,
        });

        // Instance buffer (updated every frame with mapAsync or writeBuffer)
        this._instanceBuf = device.createBuffer({
            label: 'glyph-instances',
            size: INSTANCE_BYTE_STRIDE * MAX_GLYPHS_PER_FRAME,
            usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
        });

        // Sampler (nearest — we use CPU-AA via smoothstep in the shader)
        const sampler = this._device.createSampler({
            label: 'atlas-sampler',
            magFilter: 'nearest',
            minFilter: 'nearest',
            mipmapFilter: 'nearest',
        });

        this._bindGroup = this._device.createBindGroup({
            label: 'scene-bg',
            layout: this._bgl,
            entries: [
                { binding: 0, resource: { buffer: this._uniformBuf } },
                { binding: 1, resource: this._atlas.view },
                { binding: 2, resource: sampler },
            ],
        });
    }

    // ── Frame ─────────────────────────────────────────────────────────────────

    /**
     * Render a frame.
     * @param {string[]} lines       — all file lines
     * @param {number}   scrollY     — current scroll offset in pixels
     * @param {number}   lineHeight  — px per line
     * @param {string}   fontFamily  — CSS font family
     * @param {number}   fontSize    — CSS px font size
     * @returns {number} CPU encode time in ms
     */
    renderFrame(lines, scrollY, lineHeight, fontFamily = 'monospace', fontSize = 13) {
        const t0 = performance.now();
        const canvas = this._canvas;
        const W = canvas.width;
        const H = canvas.height;
        const device = this._device;

        // Update viewport uniform
        const uniformData = new Float32Array([W, H, this._atlasSize, 0]);
        device.queue.writeBuffer(this._uniformBuf, 0, uniformData);

        // Build instance data for visible glyphs
        let instanceCount = 0;
        const f32 = this._f32view;
        const u32 = this._u32view;

        const firstLine = Math.max(0, Math.floor(scrollY / lineHeight));
        const lastLine = Math.min(lines.length - 1, firstLine + Math.ceil(H / lineHeight) + 2);
        const COLOR_RGBA = 0xC9D1D9FF; // text color as u32

        for (let li = firstLine; li <= lastLine && instanceCount < MAX_GLYPHS_PER_FRAME; li++) {
            const text = lines[li] || '';
            const baseY = li * lineHeight - scrollY;
            let cursorX = 52; // left margin (line number area)

            for (let ci = 0; ci < text.length && instanceCount < MAX_GLYPHS_PER_FRAME; ci++) {
                const cp = text.codePointAt(ci) || 32;
                if (cp > 0xFFFF) ci++; // surrogate pair advance

                const slot = this._atlas.get(fontFamily, cp, fontSize);
                if (!slot) { cursorX += fontSize * 0.6; continue; } // fallback advance

                const base = (instanceCount * INSTANCE_BYTE_STRIDE) >> 2; // f32 index
                f32[base + 0] = cursorX + slot.offsetX;
                f32[base + 1] = baseY + slot.offsetY;
                f32[base + 2] = slot.u;
                f32[base + 3] = slot.v;
                f32[base + 4] = slot.w;
                f32[base + 5] = slot.h;
                u32[base + 6] = COLOR_RGBA;
                f32[base + 7] = slot.w;
                f32[base + 8] = slot.h;

                cursorX += slot.advance;
                instanceCount++;
            }
        }

        if (instanceCount === 0) return 0;

        // Upload instance data
        device.queue.writeBuffer(
            this._instanceBuf, 0,
            this._instanceData, 0,
            instanceCount * INSTANCE_BYTE_STRIDE
        );

        // Encode render pass
        const encoder = device.createCommandEncoder({ label: 'frame-encoder' });
        const pass = encoder.beginRenderPass({
            colorAttachments: [{
                view: this._context.getCurrentTexture().createView(),
                clearValue: { r: 0.102, g: 0.106, b: 0.118, a: 1.0 }, // #1a1b1e
                loadOp: 'clear',
                storeOp: 'store',
            }],
        });

        pass.setPipeline(this._pipeline);
        pass.setBindGroup(0, this._bindGroup);
        pass.setVertexBuffer(0, this._instanceBuf);
        pass.draw(6, instanceCount, 0, 0); // 6 verts per quad (2 triangles)
        pass.end();

        device.queue.submit([encoder.finish()]);

        return performance.now() - t0;
    }

    get deviceLost() { return this._device?.lost; }
}

window.GlyphRenderer = GlyphRenderer;
