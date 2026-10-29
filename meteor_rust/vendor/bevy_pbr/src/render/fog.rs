use bevy_app::{App, Plugin};
use bevy_color::{ColorToComponents, LinearRgba};
use bevy_ecs::prelude::*;
use bevy_math::{Vec3, Vec4};
use bevy_render::{
    extract_component::ExtractComponentPlugin,
    render_resource::{DynamicUniformBuffer, ShaderType},
    renderer::{RenderDevice, RenderQueue},
    view::ExtractedView,
    GpuResourceAppExt, Render, RenderApp, RenderSystems,
};
use bevy_shader::load_shader_library;

use crate::{DistanceFog, FogFalloff};

#[derive(Copy, Clone, ShaderType, Default, Debug)]
pub struct GpuFog {

    base_color: Vec4,

    directional_light_color: Vec4,

    be: Vec3,

    directional_light_exponent: f32,

    bi: Vec3,

    mode: u32,
}

#[expect(unused, reason = "Kept in sync with `mesh_view_types.wgsl`")]
const GPU_FOG_MODE_OFF: u32 = 0;
const GPU_FOG_MODE_LINEAR: u32 = 1;
const GPU_FOG_MODE_EXPONENTIAL: u32 = 2;
const GPU_FOG_MODE_EXPONENTIAL_SQUARED: u32 = 3;
const GPU_FOG_MODE_ATMOSPHERIC: u32 = 4;

#[derive(Default, Resource)]
pub struct FogMeta {
    pub gpu_fogs: DynamicUniformBuffer<GpuFog>,
}

pub fn prepare_fog(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut fog_meta: ResMut<FogMeta>,
    views: Query<(Entity, &DistanceFog), With<ExtractedView>>,
) {
    let views_iter = views.iter();
    let view_count = views_iter.len();
    let Some(mut writer) = fog_meta
        .gpu_fogs
        .get_writer(view_count, &render_device, &render_queue)
    else {
        return;
    };
    for (entity, fog) in views_iter {
        let gpu_fog = {
            match &fog.falloff {
                FogFalloff::Linear { start, end } => GpuFog {
                    mode: GPU_FOG_MODE_LINEAR,
                    base_color: LinearRgba::from(fog.color).to_vec4(),
                    directional_light_color: LinearRgba::from(fog.directional_light_color)
                        .to_vec4(),
                    directional_light_exponent: fog.directional_light_exponent,
                    be: Vec3::new(*start, *end, 0.0),
                    ..Default::default()
                },
                FogFalloff::Exponential { density } => GpuFog {
                    mode: GPU_FOG_MODE_EXPONENTIAL,
                    base_color: LinearRgba::from(fog.color).to_vec4(),
                    directional_light_color: LinearRgba::from(fog.directional_light_color)
                        .to_vec4(),
                    directional_light_exponent: fog.directional_light_exponent,
                    be: Vec3::new(*density, 0.0, 0.0),
                    ..Default::default()
                },
                FogFalloff::ExponentialSquared { density } => GpuFog {
                    mode: GPU_FOG_MODE_EXPONENTIAL_SQUARED,
                    base_color: LinearRgba::from(fog.color).to_vec4(),
                    directional_light_color: LinearRgba::from(fog.directional_light_color)
                        .to_vec4(),
                    directional_light_exponent: fog.directional_light_exponent,
                    be: Vec3::new(*density, 0.0, 0.0),
                    ..Default::default()
                },
                FogFalloff::Atmospheric {
                    extinction,
                    inscattering,
                } => GpuFog {
                    mode: GPU_FOG_MODE_ATMOSPHERIC,
                    base_color: LinearRgba::from(fog.color).to_vec4(),
                    directional_light_color: LinearRgba::from(fog.directional_light_color)
                        .to_vec4(),
                    directional_light_exponent: fog.directional_light_exponent,
                    be: *extinction,
                    bi: *inscattering,
                },
            }
        };

        commands.entity(entity).insert(ViewFogUniformOffset {
            offset: writer.write(&gpu_fog),
        });
    }
}

#[derive(Component)]
pub struct ViewFogUniformOffset {
    pub offset: u32,
}

pub struct FogPlugin;

impl Plugin for FogPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "fog.wgsl");

        app.add_plugins(ExtractComponentPlugin::<DistanceFog>::default());

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_gpu_resource::<FogMeta>()
                .add_systems(Render, prepare_fog.in_set(RenderSystems::PrepareResources));
        }
    }
}
