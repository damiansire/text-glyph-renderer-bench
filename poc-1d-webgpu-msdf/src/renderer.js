/**
 * renderer.js — WebGPU MSDF Renderer for PoC 1D
 *
 * Key differences versus PoC 1C (dynamic atlas):
 *  1. Atlas is RGBA8 (pre-generated offline by msdf-atlas-gen).
 *     Loaded once from inter_msdf_atlas.png + inter_msdf_atlas.json.
 *  2. NO CPU rasterization at runtime → atlas hit rate is always 1.0.
 *  3. Glyph lookup: codepoint → metadata from JSON (advance, UV box, offsets).
 *  4. Uniform buffer includes `px_range` field for adaptive AA in the shader.
 *  5. Same instanced-quad draw call as PoC 1C.
 *
 * Expected behaviour:
 *  - First frame may be slow (loading atlas PNG via createImageBitmap).
 *  - Steady-state: ZERO atlas misses, lowest CPU encode time of all WebGPU PoCs.
 *  - Works at any DPI / zoom without re-rasterization.
 */

'use strict';

const INSTANCE_BYTE_STRIDE = 40;
const MAX_GLYPHS_PER_FRAME = 65536;
const ATLAS_FORMAT = 'rgba8unorm';  // MSDF is 3-channel; we use RGBA8 from PNG

class MSDFRenderer {
    constructor() {
        this._device = null;
        this._pipeline = null;
        this._uniformBuf = null;
        this._instanceBuf = null;
        this._bindGroup = null;
        this._glyphMap = new Map();  // codepoint → GlyphMetrics
        this._atlasSize = 2048;
        this._pxRange = 4;
        this._instanceData = new ArrayBuffer(INSTANCE_BYTE_STRIDE * MAX_GLYPHS_PER_FRAME);
        this._f32view = new Float32Array(this._instanceData);
        this._u32view = new Uint32Array(this._instanceData);
    }

    // ── Init ──────────────────────────────────────────────────────────────────

    async init(canvas, atlasMetaJson) {
        this._canvas = canvas;

        // Parse atlas metadata JSON
        this._parseAtlasMeta(atlasMetaJson);

        const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
        if (!adapter) throw new Error('No WebGPU adapter');
        this._device = await adapter.requestDevice();

        this._context = canvas.getContext('webgpu');
        const format = navigator.gpu.getPreferredCanvasFormat();
        this._format = format;
        this._context.configure({ device: this._device, format, alphaMode: 'premultiplied' });

        await this._buildPipeline(format);
        this._buildBuffers();

        // Load atlas texture from PNG
        await this._loadAtlasTexture();

        return this._device;
    }

    _parseAtlasMeta(json) {
        const { atlas, glyphs } = json;
        this._atlasSize = atlas.width || 2048;
        this._pxRange = atlas.distanceRange || 4;

        for (const g of glyphs) {
            if (!g.atlasBounds) continue;
            const { left: u, bottom: v, right: r, top: t } = g.atlasBounds;
            const { left: pl, bottom: pb, right: pr, top: pt } = g.planeBounds || {};
            this._glyphMap.set(g.unicode, {
                u: u,
                v: this._atlasSize - t,  // flip Y (msdf-atlas-gen uses bottom-origin)
                w: r - u,
                h: t - v,
                offsetX: pl || 0,
                offsetY: -(pt || 0),           // flip Y
                advance: g.advance || 0,
            });
        }
    }

    async _loadAtlasTexture() {
        const fs = require('fs');
        const png = fs.readFileSync('./assets/inter_msdf_atlas.png');
        const blob = new Blob([png], { type: 'image/png' });
        const bitmap = await createImageBitmap(blob);

        const texture = this._device.createTexture({
            label: 'msdf-atlas',
            size: [bitmap.width, bitmap.height, 1],
            format: ATLAS_FORMAT,
            usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
        });

        this._device.queue.copyExternalImageToTexture(
            { source: bitmap },
            { texture },
            [bitmap.width, bitmap.height]
        );

        this._atlasTex = texture;
        this._atlasView = texture.createView();

        // Rebuild bind group with the real texture
        this._buildBindGroup();
    }

    async _buildPipeline(format) {
        const device = this._device;
        const [vertCode, fragCode] = await Promise.all([
            fetch('./src/shaders/glyph.vert.wgsl').then(r => r.text()),
            fetch('./src/shaders/glyph_msdf.frag.wgsl').then(r => r.text()),
        ]);

        const vertModule = device.createShaderModule({ label: 'msdf-vert', code: vertCode });
        const fragModule = device.createShaderModule({ label: 'msdf-frag', code: fragCode });

        // MSDF needs LINEAR sampling for smooth interpolation
        this._sampler = device.createSampler({
            magFilter: 'linear', minFilter: 'linear', mipmapFilter: 'linear',
        });

        this._bgl = device.createBindGroupLayout({
            entries: [
                { binding: 0, visibility: GPUShaderStage.VERTEX | GPUShaderStage.FRAGMENT, buffer: { type: 'uniform' } },
                { binding: 1, visibility: GPUShaderStage.FRAGMENT, texture: { sampleType: 'float' } },
                { binding: 2, visibility: GPUShaderStage.FRAGMENT, sampler: { type: 'filtering' } },
            ],
        });

        this._pipeline = device.createRenderPipeline({
            label: 'msdf-pipeline',
            layout: device.createPipelineLayout({ bindGroupLayouts: [this._bgl] }),
            vertex: {
                module: vertModule, entryPoint: 'vs_main',
                buffers: [{
                    arrayStride: INSTANCE_BYTE_STRIDE,
                    stepMode: 'instance',
                    attributes: [
                        { shaderLocation: 0, offset: 0, format: 'float32x2' },
                        { shaderLocation: 1, offset: 8, format: 'float32x4' },
                        { shaderLocation: 2, offset: 24, format: 'uint32' },
                        { shaderLocation: 3, offset: 28, format: 'float32x2' },
                    ],
                }],
            },
            fragment: {
                module: fragModule, entryPoint: 'fs_main',
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
        // Uniforms: viewport(vec2f), atlas_size(f32), px_range(f32)
        this._uniformBuf = device.createBuffer({ size: 16, usage: GPUBufferUsage.UNIFORM | GPUBufferUsage.COPY_DST });
        this._instanceBuf = device.createBuffer({
            size: INSTANCE_BYTE_STRIDE * MAX_GLYPHS_PER_FRAME,
            usage: GPUBufferUsage.VERTEX | GPUBufferUsage.COPY_DST,
        });
    }

    _buildBindGroup() {
        this._bindGroup = this._device.createBindGroup({
            layout: this._bgl,
            entries: [
                { binding: 0, resource: { buffer: this._uniformBuf } },
                { binding: 1, resource: this._atlasView },
                { binding: 2, resource: this._sampler },
            ],
        });
    }

    // ── Frame ─────────────────────────────────────────────────────────────────

    renderFrame(lines, scrollY, lineHeight, fontSize = 13) {
        if (!this._bindGroup) return 0; // Atlas not loaded yet
        const t0 = performance.now();
        const W = this._canvas.width;
        const H = this._canvas.height;
        const scale = fontSize / 64; // MSDF was generated at 64px

        this._device.queue.writeBuffer(
            this._uniformBuf, 0,
            new Float32Array([W, H, this._atlasSize, this._pxRange])
        );

        let instanceCount = 0;
        const f32 = this._f32view;
        const u32 = this._u32view;

        const firstLine = Math.max(0, Math.floor(scrollY / lineHeight));
        const lastLine = Math.min(lines.length - 1, firstLine + Math.ceil(H / lineHeight) + 2);
        const COLOR_U32 = 0xC9D1D9FF;

        for (let li = firstLine; li <= lastLine && instanceCount < MAX_GLYPHS_PER_FRAME; li++) {
            const text = lines[li] || '';
            const baseY = li * lineHeight - scrollY;
            let cursorX = 52;

            for (let ci = 0; ci < text.length && instanceCount < MAX_GLYPHS_PER_FRAME; ci++) {
                const cp = text.codePointAt(ci) || 32;
                if (cp > 0xFFFF) ci++;
                const g = this._glyphMap.get(cp) || this._glyphMap.get(0x3F); // '?' fallback
                if (!g) { cursorX += fontSize * 0.6; continue; }

                const gw = g.w * scale;
                const gh = g.h * scale;
                const base = (instanceCount * INSTANCE_BYTE_STRIDE) >> 2;
                f32[base + 0] = cursorX + g.offsetX * scale;
                f32[base + 1] = baseY + g.offsetY * scale;
                f32[base + 2] = g.u;
                f32[base + 3] = g.v;
                f32[base + 4] = g.w;
                f32[base + 5] = g.h;
                u32[base + 6] = COLOR_U32;
                f32[base + 7] = gw;
                f32[base + 8] = gh;

                cursorX += g.advance * scale;
                instanceCount++;
            }
        }

        if (instanceCount === 0) return 0;

        this._device.queue.writeBuffer(this._instanceBuf, 0, this._instanceData, 0, instanceCount * INSTANCE_BYTE_STRIDE);

        const encoder = this._device.createCommandEncoder();
        const pass = encoder.beginRenderPass({
            colorAttachments: [{
                view: this._context.getCurrentTexture().createView(),
                clearValue: { r: 0.102, g: 0.106, b: 0.118, a: 1 },
                loadOp: 'clear', storeOp: 'store',
            }],
        });
        pass.setPipeline(this._pipeline);
        pass.setBindGroup(0, this._bindGroup);
        pass.setVertexBuffer(0, this._instanceBuf);
        pass.draw(6, instanceCount);
        pass.end();
        this._device.queue.submit([encoder.finish()]);

        return performance.now() - t0;
    }
}

window.MSDFRenderer = MSDFRenderer;
