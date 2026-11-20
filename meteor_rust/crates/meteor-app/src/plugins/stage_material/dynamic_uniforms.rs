use super::water::interaction::LiquidContacts;
use super::*;
use bevy::ecs::system::SystemParamItem;
use bevy::render::render_resource::{
    binding_types::uniform_buffer, AsBindGroupError, BindGroupLayout, BindGroupLayoutEntry, Buffer,
    OwnedBindingResource, ShaderStages, UniformBuffer, UnpreparedBindGroup,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};
use std::collections::{HashMap, HashSet};

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use crate::plugins::graphics_pack::sky::cache::{CacheView, CloudCache};
#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
use bevy::render::{
    render_resource::{
        binding_types::{sampler, texture_2d_array},
        SamplerBindingType, TextureSampleType, TextureViewDimension,
    },
    texture::FallbackImage,
};

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
type SkyBindingParams = (
    Option<Res<'static, CloudCache>>,
    Res<'static, FallbackImage>,
);
#[cfg(any(target_arch = "wasm32", target_os = "android"))]
type SkyBindingParams = ();

type ParamKey = [u32; 83];

#[derive(Resource, Default, Clone)]
pub(crate) struct FrameUniforms {
    pub camera: CameraOcclusionParams,
    pub seconds: f64,
    pub scene_normal_maps: bool,
    pub water_enhance: bool,
    pub liquid_contacts: LiquidContacts,

    pub lighting: Vec4,
    pub graphics: crate::plugins::graphics_pack::GraphicsFrame,
}

pub(crate) fn split_time(seconds: f64) -> Vec2 {
    let high = (seconds / 4096.0).floor() * 4096.0;
    Vec2::new(high as f32, (seconds - high) as f32)
}

#[derive(ShaderType, Clone)]
struct GpuParams {
    base: GmdlMaterialParams,
    scroll: Vec4,
    detail: Vec4,
    body_glow: Vec4,
    body_style: Vec4,
    water: Vec4,
    water_bounds: Vec4,
    first_person_clip: Vec4,
    water_dynamics_bounds: Vec4,
    pbr: Vec4,
    pbr_tint: Vec4,
    pbr_textures: Vec4,
    water_optics: WaterOptics,
    grass_ground: Vec4,
    grass_bounds: Vec4,
    pbr_lighting: Vec3,
    grass_underlay_scale: f32,
    water_sky_style: Vec4,
    water_sky_ranges: Vec4,
}

impl GmdlMaterial {
    fn param_key(&self) -> ParamKey {
        let p = &self.params;
        let existing = [
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
            self.normal_texture.is_some() as u32,
            self.normal_strength.to_bits(),
            self.normal_contrast.to_bits(),
            self.body_glow.x.to_bits(),
            self.body_glow.y.to_bits(),
            self.body_glow.z.to_bits(),
            self.body_glow.w.to_bits(),
            self.body_style.x.to_bits(),
            self.body_style.y.to_bits(),
            self.body_style.z.to_bits(),
            self.body_style.w.to_bits(),
            self.water.x.to_bits(),
            self.water.y.to_bits(),
            self.water.z.to_bits(),
            self.water.w.to_bits(),
            self.water_bounds.x.to_bits(),
            self.water_bounds.y.to_bits(),
            self.water_bounds.z.to_bits(),
            self.water_bounds.w.to_bits(),
            self.water_live_reflection as u32,
            self.first_person_clip.x.to_bits(),
            self.first_person_clip.y.to_bits(),
            self.first_person_clip.z.to_bits(),
            self.first_person_clip.w.to_bits(),
            self.water_dynamics_bounds.x.to_bits(),
            self.water_dynamics_bounds.y.to_bits(),
            self.water_dynamics_bounds.z.to_bits(),
            self.water_dynamics_bounds.w.to_bits(),
            self.pbr.x.to_bits(),
            self.pbr.y.to_bits(),
            self.pbr.z.to_bits(),
            self.pbr.w.to_bits(),
            self.pbr_tint.x.to_bits(),
            self.pbr_tint.y.to_bits(),
            self.pbr_tint.z.to_bits(),
            self.pbr_tint.w.to_bits(),
            self.pbr_textures.x.to_bits(),
            self.pbr_textures.y.to_bits(),
            self.pbr_textures.z.to_bits(),
            self.pbr_textures.w.to_bits(),
        ];
        let mut key = [0; 83];
        key[..50].copy_from_slice(&existing);
        key[50] = u32::from(self.water_screen_refraction);
        key[51..55].copy_from_slice(
            &self
                .water_optics
                .absorption_ior
                .to_array()
                .map(f32::to_bits),
        );
        key[55..59].copy_from_slice(&self.water_optics.scattering_g.to_array().map(f32::to_bits));
        key[59..63].copy_from_slice(&self.water_optics.control.to_array().map(f32::to_bits));
        key[63..67].copy_from_slice(&self.grass_ground.to_array().map(f32::to_bits));
        key[67..71].copy_from_slice(&self.grass_bounds.to_array().map(f32::to_bits));
        key[71..74].copy_from_slice(&self.pbr_lighting.to_array().map(f32::to_bits));
        key[74] = self.grass_underlay_scale.to_bits();
        key[75..79].copy_from_slice(&self.water_sky_style.to_array().map(f32::to_bits));
        key[79..83].copy_from_slice(&self.water_sky_ranges.to_array().map(f32::to_bits));
        key
    }
}

#[derive(Resource, Default)]
struct LiveParams(HashSet<ParamKey>);

#[derive(Resource, Default)]
pub struct SharedBuffers {
    camera: UniformBuffer<CameraOcclusionParams>,
    disabled: UniformBuffer<CameraOcclusionParams>,
    clock: UniformBuffer<Vec4>,
    lighting: UniformBuffer<Vec4>,
    liquid_contacts: UniformBuffer<LiquidContacts>,
    graphics: UniformBuffer<crate::plugins::graphics_pack::GraphicsFrame>,
    grass_light: UniformBuffer<Vec4>,
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    empty_sky: UniformBuffer<CacheView>,
}

impl SharedBuffers {
    pub(in crate::plugins) fn clock(&self) -> Result<Buffer, AsBindGroupError> {
        self.clock
            .buffer()
            .cloned()
            .ok_or(AsBindGroupError::RetryNextUpdate)
    }

    pub(in crate::plugins) fn grass_light(&self) -> Result<Buffer, AsBindGroupError> {
        self.grass_light
            .buffer()
            .cloned()
            .ok_or(AsBindGroupError::RetryNextUpdate)
    }
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
                    pbr: material.pbr,
                    pbr_tint: material.pbr_tint,
                    pbr_textures: material.pbr_textures,
                    scroll: material.uv_scroll,
                    body_glow: material.body_glow,
                    body_style: material.body_style,
                    water: material.water,
                    water_bounds: material.water_bounds,
                    first_person_clip: material.first_person_clip,
                    water_dynamics_bounds: material.water_dynamics_bounds,
                    water_optics: material.water_optics,
                    grass_ground: material.grass_ground,
                    grass_bounds: material.grass_bounds,
                    pbr_lighting: material.pbr_lighting,
                    grass_underlay_scale: material.grass_underlay_scale,
                    water_sky_style: material.water_sky_style,
                    water_sky_ranges: material.water_sky_ranges,
                    detail: Vec4::new(
                        if material.normal_texture.is_some() {
                            1.0
                        } else {
                            0.0
                        },
                        f32::from(
                            material.water_live_reflection as u8
                                + 2 * material.water_screen_refraction as u8,
                        ),
                        material.normal_strength,
                        material.normal_contrast,
                    ),
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
    #[texture(6)]
    #[sampler(7)]
    normal_texture: Option<Handle<Image>>,
    #[texture(8)]
    #[sampler(9)]
    water_reflection: Option<Handle<Image>>,
    #[texture(10)]
    #[sampler(11)]
    water_depth: Option<Handle<Image>>,
    #[texture(12)]
    #[sampler(13)]
    water_normal: Option<Handle<Image>>,
    #[texture(15)]
    #[sampler(16)]
    surface_detail: Option<Handle<Image>>,
    #[texture(18, visibility(fragment))]
    surface_aux: Option<Handle<Image>>,
}

impl AsBindGroup for GmdlMaterial {
    type Data = GmdlMaterialKey;
    type Param = (
        <Textures as AsBindGroup>::Param,
        Res<'static, SharedBuffers>,
        ResMut<'static, ParamBuffers>,
        Res<'static, RenderQueue>,
        SkyBindingParams,
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
        let (textures, shared, cache, queue, _sky) = param;
        let camera = if self.camera_occlusion_enabled {
            &shared.camera
        } else {
            &shared.disabled
        };
        let camera = camera
            .buffer()
            .ok_or(AsBindGroupError::RetryNextUpdate)?
            .clone();
        let clock = shared.clock()?;
        let lighting = shared
            .lighting
            .buffer()
            .ok_or(AsBindGroupError::RetryNextUpdate)?
            .clone();
        let liquid_contacts = shared
            .liquid_contacts
            .buffer()
            .ok_or(AsBindGroupError::RetryNextUpdate)?
            .clone();

        let pbr = self.pbr.x > 0.5;
        let mut result = Textures {
            texture: self.texture.clone(),
            normal_texture: self.normal_texture.clone(),
            water_reflection: if pbr {
                self.env_specular.clone()
            } else {
                self.water_reflection.clone()
            },
            water_depth: if pbr {
                self.env_diffuse.clone()
            } else {
                self.water_depth.clone()
            },
            water_normal: if pbr {
                self.orm_texture.clone()
            } else {
                self.water_normal.clone()
            },

            surface_detail: self
                .water_dynamics
                .clone()
                .or_else(|| self.grass_coverage.clone()),

            surface_aux: self
                .water_sky_visibility
                .clone()
                .or_else(|| self.grass_underlay.clone()),
        }
        .unprepared_bind_group(layout, device, textures, force_no_bindless)?;
        result
            .bindings
            .push((14, OwnedBindingResource::Buffer(liquid_contacts)));
        result.bindings.push((
            17,
            OwnedBindingResource::Buffer(
                shared
                    .graphics
                    .buffer()
                    .ok_or(AsBindGroupError::RetryNextUpdate)?
                    .clone(),
            ),
        ));
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        {
            let (clouds, fallback) = _sky;
            let (view, sampler, buffer) = if self.water_sky_reflection {
                clouds
                    .as_ref()
                    .and_then(|clouds| clouds.reflection_bindings())
                    .ok_or(AsBindGroupError::RetryNextUpdate)?
            } else {
                (
                    &fallback.d2_array.texture_view,
                    &fallback.d2_array.sampler,
                    shared
                        .empty_sky
                        .buffer()
                        .ok_or(AsBindGroupError::RetryNextUpdate)?,
                )
            };
            result.bindings.extend([
                (
                    19,
                    OwnedBindingResource::TextureView(TextureViewDimension::D2Array, view.clone()),
                ),
                (
                    20,
                    OwnedBindingResource::Sampler(SamplerBindingType::Filtering, sampler.clone()),
                ),
                (21, OwnedBindingResource::Buffer(buffer.clone())),
            ]);
        }
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
        result
            .bindings
            .push((5, OwnedBindingResource::Buffer(lighting)));
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
        entries.push(uniform_buffer::<Vec4>(false).build(5, ShaderStages::FRAGMENT));
        entries.push(uniform_buffer::<LiquidContacts>(false).build(14, ShaderStages::FRAGMENT));
        entries.push(
            uniform_buffer::<crate::plugins::graphics_pack::GraphicsFrame>(false)
                .build(17, ShaderStages::FRAGMENT),
        );
        #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
        entries.extend([
            texture_2d_array(TextureSampleType::Float { filterable: true })
                .build(19, ShaderStages::FRAGMENT),
            sampler(SamplerBindingType::Filtering).build(20, ShaderStages::FRAGMENT),
            uniform_buffer::<CacheView>(false).build(21, ShaderStages::FRAGMENT),
        ]);
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
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    if shared.empty_sky.buffer().is_none() {
        shared.empty_sky.write_buffer(&device, &queue);
    }
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
    if shared.lighting.buffer().is_none() || *shared.lighting.get() != frame.lighting {
        shared.lighting.set(frame.lighting);
        shared.lighting.write_buffer(&device, &queue);
    }
    if shared.liquid_contacts.buffer().is_none()
        || *shared.liquid_contacts.get() != frame.liquid_contacts
    {
        shared.liquid_contacts.set(frame.liquid_contacts);
        shared.liquid_contacts.write_buffer(&device, &queue);
    }
    if shared.graphics.buffer().is_none() || *shared.graphics.get() != frame.graphics {
        shared.graphics.set(frame.graphics);
        shared.graphics.write_buffer(&device, &queue);
    }
    let grass_light = frame.graphics.grass_light();
    if shared.grass_light.buffer().is_none() || *shared.grass_light.get() != grass_light {
        shared.grass_light.set(grass_light);
        shared.grass_light.write_buffer(&device, &queue);
    }
    let clock = split_time(frame.seconds)
        .extend(if frame.scene_normal_maps { 1.0 } else { 0.0 })
        .extend(if frame.water_enhance { 1.0 } else { 0.0 });
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
                shared.liquid_contacts.buffer().unwrap().id(),
            )
        };
        let original = ids(&app);
        for i in 1..20 {
            let mut frame = app.world_mut().resource_mut::<FrameUniforms>();
            frame.camera.eye = Vec4::new(i as f32, 2.0, 3.0, 0.8);
            frame.seconds = f64::from(i) / 60.0;
            frame.liquid_contacts.counts.x = 1.0;
            frame.liquid_contacts.positions[0].w = i as f32 / 60.0;
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
        let mut glowing = material.clone();
        glowing.body_glow = Vec4::new(1.0, 0.035, 0.015, 0.9);
        assert_ne!(cache.get_buffer(&glowing, &device, &queue).id(), initial);
        let mut styled = material.clone();
        styled.body_style = Vec4::new(1.3, 4.0, 3.0, 1.7);
        let styled_id = cache.get_buffer(&styled, &device, &queue).id();
        assert_ne!(styled_id, initial);
        assert_ne!(
            styled_id,
            cache.get_buffer(&glowing, &device, &queue).id(),
            "style changes must select their own parameter buffer"
        );
        assert_eq!(cache.0.len(), 3);
        let mut water = material.clone();
        water.water = Vec4::new(1.0, 12.0, 0.0, 0.0);
        let water_id = cache.get_buffer(&water, &device, &queue).id();
        assert_ne!(water_id, initial);
        water.water_bounds = Vec4::new(-2.0, 3.0, 10.0, 12.0);
        assert_ne!(cache.get_buffer(&water, &device, &queue).id(), water_id);
        let mut relief = material.clone();
        relief.normal_strength = 1.8;
        let relief_id = cache.get_buffer(&relief, &device, &queue).id();
        assert_ne!(relief_id, initial);
        relief.normal_contrast = 1.5;
        assert_ne!(cache.get_buffer(&relief, &device, &queue).id(), relief_id);
        let mut first_person = material.clone();
        first_person.first_person_clip.w = 0.9;
        assert_ne!(
            cache.get_buffer(&first_person, &device, &queue).id(),
            initial,
            "self-occlusion radius must select its own GPU parameter buffer"
        );
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
        assert_eq!(
            entries.len(),
            if cfg!(any(target_arch = "wasm32", target_os = "android")) {
                19
            } else {
                22
            }
        );

        use bevy::render::render_resource::*;
        let source = super::super::standalone_shader_source(true)
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
