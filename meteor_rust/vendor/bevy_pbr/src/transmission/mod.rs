mod node;
mod phase;
mod texture;

use bevy_app::{App, Plugin};
use bevy_camera::Camera3d;
use bevy_core_pipeline::{
    core_3d::{main_opaque_pass_3d, main_transparent_pass_3d},
    schedule::{Core3d, Core3dSystems},
};
use bevy_ecs::{prelude::*, schedule::IntoScheduleConfigs};
use bevy_reflect::prelude::*;
use bevy_render::{
    extract_component::{ExtractComponent, ExtractComponentPlugin},
    render_phase::{sort_phase_system, AddRenderCommand, DrawFunctions, ViewSortedRenderPhases},
    ExtractSchedule, Render, RenderApp, RenderSystems,
};
use bevy_shader::load_shader_library;
pub use node::main_transmissive_pass_3d;
pub use phase::Transmissive3d;
pub use texture::ViewTransmissionTexture;

use texture::prepare_core_3d_transmission_textures;

use crate::{DrawMaterial, MeshPipelineKey};

pub struct ScreenSpaceTransmissionPlugin;

impl Plugin for ScreenSpaceTransmissionPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "transmission.wgsl");

        app.add_plugins(ExtractComponentPlugin::<ScreenSpaceTransmission>::default())
            .register_required_components::<Camera3d, ScreenSpaceTransmission>();

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_resource::<DrawFunctions<Transmissive3d>>()
            .init_resource::<ViewSortedRenderPhases<Transmissive3d>>()
            .add_render_command::<Transmissive3d, DrawMaterial>()
            .add_systems(
                Render,
                sort_phase_system::<Transmissive3d>.in_set(RenderSystems::PhaseSort),
            )
            .add_systems(ExtractSchedule, phase::extract_transmissive_camera_phases)
            .add_systems(
                Render,
                prepare_core_3d_transmission_textures.in_set(RenderSystems::PrepareResources),
            )
            .add_systems(
                Core3d,
                main_transmissive_pass_3d
                    .after(main_opaque_pass_3d)
                    .before(main_transparent_pass_3d)
                    .in_set(Core3dSystems::MainPass),
            );
    }
}

#[derive(Component, Reflect, Clone, ExtractComponent)]
#[reflect(Component, Default, Clone)]
pub struct ScreenSpaceTransmission {

    pub steps: usize,

    pub quality: ScreenSpaceTransmissionQuality,
}

impl Default for ScreenSpaceTransmission {
    fn default() -> Self {
        Self {
            steps: 1,
            quality: Default::default(),
        }
    }
}

#[derive(Default, Clone, Copy, Reflect, PartialEq, PartialOrd, Debug)]
#[reflect(Default, Clone, Debug, PartialEq)]
pub enum ScreenSpaceTransmissionQuality {

    Low,

    #[default]
    Medium,

    High,

    Ultra,
}

impl ScreenSpaceTransmissionQuality {
    pub const fn pipeline_key(self) -> MeshPipelineKey {
        match self {
            ScreenSpaceTransmissionQuality::Low => {
                MeshPipelineKey::SCREEN_SPACE_SPECULAR_TRANSMISSION_LOW
            }
            ScreenSpaceTransmissionQuality::Medium => {
                MeshPipelineKey::SCREEN_SPACE_SPECULAR_TRANSMISSION_MEDIUM
            }
            ScreenSpaceTransmissionQuality::High => {
                MeshPipelineKey::SCREEN_SPACE_SPECULAR_TRANSMISSION_HIGH
            }
            ScreenSpaceTransmissionQuality::Ultra => {
                MeshPipelineKey::SCREEN_SPACE_SPECULAR_TRANSMISSION_ULTRA
            }
        }
    }
}
