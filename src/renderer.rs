//! wgpu renderer for the CyberDesk shell.
//!
//! Composites, every frame:
//!   1. the shell background — the Pulse Grid circuit board (`pulsegrid_*.wgsl`),
//!      the sole background since CD-06 (D-0013), and
//!   2. the surf-zone page — the CEF off-screen texture drawn at the zone
//!      rectangle with rounded corners (`page.wgsl`), blended over the shell.
//!
//! The CARVILON ring (`ring.wgsl`, [`RingUniforms`], [`ring_pipeline`]) is kept
//! dormant in the tree — nothing renders it anymore; its motif migrates to the
//! start animation / Energy Core in Season 2 (D-0013).
//!
//! All wgpu work lives on the main thread. CEF's `on_paint` (on the CEF UI
//! thread) only hands over raw BGRA bytes; [`upload_slot`](SurfaceRenderer::upload_slot)
//! copies them into the owning slot's persistent page texture here.
//!
//! The off-screen [`capture`] path renders the full shell background (the Pulse
//! Grid) to a PNG for headless visual self-tests.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use winit::window::Window;

use crate::pulsegrid;
use crate::slots::MAX_SLOTS;

/// Non-sRGB render target so CEF's BGRA bytes and our sRGB brand colors pass
/// through unchanged (matches the cef-rs OSR example).
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

// Dormant since CD-06 (D-0013): the ring no longer renders in the shell or the
// capture path. Kept in the tree for the Season-2 start animation / Energy Core.
#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RingUniforms {
    resolution: [f32; 2],
    time: f32,
    is_srgb: u32,
    bg: [f32; 4],
    brand: [f32; 4],
    geom: [f32; 4],  // radius, stroke, gap_half_rad, rotation_speed
    inner: [f32; 4], // inner_radius, inner_stroke, glow, _pad
}

impl RingUniforms {
    #[allow(dead_code)]
    fn from_theme(theme: &crate::theme::Theme, resolution: [f32; 2], time: f32, is_srgb: u32) -> Self {
        let c = &theme.colors;
        let r = &theme.ring;
        let rgb4 = |v: [f32; 3]| [v[0], v[1], v[2], 1.0];
        Self {
            resolution,
            time,
            is_srgb,
            bg: rgb4(c.background_rgb()),
            brand: rgb4(c.brand_rgb()),
            geom: [
                r.radius,
                r.stroke,
                r.gap_degrees.to_radians(),
                theme.ring_rotation_speed(),
            ],
            inner: [r.inner_radius, r.inner_stroke, r.glow, 0.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PageUniforms {
    rect_ndc: [f32; 4],
    px_size: [f32; 2],
    corner_radius: f32,
    feather: f32,
    feather_exp: f32,
    _pad: [f32; 3], // std140: round the struct up to 48 bytes
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GearUniforms {
    resolution: [f32; 2],
    center: [f32; 2],
    radius: f32,
    hover: f32,
    _pad: [f32; 2],
    brand: [f32; 4],
}

#[allow(dead_code)] // Dormant since CD-06 (D-0013); kept for the Season-2 motif.
fn ring_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::Buffer, wgpu::BindGroup) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ring-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("ring.wgsl").into()),
    });
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ring-uniforms"),
        size: std::mem::size_of::<RingUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ring-bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ring-bg"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ring-pl"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ring-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // Premultiplied OVER: the ring is transparent except the arc, so
                // it composites over the Deep Field. (In the capture path the ring
                // outputs alpha = 1, so OVER reduces to a replace.)
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent::OVER,
                    alpha: wgpu::BlendComponent::OVER,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    (pipeline, uniform_buf, bind_group)
}

/// Shared page-compositing pipeline (`page.wgsl`) — one pipeline, bind-group
/// layouts and sampler, reused by every surf slot and the internal overlay
/// (CD-09). Per-target data (the rect uniform + the CEF texture) lives in
/// [`PageTarget`].
struct PagePipeline {
    pipeline: wgpu::RenderPipeline,
    uniform_bgl: wgpu::BindGroupLayout,
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl PagePipeline {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("page-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("page.wgsl").into()),
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("page-uniform-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("page-tex-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("page-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("page-pl"),
            bind_group_layouts: &[Some(&uniform_bgl), Some(&tex_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("page-pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Premultiplied OVER: page (premultiplied by the shader's
                    // rounded-corner mask) composites over the shell.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent::OVER,
                        alpha: wgpu::BlendComponent::OVER,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, uniform_bgl, tex_bgl, sampler }
    }
}

/// One page-compositing target: a surf slot or the internal overlay. Owns its
/// rect uniform (each slot draws at a different rectangle in the same pass, so
/// the uniform cannot be shared) and its persistent CEF texture, recreated only
/// when the frame size changes.
struct PageTarget {
    uniform_buf: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    texture: Option<wgpu::Texture>,
    tex_bind_group: Option<wgpu::BindGroup>,
    width: u32,
    height: u32,
}

impl PageTarget {
    fn new(device: &wgpu::Device, pipe: &PagePipeline) -> Self {
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("page-uniforms"),
            size: std::mem::size_of::<PageUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("page-uniform-bg"),
            layout: &pipe.uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        Self {
            uniform_buf,
            uniform_bind_group,
            texture: None,
            tex_bind_group: None,
            width: 0,
            height: 0,
        }
    }

    fn has_texture(&self) -> bool {
        self.tex_bind_group.is_some()
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipe: &PagePipeline,
        data: &[u8],
        w: u32,
        h: u32,
    ) {
        if w == 0 || h == 0 || data.len() < (w * h * 4) as usize {
            return;
        }
        if self.texture.is_none() || self.width != w || self.height != h {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("page-texture"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Bgra8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("page-tex-bg"),
                layout: &pipe.tex_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&pipe.sampler),
                    },
                ],
            });
            self.texture = Some(texture);
            self.tex_bind_group = Some(bind_group);
            self.width = w;
            self.height = h;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: self.texture.as_ref().unwrap(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data[..(w * h * 4) as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FieldUniforms {
    resolution: [f32; 2],
    time: f32,
    _pad: f32,
    base: [f32; 4],
    brand: [f32; 4],
    breathing: [f32; 4], // period, amplitude, _, _
    nebula: [f32; 4],    // a_period, b_period, amplitude, _
    dust: [f32; 4],      // amplitude, twinkle_period, _, _
    sweep: [f32; 4],     // period_min, period_max, amplitude, _
}

impl FieldUniforms {
    fn from_theme(theme: &crate::theme::Theme, resolution: [f32; 2], time: f32) -> Self {
        let d = &theme.deep_field;
        let rgb4 = |v: [f32; 3]| [v[0], v[1], v[2], 1.0];
        Self {
            resolution,
            time,
            _pad: 0.0,
            base: rgb4(theme.colors.background_rgb()),
            brand: rgb4(theme.colors.brand_rgb()),
            breathing: [d.breathing_period, d.breathing_amplitude, 0.0, 0.0],
            nebula: [d.nebula_a_period, d.nebula_b_period, d.nebula_amplitude, 0.0],
            dust: [d.dust_amplitude, d.dust_twinkle_period, 0.0, 0.0],
            sweep: [d.sweep_period_min, d.sweep_period_max, d.sweep_amplitude, 0.0],
        }
    }
}

/// The Deep Field background: a procedural field rendered to a half-resolution
/// target (`deepfield.wgsl`) and upscaled into the frame (`blit.wgsl`).
struct DeepField {
    field_pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    composite_pipeline: wgpu::RenderPipeline,
    composite_bgl: wgpu::BindGroupLayout,
    composite_bind_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    target_view: Option<wgpu::TextureView>,
    half_w: u32,
    half_h: u32,
    frame: u64,
    needs_render: bool,
}

impl DeepField {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let field_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("deepfield-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("deepfield.wgsl").into()),
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("field-uniforms"),
            size: std::mem::size_of::<FieldUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("field-uniform-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("field-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let field_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("field-pl"),
            bind_group_layouts: &[Some(&uniform_bgl)],
            immediate_size: 0,
        });
        let field_pipeline = fullscreen_pipeline(device, &field_shader, &field_layout, format, false);

        // Composite (upscale blit).
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        let composite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("field-composite-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("field-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("field-composite-pl"),
            bind_group_layouts: &[Some(&composite_bgl)],
            immediate_size: 0,
        });
        let composite_pipeline =
            fullscreen_pipeline(device, &blit_shader, &composite_layout, format, false);

        Self {
            field_pipeline,
            uniform_buf,
            uniform_bind_group,
            composite_pipeline,
            composite_bgl,
            composite_bind_group: None,
            sampler,
            target_view: None,
            half_w: 0,
            half_h: 0,
            frame: 0,
            needs_render: true,
        }
    }

    fn ensure_target(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        let hw = (w / 2).max(1);
        let hh = (h / 2).max(1);
        if self.target_view.is_none() || self.half_w != hw || self.half_h != hh {
            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("deepfield-target"),
                size: wgpu::Extent3d {
                    width: hw,
                    height: hh,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SURFACE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("field-composite-bg"),
                layout: &self.composite_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.target_view = Some(view);
            self.composite_bind_group = Some(bind_group);
            self.half_w = hw;
            self.half_h = hh;
            self.needs_render = true;
        }
    }
}

/// Build a fullscreen-triangle render pipeline for a shader with `vs_main`/
/// `fs_main`. `blend` selects premultiplied OVER (for overlays that leave the
/// rest of the frame untouched) versus an opaque REPLACE target.
fn fullscreen_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    blend: bool,
) -> wgpu::RenderPipeline {
    let blend_state = if blend {
        wgpu::BlendState {
            color: wgpu::BlendComponent::OVER,
            alpha: wgpu::BlendComponent::OVER,
        }
    } else {
        wgpu::BlendState::REPLACE
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fullscreen-pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend_state),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

// --- Pulse Grid background (CD-05, D-0012) ----------------------------------

/// Full-resolution HDR bake target — thin lines stay crisp and glow above 1.0
/// survives the intensity scaling in the composite (a one-time cost, not
/// per-frame). 16-bit float is core-blendable and can be sampled with
/// `textureLoad`.
const BAKE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Per-instance vertex layout for `pulsegrid_sprite.wgsl` (matches
/// [`pulsegrid::SpriteInstance`]).
const SPRITE_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4];

/// Premultiplied additive blend — overlapping glow accumulates.
fn additive_blend() -> wgpu::BlendState {
    let add = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    };
    wgpu::BlendState { color: add, alpha: add }
}

/// Composite + life-pass globals (mirrors `Globals` in the shaders). The zone
/// rects (Stage C) dim the background under content; `vec4` array aligns to 16
/// bytes (std140), so `_pad` keeps `zones` on a 16-byte boundary.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpriteGlobals {
    base: [f32; 4],
    resolution: [f32; 2],
    glow_intensity: f32,
    zone_shadow: f32,
    zone_feather: f32,
    zone_count: u32,
    _pad: [f32; 2],
    // Up to 8 content rects (x, y, w, h) in physical px: up to MAX_SLOTS slots +
    // 2 side zones + the one open overlay (bar / settings card) = 7 max — CD-11
    // grew this from 6 (CD-09 grew it from 4).
    zones: [[f32; 4]; 8],
}

/// Max content rects the zone-shadow uniform carries (see [`SpriteGlobals`]).
const MAX_ZONES: usize = 8;

/// Micro-lattice uniforms (mirrors `Lattice` in `pulsegrid_lattice.wgsl`). Since
/// CD-06 it carries three depth weaves — each `layers[i]` is `(cell, dot_radius,
/// glow, _)` for far / mid / near.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LatticeUniforms {
    brand: [f32; 4],
    resolution: [f32; 2],
    aa: f32,
    _pad0: f32,
    layers: [[f32; 4]; 3],
}

/// A fullscreen-triangle pipeline with an explicit blend state (the shared
/// `fullscreen_pipeline` only offers OVER / REPLACE).
fn blended_fullscreen_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    blend: wgpu::BlendState,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("pulsegrid-fullscreen-pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// An instanced sprite pipeline (unit quad × instance buffer) for the SDF
/// primitives — additive, targeting `format`.
fn sprite_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("pulsegrid-sprite-pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<pulsegrid::SpriteInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &SPRITE_ATTRS,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(additive_blend()),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// The Pulse Grid background: a seeded circuit board baked once into a full-res
/// HDR texture (micro lattice + traces + pads + solder dots + bus lines) and
/// composited each frame as the backmost layer, scaled by the glow-intensity
/// uniform. Trace polylines stay on the CPU (in `board`) for the Stage B life
/// layer.
struct PulseGrid {
    // Micro-lattice fullscreen pass (baked into `bake_view`).
    lattice_pipeline: wgpu::RenderPipeline,
    lattice_buf: wgpu::Buffer,
    lattice_bg: wgpu::BindGroup,

    // Instanced SDF primitives (traces/pads/dots/bus) baked into `bake_view`.
    sprite_bake_pipeline: wgpu::RenderPipeline,
    bake_globals_buf: wgpu::Buffer,
    bake_globals_bg: wgpu::BindGroup,

    // Live layer: travelling pulses + node flares, drawn over the composite.
    sprite_life_pipeline: wgpu::RenderPipeline,
    life_globals_bg: wgpu::BindGroup,
    life_buf: wgpu::Buffer,
    life_cap: u32,
    life_count: u32,
    sim: Option<pulsegrid::PulseSim>,
    last_time: f32,

    // Composite: read the bake, add over the base, scale by glow intensity.
    composite_pipeline: wgpu::RenderPipeline,
    composite_bgl: wgpu::BindGroupLayout,
    composite_bg: Option<wgpu::BindGroup>,

    // Live globals, shared by the composite (and the Stage B life pass).
    globals_buf: wgpu::Buffer,

    bake_view: Option<wgpu::TextureView>,
    prim_buf: Option<wgpu::Buffer>,
    prim_count: u32,

    // Regeneration guards.
    width: u32,
    height: u32,
    scale: f32,
    seed: u64,
    needs_bake: bool,

    // CPU board model (kept for the life layer).
    board: Option<pulsegrid::Board>,
}

impl PulseGrid {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        // Sprite shader + its globals bind-group layout (VS reads resolution).
        let sprite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pulsegrid-sprite-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pulsegrid_sprite.wgsl").into()),
        });
        let sprite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pulsegrid-sprite-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let sprite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pulsegrid-sprite-pl"),
            bind_group_layouts: &[Some(&sprite_bgl)],
            immediate_size: 0,
        });
        let sprite_bake_pipeline = sprite_pipeline(device, &sprite_shader, &sprite_layout, BAKE_FORMAT);
        // Same shader, but targeting the surface format for the live pass.
        let sprite_life_pipeline = sprite_pipeline(device, &sprite_shader, &sprite_layout, format);

        let bake_globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pulsegrid-bake-globals"),
            size: std::mem::size_of::<SpriteGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bake_globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pulsegrid-bake-globals-bg"),
            layout: &sprite_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: bake_globals_buf.as_entire_binding(),
            }],
        });

        // Live globals buffer (written every frame).
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pulsegrid-globals"),
            size: std::mem::size_of::<SpriteGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let life_globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pulsegrid-life-globals-bg"),
            layout: &sprite_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        // Live instance buffer — a generous fixed cap. Pulses now span three
        // depth layers (each head + trail), but even at ultrawide the total
        // stays well under this.
        let life_cap: u32 = 4096;
        let life_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pulsegrid-life"),
            size: (life_cap as usize * std::mem::size_of::<pulsegrid::SpriteInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Lattice pass.
        let lattice_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pulsegrid-lattice-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pulsegrid_lattice.wgsl").into()),
        });
        let lattice_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pulsegrid-lattice-uniforms"),
            size: std::mem::size_of::<LatticeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lattice_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pulsegrid-lattice-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let lattice_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pulsegrid-lattice-bg"),
            layout: &lattice_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: lattice_buf.as_entire_binding(),
            }],
        });
        let lattice_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pulsegrid-lattice-pl"),
            bind_group_layouts: &[Some(&lattice_bgl)],
            immediate_size: 0,
        });
        let lattice_pipeline =
            blended_fullscreen_pipeline(device, &lattice_shader, &lattice_layout, BAKE_FORMAT, additive_blend());

        // Composite pass (globals uniform + baked texture).
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pulsegrid-composite-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("pulsegrid_composite.wgsl").into()),
        });
        let composite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pulsegrid-composite-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
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
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    },
                    count: None,
                },
            ],
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pulsegrid-composite-pl"),
            bind_group_layouts: &[Some(&composite_bgl)],
            immediate_size: 0,
        });
        // Backmost, opaque: REPLACE (writes base + glow over the whole frame).
        let composite_pipeline =
            fullscreen_pipeline(device, &composite_shader, &composite_layout, format, false);

        Self {
            lattice_pipeline,
            lattice_buf,
            lattice_bg,
            sprite_bake_pipeline,
            bake_globals_buf,
            bake_globals_bg,
            sprite_life_pipeline,
            life_globals_bg,
            life_buf,
            life_cap,
            life_count: 0,
            sim: None,
            last_time: 0.0,
            composite_pipeline,
            composite_bgl,
            composite_bg: None,
            globals_buf,
            bake_view: None,
            prim_buf: None,
            prim_count: 0,
            width: 0,
            height: 0,
            scale: 1.0,
            seed: 0,
            needs_bake: false,
            board: None,
        }
    }

    /// (Re)generate the board and bake resources when the frame size, DPI scale
    /// or seed changes, then write the live globals for this frame. Returns
    /// whether a bake pass must run (consumed by [`SurfaceRenderer::render`]).
    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        w: u32,
        h: u32,
        scale: f32,
        theme: &crate::theme::Theme,
        base: [f32; 3],
        glow_intensity: f32,
        zones: &[[f32; 4]],
    ) -> bool {
        let cfg = &theme.background;
        let dirty = self.bake_view.is_none()
            || self.width != w
            || self.height != h
            || self.scale != scale
            || self.seed != cfg.seed;

        if dirty {
            let t0 = std::time::Instant::now();

            // New full-res HDR bake target.
            let bake = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("pulsegrid-bake"),
                size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: BAKE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let bake_view = bake.create_view(&wgpu::TextureViewDescriptor::default());

            // Deterministic board generation.
            let brand = theme.colors.brand_rgb();
            let board = pulsegrid::generate(w.max(1), h.max(1), scale, cfg, brand);
            self.prim_count = board.prims.len() as u32;

            let prim_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pulsegrid-prims"),
                size: (board.prims.len().max(1) * std::mem::size_of::<pulsegrid::SpriteInstance>())
                    as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if !board.prims.is_empty() {
                queue.write_buffer(&prim_buf, 0, bytemuck::cast_slice(&board.prims));
            }

            // Lattice + bake globals (static until the next regeneration). Three
            // depth weaves: near (front, brightest) plus dimmer, finer mid/far.
            let near_cell = (cfg.lattice_cell * scale).max(4.0);
            let mid_cell = (cfg.lattice_cell * scale * cfg.mid_cell_scale).max(3.0);
            let far_cell = (cfg.lattice_cell * scale * cfg.far_cell_scale).max(3.0);
            let dot = cfg.lattice_dot * scale;
            let g = cfg.lattice_glow;
            let lattice = LatticeUniforms {
                brand: [brand[0], brand[1], brand[2], 1.0],
                resolution: [w as f32, h as f32],
                aa: 0.9,
                _pad0: 0.0,
                layers: [
                    [far_cell, dot, g * cfg.far_bright, 0.0],
                    [mid_cell, dot, g * cfg.mid_bright, 0.0],
                    [near_cell, dot, g, 0.0],
                ],
            };
            queue.write_buffer(&self.lattice_buf, 0, bytemuck::bytes_of(&lattice));

            let bake_globals = SpriteGlobals {
                base: [base[0], base[1], base[2], 1.0],
                resolution: [w as f32, h as f32],
                glow_intensity: 1.0, // bake stores raw glow; composite re-applies intensity
                zone_shadow: 1.0,    // no zone shadow in the bake (it is raw glow)
                zone_feather: 0.0,
                zone_count: 0,
                _pad: [0.0, 0.0],
                zones: [[0.0; 4]; MAX_ZONES],
            };
            queue.write_buffer(&self.bake_globals_buf, 0, bytemuck::bytes_of(&bake_globals));

            // Composite bind group (globals + the fresh bake view).
            self.composite_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("pulsegrid-composite-bg"),
                layout: &self.composite_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.globals_buf.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&bake_view),
                    },
                ],
            }));

            // Fresh life simulation for the new board (pulse count scales with
            // width; positions come from the regenerated polylines).
            self.sim = Some(pulsegrid::PulseSim::new(
                &board,
                &cfg.pulse,
                brand,
                w as f32,
                scale,
                cfg.seed,
            ));

            self.bake_view = Some(bake_view);
            self.prim_buf = Some(prim_buf);
            self.width = w;
            self.height = h;
            self.scale = scale;
            self.seed = cfg.seed;
            self.needs_bake = true;
            self.board = Some(board);

            eprintln!(
                "[pulsegrid] baked {} primitives for {}x{} in {:.1} ms",
                self.prim_count,
                w,
                h,
                t0.elapsed().as_secs_f32() * 1000.0
            );
        }

        // Live globals for this frame (including the content rects that dim the
        // background beneath them — the zone shadow).
        let mut zarr = [[0.0f32; 4]; MAX_ZONES];
        let zc = zones.len().min(MAX_ZONES);
        zarr[..zc].copy_from_slice(&zones[..zc]);
        let globals = SpriteGlobals {
            base: [base[0], base[1], base[2], 1.0],
            resolution: [w as f32, h as f32],
            glow_intensity,
            zone_shadow: cfg.zone_shadow,
            zone_feather: (cfg.zone_feather * scale).max(1.0),
            zone_count: zc as u32,
            _pad: [0.0, 0.0],
            zones: zarr,
        };
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        self.needs_bake
    }

    /// Record the one-time bake render pass (lattice then primitives) into the
    /// HDR bake target. Cleared to transparent so the additive draws accumulate.
    fn record_bake(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(view) = self.bake_view.as_ref() else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pulsegrid-bake-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.lattice_pipeline);
        pass.set_bind_group(0, &self.lattice_bg, &[]);
        pass.draw(0..3, 0..1);
        if let (Some(prim_buf), count) = (self.prim_buf.as_ref(), self.prim_count)
            && count > 0
        {
            pass.set_pipeline(&self.sprite_bake_pipeline);
            pass.set_bind_group(0, &self.bake_globals_bg, &[]);
            pass.set_vertex_buffer(0, prim_buf.slice(..));
            pass.draw(0..6, 0..count);
        }
    }

    /// Draw the baked circuit as the backmost layer of the shell pass.
    fn composite<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if let Some(bg) = self.composite_bg.as_ref() {
            pass.set_pipeline(&self.composite_pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Step the life simulation by the frame delta and upload the pulse/flare
    /// sprites. Must run before the frame encoder (queues a buffer write).
    fn update_life(&mut self, queue: &wgpu::Queue, time: f32, theme: &crate::theme::Theme) {
        // Clamp the delta so a stall (or the first frame) can't fling pulses
        // across the board.
        let dt = (time - self.last_time).clamp(0.0, 0.05);
        self.last_time = time;

        self.life_count = 0;
        if let (Some(sim), Some(board)) = (self.sim.as_mut(), self.board.as_ref()) {
            let insts = sim.step(board, &theme.background.pulse, dt);
            let n = insts.len().min(self.life_cap as usize);
            if n > 0 {
                queue.write_buffer(&self.life_buf, 0, bytemuck::cast_slice(&insts[..n]));
            }
            self.life_count = n as u32;
        }
    }

    /// Draw the live pulses + flares (additive, over the composite).
    fn draw_life<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.life_count > 0 {
            pass.set_pipeline(&self.sprite_life_pipeline);
            pass.set_bind_group(0, &self.life_globals_bg, &[]);
            pass.set_vertex_buffer(0, self.life_buf.slice(..));
            pass.draw(0..6, 0..self.life_count);
        }
    }
}

/// The settings gear button: a small cog drawn over everything (`gear.wgsl`).
struct Gear {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Gear {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gear-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gear.wgsl").into()),
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gear-uniforms"),
            size: std::mem::size_of::<GearUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gear-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gear-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gear-pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = fullscreen_pipeline(device, &shader, &layout, format, true);
        Self { pipeline, uniform_buf, bind_group }
    }
}

// --- Per-slot decorations (CD-09): placeholders + slot lines ----------------

/// One empty-slot placeholder instance (`slot_placeholder.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PlaceholderInstance {
    rect: [f32; 4],  // x, y, w, h (device px)
    fill: [f32; 4],  // fill rgb + corner_radius (a)
    glyph: [f32; 4], // glyph rgb + digit 1..4 (a)
    dot: [f32; 4],   // pending-dot rgb + present flag (a: 0 = none, 1 = shown)
}

/// One slot-line instance (`slot_lines.wgsl`) — the loading line + active accent.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SlotLineInstance {
    rect: [f32; 4],   // x, y, w, h (device px)
    params: [f32; 4], // loading_intensity, active(0/1), accent_th_px, loading_th_px
}

/// Placeholder globals — just the resolution (for the px→NDC vertex transform).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PlaceholderGlobals {
    resolution: [f32; 2],
    _pad: [f32; 2],
}

/// Slot-line globals — resolution, time (for the loading sweep) and brand color.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SlotLineGlobals {
    resolution: [f32; 2],
    time: f32,
    _pad: f32,
    brand: [f32; 4],
}

const SLOTLINE_ATTRS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
const PLACEHOLDER_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

/// A unit-quad, instance-stepped pipeline (premultiplied OVER) for the slot
/// decorations — one draw covers every slot.
fn instanced_over_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    stride: u64,
    attrs: &[wgpu::VertexAttribute],
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("slot-instanced-pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: stride,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: attrs,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent::OVER,
                    alpha: wgpu::BlendComponent::OVER,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Build a single-uniform globals bind group + layout (binding 0, both stages).
fn globals_bind_group(
    device: &wgpu::Device,
    label: &str,
    size: u64,
) -> (wgpu::Buffer, wgpu::BindGroupLayout, wgpu::BindGroup) {
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    });
    (buf, bgl, bg)
}

/// The lazy-slot placeholder pass (`slot_placeholder.wgsl`) — instanced fills +
/// index glyphs for slots with no browser yet.
struct SlotPlaceholder {
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    instance_buf: wgpu::Buffer,
}

impl SlotPlaceholder {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, cap: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("slot-placeholder-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("slot_placeholder.wgsl").into()),
        });
        let (globals_buf, bgl, globals_bg) = globals_bind_group(
            device,
            "slot-placeholder-globals",
            std::mem::size_of::<PlaceholderGlobals>() as u64,
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("slot-placeholder-pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = instanced_over_pipeline(
            device,
            &shader,
            &layout,
            format,
            std::mem::size_of::<PlaceholderInstance>() as u64,
            &PLACEHOLDER_ATTRS,
        );
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slot-placeholder-instances"),
            size: (cap as usize * std::mem::size_of::<PlaceholderInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, globals_buf, globals_bg, instance_buf }
    }
}

/// The per-slot line pass (`slot_lines.wgsl`) — instanced loading lines + the
/// active accent for every slot.
struct SlotLines {
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    instance_buf: wgpu::Buffer,
}

impl SlotLines {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, cap: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("slot-lines-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("slot_lines.wgsl").into()),
        });
        let (globals_buf, bgl, globals_bg) = globals_bind_group(
            device,
            "slot-lines-globals",
            std::mem::size_of::<SlotLineGlobals>() as u64,
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("slot-lines-pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = instanced_over_pipeline(
            device,
            &shader,
            &layout,
            format,
            std::mem::size_of::<SlotLineInstance>() as u64,
            &SLOTLINE_ATTRS,
        );
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("slot-lines-instances"),
            size: (cap as usize * std::mem::size_of::<SlotLineInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, globals_buf, globals_bg, instance_buf }
    }
}

// --- Drag overlay (CD-12): favorite-drag ghost + gutter drop zones ----------

/// One soft-glowing rounded rect for the drag overlay (`drag.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DragInstance {
    rect: [f32; 4],  // x, y, w, h (device px)
    color: [f32; 4], // rgb + alpha
    shape: [f32; 4], // corner_radius, glow_softness, _, _
}

const DRAG_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];

/// A command-overlay quad (CD-12): the topmost transparent pass shared by the
/// favorite-drag visuals (drag ghost, drop-zone gutter bars, slot highlight) and
/// the per-slot close orbs. `kind` selects the fragment shape — `0` a filled soft
/// rounded rect (a circle when `radius` = half), `1` a close orb (ring + cross).
/// Premultiplied OVER.
pub struct DragQuad {
    pub rect: (f32, f32, f32, f32),
    pub color: [f32; 4],
    pub radius: f32,
    pub glow: f32,
    pub kind: u32,
}

/// The drag overlay pass — instanced soft rounded rects, drawn topmost.
struct DragOverlay {
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    instance_buf: wgpu::Buffer,
}

impl DragOverlay {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, cap: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("drag-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("drag.wgsl").into()),
        });
        let (globals_buf, bgl, globals_bg) = globals_bind_group(
            device,
            "drag-globals",
            std::mem::size_of::<PlaceholderGlobals>() as u64,
        );
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("drag-pl"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = instanced_over_pipeline(
            device,
            &shader,
            &layout,
            format,
            std::mem::size_of::<DragInstance>() as u64,
            &DRAG_ATTRS,
        );
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("drag-instances"),
            size: (cap as usize * std::mem::size_of::<DragInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { pipeline, globals_buf, globals_bg, instance_buf }
    }
}

/// One slot to draw this frame (CD-09): its rect (device px), loading intensity,
/// whether it is the active slot, and its 0-based index (→ its page target and
/// its placeholder glyph digit `index + 1`). `pending` (CD-10) is the scheme
/// color of a restored-but-not-yet-spawned slot's armed URL, drawn as a small dot
/// on the placeholder so it reads as "a page is waiting here"; `None` otherwise.
pub struct SlotView {
    pub rect: (f32, f32, f32, f32),
    pub loading: f32,
    pub active: bool,
    pub index: usize,
    pub pending: Option<[f32; 3]>,
}

/// Renders the shell + N surf-slot pages to a winit window surface.
pub struct SurfaceRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    page_pipeline: PagePipeline,
    slots: Vec<PageTarget>,
    panel: PageTarget,
    placeholder: SlotPlaceholder,
    slotlines: SlotLines,
    drag: DragOverlay,
    gear: Gear,
    field: DeepField,
    pulse: PulseGrid,
    theme: crate::theme::Theme,
}

impl SurfaceRenderer {
    pub fn new(window: Arc<Window>, theme: crate::theme::Theme) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window)
            .expect("failed to create render surface");

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .expect("no compatible GPU adapter found");

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cyberdesk-device"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        }))
        .expect("failed to create GPU device");

        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface is not supported by the adapter");
        config.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        config.format = SURFACE_FORMAT;
        config.view_formats = vec![SURFACE_FORMAT];
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let page_pipeline = PagePipeline::new(&device, SURFACE_FORMAT);
        // One page target per possible slot (created up front; lazy slots simply
        // never receive a texture and draw the placeholder instead).
        let slots = (0..MAX_SLOTS)
            .map(|_| PageTarget::new(&device, &page_pipeline))
            .collect();
        let panel = PageTarget::new(&device, &page_pipeline);
        // Placeholder instances: up to MAX_SLOTS empty slots + 2 side zones.
        let placeholder = SlotPlaceholder::new(&device, SURFACE_FORMAT, MAX_SLOTS as u32 + 2);
        let slotlines = SlotLines::new(&device, SURFACE_FORMAT, MAX_SLOTS as u32);
        // Command overlay (CD-12): the drag visuals (up to MAX_SLOTS+1 gutter drop
        // zones + ghost + slot highlight) OR the per-slot close orbs (2 instances
        // each: backing disc + ring/cross). Sized for the larger, close orbs.
        let drag = DragOverlay::new(&device, SURFACE_FORMAT, MAX_SLOTS as u32 * 2 + 4);
        let gear = Gear::new(&device, SURFACE_FORMAT);
        let field = DeepField::new(&device, SURFACE_FORMAT);
        let pulse = PulseGrid::new(&device, SURFACE_FORMAT);

        Self {
            surface,
            device,
            queue,
            config,
            page_pipeline,
            slots,
            panel,
            placeholder,
            slotlines,
            drag,
            gear,
            field,
            pulse,
            theme,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Upload a freshly painted CEF frame (BGRA) into slot `i`'s texture.
    pub fn upload_slot(&mut self, i: usize, data: &[u8], w: u32, h: u32) {
        if let Some(slot) = self.slots.get_mut(i) {
            slot.upload(&self.device, &self.queue, &self.page_pipeline, data, w, h);
        }
    }

    /// Upload a freshly painted internal-view frame (BGRA) into the panel texture.
    pub fn upload_panel(&mut self, data: &[u8], w: u32, h: u32) {
        self.panel
            .upload(&self.device, &self.queue, &self.page_pipeline, data, w, h);
    }

    /// Drop slot `i`'s page texture so a closed/re-lazy slot shows the placeholder
    /// again instead of a stale page (CD-09 Ctrl+W / resize shrink).
    pub fn clear_slot(&mut self, i: usize) {
        if let Some(slot) = self.slots.get_mut(i) {
            slot.texture = None;
            slot.tex_bind_group = None;
            slot.width = 0;
            slot.height = 0;
        }
    }

    /// Render one frame. Rects are in device pixels. `slots` are the surf
    /// columns (each with its rect, loading intensity and active flag); `panel`
    /// is the internal overlay (settings card / top bar); `gear` is the settings
    /// button (center_x, center_y, radius). `feather`/`background_on` are the
    /// live toggles; `glow_intensity` scales the Pulse Grid brightness; `scale`
    /// is the DPI factor (Pulse Grid sizes are logical px). `overlay_open` shows
    /// the panel; `gear_hover` (0..1) drives the gear glow.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        time: f32,
        slots: &[SlotView],
        sides: &[(f32, f32, f32, f32)],
        drag: &[DragQuad],
        panel: (f32, f32, f32, f32),
        gear: (f32, f32, f32),
        feather: bool,
        background_on: bool,
        glow_intensity: f32,
        scale: f32,
        overlay_open: bool,
        is_bar: bool,
        bar_progress: f32,
        gear_hover: f32,
    ) {
        let (cfg_w, cfg_h) = (self.config.width, self.config.height);
        let (win_w, win_h) = (cfg_w as f32, cfg_h as f32);
        let base = self.theme.colors.background_rgb();
        let brand = self.theme.colors.brand_rgb();

        // Background selection is a template token (D-0012): Pulse Grid (Cyber
        // default) or the Deep Field (Calm). The "Animated background" toggle
        // (`background_on`) gates whichever the template picked.
        let use_pulse = self.theme.background.is_pulse_grid();
        let do_pulse = background_on && use_pulse;
        let do_deep = background_on && !use_pulse;

        // Pulse Grid: (re)generate + bake on size/scale/seed change, write the
        // live globals. Must run before the frame encoder (creates GPU
        // resources + queues writes).
        // Zone rects that dim the background: every slot rect, both side zones,
        // plus the internal overlay while it is open. Up to MAX_ZONES (8) carried.
        let mut zones: Vec<[f32; 4]> = Vec::with_capacity(slots.len() + sides.len() + 1);
        for s in slots {
            zones.push([s.rect.0, s.rect.1, s.rect.2, s.rect.3]);
        }
        for s in sides {
            zones.push([s.0, s.1, s.2, s.3]);
        }
        // The settings card dims its full rect (opaque). The CD-12 command band
        // is transparent — only its floating pills paint — so it casts NO zone
        // shadow (dimming the whole band would darken the Pulse Grid across the
        // top even where nothing shows).
        if overlay_open && !is_bar {
            zones.push([panel.0, panel.1, panel.2, panel.3]);
        }
        let _ = bar_progress;

        let pulse_bake = if do_pulse {
            let bake = self.pulse.prepare(
                &self.device,
                &self.queue,
                self.config.width,
                self.config.height,
                scale,
                &self.theme,
                base,
                glow_intensity,
                &zones,
            );
            // Advance the pulses/flares and upload this frame's sprites.
            self.pulse.update_life(&self.queue, time, &self.theme);
            bake
        } else {
            false
        };
        let corner_radius = self.theme.page.corner_radius;
        // Feathering on -> soft SDF falloff of `feather_width` px; off -> 0.0,
        // which the page shader reads as the CD-02 hard rounded edge.
        let feather_px = if feather {
            self.theme.page.feather_width
        } else {
            0.0
        };
        let feather_exp = self.theme.page.feather_exp;
        let to_ndc_x = |x: f32| (x / win_w) * 2.0 - 1.0;
        let to_ndc_y = |y: f32| 1.0 - (y / win_h) * 2.0;

        // Per-slot page uniforms (for painted slots) and the placeholder / line
        // instance lists (built once, drawn instanced). A slot without a texture
        // yet draws the placeholder; every slot gets a line instance (loading +
        // active accent).
        let st = &self.theme.slots;
        let fill_rgb = [
            base[0] + st.placeholder_fill,
            base[1] + st.placeholder_fill,
            base[2] + st.placeholder_fill,
        ];
        let glyph_rgb = [
            brand[0] * st.placeholder_glyph,
            brand[1] * st.placeholder_glyph,
            brand[2] * st.placeholder_glyph,
        ];
        let accent_th = (st.active_line * scale).max(1.0);
        let loading_th = (2.5 * scale).max(1.0);
        let mut placeholders: Vec<PlaceholderInstance> = Vec::with_capacity(slots.len() + sides.len());
        let mut lines: Vec<SlotLineInstance> = Vec::with_capacity(slots.len());
        // Side zones (CD-11): the placeholder family with the diamond glyph
        // (digit 0) — subtle fill, thin outline, a small centered core glyph. No
        // page, no loading/active lines; content arrives in later seasons.
        for s in sides {
            placeholders.push(PlaceholderInstance {
                rect: [s.0, s.1, s.2, s.3],
                fill: [fill_rgb[0], fill_rgb[1], fill_rgb[2], corner_radius],
                glyph: [glyph_rgb[0], glyph_rgb[1], glyph_rgb[2], 0.0],
                dot: [0.0, 0.0, 0.0, 0.0],
            });
        }
        for s in slots {
            let (x, y, w, h) = s.rect;
            if let Some(target) = self.slots.get(s.index) {
                if target.has_texture() {
                    let u = PageUniforms {
                        rect_ndc: [to_ndc_x(x), to_ndc_y(y), to_ndc_x(x + w), to_ndc_y(y + h)],
                        px_size: [target.width.max(1) as f32, target.height.max(1) as f32],
                        corner_radius,
                        feather: feather_px,
                        feather_exp,
                        _pad: [0.0; 3],
                    };
                    self.queue
                        .write_buffer(&target.uniform_buf, 0, bytemuck::bytes_of(&u));
                } else {
                    let dot = match s.pending {
                        Some(c) => [c[0], c[1], c[2], 1.0],
                        None => [0.0, 0.0, 0.0, 0.0],
                    };
                    placeholders.push(PlaceholderInstance {
                        rect: [x, y, w, h],
                        fill: [fill_rgb[0], fill_rgb[1], fill_rgb[2], corner_radius],
                        glyph: [glyph_rgb[0], glyph_rgb[1], glyph_rgb[2], (s.index + 1) as f32],
                        dot,
                    });
                }
            }
            lines.push(SlotLineInstance {
                rect: [x, y, w, h],
                params: [
                    s.loading.clamp(0.0, 1.0),
                    if s.active { 1.0 } else { 0.0 },
                    accent_th,
                    loading_th,
                ],
            });
        }
        let placeholder_count = (placeholders.len() as u32).min(MAX_SLOTS as u32 + 2);
        let line_count = (lines.len() as u32).min(MAX_SLOTS as u32);
        if placeholder_count > 0 {
            let pg = PlaceholderGlobals { resolution: [win_w, win_h], _pad: [0.0, 0.0] };
            self.queue
                .write_buffer(&self.placeholder.globals_buf, 0, bytemuck::bytes_of(&pg));
            self.queue.write_buffer(
                &self.placeholder.instance_buf,
                0,
                bytemuck::cast_slice(&placeholders[..placeholder_count as usize]),
            );
        }
        if line_count > 0 {
            let lg = SlotLineGlobals {
                resolution: [win_w, win_h],
                time,
                _pad: 0.0,
                brand: [brand[0], brand[1], brand[2], 1.0],
            };
            self.queue
                .write_buffer(&self.slotlines.globals_buf, 0, bytemuck::bytes_of(&lg));
            self.queue.write_buffer(
                &self.slotlines.instance_buf,
                0,
                bytemuck::cast_slice(&lines[..line_count as usize]),
            );
        }

        // Drag overlay instances (CD-12): the drag ghost + gutter drop zones +
        // slot highlight, drawn topmost.
        let drag_insts: Vec<DragInstance> = drag
            .iter()
            .take(MAX_SLOTS * 2 + 4)
            .map(|q| DragInstance {
                rect: [q.rect.0, q.rect.1, q.rect.2, q.rect.3],
                color: q.color,
                shape: [q.radius, q.glow, q.kind as f32, 0.0],
            })
            .collect();
        let drag_count = drag_insts.len() as u32;
        if drag_count > 0 {
            let dg = PlaceholderGlobals { resolution: [win_w, win_h], _pad: [0.0, 0.0] };
            self.queue
                .write_buffer(&self.drag.globals_buf, 0, bytemuck::bytes_of(&dg));
            self.queue
                .write_buffer(&self.drag.instance_buf, 0, bytemuck::cast_slice(&drag_insts));
        }

        // Panel uniforms: the internal overlay (settings card or command bar) —
        // crisp rounded corners, never feathered. Only written/drawn while open.
        if overlay_open {
            let (px, py, pw, ph) = panel;
            // The overlay always uses the hard edge (feather = 0). The settings
            // card keeps rounded corners; the top bar is square (corner_radius 0)
            // — a clean strip flush to the top edge, clipped by the scissor as it
            // slides. The exponent is inert here; carried for a complete uniform.
            let panel_u = PageUniforms {
                rect_ndc: [
                    to_ndc_x(px),
                    to_ndc_y(py),
                    to_ndc_x(px + pw),
                    to_ndc_y(py + ph),
                ],
                px_size: [self.panel.width.max(1) as f32, self.panel.height.max(1) as f32],
                corner_radius: if is_bar { 0.0 } else { corner_radius },
                feather: 0.0,
                feather_exp,
                _pad: [0.0; 3],
            };
            self.queue
                .write_buffer(&self.panel.uniform_buf, 0, bytemuck::bytes_of(&panel_u));
        }

        // Gear button uniforms (always drawn, brand-colored, hover-lit).
        let (gcx, gcy, gr) = gear;
        let gear_u = GearUniforms {
            resolution: [win_w, win_h],
            center: [gcx, gcy],
            radius: gr,
            hover: gear_hover.clamp(0.0, 1.0),
            _pad: [0.0, 0.0],
            brand: [brand[0], brand[1], brand[2], 1.0],
        };
        self.queue
            .write_buffer(&self.gear.uniform_buf, 0, bytemuck::bytes_of(&gear_u));

        // Deep Field: repaint the half-res target at ~30 fps (every other frame,
        // or right after a resize).
        self.field.frame = self.field.frame.wrapping_add(1);
        let do_field = do_deep && {
            self.field
                .ensure_target(&self.device, self.config.width, self.config.height);
            self.field.frame.is_multiple_of(2) || self.field.needs_render
        };
        if do_field {
            let fu = FieldUniforms::from_theme(
                &self.theme,
                [self.field.half_w as f32, self.field.half_h as f32],
                time,
            );
            self.queue
                .write_buffer(&self.field.uniform_buf, 0, bytemuck::bytes_of(&fu));
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-encoder"),
            });

        // Pass 1: render the Deep Field into its half-res target.
        if do_field {
            if let Some(target) = self.field.target_view.as_ref() {
                let mut fp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("deepfield-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                fp.set_pipeline(&self.field.field_pipeline);
                fp.set_bind_group(0, &self.field.uniform_bind_group, &[]);
                fp.draw(0..3, 0..1);
            }
            self.field.needs_render = false;
        }

        // Pass 1b: bake the Pulse Grid static layer (only on first frame / after
        // a resize or seed change — otherwise the baked texture is reused).
        if do_pulse && pulse_bake {
            self.pulse.record_bake(&mut encoder);
            self.pulse.needs_bake = false;
        }

        // Pass 2: shell composite — background, then ring, then page.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shell-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: base[0] as f64,
                            g: base[1] as f64,
                            b: base[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // Backmost and sole background (CD-06, D-0013): the Pulse Grid alone —
            // the ring no longer renders in the shell. Pulse Grid composites its
            // baked circuit (scaled by glow intensity); the Deep Field upscales
            // its half-res target. Either is the Cyber/Calm template choice.
            if do_pulse {
                self.pulse.composite(&mut pass);
                // Life layer (pulses + flares) over the baked circuit.
                self.pulse.draw_life(&mut pass);
            } else if do_deep
                && let Some(bg) = self.field.composite_bind_group.as_ref()
            {
                pass.set_pipeline(&self.field.composite_pipeline);
                pass.set_bind_group(0, bg, &[]);
                pass.draw(0..3, 0..1);
            }

            // Surf-slot pages: each slot that has a painted frame, at its rect.
            pass.set_pipeline(&self.page_pipeline.pipeline);
            for s in slots {
                if let Some(target) = self.slots.get(s.index)
                    && let Some(tex_bind_group) = target.tex_bind_group.as_ref()
                {
                    pass.set_bind_group(0, &target.uniform_bind_group, &[]);
                    pass.set_bind_group(1, tex_bind_group, &[]);
                    pass.draw(0..6, 0..1);
                }
            }

            // Lazy-slot placeholders (fill + index glyph), for slots with no
            // texture yet — one instanced draw.
            if placeholder_count > 0 {
                pass.set_pipeline(&self.placeholder.pipeline);
                pass.set_bind_group(0, &self.placeholder.globals_bg, &[]);
                pass.set_vertex_buffer(0, self.placeholder.instance_buf.slice(..));
                pass.draw(0..6, 0..placeholder_count);
            }

            // Per-slot loading lines (top edge) + active accent (bottom edge) —
            // one instanced draw over all slots.
            if line_count > 0 {
                pass.set_pipeline(&self.slotlines.pipeline);
                pass.set_bind_group(0, &self.slotlines.globals_bg, &[]);
                pass.set_vertex_buffer(0, self.slotlines.instance_buf.slice(..));
                pass.draw(0..6, 0..line_count);
            }

            // Internal overlay, over the pages. The CD-12 command band is a full
            // transparent top strip (corner_radius 0) — only its pills paint, so
            // it composites directly with no scissor slide (the page fades each
            // ensemble in CSS). The settings card draws its rounded opaque rect.
            if overlay_open
                && let Some(tex_bind_group) = self.panel.tex_bind_group.as_ref()
            {
                pass.set_pipeline(&self.page_pipeline.pipeline);
                pass.set_bind_group(0, &self.panel.uniform_bind_group, &[]);
                pass.set_bind_group(1, tex_bind_group, &[]);
                pass.draw(0..6, 0..1);
            }

            // Gear button, over the pages / overlay.
            pass.set_pipeline(&self.gear.pipeline);
            pass.set_bind_group(0, &self.gear.bind_group, &[]);
            pass.draw(0..3, 0..1);

            // Drag overlay (CD-12): ghost + drop zones, topmost of all.
            if drag_count > 0 {
                pass.set_pipeline(&self.drag.pipeline);
                pass.set_bind_group(0, &self.drag.globals_bg, &[]);
                pass.set_vertex_buffer(0, self.drag.instance_buf.slice(..));
                pass.draw(0..6, 0..drag_count);
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame);
    }
}

/// Render a single shell frame off-screen to a PNG (headless self-test: the
/// Pulse Grid background alone, no CEF surf zone and — since CD-06 — no ring).
/// Because the background shaders write token colors directly to a non-sRGB
/// target — exactly as the on-screen `Bgra8Unorm` path does — the PNG shows the
/// circuit as it appears fullscreen, which is the sanctioned way to eyeball it
/// without screen-scraping the desktop.
pub fn capture(path: &str, width: u32, height: u32, theme: &crate::theme::Theme) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .expect("no GPU adapter found");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("cyberdesk-capture-device"),
        required_features: wgpu::Features::empty(),
        required_limits: adapter.limits(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
    }))
    .expect("failed to create GPU device");

    let format = wgpu::TextureFormat::Rgba8Unorm;

    // Pulse Grid background (skipped when the template selects the Deep Field —
    // that path is surface-bound and not wired into the headless capture).
    let base = theme.colors.background_rgb();
    let brand = theme.colors.brand_rgb();
    let do_pulse = theme.background.is_pulse_grid();

    // Frame layout for the capture (CD-11): N placeholder columns flanked by the
    // side zones, so the frame — side zones, columns, gutters, glowing margins,
    // zone shadow, and the retreat-to-rails at four slots — can be eyeballed
    // headlessly. `CYBERDESK_CAPTURE_SLOTS=N` (default 1), clamped to the frame
    // capacity; `CYBERDESK_CAPTURE_UNITS=2,1,...` overrides it with an explicit
    // per-slot width-unit sequence (CD-10 double slots).
    let units: Vec<u32> = if let Ok(spec) = std::env::var("CYBERDESK_CAPTURE_UNITS") {
        spec.split(',')
            .filter_map(|s| s.trim().parse::<u32>().ok().map(|u| u.clamp(1, 2)))
            .collect()
    } else {
        let want = std::env::var("CYBERDESK_CAPTURE_SLOTS")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(1);
        let n = want.clamp(1, crate::slots::frame_capacity(width, 1.0, &theme.slots));
        vec![1u32; n]
    };
    let units = if units.is_empty() { vec![1u32] } else { units };
    let frame = crate::slots::frame_layout(width, height, &units, 1.0, &theme.slots);
    let rects = frame.slots.clone();
    let n = rects.len();
    // Zone shadow under the slots AND both side zones.
    let mut zones: Vec<[f32; 4]> = rects.iter().map(|r| [r.x, r.y, r.w, r.h]).collect();
    for s in [frame.left, frame.right] {
        zones.push([s.x, s.y, s.w, s.h]);
    }

    let mut pulse = PulseGrid::new(&device, format);
    if do_pulse {
        let glow = std::env::var("CYBERDESK_CAPTURE_GLOW")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(theme.background.glow_default / 100.0);
        // The slot rects double as the zone-shadow rects, so the shadow is
        // visible under each column in the self-test.
        pulse.prepare(&device, &queue, width, height, 1.0, theme, base, glow, &zones);
        // Advance the life sim to a representative animated moment (pulses have
        // travelled, at least one node flare is mid-expansion).
        for i in 1..=32 {
            pulse.update_life(&queue, i as f32 * 0.05, theme);
        }
    }

    // Placeholder columns + side zones + the active accent on slot 0 (shell-side,
    // no CEF) — exactly what a fresh all-lazy frame looks like before navigation.
    let st = &theme.slots;
    let placeholder = SlotPlaceholder::new(&device, format, MAX_SLOTS as u32 + 2);
    let slotlines = SlotLines::new(&device, format, MAX_SLOTS as u32);
    let corner = theme.page.corner_radius;
    let fill_rgb = [
        base[0] + st.placeholder_fill,
        base[1] + st.placeholder_fill,
        base[2] + st.placeholder_fill,
    ];
    let glyph_rgb = [
        brand[0] * st.placeholder_glyph,
        brand[1] * st.placeholder_glyph,
        brand[2] * st.placeholder_glyph,
    ];
    // CYBERDESK_CAPTURE_PENDING=N marks the first N columns as restored-pending
    // (a scheme-colored dot), so the CD-10 session placeholder reads headlessly.
    let pending = std::env::var("CYBERDESK_CAPTURE_PENDING")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let accent = crate::theme::hex3(&theme.colors.accent);
    // The two side zones (diamond glyph, digit 0) then the slot columns.
    let mut ph_insts: Vec<PlaceholderInstance> = [frame.left, frame.right]
        .iter()
        .map(|r| PlaceholderInstance {
            rect: [r.x, r.y, r.w, r.h],
            fill: [fill_rgb[0], fill_rgb[1], fill_rgb[2], corner],
            glyph: [glyph_rgb[0], glyph_rgb[1], glyph_rgb[2], 0.0],
            dot: [0.0, 0.0, 0.0, 0.0],
        })
        .collect();
    ph_insts.extend(rects.iter().enumerate().map(|(i, r)| PlaceholderInstance {
        rect: [r.x, r.y, r.w, r.h],
        fill: [fill_rgb[0], fill_rgb[1], fill_rgb[2], corner],
        glyph: [glyph_rgb[0], glyph_rgb[1], glyph_rgb[2], (i + 1) as f32],
        dot: if i < pending {
            [accent[0], accent[1], accent[2], 1.0]
        } else {
            [0.0, 0.0, 0.0, 0.0]
        },
    }));
    let line_insts: Vec<SlotLineInstance> = rects
        .iter()
        .enumerate()
        .map(|(i, r)| SlotLineInstance {
            rect: [r.x, r.y, r.w, r.h],
            params: [0.0, if i == 0 { 1.0 } else { 0.0 }, st.active_line.max(1.0), 2.5],
        })
        .collect();
    queue.write_buffer(
        &placeholder.globals_buf,
        0,
        bytemuck::bytes_of(&PlaceholderGlobals {
            resolution: [width as f32, height as f32],
            _pad: [0.0, 0.0],
        }),
    );
    queue.write_buffer(&placeholder.instance_buf, 0, bytemuck::cast_slice(&ph_insts));
    queue.write_buffer(
        &slotlines.globals_buf,
        0,
        bytemuck::bytes_of(&SlotLineGlobals {
            resolution: [width as f32, height as f32],
            time: 1.6,
            _pad: 0.0,
            brand: [brand[0], brand[1], brand[2], 1.0],
        }),
    );
    queue.write_buffer(&slotlines.instance_buf, 0, bytemuck::cast_slice(&line_insts));

    // CYBERDESK_CAPTURE_DRAG=1 renders a sample favorite-drag overlay (CD-12): the
    // gutter drop zones (the middle one hot) + a ghost; CYBERDESK_CAPTURE_CLOSE=1
    // renders a per-slot close orb on each slot's top-right corner. Headless checks.
    let drag = DragOverlay::new(&device, format, MAX_SLOTS as u32 * 2 + 4);
    let mut drag_insts: Vec<DragInstance> = Vec::new();
    if std::env::var("CYBERDESK_CAPTURE_CLOSE").is_ok() {
        let d = theme.command.close_size;
        let rad = d * 0.5;
        let m = 8.0 + rad;
        for r in &rects {
            let ox = r.x + r.w - m;
            let oy = r.y + m;
            let rect = [ox - rad, oy - rad, d, d];
            // Backing disc (kind 0) + ring/cross (kind 1).
            drag_insts.push(DragInstance {
                rect,
                color: [0.02, 0.03, 0.05, 0.55],
                shape: [rad, 2.0, 0.0, 0.0],
            });
            drag_insts.push(DragInstance {
                rect,
                color: [brand[0], brand[1], brand[2], 0.92],
                shape: [rad, 1.2, 1.0, 0.0],
            });
        }
    }
    if std::env::var("CYBERDESK_CAPTURE_DRAG").is_ok() {
        let g = (theme.slots.gutter).round();
        let (sy, sh) = rects.first().map(|r| (r.y, r.h)).unwrap_or((0.0, 0.0));
        // Gutter bars before slot 0, between pairs, and after the last.
        let mut bars: Vec<f32> = vec![rects[0].x - g];
        for p in 1..rects.len() {
            bars.push(rects[p - 1].x + rects[p - 1].w);
        }
        if let Some(last) = rects.last() {
            bars.push(last.x + last.w);
        }
        let hot = bars.len() / 2;
        for (i, &bx) in bars.iter().enumerate() {
            let a = if i == hot { 0.6 } else { 0.16 };
            drag_insts.push(DragInstance {
                rect: [bx, sy, g, sh],
                color: [brand[0], brand[1], brand[2], a],
                shape: [g * 0.5, if i == hot { 16.0 } else { 6.0 }, 0.0, 0.0],
            });
        }
        // Ghost circle over the hot gutter.
        let gx = bars[hot] + g * 0.5;
        let gy = sy + sh * 0.4;
        let gs = 40.0;
        drag_insts.push(DragInstance {
            rect: [gx - gs * 0.5, gy - gs * 0.5, gs, gs],
            color: [brand[0], brand[1], brand[2], 0.85],
            shape: [gs * 0.5, 13.0, 0.0, 0.0],
        });
    }
    if !drag_insts.is_empty() {
        queue.write_buffer(
            &drag.globals_buf,
            0,
            bytemuck::bytes_of(&PlaceholderGlobals { resolution: [width as f32, height as f32], _pad: [0.0, 0.0] }),
        );
        queue.write_buffer(&drag.instance_buf, 0, bytemuck::cast_slice(&drag_insts));
    }

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("capture-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let bytes_per_pixel = 4u32;
    let unpadded_bytes_per_row = width * bytes_per_pixel;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(align) * align;

    let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("capture-readback"),
        size: (padded_bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("capture-encoder"),
    });
    // Bake the static circuit into its HDR target first.
    if do_pulse {
        pulse.record_bake(&mut encoder);
    }
    {
        let clear = wgpu::Color {
            r: base[0] as f64,
            g: base[1] as f64,
            b: base[2] as f64,
            a: 1.0,
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("capture-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // The shell background is the Pulse Grid alone (CD-06, D-0013): the
        // baked circuit + its life layer, no ring.
        if do_pulse {
            pulse.composite(&mut pass);
            pulse.draw_life(&mut pass);
        }
        // Placeholder columns + side zones, then slot lines, over the background.
        pass.set_pipeline(&placeholder.pipeline);
        pass.set_bind_group(0, &placeholder.globals_bg, &[]);
        pass.set_vertex_buffer(0, placeholder.instance_buf.slice(..));
        pass.draw(0..6, 0..ph_insts.len() as u32);
        pass.set_pipeline(&slotlines.pipeline);
        pass.set_bind_group(0, &slotlines.globals_bg, &[]);
        pass.set_vertex_buffer(0, slotlines.instance_buf.slice(..));
        pass.draw(0..6, 0..n as u32);
        if !drag_insts.is_empty() {
            pass.set_pipeline(&drag.pipeline);
            pass.set_bind_group(0, &drag.globals_bg, &[]);
            pass.set_vertex_buffer(0, drag.instance_buf.slice(..));
            pass.draw(0..6, 0..drag_insts.len() as u32);
        }
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &output_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = output_buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| {
        r.expect("failed to map readback buffer");
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("failed to poll device");

    let data = slice.get_mapped_range().expect("failed to read mapped range");
    let mut pixels = Vec::with_capacity((width * height * bytes_per_pixel) as usize);
    for row in 0..height {
        let start = (row * padded_bytes_per_row) as usize;
        let end = start + unpadded_bytes_per_row as usize;
        pixels.extend_from_slice(&data[start..end]);
    }
    drop(data);
    output_buffer.unmap();

    image::save_buffer(path, &pixels, width, height, image::ExtendedColorType::Rgba8)
        .expect("failed to write PNG");
}
