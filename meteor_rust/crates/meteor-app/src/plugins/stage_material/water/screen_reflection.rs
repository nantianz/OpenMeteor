use super::interaction::LiquidSurface;
use crate::app_state::{AppState, Settings};
use crate::plugins::camera_plugin::GameplayCamera;
use crate::plugins::stage_material::GmdlMaterial;
use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::prelude::*;

#[derive(Component)]
struct WaterReflectionDepthOwner;

#[derive(Resource)]
struct WaterReflectionRuntime {
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
            prepare_session.run_if(in_state(AppState::Playing)),
        )
        .add_systems(
            OnExit(AppState::Playing),
            clear_on_session_exit.run_if(crate::plugins::ends_playing_session),
        );
}

fn clear_on_session_exit(
    mut commands: Commands,
    settings: Option<Res<Settings>>,
    cameras: Query<(Entity, Has<WaterReflectionDepthOwner>), With<GameplayCamera>>,
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
    for (entity, owned) in &cameras {
        if owned {
            release_depth_prepass(&mut commands, entity, settings.as_deref());
        }
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
    commands
        .entity(entity)
        .remove::<WaterReflectionDepthOwner>();
}

fn prepare_session(
    mut commands: Commands,
    settings: Option<Res<Settings>>,
    mut runtime: ResMut<WaterReflectionRuntime>,
    cameras: Query<
        (Entity, Has<DepthPrepass>, Has<WaterReflectionDepthOwner>),
        With<GameplayCamera>,
    >,
    surfaces: Query<(&LiquidSurface, &MeshMaterial3d<GmdlMaterial>)>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    let enabled = !cfg!(any(target_arch = "wasm32", target_os = "android"))
        && settings
            .as_ref()
            .is_some_and(|settings| settings.water_enhance);
    if runtime.prepared_water_enabled == Some(enabled) || cameras.is_empty() {
        return;
    }
    runtime.prepared_water_enabled = Some(enabled);
    let mut needs_depth = false;
    for (_, handle) in &surfaces {
        if let Some(mut material) = materials.get_mut(&handle.0) {
            if material.water.x == 1.0 {
                material.water_live_reflection = enabled && runtime.reflection_at_startup;
                material.water_screen_refraction = enabled && material.water_optics.control.y > 0.0;
                needs_depth |= material.water_live_reflection || material.water_screen_refraction;
            }
        }
    }
    for (entity, has_depth_prepass, owned) in &cameras {
        if needs_depth && !has_depth_prepass {
            commands
                .entity(entity)
                .insert((DepthPrepass, WaterReflectionDepthOwner));
        } else if !needs_depth && owned {
            release_depth_prepass(&mut commands, entity, settings.as_deref());
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
                water_live_reflections: enabled_at_startup,
                ..Default::default()
            })
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<WaterReflectionRuntime>()
            .add_systems(
                PostUpdate,
                prepare_session.run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                OnExit(AppState::Playing),
                clear_on_session_exit.run_if(crate::plugins::ends_playing_session),
            )
            .add_systems(
                bevy::state::prelude::OnTransition {
                    exited: AppState::Paused,
                    entered: AppState::Menu,
                },
                crate::plugins::cleanup_suspended_playing_session,
            );
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
        assert!(app
            .world()
            .get::<WaterReflectionDepthOwner>(camera)
            .is_some());
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
        assert!(app
            .world()
            .get::<WaterReflectionDepthOwner>(camera)
            .is_none());
    }
}
