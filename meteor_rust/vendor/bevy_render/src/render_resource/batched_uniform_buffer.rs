use super::{GpuArrayBufferIndex, GpuArrayBufferable};
use crate::{
    render_resource::DynamicUniformBuffer,
    renderer::{RenderDevice, RenderQueue},
};
use core::{marker::PhantomData, num::NonZero};
use encase::{
    private::{ArrayMetadata, BufferMut, Metadata, RuntimeSizedArray, WriteInto, Writer},
    ShaderType,
};
use nonmax::NonMaxU32;
use wgpu::{BindingResource, Limits};

#[cfg(any(
    not(feature = "webgl"),
    not(target_arch = "wasm32"),
    feature = "webgpu"
))]
const MAX_REASONABLE_UNIFORM_BUFFER_BINDING_SIZE: u64 = 1 << 20;

#[cfg(all(feature = "webgl", target_arch = "wasm32", not(feature = "webgpu")))]
const MAX_REASONABLE_UNIFORM_BUFFER_BINDING_SIZE: u64 = 1 << 12;

pub struct BatchedUniformBuffer<T: GpuArrayBufferable> {

    uniforms: DynamicUniformBuffer<MaxCapacityArray<Vec<T>>>,

    temp: MaxCapacityArray<Vec<T>>,
    current_offset: u32,
    dynamic_offset_alignment: u32,
}

impl<T: GpuArrayBufferable> BatchedUniformBuffer<T> {
    pub fn batch_size(limits: &Limits) -> usize {
        (limits
            .max_uniform_buffer_binding_size
            .min(MAX_REASONABLE_UNIFORM_BUFFER_BINDING_SIZE)
            / T::min_size().get()) as usize
    }

    pub fn new(limits: &Limits) -> Self {
        let capacity = Self::batch_size(limits);
        let alignment = limits.min_uniform_buffer_offset_alignment;

        Self {
            uniforms: DynamicUniformBuffer::new_with_alignment(alignment as u64),
            temp: MaxCapacityArray(Vec::with_capacity(capacity), capacity),
            current_offset: 0,
            dynamic_offset_alignment: alignment,
        }
    }

    #[inline]
    pub fn size(&self) -> NonZero<u64> {
        self.temp.size()
    }

    pub fn clear(&mut self) {
        self.uniforms.clear();
        self.current_offset = 0;
        self.temp.0.clear();
    }

    pub fn push(&mut self, component: T) -> GpuArrayBufferIndex<T> {
        let result = GpuArrayBufferIndex {
            index: self.temp.0.len() as u32,
            dynamic_offset: NonMaxU32::new(self.current_offset),
            element_type: PhantomData,
        };
        self.temp.0.push(component);
        if self.temp.0.len() == self.temp.1 {
            self.flush();
        }
        result
    }

    pub fn flush(&mut self) {
        self.uniforms.push(&self.temp);

        self.current_offset +=
            align_to_next(self.temp.size().get(), self.dynamic_offset_alignment as u64) as u32;

        self.temp.0.clear();
    }

    pub fn write_buffer(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        if !self.temp.0.is_empty() {
            self.flush();
        }
        self.uniforms.write_buffer(device, queue);
    }

    #[inline]
    pub fn binding(&self) -> Option<BindingResource<'_>> {
        let mut binding = self.uniforms.binding();
        if let Some(BindingResource::Buffer(binding)) = &mut binding {

            binding.size = Some(self.size());
        }
        binding
    }
}

#[inline]
fn align_to_next(value: u64, alignment: u64) -> u64 {
    debug_assert!(alignment.is_power_of_two());
    ((value - 1) | (alignment - 1)) + 1
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct MaxCapacityArray<T>(T, usize);

impl<T> ShaderType for MaxCapacityArray<T>
where
    T: ShaderType<ExtraMetadata = ArrayMetadata>,
{
    type ExtraMetadata = ArrayMetadata;

    const METADATA: Metadata<Self::ExtraMetadata> = T::METADATA;

    fn size(&self) -> NonZero<u64> {
        Self::METADATA.stride().mul(self.1.max(1) as u64).0
    }
}

impl<T> WriteInto for MaxCapacityArray<T>
where
    T: WriteInto + RuntimeSizedArray,
{
    fn write_into<B: BufferMut>(&self, writer: &mut Writer<B>) {
        debug_assert!(self.0.len() <= self.1);
        self.0.write_into(writer);
    }
}
