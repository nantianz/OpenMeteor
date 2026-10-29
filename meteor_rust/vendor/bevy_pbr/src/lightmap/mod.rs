

use bevy_app::{App, Plugin};
use bevy_asset::{AssetId, Handle};
use bevy_camera::visibility::ViewVisibility;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    lifecycle::RemovedComponents,
    query::{Changed, Or},
    reflect::ReflectComponent,
    resource::Resource,
    schedule::IntoScheduleConfigs,
    system::{Commands, Query, Res, ResMut},
    template::FromTemplate,
};
use bevy_image::Image;
use bevy_math::{uvec2, vec4, Rect, UVec2};
use bevy_platform::collections::HashSet;
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use bevy_render::{
    render_asset::RenderAssets,
    render_resource::{Sampler, TextureView, WgpuSampler, WgpuTextureView},
    renderer::RenderAdapter,
    sync_world::MainEntity,
    texture::{FallbackImage, GpuImage},
    Extract, ExtractSchedule, RenderApp, RenderStartup,
};
use bevy_render::{renderer::RenderDevice, sync_world::MainEntityHashMap};
use bevy_shader::load_shader_library;
use bevy_utils::default;
use fixedbitset::FixedBitSet;
use nonmax::{NonMaxU16, NonMaxU32};
use tracing::error;

use crate::{binding_arrays_are_usable, MeshExtractionSystems};

pub const LIGHTMAPS_PER_SLAB: usize = 4;

pub struct LightmapPlugin;

#[derive(Component, Clone, Reflect, FromTemplate)]
#[reflect(Component, Default, Clone)]
pub struct Lightmap {

    pub image: Handle<Image>,

    pub uv_rect: Rect,

    pub bicubic_sampling: bool,
}

#[derive(Debug)]
pub(crate) struct RenderLightmap {

    pub(crate) uv_rect: Rect,

    pub(crate) slab_index: LightmapSlabIndex,

    pub(crate) slot_index: LightmapSlotIndex,

    pub(crate) bicubic_sampling: bool,
}

#[derive(Resource)]
pub struct RenderLightmaps {

    pub(crate) render_lightmaps: MainEntityHashMap<RenderLightmap>,

    pub(crate) slabs: Vec<LightmapSlab>,

    free_slabs: FixedBitSet,

    pending_lightmaps: HashSet<(LightmapSlabIndex, LightmapSlotIndex)>,

    pub(crate) bindless_supported: bool,
}

pub struct LightmapSlab {

    lightmaps: Vec<AllocatedLightmap>,
    free_slots_bitmask: u32,
}

struct AllocatedLightmap {
    gpu_image: GpuImage,

    asset_id: Option<AssetId<Image>>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Deref, DerefMut)]
#[repr(transparent)]
pub struct LightmapSlabIndex(pub(crate) NonMaxU32);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Deref, DerefMut)]
#[repr(transparent)]
pub struct LightmapSlotIndex(pub(crate) NonMaxU16);

impl Plugin for LightmapPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "lightmap.wgsl");

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_systems(RenderStartup, init_render_lightmaps)
            .add_systems(
                ExtractSchedule,
                extract_lightmaps.after(MeshExtractionSystems),
            );
    }
}

fn extract_lightmaps(
    render_lightmaps: ResMut<RenderLightmaps>,
    changed_lightmaps_query: Extract<
        Query<
            (Entity, &ViewVisibility, &Lightmap),
            Or<(Changed<ViewVisibility>, Changed<Lightmap>)>,
        >,
    >,
    mut removed_lightmaps_query: Extract<RemovedComponents<Lightmap>>,
    images: Res<RenderAssets<GpuImage>>,
    fallback_images: Res<FallbackImage>,
) {
    let render_lightmaps = render_lightmaps.into_inner();

    for (entity, view_visibility, lightmap) in changed_lightmaps_query.iter() {
        if render_lightmaps
            .render_lightmaps
            .contains_key(&MainEntity::from(entity))
        {
            continue;
        }

        if !view_visibility.get() {
            continue;
        }

        let (slab_index, slot_index) =
            render_lightmaps.allocate(&fallback_images, lightmap.image.id());
        render_lightmaps.render_lightmaps.insert(
            entity.into(),
            RenderLightmap::new(
                lightmap.uv_rect,
                slab_index,
                slot_index,
                lightmap.bicubic_sampling,
            ),
        );

        render_lightmaps
            .pending_lightmaps
            .insert((slab_index, slot_index));
    }

    for entity in removed_lightmaps_query.read() {
        if changed_lightmaps_query.contains(entity) {
            continue;
        }

        let Some(RenderLightmap {
            slab_index,
            slot_index,
            ..
        }) = render_lightmaps
            .render_lightmaps
            .remove(&MainEntity::from(entity))
        else {
            continue;
        };

        render_lightmaps.remove(&fallback_images, slab_index, slot_index);
        render_lightmaps
            .pending_lightmaps
            .remove(&(slab_index, slot_index));
    }

    render_lightmaps
        .pending_lightmaps
        .retain(|&(slab_index, slot_index)| {
            let Some(asset_id) = render_lightmaps.slabs[usize::from(slab_index)].lightmaps
                [usize::from(slot_index)]
            .asset_id
            else {
                error!(
                    "Allocated lightmap should have been removed from `pending_lightmaps` by now"
                );
                return false;
            };

            let Some(gpu_image) = images.get(asset_id) else {
                return true;
            };
            render_lightmaps.slabs[usize::from(slab_index)].insert(slot_index, gpu_image.clone());
            false
        });
}

impl RenderLightmap {

    fn new(
        uv_rect: Rect,
        slab_index: LightmapSlabIndex,
        slot_index: LightmapSlotIndex,
        bicubic_sampling: bool,
    ) -> Self {
        Self {
            uv_rect,
            slab_index,
            slot_index,
            bicubic_sampling,
        }
    }
}

pub(crate) fn pack_lightmap_uv_rect(maybe_rect: Option<Rect>) -> UVec2 {
    match maybe_rect {
        Some(rect) => {
            let rect_uvec4 = (vec4(rect.min.x, rect.min.y, rect.max.x, rect.max.y) * 65535.0)
                .round()
                .as_uvec4();
            uvec2(
                rect_uvec4.x | (rect_uvec4.y << 16),
                rect_uvec4.z | (rect_uvec4.w << 16),
            )
        }
        None => UVec2::ZERO,
    }
}

impl Default for Lightmap {
    fn default() -> Self {
        Self {
            image: Default::default(),
            uv_rect: Rect::new(0.0, 0.0, 1.0, 1.0),
            bicubic_sampling: false,
        }
    }
}

pub fn init_render_lightmaps(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_adapter: Res<RenderAdapter>,
) {
    let bindless_supported = binding_arrays_are_usable(&render_device, &render_adapter);

    commands.insert_resource(RenderLightmaps {
        render_lightmaps: default(),
        slabs: vec![],
        free_slabs: FixedBitSet::new(),
        pending_lightmaps: default(),
        bindless_supported,
    });
}

impl RenderLightmaps {

    fn create_slab(&mut self, fallback_images: &FallbackImage) -> LightmapSlabIndex {
        let slab_index = LightmapSlabIndex::from(self.slabs.len());
        self.free_slabs.grow_and_insert(slab_index.into());
        self.slabs
            .push(LightmapSlab::new(fallback_images, self.bindless_supported));
        slab_index
    }

    fn allocate(
        &mut self,
        fallback_images: &FallbackImage,
        image_id: AssetId<Image>,
    ) -> (LightmapSlabIndex, LightmapSlotIndex) {
        let slab_index = match self.free_slabs.minimum() {
            None => self.create_slab(fallback_images),
            Some(slab_index) => slab_index.into(),
        };

        let slab = &mut self.slabs[usize::from(slab_index)];
        let slot_index = slab.allocate(image_id);
        if slab.is_full() {
            self.free_slabs.remove(slab_index.into());
        }

        (slab_index, slot_index)
    }

    fn remove(
        &mut self,
        fallback_images: &FallbackImage,
        slab_index: LightmapSlabIndex,
        slot_index: LightmapSlotIndex,
    ) {
        let slab = &mut self.slabs[usize::from(slab_index)];
        slab.remove(fallback_images, slot_index);

        if !slab.is_full() {
            self.free_slabs.grow_and_insert(slab_index.into());
        }
    }
}

impl LightmapSlab {
    fn new(fallback_images: &FallbackImage, bindless_supported: bool) -> LightmapSlab {
        let count = if bindless_supported {
            LIGHTMAPS_PER_SLAB
        } else {
            1
        };

        LightmapSlab {
            lightmaps: (0..count)
                .map(|_| AllocatedLightmap {
                    gpu_image: fallback_images.d2.clone(),
                    asset_id: None,
                })
                .collect(),
            free_slots_bitmask: (1 << count) - 1,
        }
    }

    fn is_full(&self) -> bool {
        self.free_slots_bitmask == 0
    }

    fn allocate(&mut self, image_id: AssetId<Image>) -> LightmapSlotIndex {
        assert!(
            !self.is_full(),
            "Attempting to allocate on a full lightmap slab"
        );
        let index = LightmapSlotIndex::from(self.free_slots_bitmask.trailing_zeros());
        self.free_slots_bitmask &= !(1 << u32::from(index));
        self.lightmaps[usize::from(index)].asset_id = Some(image_id);
        index
    }

    fn insert(&mut self, index: LightmapSlotIndex, gpu_image: GpuImage) {
        self.lightmaps[usize::from(index)] = AllocatedLightmap {
            gpu_image,
            asset_id: None,
        }
    }

    fn remove(&mut self, fallback_images: &FallbackImage, index: LightmapSlotIndex) {
        self.lightmaps[usize::from(index)] = AllocatedLightmap {
            gpu_image: fallback_images.d2.clone(),
            asset_id: None,
        };
        self.free_slots_bitmask |= 1 << u32::from(index);
    }

    pub(crate) fn build_binding_arrays(&self) -> (Vec<&WgpuTextureView>, Vec<&WgpuSampler>) {
        (
            self.lightmaps
                .iter()
                .map(|allocated_lightmap| &*allocated_lightmap.gpu_image.texture_view)
                .collect(),
            self.lightmaps
                .iter()
                .map(|allocated_lightmap| &*allocated_lightmap.gpu_image.sampler)
                .collect(),
        )
    }

    pub(crate) fn bindings_for_first_lightmap(&self) -> (&TextureView, &Sampler) {
        (
            &self.lightmaps[0].gpu_image.texture_view,
            &self.lightmaps[0].gpu_image.sampler,
        )
    }
}

impl From<u32> for LightmapSlabIndex {
    fn from(value: u32) -> Self {
        Self(NonMaxU32::new(value).unwrap())
    }
}

impl From<usize> for LightmapSlabIndex {
    fn from(value: usize) -> Self {
        Self::from(value as u32)
    }
}

impl From<u32> for LightmapSlotIndex {
    fn from(value: u32) -> Self {
        Self(NonMaxU16::new(value as u16).unwrap())
    }
}

impl From<usize> for LightmapSlotIndex {
    fn from(value: usize) -> Self {
        Self::from(value as u32)
    }
}

impl From<LightmapSlabIndex> for usize {
    fn from(value: LightmapSlabIndex) -> Self {
        value.0.get() as usize
    }
}

impl From<LightmapSlotIndex> for usize {
    fn from(value: LightmapSlotIndex) -> Self {
        value.0.get() as usize
    }
}

impl From<LightmapSlotIndex> for u16 {
    fn from(value: LightmapSlotIndex) -> Self {
        value.0.get()
    }
}

impl From<LightmapSlotIndex> for u32 {
    fn from(value: LightmapSlotIndex) -> Self {
        value.0.get() as u32
    }
}
