//! A resource pack's `core/lightmap` shader (`Lightmap`'s pass: `screenquad.vsh` and
//! `lightmap.fsh` over the 16×16 lightmap), drawn into the lightmap in place of the one
//! `Renderer::update_lightmap` computes. Without one the lightmap is computed natively.

use std::path::Path;

use horizon_shaders::{CoreProgram, Glue};

/// `LightmapInfo`'s binding in set 0, apart from vanilla's other blocks and samplers.
const BINDING: u32 = 10;
/// `LightmapInfo` in std140: six floats, then four vec3s at 16-byte steps.
const INFO_SIZE: usize = 96;

const GLUE: Glue = Glue {
    inputs: &[],
    prelude: "",
    setup: "",
    // GL's first texture row is at the bottom of clip space, wgpu's at the top.
    finish: "gl_Position.y = -gl_Position.y;",
    vertex_id: "int(gl_VertexIndex)",
    blocks: &[("LightmapInfo", BINDING)],
};

const VERTEX: &str = "assets/minecraft/shaders/core/screenquad.vsh";
const FRAGMENT: &str = "assets/minecraft/shaders/core/lightmap.fsh";

/// The packs' lightmap shader over `defaults`, when a pack replaces either stage.
pub fn load(root: &Path, defaults: &Path) -> Option<CoreProgram> {
    crate::hud::pack_text::load_core(root, defaults, [VERTEX, FRAGMENT], &[], &GLUE, "lightmap")
}

/// What `LightmapRenderStateExtractor` gives `lightmap.fsh`, as `update_lightmap` uses it.
pub struct Info {
    pub sky_factor: f32,
    pub block_factor: f32,
    pub night_vision_factor: f32,
    pub darkness_scale: f32,
    pub boss_darkening: f32,
    pub brightness: f32,
    pub block_light_tint: [f32; 3],
    pub sky_light_color: [f32; 3],
    pub ambient_color: [f32; 3],
    pub night_vision_color: [f32; 3],
}

impl Info {
    fn std140(&self) -> [u8; INFO_SIZE] {
        let mut out = [0u8; INFO_SIZE];
        let scalars = [
            self.sky_factor,
            self.block_factor,
            self.night_vision_factor,
            self.darkness_scale,
            self.boss_darkening,
            self.brightness,
        ];
        let vectors = [
            self.block_light_tint,
            self.sky_light_color,
            self.ambient_color,
            self.night_vision_color,
        ];
        let floats = scalars.iter().enumerate().map(|(i, v)| (4 * i, *v)).chain(
            vectors
                .iter()
                .enumerate()
                .flat_map(|(i, v)| (0..3).map(move |c| (32 + 16 * i + 4 * c, v[c]))),
        );
        for (at, v) in floats {
            out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// The pipeline and its uniform buffer.
pub struct PackLightmap {
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
}

impl PackLightmap {
    /// The pipeline drawing into a `format` lightmap, or none when wgpu refuses the shader (one
    /// that reads a block or sampler other than `LightmapInfo`).
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        program: &CoreProgram,
    ) -> Option<Self> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = |label, module: &naga::Module| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module.clone())),
            })
        };
        let vertex = module("Resource pack lightmap (vertex)", &program.vertex);
        let fragment = module("Resource pack lightmap (fragment)", &program.fragment);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Resource pack lightmap"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: BINDING,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Resource pack lightmap uniforms"),
            size: INFO_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Resource pack lightmap"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: BINDING,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Resource pack lightmap"),
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Resource pack lightmap"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &fragment,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            log::warn!("the resource pack's lightmap shader is not used: {e}");
            return None;
        }
        Some(Self {
            pipeline,
            group,
            uniform,
        })
    }

    /// `Lightmap.updateLightmap`: the pack's shader over the whole lightmap at `target`.
    pub fn draw(
        &self,
        uploads: &mut super::upload::Uploads,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        info: &Info,
    ) {
        uploads.buffer(encoder, &self.uniform, 0, &info.std140());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Resource pack lightmap"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        // screenquad.vsh: one triangle over the whole target.
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lightmap_info_follows_std140() {
        let info = Info {
            sky_factor: 1.0,
            block_factor: 2.0,
            night_vision_factor: 3.0,
            darkness_scale: 4.0,
            boss_darkening: 5.0,
            brightness: 6.0,
            block_light_tint: [7.0, 8.0, 9.0],
            sky_light_color: [10.0, 11.0, 12.0],
            ambient_color: [13.0, 14.0, 15.0],
            night_vision_color: [16.0, 17.0, 18.0],
        };
        let bytes = info.std140();
        let at = |offset: usize| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        assert_eq!([at(0), at(20)], [1.0, 6.0]);
        assert_eq!([at(32), at(40)], [7.0, 9.0]);
        assert_eq!([at(48), at(64), at(80), at(88)], [10.0, 13.0, 16.0, 18.0]);
    }

    #[test]
    fn vanillas_lightmap_shader_translates() {
        let core = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local/horizon-client/26.3/assets/minecraft/shaders/core");
        let (Ok(vertex), Ok(fragment)) = (
            std::fs::read_to_string(core.join("screenquad.vsh")),
            std::fs::read_to_string(core.join("lightmap.fsh")),
        ) else {
            eprintln!("skipped: no prepared resources");
            return;
        };
        horizon_shaders::translate_core(&vertex, &fragment, &[], &|_| None, &GLUE).unwrap();
    }
}
