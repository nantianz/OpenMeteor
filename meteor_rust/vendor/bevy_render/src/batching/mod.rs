use bevy_ecs::{
    component::Component,
    entity::Entity,
    system::{ResMut, SystemParam, SystemParamItem},
};
use gpu_preprocessing::UntypedPhaseIndirectParametersBuffers;
use nonmax::NonMaxU32;

use bevy_material::{descriptor::CachedRenderPipelineId, labels::DrawFunctionId};

use crate::{
    render_phase::{
        BinnedPhaseItem, CachedRenderPipelinePhaseItem, PhaseItemExtraIndex, SortedPhaseItem,
        SortedRenderPhase, ViewBinnedRenderPhases,
    },
    render_resource::{AtomicPod, GpuArrayBufferable},
    sync_world::MainEntity,
};

pub mod gpu_preprocessing;
pub mod no_gpu_preprocessing;

#[derive(Component, Default, Clone, Copy)]
pub struct NoAutomaticBatching;

#[derive(PartialEq)]
struct BatchSetMeta<T: PartialEq> {

    pipeline_id: CachedRenderPipelineId,

    draw_function_id: DrawFunctionId,
    dynamic_offset: Option<NonMaxU32>,
    user_data: T,
}

impl<T: PartialEq> BatchSetMeta<T> {
    fn new(item: &impl CachedRenderPipelinePhaseItem, user_data: T) -> Self {
        BatchSetMeta {
            pipeline_id: item.cached_pipeline(),
            draw_function_id: item.draw_function(),
            dynamic_offset: match item.extra_index() {
                PhaseItemExtraIndex::DynamicOffset(dynamic_offset) => {
                    NonMaxU32::new(dynamic_offset)
                }
                PhaseItemExtraIndex::None | PhaseItemExtraIndex::IndirectParametersIndex { .. } => {
                    None
                }
            },
            user_data,
        }
    }
}

pub trait GetBatchData {

    type Param: SystemParam + 'static;

    type BatchCompareData: PartialEq;

    type BatchSetCompareData: PartialEq;

    type BufferData: GpuArrayBufferable + Sync + Send + 'static;

    fn get_batch_data(
        param: &SystemParamItem<Self::Param>,
        query_item: (Entity, MainEntity),
    ) -> Option<(
        Self::BufferData,
        Option<(Self::BatchSetCompareData, Self::BatchCompareData)>,
    )>;
}

pub trait GetFullBatchData: GetBatchData {

    type BufferInputData: AtomicPod;

    fn get_binned_batch_data(
        param: &SystemParamItem<Self::Param>,
        query_item: MainEntity,
    ) -> Option<Self::BufferData>;

    fn get_index_and_compare_data(
        param: &SystemParamItem<Self::Param>,
        query_item: MainEntity,
    ) -> Option<(
        NonMaxU32,
        Option<(Self::BatchSetCompareData, Self::BatchCompareData)>,
    )>;

    fn get_binned_index(
        param: &SystemParamItem<Self::Param>,
        query_item: MainEntity,
    ) -> Option<NonMaxU32>;

    fn write_batch_indirect_parameters_metadata(
        indexed: bool,
        base_output_index: u32,
        batch_set_index: Option<NonMaxU32>,
        indirect_parameters_buffers: &mut UntypedPhaseIndirectParametersBuffers,
        indirect_parameters_offset: u32,
    );
}

pub fn sort_binned_render_phase<BPI>(mut phases: ResMut<ViewBinnedRenderPhases<BPI>>)
where
    BPI: BinnedPhaseItem,
{
    for phase in phases.values_mut() {
        phase.multidrawable_meshes.sort_unstable_keys();
        phase.batchable_meshes.sort_unstable_keys();
        phase.unbatchable_meshes.sort_unstable_keys();
        phase.non_mesh_items.sort_unstable_keys();
    }
}

fn batch_and_prepare_sorted_render_phase<I, GBD>(
    phase: &mut SortedRenderPhase<I>,
    mut process_item: impl FnMut(&mut I) -> Option<(GBD::BatchSetCompareData, GBD::BatchCompareData)>,
) where
    I: CachedRenderPipelinePhaseItem + SortedPhaseItem,
    GBD: GetBatchData,
{
    let items = phase.items.values_mut().map(|item| {
        let batch_data = match process_item(item) {
            Some(compare_data) if I::AUTOMATIC_BATCHING => {
                Some(BatchSetMeta::new(item, compare_data))
            }
            _ => None,
        };
        (item.batch_range_mut(), batch_data)
    });

    items.reduce(|(start_range, prev_batch_meta), (range, batch_meta)| {
        if batch_meta.is_some() && prev_batch_meta == batch_meta {
            start_range.end = range.end;
            (start_range, prev_batch_meta)
        } else {
            (range, batch_meta)
        }
    });
}
