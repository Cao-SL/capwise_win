use wgpu::util::DeviceExt;

const BASE_MAX_HEIGHT: u32 = 540;
const BASE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const BLUR_TAP_SPACING: f32 = 1.0;
const MID_BLUR_TAP_SPACING: f32 = 2.0;

/// The strength the camera preview's on/off toggle records with.
pub const DEFAULT_SKIN_SMOOTHING: f32 = 0.6;

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FilterUniforms {
    texel_size: [f32; 2],
    direction: [f32; 2],
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct CompositeUniforms {
    amount: f32,
    edge_sigma: f32,
    texture_keep: f32,
    contour_threshold: f32,
    mid_sigma: f32,
    brighten: f32,
    yellow_cut: f32,
    red_cut: f32,
}

// Full strength stops short of a complete replace on purpose: at 100% the skin
// should read as retouched, never as a mask. Sigma is in 0..1 gamma encoded
// luma, where skin texture sits near 0.02 and facial features above 0.1.
fn composite_uniforms(strength: f32) -> CompositeUniforms {
    let strength = strength.clamp(0.0, 1.0);
    CompositeUniforms {
        amount: strength.powf(0.7) * 0.9,
        edge_sigma: 0.012 + 0.018 * strength,
        texture_keep: 0.4 - 0.2 * strength,
        contour_threshold: 0.02 + 0.03 * strength,
        mid_sigma: 0.02 + 0.02 * strength,
        brighten: 0.06 * strength,
        yellow_cut: 0.036 * strength,
        red_cut: 0.018 * strength,
    }
}

struct Resources {
    dimensions: (u32, u32),
    base_a: wgpu::TextureView,
    base_b: wgpu::TextureView,
    mid_a: wgpu::TextureView,
    mid_b: wgpu::TextureView,
    output_texture: wgpu::Texture,
    output_view: wgpu::TextureView,
    horizontal_group: wgpu::BindGroup,
    vertical_group: wgpu::BindGroup,
    reduce_group: wgpu::BindGroup,
    mid_horizontal_group: wgpu::BindGroup,
    mid_vertical_group: wgpu::BindGroup,
    downsample_uniforms: wgpu::Buffer,
}

/// Retouches skin without a model, so it costs the same on every machine and
/// renders the same in preview, editor and export: a two level guided filter
/// over a fixed 540p base, confined by a chroma skin mask. The fixed base is
/// what keeps one slider value looking alike across camera resolutions.
pub struct SkinSmoother {
    downsample_pipeline: wgpu::RenderPipeline,
    reduce_pipeline: wgpu::RenderPipeline,
    blur_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    filter_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    no_person_view: wgpu::TextureView,
    composite_uniforms: wgpu::Buffer,
    last_uniforms: Option<CompositeUniforms>,
    resources: Option<Resources>,
    generation: u64,
}

fn base_dimensions(width: u32, height: u32) -> (u32, u32) {
    if height <= BASE_MAX_HEIGHT {
        return (width.max(1), height.max(1));
    }
    let base_width = (u64::from(width) * u64::from(BASE_MAX_HEIGHT)) / u64::from(height);
    ((base_width as u32).max(1), BASE_MAX_HEIGHT)
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    fragment_entry: &str,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    bind_group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

impl SkinSmoother {
    pub fn new(device: &wgpu::Device) -> Self {
        let filter_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skin Smooth Filter Shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("shaders/skin_smooth_filter.wgsl").into(),
            ),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skin Smooth Composite Shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("shaders/skin_smooth_composite.wgsl").into(),
            ),
        });
        let filter_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Skin Smooth Filter Layout"),
            entries: &[texture_entry(0), sampler_entry(1), uniform_entry(2)],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Skin Smooth Composite Layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                sampler_entry(4),
                uniform_entry(5),
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Skin Smooth Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let no_person_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Skin Smooth Empty Person Mask"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let composite_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Skin Smooth Composite Uniforms"),
            size: std::mem::size_of::<CompositeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            downsample_pipeline: fullscreen_pipeline(
                device,
                "Skin Smooth Downsample",
                &filter_layout,
                &filter_shader,
                "fs_downsample",
                BASE_FORMAT,
            ),
            reduce_pipeline: fullscreen_pipeline(
                device,
                "Skin Smooth Reduce",
                &filter_layout,
                &filter_shader,
                "fs_reduce",
                BASE_FORMAT,
            ),
            blur_pipeline: fullscreen_pipeline(
                device,
                "Skin Smooth Blur",
                &filter_layout,
                &filter_shader,
                "fs_blur",
                BASE_FORMAT,
            ),
            composite_pipeline: fullscreen_pipeline(
                device,
                "Skin Smooth Composite",
                &composite_layout,
                &composite_shader,
                "fs_composite",
                OUTPUT_FORMAT,
            ),
            filter_layout,
            composite_layout,
            sampler,
            no_person_view,
            composite_uniforms,
            last_uniforms: None,
            resources: None,
            generation: 0,
        }
    }

    /// Changes whenever the output texture is replaced, so a caller can keep
    /// a bind group for as long as this stays the same.
    pub fn output_generation(&self) -> u64 {
        self.generation
    }

    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.resources.as_ref().map(|r| &r.output_texture)
    }

    pub fn output_view(&self) -> Option<&wgpu::TextureView> {
        self.resources.as_ref().map(|r| &r.output_view)
    }

    /// Brightening only reaches pixels `person_mask` marks. Without one the
    /// frame is smoothed but keeps its brightness, because chroma alone cannot
    /// tell pale skin from a cream wall.
    pub fn process_into_encoder(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input_texture: &wgpu::Texture,
        encoder: &mut wgpu::CommandEncoder,
        strength: f32,
        person_mask: Option<&wgpu::TextureView>,
    ) {
        self.ensure_resources(device, input_texture.width(), input_texture.height());

        let uniforms = composite_uniforms(strength);
        if self.last_uniforms != Some(uniforms) {
            queue.write_buffer(&self.composite_uniforms, 0, bytemuck::bytes_of(&uniforms));
            self.last_uniforms = Some(uniforms);
        }

        let resources = self.resources.as_ref().expect("resources ensured above");
        let input_view = input_texture.create_view(&Default::default());
        let downsample_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Skin Smooth Downsample Group"),
            layout: &self.filter_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: resources.downsample_uniforms.as_entire_binding(),
                },
            ],
        });
        let composite_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Skin Smooth Composite Group"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&resources.base_a),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&resources.mid_a),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(
                        person_mask.unwrap_or(&self.no_person_view),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: self.composite_uniforms.as_entire_binding(),
                },
            ],
        });

        fullscreen_pass(
            encoder,
            "Skin Smooth Downsample",
            &resources.base_a,
            &self.downsample_pipeline,
            &downsample_group,
        );
        fullscreen_pass(
            encoder,
            "Skin Smooth Blur H",
            &resources.base_b,
            &self.blur_pipeline,
            &resources.horizontal_group,
        );
        fullscreen_pass(
            encoder,
            "Skin Smooth Blur V",
            &resources.base_a,
            &self.blur_pipeline,
            &resources.vertical_group,
        );
        fullscreen_pass(
            encoder,
            "Skin Smooth Reduce",
            &resources.mid_a,
            &self.reduce_pipeline,
            &resources.reduce_group,
        );
        fullscreen_pass(
            encoder,
            "Skin Smooth Mid Blur H",
            &resources.mid_b,
            &self.blur_pipeline,
            &resources.mid_horizontal_group,
        );
        fullscreen_pass(
            encoder,
            "Skin Smooth Mid Blur V",
            &resources.mid_a,
            &self.blur_pipeline,
            &resources.mid_vertical_group,
        );
        fullscreen_pass(
            encoder,
            "Skin Smooth Composite",
            &resources.output_view,
            &self.composite_pipeline,
            &composite_group,
        );
    }

    fn ensure_resources(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self
            .resources
            .as_ref()
            .is_some_and(|r| r.dimensions == (width, height))
        {
            return;
        }

        let (base_width, base_height) = base_dimensions(width, height);
        let (mid_width, mid_height) = ((base_width / 2).max(1), (base_height / 2).max(1));
        let create_base = |label: &str, (width, height): (u32, u32)| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: BASE_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let base_a = create_base("Skin Smooth Base A", (base_width, base_height));
        let base_b = create_base("Skin Smooth Base B", (base_width, base_height));
        let mid_a = create_base("Skin Smooth Mid A", (mid_width, mid_height));
        let mid_b = create_base("Skin Smooth Mid B", (mid_width, mid_height));
        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Skin Smooth Output"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let output_view = output_texture.create_view(&Default::default());

        let texel_size = [1.0 / base_width as f32, 1.0 / base_height as f32];
        let mid_texel_size = [1.0 / mid_width as f32, 1.0 / mid_height as f32];
        let uniform_buffer = |label: &str, texel_size: [f32; 2], direction: [f32; 2]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::bytes_of(&FilterUniforms {
                    texel_size,
                    direction,
                }),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let downsample_uniforms =
            uniform_buffer("Skin Smooth Downsample Uniforms", texel_size, [0.0, 0.0]);
        let horizontal = uniform_buffer(
            "Skin Smooth H Uniforms",
            texel_size,
            [BLUR_TAP_SPACING, 0.0],
        );
        let vertical = uniform_buffer(
            "Skin Smooth V Uniforms",
            texel_size,
            [0.0, BLUR_TAP_SPACING],
        );
        let reduce = uniform_buffer("Skin Smooth Reduce Uniforms", mid_texel_size, [0.0, 0.0]);
        let mid_horizontal = uniform_buffer(
            "Skin Smooth Mid H Uniforms",
            mid_texel_size,
            [MID_BLUR_TAP_SPACING, 0.0],
        );
        let mid_vertical = uniform_buffer(
            "Skin Smooth Mid V Uniforms",
            mid_texel_size,
            [0.0, MID_BLUR_TAP_SPACING],
        );
        let filter_group = |label: &str, view: &wgpu::TextureView, buffer: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.filter_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            })
        };
        let horizontal_group = filter_group("Skin Smooth H Group", &base_a, &horizontal);
        let vertical_group = filter_group("Skin Smooth V Group", &base_b, &vertical);
        let reduce_group = filter_group("Skin Smooth Reduce Group", &base_a, &reduce);
        let mid_horizontal_group = filter_group("Skin Smooth Mid H Group", &mid_a, &mid_horizontal);
        let mid_vertical_group = filter_group("Skin Smooth Mid V Group", &mid_b, &mid_vertical);

        self.generation = self.generation.wrapping_add(1);
        self.resources = Some(Resources {
            dimensions: (width, height),
            base_a,
            base_b,
            mid_a,
            mid_b,
            output_texture,
            output_view,
            horizontal_group,
            vertical_group,
            reduce_group,
            mid_horizontal_group,
            mid_vertical_group,
            downsample_uniforms,
        });
    }
}
