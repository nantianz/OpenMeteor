

use bevy_app::{App, Plugin};
use bevy_asset::{embedded_asset, Assets, Handle};
use bevy_core_pipeline::{
    core_3d::prepare_core_3d_depth_textures,
    schedule::{Core3d, Core3dSystems},
};
use bevy_ecs::{resource::Resource, schedule::IntoScheduleConfigs as _};
use bevy_light::FogVolume;
use bevy_math::{
    primitives::{Cuboid, Plane3d},
    Vec2, Vec3,
};
use bevy_mesh::{Mesh, Meshable};
use bevy_render::{
    render_resource::SpecializedRenderPipelines,
    sync_component::{SyncComponent, SyncComponentPlugin},
    ExtractSchedule, GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems,
};
use render::{volumetric_fog, VolumetricFogPipeline, VolumetricFogUniformBuffer};

use crate::{volumetric_fog::render::init_volumetric_fog_pipeline, MeshPipelineSystems};

pub mod render;

pub struct VolumetricFogPlugin;

#[derive(Resource)]
pub struct FogAssets {
    plane_mesh: Handle<Mesh>,
    cube_mesh: Handle<Mesh>,
}

impl Plugin for VolumetricFogPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "volumetric_fog.wgsl");

        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        let plane_mesh = meshes.add(Plane3d::new(Vec3::Z, Vec2::ONE).mesh());
        let cube_mesh = meshes.add(Cuboid::new(1.0, 1.0, 1.0).mesh());

        app.add_plugins(SyncComponentPlugin::<FogVolume, Self>::default());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .insert_resource(FogAssets {
                plane_mesh,
                cube_mesh,
            })
            .init_gpu_resource::<SpecializedRenderPipelines<VolumetricFogPipeline>>()
            .init_gpu_resource::<VolumetricFogUniformBuffer>()
            .add_systems(
                RenderStartup,
                init_volumetric_fog_pipeline.after(MeshPipelineSystems),
            )
            .add_systems(ExtractSchedule, render::extract_volumetric_fog)
            .add_systems(
                Render,
                (
                    render::prepare_volumetric_fog_pipelines.in_set(RenderSystems::Prepare),
                    render::prepare_volumetric_fog_uniforms.in_set(RenderSystems::Prepare),
                    render::prepare_view_depth_textures_for_volumetric_fog
                        .in_set(RenderSystems::Prepare)
                        .before(prepare_core_3d_depth_textures),
                ),
            )
            .add_systems(
                Core3d,
                volumetric_fog
                    .after(Core3dSystems::MainPass)
                    .before(Core3dSystems::EarlyPostProcess),
            );
    }
}

impl SyncComponent<VolumetricFogPlugin> for FogVolume {
    type Target = Self;
}
