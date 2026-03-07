// Shared vertex shader — same as PoC 1C (instanced quads)
// The MSDF PoC only replaces the fragment shader; vertex stage is identical.
// This file is a symlink-equivalent: just re-uses the same WGSL logic.
//
// GlyphInstance layout (same as poc-1c):
//   screen_pos: vec2f, atlas_uv: vec4f (u,v,w,h in atlas pixels),
//   color_rgba: u32, glyph_wh: vec2f

struct GlyphInstance {
    @location(0) screen_pos : vec2<f32>,
    @location(1) atlas_uv   : vec4<f32>,
    @location(2) color_rgba : u32,
    @location(3) glyph_wh   : vec2<f32>,
}

struct Uniforms {
    viewport_size : vec2<f32>,
    atlas_size    : f32,
    px_range      : f32,
}

@group(0) @binding(0) var<uniform> uniforms : Uniforms;

struct VertexOutput {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) uv             : vec2<f32>,
    @location(1) color          : vec4<f32>,
    @location(2) screen_size    : vec2<f32>,
}

const QUAD_OFFSETS = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
);

@vertex
fn vs_main(inst: GlyphInstance, @builtin(vertex_index) vid: u32) -> VertexOutput {
    let offset = QUAD_OFFSETS[vid];
    let px = inst.screen_pos + offset * inst.glyph_wh;
    let clip = vec2<f32>(
         px.x / uniforms.viewport_size.x * 2.0 - 1.0,
        -px.y / uniforms.viewport_size.y * 2.0 + 1.0,
    );
    let atlas_inv = 1.0 / uniforms.atlas_size;
    let uv = (inst.atlas_uv.xy + offset * inst.atlas_uv.zw) * atlas_inv;
    let r = f32((inst.color_rgba >> 24u) & 0xFFu) / 255.0;
    let g = f32((inst.color_rgba >> 16u) & 0xFFu) / 255.0;
    let b = f32((inst.color_rgba >>  8u) & 0xFFu) / 255.0;
    let a = f32( inst.color_rgba         & 0xFFu) / 255.0;
    return VertexOutput(
        vec4<f32>(clip, 0.0, 1.0),
        uv,
        vec4<f32>(r, g, b, a),
        inst.glyph_wh,
    );
}
