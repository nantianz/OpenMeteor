use super::interaction::LiquidSurface;
use super::waves::WaterWaveMesh;
use crate::app_state::{AppState, LoadedStageInfo, Settings};
use crate::plugins::camera_plugin::GameplayCamera;
use crate::plugins::graphics_pack::effects::VisibleSoftEffects;
use crate::plugins::render_plugin::SceneDeform;
use crate::plugins::stage_material::{FrameUniforms, GmdlMaterial};
use bevy::asset::AssetEventSystems;
use bevy::camera::{primitives::Frustum, visibility::VisibilitySystems};
use bevy::core_pipeline::prepass::{DeferredPrepass, DepthPrepass};
use bevy::prelude::*;

use bevy::render::{
    occlusion_culling::OcclusionCulling, renderer::RenderAdapterInfo, view::NoIndirectDrawing,
};
#[derive(Component)]
struct SceneDepthOwner;
#[derive(Component)]
struct StageOcclusionOwner;

#[derive(Resource)]
pub(super) struct WaterReflectionRuntime {
    reflection_at_startup: bool,
    prepared_water_enabled: Option<bool>,
}

impl FromWorld for WaterReflectionRuntime {
    fn from_world(world: &mut World) -> Self {
        Self {
            reflection_at_startup: world
                .get_resource::<Settings>()
                .is_some_and(|settings| settings.water_live_reflections),
            prepared_water_enabled: None,
        }
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<WaterReflectionRuntime>()
        .add_systems(
            PostUpdate,
            prepare_session
                .before(AssetEventSystems)
                .run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            PostUpdate,
            sync_depth_prepass
                .after(prepare_session)
                .after(VisibilitySystems::MarkNewlyHiddenEntitiesInvisible)
                .run_if(in_state(AppState::Playing).or_else(in_state(AppState::Paused))),
        )
        .add_systems(
            OnExit(AppState::Playing),
            clear_on_session_exit.run_if(crate::plugins::ends_playing_session),
        );
}

fn clear_on_session_exit(
    mut commands: Commands,
    settings: Option<Res<Settings>>,
    cameras: Query<Entity, (With<GameplayCamera>, With<SceneDepthOwner>)>,
    occlusion_cameras: Query<Entity, With<StageOcclusionOwner>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut runtime: ResMut<WaterReflectionRuntime>,
) {
    runtime.prepared_water_enabled = None;
    for (_, material) in materials.iter_mut() {
        if material.water_live_reflection || material.water_screen_refraction {
            material.water_live_reflection = false;
            material.water_screen_refraction = false;
        }
    }
    for entity in &cameras {
        release_depth_prepass(&mut commands, entity, settings.as_deref());
    }
    for entity in &occlusion_cameras {
        commands
            .entity(entity)
            .remove::<(OcclusionCulling, StageOcclusionOwner)>();
    }
}

fn release_depth_prepass(commands: &mut Commands, entity: Entity, settings: Option<&Settings>) {
    if !cfg!(all(
        feature = "dlss5",
        target_os = "windows",
        target_arch = "x86_64"
    )) || !settings.is_some_and(|settings| settings.dlss5_neural_rendering)
    {
        commands.entity(entity).remove::<DepthPrepass>();
    }
    commands.entity(entity).remove::<SceneDepthOwner>();
}

pub(super) fn prepare_session(
    settings: Option<Res<Settings>>,
    mut runtime: ResMut<WaterReflectionRuntime>,
    surfaces: Query<(Ref<LiquidSurface>, &MeshMaterial3d<GmdlMaterial>)>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    let enabled = !cfg!(any(target_arch = "wasm32", target_os = "android"))
        && settings
            .as_ref()
            .is_some_and(|settings| settings.water_enhance);
    let update_materials = runtime.prepared_water_enabled != Some(enabled);
    for (surface, handle) in &surfaces {
        if (update_materials || surface.is_added()) && surface.is_clear_water() {
            if let Some(mut material) = materials.get_mut(&handle.0) {
                material.water_live_reflection = enabled && runtime.reflection_at_startup;
                material.water_screen_refraction = enabled && material.water_optics.control.y > 0.0;
            }
        }
    }
    if update_materials {
        runtime.prepared_water_enabled = Some(enabled);
    }
}
fn sync_depth_prepass(
    mut commands: Commands,
    settings: Option<Res<Settings>>,
    stage: Option<Res<LoadedStageInfo>>,
    adapter: Option<Res<RenderAdapterInfo>>,
    frame: Option<Res<FrameUniforms>>,
    effects: VisibleSoftEffects,
    runtime: Res<WaterReflectionRuntime>,
    cameras: Query<
        (
            Entity,
            &Camera,
            Option<&Frustum>,
            Has<DepthPrepass>,
            Has<SceneDepthOwner>,
            Has<OcclusionCulling>,
            Has<StageOcclusionOwner>,
            Has<DeferredPrepass>,
            Has<NoIndirectDrawing>,
        ),
        With<GameplayCamera>,
    >,
    surfaces: Query<(
        &LiquidSurface,
        &MeshMaterial3d<GmdlMaterial>,
        Option<&WaterWaveMesh>,
        Has<SceneDeform>,
    )>,
    materials: Res<Assets<GmdlMaterial>>,
) {
    if cfg!(any(target_arch = "wasm32", target_os = "android")) {
        return;
    }
    let soft_effects = effects.wants_depth_prepass();
    let try_occlusion = stage.is_some_and(|stage| {
        crate::plugins::scene_render_optimizations::options(&stage.stage_id).occlusion
    }) && adapter
        .is_some_and(|adapter| adapter.0.backend == wgpu::Backend::Vulkan)
        && frame.is_none_or(|frame| frame.camera.eye.w <= 0.0);
    for (
        entity,
        camera,
        frustum,
        has_depth_prepass,
        owned,
        has_occlusion,
        owns_occlusion,
        deferred,
        no_indirect_drawing,
    ) in &cameras
    {
        let needs_depth = camera.is_active
            && (soft_effects
                || (runtime.prepared_water_enabled == Some(true)
                    && surfaces.iter().any(|(surface, handle, wave, deformed)| {
                        materials.get(&handle.0).is_some_and(|material| {
                            material.water_live_reflection || material.water_screen_refraction
                        }) && surface.may_be_visible(frustum, wave, deformed)
                    })));
        if needs_depth && !has_depth_prepass {
            commands
                .entity(entity)
                .insert((DepthPrepass, SceneDepthOwner));
        } else if !needs_depth && owned {
            release_depth_prepass(&mut commands, entity, settings.as_deref());
        }
        let occlusion = try_occlusion
            && camera.is_active
            && !deferred
            && !no_indirect_drawing
            && (needs_depth || (has_depth_prepass && !owned));
        if occlusion && !has_occlusion {
            commands
                .entity(entity)
                .insert((OcclusionCulling, StageOcclusionOwner));
        } else if !occlusion && owns_occlusion {
            commands
                .entity(entity)
                .remove::<(OcclusionCulling, StageOcclusionOwner)>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::stage_material::{GmdlMaterialParams, GmdlMode};

    fn fixture(enabled_at_startup: bool) -> (App, Entity, Handle<GmdlMaterial>) {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(AppState::Playing)
            .insert_resource(Settings {
                water_enhance: true,
                water_live_reflections: enabled_at_startup,
                ..Default::default()
            })
            .init_resource::<Assets<GmdlMaterial>>()
            .add_systems(
                bevy::state::prelude::OnTransition {
                    exited: AppState::Paused,
                    entered: AppState::Menu,
                },
                crate::plugins::cleanup_suspended_playing_session,
            );
        install(&mut app);
        let mut material = GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 0.6,
                alpha_cutoff: 0.0,
                mode: GmdlMode::Blend.as_u32(),
                has_texture: 0,
                uv_offset: Vec2::ZERO,
            },
            None,
            false,
            GmdlMode::Blend,
        );
        material.water.x = 1.0;
        let material = app
            .world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .add(material);
        let camera = app
            .world_mut()
            .spawn((GameplayCamera, Camera::default()))
            .id();
        app.world_mut().spawn((
            LiquidSurface::test_surface(1.0),
            MeshMaterial3d(material.clone()),
        ));
        (app, camera, material)
    }

    fn material_live(app: &App, handle: &Handle<GmdlMaterial>) -> bool {
        app.world()
            .resource::<Assets<GmdlMaterial>>()
            .get(handle)
            .unwrap()
            .water_live_reflection
    }

    #[test]
    fn reflection_setting_is_sampled_only_at_process_start() {
        let (mut off, off_camera, off_material) = fixture(false);
        off.world_mut()
            .resource_mut::<Settings>()
            .water_live_reflections = true;
        off.update();
        assert!(!material_live(&off, &off_material));
        assert!(off.world().get::<DepthPrepass>(off_camera).is_none());

        if cfg!(any(target_arch = "wasm32", target_os = "android")) {
            return;
        }
        let (mut on, on_camera, on_material) = fixture(true);
        on.update();
        assert!(material_live(&on, &on_material));
        assert!(on.world().get::<DepthPrepass>(on_camera).is_some());

        on.world_mut()
            .resource_mut::<Settings>()
            .water_live_reflections = false;
        on.update();
        assert!(material_live(&on, &on_material));
        assert!(on.world().get::<DepthPrepass>(on_camera).is_some());
    }

    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    #[test]
    fn camera_frustum_changes_water_depth_without_changing_the_material() {
        use bevy::camera::CameraProjection;
        let (mut app, camera, water) = fixture(true);
        let surface = app
            .world_mut()
            .query_filtered::<Entity, With<LiquidSurface>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(surface)
            .insert(LiquidSurface::test_pool(0.0));
        let eye = Vec3::new(0.0, 4.0, 32.0);
        let projection = PerspectiveProjection::default();
        let away = Vec3::Z * 64.0;
        for (target, visible) in [(away, false), (Vec3::ZERO, true), (away, false)] {
            let transform = Transform::from_translation(eye).looking_at(target, Vec3::Y);
            app.world_mut()
                .entity_mut(camera)
                .insert(projection.compute_frustum(&transform.into()));
            app.update();
            assert_eq!(app.world().get::<DepthPrepass>(camera).is_some(), visible);
            assert!(material_live(&app, &water));
        }
        app.world_mut().entity_mut(camera).insert(DepthPrepass);
        app.update();
        assert!(app.world().get::<DepthPrepass>(camera).is_some());
    }
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    #[test]
    fn soft_emitter_only_requests_depth_while_visible() {
        use crate::plugins::graphics_pack::{config::EffectEmitter, fx::FxEmitter};
        let (mut app, camera, _) = fixture(false);
        let emitter = app
            .world_mut()
            .spawn((
                FxEmitter {
                    spec: EffectEmitter {
                        soft: true,
                        ..default()
                    },
                    root: camera,
                    root_world: GlobalTransform::IDENTITY,
                    delay_left: 0.0,
                    started: true,
                    age: 0.0,
                    particles: vec![],
                    rng: 0,
                    mesh: default(),
                    material: default(),
                },
                ViewVisibility::HIDDEN,
            ))
            .id();
        for visibility in [
            ViewVisibility::HIDDEN,
            ViewVisibility::VISIBLE,
            ViewVisibility::HIDDEN,
        ] {
            app.world_mut().entity_mut(emitter).insert(visibility);
            app.update();
            assert_eq!(
                app.world().get::<DepthPrepass>(camera).is_some(),
                visibility.get()
            );
        }
    }
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    #[test]
    fn water_toggle_releases_refraction_pass_and_restores_it() {
        let (mut app, camera, water) = fixture(false);
        app.world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .get_mut(&water)
            .unwrap()
            .water_optics
            .control
            .y = 0.6;
        for enabled in [true, false, true] {
            app.world_mut().resource_mut::<Settings>().water_enhance = enabled;
            app.update();
            let materials = app.world().resource::<Assets<GmdlMaterial>>();
            assert_eq!(
                materials.get(&water).unwrap().water_screen_refraction,
                enabled
            );
            assert_eq!(app.world().get::<DepthPrepass>(camera).is_some(), enabled);
        }
    }

    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    #[test]
    fn paused_to_menu_clears_the_fixed_session_pipeline() {
        let (mut app, camera, material) = fixture(true);
        app.update();
        assert!(material_live(&app, &material));
        assert!(app.world().get::<SceneDepthOwner>(camera).is_some());
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Paused);
        app.update();
        assert!(material_live(&app, &material));
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Menu);
        app.update();
        assert!(!material_live(&app, &material));
        assert!(app.world().get::<DepthPrepass>(camera).is_none());
        assert!(app.world().get::<SceneDepthOwner>(camera).is_none());
    }
}
