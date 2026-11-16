use crate::app_state::{AppState, Settings};
use crate::plugins::stage_material::{FrameUniforms, GmdlMaterial, GmdlMode};
use bevy::light::cascade::Cascade;
use bevy::light::{
    Cascades, DirectionalLightShadowMap, NotShadowCaster, NotShadowReceiver, SimulationLightSystems,
};
use bevy::prelude::*;

const LIGHT_ILLUMINANCE: f32 = 1.0;

#[derive(Component)]
struct MeteorDynamicSun;

pub struct DynamicShadowsPlugin;

impl Plugin for DynamicShadowsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameUniforms>()
            .add_systems(
                Update,
                (sync_dynamic_sun, sync_gmdl_shadow_flags)
                    .run_if(in_state(AppState::Playing).or_else(in_state(AppState::Paused))),
            )
            .add_systems(
                PostUpdate,
                stabilize_shadow_projection
                    .after(SimulationLightSystems::UpdateDirectionalLightCascades)
                    .before(SimulationLightSystems::UpdateLightFrusta),
            )
            .add_systems(
                OnExit(AppState::Playing),
                despawn_dynamic_sun.run_if(crate::plugins::ends_playing_session),
            );
    }
}

fn sync_dynamic_sun(
    mut commands: Commands,
    settings: Res<Settings>,
    mut existing: Query<(Entity, &mut DirectionalLight), With<MeteorDynamicSun>>,
    mut frame: ResMut<FrameUniforms>,
) {
    frame.lighting = Vec4::new(
        if settings.environment_lighting {
            1.0
        } else {
            0.0
        },
        0.65,
        0.55,
        if settings.dynamic_shadows { 0.8 } else { 0.0 },
    );
    let want = settings.dynamic_shadows || settings.environment_lighting;
    let have = existing.iter().next().map(|(entity, _)| entity);
    match sun_action(want, have.is_some()) {
        SunAction::Spawn => {
            commands.spawn((
                MeteorDynamicSun,
                DirectionalLight {
                    color: Color::WHITE,
                    illuminance: LIGHT_ILLUMINANCE,
                    shadow_maps_enabled: settings.dynamic_shadows,
                    shadow_depth_bias: 0.01,

                    shadow_normal_bias: 1.0,
                    ..default()
                },
                Transform::from_xyz(40.0, 80.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
                bevy::light::CascadeShadowConfigBuilder {
                    num_cascades: 1,
                    maximum_distance: 10_000.0,
                    ..default()
                }
                .build(),
            ));
        }
        SunAction::Despawn => {
            if let Some(entity) = have {
                commands.entity(entity).despawn();
            }
        }
        SunAction::Keep => {
            for (_, mut light) in &mut existing {
                if light.shadow_maps_enabled != settings.dynamic_shadows {
                    light.shadow_maps_enabled = settings.dynamic_shadows;
                }
            }
        }
    }
}

const SHADOW_DIAMETER: f32 = 256.0;
const SHADOW_DEPTH: f32 = 1024.0;

fn stable_cascade(rotation: Quat, anchor: Vec3, resolution: usize) -> Cascade {
    let world_from_light = Mat4::from_quat(rotation);
    let light_from_world = world_from_light.transpose();
    let texel_size = SHADOW_DIAMETER / resolution.max(1) as f32;
    let center = (light_from_world.transform_point3(anchor) / texel_size).floor() * texel_size;
    let near_center = center + Vec3::Z * (SHADOW_DEPTH * 0.5);
    let cascade_from_world = Mat4::from_cols(
        light_from_world.x_axis,
        light_from_world.y_axis,
        light_from_world.z_axis,
        (-near_center).extend(1.0),
    );
    let clip_from_cascade = Mat4::from_cols(
        Vec4::new(2.0 / SHADOW_DIAMETER, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / SHADOW_DIAMETER, 0.0, 0.0),
        Vec4::new(0.0, 0.0, SHADOW_DEPTH.recip(), 0.0),
        Vec4::new(0.0, 0.0, 1.0, 1.0),
    );
    Cascade {
        world_from_cascade: world_from_light * Mat4::from_translation(near_center),
        clip_from_cascade,
        clip_from_world: clip_from_cascade * cascade_from_world,
        texel_size,
    }
}

fn stabilize_shadow_projection(
    map: Option<Res<DirectionalLightShadowMap>>,
    local: Query<&GlobalTransform, With<crate::plugins::stage_plugin::LocalFighter>>,
    fighters: Query<&GlobalTransform, With<crate::components::fighter::Fighter>>,
    spectator: Option<Res<crate::plugins::camera_plugin::spectator::SpectatorState>>,
    mut lights: Query<(&GlobalTransform, &mut Cascades), With<MeteorDynamicSun>>,
) {
    let Some(map) = map else {
        return;
    };
    let anchor = spectator
        .as_ref()
        .filter(|s| s.active)
        .and_then(|s| s.target)
        .and_then(|id| fighters.get(id).ok())
        .or_else(|| local.iter().next())
        .map_or(Vec3::ZERO, GlobalTransform::translation);
    for (transform, mut cascades) in &mut lights {
        let stable = stable_cascade(transform.rotation(), anchor, map.size);
        for views in cascades.cascades.values_mut() {
            views.clear();
            views.push(stable.clone());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SunAction {
    Spawn,
    Despawn,
    Keep,
}

fn sun_action(want: bool, have: bool) -> SunAction {
    match (want, have) {
        (true, false) => SunAction::Spawn,
        (false, true) => SunAction::Despawn,
        _ => SunAction::Keep,
    }
}

fn despawn_dynamic_sun(mut commands: Commands, existing: Query<Entity, With<MeteorDynamicSun>>) {
    for entity in &existing {
        commands.entity(entity).despawn();
    }
}

fn sync_gmdl_shadow_flags(
    materials: Res<Assets<GmdlMaterial>>,
    query: Query<(
        Entity,
        Ref<MeshMaterial3d<GmdlMaterial>>,
        Has<NotShadowCaster>,
        Has<NotShadowReceiver>,
    )>,
    mut commands: Commands,
) {
    for (entity, handle, no_cast, no_receive) in &query {
        if !materials.is_changed() && !handle.is_changed() {
            continue;
        }
        let Some(material) = materials.get(&handle.0) else {
            continue;
        };
        let (want_no_cast, want_no_receive) = match material.mode {
            GmdlMode::Opaque => (false, false),
            GmdlMode::Blend => (true, false),
            GmdlMode::Add | GmdlMode::SrcColorOne | GmdlMode::MultiplyShadow => (true, true),
        };
        let mut entity = commands.entity(entity);
        if no_cast != want_no_cast {
            if want_no_cast {
                entity.insert(NotShadowCaster);
            } else {
                entity.remove::<NotShadowCaster>();
            }
        }
        if no_receive != want_no_receive {
            if want_no_receive {
                entity.insert(NotShadowReceiver);
            } else {
                entity.remove::<NotShadowReceiver>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::stage_material::GmdlMaterialParams;
    use bevy::state::app::AppExtStates;

    #[test]
    fn actor_travel_preserves_world_texel_alignment() {
        let rotation = Transform::from_xyz(40.0, 80.0, 30.0)
            .looking_at(Vec3::ZERO, Vec3::Y)
            .rotation;
        let a = stable_cascade(rotation, Vec3::ZERO, 2048);
        let b = stable_cascade(rotation, Vec3::new(13.0, 4.0, -17.0), 2048);
        assert_eq!(a.texel_size, b.texel_size);
        let world = Vec3::new(1.7, 2.1, -3.4);
        let delta = (b.clip_from_world.transform_point3(world)
            - a.clip_from_world.transform_point3(world))
        .xy()
            * 1024.0;
        assert!(
            (delta - delta.round()).abs().max_element() < 0.001,
            "coverage may scroll by whole texels, never slide the world's shadow grid"
        );
    }

    #[test]
    fn sun_action_follows_setting() {
        assert_eq!(sun_action(true, false), SunAction::Spawn);
        assert_eq!(sun_action(true, true), SunAction::Keep);
        assert_eq!(sun_action(false, true), SunAction::Despawn);
        assert_eq!(sun_action(false, false), SunAction::Keep);
    }

    #[test]
    fn lighting_and_shadows_update_independently_while_paused() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_resource::<Assets<GmdlMaterial>>()
            .insert_state(AppState::Playing)
            .insert_resource(Settings {
                dynamic_shadows: true,
                ..default()
            })
            .add_plugins(DynamicShadowsPlugin);
        app.update();
        let sun = app
            .world_mut()
            .query_filtered::<Entity, With<MeteorDynamicSun>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Paused);
        app.update();
        assert!(
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .shadow_maps_enabled
        );
        {
            let mut settings = app.world_mut().resource_mut::<Settings>();
            settings.environment_lighting = true;
            settings.dynamic_shadows = false;
        }
        app.update();
        assert!(
            !app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .shadow_maps_enabled
        );
        assert_eq!(app.world().resource::<FrameUniforms>().lighting.x, 1.0);
        app.world_mut()
            .resource_mut::<Settings>()
            .environment_lighting = false;
        app.update();
        assert!(app.world().get_entity(sun).is_err());
    }

    #[test]
    fn additive_materials_opt_out_of_shadows() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_resource::<Assets<GmdlMaterial>>()
            .init_state::<AppState>()
            .insert_resource(Settings::default())
            .add_plugins(DynamicShadowsPlugin);
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Playing);
        app.update();

        let handle = app
            .world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .add(GmdlMaterial::new(
                GmdlMaterialParams {
                    opacity: 1.0,
                    alpha_cutoff: 0.0,
                    mode: GmdlMode::Add.as_u32(),
                    has_texture: 0,
                    uv_offset: Vec2::ZERO,
                },
                None,
                false,
                GmdlMode::Add,
            ));
        let entity = app.world_mut().spawn(MeshMaterial3d(handle)).id();
        app.update();
        assert!(app.world().get::<NotShadowCaster>(entity).is_some());
        assert!(app.world().get::<NotShadowReceiver>(entity).is_some());
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod render_tests;
