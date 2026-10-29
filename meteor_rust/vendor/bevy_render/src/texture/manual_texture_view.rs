use bevy_camera::ManualTextureViewHandle;
use bevy_ecs::resource::Resource;
use bevy_math::UVec2;
use bevy_platform::collections::HashMap;
use bevy_render_macros::ExtractResource;
use wgpu::TextureFormat;

use crate::render_resource::TextureView;

#[derive(Debug, Clone)]
pub struct ManualTextureView {
    pub texture_view: TextureView,
    pub size: UVec2,
    pub view_format: TextureFormat,
}

impl ManualTextureView {
    pub fn with_default_format(texture_view: TextureView, size: UVec2) -> Self {
        Self {
            texture_view,
            size,
            view_format: TextureFormat::Rgba8UnormSrgb,
        }
    }
}

#[derive(Default, Clone, Resource, ExtractResource)]
pub struct ManualTextureViews(HashMap<ManualTextureViewHandle, ManualTextureView>);

impl core::ops::Deref for ManualTextureViews {
    type Target = HashMap<ManualTextureViewHandle, ManualTextureView>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl core::ops::DerefMut for ManualTextureViews {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
