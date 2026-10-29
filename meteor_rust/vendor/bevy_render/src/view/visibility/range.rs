

use super::VisibilityRange;
use bevy_app::{App, Plugin};
use bevy_ecs::{
    entity::Entity,
    lifecycle::RemovedComponents,
    query::Changed,
    resource::Resource,
    schedule::IntoScheduleConfigs as _,
    system::{Query, Res, ResMut},
};
use bevy_log::warn_once;
use bevy_math::{vec4, Vec4};
use bevy_platform::collections::HashMap;
use bevy_utils::prelude::default;
use nonmax::NonMaxU16;
use wgpu::{BufferBindingType, BufferUsages};

use crate::{
    render_resource::BufferVec,
    renderer::{RenderDevice, RenderQueue},
    sync_world::{MainEntity, MainEntityHashMap},
    Extract, ExtractSchedule, GpuResourceAppExt, Render, RenderApp, RenderSystems,
};

pub const VISIBILITY_RANGES_STORAGE_BUFFER_COUNT: u32 = 4;

const VISIBILITY_RANGE_UNIFORM_BUFFER_SIZE: usize = 64;

pub struct RenderVisibilityRangePlugin;

impl Plugin for RenderVisibilityRangePlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_gpu_resource::<RenderVisibilityRanges>()
            .add_systems(ExtractSchedule, extract_visibility_ranges)
            .add_systems(
                Render,
                write_render_visibility_ranges.in_set(RenderSystems::PrepareResourcesFlush),
            );
    }
}

#[derive(Resource)]
pub struct RenderVisibilityRanges {

    entities: MainEntityHashMap<RenderVisibilityEntityInfo>,

    range_to_index: HashMap<VisibilityRange, NonMaxU16>,

    buffer: BufferVec<Vec4>,

    buffer_dirty: bool,
}

struct RenderVisibilityEntityInfo {

    buffer_index: NonMaxU16,

    is_abrupt: bool,
}

impl Default for RenderVisibilityRanges {
    fn default() -> Self {
        Self {
            entities: default(),
            range_to_index: default(),
            buffer: BufferVec::new(
                BufferUsages::STORAGE | BufferUsages::UNIFORM | BufferUsages::VERTEX,
            ),
            buffer_dirty: true,
        }
    }
}

impl RenderVisibilityRanges {

    fn clear(&mut self) {
        self.entities.clear();
    }

    fn insert(&mut self, entity: MainEntity, visibility_range: &VisibilityRange) {

        let buffer_index = match self.range_to_index.get(visibility_range) {
            Some(index) => *index,
            None => {

                let index = u16::try_from(self.range_to_index.len())
                    .ok()
                    .and_then(|next| NonMaxU16::try_from(next).ok())
                    .unwrap_or_else(|| {
                        warn_once!(
                            "More than {} distinct `VisibilityRange`s are in use; \
                             additional ranges will share GPU slot 0 and may be \
                             culled or crossfaded incorrectly.",
                            u16::MAX
                        );
                        NonMaxU16::default()
                    });
                self.range_to_index.insert(visibility_range.clone(), index);
                self.buffer_dirty = true;
                index
            }
        };

        self.entities.insert(
            entity,
            RenderVisibilityEntityInfo {
                buffer_index,
                is_abrupt: visibility_range.is_abrupt(),
            },
        );
    }

    fn rebuild_buffer_if_dirty(&mut self) {
        if !self.buffer_dirty {
            return;
        }
        let mut ordered = vec![Vec4::ZERO; self.range_to_index.len()];
        for (range, index) in &self.range_to_index {
            ordered[index.get() as usize] = vec4(
                range.start_margin.start,
                range.start_margin.end,
                range.end_margin.start,
                range.end_margin.end,
            );
        }
        self.buffer.clear();
        for value in ordered {
            self.buffer.push(value);
        }
    }

    #[inline]
    pub fn lod_index_for_entity(&self, entity: MainEntity) -> Option<NonMaxU16> {
        self.entities.get(&entity).map(|info| info.buffer_index)
    }

    #[inline]
    pub fn entity_has_crossfading_visibility_ranges(&self, entity: MainEntity) -> bool {
        self.entities
            .get(&entity)
            .is_some_and(|info| !info.is_abrupt)
    }

    #[inline]
    pub fn buffer(&self) -> &BufferVec<Vec4> {
        &self.buffer
    }
}

pub fn extract_visibility_ranges(
    mut render_visibility_ranges: ResMut<RenderVisibilityRanges>,
    visibility_ranges_query: Extract<Query<(Entity, &VisibilityRange)>>,
    changed_ranges_query: Extract<Query<Entity, Changed<VisibilityRange>>>,
    mut removed_visibility_ranges: Extract<RemovedComponents<VisibilityRange>>,
) {
    if changed_ranges_query.is_empty() && removed_visibility_ranges.read().next().is_none() {
        return;
    }

    render_visibility_ranges.clear();
    for (entity, visibility_range) in visibility_ranges_query.iter() {
        render_visibility_ranges.insert(entity.into(), visibility_range);
    }
    render_visibility_ranges.rebuild_buffer_if_dirty();
}

pub fn write_render_visibility_ranges(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut render_visibility_ranges: ResMut<RenderVisibilityRanges>,
) {

    if !render_visibility_ranges.buffer_dirty {
        return;
    }

    match render_device.get_supported_read_only_binding_type(VISIBILITY_RANGES_STORAGE_BUFFER_COUNT)
    {

        BufferBindingType::Uniform
            if render_visibility_ranges.buffer.len() > VISIBILITY_RANGE_UNIFORM_BUFFER_SIZE =>
        {
            render_visibility_ranges
                .buffer
                .truncate(VISIBILITY_RANGE_UNIFORM_BUFFER_SIZE);
        }
        BufferBindingType::Uniform
            if render_visibility_ranges.buffer.len() < VISIBILITY_RANGE_UNIFORM_BUFFER_SIZE =>
        {
            while render_visibility_ranges.buffer.len() < VISIBILITY_RANGE_UNIFORM_BUFFER_SIZE {
                render_visibility_ranges.buffer.push(default());
            }
        }

        BufferBindingType::Storage { .. } if render_visibility_ranges.buffer.is_empty() => {
            render_visibility_ranges.buffer.push(default());
        }

        _ => {}
    }

    render_visibility_ranges
        .buffer
        .write_buffer(&render_device, &render_queue);
    render_visibility_ranges.buffer_dirty = false;
}
