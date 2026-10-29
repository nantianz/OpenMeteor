#[cfg(target_os = "android")]
pub(crate) mod android_assets;
pub mod app_state;
pub mod components;
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod desktop_icon;
mod graphics_settings;
#[cfg(all(
    feature = "dlss5",
    target_os = "windows",
    target_arch = "x86_64",
    target_env = "msvc"
))]
mod neural_rendering;
#[cfg(any(target_arch = "wasm32", target_os = "android", test))]
mod pack_manifest;
pub mod plugins;
mod resource_io;
pub mod text_codec;
#[cfg(target_arch = "wasm32")]
pub mod wasm_assets;
mod window_settings;

use app_state::{AppState, LaunchRequest, LoadedStageInfo};
use bevy::prelude::*;

#[cfg(any(target_os = "android", test))]
fn android_uniform_skinning_wgpu_settings() -> bevy::render::settings::WgpuSettings {
    let mut settings = bevy::render::settings::WgpuSettings::default();
    let mut constrained = settings.limits.clone();
    constrained.max_storage_buffers_per_shader_stage = 0;
    settings.constrained_limits = Some(constrained);
    settings
}

#[cfg(any(target_os = "android", test))]
fn android_render_plugins(plugins: bevy::app::PluginGroupBuilder) -> bevy::app::PluginGroupBuilder {
    plugins
        .set(bevy::render::RenderPlugin {
            render_creation: android_uniform_skinning_wgpu_settings().into(),
            ..default()
        })
        .set(bevy::pbr::PbrPlugin {
            use_gpu_instance_buffer_builder: false,
            ..default()
        })
        .add(CpuMeshPreprocessingPlugin)
}

#[cfg(any(
    target_os = "android",
    all(target_os = "windows", feature = "angle-dx11"),
    test
))]
struct CpuMeshPreprocessingPlugin;

#[cfg(any(
    target_os = "android",
    all(target_os = "windows", feature = "angle-dx11"),
    test
))]
impl Plugin for CpuMeshPreprocessingPlugin {
    fn build(&self, app: &mut App) {
        use bevy::render::{
            batching::gpu_preprocessing::{GpuPreprocessingMode, GpuPreprocessingSupport},
            init_gpu_resource, RenderApp, RenderStartup,
        };

        fn disable_gpu_preprocessing(mut support: ResMut<GpuPreprocessingSupport>) {
            support.max_supported_mode = GpuPreprocessingMode::None;
        }

        app.sub_app_mut(RenderApp)
            .insert_resource(GpuPreprocessingSupport {
                max_supported_mode: GpuPreprocessingMode::None,
            })
            .add_systems(
                RenderStartup,
                disable_gpu_preprocessing.after(init_gpu_resource::<GpuPreprocessingSupport>),
            );
    }
}

#[cfg(target_os = "android")]
fn log_android_skinning_buffer_mode(device: Res<bevy::render::renderer::RenderDevice>) {
    info!(
        "android skinning buffer mode: max_storage_buffers_per_shader_stage={} uniform_skinning={}",
        device.limits().max_storage_buffers_per_shader_stage,
        bevy::pbr::skins_use_uniform_buffers(&device.limits()),
    );
}

pub struct MeteorPlugin;

impl Plugin for MeteorPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppState>()
            .insert_resource(LaunchRequest::default())
            .insert_resource(LoadedStageInfo::default())
            .insert_resource(app_state::RoomState::default())
            .add_systems(OnEnter(AppState::Menu), || {
                meteor_asset::runtime_assets::retain_stage("")
            });
        #[cfg(target_os = "android")]
        android_assets::install(app);
        plugins::install_frame_clock(app);
        plugins::configure_fighter_frame_sets(app);
        plugins::fighter_frame::install(app);
        app.add_plugins(plugins::AssetPlugin)
            .add_plugins(plugins::UiScalePlugin)
            .add_plugins(plugins::UiComponentsPlugin)
            .add_plugins(plugins::IflPlugin)
            .add_plugins(plugins::StagePlugin)
            .add_plugins(plugins::AnimPlugin)
            .add_plugins(plugins::PhysicsPlugin)
            .add_plugins(plugins::CombatPlugin)
            .add_plugins(plugins::RangedPlugin)
            .add_plugins(plugins::AiPlugin)
            .add_plugins(plugins::InputPlugin)
            .add_plugins(plugins::MobileControlsPlugin)
            .add_plugins(plugins::ItemPlugin)
            .add_plugins(plugins::MovementPlugin)
            .add_plugins(plugins::CharacterContactPlugin)
            .add_plugins(plugins::CharacterPlugin)
            .add_plugins(plugins::WeaponTrailPlugin)
            .add_plugins(plugins::CameraPlugin)
            .add_plugins(plugins::DisplaySettingsPlugin)
            .add_plugins(plugins::ImpactFxPlugin)
            .add_plugins(plugins::EffectPlugin)
            .add_plugins(plugins::NetworkPlugin)
            .add_plugins(plugins::AudioPlugin)
            .add_plugins(plugins::ReplayPlugin)
            .add_plugins(plugins::RenderPlugin)
            .add_plugins(plugins::UiPlugin)
            .add_plugins(plugins::DebugPlugin);
    }
}

pub fn build_app() -> App {
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    if !meteor_asset::runtime_assets::original_directory_active() {
        let root = meteor_asset::pack_repository::default_pack_directory().expect("MPK directory");
        meteor_asset::runtime_assets::initialize_native(&root).unwrap_or_else(|error| {
            panic!("{error}; run npm --prefix asset-tools run build:packs and set METEOR_PACKS_DIR")
        });
    }
    #[cfg(target_os = "android")]
    android_assets::bootstrap().expect("required startup MPK");

    let (settings, keymap) = app_state::load_settings();
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    let graphics_settings = graphics_settings::renderer_settings(&settings);
    let mut app = App::new();
    #[cfg(all(
        feature = "dlss5",
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc"
    ))]
    let graphics_settings = neural_rendering::install(&mut app, graphics_settings);
    app.insert_resource(settings).insert_resource(keymap);
    let default_plugins = DefaultPlugins
        .set(bevy::asset::AssetPlugin {
            file_path: "packs".into(),
            ..default()
        })
        .set(WindowPlugin {
            primary_window: Some(Window {
                title: "流星再世".into(),
                resolution: (1280_u32, 720_u32).into(),

                present_mode: bevy::window::PresentMode::AutoNoVsync,

                #[cfg(target_arch = "wasm32")]
                canvas: Some("#bevy".into()),
                #[cfg(target_arch = "wasm32")]
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        });
    #[cfg(all(target_os = "windows", feature = "angle-dx11"))]
    let default_plugins = if graphics_settings::angle_may_be_selected(&graphics_settings) {
        default_plugins
            .set(bevy::pbr::PbrPlugin {
                use_gpu_instance_buffer_builder: false,
                ..default()
            })
            .add(CpuMeshPreprocessingPlugin)
    } else {
        default_plugins
    };
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    let default_plugins = default_plugins.set(bevy::render::RenderPlugin {
        render_creation: graphics_settings.into(),
        ..default()
    });
    #[cfg(target_os = "android")]
    let default_plugins = android_render_plugins(default_plugins);
    app.add_plugins(default_plugins.disable::<bevy::audio::AudioPlugin>());
    #[cfg(target_os = "macos")]
    app.add_systems(Startup, desktop_icon::install);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    app.add_systems(Update, desktop_icon::install);
    #[cfg(target_os = "android")]
    app.add_systems(Startup, log_android_skinning_buffer_mode);
    app.add_plugins(MeteorPlugin);
    app.add_systems(Update, graphics_settings::update_current_renderer);
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    app.add_systems(Update, window_settings::sync_window_settings);
    plugins::server_route::install(&mut app);
    app.insert_state(AppState::StudioSplash)
        .add_plugins(plugins::studio_splash::StudioSplashPlugin);
    #[cfg(target_arch = "wasm32")]
    app.add_systems(Last, notify_browser_menu_ready);
    plugins::frame_work::install(&mut app);
    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(plugins::perf_plugin::PerfPlugin);
    app
}

#[cfg(target_arch = "wasm32")]
fn notify_browser_menu_ready(
    menu: Query<(), With<app_state::MenuRoot>>,
    mut notified: Local<bool>,
) {
    if !*notified && !menu.is_empty() {
        #[wasm_bindgen::prelude::wasm_bindgen(
            inline_js = "export function meteorMenuReady() { window.dispatchEvent(new Event('meteor-ready')); }"
        )]
        extern "C" {
            fn meteorMenuReady();
        }
        meteorMenuReady();
        *notified = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_renderer_forces_bevy_uniform_skinning_path() {
        let settings = android_uniform_skinning_wgpu_settings();
        assert_eq!(
            settings
                .constrained_limits
                .expect("Android renderer must constrain WGPU limits")
                .max_storage_buffers_per_shader_stage,
            0
        );
    }
}
