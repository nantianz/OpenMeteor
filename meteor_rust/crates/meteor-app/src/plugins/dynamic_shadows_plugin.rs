use super::render_plugin::legacy_stage_render::{SceneRenderProfile, ShadowRule};
use crate::app_state::{AppState, Settings};
use crate::plugins::stage_material::{FrameUniforms, GmdlMaterial, GmdlMode};
use bevy::camera::primitives::MeshAabb;
use bevy::light::cascade::Cascade;
use bevy::light::{
    Cascades, DirectionalLightShadowMap, NotShadowCaster, NotShadowReceiver, SimulationLightSystems,
};
use bevy::pbr::{DirectionalShadowCasters, ShadowCacheTarget, StaticShadowCaster};
use bevy::prelude::*;

use bevy::render::{
    sync_world::MainEntity, view::ExtractedView, Extract, ExtractSchedule, Render, RenderApp,
    RenderSystems,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
const LIGHT_ILLUMINANCE: f32 = 1.0;

#[derive(Component)]
struct MeteorDynamicSun;

#[derive(Component)]
struct AuthoredCascades;
#[derive(Resource, Clone)]
pub(super) struct StageShadowBake {
    bounds: (Vec3, Vec3),
    casters: usize,
    camera: MainEntity,
    ready: Arc<AtomicBool>,
}
impl StageShadowBake {
    pub(super) fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
}
pub struct DynamicShadowsPlugin;

impl Plugin for DynamicShadowsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameUniforms>()
            .add_systems(
                Update,
                (
                    sync_dynamic_sun.after(super::graphics_pack::sky::update_cycle),
                    sync_gmdl_shadow_flags,
                )
                    .run_if(in_state(AppState::Playing).or_else(in_state(AppState::Paused))),
            )
            .add_systems(Update, sync_shadowmap_size.before(sync_dynamic_sun))
            .add_systems(
                PostUpdate,
                stabilize_shadow_projection
                    .after(SimulationLightSystems::UpdateDirectionalLightCascades)
                    .before(SimulationLightSystems::UpdateLightFrusta),
            )
            .add_systems(
                OnExit(AppState::Playing),
                (despawn_dynamic_sun, clear_stage_shadow_bake)
                    .run_if(crate::plugins::ends_playing_session),
            );
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .add_systems(ExtractSchedule, extract_stage_shadow_bake)
            .add_systems(
                Render,
                confirm_stage_shadow_bake
                    .in_set(super::render_pacing::DrawFrame)
                    .after(RenderSystems::Render)
                    .before(RenderSystems::Cleanup),
            );
        app.add_systems(
            OnEnter(AppState::Playing),
            prepare_stage_shadow_bake
                .after(super::render_plugin::setup_stage_scene)
                .run_if(crate::plugins::starts_fresh_playing_session),
        );
    }
}
fn prepare_stage_shadow_bake(
    mut commands: Commands,
    meshes: Res<Assets<Mesh>>,
    loaded: Option<Res<crate::app_state::LoadedStageInfo>>,
    camera: Query<Entity, With<super::camera_plugin::GameplayCamera>>,
    casters: Query<(&Mesh3d, &Transform), With<StaticShadowCaster>>,
) {
    let stage = loaded.as_ref().map_or("", |stage| stage.stage_id.as_str());
    if !super::scene_render_optimizations::static_shadows(stage) {
        return;
    }
    let Ok(camera) = camera.single() else { return };
    let mut world_min = Vec3::splat(f32::INFINITY);
    let mut world_max = Vec3::splat(f32::NEG_INFINITY);
    let mut count = 0;
    for (mesh, transform) in &casters {
        let Some(bounds) = meshes.get(&mesh.0).and_then(MeshAabb::compute_aabb) else {
            continue;
        };
        let affine = transform.compute_affine();
        let center = affine.transform_point3a(bounds.center);
        let axes = affine.matrix3;
        let extent = axes.x_axis.abs() * bounds.half_extents.x
            + axes.y_axis.abs() * bounds.half_extents.y
            + axes.z_axis.abs() * bounds.half_extents.z;
        if !center.is_finite() || !extent.is_finite() {
            continue;
        }
        world_min = world_min.min((center - extent).into());
        world_max = world_max.max((center + extent).into());
        count += 1;
    }
    if count > 0 {
        commands.insert_resource(StageShadowBake {
            bounds: (world_min, world_max),
            casters: count,
            camera: camera.into(),
            ready: Arc::default(),
        });
    }
}
fn clear_stage_shadow_bake(mut commands: Commands) {
    commands.remove_resource::<StageShadowBake>();
}
fn extract_stage_shadow_bake(mut commands: Commands, bake: Extract<Option<Res<StageShadowBake>>>) {
    if let Some(bake) = bake.as_deref().filter(|bake| !bake.ready()) {
        commands.insert_resource(bake.clone());
    } else {
        commands.remove_resource::<StageShadowBake>();
    }
}
fn confirm_stage_shadow_bake(
    bake: Option<Res<StageShadowBake>>,
    caches: Query<(&ShadowCacheTarget, &ExtractedView)>,
) {
    let Some(bake) = bake else { return };
    if caches.iter().any(|(cache, view)| {
        view.retained_view_entity.auxiliary_entity == bake.camera
            && cache.cached_caster_count() >= bake.casters
    }) {
        bake.ready.store(true, Ordering::Release);
    }
}

fn sync_dynamic_sun(
    mut commands: Commands,
    settings: Res<Settings>,
    mut existing: Query<
        (
            Entity,
            &mut DirectionalLight,
            &mut Transform,
            &mut DirectionalShadowCasters,
        ),
        With<MeteorDynamicSun>,
    >,
    graphics: Option<Res<crate::plugins::graphics_pack::GraphicsPacks>>,
    loaded: Option<Res<crate::app_state::LoadedStageInfo>>,
    sky: Query<&crate::plugins::graphics_pack::sky::CloudSky>,
    render_profile: Option<Res<SceneRenderProfile>>,
    bake: Option<Res<StageShadowBake>>,
    shadowmap: Option<Res<DirectionalLightShadowMap>>,
    mut frame: ResMut<FrameUniforms>,
) {
    let atmosphere = sky.iter().find_map(|sky| sky.0);
    let authored_sun = render_profile
        .as_ref()
        .filter(|_| atmosphere.is_none())
        .and_then(|profile| profile.sun.as_ref());
    let allows_shadows = authored_sun.is_none_or(|sun| sun.shadows_enabled);
    let shadows = allows_shadows && (settings.scene_shadows || settings.dynamic_shadows);
    let shadow_casters = match (settings.scene_shadows, settings.dynamic_shadows) {
        (true, false) => DirectionalShadowCasters::StaticOnly,
        (false, true) => DirectionalShadowCasters::DynamicOnly,
        _ => DirectionalShadowCasters::All,
    };
    if !allows_shadows || !settings.scene_shadows {
        if let Some(bake) = &bake {
            bake.ready.store(true, Ordering::Release);
        }
    }
    frame.lighting = Vec4::new(
        if settings.environment_lighting {
            1.0
        } else {
            0.0
        },
        0.65,
        0.55,
        if shadows { 0.8 } else { 0.0 },
    );
    let stage = loaded.as_ref().map_or("", |s| s.stage_id.as_str());
    let environment = graphics.as_ref().and_then(|p| {
        p.scenes(stage)
            .filter_map(|(_, s)| s.environment.as_ref())
            .last()
    });
    let direction = atmosphere
        .map(|light| light.sun.truncate())
        .or_else(|| authored_sun.map(|sun| sun.direction))
        .or_else(|| environment.map(|e| Vec3::from_array(e.sun_direction).normalize()));
    let sun_transform = direction.map_or_else(
        || Transform::from_xyz(40.0, 80.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
        |direction| {
            Transform::from_translation(direction * 100.0).looking_at(
                Vec3::ZERO,
                if direction.y.abs() > 0.99 {
                    Vec3::Z
                } else {
                    Vec3::Y
                },
            )
        },
    );
    let want = shadows || settings.environment_lighting;
    let have = existing.iter().next().map(|(entity, _, _, _)| entity);
    match sun_action(want, have.is_some()) {
        SunAction::Spawn => {
            let mut sun = commands.spawn((
                MeteorDynamicSun,
                shadow_casters,
                DirectionalLight {
                    color: Color::WHITE,
                    illuminance: LIGHT_ILLUMINANCE,
                    shadow_maps_enabled: shadows,
                    shadow_depth_bias: 0.01,

                    shadow_normal_bias: shadow_normal_bias(authored_sun, shadowmap.as_deref()),
                    ..default()
                },
                sun_transform,
                authored_sun
                    .filter(|sun| !sun.heavy_scene && bake.is_none())
                    .map_or_else(
                        || {
                            bevy::light::CascadeShadowConfigBuilder {
                                num_cascades: 1,
                                maximum_distance: 10_000.0,
                                ..default()
                            }
                            .build()
                        },
                        |sun| sun.cascades(),
                    ),
            ));
            if bake.is_none() && authored_sun.is_some_and(|sun| !sun.heavy_scene) {
                sun.insert(AuthoredCascades);
            } else if super::scene_render_optimizations::static_shadows(stage) {
                sun.insert(bevy::pbr::CacheStaticDirectionalShadows);
            }
        }
        SunAction::Despawn => {
            if let Some(entity) = have {
                commands.entity(entity).despawn();
            }
        }
        SunAction::Keep => {
            for (_, mut light, mut transform, mut casters) in &mut existing {
                if *transform != sun_transform {
                    *transform = sun_transform;
                }
                casters.set_if_neq(shadow_casters);
                if light.shadow_maps_enabled != shadows {
                    light.shadow_maps_enabled = shadows;
                }
                let bias = shadow_normal_bias(authored_sun, shadowmap.as_deref());
                if light.shadow_normal_bias != bias {
                    light.shadow_normal_bias = bias;
                }
            }
        }
    }
}
fn shadow_normal_bias(
    sun: Option<&super::render_plugin::legacy_stage_render::SceneSun>,
    shadowmap: Option<&DirectionalLightShadowMap>,
) -> f32 {
    sun.and_then(|sun| sun.normal_offset).map_or(1.0, |offset| {
        offset.max(0.0) * shadowmap.map_or(2048, |map| map.size) as f32
    })
}
fn sync_shadowmap_size(
    profile: Option<Res<SceneRenderProfile>>,
    sky: Query<&crate::plugins::graphics_pack::sky::CloudSky>,
    settings: Res<Settings>,
    shadowmap: Option<ResMut<DirectionalLightShadowMap>>,
    mut original_size: Local<Option<usize>>,
) {
    let Some(mut shadowmap) = shadowmap else {
        return;
    };
    let authored_size = profile
        .filter(|_| sky.iter().all(|sky| sky.0.is_none()))
        .and_then(|profile| {
            profile.sun.as_ref().and_then(|sun| {
                ((settings.scene_shadows || settings.dynamic_shadows) && sun.shadows_enabled)
                    .then_some(sun.shadowmap_size)
                    .flatten()
            })
        });
    let size = match settings.shadow_quality {
        1 => Some(1024),
        2 => Some(2048),
        3 => Some(4096),
        _ => authored_size,
    };
    if let Some(size) = size {
        original_size.get_or_insert(shadowmap.size);
        if shadowmap.size != size {
            shadowmap.size = size;
        }
    } else if let Some(size) = original_size.take() {
        shadowmap.size = size;
    }
}

const SHADOW_DIAMETER: f32 = 256.0;
const SHADOW_DEPTH: f32 = 1024.0;

fn stable_cascade(rotation: Quat, anchor: Vec3, resolution: usize) -> Cascade {
    let world_from_light = Mat4::from_quat(rotation);
    let light_from_world = world_from_light.transpose();
    let texel_size = SHADOW_DIAMETER / resolution.max(1) as f32;
    let center = (light_from_world.transform_point3(anchor) / texel_size).floor() * texel_size;
    orthographic_cascade(
        world_from_light,
        center,
        SHADOW_DIAMETER,
        SHADOW_DEPTH,
        resolution,
    )
}
fn map_cascade(rotation: Quat, (min, max): (Vec3, Vec3), resolution: usize) -> Cascade {
    let world_from_light = Mat4::from_quat(rotation);
    let light_from_world = world_from_light.transpose();
    let center = light_from_world.transform_point3((min + max) * 0.5);
    let half = (max - min) * 0.5;
    let extent = light_from_world.x_axis.truncate().abs() * half.x
        + light_from_world.y_axis.truncate().abs() * half.y
        + light_from_world.z_axis.truncate().abs() * half.z;
    let diameter = 2.0 * (extent.x.max(extent.y) + 8.0);
    let depth = 2.0 * (extent.z + 8.0);
    orthographic_cascade(world_from_light, center, diameter, depth, resolution)
}
fn orthographic_cascade(
    world_from_light: Mat4,
    center: Vec3,
    diameter: f32,
    depth: f32,
    resolution: usize,
) -> Cascade {
    let light_from_world = world_from_light.transpose();
    let near_center = center + Vec3::Z * (depth * 0.5);
    let cascade_from_world = Mat4::from_cols(
        light_from_world.x_axis,
        light_from_world.y_axis,
        light_from_world.z_axis,
        (-near_center).extend(1.0),
    );
    let clip_from_cascade = Mat4::from_cols(
        Vec4::new(2.0 / diameter, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / diameter, 0.0, 0.0),
        Vec4::new(0.0, 0.0, depth.recip(), 0.0),
        Vec4::new(0.0, 0.0, 1.0, 1.0),
    );
    Cascade {
        world_from_cascade: world_from_light * Mat4::from_translation(near_center),
        clip_from_cascade,
        clip_from_world: clip_from_cascade * cascade_from_world,
        texel_size: diameter / resolution.max(1) as f32,
    }
}

fn stabilize_shadow_projection(
    map: Option<Res<DirectionalLightShadowMap>>,
    bake: Option<Res<StageShadowBake>>,
    local: Query<&GlobalTransform, With<crate::plugins::stage_plugin::LocalFighter>>,
    fighters: Query<&GlobalTransform, With<crate::components::fighter::Fighter>>,
    spectator: Option<Res<crate::plugins::camera_plugin::spectator::SpectatorState>>,
    mut lights: Query<
        (&GlobalTransform, &mut Cascades),
        (With<MeteorDynamicSun>, Without<AuthoredCascades>),
    >,
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
        let stable = bake.as_deref().map_or_else(
            || stable_cascade(transform.rotation(), anchor, map.size),
            |bake| map_cascade(transform.rotation(), bake.bounds, map.size),
        );
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
        Option<Ref<ShadowRule>>,
    )>,
    mut commands: Commands,
) {
    for (entity, handle, no_cast, no_receive, rule) in &query {
        if !materials.is_changed()
            && !handle.is_changed()
            && !rule.as_ref().is_some_and(|rule| rule.is_changed())
        {
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
        let want_no_cast = rule
            .as_ref()
            .and_then(|rule| rule.cast_shadows)
            .map_or(want_no_cast, |cast| !cast);
        let want_no_receive = rule
            .as_ref()
            .and_then(|rule| rule.receive_shadows)
            .map_or(want_no_receive, |receive| !receive);
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
    fn fixed_map_shadow_covers_every_corner_at_each_resolution() {
        let rotation = Transform::from_xyz(40.0, 80.0, 30.0)
            .looking_at(Vec3::ZERO, Vec3::Y)
            .rotation;
        let min = Vec3::new(-124.0, -9.0, 17.0);
        let max = Vec3::new(483.0, 44.0, 87.0);
        let low = map_cascade(rotation, (min, max), 1024);
        let high = map_cascade(rotation, (min, max), 4096);
        assert_eq!(low.clip_from_world, high.clip_from_world);
        assert_eq!(low.texel_size, 4.0 * high.texel_size);
        for x in [min.x, max.x] {
            for y in [min.y, max.y] {
                for z in [min.z, max.z] {
                    let p = high.clip_from_world.transform_point3(Vec3::new(x, y, z));
                    assert!(p.xy().abs().max_element() < 1.0);
                    assert!((0.0..1.0).contains(&p.z));
                }
            }
        }
    }
    #[test]
    fn shadow_quality_applies_and_restores_default_without_a_stage_override() {
        let mut app = App::new();
        app.insert_resource(Settings {
            shadow_quality: 1,
            ..default()
        })
        .insert_resource(DirectionalLightShadowMap { size: 2048 })
        .add_systems(Update, sync_shadowmap_size);
        app.update();
        assert_eq!(
            app.world().resource::<DirectionalLightShadowMap>().size,
            1024
        );
        app.world_mut().resource_mut::<Settings>().shadow_quality = 3;
        app.update();
        assert_eq!(
            app.world().resource::<DirectionalLightShadowMap>().size,
            4096
        );
        app.world_mut().resource_mut::<Settings>().shadow_quality = 0;
        app.update();
        assert_eq!(
            app.world().resource::<DirectionalLightShadowMap>().size,
            2048
        );
    }
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
    fn shadow_direction_tracks_the_active_day_or_night_light() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_resource::<Assets<GmdlMaterial>>()
            .insert_state(AppState::Playing)
            .insert_resource(Settings {
                dynamic_shadows: true,
                ..default()
            })
            .add_plugins(DynamicShadowsPlugin);
        let sky = app
            .world_mut()
            .spawn(crate::plugins::graphics_pack::sky::CloudSky(Some(
                crate::plugins::graphics_pack::sky::SkyLighting {
                    sky: Vec3::splat(0.1),
                    ground: Vec3::splat(0.02),
                    sun: Vec3::new(0.7, 0.6, 0.3).normalize().extend(1.0),
                    sun_color: Vec3::ONE,
                    night_factor: 0.0,
                },
            )))
            .id();
        app.update();
        let sun = app
            .world_mut()
            .query_filtered::<Entity, With<MeteorDynamicSun>>()
            .single(app.world())
            .unwrap();
        let direction = Vec3::new(-0.6, 0.7, -0.2).normalize();
        app.world_mut()
            .get_mut::<crate::plugins::graphics_pack::sky::CloudSky>(sky)
            .unwrap()
            .0
            .as_mut()
            .unwrap()
            .sun = direction.extend(0.1);
        app.update();
        let actual = app.world().get::<Transform>(sun).unwrap().rotation * Vec3::Z;
        assert!(
            actual.dot(direction) > 0.999,
            "shadow map should face the active moon"
        );
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
            app.world()
                .get::<DirectionalLight>(sun)
                .unwrap()
                .shadow_maps_enabled
        );
        assert_eq!(
            app.world().get::<DirectionalShadowCasters>(sun),
            Some(&DirectionalShadowCasters::StaticOnly),
        );
        app.world_mut().resource_mut::<Settings>().scene_shadows = false;
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
