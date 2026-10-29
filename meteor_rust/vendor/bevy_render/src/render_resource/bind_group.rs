use crate::{
    render_asset::RenderAssets,
    render_resource::{BindGroupLayout, Buffer, PipelineCache, Sampler, TextureView},
    renderer::{RenderDevice, WgpuWrapper},
    texture::GpuImage,
};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::system::{SystemParam, SystemParamItem};
use bevy_material::descriptor::BindGroupLayoutDescriptor;
pub use bevy_render_macros::AsBindGroup;
use bevy_utils::define_atomic_id;
use core::ops::Deref;
use encase::ShaderType;
use thiserror::Error;
use wgpu::{
    BindGroupEntry, BindGroupLayoutEntry, BindingResource, SamplerBindingType, TextureViewDimension,
};

use super::{BindlessDescriptor, BindlessSlabResourceLimit};

define_atomic_id!(BindGroupId);

#[derive(Clone, Debug)]
pub struct BindGroup {
    id: BindGroupId,
    value: WgpuWrapper<wgpu::BindGroup>,
}

impl BindGroup {

    #[inline]
    pub fn id(&self) -> BindGroupId {
        self.id
    }
}

impl PartialEq for BindGroup {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for BindGroup {}

impl core::hash::Hash for BindGroup {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.id.0.hash(state);
    }
}

impl From<wgpu::BindGroup> for BindGroup {
    fn from(value: wgpu::BindGroup) -> Self {
        BindGroup {
            id: BindGroupId::new(),
            value: WgpuWrapper::new(value),
        }
    }
}

impl<'a> From<&'a BindGroup> for Option<&'a wgpu::BindGroup> {
    fn from(value: &'a BindGroup) -> Self {
        Some(value.deref())
    }
}

impl<'a> From<&'a mut BindGroup> for Option<&'a wgpu::BindGroup> {
    fn from(value: &'a mut BindGroup) -> Self {
        Some(&*value)
    }
}

impl Deref for BindGroup {
    type Target = wgpu::BindGroup;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

pub trait AsBindGroup {

    type Data: Send + Sync;

    type Param: SystemParam + 'static;

    fn bindless_slot_count() -> Option<BindlessSlabResourceLimit> {
        None
    }

    fn bindless_supported(_: &RenderDevice) -> bool {
        true
    }

    fn label() -> &'static str;

    fn as_bind_group(
        &self,
        layout_descriptor: &BindGroupLayoutDescriptor,
        render_device: &RenderDevice,
        pipeline_cache: &PipelineCache,
        param: &mut SystemParamItem<'_, '_, Self::Param>,
    ) -> Result<PreparedBindGroup, AsBindGroupError> {
        let layout = &pipeline_cache.get_bind_group_layout(layout_descriptor);

        let UnpreparedBindGroup { bindings } =
            Self::unprepared_bind_group(self, layout, render_device, param, false)?;

        let entries = bindings
            .iter()
            .map(|(index, binding)| BindGroupEntry {
                binding: *index,
                resource: binding.get_binding(),
            })
            .collect::<Vec<_>>();

        let bind_group = render_device.create_bind_group(Self::label(), layout, &entries);

        Ok(PreparedBindGroup {
            bindings,
            bind_group,
        })
    }

    fn bind_group_data(&self) -> Self::Data;

    fn unprepared_bind_group(
        &self,
        layout: &BindGroupLayout,
        render_device: &RenderDevice,
        param: &mut SystemParamItem<'_, '_, Self::Param>,
        force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError>;

    fn bind_group_layout(render_device: &RenderDevice) -> BindGroupLayout
    where
        Self: Sized,
    {
        render_device.create_bind_group_layout(
            Self::label(),
            &Self::bind_group_layout_entries(render_device, false),
        )
    }

    fn bind_group_layout_descriptor(render_device: &RenderDevice) -> BindGroupLayoutDescriptor
    where
        Self: Sized,
    {
        BindGroupLayoutDescriptor {
            label: Self::label().into(),
            entries: Self::bind_group_layout_entries(render_device, false),
        }
    }

    fn bind_group_layout_entries(
        render_device: &RenderDevice,
        force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry>
    where
        Self: Sized;

    fn bindless_descriptor() -> Option<BindlessDescriptor> {
        None
    }
}

#[derive(Debug, Error)]
pub enum AsBindGroupError {

    #[error("The bind group could not be generated")]
    RetryNextUpdate,
    #[error("Create the bind group via `as_bind_group()` instead")]
    CreateBindGroupDirectly,
    #[error("At binding index {0}, the provided image sampler `{1}` does not match the required sampler type(s) `{2}`.")]
    InvalidSamplerType(u32, String, String),
}

pub struct PreparedBindGroup {
    pub bindings: BindingResources,
    pub bind_group: BindGroup,
}

pub struct UnpreparedBindGroup {
    pub bindings: BindingResources,
}

#[derive(Deref, DerefMut)]
pub struct BindingResources(pub Vec<(u32, OwnedBindingResource)>);

#[derive(Debug)]
pub enum OwnedBindingResource {
    Buffer(Buffer),
    TextureView(TextureViewDimension, TextureView),
    Sampler(SamplerBindingType, Sampler),
    Data(OwnedData),
}

#[derive(Debug, Deref, DerefMut)]
pub struct OwnedData(pub Vec<u8>);

impl OwnedBindingResource {

    pub fn get_binding(&self) -> BindingResource<'_> {
        match self {
            OwnedBindingResource::Buffer(buffer) => buffer.as_entire_binding(),
            OwnedBindingResource::TextureView(_, view) => BindingResource::TextureView(view),
            OwnedBindingResource::Sampler(_, sampler) => BindingResource::Sampler(sampler),
            OwnedBindingResource::Data(_) => panic!("`OwnedData` has no binding resource"),
        }
    }
}

pub trait AsBindGroupShaderType<T: ShaderType> {

    fn as_bind_group_shader_type(&self, images: &RenderAssets<GpuImage>) -> T;
}

impl<T, U: ShaderType> AsBindGroupShaderType<U> for T
where
    for<'a> &'a T: Into<U>,
{
    #[inline]
    fn as_bind_group_shader_type(&self, _images: &RenderAssets<GpuImage>) -> U {
        self.into()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use bevy_asset::Handle;
    use bevy_image::Image;

    #[test]
    fn texture_visibility() {
        #[expect(
            dead_code,
            reason = "This is a derive macro compilation test. It will not be constructed."
        )]
        #[derive(AsBindGroup)]
        pub struct TextureVisibilityTest {
            #[texture(0, visibility(all))]
            pub all: Handle<Image>,
            #[texture(1, visibility(none))]
            pub none: Handle<Image>,
            #[texture(2, visibility(fragment))]
            pub fragment: Handle<Image>,
            #[texture(3, visibility(vertex))]
            pub vertex: Handle<Image>,
            #[texture(4, visibility(compute))]
            pub compute: Handle<Image>,
            #[texture(5, visibility(vertex, fragment))]
            pub vertex_fragment: Handle<Image>,
            #[texture(6, visibility(vertex, compute))]
            pub vertex_compute: Handle<Image>,
            #[texture(7, visibility(fragment, compute))]
            pub fragment_compute: Handle<Image>,
            #[texture(8, visibility(vertex, fragment, compute))]
            pub vertex_fragment_compute: Handle<Image>,
        }
    }
}
