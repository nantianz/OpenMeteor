use crate::renderer::{RenderDevice, WgpuWrapper};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    resource::Resource,
    world::{FromWorld, World},
};
use bevy_image::ImageSamplerDescriptor;
use bevy_utils::define_atomic_id;
use core::ops::Deref;

define_atomic_id!(TextureId);

#[derive(Clone, Debug)]
pub struct Texture {
    id: TextureId,
    value: WgpuWrapper<wgpu::Texture>,
}

impl Texture {

    #[inline]
    pub fn id(&self) -> TextureId {
        self.id
    }

    pub fn create_view(&self, desc: &wgpu::TextureViewDescriptor) -> TextureView {
        TextureView::from(self.value.create_view(desc))
    }
}

impl From<wgpu::Texture> for Texture {
    fn from(value: wgpu::Texture) -> Self {
        Texture {
            id: TextureId::new(),
            value: WgpuWrapper::new(value),
        }
    }
}

impl Deref for Texture {
    type Target = wgpu::Texture;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

define_atomic_id!(TextureViewId);

#[derive(Clone, Debug)]
pub struct TextureView {
    id: TextureViewId,
    value: WgpuWrapper<wgpu::TextureView>,
}

pub struct SurfaceTexture {
    value: WgpuWrapper<wgpu::SurfaceTexture>,
}

impl SurfaceTexture {
    pub fn present(self) {
        self.value.into_inner().present();
    }
}

impl TextureView {

    #[inline]
    pub fn id(&self) -> TextureViewId {
        self.id
    }
}

impl From<wgpu::TextureView> for TextureView {
    fn from(value: wgpu::TextureView) -> Self {
        TextureView {
            id: TextureViewId::new(),
            value: WgpuWrapper::new(value),
        }
    }
}

impl From<wgpu::SurfaceTexture> for SurfaceTexture {
    fn from(value: wgpu::SurfaceTexture) -> Self {
        SurfaceTexture {
            value: WgpuWrapper::new(value),
        }
    }
}

impl Deref for TextureView {
    type Target = wgpu::TextureView;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl Deref for SurfaceTexture {
    type Target = wgpu::SurfaceTexture;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

define_atomic_id!(SamplerId);

#[derive(Clone, Debug)]
pub struct Sampler {
    id: SamplerId,
    value: WgpuWrapper<wgpu::Sampler>,
}

impl Sampler {

    #[inline]
    pub fn id(&self) -> SamplerId {
        self.id
    }
}

impl From<wgpu::Sampler> for Sampler {
    fn from(value: wgpu::Sampler) -> Self {
        Sampler {
            id: SamplerId::new(),
            value: WgpuWrapper::new(value),
        }
    }
}

impl Deref for Sampler {
    type Target = wgpu::Sampler;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

#[derive(Resource, Debug, Clone, Deref)]
pub struct DefaultImageSamplerDescriptor(pub ImageSamplerDescriptor);

#[derive(Resource, Debug, Clone, Deref, DerefMut)]
pub struct DefaultImageSampler(pub(crate) Sampler);

impl FromWorld for DefaultImageSampler {
    fn from_world(world: &mut World) -> Self {
        let descriptor = world.resource::<DefaultImageSamplerDescriptor>();
        let wgpu_descriptor = descriptor.as_wgpu();
        let device = world.resource::<RenderDevice>();
        let sampler = device.create_sampler(&wgpu_descriptor);
        Self(sampler)
    }
}
