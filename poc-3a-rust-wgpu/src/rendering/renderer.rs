//! renderer.rs — GPU pipeline + headless render for PoC 3A.
//!
//! No window/winit: the benchmark renders into an offscreen `wgpu::Texture`
//! sized to a fixed viewport (`VIEWPORT_W` x `VIEWPORT_H`), so this works
//! without a display (CI-safe on machines that DO have a GPU adapter).
//!
//! Frame pipeline, mirroring `poc-1c-webgpu-atlas/src/renderer.js`:
//!   1. `shape_line` — HarfBuzz (rustybuzz) shapes each visible line into glyph
//!      ids + positions (font units, scaled to pixels here).
//!   2. `build_glyph_instances` — for each shaped glyph, fetches (or rasterizes
//!      + uploads on a miss) its atlas slot and appends one `GlyphInstance`.
//!   3. `GlyphRenderer::render_frame` — uploads the instance buffer and encodes
//!      ONE render pass with ONE draw call (instanced quads) sampling the atlas.

use crate::rendering::atlas::GlyphAtlas;

pub(crate) const VIEWPORT_W: u32 = 1440;
pub(crate) const VIEWPORT_H: u32 = 900;
const MAX_GLYPHS_PER_FRAME: usize = 65_536;
const SHADER_SRC: &str = include_str!("shaders/glyph.wgsl");

// ── Headless GPU context ────────────────────────────────────────────────────

/// A headless (no window/surface) `wgpu::Device` + `wgpu::Queue`.
#[derive(Debug)]
pub(crate) struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub backend: wgpu::Backend,
    pub adapter_name: String,
}

impl GpuContext {
    /// Requests a headless GPU device. Returns `None` if no adapter is
    /// available (e.g. a display-less CI runner such as GitHub Actions
    /// `ubuntu-latest`, which typically has neither Vulkan nor DX12). Callers
    /// MUST handle `None` gracefully: `cargo test --workspace` and the
    /// `--bench` CLI path both run there.
    pub(crate) fn new_headless() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))?;
        let info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("poc-3a-headless-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .ok()?;
        Some(Self {
            device,
            queue,
            backend: info.backend,
            adapter_name: info.name,
        })
    }
}

// ── Shaping ──────────────────────────────────────────────────────────────────

/// One shaped glyph: id + pixel-space offsets/advance (HarfBuzz output is in
/// font design units; this scales it by `font_size_px / units_per_em`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShapedGlyph {
    pub glyph_id: u16,
    pub x_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

/// Shapes `text` with HarfBuzz (rustybuzz) against `face`. Left-to-right only;
/// no bidi (out of scope for this PoC, same simplification 3B's skrifa path
/// makes).
pub(crate) fn shape_line(
    face: &rustybuzz::Face,
    text: &str,
    font_size_px: f32,
) -> Vec<ShapedGlyph> {
    if text.is_empty() {
        return Vec::new();
    }
    let scale = font_size_px / face.units_per_em() as f32;

    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.set_direction(rustybuzz::Direction::LeftToRight);
    buffer.guess_segment_properties();
    let shaped = rustybuzz::shape(face, &[], buffer);

    shaped
        .glyph_infos()
        .iter()
        .zip(shaped.glyph_positions())
        .map(|(info, pos)| ShapedGlyph {
            // `glyph_id` is guaranteed <= u16::MAX per rustybuzz's own doc comment.
            #[allow(
                clippy::cast_possible_truncation,
                reason = "rustybuzz::hb_glyph_info_t::glyph_id is documented <= u16::MAX after shaping"
            )]
            glyph_id: info.glyph_id as u16,
            x_advance: pos.x_advance as f32 * scale,
            x_offset: pos.x_offset as f32 * scale,
            y_offset: pos.y_offset as f32 * scale,
        })
        .collect()
}

// ── Instance data ────────────────────────────────────────────────────────────

/// Per-glyph instance data uploaded to the GPU: one instance = one textured
/// quad. Byte layout matches `shaders/glyph.wgsl`'s `GlyphInstance` struct and
/// `poc-1c-webgpu-atlas/src/renderer.js`'s `INSTANCE_BYTE_STRIDE = 40`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GlyphInstance {
    pub screen_pos: [f32; 2],
    /// (u, v, w, h) in atlas PIXELS (the shader divides by `atlas_size`).
    pub atlas_uv: [f32; 4],
    pub color_rgba: u32,
    pub glyph_wh: [f32; 2],
}

impl GlyphInstance {
    /// `usize` for CPU-side buffer sizing; widened to `u64` (never truncates,
    /// `STRIDE` is a small compile-time constant) wherever wgpu wants a byte count.
    pub(crate) const STRIDE: usize = 40;

    fn write_le(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.screen_pos[0].to_le_bytes());
        out.extend_from_slice(&self.screen_pos[1].to_le_bytes());
        out.extend_from_slice(&self.atlas_uv[0].to_le_bytes());
        out.extend_from_slice(&self.atlas_uv[1].to_le_bytes());
        out.extend_from_slice(&self.atlas_uv[2].to_le_bytes());
        out.extend_from_slice(&self.atlas_uv[3].to_le_bytes());
        out.extend_from_slice(&self.color_rgba.to_le_bytes());
        out.extend_from_slice(&self.glyph_wh[0].to_le_bytes());
        out.extend_from_slice(&self.glyph_wh[1].to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // trailing pad, keeps stride at 40.
    }
}

/// Shapes every line and appends one `GlyphInstance` per visible, non-whitespace
/// glyph, rasterizing/uploading atlas misses along the way. `lines[0]`'s
/// baseline is drawn at `line_height` (i.e. one line height below the top of
/// the viewport) so ascenders are not clipped.
#[allow(
    clippy::too_many_arguments,
    reason = "each argument is a distinct GPU/font/layout object a real frame needs; grouping them into a struct \
              would just move the same count into field initialization at every call site"
)]
pub(crate) fn build_glyph_instances(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    ttf_face: &ttf_parser::Face,
    rb_face: &rustybuzz::Face,
    atlas: &mut GlyphAtlas,
    lines: &[&str],
    line_height: f32,
    font_size_px: f32,
    left_margin: f32,
    color_rgba: u32,
) -> Vec<GlyphInstance> {
    let mut instances = Vec::with_capacity(lines.len() * 40); // rough per-line glyph estimate.

    for (row, line) in lines.iter().enumerate() {
        if instances.len() >= MAX_GLYPHS_PER_FRAME {
            break;
        }
        let glyphs = shape_line(rb_face, line, font_size_px);
        let mut pen_x = left_margin;
        #[allow(
            clippy::cast_possible_truncation,
            reason = "row index -> pixel y offset is bounded by the (small) visible-line window"
        )]
        let baseline_y = (row as f32 + 1.0) * line_height;

        for g in glyphs {
            if instances.len() >= MAX_GLYPHS_PER_FRAME {
                break;
            }
            if let Some(slot) = atlas.get_or_insert(
                device,
                queue,
                ttf_face,
                ttf_parser::GlyphId(g.glyph_id),
                font_size_px,
            ) {
                if slot.glyph_w > 0.0 && slot.glyph_h > 0.0 {
                    instances.push(GlyphInstance {
                        screen_pos: [
                            pen_x + g.x_offset + slot.bearing_x,
                            baseline_y + g.y_offset + slot.bearing_y,
                        ],
                        atlas_uv: [
                            slot.atlas_x as f32,
                            slot.atlas_y as f32,
                            slot.glyph_w,
                            slot.glyph_h,
                        ],
                        color_rgba,
                        glyph_wh: [slot.glyph_w, slot.glyph_h],
                    });
                }
            }
            pen_x += g.x_advance;
        }
    }

    instances
}

// ── GPU pipeline + offscreen render target ─────────────────────────────────

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The instanced-quad render pipeline plus an offscreen color target. No
/// window/surface: every frame renders into `color_view`, which a test can
/// optionally read back with `read_color_texture_rgba`.
pub(crate) struct GlyphRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    uniform_buf: wgpu::Buffer,
    instance_buf: wgpu::Buffer,
    color_texture: wgpu::Texture,
    color_view: wgpu::TextureView,
    viewport: (u32, u32),
    atlas_size: u32,
}

impl std::fmt::Debug for GlyphRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlyphRenderer")
            .field("viewport", &self.viewport)
            .field("atlas_size", &self.atlas_size)
            .finish_non_exhaustive()
    }
}

impl GlyphRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        atlas: &GlyphAtlas,
        viewport_w: u32,
        viewport_h: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyph-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyph-scene-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyph-pipeline-layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let attrs =
            wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Uint32, 3 => Float32x2];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: GlyphInstance::STRIDE as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attrs,
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyph-uniforms"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyph-instances"),
            size: (GlyphInstance::STRIDE * MAX_GLYPHS_PER_FRAME) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph-scene-bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(atlas.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let color_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen-color"),
            size: wgpu::Extent3d {
                width: viewport_w,
                height: viewport_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            pipeline,
            bind_group,
            uniform_buf,
            instance_buf,
            color_texture,
            color_view,
            viewport: (viewport_w, viewport_h),
            atlas_size: crate::rendering::atlas::ATLAS_SIZE,
        }
    }

    /// Uploads `instances` and encodes+submits one render pass with one
    /// instanced draw call. Returns the number of glyphs actually drawn
    /// (clamped to `MAX_GLYPHS_PER_FRAME`).
    pub(crate) fn render_frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[GlyphInstance],
    ) -> usize {
        let count = instances.len().min(MAX_GLYPHS_PER_FRAME);

        let (vw, vh) = self.viewport;
        #[allow(
            clippy::cast_precision_loss,
            reason = "viewport dimensions are small (<= a few thousand px), no meaningful precision loss as f32"
        )]
        let uniform_data: [f32; 4] = [vw as f32, vh as f32, self.atlas_size as f32, 0.0];
        queue.write_buffer(&self.uniform_buf, 0, &bytemuck_f32_slice(&uniform_data));

        let mut instance_bytes = Vec::with_capacity(count * GlyphInstance::STRIDE);
        for inst in &instances[..count] {
            inst.write_le(&mut instance_bytes);
        }
        if count > 0 {
            queue.write_buffer(&self.instance_buf, 0, &instance_bytes);
        }

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("glyph-frame-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glyph-frame-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.102,
                            g: 0.106,
                            b: 0.118,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            if count > 0 {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.set_vertex_buffer(0, self.instance_buf.slice(..));
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "count is clamped to MAX_GLYPHS_PER_FRAME (65_536), fits u32"
                )]
                pass.draw(0..6, 0..count as u32);
            }
        }
        queue.submit(std::iter::once(encoder.finish()));

        count
    }

    /// Reads the offscreen color target back to CPU as tightly-packed RGBA8
    /// (row padding required by `COPY_BYTES_PER_ROW_ALIGNMENT` is stripped).
    /// Blocking: only meant for smoke tests, never the timed hot loop.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "viewport/row byte sizes are small (<= a few MB), fit u32/usize on any real target"
    )]
    pub(crate) fn read_color_texture_rgba(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Vec<u8> {
        let (w, h) = self.viewport;
        let unpadded_bpr = w * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bpr = unpadded_bpr.div_ceil(align) * align;

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyph-readback"),
            size: u64::from(padded_bpr) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("glyph-readback-encoder"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.color_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(std::iter::once(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        // `Maintain::Wait` blocks until the GPU work (and pending map callbacks)
        // completes, so `rx.recv()` below never actually blocks on real work.
        device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .expect("map_async callback channel closed unexpectedly")
            .expect("buffer mapping failed");

        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((unpadded_bpr * h) as usize);
        for row in 0..h {
            let start = (row * padded_bpr) as usize;
            out.extend_from_slice(&data[start..start + unpadded_bpr as usize]);
        }
        drop(data);
        buffer.unmap();
        out
    }
}

/// Reinterprets a small `&[f32]` as `&[u8]` for `queue.write_buffer`, without
/// pulling in `bytemuck` for a single 16-byte uniform upload.
fn bytemuck_f32_slice(data: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 4);
    for v in data {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Skips (rather than fails) on a GPU-less machine — see module docs on
    /// `GpuContext::new_headless`. This exact repo's dev machine (Windows) has
    /// a working DX12 adapter, so this test exercises the real GPU path there.
    macro_rules! require_gpu {
        () => {
            match GpuContext::new_headless() {
                Some(ctx) => ctx,
                None => {
                    eprintln!(
                        "skipping GPU test: no wgpu adapter available in this environment \
                         (expected on headless CI runners without Vulkan/DX12/Metal)"
                    );
                    return;
                }
            }
        };
    }

    /// `AHEM` (a standard CSS-testing font, every glyph a simple square/
    /// rectangle) has a real `hhea` table and a full ASCII `cmap`, unlike
    /// `SIMPLE_GLYF` — `ttf_parser::Face::parse` requires `hhea`
    /// (`NoHheaTable` otherwise) and HarfBuzz needs a working cmap to shape.
    fn load_test_font_bytes() -> &'static [u8] {
        font_test_data::AHEM
    }

    #[test]
    fn shape_line_produces_one_glyph_per_char_for_ascii() {
        let data = load_test_font_bytes();
        let ttf_face = ttf_parser::Face::parse(data, 0).expect("valid test font");
        let rb_face = rustybuzz::Face::from_face(ttf_face);
        let glyphs = shape_line(&rb_face, "abc", 16.0);
        assert_eq!(glyphs.len(), 3, "expected one shaped glyph per ASCII char");
        assert!(glyphs.iter().any(|g| g.x_advance > 0.0));
    }

    #[test]
    fn shape_line_empty_string_is_empty() {
        let data = load_test_font_bytes();
        let ttf_face = ttf_parser::Face::parse(data, 0).expect("valid test font");
        let rb_face = rustybuzz::Face::from_face(ttf_face);
        assert!(shape_line(&rb_face, "", 16.0).is_empty());
    }

    #[test]
    fn gpu_context_reports_a_real_backend_on_this_machine() {
        let ctx = require_gpu!();
        println!(
            "GPU adapter: {} (backend={:?})",
            ctx.adapter_name, ctx.backend
        );
        // Just confirms `get_info()` returned something real, not a specific
        // backend: CI machines with a Vulkan-only or Metal adapter are also
        // valid "has a GPU" outcomes, DX12 is only guaranteed on this Windows
        // dev machine (see module docs / task report).
    }

    #[test]
    fn render_frame_smoke_test_draws_nonbackground_pixels() {
        let ctx = require_gpu!();
        let data = load_test_font_bytes();
        let ttf_face = ttf_parser::Face::parse(data, 0).expect("valid test font");
        let rb_face = rustybuzz::Face::from_face(ttf_parser::Face::parse(data, 0).unwrap());

        let mut atlas = GlyphAtlas::new(&ctx.device, crate::rendering::atlas::ATLAS_SIZE);
        let renderer = GlyphRenderer::new(&ctx.device, &atlas, 200, 100);

        let instances = build_glyph_instances(
            &ctx.device,
            &ctx.queue,
            &ttf_face,
            &rb_face,
            &mut atlas,
            &["abc"],
            20.0,
            32.0,
            10.0,
            0xC9D1D9FF,
        );
        assert!(
            !instances.is_empty(),
            "expected at least one glyph instance"
        );

        let drawn = renderer.render_frame(&ctx.device, &ctx.queue, &instances);
        assert_eq!(drawn, instances.len());

        let pixels = renderer.read_color_texture_rgba(&ctx.device, &ctx.queue);
        // Background clear color is (0.102, 0.106, 0.118) -> roughly (26, 27, 30) in u8.
        let background = [26u8, 27, 30];
        let has_non_background = pixels.chunks_exact(4).any(|px| {
            (px[0] as i32 - background[0] as i32).abs() > 8
                || (px[1] as i32 - background[1] as i32).abs() > 8
                || (px[2] as i32 - background[2] as i32).abs() > 8
        });
        assert!(
            has_non_background,
            "expected the rendered frame to contain glyph ink, found only background pixels"
        );
    }
}
