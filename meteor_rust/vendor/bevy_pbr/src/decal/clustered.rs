

use core::{num::NonZero, ops::Deref};

use bevy_app::{App, Plugin};
use bevy_asset::{AssetId, Handle};
use bevy_camera::visibility::ViewVisibility;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    entity::{Entity, EntityHashMap},
    query::With,
    resource::Resource,
    schedule::IntoScheduleConfigs as _,
    system::{Commands, Local, Query, Res, ResMut},
};
use bevy_image::Image;
use bevy_light::{ClusteredDecal, DirectionalLightTexture, PointLightTexture, SpotLightTexture};
use bevy_math::{Mat4, Vec3};
use bevy_platform::collections::HashMap;
use bevy_render::{
    render_asset::RenderAssets,
    render_resource::{
        binding_types, BindGroupLayoutEntryBuilder, Buffer, BufferUsages, RawBufferVec, Sampler,
        SamplerBindingType, ShaderType, TextureSampleType, TextureView,
    },
    renderer::{RenderAdapter, RenderDevice, RenderQueue},
    settings::WgpuFeatures,
    sync_component::{SyncComponent, SyncComponentPlugin},
    sync_world::RenderEntity,
    texture::{FallbackImage, GpuImage},
    Extract, ExtractSchedule, GpuResourceAppExt, Render, RenderApp, RenderSystems,
};
use bevy_shader::load_shader_library;
use bevy_transform::components::GlobalTransform;
use bytemuck::{Pod, Zeroable};

use crate::{binding_arrays_are_usable, prepare_lights, GlobalClusterableObjectMeta};

const IMAGES_PER_DECAL: usize = 4;

pub struct ClusteredDecalPlugin;

#[derive(Resource, Default)]
pub struct RenderClusteredDecals {

    pub binding_index_to_textures: Vec<AssetId<Image>>,

    texture_to_binding_index: HashMap<AssetId<Image>, i32>,

    decals: Vec<RenderClusteredDecal>,

    entity_to_decal_index: EntityHashMap<usize>,
}

impl RenderClusteredDecals {

    fn clear(&mut self) {
        self.binding_index_to_textures.clear();
        self.texture_to_binding_index.clear();
        self.decals.clear();
        self.entity_to_decal_index.clear();
    }

    pub fn insert_decal(
        &mut self,
        entity: Entity,
        images: [Option<AssetId<Image>>; IMAGES_PER_DECAL],
        local_from_world: Mat4,
        world_position: Vec3,
        bounding_sphere_radius: f32,
        tag: u32,
    ) {
        let image_indices = images.map(|maybe_image_id| match maybe_image_id {
            Some(ref image_id) => self.get_or_insert_image(image_id),
            None => -1,
        });
        let decal_index = self.decals.len();
        self.decals.push(RenderClusteredDecal {
            local_from_world,
            image_indices,
            world_position,
            bounding_sphere_radius,
            tag,
            pad_a: 0,
            pad_b: 0,
            pad_c: 0,
        });
        self.entity_to_decal_index.insert(entity, decal_index);
    }

    pub fn get(&self, entity: Entity) -> Option<usize> {
        self.entity_to_decal_index.get(&entity).copied()
    }

    pub fn len(&self) -> usize {
        self.decals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.decals.is_empty()
    }
}

pub(crate) struct RenderViewClusteredDecalBindGroupEntries<'a> {

    pub(crate) decals: &'a Buffer,

    pub(crate) texture_views: Vec<&'a <TextureView as Deref>::Target>,

    pub(crate) sampler: &'a Sampler,
}

#[derive(Resource, Deref, DerefMut)]
pub struct DecalsBuffer(RawBufferVec<RenderClusteredDecal>);

impl Default for DecalsBuffer {
    fn default() -> Self {
        DecalsBuffer(RawBufferVec::new(BufferUsages::STORAGE))
    }
}

impl Plugin for ClusteredDecalPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "clustered.wgsl");

        app.add_plugins(SyncComponentPlugin::<ClusteredDecal, Self>::default());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_gpu_resource::<DecalsBuffer>()
            .init_resource::<RenderClusteredDecals>()
            .add_systems(ExtractSchedule, (extract_decals, extract_clustered_decal))
            .add_systems(
                Render,
                prepare_decals
                    .in_set(RenderSystems::PrepareViews)
                    .after(prepare_lights),
            )
            .add_systems(
                Render,
                upload_decals.in_set(RenderSystems::PrepareResources),
            );
    }
}

impl SyncComponent<ClusteredDecalPlugin> for ClusteredDecal {
    type Target = Self;
}

fn extract_clustered_decal(
    mut commands: Commands,
    mut previous_len: Local<usize>,
    query: Extract<Query<(RenderEntity, &ClusteredDecal)>>,
) {
    let mut values = Vec::with_capacity(*previous_len);
    for (entity, query_item) in &query {
        values.push((entity, query_item.clone()));
    }
    *previous_len = values.len();
    commands.try_insert_batch(values);
}

#[derive(Clone, Copy, Default, ShaderType, Pod, Zeroable)]
#[repr(C)]
pub struct RenderClusteredDecal {

    local_from_world: Mat4,

    image_indices: [i32; 4],
    world_position: Vec3,
    bounding_sphere_radius: f32,

    tag: u32,

    pad_a: u32,

    pad_b: u32,

    pad_c: u32,
}

pub fn extract_decals(
    decals: Extract<
        Query<(
            RenderEntity,
            &ClusteredDecal,
            &GlobalTransform,
            &ViewVisibility,
        )>,
    >,
    spot_light_textures: Extract<
        Query<(
            RenderEntity,
            &SpotLightTexture,
            &GlobalTransform,
            &ViewVisibility,
        )>,
    >,
    point_light_textures: Extract<
        Query<(
            RenderEntity,
            &PointLightTexture,
            &GlobalTransform,
            &ViewVisibility,
        )>,
    >,
    directional_light_textures: Extract<
        Query<(
            RenderEntity,
            &DirectionalLightTexture,
            &GlobalTransform,
            &ViewVisibility,
        )>,
    >,
    mut render_decals: ResMut<RenderClusteredDecals>,
) {

    render_decals.clear();

    extract_clustered_decals(&decals, &mut render_decals);
    extract_spot_light_textures(&spot_light_textures, &mut render_decals);
    extract_point_light_textures(&point_light_textures, &mut render_decals);
    extract_directional_light_textures(&directional_light_textures, &mut render_decals);
}

fn extract_clustered_decals(
    decals: &Extract<
        Query<(
            RenderEntity,
            &ClusteredDecal,
            &GlobalTransform,
            &ViewVisibility,
        )>,
    >,
    render_decals: &mut RenderClusteredDecals,
) {

    for (decal_entity, clustered_decal, global_transform, view_visibility) in decals {

        if !view_visibility.get() {
            continue;
        }

        render_decals.insert_decal(
            decal_entity,
            [
                clustered_decal.base_color_texture.as_ref().map(Handle::id),
                clustered_decal.normal_map_texture.as_ref().map(Handle::id),
                clustered_decal
                    .metallic_roughness_texture
                    .as_ref()
                    .map(Handle::id),
                clustered_decal.emissive_texture.as_ref().map(Handle::id),
            ],
            global_transform.affine().inverse().into(),
            global_transform.translation(),
            (global_transform.scale() * Vec3::ONE).length(),
            clustered_decal.tag,
        );
    }
}

fn extract_spot_light_textures(
    spot_light_textures: &Extract<
        Query<(
            RenderEntity,
            &SpotLightTexture,
            &GlobalTransform,
            &ViewVisibility,
        )>,
    >,
    render_decals: &mut RenderClusteredDecals,
) {
    for (decal_entity, texture, global_transform, view_visibility) in spot_light_textures {

        if !view_visibility.get() {
            continue;
        }

        render_decals.insert_decal(
            decal_entity,
            [Some(texture.image.id()), None, None, None],
            global_transform.affine().inverse().into(),
            global_transform.translation(),
            (global_transform.scale() * Vec3::ONE).length(),
            0,
        );
    }
}

fn extract_point_light_textures(
    point_light_textures: &Extract<
        Query<(
            RenderEntity,
            &PointLightTexture,
            &GlobalTransform,
            &ViewVisibility,
        )>,
    >,
    render_decals: &mut RenderClusteredDecals,
) {
    for (decal_entity, texture, global_transform, view_visibility) in point_light_textures {

        if !view_visibility.get() {
            continue;
        }

        render_decals.insert_decal(
            decal_entity,
            [Some(texture.image.id()), None, None, None],
            global_transform.affine().inverse().into(),
            global_transform.translation(),
            (global_transform.scale() * Vec3::ONE).length(),
            texture.cubemap_layout as u32,
        );
    }
}

fn extract_directional_light_textures(
    directional_light_textures: &Extract<
        Query<(
            RenderEntity,
            &DirectionalLightTexture,
            &GlobalTransform,
            &ViewVisibility,
        )>,
    >,
    render_decals: &mut RenderClusteredDecals,
) {
    for (decal_entity, texture, global_transform, view_visibility) in directional_light_textures {

        if !view_visibility.get() {
            continue;
        }

        render_decals.insert_decal(
            decal_entity,
            [Some(texture.image.id()), None, None, None],
            global_transform.affine().inverse().into(),
            global_transform.translation(),
            (global_transform.scale() * Vec3::ONE).length(),
            if texture.tiled { 1 } else { 0 },
        );
    }
}

fn prepare_decals(
    decals: Query<Entity, With<ClusteredDecal>>,
    mut global_clusterable_object_meta: ResMut<GlobalClusterableObjectMeta>,
    render_decals: Res<RenderClusteredDecals>,
) {
    for decal_entity in &decals {
        if let Some(index) = render_decals.entity_to_decal_index.get(&decal_entity) {
            global_clusterable_object_meta
                .entity_to_index
                .insert(decal_entity, *index);
        }
    }
}

pub(crate) fn get_bind_group_layout_entries(
    render_device: &RenderDevice,
    render_adapter: &RenderAdapter,
) -> Option<[BindGroupLayoutEntryBuilder; 3]> {

    if !clustered_decals_are_usable(render_device, render_adapter) {
        return None;
    }

    Some([

        binding_types::storage_buffer_read_only::<RenderClusteredDecal>(false),

        binding_types::texture_2d(TextureSampleType::Float { filterable: true })
            .count(NonZero::<u32>::new(max_view_decals(render_device)).unwrap()),

        binding_types::sampler(SamplerBindingType::Filtering),
    ])
}

impl<'a> RenderViewClusteredDecalBindGroupEntries<'a> {

    pub(crate) fn get(
        render_decals: &RenderClusteredDecals,
        decals_buffer: &'a DecalsBuffer,
        images: &'a RenderAssets<GpuImage>,
        fallback_image: &'a FallbackImage,
        render_device: &RenderDevice,
        render_adapter: &RenderAdapter,
    ) -> Option<RenderViewClusteredDecalBindGroupEntries<'a>> {

        if !clustered_decals_are_usable(render_device, render_adapter) {
            return None;
        }

        let sampler = match render_decals
            .binding_index_to_textures
            .iter()
            .filter_map(|image_id| images.get(*image_id))
            .next()
        {
            Some(gpu_image) => &gpu_image.sampler,
            None => &fallback_image.d2.sampler,
        };

        let mut texture_views = vec![];
        for image_id in &render_decals.binding_index_to_textures {
            match images.get(*image_id) {
                None => texture_views.push(&*fallback_image.d2.texture_view),
                Some(gpu_image) => texture_views.push(&*gpu_image.texture_view),
            }
        }

        if !render_device
            .features()
            .contains(WgpuFeatures::PARTIALLY_BOUND_BINDING_ARRAY)
        {
            let max_view_decals = max_view_decals(render_device);
            while texture_views.len() < max_view_decals as usize {
                texture_views.push(&*fallback_image.d2.texture_view);
            }
        } else if texture_views.is_empty() {
            texture_views.push(&*fallback_image.d2.texture_view);
        }

        Some(RenderViewClusteredDecalBindGroupEntries {
            decals: decals_buffer.buffer()?,
            texture_views,
            sampler,
        })
    }
}

impl RenderClusteredDecals {

    fn get_or_insert_image(&mut self, image_id: &AssetId<Image>) -> i32 {
        *self
            .texture_to_binding_index
            .entry(*image_id)
            .or_insert_with(|| {
                let index = self.binding_index_to_textures.len() as i32;
                self.binding_index_to_textures.push(*image_id);
                index
            })
    }
}

fn upload_decals(
    render_decals: Res<RenderClusteredDecals>,
    mut decals_buffer: ResMut<DecalsBuffer>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    decals_buffer.clear();

    for &decal in &render_decals.decals {
        decals_buffer.push(decal);
    }

    if decals_buffer.is_empty() {
        decals_buffer.push(RenderClusteredDecal::default());
    }

    decals_buffer.write_buffer(&render_device, &render_queue);
}

pub fn clustered_decals_are_usable(
    render_device: &RenderDevice,
    render_adapter: &RenderAdapter,
) -> bool {

    binding_arrays_are_usable(render_device, render_adapter)
        && cfg!(feature = "pbr_clustered_decals")
}

fn max_view_decals(render_device: &RenderDevice) -> u32 {

    if render_device
        .features()
        .contains(WgpuFeatures::PARTIALLY_BOUND_BINDING_ARRAY)
    {

        1024
    } else {
        8
    }
}
