use core::{iter, marker::PhantomData, ops::Range, slice};

use crate::{
    render_resource::{AtomicPod, Buffer},
    renderer::{RenderDevice, RenderQueue},
};
use bytemuck::{must_cast_slice, NoUninit};
use encase::{
    internal::{WriteInto, Writer},
    ShaderType,
};
use thiserror::Error;
use wgpu::{BindingResource, BufferAddress, BufferUsages};

use super::GpuArrayBufferable;

pub struct RawBufferVec<T: NoUninit> {
    values: Vec<T>,
    buffer: Option<Buffer>,
    capacity: usize,
    item_size: usize,
    buffer_usage: BufferUsages,
    label: Option<String>,
    changed: bool,
}

impl<T: NoUninit> RawBufferVec<T> {

    pub const fn new(buffer_usage: BufferUsages) -> Self {
        Self {
            values: Vec::new(),
            buffer: None,
            capacity: 0,
            item_size: size_of::<T>(),
            buffer_usage,
            label: None,
            changed: false,
        }
    }

    #[inline]
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    #[inline]
    pub fn binding(&self) -> Option<BindingResource<'_>> {
        Some(BindingResource::Buffer(
            self.buffer()?.as_entire_buffer_binding(),
        ))
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn push(&mut self, value: T) -> usize {
        let index = self.values.len();
        self.values.push(value);
        index
    }

    pub fn append(&mut self, other: &mut RawBufferVec<T>) {
        self.values.append(&mut other.values);
    }

    pub fn get(&self, index: u32) -> Option<&T> {
        self.values.get(index as usize)
    }

    pub fn set(&mut self, index: u32, value: T) {
        self.values[index as usize] = value;
    }

    pub fn reserve_internal(&mut self, count: usize) {
        self.values.reserve(count);
    }

    pub fn set_label(&mut self, label: Option<&str>) {
        let label = label.map(str::to_string);

        if label != self.label {
            self.changed = true;
        }

        self.label = label;
    }

    pub fn get_label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn reserve(&mut self, capacity: usize, device: &RenderDevice) {
        let size = self.item_size * capacity;
        if capacity > self.capacity || (self.changed && size > 0) {
            self.capacity = capacity;
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: make_buffer_label::<Self>(&self.label),
                size: size as BufferAddress,
                usage: BufferUsages::COPY_DST | self.buffer_usage,
                mapped_at_creation: false,
            }));
            self.changed = false;
        }
    }

    pub fn write_buffer(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        if self.values.is_empty() {
            return;
        }
        self.reserve(self.values.len(), device);
        if let Some(buffer) = &self.buffer {
            let range = 0..self.item_size * self.values.len();
            let bytes: &[u8] = must_cast_slice(&self.values);
            queue.write_buffer(buffer, 0, &bytes[range]);
        }
    }

    pub fn write_buffer_range(
        &mut self,
        render_queue: &RenderQueue,
        range: Range<usize>,
    ) -> Result<(), WriteBufferRangeError> {
        if self.values.is_empty() {
            return Err(WriteBufferRangeError::NoValuesToUpload);
        }
        if range.end > self.item_size * self.capacity {
            return Err(WriteBufferRangeError::RangeBiggerThanBuffer);
        }
        if let Some(buffer) = &self.buffer {

            let bytes: &[u8] = must_cast_slice(&self.values[range.start..range.end]);
            render_queue.write_buffer(buffer, (range.start * self.item_size) as u64, bytes);
            Ok(())
        } else {
            Err(WriteBufferRangeError::BufferNotInitialized)
        }
    }

    pub fn truncate(&mut self, len: usize) {
        self.values.truncate(len);
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    pub fn pop(&mut self) -> Option<T> {
        self.values.pop()
    }

    pub fn swap_remove(&mut self, index: usize) -> T {
        self.values.swap_remove(index)
    }

    pub fn values(&self) -> &Vec<T> {
        &self.values
    }

    pub fn values_mut(&mut self) -> &mut Vec<T> {
        &mut self.values
    }
}

impl<T> RawBufferVec<T>
where
    T: NoUninit + Default,
{
    pub fn grow_set(&mut self, index: u32, value: T) {
        self.values.reserve(index as usize + 1);
        while index as usize + 1 > self.len() {
            self.values.push(T::default());
        }
        self.values[index as usize] = value;
    }
}

impl<T: NoUninit> Extend<T> for RawBufferVec<T> {
    #[inline]
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.values.extend(iter);
    }
}

pub struct AtomicRawBufferVec<T>
where
    T: AtomicPod,
{

    values: Vec<T::Blob>,

    buffer: Option<Buffer>,

    capacity: usize,

    buffer_usage: BufferUsages,

    label: Option<String>,

    changed: bool,
    phantom: PhantomData<T>,
}

impl<T> AtomicRawBufferVec<T>
where
    T: AtomicPod,
{

    pub const fn new(buffer_usage: BufferUsages) -> Self {
        Self {
            values: Vec::new(),
            buffer: None,
            capacity: 0,
            buffer_usage,
            label: None,
            changed: false,
            phantom: PhantomData,
        }
    }

    pub fn with_label(buffer_usage: BufferUsages, label: &str) -> Self {
        Self {
            values: Vec::new(),
            buffer: None,
            capacity: 0,
            buffer_usage,
            label: Some(label.to_string()),
            changed: false,
            phantom: PhantomData,
        }
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    pub fn len(&self) -> u32 {
        self.values.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&mut self, value: T) -> u32 {
        let index = self.values.len();
        self.values.push(T::Blob::default());
        value.write_to_blob(&self.values[index]);
        index as u32
    }

    pub fn get(&self, index: u32) -> T {
        T::read_from_blob(&self.values[index as usize])
    }

    pub fn set(&self, index: u32, value: T) {
        value.write_to_blob(&self.values[index as usize]);
    }

    pub fn reserve(&mut self, capacity: usize, device: &RenderDevice) {
        let size = size_of::<T::Blob>() * capacity;
        if capacity > self.capacity || (self.changed && size > 0) {
            self.capacity = capacity;
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: make_buffer_label::<Self>(&self.label),
                size: size as BufferAddress,
                usage: BufferUsages::COPY_DST | self.buffer_usage,
                mapped_at_creation: false,
            }));
            self.changed = false;
        }
    }

    pub fn write_buffer(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        self.write_buffer_range(0..self.values.len(), device, queue);
    }

    pub fn write_buffer_range(
        &mut self,
        range: Range<usize>,
        device: &RenderDevice,
        queue: &RenderQueue,
    ) {
        assert!(
            range.start <= range.end
                && range.start <= self.values.len()
                && range.end <= self.values.len()
        );
        if range.start == range.end {
            return;
        }
        self.reserve(range.end, device);

        let Some(buffer) = &self.buffer else { return };

        unsafe {
            let bytes: &[u8] = slice::from_raw_parts(
                self.values.as_ptr().add(range.start).cast::<u8>(),
                (range.end - range.start) * size_of::<T::Blob>(),
            );
            let start_offset = range.start as u64 * size_of::<T::Blob>() as u64;
            queue.write_buffer(buffer, start_offset, bytes);
        }
    }

    #[inline]
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    pub fn grow(&mut self, new_len: u32) {
        if self.len() < new_len {
            self.values.resize_with(new_len as usize, T::Blob::default);
        }
    }

    pub fn truncate(&mut self, len: u32) {
        self.values.truncate(len as usize);
    }
}

pub struct BufferVec<T>
where
    T: ShaderType + WriteInto,
{
    data: Vec<u8>,
    buffer: Option<Buffer>,
    capacity: usize,
    buffer_usage: BufferUsages,
    label: Option<String>,
    label_changed: bool,
    phantom: PhantomData<T>,
}

impl<T> BufferVec<T>
where
    T: ShaderType + WriteInto,
{

    pub const fn new(buffer_usage: BufferUsages) -> Self {
        Self {
            data: vec![],
            buffer: None,
            capacity: 0,
            buffer_usage,
            label: None,
            label_changed: false,
            phantom: PhantomData,
        }
    }

    #[inline]
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    #[inline]
    pub fn binding(&self) -> Option<BindingResource<'_>> {
        Some(BindingResource::Buffer(
            self.buffer()?.as_entire_buffer_binding(),
        ))
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len() / u64::from(T::min_size()) as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn push(&mut self, value: T) -> usize {
        let element_size = u64::from(T::min_size()) as usize;
        let offset = self.data.len();

        self.data.extend(iter::repeat_n(0, element_size));

        let mut dest = &mut self.data[offset..(offset + element_size)];
        value.write_into(&mut Writer::new(&value, &mut dest, 0).unwrap());

        offset / u64::from(T::min_size()) as usize
    }

    pub fn set_label(&mut self, label: Option<&str>) {
        let label = label.map(str::to_string);

        if label != self.label {
            self.label_changed = true;
        }

        self.label = label;
    }

    pub fn get_label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn reserve_internal(&mut self, count: usize) {
        self.data.reserve(count * u64::from(T::min_size()) as usize);
    }

    pub fn reserve(&mut self, capacity: usize, device: &RenderDevice) {
        if capacity <= self.capacity && !self.label_changed {
            return;
        }

        self.capacity = capacity;
        let size = u64::from(T::min_size()) as usize * capacity;
        self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: make_buffer_label::<Self>(&self.label),
            size: size as BufferAddress,
            usage: BufferUsages::COPY_DST | self.buffer_usage,
            mapped_at_creation: false,
        }));
        self.label_changed = false;
    }

    pub fn write_buffer(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        if self.data.is_empty() {
            return;
        }

        self.reserve(self.data.len() / u64::from(T::min_size()) as usize, device);

        let Some(buffer) = &self.buffer else { return };
        queue.write_buffer(buffer, 0, &self.data);
    }

    pub fn write_buffer_range(
        &mut self,
        render_queue: &RenderQueue,
        range: Range<usize>,
    ) -> Result<(), WriteBufferRangeError> {
        if self.data.is_empty() {
            return Err(WriteBufferRangeError::NoValuesToUpload);
        }
        let item_size = u64::from(T::min_size()) as usize;
        if range.end > item_size * self.capacity {
            return Err(WriteBufferRangeError::RangeBiggerThanBuffer);
        }
        if let Some(buffer) = &self.buffer {
            let bytes = &self.data[range.start..range.end];
            render_queue.write_buffer(buffer, (range.start * item_size) as u64, bytes);
            Ok(())
        } else {
            Err(WriteBufferRangeError::BufferNotInitialized)
        }
    }

    pub fn truncate(&mut self, len: usize) {
        self.data.truncate(u64::from(T::min_size()) as usize * len);
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }
}

pub struct UninitBufferVec<T>
where
    T: GpuArrayBufferable,
{
    buffer: Option<Buffer>,
    len: usize,
    capacity: usize,
    item_size: usize,
    buffer_usage: BufferUsages,
    label: Option<String>,
    label_changed: bool,
    phantom: PhantomData<T>,
}

impl<T> UninitBufferVec<T>
where
    T: GpuArrayBufferable,
{

    pub const fn new(buffer_usage: BufferUsages) -> Self {
        Self {
            len: 0,
            buffer: None,
            capacity: 0,
            item_size: size_of::<T>(),
            buffer_usage,
            label: None,
            label_changed: false,
            phantom: PhantomData,
        }
    }

    #[inline]
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    #[inline]
    pub fn binding(&self) -> Option<BindingResource<'_>> {
        Some(BindingResource::Buffer(
            self.buffer()?.as_entire_buffer_binding(),
        ))
    }

    pub fn add(&mut self) -> usize {
        self.add_multiple(1)
    }

    pub fn add_multiple(&mut self, count: usize) -> usize {
        let index = self.len;
        self.len += count;
        index
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn set_label(&mut self, label: Option<&str>) {
        let label = label.map(str::to_string);

        if label != self.label {
            self.label_changed = true;
        }

        self.label = label;
    }

    pub fn get_label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    pub fn reserve(&mut self, capacity: usize, device: &RenderDevice) {
        if capacity <= self.capacity && !self.label_changed {
            return;
        }

        self.capacity = capacity;
        let size = self.item_size * capacity;
        self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
            label: make_buffer_label::<Self>(&self.label),
            size: size as BufferAddress,
            usage: BufferUsages::COPY_DST | self.buffer_usage,
            mapped_at_creation: false,
        }));

        self.label_changed = false;
    }

    pub fn write_buffer(&mut self, device: &RenderDevice) {
        if !self.is_empty() {
            self.reserve(self.len, device);
        }
    }
}

pub struct PartialBufferVec<T>
where
    T: NoUninit,
{

    values: Vec<T>,

    buffer: Option<Buffer>,

    capacity: usize,

    uninit_element_count: usize,

    buffer_usages: BufferUsages,

    label: String,
}

impl<T> PartialBufferVec<T>
where
    T: NoUninit,
{

    pub fn new(buffer_usages: BufferUsages, label: String) -> PartialBufferVec<T> {
        PartialBufferVec {
            values: vec![],
            buffer: None,
            capacity: 0,
            uninit_element_count: 0,
            buffer_usages: buffer_usages | BufferUsages::COPY_DST,
            label,
        }
    }

    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    pub fn clear(&mut self) {
        self.values.clear();
        self.uninit_element_count = 0;
    }

    fn reserve(&mut self, capacity: usize, render_device: &RenderDevice) {
        if capacity <= self.capacity {
            return;
        }

        let size = size_of::<T>() * capacity;
        self.capacity = capacity;
        self.buffer = Some(render_device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&self.label),
            size: size as u64,
            usage: self.buffer_usages,
            mapped_at_creation: false,
        }));
    }

    pub fn write_buffer(&mut self, render_device: &RenderDevice, render_queue: &RenderQueue) {
        if self.is_empty() {
            return;
        }

        self.reserve(self.len(), render_device);

        let Some(ref buffer) = self.buffer else {
            return;
        };
        render_queue.write_buffer(buffer, 0, must_cast_slice(&self.values[..]));
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.values.len() + self.uninit_element_count
    }

    pub fn push_init(&mut self, value: T) -> usize {
        debug_assert_eq!(self.uninit_element_count, 0);
        let index = self.values.len();
        self.values.push(value);
        index
    }

    pub fn push_multiple_uninit(&mut self, count: usize) -> usize {
        let first_index = self.values.len() + self.uninit_element_count;
        self.uninit_element_count += count;
        first_index
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone, Error)]
pub enum WriteBufferRangeError {
    #[error("the range is bigger than the capacity of the buffer")]
    RangeBiggerThanBuffer,
    #[error("the gpu buffer is not initialized")]
    BufferNotInitialized,
    #[error("there are no values to upload")]
    NoValuesToUpload,
}

#[inline]
#[cfg_attr(
    not(feature = "type_label_buffers"),
    expect(
        clippy::extra_unused_type_parameters,
        reason = "conditional compilation"
    )
)]
pub(crate) fn make_buffer_label<'a, T>(label: &'a Option<String>) -> Option<&'a str> {
    #[cfg(feature = "type_label_buffers")]
    if label.is_none() {
        return Some(core::any::type_name::<T>());
    }
    label.as_deref()
}
