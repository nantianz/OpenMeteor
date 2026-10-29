use core::{marker::PhantomData, num::NonZero};

use crate::{
    render_resource::{make_buffer_label, Buffer},
    renderer::{RenderDevice, RenderQueue},
};
use encase::{
    internal::{AlignmentValue, BufferMut, WriteInto},
    DynamicUniformBuffer as DynamicUniformBufferWrapper, ShaderType,
    UniformBuffer as UniformBufferWrapper,
};
use wgpu::{
    util::BufferInitDescriptor, BindingResource, BufferBinding, BufferDescriptor, BufferUsages,
};

use super::IntoBinding;

pub struct UniformBuffer<T: ShaderType> {
    value: T,
    scratch: UniformBufferWrapper<Vec<u8>>,
    buffer: Option<Buffer>,
    label: Option<String>,
    changed: bool,
    buffer_usage: BufferUsages,
}

impl<T: ShaderType> From<T> for UniformBuffer<T> {
    fn from(value: T) -> Self {
        Self {
            value,
            scratch: UniformBufferWrapper::new(Vec::new()),
            buffer: None,
            label: None,
            changed: false,
            buffer_usage: BufferUsages::COPY_DST | BufferUsages::UNIFORM,
        }
    }
}

impl<T: ShaderType + Default> Default for UniformBuffer<T> {
    fn default() -> Self {
        Self {
            value: T::default(),
            scratch: UniformBufferWrapper::new(Vec::new()),
            buffer: None,
            label: None,
            changed: false,
            buffer_usage: BufferUsages::COPY_DST | BufferUsages::UNIFORM,
        }
    }
}

impl<T: ShaderType + WriteInto> UniformBuffer<T> {
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

    pub fn set(&mut self, value: T) {
        self.value = value;
    }

    pub fn get(&self) -> &T {
        &self.value
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
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

    pub fn add_usages(&mut self, usage: BufferUsages) {
        self.buffer_usage |= usage;
        self.changed = true;
    }

    pub fn write_buffer(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        self.scratch.write(&self.value).unwrap();

        if self.changed || self.buffer.is_none() {
            self.buffer = Some(device.create_buffer_with_data(&BufferInitDescriptor {
                label: make_buffer_label::<Self>(&self.label),
                usage: self.buffer_usage,
                contents: self.scratch.as_ref(),
            }));
            self.changed = false;
        } else if let Some(buffer) = &self.buffer {
            queue.write_buffer(buffer, 0, self.scratch.as_ref());
        }
    }
}

impl<'a, T: ShaderType + WriteInto> IntoBinding<'a> for &'a UniformBuffer<T> {
    #[inline]
    fn into_binding(self) -> BindingResource<'a> {
        self.buffer()
            .expect("Failed to get buffer")
            .as_entire_buffer_binding()
            .into_binding()
    }
}

pub struct DynamicUniformBuffer<T: ShaderType> {
    scratch: DynamicUniformBufferWrapper<Vec<u8>>,
    buffer: Option<Buffer>,
    label: Option<String>,
    changed: bool,
    buffer_usage: BufferUsages,
    _marker: PhantomData<fn() -> T>,
}

impl<T: ShaderType> Default for DynamicUniformBuffer<T> {
    fn default() -> Self {
        Self {
            scratch: DynamicUniformBufferWrapper::new(Vec::new()),
            buffer: None,
            label: None,
            changed: false,
            buffer_usage: BufferUsages::COPY_DST | BufferUsages::UNIFORM,
            _marker: PhantomData,
        }
    }
}

impl<T: ShaderType + WriteInto> DynamicUniformBuffer<T> {
    pub fn new_with_alignment(alignment: u64) -> Self {
        Self {
            scratch: DynamicUniformBufferWrapper::new_with_alignment(Vec::new(), alignment),
            buffer: None,
            label: None,
            changed: false,
            buffer_usage: BufferUsages::COPY_DST | BufferUsages::UNIFORM,
            _marker: PhantomData,
        }
    }

    #[inline]
    pub fn buffer(&self) -> Option<&Buffer> {
        self.buffer.as_ref()
    }

    #[inline]
    pub fn binding(&self) -> Option<BindingResource<'_>> {
        Some(BindingResource::Buffer(BufferBinding {
            buffer: self.buffer()?,
            offset: 0,
            size: Some(T::min_size()),
        }))
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.scratch.as_ref().is_empty()
    }

    #[inline]
    pub fn push(&mut self, value: &T) -> u32 {
        self.scratch.write(value).unwrap() as u32
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

    pub fn add_usages(&mut self, usage: BufferUsages) {
        self.buffer_usage |= usage;
        self.changed = true;
    }

    #[inline]
    pub fn get_writer<'a>(
        &'a mut self,
        max_count: usize,
        device: &RenderDevice,
        queue: &'a RenderQueue,
    ) -> Option<DynamicUniformBufferWriter<T>> {
        let alignment = if cfg!(target_abi = "sim") {

            AlignmentValue::new(256)
        } else {
            AlignmentValue::new(device.limits().min_uniform_buffer_offset_alignment as u64)
        };

        let mut capacity = self.buffer.as_deref().map(wgpu::Buffer::size).unwrap_or(0);
        let size = alignment
            .round_up(T::min_size().get())
            .checked_mul(max_count as u64)
            .unwrap();

        if capacity < size || (self.changed && size > 0) {
            let buffer = device.create_buffer(&BufferDescriptor {
                label: make_buffer_label::<Self>(&self.label),
                usage: self.buffer_usage,
                size,
                mapped_at_creation: false,
            });
            capacity = buffer.size();
            self.buffer = Some(buffer);
            self.changed = false;
        }

        if let Some(buffer) = self.buffer.as_deref() {
            let buffer_view =
                queue.write_buffer_with(buffer, 0, NonZero::<u64>::new(buffer.size())?)?;
            Some(DynamicUniformBufferWriter {
                buffer: encase::DynamicUniformBuffer::new_with_alignment(
                    QueueWriteBufferViewWrapper {
                        capacity: capacity as usize,
                        buffer_view,
                    },
                    alignment.get(),
                ),
                _marker: PhantomData,
            })
        } else {
            None
        }
    }

    #[inline]
    pub fn write_buffer(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        let capacity = self.buffer.as_deref().map(wgpu::Buffer::size).unwrap_or(0);
        let size = self.scratch.as_ref().len() as u64;

        if capacity < size || (self.changed && size > 0) {
            self.buffer = Some(device.create_buffer_with_data(&BufferInitDescriptor {
                label: self.label.as_deref(),
                usage: self.buffer_usage,
                contents: self.scratch.as_ref(),
            }));
            self.changed = false;
        } else if let Some(buffer) = &self.buffer {
            queue.write_buffer(buffer, 0, self.scratch.as_ref());
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.scratch.as_mut().clear();
        self.scratch.set_offset(0);
    }
}

pub struct DynamicUniformBufferWriter<T> {
    buffer: encase::DynamicUniformBuffer<QueueWriteBufferViewWrapper>,
    _marker: PhantomData<fn() -> T>,
}

impl<T: ShaderType + WriteInto> DynamicUniformBufferWriter<T> {
    pub fn write(&mut self, value: &T) -> u32 {
        self.buffer.write(value).unwrap() as u32
    }
}

struct QueueWriteBufferViewWrapper {
    buffer_view: wgpu::QueueWriteBufferView,

    capacity: usize,
}

impl BufferMut for QueueWriteBufferViewWrapper {
    #[inline]
    fn capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    fn write<const N: usize>(&mut self, offset: usize, val: &[u8; N]) {
        self.buffer_view
            .slice(offset..offset + val.len())
            .copy_from_slice(val);
    }

    #[inline]
    fn write_slice(&mut self, offset: usize, val: &[u8]) {
        self.buffer_view
            .slice(offset..offset + val.len())
            .copy_from_slice(val);
    }
}

impl<'a, T: ShaderType + WriteInto> IntoBinding<'a> for &'a DynamicUniformBuffer<T> {
    #[inline]
    fn into_binding(self) -> BindingResource<'a> {
        self.binding().unwrap()
    }
}
