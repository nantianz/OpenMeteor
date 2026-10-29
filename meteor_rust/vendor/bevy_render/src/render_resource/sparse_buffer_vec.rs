

use alloc::sync::{Arc, Weak};
use core::{
    iter, slice,
    sync::atomic::{AtomicU64, Ordering},
};

use bevy_app::{App, Plugin};
use bevy_asset::{embedded_asset, load_embedded_asset, Handle};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    resource::Resource,
    schedule::IntoScheduleConfigs as _,
    system::{Res, ResMut},
    world::{FromWorld, World},
};
use bevy_log::{error, info};
use bevy_material::{
    bind_group_layout_entries::{
        binding_types::{storage_buffer, storage_buffer_read_only, uniform_buffer},
        BindGroupLayoutEntries,
    },
    descriptor::{BindGroupLayoutDescriptor, CachedComputePipelineId, ComputePipelineDescriptor},
};
use bevy_shader::Shader;
use bytemuck::{Pod, Zeroable};
use encase::ShaderType;
use weak_table::WeakKeyHashMap;
use wgpu::{BufferDescriptor, BufferUsages, ComputePassDescriptor, ShaderStages};

use crate::{
    diagnostic::{DiagnosticsRecorder, RecordDiagnostics as _},
    render_resource::{
        AtomicPod, BindGroup, BindGroupEntries, Buffer, PipelineCache, RawBufferVec,
        SpecializedComputePipeline, SpecializedComputePipelines, UniformBuffer,
    },
    renderer::{RenderDevice, RenderGraph, RenderGraphSystems, RenderQueue},
    ExtractSchedule, RenderApp,
};

pub struct SparseBufferPlugin;

impl Plugin for SparseBufferPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "sparse_buffer_update.wgsl");
    }

    fn finish(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_resource::<SparseBufferUpdateJobs>()
            .init_resource::<SparseBufferUpdatePipelines>()
            .init_resource::<SpecializedComputePipelines<SparseBufferUpdatePipelines>>()
            .init_resource::<SparseBufferUpdateBindGroups>()
            .add_systems(ExtractSchedule, clear_sparse_buffer_jobs)
            .add_systems(
                RenderGraph,

                update_sparse_buffers.in_set(RenderGraphSystems::Begin),
            );
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Deref, DerefMut)]
pub struct SparseBufferId(pub u64);

pub type SparseBufferHandle = Arc<SparseBufferId>;

static NEXT_SPARSE_BUFFER_ID: AtomicU64 = AtomicU64::new(0);

const SPARSE_BUFFER_UPDATE_WORKGROUP_SIZE: u32 = 256;

const SPARSE_UPLOAD_THRESHOLD: f64 = 0.15;

const MAX_WORKGROUPS: u32 = 65535;

const REALLOCATION_FACTOR: f64 = 1.5;

const REALLOCATION_SIZE_MULTIPLE: usize = 256;

const PAGES_PER_DIRTY_WORD: u32 = 64;

#[derive(Resource)]
pub struct SparseBufferUpdatePipelines {

    bind_group_layout: Option<BindGroupLayoutDescriptor>,

    shader: Option<Handle<Shader>>,
}

#[derive(Resource)]
pub struct SparseBufferUpdateBindGroups {

    bind_groups: WeakKeyHashMap<Weak<SparseBufferId>, SparseBufferUpdateBindGroup>,

    pipeline_id: CachedComputePipelineId,
}

pub struct SparseBufferUpdateBindGroup {

    bind_group: BindGroup,
}

#[derive(Resource, Default, Deref, DerefMut)]
pub struct SparseBufferUpdateJobs(pub Vec<SparseBufferUpdateJob>);

pub struct SparseBufferUpdateJob {

    sparse_buffer_handle: SparseBufferHandle,

    updated_page_count: u32,

    page_size_log2: u32,

    element_word_size: u32,

    label: Arc<str>,
}

impl SparseBufferUpdateJob {

    fn page_size(&self) -> u32 {
        1 << self.page_size_log2
    }

    fn words_to_update(&self) -> u32 {
        self.updated_page_count * self.page_size() * self.element_word_size
    }

    fn workgroup_count(&self) -> u32 {
        self.words_to_update()
            .div_ceil(SPARSE_BUFFER_UPDATE_WORKGROUP_SIZE)
    }
}

#[derive(Clone, Copy, Default, ShaderType, Pod, Zeroable)]
#[repr(C)]
struct GpuSparseBufferUpdateMetadata {

    element_size: u32,

    updated_page_count: u32,

    page_size_log2: u32,
}

fn update_sparse_buffers(
    sparse_buffer_update_jobs: Res<SparseBufferUpdateJobs>,
    sparse_buffer_update_bind_groups: Res<SparseBufferUpdateBindGroups>,
    pipeline_cache: Res<PipelineCache>,
    mut diagnostics: Option<ResMut<DiagnosticsRecorder>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {

    if sparse_buffer_update_jobs.is_empty() {
        return;
    }

    let mut command_encoder =
        render_device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sparse buffer update"),
        });

    let time_span = diagnostics
        .as_mut()
        .map(|diagnostics| diagnostics.time_span(&mut command_encoder, "sparse buffer update"));

    command_encoder.push_debug_group("sparse buffer update");

    let Some(compute_pipeline) =
        pipeline_cache.get_compute_pipeline(sparse_buffer_update_bind_groups.pipeline_id)
    else {
        return;
    };

    for sparse_buffer_update_job in sparse_buffer_update_jobs.iter() {
        let Some(sparse_buffer_update_bind_group) = sparse_buffer_update_bind_groups
            .bind_groups
            .get(&sparse_buffer_update_job.sparse_buffer_handle)
        else {
            continue;
        };

        let mut sparse_buffer_update_pass =
            command_encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some(&*format!(
                    "sparse buffer update ({})",
                    &sparse_buffer_update_job.label
                )),
                timestamp_writes: None,
            });
        sparse_buffer_update_pass.set_pipeline(compute_pipeline);
        sparse_buffer_update_pass.set_bind_group(
            0,
            &sparse_buffer_update_bind_group.bind_group,
            &[],
        );
        sparse_buffer_update_pass.dispatch_workgroups(
            sparse_buffer_update_job.workgroup_count(),
            1,
            1,
        );
    }

    command_encoder.pop_debug_group();
    if let Some(time_span) = time_span {
        time_span.end(&mut command_encoder);
    }

    render_queue.submit([command_encoder.finish()]);
}

fn clear_sparse_buffer_jobs(mut sparse_buffer_update_jobs: ResMut<SparseBufferUpdateJobs>) {
    sparse_buffer_update_jobs.clear();
}

impl FromWorld for SparseBufferUpdatePipelines {
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        let limit = render_device.limits().max_storage_buffers_per_shader_stage;

        if limit < 3 {
            info!(
                "Sparse buffer updates disabled. RenderDevice lacks support: max_storage_buffers_per_shader_stage ({}) < 3.",
                limit
            );

            return SparseBufferUpdatePipelines {
                bind_group_layout: None,
                shader: None,
            };
        }

        let bind_group_layout = BindGroupLayoutDescriptor::new(
            "sparse buffer update bind group layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (

                    storage_buffer::<u32>(false),

                    storage_buffer_read_only::<u32>(false),

                    storage_buffer_read_only::<u32>(false),

                    uniform_buffer::<GpuSparseBufferUpdateMetadata>(false),
                ),
            ),
        );

        SparseBufferUpdatePipelines {
            bind_group_layout: Some(bind_group_layout),
            shader: Some(load_embedded_asset!(world, "sparse_buffer_update.wgsl")),
        }
    }
}

impl SpecializedComputePipeline for SparseBufferUpdatePipelines {
    type Key = ();

    fn specialize(&self, _: Self::Key) -> ComputePipelineDescriptor {
        ComputePipelineDescriptor {
            label: Some("sparse buffer update pipeline".into()),
            layout: self.bind_group_layout.clone().into_iter().collect(),
            shader: self.shader.clone().unwrap_or_default(),
            shader_defs: vec![],
            ..ComputePipelineDescriptor::default()
        }
    }
}

struct SparseBufferStagingBuffers {

    source_data: RawBufferVec<u32>,

    indices: RawBufferVec<u32>,

    element_word_size: u32,

    page_size_log2: u32,
}

impl SparseBufferStagingBuffers {

    fn page_size(&self) -> usize {
        1 << self.page_size_log2
    }

    fn new(label: &str, element_word_size: u32, page_size_log2: u32) -> SparseBufferStagingBuffers {
        let mut source_data_buffer =
            RawBufferVec::new(BufferUsages::COPY_DST | BufferUsages::STORAGE);
        source_data_buffer.set_label(Some(&*format!("{} staging buffer", label)));

        let mut indices_buffer = RawBufferVec::new(BufferUsages::COPY_DST | BufferUsages::STORAGE);
        indices_buffer.set_label(Some(&*format!("{} index buffer", label)));

        SparseBufferStagingBuffers {
            source_data: source_data_buffer,
            indices: indices_buffer,
            element_word_size,
            page_size_log2,
        }
    }

    fn updated_page_count(&self) -> u32 {

        let element_count = self.source_data.len() / self.element_word_size as usize;
        (element_count / self.page_size()) as u32
    }

    fn write_buffers(
        &mut self,
        metadata_uniform: &mut UniformBuffer<GpuSparseBufferUpdateMetadata>,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
    ) {
        metadata_uniform.get_mut().updated_page_count = self.updated_page_count();
        metadata_uniform.write_buffer(render_device, render_queue);

        self.source_data.write_buffer(render_device, render_queue);
        self.indices.write_buffer(render_device, render_queue);
    }

    fn should_perform_full_reupload(&self, changed_page_count: u32, buffer_length: usize) -> bool {

        let total_changed_word_count =
            changed_page_count * self.page_size() as u32 * self.element_word_size;
        if total_changed_word_count > MAX_WORKGROUPS * SPARSE_BUFFER_UPDATE_WORKGROUP_SIZE {
            return true;
        }

        let sparse_upload_fraction =
            changed_page_count as f64 / buffer_length.div_ceil(self.page_size()) as f64;
        sparse_upload_fraction > SPARSE_UPLOAD_THRESHOLD
    }
}

pub struct AtomicSparseBufferVec<T>
where
    T: AtomicPod,
{

    handle: SparseBufferHandle,

    values: Vec<T::Blob>,

    data_buffer: Option<Buffer>,

    staging_buffers: SparseBufferStagingBuffers,

    metadata_uniform: UniformBuffer<GpuSparseBufferUpdateMetadata>,

    capacity: usize,

    buffer_usages: BufferUsages,

    label: Arc<str>,

    dirty_pages: Vec<AtomicU64>,

    needs_full_reupload: bool,

    sparse_update_scheduled: bool,
}

impl<T> AtomicSparseBufferVec<T>
where
    T: AtomicPod,
{

    fn page_size(&self) -> u32 {
        1 << self.staging_buffers.page_size_log2
    }

    pub fn new(buffer_usages: BufferUsages, page_size_log2: u32, label: Arc<str>) -> Self {

        debug_assert_eq!(size_of::<T>() % 4, 0);
        let element_word_size = size_of::<T>() / 4;

        let id = Arc::new(SparseBufferId(
            NEXT_SPARSE_BUFFER_ID.fetch_add(1, Ordering::Relaxed),
        ));

        Self {
            handle: id,
            values: vec![],
            data_buffer: None,
            staging_buffers: SparseBufferStagingBuffers::new(
                &label,
                element_word_size as u32,
                page_size_log2,
            ),
            metadata_uniform: UniformBuffer::from(GpuSparseBufferUpdateMetadata::new::<T>(
                page_size_log2,
            )),
            capacity: 0,
            buffer_usages: buffer_usages | BufferUsages::COPY_DST,
            label,
            dirty_pages: vec![],
            needs_full_reupload: false,
            sparse_update_scheduled: false,
        }
    }

    pub fn len(&self) -> u32 {
        self.values.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn buffer(&self) -> Option<&Buffer> {
        self.data_buffer.as_ref()
    }

    pub fn clear(&mut self) {
        self.truncate(0);
    }

    pub fn get(&self, index: u32) -> T {
        T::read_from_blob(&self.values[index as usize])
    }

    pub fn set(&self, index: u32, value: T) {
        value.write_to_blob(&self.values[index as usize]);
        self.note_changed_index(index);
    }

    pub fn push(&mut self, value: T) -> u32 {
        let index = self.values.len() as u32;
        self.values.push(T::Blob::default());
        value.write_to_blob(&self.values[index as usize]);

        let page_word = (self.index_to_page(index) / PAGES_PER_DIRTY_WORD) as usize;
        while self.dirty_pages.len() < page_word + 1 {
            self.dirty_pages.push(AtomicU64::default());
        }
        self.note_changed_index(index);

        index
    }

    fn note_changed_index(&self, index: u32) {
        let page = self.index_to_page(index);
        let (page_word, page_in_word) = (page / PAGES_PER_DIRTY_WORD, page % PAGES_PER_DIRTY_WORD);
        self.dirty_pages[page_word as usize].fetch_or(1 << page_in_word, Ordering::Relaxed);
    }

    fn index_to_page(&self, index: u32) -> u32 {
        index / self.page_size()
    }

    pub fn reserve(&mut self, new_capacity: usize, render_device: &RenderDevice) {
        reserve(
            new_capacity,
            &mut self.capacity,
            &self.label,
            &mut self.data_buffer,
            self.buffer_usages,
            &mut self.needs_full_reupload,
            size_of::<T::Blob>(),
            render_device,
        );
    }

    pub fn grow(&mut self, new_len: u32) {
        let old_len = self.values.len() as u32;
        if old_len >= new_len {
            return;
        }

        self.values.reserve(new_len as usize - old_len as usize);
        self.values.resize_with(new_len as usize, T::Blob::default);

        let old_final_page = self.index_to_page(old_len);
        let old_final_page_word_index = old_final_page / PAGES_PER_DIRTY_WORD;
        let old_final_page_in_word = old_final_page % PAGES_PER_DIRTY_WORD;

        if old_final_page_in_word != 0
            && let Some(ref mut old_final_atomic_page_word) =
                self.dirty_pages.get_mut(old_final_page_word_index as usize)
        {
            *old_final_atomic_page_word.get_mut() |= !((1u64 << old_final_page_in_word) - 1);
        }

        let new_page_count = self.index_to_page(new_len);
        self.dirty_pages.resize_with(
            (new_page_count as usize).div_ceil(PAGES_PER_DIRTY_WORD as usize),
            || AtomicU64::new(u64::MAX),
        );
    }

    pub fn truncate(&mut self, len: u32) {
        self.values.truncate(len as usize);

        let page = self.index_to_page(len);
        self.dirty_pages
            .truncate(page.div_ceil(PAGES_PER_DIRTY_WORD) as usize);
    }

    pub fn write_buffers(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {
        if self.values.is_empty() {
            return;
        }

        let good_size = calculate_allocation_size(self.values.len());
        self.reserve(good_size, render_device);

        if self.should_perform_full_reupload(render_device) {
            self.write_entire_buffer(render_queue);
        } else {
            self.prepare_sparse_upload(render_device, render_queue);
        }
    }

    fn should_perform_full_reupload(&self, render_device: &RenderDevice) -> bool {
        if self.needs_full_reupload {
            return true;
        }

        if render_device.limits().max_storage_buffers_per_shader_stage < 3 {
            return true;
        }

        let changed_page_count: u32 = self
            .dirty_pages
            .iter()
            .map(|atomic_page_word| atomic_page_word.load(Ordering::Relaxed).count_ones())
            .sum();

        self.staging_buffers
            .should_perform_full_reupload(changed_page_count, self.values.len())
    }

    fn write_entire_buffer(&mut self, render_queue: &RenderQueue) {
        let Some(ref mut data_buffer) = self.data_buffer else {
            error!("Dirty sparse buffer should have created a data buffer by now");
            return;
        };

        unsafe {
            render_queue.write_buffer(
                data_buffer,
                0,
                slice::from_raw_parts(
                    self.values.as_ptr().cast::<u8>(),
                    self.values.len() * size_of::<T::Blob>(),
                ),
            );
        }

        for atomic_page_word in self.dirty_pages.iter() {
            atomic_page_word.store(0, Ordering::Relaxed);
        }
        self.sparse_update_scheduled = false;
    }

    fn prepare_sparse_upload(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {

        for (page_word_index, atomic_page_word) in self.dirty_pages.iter().enumerate() {
            let page_word = atomic_page_word.load(Ordering::Relaxed);
            for page_index_in_word in BitIter::new(page_word) {
                let page = page_word_index as u32 * PAGES_PER_DIRTY_WORD + page_index_in_word;

                self.staging_buffers.indices.push(page);

                let page_size = self.staging_buffers.page_size();
                let page_start = page as usize * page_size;
                let page_end = page_start + page_size;
                for value_index in page_start..page_end {
                    match self.values.get(value_index) {
                        Some(blob) => {
                            let value = T::read_from_blob(blob);
                            self.staging_buffers
                                .source_data
                                .extend(bytemuck::cast_slice(&[value]).iter().copied());
                        }
                        None => {
                            self.staging_buffers.source_data.extend(iter::repeat_n(
                                0,
                                self.staging_buffers.element_word_size as usize,
                            ));
                        }
                    }
                }

                debug_assert_eq!(
                    self.staging_buffers.source_data.len()
                        % (self.staging_buffers.element_word_size as usize
                            * self.staging_buffers.page_size()),
                    0
                );
            }

            atomic_page_word.store(0, Ordering::Relaxed);
        }

        self.sparse_update_scheduled = !self.staging_buffers.source_data.is_empty();
        if self.sparse_update_scheduled {
            self.staging_buffers.write_buffers(
                &mut self.metadata_uniform,
                render_device,
                render_queue,
            );
        }
    }

    pub fn prepare_to_populate_buffers(
        &mut self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        sparse_buffer_update_jobs: &mut SparseBufferUpdateJobs,
        sparse_buffer_update_bind_groups: &mut SparseBufferUpdateBindGroups,
        sparse_buffer_update_pipelines: &SparseBufferUpdatePipelines,
    ) {
        if self.sparse_update_scheduled {
            match (&self.data_buffer, self.metadata_uniform.buffer()) {
                (Some(data_buffer), Some(metadata_buffer)) => {
                    prepare_to_populate_buffers(
                        self.handle.clone(),
                        &self.label,
                        data_buffer,
                        &mut self.staging_buffers,
                        metadata_buffer,
                        render_device,
                        pipeline_cache,
                        sparse_buffer_update_jobs,
                        sparse_buffer_update_bind_groups,
                        sparse_buffer_update_pipelines,
                    );
                }
                _ => {
                    error!("Buffers should have been created by now");
                }
            }
        }

        self.staging_buffers.source_data.clear();
        self.staging_buffers.indices.clear();

        self.needs_full_reupload = false;
        self.sparse_update_scheduled = false;
    }
}

impl FromWorld for SparseBufferUpdateBindGroups {
    fn from_world(world: &mut World) -> Self {
        world.resource_scope::<SpecializedComputePipelines<SparseBufferUpdatePipelines>, _>(
            |world, mut specialized_sparse_buffer_update_pipelines| {
                let pipeline_cache = world.resource::<PipelineCache>();
                let sparse_buffer_update_pipelines =
                    world.resource::<SparseBufferUpdatePipelines>();
                let pipeline_id = specialized_sparse_buffer_update_pipelines.specialize(
                    pipeline_cache,
                    sparse_buffer_update_pipelines,
                    (),
                );

                SparseBufferUpdateBindGroups {
                    bind_groups: WeakKeyHashMap::default(),
                    pipeline_id,
                }
            },
        )
    }
}

fn prepare_to_populate_buffers(
    sparse_buffer_handle: SparseBufferHandle,
    label: &Arc<str>,
    data_buffer: &Buffer,
    staging_buffers: &mut SparseBufferStagingBuffers,
    metadata_buffer: &Buffer,
    render_device: &RenderDevice,
    pipeline_cache: &PipelineCache,
    sparse_buffer_update_jobs: &mut SparseBufferUpdateJobs,
    sparse_buffer_update_bind_groups: &mut SparseBufferUpdateBindGroups,
    sparse_buffer_update_pipelines: &SparseBufferUpdatePipelines,
) {
    let (Some(source_data_staging_buffer), Some(indices_staging_buffer)) = (
        staging_buffers.source_data.buffer(),
        staging_buffers.indices.buffer(),
    ) else {
        error!("Staging buffers should have been created by now");
        return;
    };

    let Some(bind_group_layout) = &sparse_buffer_update_pipelines.bind_group_layout else {
        return;
    };

    sparse_buffer_update_jobs.push(SparseBufferUpdateJob {
        sparse_buffer_handle: sparse_buffer_handle.clone(),
        page_size_log2: staging_buffers.page_size_log2,
        updated_page_count: staging_buffers.updated_page_count(),
        element_word_size: staging_buffers.element_word_size,
        label: (*label).clone(),
    });

    let bind_group = render_device.create_bind_group(
        Some(&*format!("{} bind group", label)),
        &pipeline_cache.get_bind_group_layout(bind_group_layout),
        &BindGroupEntries::sequential((

            data_buffer.as_entire_binding(),

            source_data_staging_buffer.as_entire_binding(),

            indices_staging_buffer.as_entire_binding(),

            metadata_buffer.as_entire_binding(),
        )),
    );
    sparse_buffer_update_bind_groups.bind_groups.insert(
        sparse_buffer_handle,
        SparseBufferUpdateBindGroup { bind_group },
    );
}

fn reserve(
    new_capacity: usize,
    capacity: &mut usize,
    label: &str,
    data_buffer: &mut Option<Buffer>,
    buffer_usages: BufferUsages,
    needs_full_reupload: &mut bool,
    element_size: usize,
    render_device: &RenderDevice,
) {

    if new_capacity == 0 || new_capacity <= *capacity {
        return;
    }

    *capacity = new_capacity;
    *data_buffer = Some(render_device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size: element_size as u64 * new_capacity as u64,
        usage: buffer_usages,
        mapped_at_creation: false,
    }));

    *needs_full_reupload = true;
}

impl GpuSparseBufferUpdateMetadata {

    fn new<T>(page_size_log2: u32) -> GpuSparseBufferUpdateMetadata {
        assert_eq!(size_of::<T>() % 4, 0);
        GpuSparseBufferUpdateMetadata {
            element_size: (size_of::<T>() / 4) as u32,
            updated_page_count: 0,
            page_size_log2,
        }
    }
}

struct BitIter(u64);

impl BitIter {
    fn new(bits: u64) -> BitIter {
        BitIter(bits)
    }
}

impl Iterator for BitIter {
    type Item = u32;

    fn next(&mut self) -> Option<Self::Item> {
        let trailing_zeros = self.0.trailing_zeros();
        if trailing_zeros == 64 {
            return None;
        }
        self.0 &= !(1 << trailing_zeros);
        Some(trailing_zeros)
    }
}

fn calculate_allocation_size(length: usize) -> usize {
    let exponent = (length as f64).log(REALLOCATION_FACTOR).ceil();
    let size = REALLOCATION_FACTOR.powf(exponent) as usize;
    size.next_multiple_of(REALLOCATION_SIZE_MULTIPLE)
}
