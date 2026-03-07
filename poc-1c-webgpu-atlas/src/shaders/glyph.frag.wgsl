// glyph.frag.wgsl — Fragment shader for atlas-sampled glyph rendering
//
// Samples the R8Unorm atlas texture (single channel: coverage mask).
// Multiplies coverage by the instance color's alpha to produce the final
// pre-multiplied alpha output for compositing on top of the background.
//
// Anti-aliasing: smoothstep on coverage value compensates for the 1-texel
// rasterisation on the CPU side.  Not true MSAA — just soft edge blending.

@group(0) @binding(1) var atlas_texture : texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler : sampler;

struct VertexOutput {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) uv             : vec2<f32>,
    @location(1) color          : vec4<f32>,
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sample the coverage from the R8Unorm atlas (r channel = glyph alpha mask)
    let coverage = textureSample(atlas_texture, atlas_sampler, in.uv).r;

    // Soft edge: smoothstep gives sub-pixel-quality anti-aliasing without MSAA.
    // Thresholds [0.4, 0.6] tuned for 1x canvas rasterization.
    let alpha = smoothstep(0.40, 0.60, coverage) * in.color.a;

    // Premultiplied alpha output for correct compositing over dark backgrounds
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
