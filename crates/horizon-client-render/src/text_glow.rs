//! The glow layer: bloom for mods' glowing world text (`HudBatch::glow`).
//! Those HUD vertices are drawn alone into a cleared texture the frame's size,
//! halved, blurred by two rounds of a horizontal then a vertical Gaussian (the
//! second twice as wide), and added to the frame. Only what a mod marks as
//! glowing blooms: the rest of the frame is not read, so a bright sky does not
//! glow and the cost is a few passes over a half-size texture.

/// (direction, spread in half-size texels) of the blur passes.
const BLUR: [([f32; 2], f32); 4] = [
    ([1.0, 0.0], 1.0),
    ([0.0, 1.0], 1.0),
    ([1.0, 0.0], 2.0),
    ([0.0, 1.0], 2.0),
];
/// The blurred layer is added this many times: blurring spreads a thin glyph's
/// light over a wide halo, which alone would be faint.
const STRENGTH: f32 = 2.5;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    texel_dir: [f32; 4],
    strength: [f32; 4],
}

pub struct TextGlow {
    layout: wgpu::BindGroupLayout,
    down: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    add: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// The down pass's, the blur passes', the add's.
    uniforms: Vec<wgpu::Buffer>,
    /// The layer at the frame's size, then two half-size textures.
    targets: Vec<wgpu::TextureView>,
    size: [u32; 2],
    format: wgpu::TextureFormat,
}
impl TextGlow {
    /// `format` is the swapchain's; the layer works in its non-sRGB view, as the HUD does.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let format = format.remove_srgb_suffix();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Text glow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("text_glow.wgsl").into()),
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Text glow bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: fragment,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Text glow"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = |entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Text glow pass"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let additive = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let (down, blur, add) = (
            pipeline("fs_down", None),
            pipeline("fs_blur", None),
            pipeline("fs_add", Some(additive)),
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Text glow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniforms = (0..BLUR.len() + 2)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Text glow pass"),
                    size: std::mem::size_of::<Uniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        Self {
            layout,
            down,
            blur,
            add,
            sampler,
            uniforms,
            targets: Vec::new(),
            size: [0, 0],
            format,
        }
    }
    /// The layer, cleared, for the HUD to draw the glowing vertices into; the
    /// frame is `size`.
    pub fn begin(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        size: [u32; 2],
    ) -> &wgpu::TextureView {
        if self.size != size || self.targets.is_empty() {
            self.size = size;
            let half = size.map(|s| s.div_ceil(2));
            self.targets = [size, half, half]
                .into_iter()
                .map(|[width, height]| {
                    device
                        .create_texture(&wgpu::TextureDescriptor {
                            label: Some("Text glow layer"),
                            size: wgpu::Extent3d {
                                width,
                                height,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: self.format,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::RENDER_ATTACHMENT,
                            view_formats: &[],
                        })
                        .create_view(&Default::default())
                })
                .collect();
        }
        Self::run(encoder, &self.targets[0], None, true);
        &self.targets[0]
    }
    /// Give the layer's video memory back until a frame glows again.
    pub fn release(&mut self) {
        self.targets.clear();
    }
    /// Blur the layer `begin` gave and add it to `frame` (the swapchain's non-sRGB view).
    pub fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::TextureView,
        mut timing: Option<&mut crate::gfx::pack_timing::PassTiming>,
    ) {
        let [full, a, b] = [&self.targets[0], &self.targets[1], &self.targets[2]];
        let texel = |s: [u32; 2]| [1.0 / s[0] as f32, 1.0 / s[1] as f32];
        let half = self.size.map(|s| s.div_ceil(2));
        let [fx, fy] = texel(self.size);
        let [hx, hy] = texel(half);
        let mut slot = 0;
        let mut pass = |pipeline: &wgpu::RenderPipeline,
                        data: Uniform,
                        source: &wgpu::TextureView,
                        output: &wgpu::TextureView,
                        clear: bool| {
            queue.write_buffer(&self.uniforms[slot], 0, bytemuck::bytes_of(&data));
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Text glow pass"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.uniforms[slot].as_entire_binding(),
                    },
                ],
            });
            slot += 1;
            let mut pass = Self::run(encoder, output, timing.as_deref_mut(), clear);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        };
        pass(
            &self.down,
            Uniform {
                texel_dir: [fx, fy, 0.0, 0.0],
                strength: [0.0; 4],
            },
            full,
            a,
            true,
        );
        for (i, (dir, spread)) in BLUR.iter().enumerate() {
            let (source, output) = if i % 2 == 0 { (a, b) } else { (b, a) };
            let data = Uniform {
                texel_dir: [hx, hy, dir[0] * spread, dir[1] * spread],
                strength: [0.0; 4],
            };
            pass(&self.blur, data, source, output, true);
        }
        pass(
            &self.add,
            Uniform {
                texel_dir: [hx, hy, 0.0, 0.0],
                strength: [STRENGTH, 0.0, 0.0, 0.0],
            },
            a,
            frame,
            false,
        );
    }
    /// A pass into `output`, cleared to transparent black or loaded.
    fn run<'e>(
        encoder: &'e mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        timing: Option<&mut crate::gfx::pack_timing::PassTiming>,
        clear: bool,
    ) -> wgpu::RenderPass<'e> {
        let load = if clear {
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
        } else {
            wgpu::LoadOp::Load
        };
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Text glow pass"),
            timestamp_writes: timing.and_then(|t| t.writes("Text glow")),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn text_glow_shader_validates() {
        let module = naga::front::wgsl::parse_str(include_str!("text_glow.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }
}
