use core::ops::Range;

use bevy_camera::{Camera, Camera3d};
use bevy_core_pipeline::core_3d::TransparentSortingInfo3d;
use bevy_ecs::{
    entity::{Entity, EntityHash},
    query::With,
    system::{Local, Query, ResMut},
};
use bevy_material::{descriptor::CachedRenderPipelineId, labels::DrawFunctionId};
use bevy_math::FloatOrd;
use bevy_platform::collections::HashSet;
use bevy_render::{
    render_phase::{
        CachedRenderPipelinePhaseItem, PhaseItem, PhaseItemExtraIndex, SortedPhaseItem,
        ViewSortedRenderPhases,
    },
    sync_world::MainEntity,
    view::{ExtractedView, RetainedViewEntity},
    Extract,
};
use indexmap::IndexMap;

pub struct Transmissive3d {
    pub sorting_info: TransparentSortingInfo3d,
    pub distance: f32,
    pub pipeline: CachedRenderPipelineId,
    pub entity: (Entity, MainEntity),
    pub draw_function: DrawFunctionId,
    pub batch_range: Range<u32>,
    pub extra_index: PhaseItemExtraIndex,

    pub indexed: bool,
}

impl PhaseItem for Transmissive3d {

    const AUTOMATIC_BATCHING: bool = false;

    #[inline]
    fn entity(&self) -> Entity {
        self.entity.0
    }

    #[inline]
    fn main_entity(&self) -> MainEntity {
        self.entity.1
    }

    #[inline]
    fn draw_function(&self) -> DrawFunctionId {
        self.draw_function
    }

    #[inline]
    fn batch_range(&self) -> &Range<u32> {
        &self.batch_range
    }

    #[inline]
    fn batch_range_mut(&mut self) -> &mut Range<u32> {
        &mut self.batch_range
    }

    #[inline]
    fn extra_index(&self) -> PhaseItemExtraIndex {
        self.extra_index.clone()
    }

    #[inline]
    fn batch_range_and_extra_index_mut(&mut self) -> (&mut Range<u32>, &mut PhaseItemExtraIndex) {
        (&mut self.batch_range, &mut self.extra_index)
    }
}

impl SortedPhaseItem for Transmissive3d {

    type SortKey = FloatOrd;

    #[inline]
    fn sort_key(&self) -> Self::SortKey {
        FloatOrd(self.distance)
    }

    #[inline]
    fn sort(items: &mut IndexMap<(Entity, MainEntity), Transmissive3d, EntityHash>) {
        items.sort_by_key(|_, item| item.sort_key());
    }

    fn recalculate_sort_keys(
        items: &mut IndexMap<(Entity, MainEntity), Self, EntityHash>,
        view: &ExtractedView,
    ) {

        let rangefinder = view.rangefinder3d();
        for item in items.values_mut() {
            item.distance = item.sorting_info.sort_distance(&rangefinder);
        }
    }

    #[inline]
    fn indexed(&self) -> bool {
        self.indexed
    }
}

impl CachedRenderPipelinePhaseItem for Transmissive3d {
    #[inline]
    fn cached_pipeline(&self) -> CachedRenderPipelineId {
        self.pipeline
    }
}

pub fn extract_transmissive_camera_phases(
    mut transmissive_3d_phases: ResMut<ViewSortedRenderPhases<Transmissive3d>>,
    cameras: Extract<Query<(Entity, &Camera), With<Camera3d>>>,
    mut live_entities: Local<HashSet<RetainedViewEntity>>,
) {
    live_entities.clear();

    for (main_entity, camera) in &cameras {
        if !camera.is_active {
            continue;
        }

        let retained_view_entity = RetainedViewEntity::new(main_entity.into(), None, 0);

        transmissive_3d_phases.prepare_for_new_frame(retained_view_entity);
        live_entities.insert(retained_view_entity);
    }

    transmissive_3d_phases.retain(|view_entity, _| live_entities.contains(view_entity));
}
