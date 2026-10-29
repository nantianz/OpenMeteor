use super::*;
use bevy::ecs::system::SystemParamItem;
use bevy::render::render_resource::{
    binding_types::uniform_buffer, AsBindGroupError, BindGroupLayout, BindGroupLayoutEntry, Buffer,
    OwnedBindingResource, ShaderStages, UniformBuffer, UnpreparedBindGroup,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use std::collections::{HashMap, HashSet};

type ParamKey = [u32; 10];

#[derive(Resource, Default, Clone)]
pub(crate) struct FrameUniforms {
    pub camera: CameraOcclusionParams,
    pub seconds: f64,
}

pub(crate) fn split_time(seconds: f64) -> Vec2 {
    let high = (seconds / 4096.0).floor() * 4096.0;
    Vec2::new(high as f32, (seconds - high) as f32)
}

#[derive(ShaderType, Clone)]
struct GpuParams {
    base: GmdlMaterialParams,
    scroll: Vec4,
}

impl GmdlMaterial {
    fn param_key(&self) -> ParamKey {
        let p = &self.params;
        [
            p.opacity.to_bits(),
            p.alpha_cutoff.to_bits(),
            p.mode,
            p.has_texture,
            p.uv_offset.x.to_bits(),
            p.uv_offset.y.to_bits(),
            self.uv_scroll.x.to_bits(),
            self.uv_scroll.y.to_bits(),
            self.uv_scroll.z.to_bits(),
            self.uv_scroll.w.to_bits(),
        ]
    }
}

#[derive(Resource, Default)]
struct LiveParams(HashSet<ParamKey>);

#[derive(Resource, Default)]
pub struct SharedBuffers {
    camera: UniformBuffer<CameraOcclusionParams>,
    disabled: UniformBuffer<CameraOcclusionParams>,
    clock: UniformBuffer<Vec4>,
}

#[derive(Resource, Default)]
pub struct ParamBuffers(HashMap<ParamKey, Buffer>);

impl ParamBuffers {
    fn get_buffer(
        &mut self,
        material: &GmdlMaterial,
        device: &RenderDevice,
        queue: &RenderQueue,
    ) -> Buffer {
        let key = material.param_key();
        self.0
            .entry(key)
            .or_insert_with(|| {
                let mut uniform = UniformBuffer::from(GpuParams {
                    base: material.params.clone(),
                    scroll: material.uv_scroll,
                });
                uniform.write_buffer(device, queue);
                #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
                super::super::perf_plugin::count_event(2);
                uniform.buffer().unwrap().clone()
            })
            .clone()
    }
}

#[derive(AsBindGroup)]
pub struct Textures {
    #[texture(1)]
    #[sampler(2)]
    texture: Option<Handle<Image>>,
}

impl AsBindGroup for GmdlMaterial {
    type Data = GmdlMaterialKey;
    type Param = (
        <Textures as AsBindGroup>::Param,
        Res<'static, SharedBuffers>,
        ResMut<'static, ParamBuffers>,
        Res<'static, RenderQueue>,
    );

    fn label() -> &'static str {
        "gmdl persistent uniforms"
    }
    fn bind_group_data(&self) -> Self::Data {
        self.key()
    }
    fn unprepared_bind_group(
        &self,
        layout: &BindGroupLayout,
        device: &RenderDevice,
        param: &mut SystemParamItem<'_, '_, Self::Param>,
        force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
        let (textures, shared, cache, queue) = param;
        let camera = if self.camera_occlusion_enabled {
            &shared.camera
        } else {
            &shared.disabled
        };
        let camera = camera
            .buffer()
            .ok_or(AsBindGroupError::RetryNextUpdate)?
            .clone();
        let clock = shared
            .clock
            .buffer()
            .ok_or(AsBindGroupError::RetryNextUpdate)?
            .clone();
        let mut result = Textures {
            texture: self.texture.clone(),
        }
        .unprepared_bind_group(layout, device, textures, force_no_bindless)?;
        let buffer = cache.get_buffer(self, device, queue);
        result
            .bindings
            .push((0, OwnedBindingResource::Buffer(buffer)));
        result
            .bindings
            .push((3, OwnedBindingResource::Buffer(camera)));
        result
            .bindings
            .push((4, OwnedBindingResource::Buffer(clock)));
        #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
        super::super::perf_plugin::count_event(0);
        Ok(result)
    }
    fn bind_group_layout_entries(
        device: &RenderDevice,
        force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let mut entries = Textures::bind_group_layout_entries(device, force_no_bindless);
        entries.push(uniform_buffer::<GpuParams>(false).build(0, ShaderStages::FRAGMENT));
        entries
            .push(uniform_buffer::<CameraOcclusionParams>(false).build(3, ShaderStages::FRAGMENT));
        entries.push(uniform_buffer::<Vec4>(false).build(4, ShaderStages::FRAGMENT));
        entries
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<FrameUniforms>();
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<SharedBuffers>()
            .init_resource::<ParamBuffers>()
            .init_resource::<FrameUniforms>()
            .init_resource::<LiveParams>()
            .add_systems(ExtractSchedule, extract_frame)
            .add_systems(
                Render,
                upload_frame
                    .after(RenderSystems::ExtractCommands)
                    .before(RenderSystems::PrepareAssets),
            );
    }
}

fn extract_frame(
    mut commands: Commands,
    frame: Extract<Res<FrameUniforms>>,
    materials: Extract<Res<Assets<GmdlMaterial>>>,
) {
    commands.insert_resource((**frame).clone());
    commands.insert_resource(LiveParams(
        materials.iter().map(|(_, m)| m.param_key()).collect(),
    ));
}

fn upload_frame(
    frame: Res<FrameUniforms>,
    live: Res<LiveParams>,
    mut shared: ResMut<SharedBuffers>,
    mut cache: ResMut<ParamBuffers>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
) {
    cache.0.retain(|key, _| live.0.contains(key));
    if shared.camera.buffer().is_none() || *shared.camera.get() != frame.camera {
        #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
        if shared.camera.buffer().is_none() {
            super::super::perf_plugin::count_event(1);
        }
        shared.camera.set(frame.camera);
        shared.camera.write_buffer(&device, &queue);
    }
    if shared.disabled.buffer().is_none() {
        shared.disabled.write_buffer(&device, &queue);
        #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
        super::super::perf_plugin::count_event(1);
    }
    let clock = split_time(frame.seconds).extend(0.0).extend(0.0);
    if shared.clock.buffer().is_none() || *shared.clock.get() != clock {
        #[cfg(all(feature = "perf-diagnostics", not(target_arch = "wasm32")))]
        if shared.clock.buffer().is_none() {
            super::super::perf_plugin::count_event(1);
        }
        shared.clock.set(clock);
        shared.clock.write_buffer(&device, &queue);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_buffers_are_reused_and_orphaned_parameters_are_released() {
        use bevy::render::settings::{Backends, WgpuSettings};
        let resources = bevy::tasks::block_on(bevy::render::renderer::initialize_renderer(
            Backends::PRIMARY,
            None,
            &WgpuSettings::default(),
            #[cfg(all(
                feature = "dlss5",
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc"
            ))]
            Default::default(),
        ));
        let device = resources.0.clone();
        let queue = resources.1.clone();
        let mut app = App::new();
        app.insert_resource(device.clone())
            .insert_resource(queue.clone())
            .init_resource::<SharedBuffers>()
            .init_resource::<ParamBuffers>()
            .init_resource::<FrameUniforms>()
            .init_resource::<LiveParams>()
            .add_systems(Update, upload_frame);
        app.update();
        let ids = |app: &App| {
            let shared = app.world().resource::<SharedBuffers>();
            (
                shared.camera.buffer().unwrap().id(),
                shared.disabled.buffer().unwrap().id(),
                shared.clock.buffer().unwrap().id(),
            )
        };
        let original = ids(&app);
        for i in 1..20 {
            let mut frame = app.world_mut().resource_mut::<FrameUniforms>();
            frame.camera.eye = Vec4::new(i as f32, 2.0, 3.0, 0.8);
            frame.seconds = f64::from(i) / 60.0;
            drop(frame);
            app.update();
            assert_eq!(
                ids(&app),
                original,
                "changing values must not recreate buffers"
            );
        }
        let material = GmdlMaterial::new_scene(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.0,
                mode: 0,
                has_texture: 1,
                uv_offset: Vec2::ZERO,
            },
            None,
            false,
            GmdlMode::Opaque,
        );
        let mut cache = app.world_mut().resource_mut::<ParamBuffers>();
        let initial = cache.get_buffer(&material, &device, &queue).id();
        let mut next_texture = material.clone();
        next_texture.texture = Some(bevy::asset::uuid_handle!(
            "00000000-0000-0000-0000-000000000007"
        ));
        assert_eq!(
            cache.get_buffer(&next_texture, &device, &queue).id(),
            initial
        );
        assert_eq!(cache.0.len(), 1);
        drop(cache);
        app.world_mut()
            .resource_mut::<LiveParams>()
            .0
            .insert(material.param_key());
        app.update();
        assert_eq!(app.world().resource::<ParamBuffers>().0.len(), 1);
        app.world_mut().resource_mut::<LiveParams>().0.clear();
        app.world_mut().resource_mut::<FrameUniforms>().camera = CameraOcclusionParams::default();
        app.update();
        assert!(app.world().resource::<ParamBuffers>().0.is_empty());
        assert_eq!(ids(&app), original);
        assert_eq!(
            *app.world().resource::<SharedBuffers>().camera.get(),
            CameraOcclusionParams::default()
        );
        assert_eq!(
            *app.world().resource::<SharedBuffers>().disabled.get(),
            CameraOcclusionParams::default()
        );

        let entries = GmdlMaterial::bind_group_layout_entries(&device, false);
        assert_eq!(entries.len(), 5);

        use bevy::render::render_resource::*;
        let source = include_str!("../gmdl_material.wgsl")
            .replace("#import bevy_pbr::forward_io::VertexOutput;", "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, @location(2) uv: vec2<f32>, @location(5) color: vec4<f32>, };")
            .replace("#import bevy_pbr::mesh_view_bindings::view;", "struct TestView { world_position: vec3<f32>, }; @group(0) @binding(0) var<uniform> view: TestView;")
            .replace("#{MATERIAL_BIND_GROUP}", "3")
            .replace("#ifdef VERTEX_COLORS", "").replace("#endif", "")
            + "\n@vertex fn test_vertex() -> VertexOutput { var o: VertexOutput; o.position=vec4<f32>(0.0,0.0,0.0,1.0); o.world_position=o.position; o.uv=vec2<f32>(0.0); o.color=vec4<f32>(1.0); return o; }";
        let shader = device.create_and_validate_shader_module(ShaderModuleDescriptor {
            label: None,
            source: ShaderSource::Wgsl(source.into()),
        });
        let view_layout = device.create_bind_group_layout(
            "test view",
            &[uniform_buffer::<Vec4>(false).build(0, ShaderStages::FRAGMENT)],
        );
        let empty = device.create_bind_group_layout("test empty", &[]);
        let layout = GmdlMaterial::bind_group_layout(&device);
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[
                Some(&view_layout),
                Some(&empty),
                Some(&empty),
                Some(&layout),
            ],
            immediate_size: 0,
        });
        let _pipeline = device.create_render_pipeline(&RawRenderPipelineDescriptor {
            label: Some("GMDL dynamic uniform validation"),
            layout: Some(&pipeline_layout),
            vertex: RawVertexState {
                module: &shader,
                entry_point: Some("test_vertex"),
                compilation_options: default(),
                buffers: &[],
            },
            primitive: default(),
            depth_stencil: None,
            multisample: default(),
            fragment: Some(RawFragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: default(),
                targets: &[Some(ColorTargetState {
                    format: TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
    }
}
