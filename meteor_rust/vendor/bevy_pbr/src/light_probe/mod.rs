

use bevy_app::{App, Plugin};
use bevy_asset::AssetId;
use bevy_camera::{visibility::VisibleEntities, Camera3d};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    query::{QueryData, ReadOnlyQueryData, With},
    resource::Resource,
    schedule::IntoScheduleConfigs,
    system::{Commands, Local, Query, Res, ResMut},
};
use bevy_image::Image;
use bevy_light::{
    cluster::ClusterVisibilityClass, EnvironmentMapLight, IrradianceVolume, LightProbe,
};
use bevy_math::{Affine3A, FloatOrd, Mat4, Quat, Vec3, Vec4};
use bevy_platform::collections::HashMap;
use bevy_render::{
    extract_instances::ExtractInstancesPlugin,
    render_asset::RenderAssets,
    render_resource::{DynamicUniformBuffer, Sampler, ShaderType, TextureView},
    renderer::{RenderAdapter, RenderAdapterInfo, RenderDevice, RenderQueue, WgpuWrapper},
    settings::WgpuFeatures,
    sync_world::{MainEntity, MainEntityHashMap, RenderEntity},
    texture::{FallbackImage, GpuImage},
    view::ExtractedView,
    Extract, ExtractSchedule, GpuResourceAppExt, Render, RenderApp, RenderSystems,
};
use bevy_shader::load_shader_library;
use bevy_transform::prelude::GlobalTransform;
use bitflags::bitflags;
use tracing::error;

use core::{any::TypeId, hash::Hash, ops::Deref};

use crate::{
    extract_clusters_for_cpu_clustering, generate::EnvironmentMapGenerationPlugin,
    gpu::extract_clusters_for_gpu_clustering, light_probe::environment_map::EnvironmentMapIds,
};

pub mod environment_map;
pub mod generate;
pub mod irradiance_volume;

pub const MAX_VIEW_LIGHT_PROBES: usize = 8;

const STANDARD_MATERIAL_FRAGMENT_SHADER_MIN_TEXTURE_BINDINGS: usize = 16;

pub struct LightProbePlugin;

#[derive(Clone, Copy, ShaderType, Default)]
struct RenderLightProbe {

    light_from_world_transposed: [Vec4; 3],

    falloff: Vec3,

    bounding_sphere_radius: f32,

    parallax_correction_bounds: Vec3,

    intensity: f32,

    world_position: Vec3,

    texture_index: i32,

    flags: u32,
}

#[derive(ShaderType)]
pub struct LightProbesUniform {

    reflection_probes: [RenderLightProbe; MAX_VIEW_LIGHT_PROBES],

    irradiance_volumes: [RenderLightProbe; MAX_VIEW_LIGHT_PROBES],

    reflection_probe_count: i32,

    irradiance_volume_count: i32,

    view_cubemap_index: i32,

    smallest_specular_mip_level_for_view: u32,

    view_rotation: Vec4,

    intensity_for_view: f32,

    view_environment_map_affects_lightmapped_mesh_diffuse: u32,
}

#[derive(Resource, Default, Deref, DerefMut)]
pub struct LightProbesBuffer(DynamicUniformBuffer<LightProbesUniform>);

#[derive(Component, Default, Deref, DerefMut)]
pub struct ViewLightProbesUniformOffset(u32);

struct LightProbeInfo<C>
where
    C: LightProbeComponent,
{

    main_entity: MainEntity,

    light_from_world: [Vec4; 3],

    world_from_light: Affine3A,

    bounding_sphere_radius: f32,

    falloff: Vec3,

    parallax_correction_bounds: Vec3,

    intensity: f32,

    flags: RenderLightProbeFlags,

    asset_id: C::AssetId,
}

bitflags! {

    #[derive(Clone, Copy, PartialEq, Debug)]
    pub struct RenderLightProbeFlags: u8 {

        const AFFECTS_LIGHTMAPPED_MESH_DIFFUSE = 1;

        const ENABLE_PARALLAX_CORRECTION = 2;
    }
}

#[derive(Component, Default)]
pub struct RenderViewLightProbes<C>
where
    C: LightProbeComponent,
{

    pub binding_index_to_textures: Vec<C::AssetId>,

    cubemap_to_binding_index: HashMap<C::AssetId, u32>,

    render_light_probes: Vec<RenderLightProbe>,

    pub main_entity_to_render_light_probe_index: MainEntityHashMap<u32>,

    view_light_probe_info: Option<C::ViewLightProbeInfo>,
}

pub trait LightProbeComponent: Send + Sync + Component + Sized {

    type AssetId: Send + Sync + Clone + Eq + Hash;

    type ViewLightProbeInfo: Send + Sync + Default;

    type QueryData: ReadOnlyQueryData;

    fn id(&self, image_assets: &RenderAssets<GpuImage>) -> Option<Self::AssetId>;

    fn intensity(&self) -> f32;

    fn flags(
        &self,
        query_components: &<Self::QueryData as QueryData>::Item<'_, '_>,
    ) -> RenderLightProbeFlags;

    fn create_render_view_light_probes(
        view_component: Option<&Self>,
        image_assets: &RenderAssets<GpuImage>,
    ) -> RenderViewLightProbes<Self>;

    fn get_world_from_light_matrix(&self, original_world_from_light: &Affine3A) -> Affine3A {
        *original_world_from_light
    }

    fn parallax_correction_bounds(
        &self,
        _query_components: &<Self::QueryData as QueryData>::Item<'_, '_>,
    ) -> Vec3 {
        Vec3::ZERO
    }
}

impl Plugin for LightProbePlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "light_probe.wgsl");
        load_shader_library!(app, "environment_map.wgsl");
        load_shader_library!(app, "irradiance_volume.wgsl");

        app.add_plugins((
            EnvironmentMapGenerationPlugin,
            ExtractInstancesPlugin::<EnvironmentMapIds>::new(),
        ));

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_gpu_resource::<LightProbesBuffer>()
            .add_systems(
                ExtractSchedule,
                gather_light_probes::<EnvironmentMapLight>
                    .before(extract_clusters_for_cpu_clustering)
                    .before(extract_clusters_for_gpu_clustering),
            )
            .add_systems(
                ExtractSchedule,
                gather_light_probes::<IrradianceVolume>
                    .before(extract_clusters_for_cpu_clustering)
                    .before(extract_clusters_for_gpu_clustering),
            )
            .add_systems(
                Render,
                upload_light_probes.in_set(RenderSystems::PrepareResources),
            );
    }
}

fn gather_light_probes<C>(
    image_assets: Res<RenderAssets<GpuImage>>,
    light_probe_query: Extract<Query<(Entity, &GlobalTransform, &LightProbe, &C, C::QueryData)>>,
    view_query: Extract<
        Query<(RenderEntity, &GlobalTransform, &VisibleEntities, Option<&C>), With<Camera3d>>,
    >,
    mut view_light_probe_info: Local<Vec<LightProbeInfo<C>>>,
    mut commands: Commands,
) where
    C: LightProbeComponent,
{

    for (view_entity, view_transform, visible_entities, view_component) in view_query.iter() {
        view_light_probe_info.clear();
        let visible_light_probes = visible_entities.get(TypeId::of::<ClusterVisibilityClass>());
        for &main_entity in visible_light_probes {
            let Ok(query_row) = light_probe_query.get(main_entity) else {

                continue;
            };

            if let Some(light_probe_info) = LightProbeInfo::new(query_row, &image_assets) {
                view_light_probe_info.push(light_probe_info);
            }
        }

        view_light_probe_info.sort_by_cached_key(|light_probe_info| {
            light_probe_info.camera_distance_sort_key(view_transform)
        });

        let mut render_view_light_probes =
            C::create_render_view_light_probes(view_component, &image_assets);

        render_view_light_probes.maybe_gather_light_probes(&view_light_probe_info);

        if render_view_light_probes.is_empty() {
            commands
                .get_entity(view_entity)
                .expect("View entity wasn't synced.")
                .remove::<RenderViewLightProbes<C>>();
        } else {
            commands
                .get_entity(view_entity)
                .expect("View entity wasn't synced.")
                .insert(render_view_light_probes);
        }
    }
}

fn upload_light_probes(
    mut commands: Commands,
    views: Query<Entity, With<ExtractedView>>,
    mut light_probes_buffer: ResMut<LightProbesBuffer>,
    mut view_light_probes_query: Query<(
        Option<&RenderViewLightProbes<EnvironmentMapLight>>,
        Option<&RenderViewLightProbes<IrradianceVolume>>,
    )>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {

    if views.is_empty() {
        return;
    }

    let Some(mut writer) =
        light_probes_buffer.get_writer(views.iter().len(), &render_device, &render_queue)
    else {
        return;
    };

    for view_entity in views.iter() {
        let Ok((render_view_environment_maps, render_view_irradiance_volumes)) =
            view_light_probes_query.get_mut(view_entity)
        else {
            error!("Failed to find `RenderViewLightProbes` for the view!");
            continue;
        };

        let maybe_view_light_probe_info =
            render_view_environment_maps.and_then(|maps| maps.view_light_probe_info.as_ref());
        let mut light_probes_uniform = LightProbesUniform {
            reflection_probes: [RenderLightProbe::default(); MAX_VIEW_LIGHT_PROBES],
            irradiance_volumes: [RenderLightProbe::default(); MAX_VIEW_LIGHT_PROBES],
            reflection_probe_count: render_view_environment_maps
                .map(RenderViewLightProbes::len)
                .unwrap_or_default()
                .min(MAX_VIEW_LIGHT_PROBES) as i32,
            irradiance_volume_count: render_view_irradiance_volumes
                .map(RenderViewLightProbes::len)
                .unwrap_or_default()
                .min(MAX_VIEW_LIGHT_PROBES) as i32,
            view_cubemap_index: match maybe_view_light_probe_info {
                Some(view_light_probe_info) => view_light_probe_info.cubemap_index,
                None => -1,
            },
            smallest_specular_mip_level_for_view: match maybe_view_light_probe_info {
                Some(view_light_probe_info) => view_light_probe_info.smallest_specular_mip_level,
                None => 0,
            },
            intensity_for_view: match maybe_view_light_probe_info {
                Some(view_light_probe_info) => view_light_probe_info.intensity,
                None => 1.0,
            },
            view_environment_map_affects_lightmapped_mesh_diffuse: match maybe_view_light_probe_info
            {
                Some(view_light_probe_info) => {
                    view_light_probe_info.affects_lightmapped_mesh_diffuse as u32
                }
                None => 1,
            },
            view_rotation: match maybe_view_light_probe_info {
                Some(view_light_probe_info) => view_light_probe_info.rotation.inverse().into(),
                None => Quat::IDENTITY.into(),
            },
        };

        if let Some(render_view_environment_maps) = render_view_environment_maps {
            render_view_environment_maps.add_to_uniform(
                &mut light_probes_uniform.reflection_probes,
                &mut light_probes_uniform.reflection_probe_count,
            );
        }

        if let Some(render_view_irradiance_volumes) = render_view_irradiance_volumes {
            render_view_irradiance_volumes.add_to_uniform(
                &mut light_probes_uniform.irradiance_volumes,
                &mut light_probes_uniform.irradiance_volume_count,
            );
        }

        let uniform_offset = writer.write(&light_probes_uniform);

        commands
            .entity(view_entity)
            .insert(ViewLightProbesUniformOffset(uniform_offset));
    }
}

impl Default for LightProbesUniform {
    fn default() -> Self {
        Self {
            reflection_probes: [RenderLightProbe::default(); MAX_VIEW_LIGHT_PROBES],
            irradiance_volumes: [RenderLightProbe::default(); MAX_VIEW_LIGHT_PROBES],
            reflection_probe_count: 0,
            irradiance_volume_count: 0,
            view_cubemap_index: -1,
            smallest_specular_mip_level_for_view: 0,
            intensity_for_view: 1.0,
            view_environment_map_affects_lightmapped_mesh_diffuse: 1,
            view_rotation: Quat::IDENTITY.into(),
        }
    }
}

impl<C> LightProbeInfo<C>
where
    C: LightProbeComponent,
{

    fn new(
        (main_entity, light_probe_transform, light_probe, environment_map, query_components): (
            Entity,
            &GlobalTransform,
            &LightProbe,
            &C,
            <C::QueryData as QueryData>::Item<'_, '_>,
        ),
        image_assets: &RenderAssets<GpuImage>,
    ) -> Option<LightProbeInfo<C>> {
        let world_from_light =
            environment_map.get_world_from_light_matrix(&light_probe_transform.affine());
        let light_from_world_transposed = Mat4::from(world_from_light.inverse()).transpose();
        let bounding_sphere_radius = (world_from_light.matrix3 * Vec3::ONE).length();
        environment_map.id(image_assets).map(|id| LightProbeInfo {
            main_entity: main_entity.into(),
            world_from_light,
            light_from_world: [
                light_from_world_transposed.x_axis,
                light_from_world_transposed.y_axis,
                light_from_world_transposed.z_axis,
            ],
            bounding_sphere_radius,
            falloff: light_probe.falloff,
            parallax_correction_bounds: environment_map
                .parallax_correction_bounds(&query_components),
            asset_id: id,
            intensity: environment_map.intensity(),
            flags: environment_map.flags(&query_components),
        })
    }

    fn camera_distance_sort_key(&self, view_transform: &GlobalTransform) -> FloatOrd {
        FloatOrd(
            (self.world_from_light.translation - view_transform.translation_vec3a())
                .length_squared(),
        )
    }
}

impl<C> RenderViewLightProbes<C>
where
    C: LightProbeComponent,
{

    fn new() -> RenderViewLightProbes<C> {
        RenderViewLightProbes {
            binding_index_to_textures: vec![],
            cubemap_to_binding_index: HashMap::default(),
            main_entity_to_render_light_probe_index: HashMap::default(),
            render_light_probes: vec![],
            view_light_probe_info: None,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.render_light_probes.is_empty() && self.view_light_probe_info.is_none()
    }

    pub(crate) fn len(&self) -> usize {
        self.render_light_probes.len()
    }

    pub(crate) fn get_or_insert_cubemap(&mut self, cubemap_id: &C::AssetId) -> u32 {
        *self
            .cubemap_to_binding_index
            .entry((*cubemap_id).clone())
            .or_insert_with(|| {
                let index = self.binding_index_to_textures.len() as u32;
                self.binding_index_to_textures.push((*cubemap_id).clone());
                index
            })
    }

    fn add_to_uniform(
        &self,
        render_light_probes: &mut [RenderLightProbe; MAX_VIEW_LIGHT_PROBES],
        render_light_probe_count: &mut i32,
    ) {
        render_light_probes[0..self.render_light_probes.len()]
            .copy_from_slice(&self.render_light_probes[..]);
        *render_light_probe_count = self.render_light_probes.len() as i32;
    }

    fn maybe_gather_light_probes(&mut self, light_probes: &[LightProbeInfo<C>]) {
        for light_probe in light_probes.iter().take(MAX_VIEW_LIGHT_PROBES) {

            let cubemap_index = self.get_or_insert_cubemap(&light_probe.asset_id);

            let render_light_probe_index = self.render_light_probes.len() as u32;
            debug_assert!((render_light_probe_index as usize) < MAX_VIEW_LIGHT_PROBES);
            self.main_entity_to_render_light_probe_index
                .insert(light_probe.main_entity, render_light_probe_index);

            self.render_light_probes.push(RenderLightProbe {
                light_from_world_transposed: light_probe.light_from_world,
                falloff: light_probe.falloff,
                parallax_correction_bounds: light_probe.parallax_correction_bounds,
                world_position: light_probe.world_from_light.translation.into(),
                bounding_sphere_radius: light_probe.bounding_sphere_radius,
                texture_index: cubemap_index as i32,
                intensity: light_probe.intensity,
                flags: light_probe.flags.bits() as u32,
            });
        }
    }
}

impl<C> Clone for LightProbeInfo<C>
where
    C: LightProbeComponent,
{
    fn clone(&self) -> Self {
        Self {
            main_entity: self.main_entity,
            light_from_world: self.light_from_world,
            world_from_light: self.world_from_light,
            falloff: self.falloff,
            parallax_correction_bounds: self.parallax_correction_bounds,
            bounding_sphere_radius: self.bounding_sphere_radius,
            intensity: self.intensity,
            flags: self.flags,
            asset_id: self.asset_id.clone(),
        }
    }
}

pub(crate) fn add_cubemap_texture_view<'a>(
    texture_views: &mut Vec<&'a <TextureView as Deref>::Target>,
    sampler: &mut Option<&'a Sampler>,
    image_id: AssetId<Image>,
    images: &'a RenderAssets<GpuImage>,
    fallback_image: &'a FallbackImage,
) {
    match images.get(image_id) {
        None => {

            texture_views.push(&*fallback_image.cube.texture_view);
        }
        Some(image) => {

            if sampler.is_none() {
                *sampler = Some(&image.sampler);
            }

            texture_views.push(&*image.texture_view);
        }
    }
}

pub(crate) fn binding_arrays_are_usable(
    render_device: &RenderDevice,
    render_adapter: &RenderAdapter,
) -> bool {
    let adapter_info = RenderAdapterInfo(WgpuWrapper::new(render_adapter.get_info()));

    !cfg!(feature = "shader_format_glsl")
        && bevy_render::get_adreno_model(&adapter_info).is_none_or(|model| model > 610)
        && render_device
            .limits()
            .max_binding_array_elements_per_shader_stage
            >= (STANDARD_MATERIAL_FRAGMENT_SHADER_MIN_TEXTURE_BINDINGS + MAX_VIEW_LIGHT_PROBES)
                as u32
        && render_device
            .limits()
            .max_binding_array_sampler_elements_per_shader_stage
            >= (STANDARD_MATERIAL_FRAGMENT_SHADER_MIN_TEXTURE_BINDINGS + MAX_VIEW_LIGHT_PROBES)
                as u32
        && render_device.features().contains(
            WgpuFeatures::TEXTURE_BINDING_ARRAY
                | WgpuFeatures::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
        )
}
