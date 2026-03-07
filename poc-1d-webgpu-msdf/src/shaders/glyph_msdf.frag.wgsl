// glyph_msdf.frag.wgsl — Fragment shader for MSDF (Multi-channel Signed Distance Field) glyphs
//
// Key difference vs. plain atlas (glyph.frag.wgsl):
//   The atlas is RGBA (3 channels used for MSDF, 1 for optional SDF).
//   We extract the median of R,G,B channels — this is the fundamental
//   MSDF operation that eliminates the corner artifacts of single-channel SDF.
//
// Advantages over dynamic atlas (PoC 1C):
//   1. Scale-independent: ONE atlas serves all font sizes (8pt – 72pt+).
//   2. No CPU rasterization at runtime → zero atlas misses after initial load.
//   3. No LRU evictions → hitRate = 1.0 always.
//
// Disadvantages:
//   1. Pre-generation step (msdf-atlas-gen, ~5s offline).
//   2. Atlas is RGB (3×8bit) → 25% more VRAM than R8Unorm.
//   3. Corner artefacts for very complex glyphs (CJK strokes, rare edge cases).
//   4. Does NOT support subpixel hinting (acceptable for Retina displays).

@group(0) @binding(1) var msdf_atlas   : texture_2d<f32>;
@group(0) @binding(2) var msdf_sampler : sampler;

struct Uniforms {
    viewport_size : vec2<f32>,
    atlas_size    : f32,
    px_range      : f32,   // pxRange used during MSDF generation (typically 4.0)
}

@group(0) @binding(0) var<uniform> uniforms : Uniforms;

struct VertexOutput {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) uv             : vec2<f32>,
    @location(1) color          : vec4<f32>,
    @location(2) screen_size    : vec2<f32>, // glyph size in screen pixels (for px_range scaling)
}

// The MSDF median function — core of the technique.
// Computes the median of three values without branching.
fn median(r: f32, g: f32, b: f32) -> f32 {
    return max(min(r, g), min(max(r, g), b));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sample all 3 channels of the MSDF
    let msd = textureSample(msdf_atlas, msdf_sampler, in.uv).rgb;

    // Signed distance from the median channel (dimensionless, -0.5 to +0.5)
    let sd = median(msd.r, msd.g, msd.b) - 0.5;

    // Convert to screen-space distance:
    // px_range is the "spread" in texels; scaling by the texel size in screen
    // pixels gives a screen-space width.  This makes the AA band correct at
    // any zoom level — the key benefit of MSDF vs. single-channel SDF.
    let texel_size = 1.0 / uniforms.atlas_size;
    let screen_px_range = uniforms.px_range *
        (in.screen_size.x * texel_size + in.screen_size.y * texel_size) * 0.5;
    let screen_px_range_clamped = max(screen_px_range, 1.0);

    // Anti-aliased coverage
    let alpha = clamp(sd * screen_px_range_clamped + 0.5, 0.0, 1.0) * in.color.a;

    // Premultiplied alpha output
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
