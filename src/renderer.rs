//! wgpu renderer for the CyberDesk shell.
//!
//! Composites, every frame:
//!   1. the shell — dark background + rotating CARVILON ring (`ring.wgsl`), and
//!   2. the surf-zone page — the CEF off-screen texture drawn at the zone
//!      rectangle with rounded corners (`page.wgsl`), blended over the shell.
//!
//! All wgpu work lives on the main thread. CEF's `on_paint` (on the CEF UI
//! thread) only hands over raw BGRA bytes; [`upload_page`](SurfaceRenderer::upload_page)
//! copies them into the persistent page texture here.
//!
//! The off-screen [`capture`] path renders the ring only (headless self-test).

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use winit::window::Window;

/// Non-sRGB render target so CEF's BGRA bytes and our sRGB brand colors pass
/// through unchanged (matches the cef-rs OSR example).
const SURFACE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;

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
    _pad: f32,
}

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

struct PagePass {
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    tex_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    // Persistent page texture; recreated only when the CEF frame size changes.
    texture: Option<wgpu::Texture>,
    tex_bind_group: Option<wgpu::BindGroup>,
    width: u32,
    height: u32,
}

impl PagePass {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("page-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("page.wgsl").into()),
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("page-uniforms"),
            size: std::mem::size_of::<PageUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
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
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("page-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let tex_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            bind_group_layouts: &[Some(&uniform_bgl), Some(&tex_bind_group_layout)],
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
        Self {
            pipeline,
            uniform_buf,
            uniform_bind_group,
            tex_bind_group_layout,
            sampler,
            texture: None,
            tex_bind_group: None,
            width: 0,
            height: 0,
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8], w: u32, h: u32) {
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
                layout: &self.tex_bind_group_layout,
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
/// `fs_main`, an opaque REPLACE target.
fn fullscreen_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    _blend: bool,
) -> wgpu::RenderPipeline {
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
                blend: Some(wgpu::BlendState::REPLACE),
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

/// Renders the shell + surf-zone page to a winit window surface.
pub struct SurfaceRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    ring_pipeline: wgpu::RenderPipeline,
    ring_uniform_buf: wgpu::Buffer,
    ring_bind_group: wgpu::BindGroup,
    page: PagePass,
    field: DeepField,
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

        let (ring_pipeline, ring_uniform_buf, ring_bind_group) = ring_pipeline(&device, SURFACE_FORMAT);
        let page = PagePass::new(&device, SURFACE_FORMAT);
        let field = DeepField::new(&device, SURFACE_FORMAT);

        Self {
            surface,
            device,
            queue,
            config,
            ring_pipeline,
            ring_uniform_buf,
            ring_bind_group,
            page,
            field,
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

    /// Upload a freshly painted CEF frame (BGRA) into the page texture.
    pub fn upload_page(&mut self, data: &[u8], w: u32, h: u32) {
        self.page.upload(&self.device, &self.queue, data, w, h);
    }

    /// Render one frame. `zone` is the surf-zone rect in device pixels
    /// (x, y, w, h). `feather` and `deep_field` are the live settings toggles
    /// (consumed by the page feathering in Stage C and the background in Stage B).
    pub fn render(&mut self, time: f32, zone: (f32, f32, f32, f32), feather: bool, deep_field: bool) {
        let _ = feather; // consumed in CD-03 Stage C
        let (win_w, win_h) = (self.config.width as f32, self.config.height as f32);
        let corner_radius = self.theme.page.corner_radius;

        // Ring uniforms (all values from theme tokens).
        let ring = RingUniforms::from_theme(&self.theme, [win_w, win_h], time, 0);
        self.queue
            .write_buffer(&self.ring_uniform_buf, 0, bytemuck::bytes_of(&ring));

        // Page uniforms: zone rect -> NDC (y flipped).
        let (zx, zy, zw, zh) = zone;
        let to_ndc_x = |x: f32| (x / win_w) * 2.0 - 1.0;
        let to_ndc_y = |y: f32| 1.0 - (y / win_h) * 2.0;
        let page = PageUniforms {
            rect_ndc: [
                to_ndc_x(zx),
                to_ndc_y(zy),
                to_ndc_x(zx + zw),
                to_ndc_y(zy + zh),
            ],
            px_size: [self.page.width.max(1) as f32, self.page.height.max(1) as f32],
            corner_radius,
            _pad: 0.0,
        };
        self.queue
            .write_buffer(&self.page.uniform_buf, 0, bytemuck::bytes_of(&page));

        // Deep Field: repaint the half-res target at ~30 fps (every other frame,
        // or right after a resize).
        self.field.frame = self.field.frame.wrapping_add(1);
        let do_field = deep_field && {
            self.field
                .ensure_target(&self.device, self.config.width, self.config.height);
            self.field.frame % 2 == 0 || self.field.needs_render
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

        // Pass 2: shell composite — Deep Field (upscaled), then ring, then page.
        let base = self.theme.colors.background_rgb();
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

            // Backmost: the Deep Field (upscaled from half res), when enabled.
            if deep_field {
                if let Some(bg) = self.field.composite_bind_group.as_ref() {
                    pass.set_pipeline(&self.field.composite_pipeline);
                    pass.set_bind_group(0, bg, &[]);
                    pass.draw(0..3, 0..1);
                }
            }

            // Ring (transparent, composited over the field).
            pass.set_pipeline(&self.ring_pipeline);
            pass.set_bind_group(0, &self.ring_bind_group, &[]);
            pass.draw(0..3, 0..1);

            // Surf-zone page (over everything), if a frame has arrived.
            if let Some(tex_bind_group) = self.page.tex_bind_group.as_ref() {
                pass.set_pipeline(&self.page.pipeline);
                pass.set_bind_group(0, &self.page.uniform_bind_group, &[]);
                pass.set_bind_group(1, tex_bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame);
    }
}

/// Render a single ring frame off-screen to a PNG (headless self-test; renders
/// the shell only, no CEF).
pub fn capture(path: &str, width: u32, height: u32, time: f32, theme: &crate::theme::Theme) {
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
    let (pipeline, uniform_buf, bind_group) = ring_pipeline(&device, format);
    // sRGB target: convert the token sRGB values to linear in-shader.
    let ring = RingUniforms::from_theme(theme, [width as f32, height as f32], time, 1);
    queue.write_buffer(&uniform_buf, 0, bytemuck::bytes_of(&ring));

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
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("capture-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
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
