

mod draw;
mod draw_state;
mod rangefinder;

use bevy_app::{App, Plugin};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::entity::EntityHash;
use bevy_platform::collections::{hash_map::Entry, HashMap};
use bevy_utils::default;
use bytemuck::{Pod, Zeroable};
pub use draw::*;
pub use draw_state::*;
use encase::ShaderType;
use encase::{internal::WriteInto, ShaderSize};
use indexmap::IndexMap;
use nonmax::NonMaxU32;
pub use rangefinder::*;
use wgpu::{BufferUsages, Features};

use crate::batching::gpu_preprocessing::{
    GpuPreprocessingMode, GpuPreprocessingSupport, PhaseBatchedInstanceBuffers,
    PhaseIndirectParametersBuffers,
};
use crate::render_resource::RawBufferVec;
use crate::renderer::RenderDevice;
use crate::sync_world::{MainEntity, MainEntityHashMap};
use crate::view::{ExtractedView, RetainedViewEntity};
use crate::RenderDebugFlags;
use bevy_material::descriptor::CachedRenderPipelineId;

use crate::{
    batching::{
        self,
        gpu_preprocessing::{self, BatchedInstanceBuffers},
        no_gpu_preprocessing::{self, BatchedInstanceBuffer},
        GetFullBatchData,
    },
    render_resource::{GpuArrayBufferIndex, PipelineCache},
    GpuResourceAppExt, Render, RenderApp, RenderSystems,
};
use bevy_ecs::{
    prelude::*,
    system::{lifetimeless::SRes, SystemParamItem},
};
use bevy_log::warn;
pub use bevy_material::labels::DrawFunctionId;
pub use bevy_material_macros::DrawFunctionLabel;
pub use bevy_material_macros::ShaderLabel;
use bevy_render::renderer::RenderAdapterInfo;
use core::{
    fmt::Debug,
    hash::Hash,
    iter,
    marker::PhantomData,
    mem,
    ops::{Range, RangeBounds},
};
use smallvec::SmallVec;

#[derive(Resource, Deref, DerefMut)]
pub struct ViewBinnedRenderPhases<BPI>(pub HashMap<RetainedViewEntity, BinnedRenderPhase<BPI>>)
where
    BPI: BinnedPhaseItem;

pub struct BinnedRenderPhase<BPI>
where
    BPI: BinnedPhaseItem,
{

    pub multidrawable_meshes: IndexMap<BPI::BatchSetKey, RenderMultidrawableBatchSet<BPI>>,

    pub batchable_meshes: IndexMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,

    pub unbatchable_meshes: IndexMap<(BPI::BatchSetKey, BPI::BinKey), UnbatchableBinnedEntities>,

    pub non_mesh_items: IndexMap<(BPI::BatchSetKey, BPI::BinKey), NonMeshEntities>,

    pub(crate) batch_sets: BinnedRenderPhaseBatchSets<BPI::BinKey>,

    cached_entity_bin_keys: MainEntityHashMap<CachedBinnedEntity<BPI>>,

    gpu_preprocessing_mode: GpuPreprocessingMode,
}

#[derive(Default)]
pub struct RenderBin {

    entities: IndexMap<MainEntity, InputUniformIndex, EntityHash>,
}

#[derive(Default)]
pub struct RenderMultidrawableBin {

    pub(crate) entity_to_binned_mesh_instance_index:
        MainEntityHashMap<RenderBinnedMeshInstanceIndex>,
}

impl RenderMultidrawableBin {

    fn new() -> RenderMultidrawableBin {
        RenderMultidrawableBin {
            entity_to_binned_mesh_instance_index: HashMap::default(),
        }
    }

    fn is_empty(&self) -> bool {
        self.entity_to_binned_mesh_instance_index.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Deref, DerefMut)]
pub(crate) struct RenderBinnedMeshInstanceIndex(pub(crate) u32);

pub struct RenderMultidrawableBatchSetGpuBuffers {

    pub render_binned_mesh_instance_buffer: RawBufferVec<GpuRenderBinnedMeshInstance>,

    pub bin_index_to_indirect_parameters_offset_buffer: RawBufferVec<u32>,
}

#[derive(Clone, Copy)]
pub(crate) struct CpuRenderBinnedMeshInstance {

    pub(crate) main_entity: MainEntity,

    bin_index: RenderBinIndex,
}

impl Default for CpuRenderBinnedMeshInstance {
    fn default() -> Self {
        CpuRenderBinnedMeshInstance {
            main_entity: MainEntity::from(Entity::PLACEHOLDER),
            bin_index: RenderBinIndex::default(),
        }
    }
}

impl RenderMultidrawableBatchSetGpuBuffers {

    fn new() -> RenderMultidrawableBatchSetGpuBuffers {
        let mut render_bin_entry_buffer = RawBufferVec::new(BufferUsages::STORAGE);
        render_bin_entry_buffer.set_label(Some("render bin entry buffer"));
        let mut bin_index_to_indirect_parameters_offset_buffer =
            RawBufferVec::new(BufferUsages::STORAGE);
        bin_index_to_indirect_parameters_offset_buffer
            .set_label(Some("bin index to indirect parameters offset buffer"));

        RenderMultidrawableBatchSetGpuBuffers {
            render_binned_mesh_instance_buffer: render_bin_entry_buffer,
            bin_index_to_indirect_parameters_offset_buffer,
        }
    }

    fn insert(
        &mut self,
        bin: &mut RenderMultidrawableBin,
        cpu_binned_mesh_instance_buffer: &mut Vec<CpuRenderBinnedMeshInstance>,
        main_entity: MainEntity,
        input_uniform_index: InputUniformIndex,
        bin_index: RenderBinIndex,
    ) {

        let gpu_render_bin_entry = GpuRenderBinnedMeshInstance {
            input_uniform_index: input_uniform_index.0,
            bin_index: bin_index.0,
        };

        let render_binned_mesh_instance_buffer_index =
            match bin.entity_to_binned_mesh_instance_index.entry(main_entity) {
                Entry::Occupied(occupied_entry) => *occupied_entry.get(),
                Entry::Vacant(vacant_entry) => {
                    let render_bin_buffer_index = RenderBinnedMeshInstanceIndex(
                        self.render_binned_mesh_instance_buffer
                            .push(GpuRenderBinnedMeshInstance::default())
                            as u32,
                    );
                    cpu_binned_mesh_instance_buffer.push(CpuRenderBinnedMeshInstance::default());
                    vacant_entry.insert(render_bin_buffer_index);
                    render_bin_buffer_index
                }
            };

        self.render_binned_mesh_instance_buffer.values_mut()
            [render_binned_mesh_instance_buffer_index.0 as usize] = gpu_render_bin_entry;
        cpu_binned_mesh_instance_buffer[render_binned_mesh_instance_buffer_index.0 as usize] =
            CpuRenderBinnedMeshInstance {
                main_entity,
                bin_index,
            };

        debug_assert_eq!(
            self.render_binned_mesh_instance_buffer.len(),
            cpu_binned_mesh_instance_buffer.len()
        );
    }

    #[must_use]
    fn remove(
        &mut self,
        bin: &mut RenderMultidrawableBin,
        cpu_binned_mesh_instance_buffer: &mut Vec<CpuRenderBinnedMeshInstance>,
        entity_to_remove: MainEntity,
    ) -> Option<(RenderBinnedMeshInstanceIndex, CpuRenderBinnedMeshInstance)> {

        let old_index = bin
            .entity_to_binned_mesh_instance_index
            .remove(&entity_to_remove)
            .expect("Entity not in bin");

        cpu_binned_mesh_instance_buffer.swap_remove(old_index.0 as usize);
        self.render_binned_mesh_instance_buffer
            .swap_remove(old_index.0 as usize);

        debug_assert_eq!(
            cpu_binned_mesh_instance_buffer.len(),
            self.render_binned_mesh_instance_buffer.len()
        );

        cpu_binned_mesh_instance_buffer
            .get(old_index.0 as usize)
            .map(|entity_indices| (old_index, *entity_indices))
    }
}

#[derive(Clone, Copy, Default, PartialEq, Debug, Pod, Zeroable, Deref, DerefMut)]
#[repr(transparent)]
pub(crate) struct RenderBinIndex(pub(crate) u32);

pub struct RenderMultidrawableBatchSet<BPI>
where
    BPI: BinnedPhaseItem,
{

    pub(crate) gpu_buffers: RenderMultidrawableBatchSetGpuBuffers,

    pub(crate) bin_key_to_bin_index: HashMap<BPI::BinKey, RenderBinIndex>,

    bins: Vec<Option<RenderMultidrawableBin>>,

    bin_free_list: Vec<RenderBinIndex>,

    indirect_parameters_offset_to_bin_index: Vec<RenderBinIndex>,

    pub(crate) render_binned_mesh_instances_cpu: Vec<CpuRenderBinnedMeshInstance>,
}

impl<BPI> RenderMultidrawableBatchSet<BPI>
where
    BPI: BinnedPhaseItem,
{

    fn new() -> RenderMultidrawableBatchSet<BPI> {
        RenderMultidrawableBatchSet {
            gpu_buffers: RenderMultidrawableBatchSetGpuBuffers::new(),
            bin_key_to_bin_index: HashMap::default(),
            bins: vec![],
            bin_free_list: vec![],
            indirect_parameters_offset_to_bin_index: vec![],
            render_binned_mesh_instances_cpu: vec![],
        }
    }

    pub(crate) fn representative_entity(&self) -> Option<MainEntity> {
        let first_bin_index = self.bin_key_to_bin_index.values().next()?;
        let first_bin = self.bin(*first_bin_index).expect("Bin should be present");
        first_bin
            .entity_to_binned_mesh_instance_index
            .keys()
            .next()
            .copied()
    }

    pub(crate) fn bin(&self, bin_index: RenderBinIndex) -> Option<&RenderMultidrawableBin> {
        self.bins
            .get(bin_index.0 as usize)
            .and_then(|bin| bin.as_ref())
    }

    fn insert(
        &mut self,
        bin_key: BPI::BinKey,
        main_entity: MainEntity,
        input_uniform_index: InputUniformIndex,
    ) {
        let bin_index;
        match self.bin_key_to_bin_index.entry(bin_key) {
            Entry::Occupied(occupied_entry) => {
                bin_index = *occupied_entry.get();
            }
            Entry::Vacant(vacant_entry) => {

                bin_index = self
                    .bin_free_list
                    .pop()
                    .unwrap_or(RenderBinIndex(self.bins.len() as u32));

                if bin_index.0 as usize == self.bins.len() {
                    self.bins.push(Some(RenderMultidrawableBin::new()));
                } else {
                    debug_assert!(self.bins[bin_index.0 as usize].is_none());
                    self.bins[bin_index.0 as usize] = Some(RenderMultidrawableBin::new());
                }
                vacant_entry.insert(bin_index);

                self.allocate_indirect_parameters(bin_index);
            }
        }

        let bin = self.bins[bin_index.0 as usize].as_mut().unwrap();
        self.gpu_buffers.insert(
            bin,
            &mut self.render_binned_mesh_instances_cpu,
            main_entity,
            input_uniform_index,
            bin_index,
        );
    }

    fn remove(&mut self, main_entity: MainEntity, bin_key: &BPI::BinKey) {

        let bin_index = *self
            .bin_key_to_bin_index
            .get(bin_key)
            .expect("Bin key not present");
        let bin = self.bins[bin_index.0 as usize].as_mut().unwrap();

        let maybe_displaced_entity_indices =
            self.gpu_buffers
                .remove(bin, &mut self.render_binned_mesh_instances_cpu, main_entity);
        if let Some((old_render_bin_buffer_index, displaced_entity_indices)) =
            maybe_displaced_entity_indices
        {
            self.bins[displaced_entity_indices.bin_index.0 as usize]
                .as_mut()
                .expect("Bin not present")
                .entity_to_binned_mesh_instance_index
                .insert(
                    displaced_entity_indices.main_entity,
                    old_render_bin_buffer_index,
                );
        }

        self.remove_bin_if_empty(bin_key, bin_index);
    }

    fn allocate_indirect_parameters(&mut self, bin_index: RenderBinIndex) {

        let indirect_parameters_offset = self.indirect_parameters_offset_to_bin_index.len() as u32;
        self.indirect_parameters_offset_to_bin_index.push(bin_index);

        if bin_index.0 as usize
            == self
                .gpu_buffers
                .bin_index_to_indirect_parameters_offset_buffer
                .len()
        {
            self.gpu_buffers
                .bin_index_to_indirect_parameters_offset_buffer
                .push(indirect_parameters_offset);
        } else {
            self.gpu_buffers
                .bin_index_to_indirect_parameters_offset_buffer
                .values_mut()[bin_index.0 as usize] = indirect_parameters_offset;
        }
    }

    fn remove_bin_if_empty(&mut self, bin_key: &BPI::BinKey, bin_index: RenderBinIndex) {

        let bin = self.bins[bin_index.0 as usize].as_mut().unwrap();
        if !bin.is_empty() {
            return;
        }

        self.bin_key_to_bin_index.remove(bin_key);
        self.bin_free_list.push(bin_index);
        self.bins[bin_index.0 as usize] = None;

        let indirect_parameters_offset = mem::replace(
            &mut self
                .gpu_buffers
                .bin_index_to_indirect_parameters_offset_buffer
                .values_mut()[bin_index.0 as usize],
            u32::MAX,
        );
        let removed_bin_index = self
            .indirect_parameters_offset_to_bin_index
            .swap_remove(indirect_parameters_offset as usize);
        debug_assert_eq!(bin_index, removed_bin_index);

        if let Some(displaced_bin_index) = self
            .indirect_parameters_offset_to_bin_index
            .get(indirect_parameters_offset as usize)
        {
            self.gpu_buffers
                .bin_index_to_indirect_parameters_offset_buffer
                .set(displaced_bin_index.0, indirect_parameters_offset);
        }
    }

    fn is_empty(&self) -> bool {
        self.bin_free_list.len() == self.bins.len()
    }

    pub(crate) fn bin_count(&self) -> usize {
        self.bin_key_to_bin_index.len()
    }
}

#[derive(Clone, Copy, Default, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct GpuRenderBinnedMeshInstance {

    pub(crate) input_uniform_index: u32,

    bin_index: u32,
}

pub struct CachedBinnedEntity<BPI>
where
    BPI: BinnedPhaseItem,
{

    pub cached_bin_key: Option<CachedBinKey<BPI>>,
}

pub struct CachedBinKey<BPI>
where
    BPI: BinnedPhaseItem,
{

    pub batch_set_key: BPI::BatchSetKey,

    pub bin_key: BPI::BinKey,

    pub phase_type: BinnedRenderPhaseType,
}

impl<BPI> Clone for CachedBinnedEntity<BPI>
where
    BPI: BinnedPhaseItem,
{
    fn clone(&self) -> Self {
        CachedBinnedEntity {
            cached_bin_key: self.cached_bin_key.clone(),
        }
    }
}

impl<BPI> Clone for CachedBinKey<BPI>
where
    BPI: BinnedPhaseItem,
{
    fn clone(&self) -> Self {
        CachedBinKey {
            batch_set_key: self.batch_set_key.clone(),
            bin_key: self.bin_key.clone(),
            phase_type: self.phase_type,
        }
    }
}

impl<BPI> PartialEq for CachedBinKey<BPI>
where
    BPI: BinnedPhaseItem,
{
    fn eq(&self, other: &Self) -> bool {
        self.batch_set_key == other.batch_set_key
            && self.bin_key == other.bin_key
            && self.phase_type == other.phase_type
    }
}

pub enum BinnedRenderPhaseBatchSets<BK> {

    DynamicUniforms(Vec<SmallVec<[BinnedRenderPhaseBatch; 1]>>),

    Direct(Vec<BinnedRenderPhaseBatch>),

    MultidrawIndirect(Vec<BinnedRenderPhaseBatchSet<BK>>),
}

pub struct BinnedRenderPhaseBatchSet<BK> {

    pub(crate) first_batch: BinnedRenderPhaseBatch,

    pub(crate) bin_key: BK,

    pub(crate) batch_count: u32,

    pub(crate) index: u32,

    pub(crate) first_work_item_index: u32,
}

impl<BK> BinnedRenderPhaseBatchSets<BK> {
    fn clear(&mut self) {
        match *self {
            BinnedRenderPhaseBatchSets::DynamicUniforms(ref mut vec) => vec.clear(),
            BinnedRenderPhaseBatchSets::Direct(ref mut vec) => vec.clear(),
            BinnedRenderPhaseBatchSets::MultidrawIndirect(ref mut vec) => vec.clear(),
        }
    }
}

#[derive(Debug)]
pub struct BinnedRenderPhaseBatch {

    pub representative_entity: (Entity, MainEntity),

    pub instance_range: Range<u32>,

    pub extra_index: PhaseItemExtraIndex,
}

pub struct UnbatchableBinnedEntities {

    pub entities: MainEntityHashMap<Entity>,

    pub(crate) buffer_indices: UnbatchableBinnedEntityIndexSet,
}

pub struct NonMeshEntities {

    pub entities: MainEntityHashMap<Entity>,
}

#[derive(Default)]

pub(crate) enum UnbatchableBinnedEntityIndexSet {

    #[default]
    NoEntities,

    Sparse {

        instance_range: Range<u32>,

        first_indirect_parameters_index: Option<NonMaxU32>,
    },

    Dense(Vec<UnbatchableBinnedEntityIndices>),
}

#[derive(Clone)]
pub(crate) struct UnbatchableBinnedEntityIndices {

    pub(crate) instance_index: u32,

    pub(crate) extra_index: PhaseItemExtraIndex,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BinnedRenderPhaseType {

    MultidrawableMesh,

    BatchableMesh,

    UnbatchableMesh,

    NonMesh,
}

impl<T> From<GpuArrayBufferIndex<T>> for UnbatchableBinnedEntityIndices
where
    T: Clone + ShaderSize + WriteInto,
{
    fn from(value: GpuArrayBufferIndex<T>) -> Self {
        UnbatchableBinnedEntityIndices {
            instance_index: value.index,
            extra_index: PhaseItemExtraIndex::maybe_dynamic_offset(value.dynamic_offset),
        }
    }
}

impl<BPI> Default for ViewBinnedRenderPhases<BPI>
where
    BPI: BinnedPhaseItem,
{
    fn default() -> Self {
        Self(default())
    }
}

impl<BPI> ViewBinnedRenderPhases<BPI>
where
    BPI: BinnedPhaseItem,
{
    pub fn prepare_for_new_frame(
        &mut self,
        retained_view_entity: RetainedViewEntity,
        gpu_preprocessing: GpuPreprocessingMode,
    ) {
        match self.entry(retained_view_entity) {
            Entry::Occupied(mut entry) => entry.get_mut().prepare_for_new_frame(),
            Entry::Vacant(entry) => {
                entry.insert(BinnedRenderPhase::<BPI>::new(gpu_preprocessing));
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Default, Deref, DerefMut, Debug, Pod, Zeroable)]
#[repr(transparent)]
pub struct InputUniformIndex(pub u32);

impl<BPI> BinnedRenderPhase<BPI>
where
    BPI: BinnedPhaseItem,
{

    pub fn add(
        &mut self,
        batch_set_key: BPI::BatchSetKey,
        bin_key: BPI::BinKey,
        (entity, main_entity): (Entity, MainEntity),
        input_uniform_index: InputUniformIndex,
        mut phase_type: BinnedRenderPhaseType,
    ) {

        if self.gpu_preprocessing_mode == GpuPreprocessingMode::PreprocessingOnly
            && phase_type == BinnedRenderPhaseType::MultidrawableMesh
        {
            phase_type = BinnedRenderPhaseType::BatchableMesh;
        }

        match phase_type {
            BinnedRenderPhaseType::MultidrawableMesh => {
                match self.multidrawable_meshes.entry(batch_set_key.clone()) {
                    indexmap::map::Entry::Occupied(mut entry) => {
                        entry
                            .get_mut()
                            .insert(bin_key.clone(), main_entity, input_uniform_index);
                    }
                    indexmap::map::Entry::Vacant(entry) => {
                        let mut new_batch_set = RenderMultidrawableBatchSet::new();
                        new_batch_set.insert(bin_key.clone(), main_entity, input_uniform_index);
                        entry.insert(new_batch_set);
                    }
                }
            }

            BinnedRenderPhaseType::BatchableMesh => {
                match self
                    .batchable_meshes
                    .entry((batch_set_key.clone(), bin_key.clone()).clone())
                {
                    indexmap::map::Entry::Occupied(mut entry) => {
                        entry.get_mut().insert(main_entity, input_uniform_index);
                    }
                    indexmap::map::Entry::Vacant(entry) => {
                        entry.insert(RenderBin::from_entity(main_entity, input_uniform_index));
                    }
                }
            }

            BinnedRenderPhaseType::UnbatchableMesh => {
                match self
                    .unbatchable_meshes
                    .entry((batch_set_key.clone(), bin_key.clone()))
                {
                    indexmap::map::Entry::Occupied(mut entry) => {
                        entry.get_mut().entities.insert(main_entity, entity);
                    }
                    indexmap::map::Entry::Vacant(entry) => {
                        let mut entities = MainEntityHashMap::default();
                        entities.insert(main_entity, entity);
                        entry.insert(UnbatchableBinnedEntities {
                            entities,
                            buffer_indices: default(),
                        });
                    }
                }
            }

            BinnedRenderPhaseType::NonMesh => {

                match self
                    .non_mesh_items
                    .entry((batch_set_key.clone(), bin_key.clone()).clone())
                {
                    indexmap::map::Entry::Occupied(mut entry) => {
                        entry.get_mut().entities.insert(main_entity, entity);
                    }
                    indexmap::map::Entry::Vacant(entry) => {
                        let mut entities = MainEntityHashMap::default();
                        entities.insert(main_entity, entity);
                        entry.insert(NonMeshEntities { entities });
                    }
                }
            }
        }

        self.update_cache(
            main_entity,
            Some(CachedBinKey {
                batch_set_key,
                bin_key,
                phase_type,
            }),
        );
    }

    pub fn update_cache(
        &mut self,
        main_entity: MainEntity,
        cached_bin_key: Option<CachedBinKey<BPI>>,
    ) {
        let new_cached_binned_entity = CachedBinnedEntity { cached_bin_key };
        self.cached_entity_bin_keys
            .insert(main_entity, new_cached_binned_entity);
    }

    pub fn render<'w>(
        &self,
        render_pass: &mut TrackedRenderPass<'w>,
        world: &'w World,
        view: Entity,
    ) -> Result<(), DrawError> {
        {
            let draw_functions = world.resource::<DrawFunctions<BPI>>();
            let mut draw_functions = draw_functions.write();
            draw_functions.prepare(world);

        }

        self.render_batchable_meshes(render_pass, world, view)?;
        self.render_unbatchable_meshes(render_pass, world, view)?;
        self.render_non_meshes(render_pass, world, view)?;

        Ok(())
    }

    fn render_batchable_meshes<'w>(
        &self,
        render_pass: &mut TrackedRenderPass<'w>,
        world: &'w World,
        view: Entity,
    ) -> Result<(), DrawError> {
        let draw_functions = world.resource::<DrawFunctions<BPI>>();
        let mut draw_functions = draw_functions.write();

        let render_device = world.resource::<RenderDevice>();
        let render_adapter_info = world.resource::<RenderAdapterInfo>();
        let multi_draw_indirect_count_supported = render_device
            .features()
            .contains(Features::MULTI_DRAW_INDIRECT_COUNT)

            && !matches!(render_adapter_info.backend, wgpu::Backend::Dx12);

        match self.batch_sets {
            BinnedRenderPhaseBatchSets::DynamicUniforms(ref batch_sets) => {
                debug_assert_eq!(self.batchable_meshes.len(), batch_sets.len());

                for ((batch_set_key, bin_key), batch_set) in
                    self.batchable_meshes.keys().zip(batch_sets.iter())
                {
                    for batch in batch_set {
                        let binned_phase_item = BPI::new(
                            batch_set_key.clone(),
                            bin_key.clone(),
                            batch.representative_entity,
                            batch.instance_range.clone(),
                            batch.extra_index.clone(),
                        );

                        let Some(draw_function) =
                            draw_functions.get_mut(binned_phase_item.draw_function())
                        else {
                            continue;
                        };

                        draw_function.draw(world, render_pass, view, &binned_phase_item)?;
                    }
                }
            }

            BinnedRenderPhaseBatchSets::Direct(ref batch_set) => {
                for (batch, (batch_set_key, bin_key)) in
                    batch_set.iter().zip(self.batchable_meshes.keys())
                {
                    let binned_phase_item = BPI::new(
                        batch_set_key.clone(),
                        bin_key.clone(),
                        batch.representative_entity,
                        batch.instance_range.clone(),
                        batch.extra_index.clone(),
                    );

                    let Some(draw_function) =
                        draw_functions.get_mut(binned_phase_item.draw_function())
                    else {
                        continue;
                    };

                    draw_function.draw(world, render_pass, view, &binned_phase_item)?;
                }
            }

            BinnedRenderPhaseBatchSets::MultidrawIndirect(ref batch_sets) => {
                for (batch_set_key, batch_set) in self
                    .multidrawable_meshes
                    .keys()
                    .chain(
                        self.batchable_meshes
                            .keys()
                            .map(|(batch_set_key, _)| batch_set_key),
                    )
                    .zip(batch_sets.iter())
                {
                    let batch = &batch_set.first_batch;

                    let batch_set_index = if multi_draw_indirect_count_supported {
                        NonMaxU32::new(batch_set.index)
                    } else {
                        None
                    };

                    let binned_phase_item = BPI::new(
                        batch_set_key.clone(),
                        batch_set.bin_key.clone(),
                        batch.representative_entity,
                        batch.instance_range.clone(),
                        match batch.extra_index {
                            PhaseItemExtraIndex::None => PhaseItemExtraIndex::None,
                            PhaseItemExtraIndex::DynamicOffset(ref dynamic_offset) => {
                                PhaseItemExtraIndex::DynamicOffset(*dynamic_offset)
                            }
                            PhaseItemExtraIndex::IndirectParametersIndex { ref range, .. } => {
                                PhaseItemExtraIndex::IndirectParametersIndex {
                                    range: range.start..(range.start + batch_set.batch_count),
                                    batch_set_index,
                                }
                            }
                        },
                    );

                    let Some(draw_function) =
                        draw_functions.get_mut(binned_phase_item.draw_function())
                    else {
                        continue;
                    };

                    draw_function.draw(world, render_pass, view, &binned_phase_item)?;
                }
            }
        }

        Ok(())
    }

    fn render_unbatchable_meshes<'w>(
        &self,
        render_pass: &mut TrackedRenderPass<'w>,
        world: &'w World,
        view: Entity,
    ) -> Result<(), DrawError> {
        let draw_functions = world.resource::<DrawFunctions<BPI>>();
        let mut draw_functions = draw_functions.write();

        for (batch_set_key, bin_key) in self.unbatchable_meshes.keys() {
            let unbatchable_entities =
                &self.unbatchable_meshes[&(batch_set_key.clone(), bin_key.clone())];
            for (entity_index, entity) in unbatchable_entities.entities.iter().enumerate() {
                let unbatchable_dynamic_offset = match &unbatchable_entities.buffer_indices {
                    UnbatchableBinnedEntityIndexSet::NoEntities => {

                        continue;
                    }
                    UnbatchableBinnedEntityIndexSet::Sparse {
                        instance_range,
                        first_indirect_parameters_index,
                    } => UnbatchableBinnedEntityIndices {
                        instance_index: instance_range.start + entity_index as u32,
                        extra_index: match first_indirect_parameters_index {
                            None => PhaseItemExtraIndex::None,
                            Some(first_indirect_parameters_index) => {
                                let first_indirect_parameters_index_for_entity =
                                    u32::from(*first_indirect_parameters_index)
                                        + entity_index as u32;
                                PhaseItemExtraIndex::IndirectParametersIndex {
                                    range: first_indirect_parameters_index_for_entity
                                        ..(first_indirect_parameters_index_for_entity + 1),
                                    batch_set_index: None,
                                }
                            }
                        },
                    },
                    UnbatchableBinnedEntityIndexSet::Dense(dynamic_offsets) => {
                        dynamic_offsets[entity_index].clone()
                    }
                };

                let binned_phase_item = BPI::new(
                    batch_set_key.clone(),
                    bin_key.clone(),
                    (*entity.1, *entity.0),
                    unbatchable_dynamic_offset.instance_index
                        ..(unbatchable_dynamic_offset.instance_index + 1),
                    unbatchable_dynamic_offset.extra_index,
                );

                let Some(draw_function) = draw_functions.get_mut(binned_phase_item.draw_function())
                else {
                    continue;
                };

                draw_function.draw(world, render_pass, view, &binned_phase_item)?;
            }
        }
        Ok(())
    }

    fn render_non_meshes<'w>(
        &self,
        render_pass: &mut TrackedRenderPass<'w>,
        world: &'w World,
        view: Entity,
    ) -> Result<(), DrawError> {
        let draw_functions = world.resource::<DrawFunctions<BPI>>();
        let mut draw_functions = draw_functions.write();

        for ((batch_set_key, bin_key), non_mesh_entities) in &self.non_mesh_items {
            for (main_entity, entity) in non_mesh_entities.entities.iter() {

                let binned_phase_item = BPI::new(
                    batch_set_key.clone(),
                    bin_key.clone(),
                    (*entity, *main_entity),
                    0..1,
                    PhaseItemExtraIndex::None,
                );

                let Some(draw_function) = draw_functions.get_mut(binned_phase_item.draw_function())
                else {
                    continue;
                };

                draw_function.draw(world, render_pass, view, &binned_phase_item)?;
            }
        }

        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.multidrawable_meshes.is_empty()
            && self.batchable_meshes.is_empty()
            && self.unbatchable_meshes.is_empty()
            && self.non_mesh_items.is_empty()
    }

    pub fn prepare_for_new_frame(&mut self) {
        self.batch_sets.clear();

        for unbatchable_bin in self.unbatchable_meshes.values_mut() {
            unbatchable_bin.buffer_indices.clear();
        }
    }

    pub fn remove(&mut self, main_entity: MainEntity) {
        let Some(cached_binned_entity) = self.cached_entity_bin_keys.remove(&main_entity) else {
            return;
        };

        if let Some(ref cached_bin_key) = cached_binned_entity.cached_bin_key {
            remove_entity_from_bin(
                main_entity,
                cached_bin_key,
                &mut self.multidrawable_meshes,
                &mut self.batchable_meshes,
                &mut self.unbatchable_meshes,
                &mut self.non_mesh_items,
            );
        }
    }
}

fn remove_entity_from_bin<BPI>(
    entity: MainEntity,
    entity_bin_key: &CachedBinKey<BPI>,
    multidrawable_meshes: &mut IndexMap<BPI::BatchSetKey, RenderMultidrawableBatchSet<BPI>>,
    batchable_meshes: &mut IndexMap<(BPI::BatchSetKey, BPI::BinKey), RenderBin>,
    unbatchable_meshes: &mut IndexMap<(BPI::BatchSetKey, BPI::BinKey), UnbatchableBinnedEntities>,
    non_mesh_items: &mut IndexMap<(BPI::BatchSetKey, BPI::BinKey), NonMeshEntities>,
) where
    BPI: BinnedPhaseItem,
{
    match entity_bin_key.phase_type {
        BinnedRenderPhaseType::MultidrawableMesh => {
            if let indexmap::map::Entry::Occupied(mut batch_set_entry) =
                multidrawable_meshes.entry(entity_bin_key.batch_set_key.clone())
            {
                batch_set_entry
                    .get_mut()
                    .remove(entity, &entity_bin_key.bin_key);

                if batch_set_entry.get_mut().is_empty() {
                    batch_set_entry.swap_remove();
                }
            }
        }

        BinnedRenderPhaseType::BatchableMesh => {
            if let indexmap::map::Entry::Occupied(mut bin_entry) = batchable_meshes.entry((
                entity_bin_key.batch_set_key.clone(),
                entity_bin_key.bin_key.clone(),
            )) {
                bin_entry.get_mut().remove(entity);

                if bin_entry.get_mut().is_empty() {
                    bin_entry.swap_remove();
                }
            }
        }

        BinnedRenderPhaseType::UnbatchableMesh => {
            if let indexmap::map::Entry::Occupied(mut bin_entry) = unbatchable_meshes.entry((
                entity_bin_key.batch_set_key.clone(),
                entity_bin_key.bin_key.clone(),
            )) {
                bin_entry.get_mut().entities.remove(&entity);

                if bin_entry.get_mut().entities.is_empty() {
                    bin_entry.swap_remove();
                }
            }
        }

        BinnedRenderPhaseType::NonMesh => {
            if let indexmap::map::Entry::Occupied(mut bin_entry) = non_mesh_items.entry((
                entity_bin_key.batch_set_key.clone(),
                entity_bin_key.bin_key.clone(),
            )) {
                bin_entry.get_mut().entities.remove(&entity);

                if bin_entry.get_mut().entities.is_empty() {
                    bin_entry.swap_remove();
                }
            }
        }
    }
}

impl<BPI> BinnedRenderPhase<BPI>
where
    BPI: BinnedPhaseItem,
{
    fn new(gpu_preprocessing: GpuPreprocessingMode) -> Self {
        Self {
            multidrawable_meshes: IndexMap::default(),
            batchable_meshes: IndexMap::default(),
            unbatchable_meshes: IndexMap::default(),
            non_mesh_items: IndexMap::default(),
            batch_sets: match gpu_preprocessing {
                GpuPreprocessingMode::Culling => {
                    BinnedRenderPhaseBatchSets::MultidrawIndirect(vec![])
                }
                GpuPreprocessingMode::PreprocessingOnly => {
                    BinnedRenderPhaseBatchSets::Direct(vec![])
                }
                GpuPreprocessingMode::None => BinnedRenderPhaseBatchSets::DynamicUniforms(vec![]),
            },
            cached_entity_bin_keys: MainEntityHashMap::default(),
            gpu_preprocessing_mode: gpu_preprocessing,
        }
    }
}

impl UnbatchableBinnedEntityIndexSet {

    fn indices_for_entity_index(
        &self,
        entity_index: u32,
    ) -> Option<UnbatchableBinnedEntityIndices> {
        match self {
            UnbatchableBinnedEntityIndexSet::NoEntities => None,
            UnbatchableBinnedEntityIndexSet::Sparse { instance_range, .. }
                if entity_index >= instance_range.len() as u32 =>
            {
                None
            }
            UnbatchableBinnedEntityIndexSet::Sparse {
                instance_range,
                first_indirect_parameters_index: None,
            } => Some(UnbatchableBinnedEntityIndices {
                instance_index: instance_range.start + entity_index,
                extra_index: PhaseItemExtraIndex::None,
            }),
            UnbatchableBinnedEntityIndexSet::Sparse {
                instance_range,
                first_indirect_parameters_index: Some(first_indirect_parameters_index),
            } => {
                let first_indirect_parameters_index_for_this_batch =
                    u32::from(*first_indirect_parameters_index) + entity_index;
                Some(UnbatchableBinnedEntityIndices {
                    instance_index: instance_range.start + entity_index,
                    extra_index: PhaseItemExtraIndex::IndirectParametersIndex {
                        range: first_indirect_parameters_index_for_this_batch
                            ..(first_indirect_parameters_index_for_this_batch + 1),
                        batch_set_index: None,
                    },
                })
            }
            UnbatchableBinnedEntityIndexSet::Dense(indices) => {
                indices.get(entity_index as usize).cloned()
            }
        }
    }
}

pub struct BinnedRenderPhasePlugin<BPI, GFBD>
where
    BPI: BinnedPhaseItem,
    GFBD: GetFullBatchData,
{

    pub debug_flags: RenderDebugFlags,
    phantom: PhantomData<(BPI, GFBD)>,
}

impl<BPI, GFBD> BinnedRenderPhasePlugin<BPI, GFBD>
where
    BPI: BinnedPhaseItem,
    GFBD: GetFullBatchData,
{
    pub fn new(debug_flags: RenderDebugFlags) -> Self {
        Self {
            debug_flags,
            phantom: PhantomData,
        }
    }
}

impl<BPI, GFBD> Plugin for BinnedRenderPhasePlugin<BPI, GFBD>
where
    BPI: BinnedPhaseItem,
    GFBD: GetFullBatchData + Sync + Send + 'static,
{
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_gpu_resource::<ViewBinnedRenderPhases<BPI>>()
            .allow_ambiguous_resource::<ViewBinnedRenderPhases<BPI>>()
            .init_gpu_resource::<PhaseBatchedInstanceBuffers<BPI, GFBD::BufferData>>()
            .init_gpu_resource::<PhaseIndirectParametersBuffers<BPI>>()
            .add_systems(
                Render,
                (
                    batching::sort_binned_render_phase::<BPI>.in_set(RenderSystems::PhaseSort),
                    (
                        no_gpu_preprocessing::batch_and_prepare_binned_render_phase::<BPI, GFBD>
                            .run_if(resource_exists::<BatchedInstanceBuffer<GFBD::BufferData>>),
                        gpu_preprocessing::batch_and_prepare_binned_render_phase::<BPI, GFBD>
                            .run_if(
                                resource_exists::<
                                    BatchedInstanceBuffers<GFBD::BufferData, GFBD::BufferInputData>,
                                >,
                            ),
                    )
                        .in_set(RenderSystems::PrepareResourcesBatchPhases)
                        .ambiguous_with(RenderSystems::PrepareResourcesBatchPhases),
                    gpu_preprocessing::write_binned_instance_buffers::<BPI, GFBD>
                        .run_if(
                            resource_exists::<
                                BatchedInstanceBuffers<GFBD::BufferData, GFBD::BufferInputData>,
                            >,
                        )
                        .in_set(RenderSystems::PrepareResourcesWritePhaseBuffers)
                        .ambiguous_with(RenderSystems::PrepareResourcesWritePhaseBuffers),
                    gpu_preprocessing::collect_buffers_for_phase::<BPI, GFBD>
                        .run_if(
                            resource_exists::<
                                BatchedInstanceBuffers<GFBD::BufferData, GFBD::BufferInputData>,
                            >,
                        )
                        .in_set(RenderSystems::PrepareResourcesCollectPhaseBuffers),
                ),
            );
    }
}

#[derive(Resource, Deref, DerefMut)]
pub struct ViewSortedRenderPhases<SPI>(pub HashMap<RetainedViewEntity, SortedRenderPhase<SPI>>)
where
    SPI: SortedPhaseItem;

impl<SPI> Default for ViewSortedRenderPhases<SPI>
where
    SPI: SortedPhaseItem,
{
    fn default() -> Self {
        Self(default())
    }
}

impl<SPI> ViewSortedRenderPhases<SPI>
where
    SPI: SortedPhaseItem,
{

    pub fn prepare_for_new_frame(&mut self, retained_view_entity: RetainedViewEntity) {
        match self.entry(retained_view_entity) {
            Entry::Occupied(mut entry) => {
                let render_phase = entry.get_mut();
                for (render_entity, main_entity) in render_phase.transient_items.drain(..) {
                    render_phase
                        .items
                        .swap_remove(&(render_entity, main_entity));
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(default());
            }
        }
    }
}

pub struct SortedRenderPhasePlugin<SPI, GFBD>
where
    SPI: SortedPhaseItem,
    GFBD: GetFullBatchData,
{

    pub debug_flags: RenderDebugFlags,
    phantom: PhantomData<(SPI, GFBD)>,
}

impl<SPI, GFBD> SortedRenderPhasePlugin<SPI, GFBD>
where
    SPI: SortedPhaseItem,
    GFBD: GetFullBatchData,
{
    pub fn new(debug_flags: RenderDebugFlags) -> Self {
        Self {
            debug_flags,
            phantom: PhantomData,
        }
    }
}

impl<SPI, GFBD> Plugin for SortedRenderPhasePlugin<SPI, GFBD>
where
    SPI: SortedPhaseItem + CachedRenderPipelinePhaseItem,
    GFBD: GetFullBatchData + Sync + Send + 'static,
{
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_gpu_resource::<ViewSortedRenderPhases<SPI>>()
            .allow_ambiguous_resource::<ViewSortedRenderPhases<SPI>>()
            .init_gpu_resource::<PhaseBatchedInstanceBuffers<SPI, GFBD::BufferData>>()
            .init_gpu_resource::<PhaseIndirectParametersBuffers<SPI>>()
            .add_systems(
                Render,
                (
                    (
                        no_gpu_preprocessing::batch_and_prepare_sorted_render_phase::<SPI, GFBD>
                            .run_if(resource_exists::<BatchedInstanceBuffer<GFBD::BufferData>>),
                        gpu_preprocessing::batch_and_prepare_sorted_render_phase::<SPI, GFBD>
                            .run_if(
                                resource_exists::<
                                    BatchedInstanceBuffers<GFBD::BufferData, GFBD::BufferInputData>,
                                >,
                            ),
                    )
                        .in_set(RenderSystems::PrepareResourcesBatchPhases),
                    gpu_preprocessing::collect_buffers_for_phase::<SPI, GFBD>
                        .run_if(
                            resource_exists::<
                                BatchedInstanceBuffers<GFBD::BufferData, GFBD::BufferInputData>,
                            >,
                        )
                        .in_set(RenderSystems::PrepareResourcesCollectPhaseBuffers),
                ),
            );
    }
}

impl UnbatchableBinnedEntityIndexSet {

    pub fn add(&mut self, indices: UnbatchableBinnedEntityIndices) {
        match self {
            UnbatchableBinnedEntityIndexSet::NoEntities => {
                match indices.extra_index {
                    PhaseItemExtraIndex::DynamicOffset(_) => {

                        *self = UnbatchableBinnedEntityIndexSet::Dense(vec![indices]);
                    }
                    PhaseItemExtraIndex::None => {

                        *self = UnbatchableBinnedEntityIndexSet::Sparse {
                            instance_range: indices.instance_index..indices.instance_index + 1,
                            first_indirect_parameters_index: None,
                        }
                    }
                    PhaseItemExtraIndex::IndirectParametersIndex {
                        range: ref indirect_parameters_index,
                        ..
                    } => {

                        *self = UnbatchableBinnedEntityIndexSet::Sparse {
                            instance_range: indices.instance_index..indices.instance_index + 1,
                            first_indirect_parameters_index: NonMaxU32::new(
                                indirect_parameters_index.start,
                            ),
                        }
                    }
                }
            }

            UnbatchableBinnedEntityIndexSet::Sparse {
                instance_range,
                first_indirect_parameters_index,
            } if instance_range.end == indices.instance_index
                && ((first_indirect_parameters_index.is_none()
                    && indices.extra_index == PhaseItemExtraIndex::None)
                    || first_indirect_parameters_index.is_some_and(
                        |first_indirect_parameters_index| match indices.extra_index {
                            PhaseItemExtraIndex::IndirectParametersIndex {
                                range: ref this_range,
                                ..
                            } => {
                                u32::from(first_indirect_parameters_index) + instance_range.end
                                    - instance_range.start
                                    == this_range.start
                            }
                            PhaseItemExtraIndex::DynamicOffset(_) | PhaseItemExtraIndex::None => {
                                false
                            }
                        },
                    )) =>
            {

                instance_range.end += 1;
            }

            UnbatchableBinnedEntityIndexSet::Sparse { instance_range, .. } => {

                warn!(
                    "Unbatchable binned entity index set was demoted from sparse to dense. \
                    This is a bug in the renderer. Please report it.",
                );
                let new_dynamic_offsets = (0..instance_range.len() as u32)
                    .flat_map(|entity_index| self.indices_for_entity_index(entity_index))
                    .chain(iter::once(indices))
                    .collect();
                *self = UnbatchableBinnedEntityIndexSet::Dense(new_dynamic_offsets);
            }

            UnbatchableBinnedEntityIndexSet::Dense(dense_indices) => {
                dense_indices.push(indices);
            }
        }
    }

    fn clear(&mut self) {
        match self {
            UnbatchableBinnedEntityIndexSet::Dense(dense_indices) => dense_indices.clear(),
            UnbatchableBinnedEntityIndexSet::Sparse { .. } => {
                *self = UnbatchableBinnedEntityIndexSet::NoEntities;
            }
            _ => {}
        }
    }
}

pub struct SortedRenderPhase<I>
where
    I: SortedPhaseItem,
{

    pub items: IndexMap<(Entity, MainEntity), I, EntityHash>,

    pub transient_items: Vec<(Entity, MainEntity)>,
}

impl<I> Default for SortedRenderPhase<I>
where
    I: SortedPhaseItem,
{
    fn default() -> Self {
        Self {
            items: IndexMap::default(),
            transient_items: vec![],
        }
    }
}

impl<I> SortedRenderPhase<I>
where
    I: SortedPhaseItem,
{

    #[inline]
    pub fn add_retained(&mut self, item: I) {
        self.items.insert((item.entity(), item.main_entity()), item);
    }

    #[inline]
    pub fn add_transient(&mut self, item: I) {
        let key = (item.entity(), item.main_entity());
        self.items.insert(key, item);
        self.transient_items.push(key);
    }

    #[inline]
    pub fn remove(&mut self, render_entity: Entity, main_entity: MainEntity) {
        self.items.swap_remove(&(render_entity, main_entity));
    }

    #[inline]
    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn recalculate_sort_keys(&mut self, view: &ExtractedView) {
        I::recalculate_sort_keys(&mut self.items, view);
    }

    pub fn sort(&mut self) {
        I::sort(&mut self.items);
    }

    #[inline]
    pub fn iter_entities(&'_ self) -> impl Iterator<Item = Entity> + '_ {
        self.items.values().map(PhaseItem::entity)
    }

    pub fn render<'w>(
        &self,
        render_pass: &mut TrackedRenderPass<'w>,
        world: &'w World,
        view: Entity,
    ) -> Result<(), DrawError> {
        self.render_range(render_pass, world, view, ..)
    }

    pub fn render_range<'w>(
        &self,
        render_pass: &mut TrackedRenderPass<'w>,
        world: &'w World,
        view: Entity,
        range: impl RangeBounds<usize>,
    ) -> Result<(), DrawError> {
        let items = self
            .items
            .get_range(range)
            .expect("`Range` provided to `render_range()` is out of bounds");

        let draw_functions = world.resource::<DrawFunctions<I>>();
        let mut draw_functions = draw_functions.write();
        draw_functions.prepare(world);

        let mut index = 0;
        while index < items.len() {
            let item = &items[index];
            let batch_range = item.batch_range();
            if batch_range.is_empty() {
                index += 1;
            } else {
                let draw_function = draw_functions.get_mut(item.draw_function()).unwrap();
                draw_function.draw(world, render_pass, view, item)?;
                index += batch_range.len();
            }
        }
        Ok(())
    }
}

pub trait PhaseItem: Sized + Send + Sync + 'static {

    const AUTOMATIC_BATCHING: bool = true;

    fn entity(&self) -> Entity;

    fn main_entity(&self) -> MainEntity;

    fn draw_function(&self) -> DrawFunctionId;

    fn batch_range(&self) -> &Range<u32>;
    fn batch_range_mut(&mut self) -> &mut Range<u32>;

    fn extra_index(&self) -> PhaseItemExtraIndex;

    fn batch_range_and_extra_index_mut(&mut self) -> (&mut Range<u32>, &mut PhaseItemExtraIndex);
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum PhaseItemExtraIndex {

    None,

    DynamicOffset(u32),

    IndirectParametersIndex {

        range: Range<u32>,

        batch_set_index: Option<NonMaxU32>,
    },
}

impl PhaseItemExtraIndex {

    pub fn maybe_indirect_parameters_index(
        indirect_parameters_index: Option<NonMaxU32>,
    ) -> PhaseItemExtraIndex {
        match indirect_parameters_index {
            Some(indirect_parameters_index) => PhaseItemExtraIndex::IndirectParametersIndex {
                range: u32::from(indirect_parameters_index)
                    ..(u32::from(indirect_parameters_index) + 1),
                batch_set_index: None,
            },
            None => PhaseItemExtraIndex::None,
        }
    }

    pub fn maybe_dynamic_offset(dynamic_offset: Option<NonMaxU32>) -> PhaseItemExtraIndex {
        match dynamic_offset {
            Some(dynamic_offset) => PhaseItemExtraIndex::DynamicOffset(dynamic_offset.into()),
            None => PhaseItemExtraIndex::None,
        }
    }
}

pub trait BinnedPhaseItem: PhaseItem {

    type BinKey: Clone + Send + Sync + PartialEq + Eq + Ord + Hash;

    type BatchSetKey: PhaseItemBatchSetKey;

    fn new(
        batch_set_key: Self::BatchSetKey,
        bin_key: Self::BinKey,
        representative_entity: (Entity, MainEntity),
        batch_range: Range<u32>,
        extra_index: PhaseItemExtraIndex,
    ) -> Self;
}

pub trait PhaseItemBatchSetKey: Clone + Send + Sync + PartialEq + Eq + Ord + Hash {

    fn indexed(&self) -> bool;
}

pub trait SortedPhaseItem: PhaseItem {

    type SortKey: Ord;

    fn sort_key(&self) -> Self::SortKey;

    #[inline]
    fn sort(items: &mut IndexMap<(Entity, MainEntity), Self, EntityHash>) {
        items.sort_unstable_by_key(|_, value| Self::sort_key(value));
    }

    fn recalculate_sort_keys(
        items: &mut IndexMap<(Entity, MainEntity), Self, EntityHash>,
        view: &ExtractedView,
    );

    fn indexed(&self) -> bool;
}

pub trait CachedRenderPipelinePhaseItem: PhaseItem {

    fn cached_pipeline(&self) -> CachedRenderPipelineId;
}

pub struct SetItemPipeline;

impl<P: CachedRenderPipelinePhaseItem> RenderCommand<P> for SetItemPipeline {
    type Param = SRes<PipelineCache>;
    type ViewQuery = ();
    type ItemQuery = ();
    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        _entity: Option<()>,
        pipeline_cache: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        if let Some(pipeline) = pipeline_cache
            .into_inner()
            .get_render_pipeline(item.cached_pipeline())
        {
            pass.set_render_pipeline(pipeline);
            RenderCommandResult::Success
        } else {
            RenderCommandResult::Skip
        }
    }
}

pub fn sort_phase_system<I>(
    views: Query<&ExtractedView>,
    mut render_phases: ResMut<ViewSortedRenderPhases<I>>,
) where
    I: SortedPhaseItem,
{
    for view in &views {
        let Some(phase) = render_phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        phase.recalculate_sort_keys(view);
        phase.sort();
    }
}

impl BinnedRenderPhaseType {
    pub fn mesh(
        batchable: bool,
        gpu_preprocessing_support: &GpuPreprocessingSupport,
    ) -> BinnedRenderPhaseType {
        match (batchable, gpu_preprocessing_support.max_supported_mode) {
            (true, GpuPreprocessingMode::Culling) => BinnedRenderPhaseType::MultidrawableMesh,
            (true, _) => BinnedRenderPhaseType::BatchableMesh,
            (false, _) => BinnedRenderPhaseType::UnbatchableMesh,
        }
    }
}

impl RenderBin {

    fn from_entity(entity: MainEntity, uniform_index: InputUniformIndex) -> RenderBin {
        let mut entities = IndexMap::default();
        entities.insert(entity, uniform_index);
        RenderBin { entities }
    }

    fn insert(&mut self, entity: MainEntity, uniform_index: InputUniformIndex) {
        self.entities.insert(entity, uniform_index);
    }

    fn remove(&mut self, entity_to_remove: MainEntity) {
        self.entities.swap_remove(&entity_to_remove);
    }

    fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    #[inline]
    pub fn entities(&self) -> &IndexMap<MainEntity, InputUniformIndex, EntityHash> {
        &self.entities
    }
}

#[cfg(test)]
mod tests {
    use proptest_derive::Arbitrary;

    use crate::render_phase::GpuRenderBinnedMeshInstance;

    #[test]
    #[expect(
        non_local_definitions,
        reason = "`derive(Arbitrary)` generates an impl here"
    )]
    fn render_multidrawable_batch_set() {
        use super::RenderMultidrawableBatchSet;

        use core::ops::Range;

        use bevy_ecs::entity::{Entity, EntityIndex};
        use bevy_material::labels::DrawFunctionId;
        use proptest::{bool, collection, test_runner::TestRunner};

        use crate::{
            render_phase::{
                BinnedPhaseItem, InputUniformIndex, PhaseItem, PhaseItemBatchSetKey, RenderBinIndex,
            },
            sync_world::{MainEntity, MainEntityHashMap, MainEntityHashSet},
        };

        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        struct MockBinnedPhaseItem;

        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        struct MockBinnedPhaseItemBinKey(u32);

        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        struct MockBinnedPhaseItemBatchSetKey;

        impl BinnedPhaseItem for MockBinnedPhaseItem {
            type BinKey = MockBinnedPhaseItemBinKey;

            type BatchSetKey = MockBinnedPhaseItemBatchSetKey;

            fn new(
                _: Self::BatchSetKey,
                _: Self::BinKey,
                _: (Entity, MainEntity),
                _: Range<u32>,
                _: super::PhaseItemExtraIndex,
            ) -> Self {
                Self
            }
        }

        impl PhaseItem for MockBinnedPhaseItem {
            fn entity(&self) -> Entity {
                unimplemented!()
            }

            fn main_entity(&self) -> MainEntity {
                unimplemented!()
            }

            fn draw_function(&self) -> DrawFunctionId {
                unimplemented!()
            }

            fn batch_range(&self) -> &Range<u32> {
                unimplemented!()
            }

            fn batch_range_mut(&mut self) -> &mut Range<u32> {
                unimplemented!()
            }

            fn extra_index(&self) -> super::PhaseItemExtraIndex {
                unimplemented!()
            }

            fn batch_range_and_extra_index_mut(
                &mut self,
            ) -> (&mut Range<u32>, &mut super::PhaseItemExtraIndex) {
                unimplemented!()
            }
        }

        impl PhaseItemBatchSetKey for MockBinnedPhaseItemBatchSetKey {
            fn indexed(&self) -> bool {

                true
            }
        }

        #[derive(Arbitrary, Debug)]
        enum RenderMultidrawableBatchSetOperation {

            Add {

                #[proptest(strategy = "0..32u32")]
                entity_id: u32,

                #[proptest(strategy = "0..8u32")]
                bin_index: u32,

                #[proptest(strategy = "0..1024u32")]
                input_uniform_index: u32,
            },

            Remove {

                #[proptest(strategy = "0..32u32")]
                entity_id: u32,
            },
        }

        struct ExpectedMultidrawableBatchSet {

            bin_index_to_entities: Vec<MainEntityHashSet>,

            entity_to_binned_mesh_instance: MainEntityHashMap<GpuRenderBinnedMeshInstance>,
        }

        impl ExpectedMultidrawableBatchSet {

            fn insert(
                &mut self,
                entity: MainEntity,
                bin_index: RenderBinIndex,
                input_uniform_index: InputUniformIndex,
            ) {
                self.entity_to_binned_mesh_instance.insert(
                    entity,
                    GpuRenderBinnedMeshInstance {
                        bin_index: bin_index.0,
                        input_uniform_index: input_uniform_index.0,
                    },
                );
                self.bin_index_to_entities[bin_index.0 as usize].insert(entity);
            }

            fn remove(&mut self, entity: MainEntity) -> GpuRenderBinnedMeshInstance {
                let render_binned_mesh_instance =
                    self.entity_to_binned_mesh_instance.remove(&entity).unwrap();
                self.bin_index_to_entities[render_binned_mesh_instance.bin_index as usize]
                    .remove(&entity);
                render_binned_mesh_instance
            }
        }

        let mut runner = TestRunner::default();
        runner
            .run(

                &collection::vec(
                    proptest::prelude::any::<RenderMultidrawableBatchSetOperation>(),
                    0..1024,
                ),
                |ops| {

                    let mut batch_set = RenderMultidrawableBatchSet::<MockBinnedPhaseItem>::new();

                    let mut expected = ExpectedMultidrawableBatchSet {
                        bin_index_to_entities: vec![MainEntityHashSet::default(); 1024],
                        entity_to_binned_mesh_instance: MainEntityHashMap::default(),
                    };

                    for op in ops.iter() {
                        match *op {
                            RenderMultidrawableBatchSetOperation::Add {
                                entity_id,
                                bin_index,
                                input_uniform_index,
                            } => {
                                let entity = MainEntity::from(Entity::from_index(
                                    EntityIndex::from_raw_u32(entity_id).unwrap(),
                                ));
                                let input_uniform_index = InputUniformIndex(input_uniform_index);

                                if expected
                                    .entity_to_binned_mesh_instance
                                    .contains_key(&entity)
                                {
                                    continue;
                                }

                                expected.insert(
                                    entity,
                                    RenderBinIndex(bin_index),
                                    input_uniform_index,
                                );
                                batch_set.insert(
                                    MockBinnedPhaseItemBinKey(bin_index),
                                    entity,
                                    input_uniform_index,
                                );
                            }

                            RenderMultidrawableBatchSetOperation::Remove { entity_id } => {
                                let entity = MainEntity::from(Entity::from_index(
                                    EntityIndex::from_raw_u32(entity_id).unwrap(),
                                ));

                                if !expected
                                    .entity_to_binned_mesh_instance
                                    .contains_key(&entity)
                                {
                                    continue;
                                }

                                let render_binned_mesh_instance = expected.remove(entity);
                                batch_set.remove(
                                    entity,
                                    &MockBinnedPhaseItemBinKey(
                                        render_binned_mesh_instance.bin_index,
                                    ),
                                );
                            }
                        }
                    }

                    verify(&batch_set, &expected);

                    Ok(())
                },
            )
            .unwrap();

        fn verify(
            batch_set: &RenderMultidrawableBatchSet<MockBinnedPhaseItem>,
            expected: &ExpectedMultidrawableBatchSet,
        ) {

            verify_entity_presence(
                batch_set,
                &expected.bin_index_to_entities,
                &expected.entity_to_binned_mesh_instance,
            );

            verify_render_binned_mesh_instance_buffer(batch_set, &expected.bin_index_to_entities);

            verify_indirect_parameters_offsets(batch_set);
        }

        fn verify_entity_presence(
            batch_set: &RenderMultidrawableBatchSet<MockBinnedPhaseItem>,
            expected: &[MainEntityHashSet],
            entity_to_bin_index_and_input_uniform_index: &MainEntityHashMap<
                GpuRenderBinnedMeshInstance,
            >,
        ) {
            for (bin_key_index, expected_entities) in expected.iter().enumerate() {
                let bin_key = MockBinnedPhaseItemBinKey(bin_key_index as u32);
                if expected_entities.is_empty() {
                    assert!(!batch_set.bin_key_to_bin_index.contains_key(&bin_key));
                    continue;
                }

                let Some(render_bin_index) = batch_set.bin_key_to_bin_index.get(&bin_key) else {
                    panic!("Bin not present: key {:?}", bin_key);
                };
                let Some(render_bin) = batch_set.bin(*render_bin_index) else {
                    panic!("Bin not present: index {:?}", render_bin_index);
                };
                for expected_entity in expected_entities {
                    let Some(GpuRenderBinnedMeshInstance {
                        bin_index,
                        input_uniform_index,
                    }) = entity_to_bin_index_and_input_uniform_index.get(expected_entity)
                    else {
                        panic!(
                            "Test harness bug: entity-to-bin-index-and-input-uniform-index \
                                table and expected table don't agree"
                        );
                    };
                    assert_eq!(MockBinnedPhaseItemBinKey(*bin_index), bin_key);

                    let Some(render_bin_buffer_index) = render_bin
                        .entity_to_binned_mesh_instance_index
                        .get(expected_entity)
                    else {
                        panic!("Buffer index not present");
                    };
                    let render_bin_entry = batch_set
                        .gpu_buffers
                        .render_binned_mesh_instance_buffer
                        .values()[render_bin_buffer_index.0 as usize];
                    assert_eq!(render_bin_entry.bin_index, **render_bin_index);
                    assert_eq!(render_bin_entry.input_uniform_index, *input_uniform_index);
                }
            }
        }

        fn verify_render_binned_mesh_instance_buffer(
            batch_set: &RenderMultidrawableBatchSet<MockBinnedPhaseItem>,
            expected: &[MainEntityHashSet],
        ) {
            for (render_bin_buffer_index, gpu_render_binned_mesh_instance) in batch_set
                .gpu_buffers
                .render_binned_mesh_instance_buffer
                .values()
                .iter()
                .enumerate()
            {
                let binned_mesh_instance_cpu =
                    &batch_set.render_binned_mesh_instances_cpu[render_bin_buffer_index];

                let gpu_render_bin_index = gpu_render_binned_mesh_instance.bin_index;
                assert_eq!(gpu_render_bin_index, *binned_mesh_instance_cpu.bin_index);

                let render_bin = batch_set.bins[gpu_render_bin_index as usize]
                    .as_ref()
                    .unwrap();

                let Some(entity) = render_bin
                    .entity_to_binned_mesh_instance_index
                    .iter()
                    .find_map(|(entity, buffer_index)| {
                        if render_bin_buffer_index as u32 == buffer_index.0 {
                            Some(entity)
                        } else {
                            None
                        }
                    })
                else {
                    panic!(
                        "Entity at buffer index {:?} not found in bin {:?}",
                        render_bin_buffer_index, gpu_render_bin_index
                    );
                };
                assert_eq!(binned_mesh_instance_cpu.main_entity, *entity);

                let Some(bin_key) =
                    batch_set
                        .bin_key_to_bin_index
                        .iter()
                        .find_map(|(bin_key, bin_index)| {
                            if bin_index.0 == gpu_render_bin_index {
                                Some(*bin_key)
                            } else {
                                None
                            }
                        })
                else {
                    panic!(
                        "Couldn't find a bin key for bin index {:?}",
                        gpu_render_bin_index
                    );
                };
                assert!(expected[bin_key.0 as usize].contains(entity));
            }
        }

        fn verify_indirect_parameters_offsets(
            batch_set: &RenderMultidrawableBatchSet<MockBinnedPhaseItem>,
        ) {
            for (render_bin_index, indirect_parameters_offset) in batch_set
                .gpu_buffers
                .bin_index_to_indirect_parameters_offset_buffer
                .values()
                .iter()
                .enumerate()
            {
                if *indirect_parameters_offset == u32::MAX {
                    continue;
                }
                assert_eq!(
                    batch_set.indirect_parameters_offset_to_bin_index
                        [*indirect_parameters_offset as usize],
                    RenderBinIndex(render_bin_index as u32)
                );
            }
            for (indirect_parameters_offset, render_bin_index) in batch_set
                .indirect_parameters_offset_to_bin_index
                .iter()
                .enumerate()
            {
                assert!(batch_set.bins[render_bin_index.0 as usize].is_some());
                assert_eq!(
                    *batch_set
                        .gpu_buffers
                        .bin_index_to_indirect_parameters_offset_buffer
                        .get(render_bin_index.0)
                        .unwrap(),
                    indirect_parameters_offset as u32
                );
            }
        }
    }
}
