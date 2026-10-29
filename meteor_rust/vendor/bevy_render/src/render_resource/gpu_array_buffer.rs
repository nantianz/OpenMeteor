use bevy_material::bind_group_layout_entries::{
    binding_types::{storage_buffer_read_only, uniform_buffer_sized},
    BindGroupLayoutEntryBuilder,
};

use super::BufferVec;
use crate::{
    render_resource::batched_uniform_buffer::BatchedUniformBuffer,
    renderer::{RenderDevice, RenderQueue},
};
use bevy_ecs::{prelude::Component, resource::Resource};
use core::marker::PhantomData;
use encase::{private::WriteInto, ShaderSize, ShaderType};
use nonmax::NonMaxU32;
use wgpu::{BindingResource, BufferUsages, Limits};

pub trait GpuArrayBufferable: ShaderType + ShaderSize + WriteInto + Clone {}

impl<T: ShaderType + ShaderSize + WriteInto + Clone> GpuArrayBufferable for T {}

#[derive(Resource)]
pub enum GpuArrayBuffer<T: GpuArrayBufferable> {
    Uniform(BatchedUniformBuffer<T>),
    Storage(BufferVec<T>),
}

impl<T: GpuArrayBufferable> GpuArrayBuffer<T> {
    pub fn new(limits: &Limits) -> Self {
        if limits.max_storage_buffers_per_shader_stage == 0 {
            GpuArrayBuffer::Uniform(BatchedUniformBuffer::new(limits))
        } else {
            GpuArrayBuffer::Storage(BufferVec::new(BufferUsages::STORAGE))
        }
    }

    pub fn clear(&mut self) {
        match self {
            GpuArrayBuffer::Uniform(buffer) => buffer.clear(),
            GpuArrayBuffer::Storage(buffer) => buffer.clear(),
        }
    }

    pub fn push(&mut self, value: T) -> GpuArrayBufferIndex<T> {
        match self {
            GpuArrayBuffer::Uniform(buffer) => buffer.push(value),
            GpuArrayBuffer::Storage(buffer) => {
                let index = buffer.push(value) as u32;
                GpuArrayBufferIndex {
                    index,
                    dynamic_offset: None,
                    element_type: PhantomData,
                }
            }
        }
    }

    pub fn write_buffer(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        match self {
            GpuArrayBuffer::Uniform(buffer) => buffer.write_buffer(device, queue),
            GpuArrayBuffer::Storage(buffer) => buffer.write_buffer(device, queue),
        }
    }

    pub fn binding_layout(limits: &Limits) -> BindGroupLayoutEntryBuilder {
        if limits.max_storage_buffers_per_shader_stage == 0 {
            uniform_buffer_sized(
                true,

                None,
            )
        } else {
            storage_buffer_read_only::<T>(false)
        }
    }

    pub fn binding(&self) -> Option<BindingResource<'_>> {
        match self {
            GpuArrayBuffer::Uniform(buffer) => buffer.binding(),
            GpuArrayBuffer::Storage(buffer) => buffer.binding(),
        }
    }

    pub fn batch_size(limits: &Limits) -> Option<u32> {
        if limits.max_storage_buffers_per_shader_stage == 0 {
            Some(BatchedUniformBuffer::<T>::batch_size(limits) as u32)
        } else {
            None
        }
    }
}

#[derive(Component, Clone)]
pub struct GpuArrayBufferIndex<T: GpuArrayBufferable> {

    pub index: u32,

    pub dynamic_offset: Option<NonMaxU32>,
    pub element_type: PhantomData<T>,
}
