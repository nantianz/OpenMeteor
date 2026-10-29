

use bevy_derive::{Deref, DerefMut};
use bevy_ecs::entity::Entity;
use bevy_ecs::resource::Resource;
use bevy_ecs::system::{Res, ResMut, StaticSystemParam};
use bevy_ecs::world::{FromWorld, World};
use bevy_log::error;
use smallvec::{smallvec, SmallVec};
use wgpu::{BindingResource, Limits};

use crate::{
    render_phase::{
        BinnedPhaseItem, BinnedRenderPhaseBatch, BinnedRenderPhaseBatchSets,
        CachedRenderPipelinePhaseItem, PhaseItemExtraIndex, SortedPhaseItem,
        ViewBinnedRenderPhases, ViewSortedRenderPhases,
    },
    render_resource::{GpuArrayBuffer, GpuArrayBufferable},
    renderer::{RenderDevice, RenderQueue},
};

use super::{GetBatchData, GetFullBatchData};

#[derive(Resource, Deref, DerefMut)]
pub struct BatchedInstanceBuffer<BD>(pub GpuArrayBuffer<BD>)
where
    BD: GpuArrayBufferable + Sync + Send + 'static;

impl<BD> FromWorld for BatchedInstanceBuffer<BD>
where
    BD: GpuArrayBufferable + Sync + Send + 'static,
{
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        BatchedInstanceBuffer(GpuArrayBuffer::new(&render_device.limits()))
    }
}

impl<BD> BatchedInstanceBuffer<BD>
where
    BD: GpuArrayBufferable + Sync + Send + 'static,
{

    pub fn new(limits: &Limits) -> Self {
        BatchedInstanceBuffer(GpuArrayBuffer::new(limits))
    }

    pub fn instance_data_binding(&self) -> Option<BindingResource<'_>> {
        self.binding()
    }
}

pub fn clear_batched_cpu_instance_buffers<GBD>(
    cpu_batched_instance_buffer: Option<ResMut<BatchedInstanceBuffer<GBD::BufferData>>>,
) where
    GBD: GetBatchData,
{
    if let Some(mut cpu_batched_instance_buffer) = cpu_batched_instance_buffer {
        cpu_batched_instance_buffer.clear();
    }
}

pub fn batch_and_prepare_sorted_render_phase<I, GBD>(
    batched_instance_buffer: ResMut<BatchedInstanceBuffer<GBD::BufferData>>,
    mut phases: ResMut<ViewSortedRenderPhases<I>>,
    param: StaticSystemParam<GBD::Param>,
) where
    I: CachedRenderPipelinePhaseItem + SortedPhaseItem,
    GBD: GetBatchData,
{
    let system_param_item = param.into_inner();

    let batched_instance_buffer = batched_instance_buffer.into_inner();

    for phase in phases.values_mut() {
        super::batch_and_prepare_sorted_render_phase::<I, GBD>(phase, |item| {
            let (buffer_data, compare_data) =
                GBD::get_batch_data(&system_param_item, (item.entity(), item.main_entity()))?;
            let buffer_index = batched_instance_buffer.push(buffer_data);

            let index = buffer_index.index;
            let (batch_range, extra_index) = item.batch_range_and_extra_index_mut();
            *batch_range = index..index + 1;
            *extra_index = PhaseItemExtraIndex::maybe_dynamic_offset(buffer_index.dynamic_offset);

            compare_data
        });
    }
}

pub fn batch_and_prepare_binned_render_phase<BPI, GFBD>(
    gpu_array_buffer: ResMut<BatchedInstanceBuffer<GFBD::BufferData>>,
    mut phases: ResMut<ViewBinnedRenderPhases<BPI>>,
    param: StaticSystemParam<GFBD::Param>,
) where
    BPI: BinnedPhaseItem,
    GFBD: GetFullBatchData,
{
    let gpu_array_buffer = gpu_array_buffer.into_inner();
    let system_param_item = param.into_inner();

    for phase in phases.values_mut() {

        for bin in phase.batchable_meshes.values_mut() {
            let mut batch_set: SmallVec<[BinnedRenderPhaseBatch; 1]> = smallvec![];
            for main_entity in bin.entities().keys() {
                let Some(buffer_data) =
                    GFBD::get_binned_batch_data(&system_param_item, *main_entity)
                else {
                    continue;
                };
                let instance = gpu_array_buffer.push(buffer_data);

                if !batch_set.last().is_some_and(|batch| {
                    batch.instance_range.end == instance.index
                        && batch.extra_index
                            == PhaseItemExtraIndex::maybe_dynamic_offset(instance.dynamic_offset)
                }) {
                    batch_set.push(BinnedRenderPhaseBatch {
                        representative_entity: (Entity::PLACEHOLDER, *main_entity),
                        instance_range: instance.index..instance.index,
                        extra_index: PhaseItemExtraIndex::maybe_dynamic_offset(
                            instance.dynamic_offset,
                        ),
                    });
                }

                if let Some(batch) = batch_set.last_mut() {
                    batch.instance_range.end = instance.index + 1;
                }
            }

            match phase.batch_sets {
                BinnedRenderPhaseBatchSets::DynamicUniforms(ref mut batch_sets) => {
                    batch_sets.push(batch_set);
                }
                BinnedRenderPhaseBatchSets::Direct(_)
                | BinnedRenderPhaseBatchSets::MultidrawIndirect { .. } => {
                    error!(
                        "Dynamic uniform batch sets should be used when GPU preprocessing is off"
                    );
                }
            }
        }

        for unbatchables in phase.unbatchable_meshes.values_mut() {
            for main_entity in unbatchables.entities.keys() {
                let Some(buffer_data) =
                    GFBD::get_binned_batch_data(&system_param_item, *main_entity)
                else {
                    continue;
                };
                let instance = gpu_array_buffer.push(buffer_data);
                unbatchables.buffer_indices.add(instance.into());
            }
        }
    }
}

pub fn write_batched_instance_buffer<GBD>(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut cpu_batched_instance_buffer: ResMut<BatchedInstanceBuffer<GBD::BufferData>>,
) where
    GBD: GetBatchData,
{
    cpu_batched_instance_buffer.write_buffer(&render_device, &render_queue);
}
