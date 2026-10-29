

use crate::Material;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    resource::Resource,
    system::{Commands, Res},
};
use bevy_platform::collections::{HashMap, HashSet};
use bevy_reflect::{prelude::ReflectDefault, Reflect};
use bevy_render::render_resource::{BindlessSlabResourceLimit, PipelineCache};
use bevy_render::{
    render_resource::{
        BindGroup, BindGroupEntry, BindGroupLayoutDescriptor, BindingNumber, BindingResource,
        BindingResources, BindlessDescriptor, BindlessIndex, BindlessIndexTableDescriptor,
        BindlessResourceType, Buffer, BufferBinding, BufferDescriptor, BufferId,
        BufferInitDescriptor, BufferUsages, CompareFunction, FilterMode, MipmapFilterMode,
        OwnedBindingResource, PreparedBindGroup, RawBufferVec, Sampler, SamplerDescriptor,
        SamplerId, TextureView, TextureViewDimension, TextureViewId, UnpreparedBindGroup,
        WgpuSampler, WgpuTextureView,
    },
    renderer::{RenderDevice, RenderQueue},
    settings::WgpuFeatures,
    texture::FallbackImage,
};
use bevy_utils::{default, TypeIdMap};
use bytemuck::{Pod, Zeroable};
use core::hash::Hash;
use core::{cmp::Ordering, iter, mem, ops::Range};
use tracing::{error, trace};

#[derive(Resource, Deref, DerefMut, Default)]
pub struct MaterialBindGroupAllocators(TypeIdMap<MaterialBindGroupAllocator>);

pub enum MaterialBindGroupAllocator {

    Bindless(Box<MaterialBindGroupBindlessAllocator>),

    NonBindless(Box<MaterialBindGroupNonBindlessAllocator>),
}

pub struct MaterialBindGroupBindlessAllocator {

    label: &'static str,

    slabs: Vec<MaterialBindlessSlab>,

    bind_group_layout: BindGroupLayoutDescriptor,

    bindless_descriptor: BindlessDescriptor,

    fallback_buffers: HashMap<BindlessIndex, Buffer>,

    slab_capacity: u32,
}

pub struct MaterialBindlessSlab {

    bind_group: Option<BindGroup>,

    bindless_index_tables: Vec<MaterialBindlessIndexTable>,

    samplers: HashMap<BindlessResourceType, MaterialBindlessBindingArray<Sampler>>,

    textures: HashMap<BindlessResourceType, MaterialBindlessBindingArray<TextureView>>,

    buffers: HashMap<BindlessIndex, MaterialBindlessBindingArray<Buffer>>,

    data_buffers: HashMap<BindlessIndex, MaterialDataBuffer>,

    free_slots: Vec<MaterialBindGroupSlot>,

    live_allocation_count: u32,

    allocated_resource_count: u32,
}

struct MaterialBindlessIndexTable {

    buffer: RetainedRawBufferVec<u32>,

    index_range: Range<BindlessIndex>,

    binding_number: BindingNumber,
}

struct MaterialBindlessBindingArray<R>
where
    R: GetBindingResourceId,
{

    binding_number: BindingNumber,

    bindings: Vec<Option<MaterialBindlessBinding<R>>>,

    resource_type: BindlessResourceType,

    resource_to_slot: HashMap<BindingResourceId, u32>,

    free_slots: Vec<u32>,

    len: u32,
}

struct MaterialBindlessBinding<R>
where
    R: GetBindingResourceId,
{

    resource: R,

    ref_count: u32,
}

pub struct MaterialBindGroupNonBindlessAllocator {

    label: &'static str,

    bind_groups: Vec<Option<MaterialNonBindlessAllocatedBindGroup>>,

    to_prepare: HashSet<MaterialBindGroupIndex>,

    free_indices: Vec<MaterialBindGroupIndex>,
}

enum MaterialNonBindlessAllocatedBindGroup {

    Unprepared {

        bind_group: UnpreparedBindGroup,

        layout: BindGroupLayoutDescriptor,
    },

    Prepared {
        bind_group: PreparedBindGroup,
        #[expect(dead_code, reason = "These buffers are only referenced by bind groups")]
        uniform_buffers: Vec<Buffer>,
    },
}

#[derive(Resource)]
pub struct FallbackBindlessResources {

    filtering_sampler: Sampler,

    non_filtering_sampler: Sampler,

    comparison_sampler: Sampler,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum BindingResourceId {

    Buffer(BufferId),

    TextureView(TextureViewDimension, TextureViewId),

    Sampler(SamplerId),

    DataBuffer,
}

enum BindingResourceArray<'a> {

    Buffers(Vec<BufferBinding<'a>>),

    TextureViews(Vec<&'a WgpuTextureView>),

    Samplers(Vec<&'a WgpuSampler>),
}

#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, Reflect)]
#[reflect(Clone, Default)]
#[repr(C)]
pub struct MaterialBindingId {

    pub group: MaterialBindGroupIndex,

    pub slot: MaterialBindGroupSlot,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable, Reflect, Deref, DerefMut,
)]
#[reflect(Default, Clone, PartialEq, Hash)]
#[repr(C)]
pub struct MaterialBindGroupIndex(pub u32);

impl From<u32> for MaterialBindGroupIndex {
    fn from(value: u32) -> Self {
        MaterialBindGroupIndex(value)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable, Reflect, Deref, DerefMut)]
#[reflect(Default, Clone, PartialEq)]
#[repr(C)]
pub struct MaterialBindGroupSlot(pub u32);

enum BufferDirtyState {

    Clean,

    NeedsReserve,

    NeedsUpload,
}

struct BindlessAllocationCandidate {

    pre_existing_resources: HashMap<BindlessIndex, u32>,

    needed_free_slots: u32,
}

trait GetBindingResourceId {

    fn binding_resource_id(&self, resource_type: BindlessResourceType) -> BindingResourceId;
}

pub struct MaterialSlab<'a>(MaterialSlabImpl<'a>);

enum MaterialSlabImpl<'a> {

    Bindless(&'a MaterialBindlessSlab),

    NonBindless(MaterialNonBindlessSlab<'a>),
}

enum MaterialNonBindlessSlab<'a> {

    Prepared(&'a PreparedBindGroup),

    Unprepared,
}

struct MaterialDataBuffer {

    binding_number: BindingNumber,

    buffer: RetainedRawBufferVec<u8>,

    aligned_element_size: u32,

    free_slots: Vec<u32>,

    len: u32,
}

#[derive(Deref, DerefMut)]
struct RetainedRawBufferVec<T>
where
    T: Pod,
{

    #[deref]
    buffer: RawBufferVec<T>,

    dirty: BufferDirtyState,
}

const DEFAULT_BINDLESS_FALLBACK_BUFFER_SIZE: u64 = 16;

impl From<u32> for MaterialBindGroupSlot {
    fn from(value: u32) -> Self {
        MaterialBindGroupSlot(value)
    }
}

impl From<MaterialBindGroupSlot> for u32 {
    fn from(value: MaterialBindGroupSlot) -> Self {
        value.0
    }
}

impl<'a> From<&'a OwnedBindingResource> for BindingResourceId {
    fn from(value: &'a OwnedBindingResource) -> Self {
        match *value {
            OwnedBindingResource::Buffer(ref buffer) => BindingResourceId::Buffer(buffer.id()),
            OwnedBindingResource::Data(_) => BindingResourceId::DataBuffer,
            OwnedBindingResource::TextureView(ref texture_view_dimension, ref texture_view) => {
                BindingResourceId::TextureView(*texture_view_dimension, texture_view.id())
            }
            OwnedBindingResource::Sampler(_, ref sampler) => {
                BindingResourceId::Sampler(sampler.id())
            }
        }
    }
}

impl GetBindingResourceId for Buffer {
    fn binding_resource_id(&self, _: BindlessResourceType) -> BindingResourceId {
        BindingResourceId::Buffer(self.id())
    }
}

impl GetBindingResourceId for Sampler {
    fn binding_resource_id(&self, _: BindlessResourceType) -> BindingResourceId {
        BindingResourceId::Sampler(self.id())
    }
}

impl GetBindingResourceId for TextureView {
    fn binding_resource_id(&self, resource_type: BindlessResourceType) -> BindingResourceId {
        let texture_view_dimension = match resource_type {
            BindlessResourceType::Texture1d => TextureViewDimension::D1,
            BindlessResourceType::Texture2d => TextureViewDimension::D2,
            BindlessResourceType::Texture2dArray => TextureViewDimension::D2Array,
            BindlessResourceType::Texture3d => TextureViewDimension::D3,
            BindlessResourceType::TextureCube => TextureViewDimension::Cube,
            BindlessResourceType::TextureCubeArray => TextureViewDimension::CubeArray,
            _ => panic!("Resource type is not a texture"),
        };
        BindingResourceId::TextureView(texture_view_dimension, self.id())
    }
}

impl MaterialBindGroupAllocator {

    pub fn new(
        render_device: &RenderDevice,
        label: &'static str,
        bindless_descriptor: Option<BindlessDescriptor>,
        bind_group_layout: BindGroupLayoutDescriptor,
        slab_capacity: Option<BindlessSlabResourceLimit>,
    ) -> MaterialBindGroupAllocator {
        if let Some(bindless_descriptor) = bindless_descriptor {
            MaterialBindGroupAllocator::Bindless(Box::new(MaterialBindGroupBindlessAllocator::new(
                render_device,
                label,
                bindless_descriptor,
                bind_group_layout,
                slab_capacity,
            )))
        } else {
            MaterialBindGroupAllocator::NonBindless(Box::new(
                MaterialBindGroupNonBindlessAllocator::new(label),
            ))
        }
    }

    pub fn get(&self, group: MaterialBindGroupIndex) -> Option<MaterialSlab<'_>> {
        match *self {
            MaterialBindGroupAllocator::Bindless(ref bindless_allocator) => bindless_allocator
                .get(group)
                .map(|bindless_slab| MaterialSlab(MaterialSlabImpl::Bindless(bindless_slab))),
            MaterialBindGroupAllocator::NonBindless(ref non_bindless_allocator) => {
                non_bindless_allocator.get(group).map(|non_bindless_slab| {
                    MaterialSlab(MaterialSlabImpl::NonBindless(non_bindless_slab))
                })
            }
        }
    }

    pub fn allocate_unprepared(
        &mut self,
        unprepared_bind_group: UnpreparedBindGroup,
        bind_group_layout: &BindGroupLayoutDescriptor,
    ) -> MaterialBindingId {
        match *self {
            MaterialBindGroupAllocator::Bindless(
                ref mut material_bind_group_bindless_allocator,
            ) => material_bind_group_bindless_allocator.allocate_unprepared(unprepared_bind_group),
            MaterialBindGroupAllocator::NonBindless(
                ref mut material_bind_group_non_bindless_allocator,
            ) => material_bind_group_non_bindless_allocator
                .allocate_unprepared(unprepared_bind_group, (*bind_group_layout).clone()),
        }
    }

    pub fn allocate_prepared(
        &mut self,
        prepared_bind_group: PreparedBindGroup,
    ) -> MaterialBindingId {
        match *self {
            MaterialBindGroupAllocator::Bindless(_) => {
                panic!(
                    "Bindless resources are incompatible with implementing `as_bind_group` \
                     directly; implement `unprepared_bind_group` instead or disable bindless"
                )
            }
            MaterialBindGroupAllocator::NonBindless(ref mut non_bindless_allocator) => {
                non_bindless_allocator.allocate_prepared(prepared_bind_group)
            }
        }
    }

    pub fn free(&mut self, material_binding_id: MaterialBindingId) {
        match *self {
            MaterialBindGroupAllocator::Bindless(
                ref mut material_bind_group_bindless_allocator,
            ) => material_bind_group_bindless_allocator.free(material_binding_id),
            MaterialBindGroupAllocator::NonBindless(
                ref mut material_bind_group_non_bindless_allocator,
            ) => material_bind_group_non_bindless_allocator.free(material_binding_id),
        }
    }

    pub fn prepare_bind_groups(
        &mut self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        fallback_bindless_resources: &FallbackBindlessResources,
        fallback_image: &FallbackImage,
    ) {
        match *self {
            MaterialBindGroupAllocator::Bindless(
                ref mut material_bind_group_bindless_allocator,
            ) => material_bind_group_bindless_allocator.prepare_bind_groups(
                render_device,
                pipeline_cache,
                fallback_bindless_resources,
                fallback_image,
            ),
            MaterialBindGroupAllocator::NonBindless(
                ref mut material_bind_group_non_bindless_allocator,
            ) => material_bind_group_non_bindless_allocator
                .prepare_bind_groups(render_device, pipeline_cache),
        }
    }

    pub fn write_buffers(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {
        match *self {
            MaterialBindGroupAllocator::Bindless(
                ref mut material_bind_group_bindless_allocator,
            ) => material_bind_group_bindless_allocator.write_buffers(render_device, render_queue),
            MaterialBindGroupAllocator::NonBindless(_) => {

            }
        }
    }

    pub fn slab_count(&self) -> usize {
        match self {
            Self::Bindless(bless) => bless.slabs.len(),
            Self::NonBindless(_) => 0,
        }
    }

    pub fn slabs_size(&self) -> usize {
        match self {
            Self::Bindless(bless) => bless
                .slabs
                .iter()
                .flat_map(|slab| {
                    slab.data_buffers
                        .iter()
                        .map(|(_, buffer)| buffer.buffer.len())
                })
                .sum(),
            Self::NonBindless(_) => 0,
        }
    }

    pub fn allocations(&self) -> u64 {
        match self {
            Self::Bindless(bless) => bless
                .slabs
                .iter()
                .map(|slab| u64::from(slab.allocated_resource_count))
                .sum(),
            Self::NonBindless(_) => 0,
        }
    }
}

impl MaterialBindlessIndexTable {

    fn new(
        bindless_index_table_descriptor: &BindlessIndexTableDescriptor,
    ) -> MaterialBindlessIndexTable {

        let mut buffer = RetainedRawBufferVec::new(BufferUsages::STORAGE);
        for _ in *bindless_index_table_descriptor.indices.start
            ..*bindless_index_table_descriptor.indices.end
        {
            buffer.push(0);
        }

        MaterialBindlessIndexTable {
            buffer,
            index_range: bindless_index_table_descriptor.indices.clone(),
            binding_number: bindless_index_table_descriptor.binding_number,
        }
    }

    fn get(&self, slot: MaterialBindGroupSlot) -> &[u32] {
        let struct_size = *self.index_range.end as usize - *self.index_range.start as usize;
        let start = struct_size * slot.0 as usize;
        &self.buffer.values()[start..(start + struct_size)]
    }

    fn get_binding(
        &self,
        slot: MaterialBindGroupSlot,
        bindless_index: BindlessIndex,
    ) -> Option<u32> {
        if bindless_index < self.index_range.start || bindless_index >= self.index_range.end {
            return None;
        }
        self.get(slot)
            .get((*bindless_index - *self.index_range.start) as usize)
            .copied()
    }

    fn table_length(&self) -> u32 {
        self.index_range.end.0 - self.index_range.start.0
    }

    fn set(
        &mut self,
        slot: MaterialBindGroupSlot,
        allocated_resource_slots: &HashMap<BindlessIndex, u32>,
    ) {
        let table_len = self.table_length() as usize;
        let range = (slot.0 as usize * table_len)..((slot.0 as usize + 1) * table_len);
        while self.buffer.len() < range.end {
            self.buffer.push(0);
        }

        for (&bindless_index, &resource_slot) in allocated_resource_slots {
            if self.index_range.contains(&bindless_index) {
                self.buffer.set(
                    *bindless_index + range.start as u32 - *self.index_range.start,
                    resource_slot,
                );
            }
        }

        self.buffer.dirty = BufferDirtyState::NeedsReserve;
    }

    fn bind_group_entry(&self) -> BindGroupEntry<'_> {
        BindGroupEntry {
            binding: *self.binding_number,
            resource: self
                .buffer
                .buffer()
                .expect("Bindings buffer must exist")
                .as_entire_binding(),
        }
    }
}

impl<T> RetainedRawBufferVec<T>
where
    T: Pod,
{

    fn new(buffer_usages: BufferUsages) -> RetainedRawBufferVec<T> {
        RetainedRawBufferVec {
            buffer: RawBufferVec::new(buffer_usages),
            dirty: BufferDirtyState::NeedsUpload,
        }
    }

    fn prepare(&mut self, render_device: &RenderDevice) {
        match self.dirty {
            BufferDirtyState::Clean | BufferDirtyState::NeedsUpload => {}
            BufferDirtyState::NeedsReserve => {
                let capacity = self.buffer.len();
                self.buffer.reserve(capacity, render_device);
                self.dirty = BufferDirtyState::NeedsUpload;
            }
        }
    }

    fn write(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {
        match self.dirty {
            BufferDirtyState::Clean => {}
            BufferDirtyState::NeedsReserve | BufferDirtyState::NeedsUpload => {
                self.buffer.write_buffer(render_device, render_queue);
                self.dirty = BufferDirtyState::Clean;
            }
        }
    }
}

impl MaterialBindGroupBindlessAllocator {

    fn new(
        render_device: &RenderDevice,
        label: &'static str,
        bindless_descriptor: BindlessDescriptor,
        bind_group_layout: BindGroupLayoutDescriptor,
        slab_capacity: Option<BindlessSlabResourceLimit>,
    ) -> MaterialBindGroupBindlessAllocator {
        let fallback_buffers = bindless_descriptor
            .buffers
            .iter()
            .map(|bindless_buffer_descriptor| {
                (
                    bindless_buffer_descriptor.bindless_index,
                    render_device.create_buffer(&BufferDescriptor {
                        label: Some("bindless fallback buffer"),
                        size: match bindless_buffer_descriptor.size {
                            Some(size) => size as u64,
                            None => DEFAULT_BINDLESS_FALLBACK_BUFFER_SIZE,
                        },
                        usage: BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    }),
                )
            })
            .collect();

        MaterialBindGroupBindlessAllocator {
            label,
            slabs: vec![],
            bind_group_layout,
            bindless_descriptor,
            fallback_buffers,
            slab_capacity: slab_capacity
                .expect("Non-bindless materials should use the non-bindless allocator")
                .resolve(),
        }
    }

    fn allocate_unprepared(
        &mut self,
        mut unprepared_bind_group: UnpreparedBindGroup,
    ) -> MaterialBindingId {
        for (slab_index, slab) in self.slabs.iter_mut().enumerate() {
            trace!("Trying to allocate in slab {}", slab_index);
            match slab.try_allocate(unprepared_bind_group, self.slab_capacity) {
                Ok(slot) => {
                    return MaterialBindingId {
                        group: MaterialBindGroupIndex(slab_index as u32),
                        slot,
                    };
                }
                Err(bind_group) => unprepared_bind_group = bind_group,
            }
        }

        let group = MaterialBindGroupIndex(self.slabs.len() as u32);
        self.slabs
            .push(MaterialBindlessSlab::new(&self.bindless_descriptor));

        let Ok(slot) = self
            .slabs
            .last_mut()
            .expect("We just pushed a slab")
            .try_allocate(unprepared_bind_group, self.slab_capacity)
        else {
            panic!("An allocation into an empty slab should always succeed")
        };

        MaterialBindingId { group, slot }
    }

    fn free(&mut self, material_binding_id: MaterialBindingId) {
        self.slabs
            .get_mut(material_binding_id.group.0 as usize)
            .expect("Slab should exist")
            .free(material_binding_id.slot, &self.bindless_descriptor);
    }

    fn get(&self, group: MaterialBindGroupIndex) -> Option<&MaterialBindlessSlab> {
        self.slabs.get(group.0 as usize)
    }

    fn prepare_bind_groups(
        &mut self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        fallback_bindless_resources: &FallbackBindlessResources,
        fallback_image: &FallbackImage,
    ) {
        for slab in &mut self.slabs {
            slab.prepare(
                render_device,
                pipeline_cache,
                self.label,
                &self.bind_group_layout,
                fallback_bindless_resources,
                &self.fallback_buffers,
                fallback_image,
                &self.bindless_descriptor,
                self.slab_capacity,
            );
        }
    }

    fn write_buffers(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {
        for slab in &mut self.slabs {
            slab.write_buffer(render_device, render_queue);
        }
    }
}

impl MaterialBindlessSlab {

    fn try_allocate(
        &mut self,
        unprepared_bind_group: UnpreparedBindGroup,
        slot_capacity: u32,
    ) -> Result<MaterialBindGroupSlot, UnpreparedBindGroup> {

        let Some(allocation_candidate) = self.check_allocation(&unprepared_bind_group) else {
            return Err(unprepared_bind_group);
        };

        if self.allocated_resource_count > 0
            && self.allocated_resource_count + allocation_candidate.needed_free_slots
                > slot_capacity
        {
            trace!("Slab is full, can't allocate");
            return Err(unprepared_bind_group);
        }

        let slot = match self.free_slots.pop() {
            Some(slot) => slot,
            None => {

                if self.live_allocation_count > 0xFFFF {
                    trace!("Slab material bind group slot would overflow, can't allocate");
                    return Err(unprepared_bind_group);
                }
                MaterialBindGroupSlot(self.live_allocation_count)
            }
        };

        self.live_allocation_count += 1;

        let allocated_resource_slots =
            self.insert_resources(unprepared_bind_group.bindings, allocation_candidate);

        for bindless_index_table in &mut self.bindless_index_tables {
            bindless_index_table.set(slot, &allocated_resource_slots);
        }

        self.bind_group = None;

        Ok(slot)
    }

    fn check_allocation(
        &self,
        unprepared_bind_group: &UnpreparedBindGroup,
    ) -> Option<BindlessAllocationCandidate> {
        let mut allocation_candidate = BindlessAllocationCandidate {
            pre_existing_resources: HashMap::default(),
            needed_free_slots: 0,
        };

        for &(bindless_index, ref owned_binding_resource) in unprepared_bind_group.bindings.iter() {
            let bindless_index = BindlessIndex(bindless_index);
            match *owned_binding_resource {
                OwnedBindingResource::Buffer(ref buffer) => {
                    let Some(binding_array) = self.buffers.get(&bindless_index) else {
                        error!(
                            "Binding array wasn't present for buffer at index {:?}",
                            bindless_index
                        );
                        return None;
                    };
                    match binding_array.find(BindingResourceId::Buffer(buffer.id())) {
                        Some(slot) => {
                            allocation_candidate
                                .pre_existing_resources
                                .insert(bindless_index, slot);
                        }
                        None => allocation_candidate.needed_free_slots += 1,
                    }
                }

                OwnedBindingResource::Data(_) => {

                }

                OwnedBindingResource::TextureView(texture_view_dimension, ref texture_view) => {
                    let bindless_resource_type = BindlessResourceType::from(texture_view_dimension);
                    match self
                        .textures
                        .get(&bindless_resource_type)
                        .expect("Missing binding array for texture")
                        .find(BindingResourceId::TextureView(
                            texture_view_dimension,
                            texture_view.id(),
                        )) {
                        Some(slot) => {
                            allocation_candidate
                                .pre_existing_resources
                                .insert(bindless_index, slot);
                        }
                        None => {
                            allocation_candidate.needed_free_slots += 1;
                        }
                    }
                }

                OwnedBindingResource::Sampler(sampler_binding_type, ref sampler) => {
                    let bindless_resource_type = BindlessResourceType::from(sampler_binding_type);
                    match self
                        .samplers
                        .get(&bindless_resource_type)
                        .expect("Missing binding array for sampler")
                        .find(BindingResourceId::Sampler(sampler.id()))
                    {
                        Some(slot) => {
                            allocation_candidate
                                .pre_existing_resources
                                .insert(bindless_index, slot);
                        }
                        None => {
                            allocation_candidate.needed_free_slots += 1;
                        }
                    }
                }
            }
        }

        Some(allocation_candidate)
    }

    fn insert_resources(
        &mut self,
        mut binding_resources: BindingResources,
        allocation_candidate: BindlessAllocationCandidate,
    ) -> HashMap<BindlessIndex, u32> {
        let mut allocated_resource_slots = HashMap::default();

        for (bindless_index, owned_binding_resource) in binding_resources.drain(..) {
            let bindless_index = BindlessIndex(bindless_index);

            let pre_existing_slot = allocation_candidate
                .pre_existing_resources
                .get(&bindless_index);

            let binding_resource_id = BindingResourceId::from(&owned_binding_resource);
            let increment_allocated_resource_count = match owned_binding_resource {
                OwnedBindingResource::Buffer(buffer) => {
                    let slot = self
                        .buffers
                        .get_mut(&bindless_index)
                        .expect("Buffer binding array should exist")
                        .insert(binding_resource_id, buffer);
                    allocated_resource_slots.insert(bindless_index, slot);

                    if let Some(pre_existing_slot) = pre_existing_slot {
                        assert_eq!(*pre_existing_slot, slot);

                        false
                    } else {
                        true
                    }
                }
                OwnedBindingResource::Data(data) => {
                    if pre_existing_slot.is_some() {
                        panic!("Data buffers can't be deduplicated")
                    }

                    let slot = self
                        .data_buffers
                        .get_mut(&bindless_index)
                        .expect("Data buffer binding array should exist")
                        .insert(&data);
                    allocated_resource_slots.insert(bindless_index, slot);
                    false
                }
                OwnedBindingResource::TextureView(texture_view_dimension, texture_view) => {
                    let bindless_resource_type = BindlessResourceType::from(texture_view_dimension);
                    let slot = self
                        .textures
                        .get_mut(&bindless_resource_type)
                        .expect("Texture array should exist")
                        .insert(binding_resource_id, texture_view);
                    allocated_resource_slots.insert(bindless_index, slot);

                    if let Some(pre_existing_slot) = pre_existing_slot {
                        assert_eq!(*pre_existing_slot, slot);

                        false
                    } else {
                        true
                    }
                }
                OwnedBindingResource::Sampler(sampler_binding_type, sampler) => {
                    let bindless_resource_type = BindlessResourceType::from(sampler_binding_type);
                    let slot = self
                        .samplers
                        .get_mut(&bindless_resource_type)
                        .expect("Sampler should exist")
                        .insert(binding_resource_id, sampler);
                    allocated_resource_slots.insert(bindless_index, slot);

                    if let Some(pre_existing_slot) = pre_existing_slot {
                        assert_eq!(*pre_existing_slot, slot);

                        false
                    } else {
                        true
                    }
                }
            };

            if increment_allocated_resource_count {
                self.allocated_resource_count += 1;
            }
        }

        allocated_resource_slots
    }

    fn free(&mut self, slot: MaterialBindGroupSlot, bindless_descriptor: &BindlessDescriptor) {

        for (bindless_index, bindless_resource_type) in
            bindless_descriptor.resources.iter().enumerate()
        {
            let bindless_index = BindlessIndex::from(bindless_index as u32);
            let Some(bindless_index_table) = self.get_bindless_index_table(bindless_index) else {
                continue;
            };
            let Some(bindless_binding) = bindless_index_table.get_binding(slot, bindless_index)
            else {
                continue;
            };

            let decrement_allocated_resource_count = match *bindless_resource_type {
                BindlessResourceType::None => false,
                BindlessResourceType::Buffer => self
                    .buffers
                    .get_mut(&bindless_index)
                    .expect("Buffer should exist with that bindless index")
                    .remove(bindless_binding),
                BindlessResourceType::DataBuffer => {
                    self.data_buffers
                        .get_mut(&bindless_index)
                        .expect("Data buffer should exist with that bindless index")
                        .remove(bindless_binding);
                    false
                }
                BindlessResourceType::SamplerFiltering
                | BindlessResourceType::SamplerNonFiltering
                | BindlessResourceType::SamplerComparison => self
                    .samplers
                    .get_mut(bindless_resource_type)
                    .expect("Sampler array should exist")
                    .remove(bindless_binding),
                BindlessResourceType::Texture1d
                | BindlessResourceType::Texture2d
                | BindlessResourceType::Texture2dArray
                | BindlessResourceType::Texture3d
                | BindlessResourceType::TextureCube
                | BindlessResourceType::TextureCubeArray => self
                    .textures
                    .get_mut(bindless_resource_type)
                    .expect("Texture array should exist")
                    .remove(bindless_binding),
            };

            if decrement_allocated_resource_count {
                self.allocated_resource_count -= 1;
            }
        }

        self.bind_group = None;

        self.free_slots.push(slot);
        self.live_allocation_count -= 1;
    }

    fn prepare(
        &mut self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        label: &'static str,
        bind_group_layout: &BindGroupLayoutDescriptor,
        fallback_bindless_resources: &FallbackBindlessResources,
        fallback_buffers: &HashMap<BindlessIndex, Buffer>,
        fallback_image: &FallbackImage,
        bindless_descriptor: &BindlessDescriptor,
        slab_capacity: u32,
    ) {

        for bindless_index_table in &mut self.bindless_index_tables {
            bindless_index_table.buffer.prepare(render_device);
        }

        for data_buffer in self.data_buffers.values_mut() {
            data_buffer.buffer.prepare(render_device);
        }

        self.prepare_bind_group(
            render_device,
            pipeline_cache,
            label,
            bind_group_layout,
            fallback_bindless_resources,
            fallback_buffers,
            fallback_image,
            bindless_descriptor,
            slab_capacity,
        );
    }

    fn prepare_bind_group(
        &mut self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        label: &'static str,
        bind_group_layout: &BindGroupLayoutDescriptor,
        fallback_bindless_resources: &FallbackBindlessResources,
        fallback_buffers: &HashMap<BindlessIndex, Buffer>,
        fallback_image: &FallbackImage,
        bindless_descriptor: &BindlessDescriptor,
        slab_capacity: u32,
    ) {

        if self.bind_group.is_some() {
            return;
        }

        let required_binding_array_size = if render_device
            .features()
            .contains(WgpuFeatures::PARTIALLY_BOUND_BINDING_ARRAY)
        {
            None
        } else {
            Some(slab_capacity)
        };

        let binding_resource_arrays = self.create_binding_resource_arrays(
            fallback_bindless_resources,
            fallback_buffers,
            fallback_image,
            bindless_descriptor,
            required_binding_array_size,
        );

        let mut bind_group_entries: Vec<_> = self
            .bindless_index_tables
            .iter()
            .map(|bindless_index_table| bindless_index_table.bind_group_entry())
            .collect();

        for &(&binding, ref binding_resource_array) in binding_resource_arrays.iter() {
            bind_group_entries.push(BindGroupEntry {
                binding,
                resource: match *binding_resource_array {
                    BindingResourceArray::Buffers(ref buffer_bindings) => {
                        BindingResource::BufferArray(&buffer_bindings[..])
                    }
                    BindingResourceArray::TextureViews(ref texture_views) => {
                        BindingResource::TextureViewArray(&texture_views[..])
                    }
                    BindingResourceArray::Samplers(ref samplers) => {
                        BindingResource::SamplerArray(&samplers[..])
                    }
                },
            });
        }

        for data_buffer in self.data_buffers.values() {
            bind_group_entries.push(BindGroupEntry {
                binding: *data_buffer.binding_number,
                resource: data_buffer
                    .buffer
                    .buffer()
                    .expect("Backing data buffer must have been uploaded by now")
                    .as_entire_binding(),
            });
        }

        self.bind_group = Some(render_device.create_bind_group(
            Some(label),
            &pipeline_cache.get_bind_group_layout(bind_group_layout),
            &bind_group_entries,
        ));
    }

    fn write_buffer(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {
        for bindless_index_table in &mut self.bindless_index_tables {
            bindless_index_table
                .buffer
                .write(render_device, render_queue);
        }

        for data_buffer in self.data_buffers.values_mut() {
            data_buffer.buffer.write(render_device, render_queue);
        }
    }

    fn create_binding_resource_arrays<'a>(
        &'a self,
        fallback_bindless_resources: &'a FallbackBindlessResources,
        fallback_buffers: &'a HashMap<BindlessIndex, Buffer>,
        fallback_image: &'a FallbackImage,
        bindless_descriptor: &'a BindlessDescriptor,
        required_binding_array_size: Option<u32>,
    ) -> Vec<(&'a u32, BindingResourceArray<'a>)> {
        let mut binding_resource_arrays = vec![];

        self.create_sampler_binding_resource_arrays(
            &mut binding_resource_arrays,
            fallback_bindless_resources,
            bindless_descriptor,
            required_binding_array_size,
        );

        self.create_texture_binding_resource_arrays(
            &mut binding_resource_arrays,
            fallback_image,
            bindless_descriptor,
            required_binding_array_size,
        );

        self.create_buffer_binding_resource_arrays(
            &mut binding_resource_arrays,
            fallback_buffers,
            bindless_descriptor,
            required_binding_array_size,
        );

        binding_resource_arrays
    }

    fn create_sampler_binding_resource_arrays<'a, 'b>(
        &'a self,
        binding_resource_arrays: &'b mut Vec<(&'a u32, BindingResourceArray<'a>)>,
        fallback_bindless_resources: &'a FallbackBindlessResources,
        bindless_descriptor: &'a BindlessDescriptor,
        required_binding_array_size: Option<u32>,
    ) {

        for (bindless_resource_type, fallback_sampler) in [
            (
                BindlessResourceType::SamplerFiltering,
                &fallback_bindless_resources.filtering_sampler,
            ),
            (
                BindlessResourceType::SamplerNonFiltering,
                &fallback_bindless_resources.non_filtering_sampler,
            ),
            (
                BindlessResourceType::SamplerComparison,
                &fallback_bindless_resources.comparison_sampler,
            ),
        ] {

            if !bindless_descriptor
                .resources
                .contains(&bindless_resource_type)
            {
                continue;
            }

            let mut sampler_bindings = vec![];

            match self.samplers.get(&bindless_resource_type) {
                Some(sampler_bindless_binding_array) => {
                    for maybe_bindless_binding in sampler_bindless_binding_array.bindings.iter() {
                        match *maybe_bindless_binding {
                            Some(ref bindless_binding) => {
                                sampler_bindings.push(&*bindless_binding.resource);
                            }
                            None => sampler_bindings.push(&**fallback_sampler),
                        }
                    }
                }

                None => {

                    sampler_bindings.push(&**fallback_sampler);
                }
            }

            if let Some(required_binding_array_size) = required_binding_array_size {
                sampler_bindings.extend(iter::repeat_n(
                    &**fallback_sampler,
                    required_binding_array_size as usize - sampler_bindings.len(),
                ));
            }

            let binding_number = bindless_resource_type
                .binding_number()
                .expect("Sampler bindless resource type must have a binding number");

            binding_resource_arrays.push((
                &**binding_number,
                BindingResourceArray::Samplers(sampler_bindings),
            ));
        }
    }

    fn create_texture_binding_resource_arrays<'a, 'b>(
        &'a self,
        binding_resource_arrays: &'b mut Vec<(&'a u32, BindingResourceArray<'a>)>,
        fallback_image: &'a FallbackImage,
        bindless_descriptor: &'a BindlessDescriptor,
        required_binding_array_size: Option<u32>,
    ) {
        for (bindless_resource_type, fallback_image) in [
            (BindlessResourceType::Texture1d, &fallback_image.d1),
            (BindlessResourceType::Texture2d, &fallback_image.d2),
            (
                BindlessResourceType::Texture2dArray,
                &fallback_image.d2_array,
            ),
            (BindlessResourceType::Texture3d, &fallback_image.d3),
            (BindlessResourceType::TextureCube, &fallback_image.cube),
            (
                BindlessResourceType::TextureCubeArray,
                &fallback_image.cube_array,
            ),
        ] {

            if !bindless_descriptor
                .resources
                .contains(&bindless_resource_type)
            {
                continue;
            }

            let mut texture_bindings = vec![];

            let binding_number = bindless_resource_type
                .binding_number()
                .expect("Texture bindless resource type must have a binding number");

            match self.textures.get(&bindless_resource_type) {
                Some(texture_bindless_binding_array) => {
                    for maybe_bindless_binding in texture_bindless_binding_array.bindings.iter() {
                        match *maybe_bindless_binding {
                            Some(ref bindless_binding) => {
                                texture_bindings.push(&*bindless_binding.resource);
                            }
                            None => texture_bindings.push(&*fallback_image.texture_view),
                        }
                    }
                }

                None => {

                    texture_bindings.push(&*fallback_image.texture_view);
                }
            }

            if let Some(required_binding_array_size) = required_binding_array_size {
                texture_bindings.extend(iter::repeat_n(
                    &*fallback_image.texture_view,
                    required_binding_array_size as usize - texture_bindings.len(),
                ));
            }

            binding_resource_arrays.push((
                binding_number,
                BindingResourceArray::TextureViews(texture_bindings),
            ));
        }
    }

    fn create_buffer_binding_resource_arrays<'a, 'b>(
        &'a self,
        binding_resource_arrays: &'b mut Vec<(&'a u32, BindingResourceArray<'a>)>,
        fallback_buffers: &'a HashMap<BindlessIndex, Buffer>,
        bindless_descriptor: &'a BindlessDescriptor,
        required_binding_array_size: Option<u32>,
    ) {
        for bindless_buffer_descriptor in bindless_descriptor.buffers.iter() {
            let Some(buffer_bindless_binding_array) =
                self.buffers.get(&bindless_buffer_descriptor.bindless_index)
            else {

                continue;
            };

            let fallback_buffer = fallback_buffers
                .get(&bindless_buffer_descriptor.bindless_index)
                .expect("Fallback buffer should exist");

            let mut buffer_bindings: Vec<_> = buffer_bindless_binding_array
                .bindings
                .iter()
                .map(|maybe_bindless_binding| {
                    let buffer = match *maybe_bindless_binding {
                        None => fallback_buffer,
                        Some(ref bindless_binding) => &bindless_binding.resource,
                    };
                    BufferBinding {
                        buffer,
                        offset: 0,
                        size: None,
                    }
                })
                .collect();

            if let Some(required_binding_array_size) = required_binding_array_size {
                buffer_bindings.extend(iter::repeat_n(
                    BufferBinding {
                        buffer: fallback_buffer,
                        offset: 0,
                        size: None,
                    },
                    required_binding_array_size as usize - buffer_bindings.len(),
                ));
            }

            binding_resource_arrays.push((
                &*buffer_bindless_binding_array.binding_number,
                BindingResourceArray::Buffers(buffer_bindings),
            ));
        }
    }

    fn bind_group(&self) -> Option<&BindGroup> {
        self.bind_group.as_ref()
    }

    fn get_bindless_index_table(
        &self,
        bindless_index: BindlessIndex,
    ) -> Option<&MaterialBindlessIndexTable> {
        let table_index = self
            .bindless_index_tables
            .binary_search_by(|bindless_index_table| {
                if bindless_index < bindless_index_table.index_range.start {
                    Ordering::Less
                } else if bindless_index >= bindless_index_table.index_range.end {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .ok()?;
        self.bindless_index_tables.get(table_index)
    }
}

impl<R> MaterialBindlessBindingArray<R>
where
    R: GetBindingResourceId,
{

    fn new(
        binding_number: BindingNumber,
        resource_type: BindlessResourceType,
    ) -> MaterialBindlessBindingArray<R> {
        MaterialBindlessBindingArray {
            binding_number,
            bindings: vec![],
            resource_type,
            resource_to_slot: HashMap::default(),
            free_slots: vec![],
            len: 0,
        }
    }

    fn find(&self, binding_resource_id: BindingResourceId) -> Option<u32> {
        self.resource_to_slot.get(&binding_resource_id).copied()
    }

    fn insert(&mut self, binding_resource_id: BindingResourceId, resource: R) -> u32 {
        match self.resource_to_slot.entry(binding_resource_id) {
            bevy_platform::collections::hash_map::Entry::Occupied(o) => {
                let slot = *o.get();

                self.bindings[slot as usize]
                    .as_mut()
                    .expect("A slot in the resource_to_slot map should have a value")
                    .ref_count += 1;

                slot
            }
            bevy_platform::collections::hash_map::Entry::Vacant(v) => {
                let slot = self.free_slots.pop().unwrap_or(self.len);
                v.insert(slot);

                if self.bindings.len() < slot as usize + 1 {
                    self.bindings.resize_with(slot as usize + 1, || None);
                }
                debug_assert!(self.bindings[slot as usize].is_none());
                self.bindings[slot as usize] = Some(MaterialBindlessBinding::new(resource));

                self.len += 1;
                slot
            }
        }
    }

    fn remove(&mut self, slot: u32) -> bool {
        let maybe_binding = &mut self.bindings[slot as usize];
        let binding = maybe_binding
            .as_mut()
            .expect("Attempted to free an already-freed binding");

        binding.ref_count -= 1;
        if binding.ref_count != 0 {
            return false;
        }

        let binding_resource_id = binding.resource.binding_resource_id(self.resource_type);
        self.resource_to_slot.remove(&binding_resource_id);

        *maybe_binding = None;
        self.free_slots.push(slot);
        self.len -= 1;
        true
    }
}

impl<R> MaterialBindlessBinding<R>
where
    R: GetBindingResourceId,
{

    fn new(resource: R) -> MaterialBindlessBinding<R> {
        MaterialBindlessBinding {
            resource,
            ref_count: 1,
        }
    }
}

pub fn material_uses_bindless_resources<M>(render_device: &RenderDevice) -> bool
where
    M: Material,
{
    M::bindless_slot_count().is_some_and(|bindless_slot_count| {
        M::bindless_supported(render_device) && bindless_slot_count.resolve() > 1
    })
}

impl MaterialBindlessSlab {

    fn new(bindless_descriptor: &BindlessDescriptor) -> MaterialBindlessSlab {
        let mut buffers = HashMap::default();
        let mut samplers = HashMap::default();
        let mut textures = HashMap::default();
        let mut data_buffers = HashMap::default();

        for (bindless_index, bindless_resource_type) in
            bindless_descriptor.resources.iter().enumerate()
        {
            let bindless_index = BindlessIndex(bindless_index as u32);
            match *bindless_resource_type {
                BindlessResourceType::None => {}
                BindlessResourceType::Buffer => {
                    let binding_number = bindless_descriptor
                        .buffers
                        .iter()
                        .find(|bindless_buffer_descriptor| {
                            bindless_buffer_descriptor.bindless_index == bindless_index
                        })
                        .expect(
                            "Bindless buffer descriptor matching that bindless index should be \
                             present",
                        )
                        .binding_number;
                    buffers.insert(
                        bindless_index,
                        MaterialBindlessBindingArray::new(binding_number, *bindless_resource_type),
                    );
                }
                BindlessResourceType::DataBuffer => {

                    let buffer_descriptor = bindless_descriptor
                        .buffers
                        .iter()
                        .find(|bindless_buffer_descriptor| {
                            bindless_buffer_descriptor.bindless_index == bindless_index
                        })
                        .expect(
                            "Bindless buffer descriptor matching that bindless index should be \
                             present",
                        );
                    data_buffers.insert(
                        bindless_index,
                        MaterialDataBuffer::new(
                            buffer_descriptor.binding_number,
                            buffer_descriptor
                                .size
                                .expect("Data buffers should have a size")
                                as u32,
                        ),
                    );
                }
                BindlessResourceType::SamplerFiltering
                | BindlessResourceType::SamplerNonFiltering
                | BindlessResourceType::SamplerComparison => {
                    samplers.insert(
                        *bindless_resource_type,
                        MaterialBindlessBindingArray::new(
                            *bindless_resource_type.binding_number().unwrap(),
                            *bindless_resource_type,
                        ),
                    );
                }
                BindlessResourceType::Texture1d
                | BindlessResourceType::Texture2d
                | BindlessResourceType::Texture2dArray
                | BindlessResourceType::Texture3d
                | BindlessResourceType::TextureCube
                | BindlessResourceType::TextureCubeArray => {
                    textures.insert(
                        *bindless_resource_type,
                        MaterialBindlessBindingArray::new(
                            *bindless_resource_type.binding_number().unwrap(),
                            *bindless_resource_type,
                        ),
                    );
                }
            }
        }

        let bindless_index_tables = bindless_descriptor
            .index_tables
            .iter()
            .map(MaterialBindlessIndexTable::new)
            .collect();

        MaterialBindlessSlab {
            bind_group: None,
            bindless_index_tables,
            samplers,
            textures,
            buffers,
            data_buffers,
            free_slots: vec![],
            live_allocation_count: 0,
            allocated_resource_count: 0,
        }
    }
}

pub fn init_fallback_bindless_resources(mut commands: Commands, render_device: Res<RenderDevice>) {
    commands.insert_resource(FallbackBindlessResources {
        filtering_sampler: render_device.create_sampler(&SamplerDescriptor {
            label: Some("fallback filtering sampler"),
            ..default()
        }),
        non_filtering_sampler: render_device.create_sampler(&SamplerDescriptor {
            label: Some("fallback non-filtering sampler"),
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Nearest,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..default()
        }),
        comparison_sampler: render_device.create_sampler(&SamplerDescriptor {
            label: Some("fallback comparison sampler"),
            compare: Some(CompareFunction::Always),
            ..default()
        }),
    });
}

impl MaterialBindGroupNonBindlessAllocator {

    fn new(label: &'static str) -> MaterialBindGroupNonBindlessAllocator {
        MaterialBindGroupNonBindlessAllocator {
            label,
            bind_groups: vec![],
            to_prepare: HashSet::default(),
            free_indices: vec![],
        }
    }

    fn allocate(&mut self, bind_group: MaterialNonBindlessAllocatedBindGroup) -> MaterialBindingId {
        let group_id = self
            .free_indices
            .pop()
            .unwrap_or(MaterialBindGroupIndex(self.bind_groups.len() as u32));
        if self.bind_groups.len() < *group_id as usize + 1 {
            self.bind_groups
                .resize_with(*group_id as usize + 1, || None);
        }

        if matches!(
            bind_group,
            MaterialNonBindlessAllocatedBindGroup::Unprepared { .. }
        ) {
            self.to_prepare.insert(group_id);
        }

        self.bind_groups[*group_id as usize] = Some(bind_group);

        MaterialBindingId {
            group: group_id,
            slot: default(),
        }
    }

    fn allocate_unprepared(
        &mut self,
        unprepared_bind_group: UnpreparedBindGroup,
        bind_group_layout: BindGroupLayoutDescriptor,
    ) -> MaterialBindingId {
        self.allocate(MaterialNonBindlessAllocatedBindGroup::Unprepared {
            bind_group: unprepared_bind_group,
            layout: bind_group_layout,
        })
    }

    fn allocate_prepared(&mut self, prepared_bind_group: PreparedBindGroup) -> MaterialBindingId {
        self.allocate(MaterialNonBindlessAllocatedBindGroup::Prepared {
            bind_group: prepared_bind_group,
            uniform_buffers: vec![],
        })
    }

    fn free(&mut self, binding_id: MaterialBindingId) {
        debug_assert_eq!(binding_id.slot, MaterialBindGroupSlot(0));
        debug_assert!(self.bind_groups[*binding_id.group as usize].is_some());
        self.bind_groups[*binding_id.group as usize] = None;
        self.to_prepare.remove(&binding_id.group);
        self.free_indices.push(binding_id.group);
    }

    fn get(&self, group: MaterialBindGroupIndex) -> Option<MaterialNonBindlessSlab<'_>> {
        self.bind_groups[group.0 as usize]
            .as_ref()
            .map(|bind_group| match bind_group {
                MaterialNonBindlessAllocatedBindGroup::Prepared { bind_group, .. } => {
                    MaterialNonBindlessSlab::Prepared(bind_group)
                }
                MaterialNonBindlessAllocatedBindGroup::Unprepared { .. } => {
                    MaterialNonBindlessSlab::Unprepared
                }
            })
    }

    fn prepare_bind_groups(
        &mut self,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
    ) {
        for bind_group_index in mem::take(&mut self.to_prepare) {
            let Some(MaterialNonBindlessAllocatedBindGroup::Unprepared {
                bind_group: unprepared_bind_group,
                layout: bind_group_layout,
            }) = mem::take(&mut self.bind_groups[*bind_group_index as usize])
            else {
                panic!("Allocation didn't exist or was already prepared");
            };

            let mut uniform_buffers = vec![];
            for (index, binding) in unprepared_bind_group.bindings.iter() {
                let OwnedBindingResource::Data(ref owned_data) = *binding else {
                    continue;
                };
                let label = format!("material uniform data {}", *index);
                let uniform_buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
                    label: Some(&label),
                    contents: &owned_data.0,
                    usage: BufferUsages::COPY_DST | BufferUsages::UNIFORM,
                });
                uniform_buffers.push(uniform_buffer);
            }

            let mut bind_group_entries = vec![];
            let mut uniform_buffers_iter = uniform_buffers.iter();
            for (index, binding) in unprepared_bind_group.bindings.iter() {
                match *binding {
                    OwnedBindingResource::Data(_) => {
                        bind_group_entries.push(BindGroupEntry {
                            binding: *index,
                            resource: uniform_buffers_iter
                                .next()
                                .expect("We should have created uniform buffers for each `Data`")
                                .as_entire_binding(),
                        });
                    }
                    _ => bind_group_entries.push(BindGroupEntry {
                        binding: *index,
                        resource: binding.get_binding(),
                    }),
                }
            }

            let bind_group = render_device.create_bind_group(
                self.label,
                &pipeline_cache.get_bind_group_layout(&bind_group_layout),
                &bind_group_entries,
            );

            self.bind_groups[*bind_group_index as usize] =
                Some(MaterialNonBindlessAllocatedBindGroup::Prepared {
                    bind_group: PreparedBindGroup {
                        bindings: unprepared_bind_group.bindings,
                        bind_group,
                    },
                    uniform_buffers,
                });
        }
    }
}

impl<'a> MaterialSlab<'a> {

    pub fn bind_group(&self) -> Option<&'a BindGroup> {
        match self.0 {
            MaterialSlabImpl::Bindless(material_bindless_slab) => {
                material_bindless_slab.bind_group()
            }
            MaterialSlabImpl::NonBindless(MaterialNonBindlessSlab::Prepared(
                prepared_bind_group,
            )) => Some(&prepared_bind_group.bind_group),
            MaterialSlabImpl::NonBindless(MaterialNonBindlessSlab::Unprepared) => None,
        }
    }
}

impl MaterialDataBuffer {

    fn new(binding_number: BindingNumber, aligned_element_size: u32) -> MaterialDataBuffer {
        MaterialDataBuffer {
            binding_number,
            buffer: RetainedRawBufferVec::new(BufferUsages::STORAGE),
            aligned_element_size,
            free_slots: vec![],
            len: 0,
        }
    }

    fn insert(&mut self, data: &[u8]) -> u32 {

        debug_assert_eq!(data.len(), self.aligned_element_size as usize);

        let slot = self.free_slots.pop().unwrap_or(self.len);

        let start = slot as usize * self.aligned_element_size as usize;
        let end = (slot as usize + 1) * self.aligned_element_size as usize;

        if self.buffer.len() < end {
            self.buffer.reserve_internal(end);
        }
        while self.buffer.values().len() < end {
            self.buffer.push(0);
        }

        self.buffer.values_mut()[start..end].copy_from_slice(data);

        self.len += 1;
        self.buffer.dirty = BufferDirtyState::NeedsReserve;
        slot
    }

    fn remove(&mut self, slot: u32) {
        self.free_slots.push(slot);
        self.len -= 1;
    }
}
