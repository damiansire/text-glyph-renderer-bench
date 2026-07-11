// glyph.wgsl — Instanced glyph-quad pipeline for PoC 3A (Rust + wgpu).
//
// Port of the Web/WebGPU version at `poc-1c-webgpu-atlas/src/shaders/glyph.vert.wgsl`
// and `glyph.frag.wgsl` (same instance layout, same atlas-sampling approach). One
// instance = one glyph, drawn as two triangles (6 vertices, no index buffer) sampling
// a coverage value out of the R8Unorm texture atlas built by `atlas.rs`.
//
// Instance layout (GlyphInstance, 40 bytes, matches `renderer.rs::INSTANCE_STRIDE`):
//   screen_pos:  vec2f — top-left of the glyph quad in screen pixels
//   atlas_uv:    vec4f — (u, v, w, h) in atlas pixels (divided by atlas_size below)
//   color_rgba:  u32   — packed RGBA8 color
//   glyph_wh:    vec2f — glyph quad size in screen pixels
//   (4 bytes of trailing padding, unused, keeps the stride 4-byte aligned at 40)

struct GlyphInstance {
    @location(0) screen_pos : vec2<f32>,
    @location(1) atlas_uv   : vec4<f32>,
    @location(2) color_rgba : u32,
    @location(3) glyph_wh   : vec2<f32>,
}

struct Uniforms {
    viewport_size : vec2<f32>,
    atlas_size    : f32,
    _pad          : f32,
}

@group(0) @binding(0) var<uniform> uniforms : Uniforms;
@group(0) @binding(1) var atlas_texture : texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler : sampler;

struct VertexOutput {
    @builtin(position) clip_pos : vec4<f32>,
    @location(0) uv             : vec2<f32>,
    @location(1) color          : vec4<f32>,
}

@vertex
fn vs_main(
    inst: GlyphInstance,
    @builtin(vertex_index) vid: u32,
) -> VertexOutput {
    // Quad offsets for 6 vertices (2 triangles, CCW winding).
    // Index:  0   1   2   3   4   5
    // Vertex: TL  TR  BL  TR  BR  BL
    //
    // A function-local `var` (not a module-scope `const`, and not a `let`
    // either: this wgpu/naga version's WGSL validator only allows *dynamic*
    // (runtime, e.g. `vid`) indexing into a `var` in the `function` address
    // space, not into a `const`/`let` array of constant literals).
    var quad_offsets = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let offset = quad_offsets[vid];

    // Screen position of this vertex, in pixels.
    let px = inst.screen_pos + offset * inst.glyph_wh;

    // Clip space: map [0, viewport] -> [-1, 1] (Y flipped: +Y is up in clip space).
    let clip = vec2<f32>(
         px.x / uniforms.viewport_size.x * 2.0 - 1.0,
        -px.y / uniforms.viewport_size.y * 2.0 + 1.0,
    );

    // UV: map atlas pixels to [0, 1].
    let atlas_inv = 1.0 / uniforms.atlas_size;
    let uv_origin = vec2<f32>(inst.atlas_uv.x, inst.atlas_uv.y) * atlas_inv;
    let uv_size   = vec2<f32>(inst.atlas_uv.z, inst.atlas_uv.w) * atlas_inv;
    let uv = uv_origin + offset * uv_size;

    // Unpack RGBA8 color.
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

// Samples the R8Unorm atlas texture (single channel: coverage mask) and multiplies
// coverage by the instance color's alpha to produce premultiplied-alpha output.
// `smoothstep` softens the 1-texel CPU rasterization edge; not true MSAA.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas_texture, atlas_sampler, in.uv).r;
    let alpha = smoothstep(0.40, 0.60, coverage) * in.color.a;
    return vec4<f32>(in.color.rgb * alpha, alpha);
}
