

use bevy_app::{App, Plugin};
use bevy_ecs::{component::Component, entity::Entity, prelude::ReflectComponent};
use bevy_reflect::{prelude::ReflectDefault, Reflect};
use bevy_shader::load_shader_library;

use crate::{extract_component::ExtractComponent, render_resource::TextureView};

pub struct OcclusionCullingPlugin;

impl Plugin for OcclusionCullingPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "mesh_preprocess_types.wgsl");
    }
}

#[derive(Component, ExtractComponent, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default, Clone)]
pub struct OcclusionCulling;

#[derive(Clone, Component)]
pub struct OcclusionCullingSubview {

    pub depth_texture_view: TextureView,

    pub depth_texture_size: u32,
}

#[derive(Clone, Component)]
pub struct OcclusionCullingSubviewEntities(pub Vec<Entity>);
