#include <metal_stdlib>
using namespace metal;

// ── Shared types (CPU/GPU) ────────────────────────────────────────────────────
// Note: include this file from Swift via a bridging header or Types.h.

struct GlyphVertex {
    float2 position  [[attribute(0)]];  // screen pixels, top-left of quad vertex
    float2 uv        [[attribute(1)]];  // atlas UV in [0,1]
    float4 color     [[attribute(2)]];  // RGBA premultiplied
};

struct GlyphUniforms {
    float2 viewportSize;   // width, height in pixels
    float  atlasSize;      // e.g. 2048.0
    float  _pad;
};

// ── Argument Buffer (Bindless Rendering — Metal 3 Tier 2) ─────────────────────
// On Apple Silicon, one pointer to this struct is the ONLY binding call.
// This eliminates per-glyph / per-draw binding overhead.

struct GlyphArguments {
    texture2d<float>  atlasTexture [[id(0)]];
    sampler           atlasSampler [[id(1)]];
};

struct VertexOut {
    float4 position [[position]];
    float2 uv;
    float4 color;
};

// ── Vertex shader ─────────────────────────────────────────────────────────────

vertex VertexOut glyph_vertex(
    GlyphVertex        in       [[stage_in]],
    constant GlyphUniforms& uni [[buffer(1)]]
) {
    VertexOut out;

    // Map screen pixels → clip space ([0,W] → [-1,1], Y flipped)
    float2 clip = float2(
         in.position.x / uni.viewportSize.x * 2.0 - 1.0,
        -in.position.y / uni.viewportSize.y * 2.0 + 1.0
    );
    out.position = float4(clip, 0.0, 1.0);
    out.uv       = in.uv;
    out.color    = in.color;
    return out;
}

// ── Fragment shader using Argument Buffer ─────────────────────────────────────
//
// The `args` parameter receives the Argument Buffer pointer set by
// `setFragmentBuffer(argumentBuffer, offset: 0, index: 0)` — ONE binding call
// gives access to all resources (texture + sampler).
//
// Metal 3 Tier 2 (Apple Silicon only): argument buffers are resident in GPU
// address space; the driver doesn't validate individual bindings per draw.

fragment float4 glyph_fragment(
    VertexOut             in   [[stage_in]],
    device GlyphArguments& args [[buffer(0)]]
) {
    // Sample the R8Unorm alpha atlas
    float coverage = args.atlasTexture.sample(args.atlasSampler, in.uv).r;

    // Soft AA edge (equivalent to smoothstep in the WebGPU WGSL version)
    float alpha = smoothstep(0.40h, 0.60h, half(coverage)) * in.color.a;

    // Premultiplied alpha output
    return float4(in.color.rgb * alpha, alpha);
}
