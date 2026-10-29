

use alloc::borrow::Cow;
use bevy_derive::{Deref, DerefMut};
use bevy_log::error;
use bevy_platform::collections::{hash_map::Entry, HashMap, HashSet};
use core::{
    cmp::Ordering,
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::Range,
};
use nonmax::NonMaxU32;
use offset_allocator::{Allocation, Allocator};
use wgpu::{BufferDescriptor, BufferSize, BufferUsages, CommandEncoderDescriptor, WriteOnly};

use crate::{
    render_resource::Buffer,
    renderer::{RenderDevice, RenderQueue},
};

pub struct SlabAllocator<I>
where
    I: SlabItem,
{

    pub slabs: HashMap<SlabId<I>, Slab<I>>,

    next_slab_id: SlabId<I>,

    pub key_to_slab: HashMap<I::Key, SlabId<I>>,

    slab_layouts: HashMap<I::Layout, Vec<SlabId<I>>>,

    pub extra_buffer_usages: BufferUsages,
}

pub trait SlabItem {

    type Key: Clone + PartialEq + Eq + Hash;

    type Layout: SlabItemLayout;

    fn label() -> Cow<'static, str>;
}

pub trait SlabItemLayout: Clone + PartialEq + Eq + Hash {

    fn size(&self) -> u64;

    fn elements_per_slot(&self) -> u32;

    fn buffer_usages(&self) -> BufferUsages;
}

trait SlabItemLayoutExt {

    fn slot_size(&self) -> u64;
}

impl<I> SlabItemLayoutExt for I
where
    I: SlabItemLayout,
{
    fn slot_size(&self) -> u64 {
        self.size() * self.elements_per_slot() as u64
    }
}

pub struct SlabAllocatorSettings {

    pub min_slab_size: u64,

    pub max_slab_size: u64,

    pub large_threshold: u64,

    pub growth_factor: f64,
}

impl Default for SlabAllocatorSettings {
    fn default() -> Self {
        Self {

            min_slab_size: 1024 * 1024,

            max_slab_size: 1024 * 1024 * 512,

            large_threshold: 1024 * 1024 * 256,

            growth_factor: 1.5,
        }
    }
}

#[derive(Deref, DerefMut)]
#[repr(transparent)]
pub struct SlabId<I>
where
    I: SlabItem,
{

    #[deref]
    pub id: NonMaxU32,
    phantom: PhantomData<I>,
}

impl<I> Clone for SlabId<I>
where
    I: SlabItem,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<I> Copy for SlabId<I> where I: SlabItem {}

impl<I> Default for SlabId<I>
where
    I: SlabItem,
{
    fn default() -> Self {
        SlabId {
            id: NonMaxU32::default(),
            phantom: PhantomData,
        }
    }
}

impl<I> PartialEq for SlabId<I>
where
    I: SlabItem,
{
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<I> Eq for SlabId<I> where I: SlabItem {}

impl<I> PartialOrd for SlabId<I>
where
    I: SlabItem,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<I> Ord for SlabId<I>
where
    I: SlabItem,
{
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(other)
    }
}

impl<I> Hash for SlabId<I>
where
    I: SlabItem,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<I> Debug for SlabId<I>
where
    I: SlabItem,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("SlabId").field("id", &self.id).finish()
    }
}

#[expect(
    clippy::large_enum_variant,
    reason = "See https://github.com/bevyengine/bevy/issues/19220"
)]
pub enum Slab<I>
where
    I: SlabItem,
{

    General(GeneralSlab<I>),

    LargeObject(LargeObjectSlab<I>),
}

pub struct GeneralSlab<I>
where
    I: SlabItem,
{

    allocator: Allocator,

    buffer: Option<Buffer>,

    resident_allocations: HashMap<I::Key, SlabAllocation>,

    pending_allocations: HashMap<I::Key, SlabAllocation>,

    element_layout: I::Layout,

    current_slot_capacity: u32,
}

pub struct LargeObjectSlab<I>
where
    I: SlabItem,
{

    buffer: Option<Buffer>,

    element_layout: I::Layout,
}

struct SlabItemAllocation<I>
where
    I: SlabItem,
{

    slab_id: SlabId<I>,

    slab_allocation: SlabAllocation,
}

impl<I> Slab<I>
where
    I: SlabItem,
{

    pub fn buffer(&self) -> Option<&Buffer> {
        match self {
            Slab::General(general_slab) => general_slab.buffer.as_ref(),
            Slab::LargeObject(large_object_slab) => large_object_slab.buffer.as_ref(),
        }
    }

    pub fn buffer_size(&self) -> u64 {
        match self.buffer() {
            Some(buffer) => buffer.size(),
            None => 0,
        }
    }

    pub fn element_layout(&self) -> &I::Layout {
        match self {
            Slab::General(general_slab) => &general_slab.element_layout,
            Slab::LargeObject(large_object_slab) => &large_object_slab.element_layout,
        }
    }
}

pub struct AllocationStage<'a, I>
where
    I: SlabItem,
{

    pub allocator: &'a mut SlabAllocator<I>,

    slabs_to_reallocate: HashMap<SlabId<I>, SlabToReallocate>,

    empty_slabs: HashSet<SlabId<I>>,
}

impl<'a, I> Drop for AllocationStage<'a, I>
where
    I: SlabItem,
{
    fn drop(&mut self) {
        if !self.slabs_to_reallocate.is_empty() || !self.empty_slabs.is_empty() {
            error!(
                "Dropping an `AllocationStage` with uncommitted reallocations or slab free \
                operations. You should call `AllocationStage::commit`."
            );
        }
    }
}

impl<'a, I> AllocationStage<'a, I>
where
    I: SlabItem,
{

    pub fn allocate(
        &mut self,
        key: &I::Key,
        data_byte_len: u64,
        layout: I::Layout,
        settings: &SlabAllocatorSettings,
    ) {
        self.allocator
            .free_existing_allocation(key, &mut self.empty_slabs);
        self.allocator.allocate(
            key,
            data_byte_len,
            layout,
            &mut self.slabs_to_reallocate,
            settings,
        );
    }

    pub fn allocate_large(&mut self, key: &I::Key, layout: I::Layout) {
        self.allocator
            .free_existing_allocation(key, &mut self.empty_slabs);
        self.allocator.allocate_large(key, layout);
    }

    pub fn commit(mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {

        self.allocator.free_empty_slabs(self.empty_slabs.drain());

        for (slab_id, slab_to_grow) in self.slabs_to_reallocate.drain() {

            if !self.allocator.slabs.contains_key(&slab_id) {
                continue;
            }
            self.allocator
                .reallocate_slab(render_device, render_queue, slab_id, slab_to_grow);
        }
    }
}

pub struct DeallocationStage<'a, I>
where
    I: SlabItem,
{

    pub allocator: &'a mut SlabAllocator<I>,

    empty_slabs: HashSet<SlabId<I>>,
}

impl<'a, I> Drop for DeallocationStage<'a, I>
where
    I: SlabItem,
{
    fn drop(&mut self) {
        if !self.empty_slabs.is_empty() {
            error!(
                "Dropping a `DeallocationStage` with uncommitted slab free operations. You should \
                call `DeallocationStage::commit`."
            );
        }
    }
}

impl<'a, I> DeallocationStage<'a, I>
where
    I: SlabItem,
{

    pub fn free(&mut self, key: &I::Key) {
        self.allocator
            .free_existing_allocation(key, &mut self.empty_slabs);
    }

    pub fn commit(mut self) {
        self.allocator.free_empty_slabs(self.empty_slabs.drain());
    }
}

#[derive(Clone)]
struct SlabAllocation {

    allocation: Allocation,

    slot_count: u32,

    padding: u32,
}

pub struct SlabAllocationBufferSlice<'a, I>
where
    I: SlabItem,
{

    pub buffer: &'a Buffer,

    pub range: Range<u32>,

    phantom: PhantomData<I>,
}

#[derive(Default)]
pub struct SlabToReallocate {

    old_slot_capacity: u32,
}

impl<I> Display for SlabId<I>
where
    I: SlabItem,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.id, f)
    }
}

impl<I> Default for SlabAllocator<I>
where
    I: SlabItem,
{
    fn default() -> Self {
        Self {
            slabs: HashMap::default(),
            next_slab_id: SlabId {
                id: NonMaxU32::default(),
                phantom: PhantomData,
            },
            key_to_slab: HashMap::default(),
            slab_layouts: HashMap::default(),
            extra_buffer_usages: BufferUsages::empty(),
        }
    }
}

impl<I> SlabAllocator<I>
where
    I: SlabItem,
{

    pub fn new() -> Self {
        Self::default()
    }

    pub fn stage_allocation(&'_ mut self) -> AllocationStage<'_, I> {
        AllocationStage {
            allocator: self,
            slabs_to_reallocate: HashMap::default(),
            empty_slabs: HashSet::default(),
        }
    }

    pub fn stage_deallocation(&'_ mut self) -> DeallocationStage<'_, I> {
        DeallocationStage {
            allocator: self,
            empty_slabs: HashSet::default(),
        }
    }

    fn allocate(
        &mut self,
        key: &I::Key,
        data_byte_len: u64,
        layout: I::Layout,
        slabs_to_grow: &mut HashMap<SlabId<I>, SlabToReallocate>,
        settings: &SlabAllocatorSettings,
    ) {
        debug_assert!(!self.key_to_slab.contains_key(key));

        let data_element_count = data_byte_len.div_ceil(layout.size()) as u32;
        let data_slot_count = data_element_count.div_ceil(layout.elements_per_slot());
        let padding = data_slot_count * layout.elements_per_slot() - data_element_count;

        if data_slot_count as u64 * layout.slot_size()
            >= settings.large_threshold.min(settings.max_slab_size)
        {
            self.allocate_large(key, layout);
        } else {
            self.allocate_general(
                key,
                data_slot_count,
                padding,
                layout,
                slabs_to_grow,
                settings,
            );
        }
    }

    fn allocate_general(
        &mut self,
        key: &I::Key,
        data_slot_count: u32,
        padding: u32,
        layout: I::Layout,
        slabs_to_grow: &mut HashMap<SlabId<I>, SlabToReallocate>,
        settings: &SlabAllocatorSettings,
    ) {
        let candidate_slabs = self.slab_layouts.entry(layout.clone()).or_default();

        let mut data_allocation = None;
        for &slab_id in &*candidate_slabs {
            let Some(Slab::General(slab)) = self.slabs.get_mut(&slab_id) else {
                unreachable!("Slab not found")
            };

            let Some(allocation) = slab.allocator.allocate(data_slot_count) else {
                continue;
            };

            match slab.grow_if_necessary(allocation.offset + data_slot_count, settings) {
                SlabGrowthResult::NoGrowthNeeded => {}
                SlabGrowthResult::NeededGrowth(slab_to_reallocate) => {

                    if let Entry::Vacant(vacant_entry) = slabs_to_grow.entry(slab_id) {
                        vacant_entry.insert(slab_to_reallocate);
                    }
                }
                SlabGrowthResult::CantGrow => continue,
            }

            data_allocation = Some(SlabItemAllocation {
                slab_id,
                slab_allocation: SlabAllocation {
                    allocation,
                    slot_count: data_slot_count,
                    padding,
                },
            });
            break;
        }

        if data_allocation.is_none() {
            let new_slab_id = self.next_slab_id;
            self.next_slab_id.id =
                NonMaxU32::new(self.next_slab_id.id.get() + 1).unwrap_or_default();

            let new_slab = GeneralSlab::new(
                new_slab_id,
                &mut data_allocation,
                settings,
                layout,
                data_slot_count,
                padding,
            );

            self.slabs.insert(new_slab_id, Slab::General(new_slab));
            candidate_slabs.push(new_slab_id);
            slabs_to_grow.insert(new_slab_id, SlabToReallocate::default());
        }

        let data_allocation = data_allocation.expect("Should have been able to allocate");

        if let Some(Slab::General(general_slab)) = self.slabs.get_mut(&data_allocation.slab_id) {
            general_slab
                .pending_allocations
                .insert(key.clone(), data_allocation.slab_allocation);
        };

        self.record_allocation(key, data_allocation.slab_id);
    }

    fn allocate_large(&mut self, key: &I::Key, layout: I::Layout) {
        let new_slab_id = self.next_slab_id;
        self.next_slab_id.id = NonMaxU32::new(self.next_slab_id.id.get() + 1).unwrap_or_default();

        self.record_allocation(key, new_slab_id);

        self.slabs.insert(
            new_slab_id,
            Slab::LargeObject(LargeObjectSlab {
                buffer: None,
                element_layout: layout,
            }),
        );
    }

    fn free_existing_allocation(&mut self, key: &I::Key, empty_slabs: &mut HashSet<SlabId<I>>) {
        if let Some(slab_id) = self.key_to_slab.remove(key) {
            self.free_allocation_in_slab(key, slab_id, empty_slabs);
        }
    }

    fn free_allocation_in_slab(
        &mut self,
        key: &I::Key,
        slab_id: SlabId<I>,
        empty_slabs: &mut HashSet<SlabId<I>>,
    ) {
        let Some(slab) = self.slabs.get_mut(&slab_id) else {
            error!("Double free: attempted to free data in a nonexistent slab");
            return;
        };

        match *slab {
            Slab::General(ref mut general_slab) => {
                let Some(slab_allocation) = general_slab
                    .resident_allocations
                    .remove(key)
                    .or_else(|| general_slab.pending_allocations.remove(key))
                else {
                    return;
                };

                general_slab.allocator.free(slab_allocation.allocation);

                if general_slab.is_empty() {
                    empty_slabs.insert(slab_id);
                }
            }
            Slab::LargeObject(_) => {
                empty_slabs.insert(slab_id);
            }
        }
    }

    fn reallocate_slab(
        &mut self,
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
        slab_id: SlabId<I>,
        slab_to_grow: SlabToReallocate,
    ) {
        let Some(Slab::General(slab)) = self.slabs.get_mut(&slab_id) else {
            error!("Couldn't find slab {} to grow", slab_id);
            return;
        };

        let old_buffer = slab.buffer.take();

        let buffer_usages =
            BufferUsages::COPY_SRC | BufferUsages::COPY_DST | slab.element_layout.buffer_usages();

        let new_buffer = render_device.create_buffer(&BufferDescriptor {
            label: Some(&format!(
                "general {} slab {} ({}buffer)",
                I::label(),
                slab_id,
                buffer_usages_to_str(buffer_usages)
            )),
            size: slab.current_slot_capacity as u64 * slab.element_layout.slot_size(),
            usage: buffer_usages | self.extra_buffer_usages,
            mapped_at_creation: false,
        });

        slab.buffer = Some(new_buffer.clone());

        let Some(old_buffer) = old_buffer else { return };

        let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some(&*format!("{} slab resize encoder", I::label())),
        });

        encoder.copy_buffer_to_buffer(
            &old_buffer,
            0,
            &new_buffer,
            0,
            slab_to_grow.old_slot_capacity as u64 * slab.element_layout.slot_size(),
        );

        let command_buffer = encoder.finish();
        render_queue.submit([command_buffer]);
    }

    fn record_allocation(&mut self, key: &I::Key, slab_id: SlabId<I>) {
        self.key_to_slab.insert(key.clone(), slab_id);
    }

    pub fn buffer_for_slab(&self, slab_id: SlabId<I>) -> Option<&Buffer> {
        self.slabs.get(&slab_id).and_then(|slab| slab.buffer())
    }

    pub fn slab_allocation_slice(
        &self,
        key: &I::Key,
        slab_id: SlabId<I>,
    ) -> Option<SlabAllocationBufferSlice<'_, I>> {
        match self.slabs.get(&slab_id)? {
            Slab::General(general_slab) => {
                let slab_allocation = general_slab.resident_allocations.get(key)?;
                Some(SlabAllocationBufferSlice {
                    buffer: general_slab.buffer.as_ref()?,
                    range: (slab_allocation.allocation.offset
                        * general_slab.element_layout.elements_per_slot())
                        ..((slab_allocation.allocation.offset + slab_allocation.slot_count)
                            * general_slab.element_layout.elements_per_slot())
                            - slab_allocation.padding,
                    phantom: PhantomData,
                })
            }

            Slab::LargeObject(large_object_slab) => {
                let buffer = large_object_slab.buffer.as_ref()?;
                Some(SlabAllocationBufferSlice {
                    buffer,
                    range: 0..((buffer.size() / large_object_slab.element_layout.size()) as u32),
                    phantom: PhantomData,
                })
            }
        }
    }

    fn free_empty_slabs(&mut self, empty_slabs: impl Iterator<Item = SlabId<I>>) {
        for empty_slab in empty_slabs {

            if let Some(Slab::General(general_slab)) = self.slabs.get(&empty_slab)
                && !general_slab.is_empty()
            {
                continue;
            }

            self.slab_layouts.values_mut().for_each(|slab_ids| {
                let idx = slab_ids.iter().position(|&slab_id| slab_id == empty_slab);
                if let Some(idx) = idx {
                    slab_ids.remove(idx);
                }
            });
            self.slabs.remove(&empty_slab);
        }
    }

    pub fn slab_count(&self) -> usize {
        self.slabs.len()
    }

    pub fn slabs_size(&self) -> u64 {
        self.slabs.iter().map(|slab| slab.1.buffer_size()).sum()
    }

    pub fn copy_element_data(
        &mut self,
        key: &I::Key,
        len: usize,
        fill_data: impl Fn(WriteOnly<[u8]>),
        render_device: &RenderDevice,
        render_queue: &RenderQueue,
    ) {
        let Some(slab_id) = self.key_to_slab.get(key) else {
            error!("Use-after-free: attempted to copy element data for an unallocated key");
            return;
        };
        let Some(slab) = self.slabs.get_mut(slab_id) else {
            error!("Use-after-free: attempted to copy element data into a nonexistent slab");
            return;
        };

        match *slab {
            Slab::General(ref mut general_slab) => {
                let (Some(buffer), Some(allocated_range)) = (
                    &general_slab.buffer,
                    general_slab.pending_allocations.remove(key),
                ) else {
                    return;
                };

                let slot_size = general_slab.element_layout.slot_size();

                if let Some(size) = BufferSize::new((len as u64).next_multiple_of(slot_size)) {

                    if let Some(mut buffer) = render_queue.write_buffer_with(
                        buffer,
                        allocated_range.allocation.offset as u64 * slot_size,
                        size,
                    ) {
                        let slice = buffer.slice(..len);
                        fill_data(slice);
                    }
                }

                general_slab
                    .resident_allocations
                    .insert(key.clone(), allocated_range);
            }

            Slab::LargeObject(ref mut large_object_slab) => {
                debug_assert!(large_object_slab.buffer.is_none());

                let buffer_usages = large_object_slab.element_layout.buffer_usages();
                let buffer = render_device.create_buffer(&BufferDescriptor {
                    label: Some(&format!(
                        "large {} slab {} ({}buffer)",
                        I::label(),
                        slab_id,
                        buffer_usages_to_str(buffer_usages)
                    )),
                    size: len as u64,
                    usage: buffer_usages | BufferUsages::COPY_DST,
                    mapped_at_creation: true,
                });
                {
                    let mut slice = buffer.slice(..).get_mapped_range_mut();

                    fill_data(slice.slice(..len));
                }
                buffer.unmap();
                large_object_slab.buffer = Some(buffer);
            }
        }
    }
}

enum SlabGrowthResult {

    NoGrowthNeeded,

    NeededGrowth(SlabToReallocate),

    CantGrow,
}

impl<I> GeneralSlab<I>
where
    I: SlabItem,
{

    fn new(
        new_slab_id: SlabId<I>,
        maybe_slab_item_allocation: &mut Option<SlabItemAllocation<I>>,
        settings: &SlabAllocatorSettings,
        layout: I::Layout,
        data_slot_count: u32,
        padding: u32,
    ) -> GeneralSlab<I> {
        let initial_slab_slot_capacity = (settings.min_slab_size.div_ceil(layout.slot_size())
            as u32)
            .max(offset_allocator::ext::min_allocator_size(data_slot_count));
        let max_slab_slot_capacity = (settings.max_slab_size.div_ceil(layout.slot_size()) as u32)
            .max(offset_allocator::ext::min_allocator_size(data_slot_count));

        let mut new_slab = GeneralSlab {
            allocator: Allocator::new(max_slab_slot_capacity),
            buffer: None,
            resident_allocations: HashMap::default(),
            pending_allocations: HashMap::default(),
            element_layout: layout,
            current_slot_capacity: initial_slab_slot_capacity,
        };

        if let Some(allocation) = new_slab.allocator.allocate(data_slot_count) {
            *maybe_slab_item_allocation = Some(SlabItemAllocation {
                slab_id: new_slab_id,
                slab_allocation: SlabAllocation {
                    slot_count: data_slot_count,
                    allocation,
                    padding,
                },
            });
        }

        new_slab
    }

    fn grow_if_necessary(
        &mut self,
        new_size_in_slots: u32,
        settings: &SlabAllocatorSettings,
    ) -> SlabGrowthResult {

        let initial_slot_capacity = self.current_slot_capacity;
        if self.current_slot_capacity >= new_size_in_slots {
            return SlabGrowthResult::NoGrowthNeeded;
        }

        while self.current_slot_capacity < new_size_in_slots {
            let new_slab_slot_capacity =
                ((self.current_slot_capacity as f64 * settings.growth_factor).ceil() as u32)
                    .min((settings.max_slab_size / self.element_layout.slot_size()) as u32);
            if new_slab_slot_capacity == self.current_slot_capacity {

                return SlabGrowthResult::CantGrow;
            }

            self.current_slot_capacity = new_slab_slot_capacity;
        }

        SlabGrowthResult::NeededGrowth(SlabToReallocate {
            old_slot_capacity: initial_slot_capacity,
        })
    }

    fn is_empty(&self) -> bool {
        self.resident_allocations.is_empty() && self.pending_allocations.is_empty()
    }
}

fn buffer_usages_to_str(buffer_usages: BufferUsages) -> &'static str {
    if buffer_usages.contains(BufferUsages::VERTEX) {
        "vertex "
    } else if buffer_usages.contains(BufferUsages::INDEX) {
        "index "
    } else if buffer_usages.contains(BufferUsages::STORAGE) {
        "storage "
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::create_dummy_device;

    struct TestItem;

    impl SlabItem for TestItem {
        type Key = u32;
        type Layout = TestLayout;

        fn label() -> Cow<'static, str> {
            "test".into()
        }
    }

    #[derive(Clone, PartialEq, Eq, Hash)]
    struct TestLayout;

    impl SlabItemLayout for TestLayout {
        fn size(&self) -> u64 {
            4
        }

        fn elements_per_slot(&self) -> u32 {
            1
        }

        fn buffer_usages(&self) -> BufferUsages {
            BufferUsages::VERTEX
        }
    }

    fn test_settings() -> SlabAllocatorSettings {
        SlabAllocatorSettings {

            min_slab_size: 1024,

            max_slab_size: 4096,
            large_threshold: 4096,
            growth_factor: 1.5,
        }
    }

    fn allocate_round(
        allocator: &mut SlabAllocator<TestItem>,
        keys: impl Iterator<Item = u32> + Clone,
        byte_len: u64,
        device: &RenderDevice,
        queue: &RenderQueue,
    ) {
        let mut stage = allocator.stage_allocation();
        for key in keys.clone() {
            stage.allocate(&key, byte_len, TestLayout, &test_settings());
        }
        stage.commit(device, queue);

        for key in keys {
            allocator.copy_element_data(&key, byte_len as usize, |_| {}, device, queue);
        }
    }

    #[test]
    fn reallocating_a_live_key_frees_the_old_allocation() {
        let (device, queue) = create_dummy_device();
        let mut allocator = SlabAllocator::<TestItem>::new();

        allocate_round(&mut allocator, 0..8, 256, &device, &queue);

        let baseline_size = allocator.slabs_size();
        let baseline_slabs = allocator.slab_count();
        assert!(baseline_size > 0, "nothing was allocated");

        for _ in 0..32 {
            allocate_round(&mut allocator, 0..8, 256, &device, &queue);
        }

        assert_eq!(
            allocator.slabs_size(),
            baseline_size,
            "reallocating live keys grew the slabs, so old allocations leaked"
        );
        assert_eq!(allocator.slab_count(), baseline_slabs);
        assert_eq!(allocator.key_to_slab.len(), 8);

        for key in 0..8u32 {
            let slab_id = allocator.key_to_slab[&key];
            assert!(allocator.slab_allocation_slice(&key, slab_id).is_some());
        }
    }

    #[test]
    fn slab_refilled_during_allocation_is_not_freed() {
        let (device, queue) = create_dummy_device();
        let mut allocator = SlabAllocator::<TestItem>::new();

        allocate_round(&mut allocator, 0..1, 256, &device, &queue);
        let slab_id = allocator.key_to_slab[&0];

        allocate_round(&mut allocator, 0..1, 256, &device, &queue);

        assert_eq!(allocator.slab_count(), 1, "the live slab was destroyed");
        assert_eq!(allocator.key_to_slab[&0], slab_id);
        assert!(allocator.slab_allocation_slice(&0, slab_id).is_some());
    }

    fn allocate_large_round(
        allocator: &mut SlabAllocator<TestItem>,
        keys: impl Iterator<Item = u32>,
        device: &RenderDevice,
        queue: &RenderQueue,
    ) {
        let mut stage = allocator.stage_allocation();
        for key in keys {
            stage.allocate_large(&key, TestLayout);
        }
        stage.commit(device, queue);
    }

    #[test]
    fn reallocating_a_live_key_with_allocate_large_frees_the_old_allocation() {
        let (device, queue) = create_dummy_device();
        let mut allocator = SlabAllocator::<TestItem>::new();

        allocate_large_round(&mut allocator, 0..4, &device, &queue);
        assert_eq!(allocator.slab_count(), 4);

        for _ in 0..32 {
            allocate_large_round(&mut allocator, 0..4, &device, &queue);
        }

        assert_eq!(
            allocator.slab_count(),
            4,
            "reallocating live keys left the previous dedicated slabs behind"
        );
        assert_eq!(allocator.key_to_slab.len(), 4);
    }

    #[test]
    fn slab_emptied_during_allocation_is_freed() {
        let (device, queue) = create_dummy_device();
        let mut allocator = SlabAllocator::<TestItem>::new();

        allocate_round(&mut allocator, 0..1, 256, &device, &queue);
        assert_eq!(allocator.slab_count(), 1);

        allocate_round(&mut allocator, 0..1, 8192, &device, &queue);

        assert_eq!(
            allocator.slab_count(),
            1,
            "the emptied general slab was not reclaimed"
        );
        let slab_id = allocator.key_to_slab[&0];
        assert!(matches!(
            allocator.slabs.get(&slab_id),
            Some(Slab::LargeObject(_))
        ));
    }
}
