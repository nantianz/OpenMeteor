

use alloc::sync::Arc;
use core::{
    any::TypeId,
    marker::PhantomData,
    mem,
    ops::Range,
    sync::atomic::{AtomicU32, Ordering},
};

use bevy_app::{App, Plugin};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    prelude::Entity,
    query::{Has, With},
    resource::Resource,
    schedule::IntoScheduleConfigs as _,
    system::{Query, Res, ResMut, StaticSystemParam},
    world::{FromWorld, World},
};
use bevy_encase_derive::ShaderType;
use bevy_log::{error, info_once};
use bevy_math::UVec4;
use bevy_platform::collections::{hash_map::Entry, HashMap, HashSet};
use bevy_tasks::ComputeTaskPool;
use bevy_utils::{default, TypeIdMap};
use bytemuck::{Pod, Zeroable};
use encase::{internal::WriteInto, ShaderSize};
use nonmax::NonMaxU32;
use wgpu::{BindingResource, BufferUsages, DownlevelFlags, Features};

use crate::{
    occlusion_culling::OcclusionCulling,
    render_phase::{
        BinnedPhaseItem, BinnedRenderPhaseBatch, BinnedRenderPhaseBatchSet,
        BinnedRenderPhaseBatchSets, CachedRenderPipelinePhaseItem, PhaseItem,
        PhaseItemBatchSetKey as _, PhaseItemExtraIndex, RenderMultidrawableBatchSet,
        SortedPhaseItem, SortedRenderPhase, UnbatchableBinnedEntityIndices, ViewBinnedRenderPhases,
        ViewSortedRenderPhases,
    },
    render_resource::{
        AtomicPod, AtomicRawBufferVec, AtomicSparseBufferVec, Buffer, GpuArrayBufferable,
        PartialBufferVec, PipelineCache, RawBufferVec, SparseBufferUpdateBindGroups,
        SparseBufferUpdateJobs, SparseBufferUpdatePipelines, UninitBufferVec,
    },
    renderer::{RenderAdapter, RenderAdapterInfo, RenderDevice, RenderQueue, WgpuWrapper},
    sync_world::{MainEntity, MainEntityHashMap},
    view::{ExtractedView, NoIndirectDrawing, RetainedViewEntity},
    GpuResourceAppExt, Render, RenderApp, RenderDebugFlags, RenderSystems,
};

use super::{BatchSetMeta, GetBatchData, GetFullBatchData};

#[derive(Default)]
pub struct BatchingPlugin {

    pub debug_flags: RenderDebugFlags,
}

impl Plugin for BatchingPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .insert_resource(IndirectParametersBuffersSettings {
                allow_copies_from_indirect_parameter_buffers: self
                    .debug_flags
                    .contains(RenderDebugFlags::ALLOW_COPIES_FROM_INDIRECT_PARAMETERS),
            })
            .init_gpu_resource::<IndirectParametersBuffers>()
            .allow_ambiguous_resource::<IndirectParametersBuffers>()
            .init_gpu_resource::<BinUnpackingBuffers>()
            .add_systems(
                Render,
                write_indirect_parameters_buffers.in_set(RenderSystems::PrepareResourcesFlush),
            )
            .add_systems(
                Render,
                clear_indirect_parameters_buffers.in_set(RenderSystems::PrepareViews),
            );
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app.init_gpu_resource::<GpuPreprocessingSupport>();
    }
}

#[derive(Clone, Copy, PartialEq, Resource)]
pub struct GpuPreprocessingSupport {

    pub max_supported_mode: GpuPreprocessingMode,
}

impl GpuPreprocessingSupport {

    #[inline]
    pub fn is_available(&self) -> bool {
        self.max_supported_mode != GpuPreprocessingMode::None
    }

    pub fn min(&self, mode: GpuPreprocessingMode) -> GpuPreprocessingMode {
        match (self.max_supported_mode, mode) {
            (GpuPreprocessingMode::None, _) | (_, GpuPreprocessingMode::None) => {
                GpuPreprocessingMode::None
            }
            (mode, GpuPreprocessingMode::Culling) | (GpuPreprocessingMode::Culling, mode) => mode,
            (GpuPreprocessingMode::PreprocessingOnly, GpuPreprocessingMode::PreprocessingOnly) => {
                GpuPreprocessingMode::PreprocessingOnly
            }
        }
    }

    pub fn is_culling_supported(&self) -> bool {
        self.max_supported_mode == GpuPreprocessingMode::Culling
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum GpuPreprocessingMode {

    None,

    PreprocessingOnly,

    Culling,
}

#[derive(Resource)]
pub struct BatchedInstanceBuffers<BD, BDI>
where
    BD: GpuArrayBufferable + Sync + Send + 'static,
    BDI: AtomicPod,
{

    pub current_input_buffer: InstanceInputUniformBuffer<BDI>,

    pub previous_input_buffer: PreviousInstanceInputUniformBuffer<BDI>,

    pub phase_instance_buffers: TypeIdMap<UntypedPhaseBatchedInstanceBuffers<BD>>,
}

impl<BD, BDI> Default for BatchedInstanceBuffers<BD, BDI>
where
    BD: GpuArrayBufferable + Sync + Send + 'static,
    BDI: AtomicPod,
{
    fn default() -> Self {
        BatchedInstanceBuffers {
            current_input_buffer: InstanceInputUniformBuffer::new(),
            previous_input_buffer: PreviousInstanceInputUniformBuffer::new(),
            phase_instance_buffers: TypeIdMap::default(),
        }
    }
}

#[derive(Resource)]
pub struct PhaseBatchedInstanceBuffers<PI, BD>
where
    PI: PhaseItem,
    BD: GpuArrayBufferable + Sync + Send + 'static,
{

    pub buffers: UntypedPhaseBatchedInstanceBuffers<BD>,
    phantom: PhantomData<PI>,
}

impl<PI, BD> Default for PhaseBatchedInstanceBuffers<PI, BD>
where
    PI: PhaseItem,
    BD: GpuArrayBufferable + Sync + Send + 'static,
{
    fn default() -> Self {
        PhaseBatchedInstanceBuffers {
            buffers: UntypedPhaseBatchedInstanceBuffers::default(),
            phantom: PhantomData,
        }
    }
}

pub struct UntypedPhaseBatchedInstanceBuffers<BD>
where
    BD: GpuArrayBufferable + Sync + Send + 'static,
{

    pub data_buffer: UninitBufferVec<BD>,

    pub work_item_buffers: HashMap<RetainedViewEntity, PreprocessWorkItemBuffers>,

    pub late_indexed_indirect_parameters_buffer:
        RawBufferVec<LatePreprocessWorkItemIndirectParameters>,

    pub late_non_indexed_indirect_parameters_buffer:
        RawBufferVec<LatePreprocessWorkItemIndirectParameters>,
}

pub struct InstanceInputUniformBuffer<BDI>
where
    BDI: AtomicPod,
{

    buffer: AtomicSparseBufferVec<BDI>,

    free_uniform_indices: Vec<u32>,
}

impl<BDI> InstanceInputUniformBuffer<BDI>
where
    BDI: AtomicPod,
{

    pub fn new() -> InstanceInputUniformBuffer<BDI> {
        InstanceInputUniformBuffer {
            buffer: AtomicSparseBufferVec::new(
                BufferUsages::STORAGE,
                8,
                Arc::from("instance input uniform buffer"),
            ),
            free_uniform_indices: vec![],
        }
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.free_uniform_indices.clear();
    }

    #[inline]
    pub fn buffer(&self) -> &AtomicSparseBufferVec<BDI> {
        &self.buffer
    }

    pub fn add(&mut self, element: BDI) -> u32 {
        match self.free_uniform_indices.pop() {
            Some(uniform_index) => {
                self.buffer.set(uniform_index, element);
                uniform_index
            }
            None => self.buffer.push(element),
        }
    }

    pub fn remove(&mut self, uniform_index: u32) {
        self.free_uniform_indices.push(uniform_index);
    }

    pub fn get(&self, uniform_index: u32) -> Option<BDI> {
        if uniform_index >= self.buffer.len() || self.free_uniform_indices.contains(&uniform_index)
        {
            None
        } else {
            Some(self.get_unchecked(uniform_index))
        }
    }

    pub fn get_unchecked(&self, uniform_index: u32) -> BDI {
        self.buffer.get(uniform_index)
    }

    pub fn set(&self, uniform_index: u32, element: BDI) {
        self.buffer.set(uniform_index, element);
    }

    pub fn ensure_nonempty(&mut self) {
        if self.buffer.is_empty() {
            self.buffer.push(default());
        }
    }

    pub fn len(&self) -> usize {
        self.buffer.len() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn into_buffer(self) -> AtomicSparseBufferVec<BDI> {
        self.buffer
    }
}

impl<BDI> Default for InstanceInputUniformBuffer<BDI>
where
    BDI: AtomicPod,
{
    fn default() -> Self {
        Self::new()
    }
}

pub struct PreviousInstanceInputUniformBuffer<BDI>
where
    BDI: AtomicPod,
{

    buffer: AtomicRawBufferVec<BDI>,

    atomic_len: AtomicU32,
}

impl<BDI> PreviousInstanceInputUniformBuffer<BDI>
where
    BDI: AtomicPod,
{

    pub fn new() -> PreviousInstanceInputUniformBuffer<BDI> {
        PreviousInstanceInputUniformBuffer {
            buffer: AtomicRawBufferVec::with_label(
                BufferUsages::STORAGE,
                "previous instance input uniform buffer",
            ),
            atomic_len: AtomicU32::new(0),
        }
    }

    fn write_buffer(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {

        debug_assert!(!self.buffer.is_empty());

        self.buffer.write_buffer_range(
            0..(self.atomic_len.load(Ordering::Relaxed) as usize).max(1),
            render_device,
            render_queue,
        );
    }

    pub fn clear(&mut self) {

        self.atomic_len.store(0, Ordering::Relaxed);
    }

    pub fn reserve(&mut self, capacity: u32) {
        self.buffer.grow(capacity);
        *self.atomic_len.get_mut() = 0;
    }

    pub fn push(&self, value: BDI) -> u32 {
        let index = self.atomic_len.fetch_add(1, Ordering::Relaxed);
        debug_assert!(
            (index as usize) < self.buffer.len() as usize,
            "push exceeded pre-allocated capacity"
        );
        self.buffer.set(index, value);
        index
    }

    pub fn ensure_nonempty(&mut self) {
        if self.buffer.is_empty() {
            self.buffer.push(default());
        }
    }

    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.buffer()
    }
}

impl<BDI> Default for PreviousInstanceInputUniformBuffer<BDI>
where
    BDI: AtomicPod,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        clippy::large_enum_variant,
        reason = "See https://github.com/bevyengine/bevy/issues/19220"
    )
)]
pub enum PreprocessWorkItemBuffers {

    Direct(RawBufferVec<PreprocessWorkItem>),

    Indirect {

        indexed: PartialBufferVec<PreprocessWorkItem>,

        non_indexed: PartialBufferVec<PreprocessWorkItem>,

        gpu_occlusion_culling: Option<GpuOcclusionCullingWorkItemBuffers>,
    },
}

pub struct GpuOcclusionCullingWorkItemBuffers {

    pub late_indexed: UninitBufferVec<PreprocessWorkItem>,

    pub late_non_indexed: UninitBufferVec<PreprocessWorkItem>,

    pub late_indirect_parameters_indexed_offset: u32,

    pub late_indirect_parameters_non_indexed_offset: u32,
}

#[derive(Clone, Copy, ShaderType, Pod, Zeroable)]
#[repr(C)]
pub struct LatePreprocessWorkItemIndirectParameters {

    dispatch_x: u32,

    dispatch_y: u32,

    dispatch_z: u32,

    work_item_count: u32,

    pad: UVec4,
}

impl Default for LatePreprocessWorkItemIndirectParameters {
    fn default() -> LatePreprocessWorkItemIndirectParameters {
        LatePreprocessWorkItemIndirectParameters {
            dispatch_x: 0,
            dispatch_y: 1,
            dispatch_z: 1,
            work_item_count: 0,
            pad: default(),
        }
    }
}

pub fn get_or_create_work_item_buffer<'a, I>(
    work_item_buffers: &'a mut HashMap<RetainedViewEntity, PreprocessWorkItemBuffers>,
    view: RetainedViewEntity,
    no_indirect_drawing: bool,
    enable_gpu_occlusion_culling: bool,
) -> &'a mut PreprocessWorkItemBuffers
where
    I: 'static,
{
    let preprocess_work_item_buffers = match work_item_buffers.entry(view) {
        Entry::Occupied(occupied_entry) => occupied_entry.into_mut(),
        Entry::Vacant(vacant_entry) => {
            if no_indirect_drawing {
                vacant_entry.insert(PreprocessWorkItemBuffers::Direct(RawBufferVec::new(
                    BufferUsages::STORAGE,
                )))
            } else {
                vacant_entry.insert(PreprocessWorkItemBuffers::Indirect {
                    indexed: PartialBufferVec::new(
                        BufferUsages::STORAGE,
                        "indexed preprocess work item buffer".to_owned(),
                    ),
                    non_indexed: PartialBufferVec::new(
                        BufferUsages::STORAGE,
                        "non-indexed preprocess work item buffer".to_owned(),
                    ),

                    gpu_occlusion_culling: None,
                })
            }
        }
    };

    if let PreprocessWorkItemBuffers::Indirect {
        ref mut gpu_occlusion_culling,
        ..
    } = *preprocess_work_item_buffers
    {
        match (
            enable_gpu_occlusion_culling,
            gpu_occlusion_culling.is_some(),
        ) {
            (false, false) | (true, true) => {}
            (false, true) => {
                *gpu_occlusion_culling = None;
            }
            (true, false) => {
                *gpu_occlusion_culling = Some(GpuOcclusionCullingWorkItemBuffers {
                    late_indexed: UninitBufferVec::new(BufferUsages::STORAGE),
                    late_non_indexed: UninitBufferVec::new(BufferUsages::STORAGE),
                    late_indirect_parameters_indexed_offset: 0,
                    late_indirect_parameters_non_indexed_offset: 0,
                });
            }
        }
    }

    preprocess_work_item_buffers
}

pub fn init_work_item_buffers(
    work_item_buffers: &mut PreprocessWorkItemBuffers,
    late_indexed_indirect_parameters_buffer: &'_ mut RawBufferVec<
        LatePreprocessWorkItemIndirectParameters,
    >,
    late_non_indexed_indirect_parameters_buffer: &'_ mut RawBufferVec<
        LatePreprocessWorkItemIndirectParameters,
    >,
) {

    if let PreprocessWorkItemBuffers::Indirect {
        gpu_occlusion_culling:
            Some(GpuOcclusionCullingWorkItemBuffers {
                ref mut late_indirect_parameters_indexed_offset,
                ref mut late_indirect_parameters_non_indexed_offset,
                ..
            }),
        ..
    } = *work_item_buffers
    {
        *late_indirect_parameters_indexed_offset = late_indexed_indirect_parameters_buffer
            .push(LatePreprocessWorkItemIndirectParameters::default())
            as u32;
        *late_indirect_parameters_non_indexed_offset = late_non_indexed_indirect_parameters_buffer
            .push(LatePreprocessWorkItemIndirectParameters::default())
            as u32;
    }
}

impl PreprocessWorkItemBuffers {

    pub fn push(&mut self, indexed: bool, preprocess_work_item: PreprocessWorkItem) {
        match *self {
            PreprocessWorkItemBuffers::Direct(ref mut buffer) => {
                buffer.push(preprocess_work_item);
            }
            PreprocessWorkItemBuffers::Indirect {
                indexed: ref mut indexed_buffer,
                non_indexed: ref mut non_indexed_buffer,
                ref mut gpu_occlusion_culling,
            } => {
                if indexed {
                    indexed_buffer.push_init(preprocess_work_item);
                } else {
                    non_indexed_buffer.push_init(preprocess_work_item);
                }

                if let Some(ref mut gpu_occlusion_culling) = *gpu_occlusion_culling {
                    if indexed {
                        gpu_occlusion_culling.late_indexed.add();
                    } else {
                        gpu_occlusion_culling.late_non_indexed.add();
                    }
                }
            }
        }
    }

    pub fn clear(&mut self) {
        match *self {
            PreprocessWorkItemBuffers::Direct(ref mut buffer) => {
                buffer.clear();
            }
            PreprocessWorkItemBuffers::Indirect {
                indexed: ref mut indexed_buffer,
                non_indexed: ref mut non_indexed_buffer,
                ref mut gpu_occlusion_culling,
            } => {
                indexed_buffer.clear();
                non_indexed_buffer.clear();

                if let Some(ref mut gpu_occlusion_culling) = *gpu_occlusion_culling {
                    gpu_occlusion_culling.late_indexed.clear();
                    gpu_occlusion_culling.late_non_indexed.clear();
                    gpu_occlusion_culling.late_indirect_parameters_indexed_offset = 0;
                    gpu_occlusion_culling.late_indirect_parameters_non_indexed_offset = 0;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Default, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct PreprocessWorkItem {

    pub input_index: u32,

    pub output_or_indirect_parameters_index: u32,
}

#[derive(Clone, Copy, Debug, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct IndirectParametersIndexed {

    pub index_count: u32,

    pub instance_count: u32,

    pub first_index: u32,

    pub base_vertex: u32,

    pub first_instance: u32,
}

#[derive(Clone, Copy, Debug, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct IndirectParametersNonIndexed {

    pub vertex_count: u32,

    pub instance_count: u32,

    pub base_vertex: u32,

    pub first_instance: u32,
}

#[derive(Clone, Copy, Default, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct IndirectParametersCpuMetadata {

    pub base_output_index: u32,

    pub batch_set_index: u32,
}

#[derive(Clone, Copy, Default, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct IndirectParametersGpuMetadata {

    pub mesh_index: u32,

    pub early_instance_count: u32,

    pub late_instance_count: u32,
}

#[derive(Clone, Copy, Default, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct IndirectBatchSet {

    pub indirect_parameters_count: u32,

    pub indirect_parameters_base: u32,
}

#[derive(Resource, Deref, DerefMut, Default)]
pub struct IndirectParametersBuffers {

    #[deref]
    pub buffers: TypeIdMap<UntypedPhaseIndirectParametersBuffers>,
}

#[derive(Resource)]
pub struct IndirectParametersBuffersSettings {

    pub allow_copies_from_indirect_parameter_buffers: bool,
}

#[derive(Clone, Copy, Pod, Zeroable, ShaderType)]
#[repr(C)]
pub struct GpuBinUnpackingMetadata {

    base_output_work_item_index: u32,

    base_indirect_parameters_index: u32,

    binned_mesh_instance_count: u32,

    pad: [u32; 61],
}

impl Default for GpuBinUnpackingMetadata {
    fn default() -> GpuBinUnpackingMetadata {
        GpuBinUnpackingMetadata {
            base_output_work_item_index: 0,
            base_indirect_parameters_index: 0,
            binned_mesh_instance_count: 0,
            pad: [0; _],
        }
    }
}

pub struct BinUnpackingJob {

    pub render_binned_mesh_instance_buffer: Buffer,

    pub bin_index_to_indirect_parameters_offset_buffer: Buffer,

    pub bin_unpacking_metadata_index: BinUnpackingMetadataIndex,

    pub mesh_instance_count: u32,
}

#[derive(Resource)]
pub struct PhaseIndirectParametersBuffers<PI>
where
    PI: PhaseItem,
{

    pub buffers: UntypedPhaseIndirectParametersBuffers,
    phantom: PhantomData<PI>,
}

impl<PI> FromWorld for PhaseIndirectParametersBuffers<PI>
where
    PI: PhaseItem,
{
    fn from_world(world: &mut World) -> Self {
        let settings = world.resource::<IndirectParametersBuffersSettings>();
        PhaseIndirectParametersBuffers {
            buffers: UntypedPhaseIndirectParametersBuffers::new(
                settings.allow_copies_from_indirect_parameter_buffers,
            ),
            phantom: PhantomData,
        }
    }
}

impl<PI> PhaseIndirectParametersBuffers<PI>
where
    PI: PhaseItem,
{

    fn allocate(&mut self, no_indirect_drawing: bool, item_is_indexed: bool) -> Option<u32> {
        if no_indirect_drawing {
            None
        } else if item_is_indexed {
            Some(self.buffers.indexed.allocate(1))
        } else {
            Some(self.buffers.non_indexed.allocate(1))
        }
    }
}

pub struct UntypedPhaseIndirectParametersBuffers {

    pub indexed: MeshClassIndirectParametersBuffers<IndirectParametersIndexed>,

    pub non_indexed: MeshClassIndirectParametersBuffers<IndirectParametersNonIndexed>,
}

impl UntypedPhaseIndirectParametersBuffers {

    pub fn new(
        allow_copies_from_indirect_parameter_buffers: bool,
    ) -> UntypedPhaseIndirectParametersBuffers {
        UntypedPhaseIndirectParametersBuffers {
            non_indexed: MeshClassIndirectParametersBuffers::new(
                allow_copies_from_indirect_parameter_buffers,
            ),
            indexed: MeshClassIndirectParametersBuffers::new(
                allow_copies_from_indirect_parameter_buffers,
            ),
        }
    }

    pub fn allocate(&mut self, indexed: bool, count: u32) -> u32 {
        if indexed {
            self.indexed.allocate(count)
        } else {
            self.non_indexed.allocate(count)
        }
    }

    fn batch_count(&self, indexed: bool) -> usize {
        if indexed {
            self.indexed.batch_count()
        } else {
            self.non_indexed.batch_count()
        }
    }

    pub fn batch_set_count(&self, indexed: bool) -> usize {
        if indexed {
            self.indexed.batch_sets.len()
        } else {
            self.non_indexed.batch_sets.len()
        }
    }

    #[inline]
    pub fn add_batch_set(&mut self, indexed: bool, indirect_parameters_base: u32) {
        if indexed {
            self.indexed.batch_sets.push(IndirectBatchSet {
                indirect_parameters_base,
                indirect_parameters_count: 0,
            });
        } else {
            self.non_indexed.batch_sets.push(IndirectBatchSet {
                indirect_parameters_base,
                indirect_parameters_count: 0,
            });
        }
    }

    pub fn get_next_batch_set_index(&self, indexed: bool) -> Option<NonMaxU32> {
        NonMaxU32::new(self.batch_set_count(indexed) as u32)
    }

    pub fn clear(&mut self) {
        self.indexed.clear();
        self.non_indexed.clear();
    }
}

#[derive(Resource)]
pub struct BinUnpackingBuffers {

    pub bin_unpacking_metadata: RawBufferVec<GpuBinUnpackingMetadata>,

    pub view_phase_buffers: HashMap<BinUnpackingBuffersKey, ViewPhaseBinUnpackingBuffers>,
}

impl Default for BinUnpackingBuffers {
    fn default() -> Self {
        let mut bin_unpacking_metadata = RawBufferVec::new(BufferUsages::UNIFORM);
        bin_unpacking_metadata.set_label(Some("bin unpacking metadata buffer"));
        BinUnpackingBuffers {
            bin_unpacking_metadata,
            view_phase_buffers: HashMap::default(),
        }
    }
}

#[derive(Default)]
pub struct ViewPhaseBinUnpackingBuffers {

    pub indexed_unpacking_jobs: Vec<BinUnpackingJob>,

    pub non_indexed_unpacking_jobs: Vec<BinUnpackingJob>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BinUnpackingBuffersKey {

    pub phase: TypeId,

    pub view: RetainedViewEntity,
}

#[derive(Clone, Copy, Debug, Deref, DerefMut)]
pub struct BinUnpackingMetadataIndex(pub NonMaxU32);

impl BinUnpackingMetadataIndex {

    pub fn uniform_offset(&self) -> u32 {
        self.get() * size_of::<GpuBinUnpackingMetadata>() as u32
    }
}

pub struct MeshClassIndirectParametersBuffers<IP>
where
    IP: Clone + ShaderSize + WriteInto,
{

    indirect_draw_parameters: UninitBufferVec<IP>,

    cpu_metadata: RawBufferVec<IndirectParametersCpuMetadata>,

    gpu_metadata: UninitBufferVec<IndirectParametersGpuMetadata>,

    batch_sets: RawBufferVec<IndirectBatchSet>,
}

impl<IP> MeshClassIndirectParametersBuffers<IP>
where
    IP: Clone + ShaderSize + WriteInto,
{
    fn new(
        allow_copies_from_indirect_parameter_buffers: bool,
    ) -> MeshClassIndirectParametersBuffers<IP> {
        let mut indirect_parameter_buffer_usages = BufferUsages::STORAGE | BufferUsages::INDIRECT;
        if allow_copies_from_indirect_parameter_buffers {
            indirect_parameter_buffer_usages |= BufferUsages::COPY_SRC;
        }

        MeshClassIndirectParametersBuffers {
            indirect_draw_parameters: UninitBufferVec::new(indirect_parameter_buffer_usages),
            cpu_metadata: RawBufferVec::new(BufferUsages::STORAGE),
            gpu_metadata: UninitBufferVec::new(BufferUsages::STORAGE),
            batch_sets: RawBufferVec::new(indirect_parameter_buffer_usages),
        }
    }

    #[inline]
    pub fn data_buffer(&self) -> Option<&Buffer> {
        self.indirect_draw_parameters.buffer()
    }

    #[inline]
    pub fn cpu_metadata_buffer(&self) -> Option<&Buffer> {
        self.cpu_metadata.buffer()
    }

    #[inline]
    pub fn gpu_metadata_buffer(&self) -> Option<&Buffer> {
        self.gpu_metadata.buffer()
    }

    #[inline]
    pub fn batch_sets_buffer(&self) -> Option<&Buffer> {
        self.batch_sets.buffer()
    }

    fn allocate(&mut self, count: u32) -> u32 {
        let length = self.indirect_draw_parameters.len();
        self.cpu_metadata.reserve_internal(count as usize);
        self.gpu_metadata.add_multiple(count as usize);
        for _ in 0..count {
            self.indirect_draw_parameters.add();
            self.cpu_metadata
                .push(IndirectParametersCpuMetadata::default());
        }
        length as u32
    }

    pub fn set(&mut self, index: u32, value: IndirectParametersCpuMetadata) {
        self.cpu_metadata.set(index, value);
    }

    #[inline]
    pub fn batch_count(&self) -> usize {
        self.indirect_draw_parameters.len()
    }

    pub fn clear(&mut self) {
        self.indirect_draw_parameters.clear();
        self.cpu_metadata.clear();
        self.gpu_metadata.clear();
        self.batch_sets.clear();
    }
}

impl FromWorld for GpuPreprocessingSupport {
    fn from_world(world: &mut World) -> Self {
        let adapter = world.resource::<RenderAdapter>();
        let device = world.resource::<RenderDevice>();

        fn is_non_supported_android_device(adapter_info: &RenderAdapterInfo) -> bool {
            crate::get_adreno_model(adapter_info).is_some_and(|model| model != 720 && model <= 730)
                || crate::get_mali_driver_version(adapter_info).is_some_and(|version| version < 48)
        }
        fn is_preprocessing_only_android_device(adapter_info: &RenderAdapterInfo) -> bool {
            crate::get_pixel10_driver_version(adapter_info).is_some()
        }

        let culling_feature_support = device
            .features()
            .contains(Features::INDIRECT_FIRST_INSTANCE | Features::IMMEDIATES);

        let limit_support = device.limits().max_storage_textures_per_shader_stage >= 12 &&
            device.limits().max_storage_buffers_per_shader_stage >= 10 &&

            device.limits().max_compute_workgroup_storage_size != 0;

        let downlevel_support = adapter
            .get_downlevel_capabilities()
            .flags
            .contains(DownlevelFlags::COMPUTE_SHADERS);

        let adapter_info = RenderAdapterInfo(WgpuWrapper::new(adapter.get_info()));

        let max_supported_mode = if device.limits().max_compute_workgroup_size_x == 0
            || is_non_supported_android_device(&adapter_info)
            || adapter_info.backend == wgpu::Backend::Gl
        {
            info_once!(
                "GPU preprocessing is not supported on this device. \
                Falling back to CPU preprocessing.",
            );
            GpuPreprocessingMode::None
        } else if !(culling_feature_support && limit_support && downlevel_support)
            || is_preprocessing_only_android_device(&adapter_info)
        {
            info_once!("Some GPU preprocessing are limited on this device.");
            GpuPreprocessingMode::PreprocessingOnly
        } else {
            info_once!("GPU preprocessing is fully supported on this device.");
            GpuPreprocessingMode::Culling
        };

        GpuPreprocessingSupport { max_supported_mode }
    }
}

impl<BD, BDI> BatchedInstanceBuffers<BD, BDI>
where
    BD: GpuArrayBufferable + Sync + Send + 'static,
    BDI: AtomicPod,
{

    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        for phase_instance_buffer in self.phase_instance_buffers.values_mut() {
            phase_instance_buffer.clear();
        }
    }
}

impl<BD> UntypedPhaseBatchedInstanceBuffers<BD>
where
    BD: GpuArrayBufferable + Sync + Send + 'static,
{
    pub fn new() -> Self {
        UntypedPhaseBatchedInstanceBuffers {
            data_buffer: UninitBufferVec::new(BufferUsages::STORAGE),
            work_item_buffers: HashMap::default(),
            late_indexed_indirect_parameters_buffer: RawBufferVec::new(
                BufferUsages::STORAGE | BufferUsages::INDIRECT,
            ),
            late_non_indexed_indirect_parameters_buffer: RawBufferVec::new(
                BufferUsages::STORAGE | BufferUsages::INDIRECT,
            ),
        }
    }

    pub fn instance_data_binding(&self) -> Option<BindingResource<'_>> {
        self.data_buffer
            .buffer()
            .map(|buffer| buffer.as_entire_binding())
    }

    pub fn clear(&mut self) {
        self.data_buffer.clear();
        self.late_indexed_indirect_parameters_buffer.clear();
        self.late_non_indexed_indirect_parameters_buffer.clear();

        for view_work_item_buffers in self.work_item_buffers.values_mut() {
            view_work_item_buffers.clear();
        }
    }
}

impl<BD> Default for UntypedPhaseBatchedInstanceBuffers<BD>
where
    BD: GpuArrayBufferable + Sync + Send + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

struct SortedRenderBatchSet<F>
where
    F: GetBatchData,
{

    phase_item_start_index: u32,

    instance_start_index: u32,

    indexed: bool,

    indirect_parameters_index_range: Option<Range<u32>>,

    meta: Option<(BatchSetMeta<F::BatchSetCompareData>, F::BatchCompareData)>,
}

impl<F> SortedRenderBatchSet<F>
where
    F: GetBatchData,
{

    fn flush<I>(
        self,
        instance_end_index: u32,
        phase: &mut SortedRenderPhase<I>,
        phase_indirect_parameters_buffers: &mut UntypedPhaseIndirectParametersBuffers,
    ) where
        I: CachedRenderPipelinePhaseItem + SortedPhaseItem,
    {
        let (batch_range, batch_extra_index) =
            phase.items[self.phase_item_start_index as usize].batch_range_and_extra_index_mut();
        *batch_range = self.instance_start_index..instance_end_index;
        *batch_extra_index = match self.indirect_parameters_index_range {
            Some(ref indirect_parameters_index_range) => {
                PhaseItemExtraIndex::IndirectParametersIndex {
                    range: (*indirect_parameters_index_range).clone(),
                    batch_set_index: None,
                }
            }
            None => PhaseItemExtraIndex::None,
        };
        if let Some(ref indirect_parameters_index_range) = self.indirect_parameters_index_range {
            phase_indirect_parameters_buffers
                .add_batch_set(self.indexed, indirect_parameters_index_range.start);
        }
    }
}

pub fn clear_batched_gpu_instance_buffers<GFBD>(
    gpu_batched_instance_buffers: Option<
        ResMut<BatchedInstanceBuffers<GFBD::BufferData, GFBD::BufferInputData>>,
    >,
) where
    GFBD: GetFullBatchData,
{

    if let Some(mut gpu_batched_instance_buffers) = gpu_batched_instance_buffers {
        gpu_batched_instance_buffers.clear();
    }
}

pub fn delete_old_work_item_buffers<GFBD>(
    mut gpu_batched_instance_buffers: ResMut<
        BatchedInstanceBuffers<GFBD::BufferData, GFBD::BufferInputData>,
    >,
    extracted_views: Query<&ExtractedView>,
) where
    GFBD: GetFullBatchData,
{
    let retained_view_entities: HashSet<_> = extracted_views
        .iter()
        .map(|extracted_view| extracted_view.retained_view_entity)
        .collect();
    for phase_instance_buffers in gpu_batched_instance_buffers
        .phase_instance_buffers
        .values_mut()
    {
        phase_instance_buffers
            .work_item_buffers
            .retain(|retained_view_entity, _| {
                retained_view_entities.contains(retained_view_entity)
            });
    }
}

pub fn batch_and_prepare_sorted_render_phase<I, GFBD>(
    mut phase_batched_instance_buffers: ResMut<PhaseBatchedInstanceBuffers<I, GFBD::BufferData>>,
    mut phase_indirect_parameters_buffers: ResMut<PhaseIndirectParametersBuffers<I>>,
    mut sorted_render_phases: ResMut<ViewSortedRenderPhases<I>>,
    mut views: Query<(
        &ExtractedView,
        Has<NoIndirectDrawing>,
        Has<OcclusionCulling>,
    )>,
    system_param_item: StaticSystemParam<GFBD::Param>,
) where
    I: CachedRenderPipelinePhaseItem + SortedPhaseItem,
    GFBD: GetFullBatchData,
{

    let UntypedPhaseBatchedInstanceBuffers {
        ref mut data_buffer,
        ref mut work_item_buffers,
        ref mut late_indexed_indirect_parameters_buffer,
        ref mut late_non_indexed_indirect_parameters_buffer,
    } = phase_batched_instance_buffers.buffers;

    for (extracted_view, no_indirect_drawing, gpu_occlusion_culling) in &mut views {
        let Some(phase) = sorted_render_phases.get_mut(&extracted_view.retained_view_entity) else {
            continue;
        };

        let work_item_buffer = get_or_create_work_item_buffer::<I>(
            work_item_buffers,
            extracted_view.retained_view_entity,
            no_indirect_drawing,
            gpu_occlusion_culling,
        );

        init_work_item_buffers(
            work_item_buffer,
            late_indexed_indirect_parameters_buffer,
            late_non_indexed_indirect_parameters_buffer,
        );

        let mut batch_set: Option<SortedRenderBatchSet<GFBD>> = None;

        for current_index in 0..phase.items.len() {

            let item = &phase.items[current_index];
            let entity = item.main_entity();
            let item_is_indexed = item.indexed();
            let current_batch_input_index =
                GFBD::get_index_and_compare_data(&system_param_item, entity);

            let Some((current_input_index, current_meta)) = current_batch_input_index else {

                if let Some(batch_set) = batch_set.take() {
                    batch_set.flush(
                        data_buffer.len() as u32,
                        phase,
                        &mut phase_indirect_parameters_buffers.buffers,
                    );
                }

                continue;
            };
            let current_meta = if I::AUTOMATIC_BATCHING {
                current_meta.map(|(batch_set_meta, batch_meta)| {
                    (
                        BatchSetMeta::new(&phase.items[current_index], batch_set_meta),
                        batch_meta,
                    )
                })
            } else {
                None
            };

            let can_batch = match batch_set.as_ref() {
                None => SortedPhaseItemBatchability::BreakBatchSet,
                Some(batch_set) => match (&current_meta, &batch_set.meta) {
                    (
                        &Some((ref current_batch_set_key, ref current_bin_key)),
                        &Some((ref batch_set_key, ref bin_key)),
                    ) => {
                        if *current_batch_set_key == *batch_set_key {
                            if *current_bin_key == *bin_key {
                                SortedPhaseItemBatchability::BatchOk
                            } else if no_indirect_drawing {

                                SortedPhaseItemBatchability::BreakBatchSet
                            } else {
                                SortedPhaseItemBatchability::BreakBatch
                            }
                        } else {
                            SortedPhaseItemBatchability::BreakBatchSet
                        }
                    }
                    _ => SortedPhaseItemBatchability::BreakBatchSet,
                },
            };

            let output_index = data_buffer.add() as u32;

            match can_batch {
                SortedPhaseItemBatchability::BreakBatchSet => {

                    if let Some(batch_set) = batch_set.take() {
                        batch_set.flush(
                            output_index,
                            phase,
                            &mut phase_indirect_parameters_buffers.buffers,
                        );
                    }

                    let indirect_parameters_index = phase_indirect_parameters_buffers
                        .allocate(no_indirect_drawing, item_is_indexed);

                    if let Some(indirect_parameters_index) = indirect_parameters_index {
                        GFBD::write_batch_indirect_parameters_metadata(
                            item_is_indexed,
                            output_index,
                            None,
                            &mut phase_indirect_parameters_buffers.buffers,
                            indirect_parameters_index,
                        );
                    }

                    batch_set = Some(SortedRenderBatchSet {
                        phase_item_start_index: current_index as u32,
                        instance_start_index: output_index,
                        indexed: item_is_indexed,
                        indirect_parameters_index_range: indirect_parameters_index
                            .map(|i| i..(i + 1)),
                        meta: current_meta,
                    });
                }

                SortedPhaseItemBatchability::BreakBatch => {

                    let maybe_indirect_parameters_index = phase_indirect_parameters_buffers
                        .allocate(no_indirect_drawing, item_is_indexed);

                    if let (&mut Some(ref mut batch_set), Some(indirect_parameters_index)) =
                        (&mut batch_set, maybe_indirect_parameters_index)
                    {
                        GFBD::write_batch_indirect_parameters_metadata(
                            item_is_indexed,
                            output_index,
                            None,
                            &mut phase_indirect_parameters_buffers.buffers,
                            indirect_parameters_index,
                        );

                        batch_set.meta = current_meta;

                        let indirect_parameters_index_range = batch_set
                            .indirect_parameters_index_range
                            .as_mut()
                            .expect("Can't allocate in a multidraw set if we aren't multidrawing");
                        debug_assert_eq!(
                            indirect_parameters_index,
                            indirect_parameters_index_range.end
                        );
                        indirect_parameters_index_range.end += 1;
                    }
                }

                SortedPhaseItemBatchability::BatchOk => {}
            };

            if let Some(batch_set) = batch_set.as_ref() {
                work_item_buffer.push(
                    item_is_indexed,
                    PreprocessWorkItem {
                        input_index: current_input_index.into(),
                        output_or_indirect_parameters_index: match (
                            no_indirect_drawing,
                            &batch_set.indirect_parameters_index_range,
                        ) {
                            (true, _) => output_index,
                            (false, Some(indirect_parameters_index_range)) => {
                                indirect_parameters_index_range.end - 1
                            }
                            (false, None) => 0,
                        },
                    },
                );
            }
        }

        if let Some(batch_set) = batch_set.take() {
            batch_set.flush(
                data_buffer.len() as u32,
                phase,
                &mut phase_indirect_parameters_buffers.buffers,
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum SortedPhaseItemBatchability {

    BatchOk,

    BreakBatch,

    BreakBatchSet,
}

pub fn batch_and_prepare_binned_render_phase<BPI, GFBD>(
    mut phase_batched_instance_buffers: ResMut<PhaseBatchedInstanceBuffers<BPI, GFBD::BufferData>>,
    phase_indirect_parameters_buffers: ResMut<PhaseIndirectParametersBuffers<BPI>>,
    mut binned_render_phases: ResMut<ViewBinnedRenderPhases<BPI>>,
    mut views: Query<
        (
            &ExtractedView,
            Has<NoIndirectDrawing>,
            Has<OcclusionCulling>,
        ),
        With<ExtractedView>,
    >,
    param: StaticSystemParam<GFBD::Param>,
) where
    BPI: BinnedPhaseItem,
    GFBD: GetFullBatchData,
{
    let system_param_item = param.into_inner();

    let phase_indirect_parameters_buffers = phase_indirect_parameters_buffers.into_inner();

    let UntypedPhaseBatchedInstanceBuffers {
        ref mut data_buffer,
        ref mut work_item_buffers,
        ref mut late_indexed_indirect_parameters_buffer,
        ref mut late_non_indexed_indirect_parameters_buffer,
    } = phase_batched_instance_buffers.buffers;

    for (extracted_view, no_indirect_drawing, gpu_occlusion_culling) in &mut views {
        let Some(phase) = binned_render_phases.get_mut(&extracted_view.retained_view_entity) else {
            continue;
        };

        let work_item_buffer = get_or_create_work_item_buffer::<BPI>(
            work_item_buffers,
            extracted_view.retained_view_entity,
            no_indirect_drawing,
            gpu_occlusion_culling,
        );

        init_work_item_buffers(
            work_item_buffer,
            late_indexed_indirect_parameters_buffer,
            late_non_indexed_indirect_parameters_buffer,
        );

        for (key, unbatchables) in &mut phase.unbatchable_meshes {

            let mut indirect_parameters_offset = if no_indirect_drawing {
                None
            } else if key.0.indexed() {
                Some(
                    phase_indirect_parameters_buffers
                        .buffers
                        .indexed
                        .allocate(unbatchables.entities.len() as u32),
                )
            } else {
                Some(
                    phase_indirect_parameters_buffers
                        .buffers
                        .non_indexed
                        .allocate(unbatchables.entities.len() as u32),
                )
            };

            for main_entity in unbatchables.entities.keys() {
                let Some(input_index) = GFBD::get_binned_index(&system_param_item, *main_entity)
                else {
                    continue;
                };
                let output_index = data_buffer.add() as u32;

                if let Some(ref mut indirect_parameters_index) = indirect_parameters_offset {

                    GFBD::write_batch_indirect_parameters_metadata(
                        key.0.indexed(),
                        output_index,
                        None,
                        &mut phase_indirect_parameters_buffers.buffers,
                        *indirect_parameters_index,
                    );
                    work_item_buffer.push(
                        key.0.indexed(),
                        PreprocessWorkItem {
                            input_index: input_index.into(),
                            output_or_indirect_parameters_index: *indirect_parameters_index,
                        },
                    );
                    unbatchables
                        .buffer_indices
                        .add(UnbatchableBinnedEntityIndices {
                            instance_index: *indirect_parameters_index,
                            extra_index: PhaseItemExtraIndex::IndirectParametersIndex {
                                range: *indirect_parameters_index..(*indirect_parameters_index + 1),
                                batch_set_index: None,
                            },
                        });
                    phase_indirect_parameters_buffers
                        .buffers
                        .add_batch_set(key.0.indexed(), *indirect_parameters_index);
                    *indirect_parameters_index += 1;
                } else {
                    work_item_buffer.push(
                        key.0.indexed(),
                        PreprocessWorkItem {
                            input_index: input_index.into(),
                            output_or_indirect_parameters_index: output_index,
                        },
                    );
                    unbatchables
                        .buffer_indices
                        .add(UnbatchableBinnedEntityIndices {
                            instance_index: output_index,
                            extra_index: PhaseItemExtraIndex::None,
                        });
                }
            }
        }

        for (key, bin) in &phase.batchable_meshes {
            let mut batch: Option<BinnedRenderPhaseBatch> = None;
            for (&main_entity, &input_index) in bin.entities() {
                let output_index = data_buffer.add() as u32;

                match batch {
                    Some(ref mut batch) => {
                        batch.instance_range.end = output_index + 1;

                        work_item_buffer.push(
                            key.0.indexed(),
                            PreprocessWorkItem {
                                input_index: *input_index,
                                output_or_indirect_parameters_index: match (
                                    no_indirect_drawing,
                                    &batch.extra_index,
                                ) {
                                    (true, _) => output_index,
                                    (
                                        false,
                                        PhaseItemExtraIndex::IndirectParametersIndex {
                                            range: indirect_parameters_range,
                                            ..
                                        },
                                    ) => indirect_parameters_range.start,
                                    (false, &PhaseItemExtraIndex::DynamicOffset(_))
                                    | (false, &PhaseItemExtraIndex::None) => 0,
                                },
                            },
                        );
                    }

                    None if !no_indirect_drawing => {

                        let indirect_parameters_index = phase_indirect_parameters_buffers
                            .buffers
                            .allocate(key.0.indexed(), 1);
                        let batch_set_index = phase_indirect_parameters_buffers
                            .buffers
                            .get_next_batch_set_index(key.0.indexed());

                        GFBD::write_batch_indirect_parameters_metadata(
                            key.0.indexed(),
                            output_index,
                            batch_set_index,
                            &mut phase_indirect_parameters_buffers.buffers,
                            indirect_parameters_index,
                        );
                        work_item_buffer.push(
                            key.0.indexed(),
                            PreprocessWorkItem {
                                input_index: *input_index,
                                output_or_indirect_parameters_index: indirect_parameters_index,
                            },
                        );
                        batch = Some(BinnedRenderPhaseBatch {
                            representative_entity: (Entity::PLACEHOLDER, main_entity),
                            instance_range: output_index..output_index + 1,
                            extra_index: PhaseItemExtraIndex::IndirectParametersIndex {
                                range: indirect_parameters_index..(indirect_parameters_index + 1),
                                batch_set_index: None,
                            },
                        });
                    }

                    None => {

                        work_item_buffer.push(
                            key.0.indexed(),
                            PreprocessWorkItem {
                                input_index: *input_index,
                                output_or_indirect_parameters_index: output_index,
                            },
                        );
                        batch = Some(BinnedRenderPhaseBatch {
                            representative_entity: (Entity::PLACEHOLDER, main_entity),
                            instance_range: output_index..output_index + 1,
                            extra_index: PhaseItemExtraIndex::None,
                        });
                    }
                }
            }

            if let Some(batch) = batch {
                match phase.batch_sets {
                    BinnedRenderPhaseBatchSets::DynamicUniforms(_) => {
                        error!("Dynamic uniform batch sets shouldn't be used here");
                    }
                    BinnedRenderPhaseBatchSets::Direct(ref mut vec) => {
                        vec.push(batch);
                    }
                    BinnedRenderPhaseBatchSets::MultidrawIndirect(ref mut vec) => {

                        vec.push(BinnedRenderPhaseBatchSet {
                            first_batch: batch,
                            batch_count: 1,
                            bin_key: key.1.clone(),
                            index: phase_indirect_parameters_buffers
                                .buffers
                                .batch_set_count(key.0.indexed())
                                as u32,

                            first_work_item_index: 0,
                        });
                    }
                }
            }
        }

        if let (
            &mut BinnedRenderPhaseBatchSets::MultidrawIndirect(ref mut batch_sets),
            &mut PreprocessWorkItemBuffers::Indirect {
                indexed: ref mut indexed_work_item_buffer,
                non_indexed: ref mut non_indexed_work_item_buffer,
                gpu_occlusion_culling: ref mut gpu_occlusion_culling_buffers,
            },
        ) = (&mut phase.batch_sets, &mut *work_item_buffer)
        {

            let mut indexed_preparer: MultidrawableBatchSetPreparer<BPI, GFBD> =
                MultidrawableBatchSetPreparer::new(
                    phase_indirect_parameters_buffers.buffers.batch_count(true) as u32,
                    phase_indirect_parameters_buffers
                        .buffers
                        .indexed
                        .batch_sets
                        .len() as u32,
                );
            let mut non_indexed_preparer: MultidrawableBatchSetPreparer<BPI, GFBD> =
                MultidrawableBatchSetPreparer::new(
                    phase_indirect_parameters_buffers.buffers.batch_count(false) as u32,
                    phase_indirect_parameters_buffers
                        .buffers
                        .non_indexed
                        .batch_sets
                        .len() as u32,
                );

            for (batch_set_key, bins) in &phase.multidrawable_meshes {
                if batch_set_key.indexed() {
                    indexed_preparer.prepare_multidrawable_binned_batch_set(
                        bins,
                        data_buffer,
                        indexed_work_item_buffer,
                        &mut phase_indirect_parameters_buffers.buffers.indexed,
                        batch_sets,
                    );
                } else {
                    non_indexed_preparer.prepare_multidrawable_binned_batch_set(
                        bins,
                        data_buffer,
                        non_indexed_work_item_buffer,
                        &mut phase_indirect_parameters_buffers.buffers.non_indexed,
                        batch_sets,
                    );
                }
            }

            if let Some(gpu_occlusion_culling_buffers) = gpu_occlusion_culling_buffers {
                gpu_occlusion_culling_buffers
                    .late_indexed
                    .add_multiple(indexed_preparer.work_item_count);
                gpu_occlusion_culling_buffers
                    .late_non_indexed
                    .add_multiple(non_indexed_preparer.work_item_count);
            }
        }
    }
}

struct MultidrawableBatchSetPreparer<BPI, GFBD>
where
    BPI: BinnedPhaseItem,
    GFBD: GetFullBatchData,
{

    indirect_parameters_index: u32,

    batch_set_index: u32,

    work_item_count: usize,
    phantom: PhantomData<(BPI, GFBD)>,
}

impl<BPI, GFBD> MultidrawableBatchSetPreparer<BPI, GFBD>
where
    BPI: BinnedPhaseItem,
    GFBD: GetFullBatchData,
{

    #[inline]
    fn new(initial_indirect_parameters_index: u32, initial_batch_set_index: u32) -> Self {
        MultidrawableBatchSetPreparer {
            indirect_parameters_index: initial_indirect_parameters_index,
            batch_set_index: initial_batch_set_index,
            work_item_count: 0,
            phantom: PhantomData,
        }
    }

    #[inline]
    fn prepare_multidrawable_binned_batch_set<IP>(
        &mut self,
        batch_set: &RenderMultidrawableBatchSet<BPI>,
        data_buffer: &mut UninitBufferVec<GFBD::BufferData>,
        work_item_buffer: &mut PartialBufferVec<PreprocessWorkItem>,
        mesh_class_buffers: &mut MeshClassIndirectParametersBuffers<IP>,
        batch_sets: &mut Vec<BinnedRenderPhaseBatchSet<BPI::BinKey>>,
    ) where
        IP: Clone + ShaderSize + WriteInto,
    {
        let current_indexed_batch_set_index = self.batch_set_index;
        let current_output_index = data_buffer.len() as u32;
        let first_work_item_index = work_item_buffer.len() as u32;

        let indirect_parameters_base = self.indirect_parameters_index;

        let Some((first_bin_key, first_bin_index)) = batch_set.bin_key_to_bin_index.iter().next()
        else {
            return;
        };
        let first_bin = batch_set
            .bin(*first_bin_index)
            .expect("At least one bin must be present in each batch set");
        let first_bin_len = first_bin.entity_to_binned_mesh_instance_index.len();
        let first_bin_entity = batch_set
            .representative_entity()
            .unwrap_or(MainEntity::from(Entity::PLACEHOLDER));

        let cpu_metadata_offset = mesh_class_buffers.cpu_metadata.len() as u32;
        for _ in 0..batch_set.bin_count() {
            mesh_class_buffers
                .cpu_metadata
                .push(IndirectParametersCpuMetadata {

                    base_output_index: 0,
                    batch_set_index: self.batch_set_index,
                });
        }

        for bin_index in batch_set.bin_key_to_bin_index.values() {
            let bin = batch_set.bin(*bin_index).expect("Bin not present");

            let indirect_parameters_offset = *batch_set
                .gpu_buffers
                .bin_index_to_indirect_parameters_offset_buffer
                .get(bin_index.0)
                .unwrap();
            mesh_class_buffers.cpu_metadata.values_mut()
                [cpu_metadata_offset as usize + indirect_parameters_offset as usize]
                .base_output_index = data_buffer.len() as u32;

            let bin_entity_count = bin.entity_to_binned_mesh_instance_index.len();
            work_item_buffer.push_multiple_uninit(bin_entity_count);
            data_buffer.add_multiple(bin_entity_count);
            self.work_item_count += bin_entity_count;
        }

        let bin_count = batch_set.bin_count();
        mesh_class_buffers.gpu_metadata.add_multiple(bin_count);
        mesh_class_buffers
            .indirect_draw_parameters
            .add_multiple(bin_count);

        mesh_class_buffers.batch_sets.push(IndirectBatchSet {
            indirect_parameters_base,
            indirect_parameters_count: 0,
        });

        self.indirect_parameters_index += bin_count as u32;
        self.batch_set_index += 1;

        batch_sets.push(BinnedRenderPhaseBatchSet {
            first_batch: BinnedRenderPhaseBatch {
                representative_entity: (Entity::PLACEHOLDER, first_bin_entity),
                instance_range: current_output_index..(current_output_index + first_bin_len as u32),
                extra_index: PhaseItemExtraIndex::maybe_indirect_parameters_index(NonMaxU32::new(
                    indirect_parameters_base,
                )),
            },
            bin_key: (*first_bin_key).clone(),
            batch_count: self.indirect_parameters_index - indirect_parameters_base,
            index: current_indexed_batch_set_index,
            first_work_item_index,
        });
    }
}

pub fn collect_buffers_for_phase<PI, GFBD>(
    mut phase_batched_instance_buffers: ResMut<PhaseBatchedInstanceBuffers<PI, GFBD::BufferData>>,
    mut phase_indirect_parameters_buffers: ResMut<PhaseIndirectParametersBuffers<PI>>,
    mut batched_instance_buffers: ResMut<
        BatchedInstanceBuffers<GFBD::BufferData, GFBD::BufferInputData>,
    >,
    mut indirect_parameters_buffers: ResMut<IndirectParametersBuffers>,
    indirect_parameters_buffers_settings: Res<IndirectParametersBuffersSettings>,
) where
    PI: PhaseItem,
    GFBD: GetFullBatchData + Send + Sync + 'static,
{

    let untyped_phase_batched_instance_buffers =
        mem::take(&mut phase_batched_instance_buffers.buffers);
    if let Some(mut old_untyped_phase_batched_instance_buffers) = batched_instance_buffers
        .phase_instance_buffers
        .insert(TypeId::of::<PI>(), untyped_phase_batched_instance_buffers)
    {
        old_untyped_phase_batched_instance_buffers.clear();
        phase_batched_instance_buffers.buffers = old_untyped_phase_batched_instance_buffers;
    }

    let untyped_phase_indirect_parameters_buffers = mem::replace(
        &mut phase_indirect_parameters_buffers.buffers,
        UntypedPhaseIndirectParametersBuffers::new(
            indirect_parameters_buffers_settings.allow_copies_from_indirect_parameter_buffers,
        ),
    );
    if let Some(mut old_untyped_phase_indirect_parameters_buffers) = indirect_parameters_buffers
        .insert(
            TypeId::of::<PI>(),
            untyped_phase_indirect_parameters_buffers,
        )
    {
        old_untyped_phase_indirect_parameters_buffers.clear();
        phase_indirect_parameters_buffers.buffers = old_untyped_phase_indirect_parameters_buffers;
    }
}

pub fn write_batched_instance_buffers<GFBD>(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    gpu_array_buffer: ResMut<BatchedInstanceBuffers<GFBD::BufferData, GFBD::BufferInputData>>,
    pipeline_cache: Res<PipelineCache>,
    mut bin_unpacking_buffers: ResMut<BinUnpackingBuffers>,
    mut sparse_buffer_update_jobs: ResMut<SparseBufferUpdateJobs>,
    mut sparse_buffer_update_bind_groups: ResMut<SparseBufferUpdateBindGroups>,
    sparse_buffer_update_pipelines: Res<SparseBufferUpdatePipelines>,
) where
    GFBD: GetFullBatchData,
{
    let BatchedInstanceBuffers {
        current_input_buffer,
        previous_input_buffer,
        phase_instance_buffers,
    } = gpu_array_buffer.into_inner();

    let render_device = &*render_device;
    let render_queue = &*render_queue;

    ComputeTaskPool::get().scope(|scope| {
        scope.spawn(async {
            #[cfg(feature = "trace")]
            let _span = bevy_log::info_span!("write_current_input_buffers").entered();
            current_input_buffer
                .buffer
                .write_buffers(render_device, render_queue);
        });
        scope.spawn(async {
            #[cfg(feature = "trace")]
            let _span = bevy_log::info_span!("write_previous_input_buffers").entered();
            previous_input_buffer.write_buffer(render_device, render_queue);
        });

        for phase_instance_buffers in phase_instance_buffers.values_mut() {
            let UntypedPhaseBatchedInstanceBuffers {
                ref mut data_buffer,
                ref mut work_item_buffers,
                ref mut late_indexed_indirect_parameters_buffer,
                ref mut late_non_indexed_indirect_parameters_buffer,
            } = *phase_instance_buffers;

            scope.spawn(async {
                #[cfg(feature = "trace")]
                let _span = bevy_log::info_span!("write_phase_instance_buffers").entered();
                data_buffer.write_buffer(render_device);
                late_indexed_indirect_parameters_buffer.write_buffer(render_device, render_queue);
                late_non_indexed_indirect_parameters_buffer
                    .write_buffer(render_device, render_queue);
            });

            for phase_work_item_buffers in work_item_buffers.values_mut() {
                scope.spawn(async {
                    #[cfg(feature = "trace")]
                    let _span = bevy_log::info_span!("write_work_item_buffers").entered();
                    match *phase_work_item_buffers {
                        PreprocessWorkItemBuffers::Direct(ref mut buffer_vec) => {
                            buffer_vec.write_buffer(render_device, render_queue);
                        }
                        PreprocessWorkItemBuffers::Indirect {
                            ref mut indexed,
                            ref mut non_indexed,
                            ref mut gpu_occlusion_culling,
                        } => {
                            indexed.write_buffer(render_device, render_queue);
                            non_indexed.write_buffer(render_device, render_queue);

                            if let Some(GpuOcclusionCullingWorkItemBuffers {
                                ref mut late_indexed,
                                ref mut late_non_indexed,
                                late_indirect_parameters_indexed_offset: _,
                                late_indirect_parameters_non_indexed_offset: _,
                            }) = *gpu_occlusion_culling
                            {
                                if !late_indexed.is_empty() {
                                    late_indexed.write_buffer(render_device);
                                }
                                if !late_non_indexed.is_empty() {
                                    late_non_indexed.write_buffer(render_device);
                                }
                            }
                        }
                    }
                });
            }
        }
    });

    current_input_buffer.buffer.prepare_to_populate_buffers(
        render_device,
        &pipeline_cache,
        &mut sparse_buffer_update_jobs,
        &mut sparse_buffer_update_bind_groups,
        &sparse_buffer_update_pipelines,
    );

    bin_unpacking_buffers
        .bin_unpacking_metadata
        .write_buffer(render_device, render_queue);
}

pub fn write_binned_instance_buffers<BPI, GFBD>(
    mut views: Query<&ExtractedView>,
    mut view_binned_render_phases: ResMut<ViewBinnedRenderPhases<BPI>>,
    bin_unpacking_buffers: ResMut<BinUnpackingBuffers>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) where
    BPI: BinnedPhaseItem,
    GFBD: GetFullBatchData,
{
    let bin_unpacking_buffers = bin_unpacking_buffers.into_inner();

    let phase_type_id = TypeId::of::<BPI>();

    let mut all_seen_view_entities = HashSet::new();

    for extracted_view in &mut views {
        all_seen_view_entities.insert(extracted_view.retained_view_entity);

        let Some(view_binned_render_phase) =
            view_binned_render_phases.get_mut(&extracted_view.retained_view_entity)
        else {
            continue;
        };

        let BinnedRenderPhaseBatchSets::MultidrawIndirect(ref batch_sets) =
            view_binned_render_phase.batch_sets
        else {
            continue;
        };

        let view_phase_bin_unpacking_buffers = bin_unpacking_buffers
            .view_phase_buffers
            .entry(BinUnpackingBuffersKey {
                phase: phase_type_id,
                view: extracted_view.retained_view_entity,
            })
            .or_default();

        view_phase_bin_unpacking_buffers
            .indexed_unpacking_jobs
            .clear();
        view_phase_bin_unpacking_buffers
            .non_indexed_unpacking_jobs
            .clear();

        let mut representative_entity_to_batch_set_bin_unpacking_metadata =
            MainEntityHashMap::default();

        for batch_set in batch_sets {
            let main_entity = batch_set.first_batch.representative_entity.1;
            if *main_entity != Entity::PLACEHOLDER
                && let PhaseItemExtraIndex::IndirectParametersIndex {
                    range: ref indirect_parameters_range,
                    ..
                } = batch_set.first_batch.extra_index
            {

                representative_entity_to_batch_set_bin_unpacking_metadata.insert(
                    main_entity,
                    BatchSetBinUnpackingMetadata {
                        base_output_work_item_index: batch_set.first_work_item_index,
                        base_indirect_parameters_index: indirect_parameters_range.start,
                    },
                );
            }
        }

        for (batch_set_key, batch_set) in view_binned_render_phase.multidrawable_meshes.iter_mut() {
            let Some(representative_entity) = batch_set.representative_entity() else {
                continue;
            };
            let Some(bin_unpacking_metadata) =
                representative_entity_to_batch_set_bin_unpacking_metadata
                    .get(&representative_entity)
            else {
                continue;
            };

            batch_set
                .gpu_buffers
                .render_binned_mesh_instance_buffer
                .write_buffer(&render_device, &render_queue);
            batch_set
                .gpu_buffers
                .bin_index_to_indirect_parameters_offset_buffer
                .write_buffer(&render_device, &render_queue);

            let (
                Some(render_bin_entry_buffer),
                Some(bin_index_to_indirect_parameters_offset_buffer),
            ) = (
                batch_set
                    .gpu_buffers
                    .render_binned_mesh_instance_buffer
                    .buffer(),
                batch_set
                    .gpu_buffers
                    .bin_index_to_indirect_parameters_offset_buffer
                    .buffer(),
            )
            else {
                continue;
            };

            let binned_mesh_instance_count = batch_set
                .gpu_buffers
                .render_binned_mesh_instance_buffer
                .len() as u32;

            let gpu_bin_unpacking_metadata_index = bin_unpacking_buffers
                .bin_unpacking_metadata
                .push(GpuBinUnpackingMetadata {
                    base_output_work_item_index: bin_unpacking_metadata.base_output_work_item_index,
                    base_indirect_parameters_index: bin_unpacking_metadata
                        .base_indirect_parameters_index,
                    binned_mesh_instance_count,
                    pad: [0; _],
                });

            let Some(gpu_bin_unpacking_metadata_index) =
                NonMaxU32::new(gpu_bin_unpacking_metadata_index as u32)
            else {
                continue;
            };

            let job = BinUnpackingJob {
                render_binned_mesh_instance_buffer: render_bin_entry_buffer.clone(),
                bin_index_to_indirect_parameters_offset_buffer:
                    bin_index_to_indirect_parameters_offset_buffer.clone(),
                bin_unpacking_metadata_index: BinUnpackingMetadataIndex(
                    gpu_bin_unpacking_metadata_index,
                ),
                mesh_instance_count: binned_mesh_instance_count,
            };

            if batch_set_key.indexed() {
                view_phase_bin_unpacking_buffers
                    .indexed_unpacking_jobs
                    .push(job);
            } else {
                view_phase_bin_unpacking_buffers
                    .non_indexed_unpacking_jobs
                    .push(job);
            }
        }
    }

    bin_unpacking_buffers
        .view_phase_buffers
        .retain(|bin_unpacking_buffers_key, _| {
            bin_unpacking_buffers_key.phase != phase_type_id
                || all_seen_view_entities.contains(&bin_unpacking_buffers_key.view)
        });
}

pub fn clear_bin_unpacking_buffers(mut bin_unpacking_buffers: ResMut<BinUnpackingBuffers>) {
    bin_unpacking_buffers.bin_unpacking_metadata.clear();
}

struct BatchSetBinUnpackingMetadata {

    base_output_work_item_index: u32,

    base_indirect_parameters_index: u32,
}

pub fn clear_indirect_parameters_buffers(
    mut indirect_parameters_buffers: ResMut<IndirectParametersBuffers>,
) {
    for phase_indirect_parameters_buffers in indirect_parameters_buffers.values_mut() {
        phase_indirect_parameters_buffers.clear();
    }
}

pub fn write_indirect_parameters_buffers(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut indirect_parameters_buffers: ResMut<IndirectParametersBuffers>,
) {
    let render_device = &*render_device;
    let render_queue = &*render_queue;
    ComputeTaskPool::get().scope(|scope| {
        for phase_indirect_parameters_buffers in indirect_parameters_buffers.values_mut() {
            scope.spawn(async {
                #[cfg(feature = "trace")]
                let _span = bevy_log::info_span!("indexed_data").entered();
                phase_indirect_parameters_buffers
                    .indexed
                    .indirect_draw_parameters
                    .write_buffer(render_device);
            });
            scope.spawn(async {
                #[cfg(feature = "trace")]
                let _span = bevy_log::info_span!("non_indexed_data").entered();
                phase_indirect_parameters_buffers
                    .non_indexed
                    .indirect_draw_parameters
                    .write_buffer(render_device);
            });

            scope.spawn(async {
                #[cfg(feature = "trace")]
                let _span = bevy_log::info_span!("indexed_cpu_metadata").entered();
                phase_indirect_parameters_buffers
                    .indexed
                    .cpu_metadata
                    .write_buffer(render_device, render_queue);
            });
            scope.spawn(async {
                #[cfg(feature = "trace")]
                let _span = bevy_log::info_span!("non_indexed_cpu_metadata").entered();
                phase_indirect_parameters_buffers
                    .non_indexed
                    .cpu_metadata
                    .write_buffer(render_device, render_queue);
            });

            scope.spawn(async {
                #[cfg(feature = "trace")]
                let _span = bevy_log::info_span!("non_indexed_gpu_metadata").entered();
                phase_indirect_parameters_buffers
                    .non_indexed
                    .gpu_metadata
                    .write_buffer(render_device);
            });
            scope.spawn(async {
                #[cfg(feature = "trace")]
                let _span = bevy_log::info_span!("indexed_gpu_metadata").entered();
                phase_indirect_parameters_buffers
                    .indexed
                    .gpu_metadata
                    .write_buffer(render_device);
            });

            scope.spawn(async {
                #[cfg(feature = "trace")]
                let _span = bevy_log::info_span!("indexed_batch_sets").entered();
                phase_indirect_parameters_buffers
                    .indexed
                    .batch_sets
                    .write_buffer(render_device, render_queue);
            });
            scope.spawn(async {
                #[cfg(feature = "trace")]
                let _span = bevy_log::info_span!("non_indexed_batch_sets").entered();
                phase_indirect_parameters_buffers
                    .non_indexed
                    .batch_sets
                    .write_buffer(render_device, render_queue);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use bytemuck::{Pod, Zeroable};

    use crate::impl_atomic_pod;

    use super::*;

    #[derive(Clone, Copy, Default, PartialEq, Debug, Pod, Zeroable)]
    #[repr(C)]
    struct TestData(u32);

    impl_atomic_pod!(TestData, TestDataBlob);

    #[test]
    fn instance_buffer_correct_behavior() {
        let mut instance_buffer = InstanceInputUniformBuffer::new();

        let index = instance_buffer.add(TestData(2));
        instance_buffer.remove(index);
        assert_eq!(instance_buffer.get_unchecked(index), TestData(2));
        assert_eq!(instance_buffer.get(index), None);

        instance_buffer.add(TestData(5));
        assert_eq!(instance_buffer.buffer().len(), 1);
    }
}
