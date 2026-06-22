import Metal
import MetalKit
import AppKit
import CoreText
import os.signpost

// ── Constants ────────────────────────────────────────────────────────────────

private let ATLAS_SIZE:         Int = 2048
private let MAX_VERTS_PER_FRAME = 524288  // 512K vertices = ~87K quads (6 verts each)
private let VERTEX_STRIDE:      Int = 32  // 2×f32 pos + 2×f32 uv + 4×f32 color

// ── MetalRenderer ─────────────────────────────────────────────────────────────

final class MetalRenderer: NSObject, MTKViewDelegate {
    private let device: MTLDevice
    private let commandQueue: MTLCommandQueue
    private let pipelineState: MTLRenderPipelineState
    private var fragmentFunction: MTLFunction!
    
    var isHeadless = false

    // ── Buffers ───────────────────────────────────────────────────────────
    /// Vertex buffer — triple-buffered to avoid CPU-GPU stalls.
    private var vertexBuffers: [MTLBuffer] = []
    private var currentBufferIndex: Int = 0
    private let sema = DispatchSemaphore(value: 3)  // triple-buffering semaphore

    /// Uniform buffer.
    private var uniformBuffer: MTLBuffer!

    /// The texture atlas (R8Unorm, 2048×2048 = 4MB VRAM).
    private var atlasTexture: MTLTexture!

    /// Argument Buffer (Metal 3 Bindless) — holds atlas texture + sampler handles.
    private var argumentBuffer: MTLBuffer!
    private var argumentEncoder: MTLArgumentEncoder!

    // ── Dependencies ──────────────────────────────────────────────────────
    var textBuffer:    TextBuffer?
    var shapingEngine: ShapingEngine?
    var atlas:         TextureAtlasSwift?

    // ── State ─────────────────────────────────────────────────────────────
    var scrollY:    CGFloat = 0
    var lineHeight: CGFloat = 20

    // ── Metrics ───────────────────────────────────────────────────────────
    var onFrameRendered: ((FrameStatsItem) -> Void)?

    struct FrameStatsItem {
        var cpuEncodeNs: UInt64
        var glyphCount:  Int
        var atlasHitRate: Float
    }

    // ── Init ──────────────────────────────────────────────────────────────

    init(mtkView: MTKView) throws {
        guard let dev = MTLCreateSystemDefaultDevice() else {
            throw NSError(domain: "MetalRenderer", code: -1, userInfo: [NSLocalizedDescriptionKey: "No Metal device"])
        }
        self.device = dev
        mtkView.device = dev
        mtkView.colorPixelFormat = .bgra8Unorm
        mtkView.preferredFramesPerSecond = 120
        mtkView.enableSetNeedsDisplay = false
        mtkView.isPaused = false

        self.commandQueue = dev.makeCommandQueue()!

        // ── Pipeline state ─────────────────────────────────────────────
        let shaderURL = Bundle.module.url(forResource: "GlyphShader", withExtension: "metal")!
        let shaderStr = try! String(contentsOf: shaderURL, encoding: .utf8)
        let library   = try! dev.makeLibrary(source: shaderStr, options: nil)
        
        let vertFn  = library.makeFunction(name: "glyph_vertex")!
        let fragFn  = library.makeFunction(name: "glyph_fragment")!
        self.fragmentFunction = fragFn

        let desc = MTLRenderPipelineDescriptor()
        desc.label                              = "GlyphPipeline"
        desc.vertexFunction                     = vertFn
        desc.fragmentFunction                   = fragFn
        desc.colorAttachments[0].pixelFormat    = .bgra8Unorm
        // Premultiplied alpha blending
        desc.colorAttachments[0].isBlendingEnabled             = true
        desc.colorAttachments[0].rgbBlendOperation             = .add
        desc.colorAttachments[0].alphaBlendOperation           = .add
        desc.colorAttachments[0].sourceRGBBlendFactor          = .one
        desc.colorAttachments[0].destinationRGBBlendFactor     = .oneMinusSourceAlpha
        desc.colorAttachments[0].sourceAlphaBlendFactor        = .one
        desc.colorAttachments[0].destinationAlphaBlendFactor   = .oneMinusSourceAlpha

        // Vertex descriptor
        let vd = MTLVertexDescriptor()
        vd.attributes[0].format = .float2; vd.attributes[0].offset = 0;  vd.attributes[0].bufferIndex = 0
        vd.attributes[1].format = .float2; vd.attributes[1].offset = 8;  vd.attributes[1].bufferIndex = 0
        vd.attributes[2].format = .float4; vd.attributes[2].offset = 16; vd.attributes[2].bufferIndex = 0
        vd.layouts[0].stride = VERTEX_STRIDE
        desc.vertexDescriptor = vd

        self.pipelineState = try dev.makeRenderPipelineState(descriptor: desc)

        super.init()

        buildBuffers()
        buildAtlas()
        buildArgumentBuffer()
    }

    // ── Buffers ───────────────────────────────────────────────────────────

    private func buildBuffers() {
        // Triple-buffered vertex buffers (3 × 512K vertices × 32 bytes = 48 MB)
        vertexBuffers = (0..<3).map { i in
            let buf = device.makeBuffer(length: MAX_VERTS_PER_FRAME * VERTEX_STRIDE,
                                        options: .storageModeShared)!
            buf.label = "VertexBuffer[\(i)]"
            return buf
        }

        // Uniform buffer (16 bytes: vec2 + f32 + pad)
        uniformBuffer = device.makeBuffer(length: 16, options: .storageModeShared)!
        uniformBuffer.label = "Uniforms"
    }

    private func buildAtlas() {
        let desc = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat: .r8Unorm,
            width:  ATLAS_SIZE,
            height: ATLAS_SIZE,
            mipmapped: false
        )
        desc.usage  = [.shaderRead]
        desc.storageMode = .shared  // shared between CPU+GPU (UMA)
        atlasTexture = device.makeTexture(descriptor: desc)!
        atlasTexture.label = "GlyphAtlas"
    }

    private func buildArgumentBuffer() {
        // Create an argument encoder for the fragment function's buffer(0) argument
        argumentEncoder = fragmentFunction.makeArgumentEncoder(bufferIndex: 0)
        let argBufLen = argumentEncoder.encodedLength
        argumentBuffer = device.makeBuffer(length: argBufLen, options: .storageModeShared)!
        argumentBuffer.label = "GlyphArgumentBuffer"

        argumentEncoder.setArgumentBuffer(argumentBuffer, offset: 0)

        // Set texture handle (Bindless — Metal 3 Tier 2)
        argumentEncoder.setTexture(atlasTexture, index: 0)

        // Set sampler handle
        let samplerDesc = MTLSamplerDescriptor()
        samplerDesc.minFilter = .nearest
        samplerDesc.magFilter = .nearest
        let sampler = device.makeSamplerState(descriptor: samplerDesc)!
        argumentEncoder.setSamplerState(sampler, index: 1)
    }

    // ── MTKViewDelegate ───────────────────────────────────────────────────

    func mtkView(_ view: MTKView, drawableSizeWillChange size: CGSize) {}

    func draw(in view: MTKView) {
        let t0 = mach_absolute_time()
        os_signpost(.begin, log: signpostLog, name: "MetalFrame")

        guard
            let textBuffer = textBuffer,
            let shapingEngine = shapingEngine,
            let atlas = atlas
        else { return }

        let commandBuffer = commandQueue.makeCommandBuffer()!
        let renderPassDescriptor = isHeadless ? nil : view.currentRenderPassDescriptor
        let drawable = isHeadless ? nil : view.currentDrawable

        // ── Viewport geometry ─────────────────────────────────────────────
        let W = Float(view.drawableSize.width)
        let H = Float(view.drawableSize.height)
        let firstLine = max(0, Int(scrollY / lineHeight))
        let lastLine  = min(textBuffer.lineCount - 1, firstLine + Int(H / Float(lineHeight)) + 2)

        // ── Lazy shaping ──────────────────────────────────────────────────
        shapingEngine.shapeViewport(buffer: textBuffer, firstLine: firstLine, lastLine: lastLine)

        // ── Build vertex data ─────────────────────────────────────────────
        sema.wait()
        let vbuf = vertexBuffers[currentBufferIndex]
        currentBufferIndex = (currentBufferIndex + 1) % 3
        let vertPtr = vbuf.contents().assumingMemoryBound(to: Float.self)
        var vertexCount = 0
        let atlasInv = Float(1.0 / Double(ATLAS_SIZE))
        let textColor = SIMD4<Float>(0.788, 0.820, 0.851, 1.0)

        for li in firstLine...lastLine {
            guard let runs = shapingEngine.runs(forLine: li) else { continue }
            let baseY = Float(CGFloat(li) * lineHeight - scrollY)
            var cursorX: Float = 52 // left margin

            for run in runs {
                for (i, glyph) in run.glyphIDs.enumerated() {
                    guard let slot = atlas.slot(forGlyph: glyph, font: run.font) else {
                        cursorX += Float(run.advances[i].width)
                        continue
                    }
                    if vertexCount + 6 > MAX_VERTS_PER_FRAME { break }

                    let x0 = cursorX + Float(slot.offsetX)
                    let y0 = baseY   + Float(slot.offsetY)
                    let x1 = x0 + Float(slot.width)
                    let y1 = y0 + Float(slot.height)
                    let u0 = Float(slot.atlasX) * atlasInv
                    let v0 = Float(slot.atlasY) * atlasInv
                    let u1 = u0 + Float(slot.width)  * atlasInv
                    let v1 = v0 + Float(slot.height) * atlasInv

                    // 6 vertices: 2 triangles (TL,TR,BL, TR,BR,BL).
                    // AUDIT FIX (P7): write each vertex directly to vertexPtr
                    // with static offsets, avoiding the per-glyph Swift Array of
                    // 6 tuples (millions of ARC allocs/sec on the hot path).
                    let floatsPerVertex = VERTEX_STRIDE / 4
                    @inline(__always)
                    func emit(_ qx: Float, _ qy: Float, _ qu: Float, _ qv: Float) {
                        let base = vertexCount * floatsPerVertex
                        vertPtr[base+0] = qx; vertPtr[base+1] = qy
                        vertPtr[base+2] = qu; vertPtr[base+3] = qv
                        vertPtr[base+4] = textColor.x; vertPtr[base+5] = textColor.y
                        vertPtr[base+6] = textColor.z; vertPtr[base+7] = textColor.w
                        vertexCount += 1
                    }
                    emit(x0,y0,u0,v0); emit(x1,y0,u1,v0); emit(x0,y1,u0,v1)
                    emit(x1,y0,u1,v0); emit(x1,y1,u1,v1); emit(x0,y1,u0,v1)
                    cursorX += Float(run.advances[i].width)
                }
            }
        }

        // ── Update uniform buffer ─────────────────────────────────────────
        let uniPtr = uniformBuffer.contents().assumingMemoryBound(to: Float.self)
        uniPtr[0] = W; uniPtr[1] = H; uniPtr[2] = Float(ATLAS_SIZE); uniPtr[3] = 0

        // ── Encode render pass ────────────────────────────────────────────
        if let renderPassDescriptor = renderPassDescriptor, let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: renderPassDescriptor) {
            encoder.label = "GlyphRenderPass"
            encoder.setRenderPipelineState(pipelineState)
            
            encoder.setVertexBuffer(vbuf,           offset: 0, index: 0)
            encoder.setVertexBuffer(uniformBuffer,  offset: 0, index: 1)
            
            // ── BINDLESS: one argument buffer binding for all fragment resources ──
            encoder.setFragmentBuffer(argumentBuffer, offset: 0, index: 0)
            // Tell Metal which resources are resident (required for Tier 2)
            encoder.useResource(atlasTexture, usage: .read, stages: .fragment)
            
            if vertexCount > 0 {
                encoder.drawPrimitives(type: .triangle, vertexStart: 0, vertexCount: vertexCount)
            }
            encoder.endEncoding()
        }

        commandBuffer.addCompletedHandler { [weak self] _ in self?.sema.signal() }
        if let drawable = drawable {
            commandBuffer.present(drawable)
        }
        commandBuffer.commit()

        os_signpost(.end, log: signpostLog, name: "MetalFrame")
        let cpuNs = machTimeDeltaNs(mach_absolute_time() - t0)
        onFrameRendered?(FrameStatsItem(
            cpuEncodeNs: cpuNs,
            glyphCount: vertexCount / 6,
            atlasHitRate: atlas.hitRate
        ))
    }
}

// ── Utilities ─────────────────────────────────────────────────────────────────

private func machTimeDeltaNs(_ delta: UInt64) -> UInt64 {
    var info = mach_timebase_info_data_t()
    mach_timebase_info(&info)
    return delta * UInt64(info.numer) / UInt64(info.denom)
}
