use bevy::camera::{CameraOutputMode, CameraUpdateSystems, ImageRenderTarget, RenderTarget};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, BlendState, TextureFormat};
use bevy::shader::{Shader, ShaderRef};
use bevy::window::PrimaryWindow;
use std::collections::HashMap;

use super::SettingsReturnState;
use crate::app_state::AppState;
use crate::plugins::camera_plugin::{GameplayCamera, UiCamera};

const SHADER: Handle<Shader> = bevy::asset::uuid_handle!("63821080-3cf1-4aef-9e1f-44a54b2fba34");

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct PauseBlurMaterial {
    #[uniform(0)]
    params: Vec4,
    #[texture(1)]
    #[sampler(2)]
    image: Handle<Image>,
}

impl UiMaterial for PauseBlurMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(SHADER)
    }
}

#[derive(Component)]
pub(crate) struct PauseForeground;

#[derive(Resource, Default)]
struct PauseBackdrop {
    image: Option<Handle<Image>>,
    material: Option<Handle<PauseBlurMaterial>>,
    camera: Option<Entity>,
    root: Option<Entity>,
    size: UVec2,
    targets: HashMap<Entity, RenderTarget>,
    foreground: HashMap<Entity, Option<UiTargetCamera>>,
}

pub(super) struct PauseBackdropPlugin;

impl Plugin for PauseBackdropPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(UiMaterialPlugin::<PauseBlurMaterial>::default())
            .init_resource::<PauseBackdrop>();
        bevy::asset::load_internal_asset!(app, SHADER, "pause_backdrop.wgsl", Shader::from_wgsl);
        app.add_systems(
            PostUpdate,
            sync_pause_backdrop
                .before(CameraUpdateSystems)
                .before(bevy::ui::UiSystems::Propagate),
        );
    }
}

fn sync_pause_backdrop(
    state: Res<State<AppState>>,
    from: Option<Res<SettingsReturnState>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut backdrop: ResMut<PauseBackdrop>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<PauseBlurMaterial>>,
    mut commands: Commands,
    mut cameras: Query<
        (Entity, &mut RenderTarget, Has<UiCamera>),
        Or<(With<GameplayCamera>, With<UiCamera>)>,
    >,
    foreground: Query<(Entity, Option<&UiTargetCamera>), With<PauseForeground>>,
) {
    let active = *state.get() == AppState::Paused
        || (*state.get() == AppState::Settings
            && from.is_some_and(|from| from.0 == AppState::Paused));
    if !active {
        for (entity, target) in backdrop.targets.drain() {
            if let Ok((_, mut current, _)) = cameras.get_mut(entity) {
                *current = target;
            }
        }
        for (entity, original) in backdrop.foreground.drain() {
            if foreground.get(entity).is_ok() {
                match original {
                    Some(target) => {
                        commands.entity(entity).insert(target);
                    }
                    None => {
                        commands.entity(entity).remove::<UiTargetCamera>();
                    }
                }
            }
        }
        if let Some(root) = backdrop.root.take() {
            commands.entity(root).despawn();
        }
        if let Some(camera) = backdrop.camera.take() {
            commands.entity(camera).despawn();
        }
        backdrop.image = None;
        backdrop.material = None;
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let size = UVec2::new(window.physical_width(), window.physical_height());
    if size.x == 0 || size.y == 0 || cameras.iter().count() < 2 {
        return;
    }
    let params = Vec4::new(
        1.0 / size.x as f32,
        1.0 / size.y as f32,
        3.0 * window.scale_factor(),
        0.0,
    );
    if backdrop.image.is_none() {
        let image = images.add(Image::new_target_texture(
            size.x,
            size.y,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
        let material = materials.add(PauseBlurMaterial {
            params,
            image: image.clone(),
        });

        let Some(output) = cameras
            .iter()
            .find(|(_, _, ui)| *ui)
            .map(|(_, target, _)| target.clone())
        else {
            return;
        };
        let camera = commands
            .spawn((
                Camera2d,
                Msaa::Off,
                Camera {
                    order: 2,
                    clear_color: ClearColorConfig::Custom(Color::NONE),
                    output_mode: CameraOutputMode::Write {
                        blend_state: Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        clear_color: ClearColorConfig::None,
                    },
                    ..default()
                },
                output,
            ))
            .id();
        let root = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                ImageNode::new(image.clone()),
                UiTargetCamera(camera),
                GlobalZIndex(-1000),
                bevy::ui::FocusPolicy::Pass,
            ))
            .with_children(|root| {
                root.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    MaterialNode(material.clone()),
                    bevy::ui::FocusPolicy::Pass,
                ));
            })
            .id();
        backdrop.image = Some(image);
        backdrop.material = Some(material);
        backdrop.camera = Some(camera);
        backdrop.root = Some(root);
        backdrop.size = size;
    } else if backdrop.size != size {
        if let Some(mut image) = images.get_mut(backdrop.image.as_ref().unwrap()) {
            *image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
        }
        backdrop.size = size;
    }
    if let Some(mut material) = materials.get_mut(backdrop.material.as_ref().unwrap()) {
        if material.params != params {
            material.params = params;
        }
    }
    let target = RenderTarget::Image(ImageRenderTarget {
        handle: backdrop.image.as_ref().unwrap().clone(),
        scale_factor: window.scale_factor(),
    });
    for (entity, mut current, _) in &mut cameras {
        backdrop
            .targets
            .entry(entity)
            .or_insert_with(|| current.clone());
        if current.normalize(None) != target.normalize(None) {
            *current = target.clone();
        }
    }
    let camera = backdrop.camera.unwrap();
    for (entity, current) in &foreground {
        backdrop
            .foreground
            .entry(entity)
            .or_insert_with(|| current.map(|target| UiTargetCamera(target.0)));
        if current.is_none_or(|target| target.0 != camera) {
            commands.entity(entity).insert(UiTargetCamera(camera));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_preserves_hud_and_routes_foreground_then_restores_targets() {
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Paused))
            .insert_resource(SettingsReturnState(AppState::Paused))
            .init_resource::<PauseBackdrop>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<PauseBlurMaterial>>()
            .add_systems(PostUpdate, sync_pause_backdrop);
        let window = app
            .world_mut()
            .spawn((
                PrimaryWindow,
                Window {
                    resolution: (320, 640).into(),
                    ..default()
                },
            ))
            .id();
        let game = app
            .world_mut()
            .spawn((GameplayCamera, Camera3d::default()))
            .id();
        let ui = app.world_mut().spawn((UiCamera, Camera2d)).id();
        let original_game = app.world().get::<RenderTarget>(game).unwrap().clone();
        let original_ui = app.world().get::<RenderTarget>(ui).unwrap().clone();
        let hud = app
            .world_mut()
            .spawn((
                crate::app_state::PlayingRoot,
                Node {
                    display: Display::Grid,
                    ..default()
                },
                Visibility::Visible,
            ))
            .id();
        let menu = app
            .world_mut()
            .spawn((PauseForeground, Node::default()))
            .id();
        app.update();
        let image = app
            .world()
            .resource::<PauseBackdrop>()
            .image
            .clone()
            .unwrap();
        let overlay = app.world().resource::<PauseBackdrop>().camera.unwrap();
        for camera in [game, ui] {
            assert!(
                matches!(app.world().get::<RenderTarget>(camera).unwrap(), RenderTarget::Image(target) if target.handle == image)
            );
        }
        assert_eq!(app.world().get::<UiTargetCamera>(menu).unwrap().0, overlay);
        assert_eq!(app.world().get::<Node>(hud).unwrap().display, Display::Grid);
        assert_eq!(
            *app.world().get::<Visibility>(hud).unwrap(),
            Visibility::Visible
        );
        app.insert_resource(State::new(AppState::Settings));
        app.update();
        assert_eq!(
            app.world()
                .resource::<PauseBackdrop>()
                .image
                .as_ref()
                .unwrap(),
            &image
        );
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .resolution
            .set(640.0, 320.0);
        app.update();
        assert_eq!(
            app.world().resource::<PauseBackdrop>().size,
            UVec2::new(640, 320)
        );
        app.insert_resource(State::new(AppState::Playing));
        app.update();
        assert_eq!(
            app.world()
                .get::<RenderTarget>(game)
                .unwrap()
                .normalize(Some(window)),
            original_game.normalize(Some(window))
        );
        assert_eq!(
            app.world()
                .get::<RenderTarget>(ui)
                .unwrap()
                .normalize(Some(window)),
            original_ui.normalize(Some(window))
        );
        assert!(app.world().get::<UiTargetCamera>(menu).is_none());
        assert!(app.world().get::<Camera>(overlay).is_none());
        assert!(app.world().resource::<PauseBackdrop>().image.is_none());
    }

    #[test]
    fn pause_gaussian_shader_validates() {
        let source = include_str!("pause_backdrop.wgsl").replace(
            "#import bevy_ui::ui_vertex_output::UiVertexOutput",
            "struct UiVertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, };",
        );
        let module = naga::front::wgsl::parse_str(&source).expect("pause Gaussian parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("pause Gaussian validates");
    }

    #[test]
    fn headless_pause_blurs_live_scene_and_hud_with_sharp_foreground() {
        use bevy::app::PluginsState;
        use bevy::render::gpu_readback::{Readback, ReadbackComplete};
        use bevy::render::render_resource::{PollType, TextureUsages};
        use std::sync::{Arc, Mutex};
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .set(bevy::render::RenderPlugin {
                    synchronous_pipeline_compilation: true,
                    ..default()
                })
                .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
                .disable::<bevy::winit::WinitPlugin>()
                .disable::<bevy::audio::AudioPlugin>(),
        )
        .insert_state(AppState::Playing)
        .init_resource::<crate::app_state::Settings>()
        .insert_resource(SettingsReturnState(AppState::Paused))
        .add_plugins((
            crate::plugins::display_settings_plugin::DisplaySettingsPlugin,
            PauseBackdropPlugin,
        ));
        app.world_mut()
            .run_system_cached(crate::plugins::camera_plugin::setup_camera)
            .unwrap();
        let mut image = Image::new_target_texture(64, 64, TextureFormat::Rgba8UnormSrgb, None);
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let game = app
            .world_mut()
            .query_filtered::<Entity, With<GameplayCamera>>()
            .single(app.world())
            .unwrap();
        let ui = app
            .world_mut()
            .query_filtered::<Entity, With<UiCamera>>()
            .single(app.world())
            .unwrap();
        for camera in [game, ui] {
            app.world_mut()
                .entity_mut(camera)
                .insert(RenderTarget::Image(target.clone().into()));
        }
        app.world_mut().entity_mut(game).insert((
            Transform::from_xyz(0.0, 0.0, 5.0),
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: bevy::camera::ScalingMode::Fixed {
                    width: 4.0,
                    height: 4.0,
                },
                ..OrthographicProjection::default_3d()
            }),
        ));
        app.world_mut().get_mut::<Camera>(game).unwrap().clear_color =
            ClearColorConfig::Custom(Color::BLACK);
        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Rectangle::new(2.0, 2.0));
        let material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::WHITE,
                unlit: true,
                ..default()
            });
        app.world_mut()
            .spawn((Mesh3d(mesh), MeshMaterial3d(material)));
        let hud = app
            .world_mut()
            .spawn((
                crate::app_state::PlayingRoot,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(45.0),
                    top: Val::Px(0.0),
                    width: Val::Px(8.0),
                    height: Val::Px(8.0),
                    ..default()
                },
                BackgroundColor(Color::srgb(1.0, 0.0, 0.0)),
            ))
            .id();
        let menu = app
            .world_mut()
            .spawn((
                PauseForeground,
                Node {
                    display: Display::None,
                    position_type: PositionType::Absolute,
                    width: Val::Px(8.0),
                    height: Val::Px(8.0),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.0, 1.0, 0.0)),
                GlobalZIndex(300),
            ))
            .id();
        let pixels = Arc::new(Mutex::new(Vec::<u8>::new()));
        let output = pixels.clone();
        app.world_mut()
            .spawn(Readback::texture(target.clone()))
            .observe(move |event: On<ReadbackComplete>| {
                *output.lock().unwrap() = event.data.clone()
            });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while app.plugins_state() != PluginsState::Ready {
            assert!(std::time::Instant::now() < deadline);
            bevy::tasks::tick_global_task_pools_on_main_thread();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        app.finish();
        app.cleanup();
        app.world_mut().spawn((
            PrimaryWindow,
            Window {
                resolution: (64, 64).into(),
                ..default()
            },
        ));
        let capture = |app: &mut App| {
            for _ in 0..24 {
                app.update();
                app.world()
                    .resource::<bevy::render::renderer::RenderDevice>()
                    .poll(PollType::Poll)
                    .unwrap();
                std::thread::sleep(std::time::Duration::from_millis(3));
            }
            pixels.lock().unwrap().clone()
        };
        let sharp = capture(&mut app);
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Paused);
        app.world_mut().get_mut::<Node>(menu).unwrap().display = Display::Flex;
        let blurred = capture(&mut app);
        assert_eq!(blurred.len(), 64 * 64 * 4);
        let pixel = |data: &[u8], x: usize, y: usize| {
            let i = (y * 64 + x) * 4;
            [data[i], data[i + 1], data[i + 2]]
        };
        assert!(
            pixel(&blurred, 14, 32)[0] > pixel(&sharp, 14, 32)[0] + 20,
            "scene blurs horizontally"
        );
        assert!(
            pixel(&blurred, 32, 14)[0] > pixel(&sharp, 32, 14)[0] + 20,
            "scene blurs vertically"
        );
        assert!(
            pixel(&blurred, 43, 3)[0] > pixel(&sharp, 43, 3)[0] + 20,
            "HUD participates in the same Gaussian"
        );
        assert_eq!(pixel(&blurred, 3, 3), [0, 255, 0], "foreground stays clear");
        assert_eq!(app.world().get::<Node>(hud).unwrap().display, Display::Flex);
        assert!(
            pixel(&blurred, 32, 32)[0] > 200,
            "scene is retained instead of turning black"
        );

        app.world_mut().get_mut::<BackgroundColor>(hud).unwrap().0 = Color::srgb(0.0, 0.0, 1.0);
        let live = capture(&mut app);
        assert!(
            pixel(&live, 47, 3)[2] > pixel(&live, 47, 3)[0],
            "paused background remains live"
        );
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Playing);
        app.world_mut().get_mut::<Node>(menu).unwrap().display = Display::None;
        let restored = capture(&mut app);
        assert_eq!(
            pixel(&restored, 14, 32),
            pixel(&sharp, 14, 32),
            "normal output restored"
        );
    }
}
