// glyph.vert.wgsl — Vertex shader for instanced glyph quads
//
// Each instance represents one glyph drawn as two triangles (quad).
// Instance data comes from a storage buffer: position, UV, color.
//
// Input layout (per instance, in GlyphInstance struct):
//   screen_pos:  vec2f — top-left of the glyph quad in screen pixels
//   atlas_uv:    vec4f — (u, v, w, h) in atlas pixels (divide by atlas_size)
//   color:       u32   — RGBA8 packed color
//   atlas_size:  f32   — atlas texture size in pixels (e.g. 2048.0)

struct GlyphInstance {
    @location(0) screen_pos : vec2<f32>,   // top-left in screen pixels
    @location(1) atlas_uv   : vec4<f32>,   // (u, v, w, h) in atlas pixels
    @location(2) color_rgba : u32,         // packed RGBA8
    @location(3) glyph_wh   : vec2<f32>,   // glyph size in screen pixels
}

struct Uniforms {
    viewport_size : vec2<f32>,
    atlas_size    : f32,
    _pad          : f32,
}

@group(0) @binding(0) var<uniform> uniforms : Uniforms;

struct VertexOutput {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) uv             : vec2<f32>,
    @location(1) color          : vec4<f32>,
}

// Quad offsets for 6 vertices (2 triangles, CCW winding)
// Index:  0   1   2   3   4   5
// Vertex: TL  TR  BL  TR  BR  BL
const QUAD_OFFSETS = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0),   // TL
    vec2<f32>(1.0, 0.0),   // TR
    vec2<f32>(0.0, 1.0),   // BL
    vec2<f32>(1.0, 0.0),   // TR
    vec2<f32>(1.0, 1.0),   // BR
    vec2<f32>(0.0, 1.0),   // BL
);

@vertex
fn vs_main(
    inst: GlyphInstance,
    @builtin(vertex_index) vid: u32,
) -> VertexOutput {
    let offset = QUAD_OFFSETS[vid];

    // Screen position of this vertex (in pixels)
    let px = inst.screen_pos + offset * inst.glyph_wh;

    // Clip space: map [0, viewport] → [-1, 1]  (Y flipped: +Y = up in clip space)
    let clip = vec2<f32>(
         px.x / uniforms.viewport_size.x * 2.0 - 1.0,
        -px.y / uniforms.viewport_size.y * 2.0 + 1.0,
    );

    // UV: map atlas pixels to [0, 1]
    let atlas_inv = 1.0 / uniforms.atlas_size;
    let uv_origin = vec2<f32>(inst.atlas_uv.x, inst.atlas_uv.y) * atlas_inv;
    let uv_size   = vec2<f32>(inst.atlas_uv.z, inst.atlas_uv.w) * atlas_inv;
    let uv = uv_origin + offset * uv_size;

    // Unpack RGBA8 color
    let r = f32((inst.color_rgba >> 24u) & 0xFFu) / 255.0;
    let g = f32((inst.color_rgba >> 16u) & 0xFFu) / 255.0;
    let b = f32((inst.color_rgba >>  8u) & 0xFFu) / 255.0;
    let a = f32((inst.color_rgba       ) & 0xFFu) / 255.0;

    return VertexOutput(
        vec4<f32>(clip, 0.0, 1.0),
        uv,
        vec4<f32>(r, g, b, a),
    );
}
