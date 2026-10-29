use crate::app_state::AppState;
use crate::plugins::camera_plugin::GameplayCamera;
use bevy::anti_alias::taa::TemporalAntiAliasing;
use bevy::prelude::*;
use bevy::render::settings::{Backends, WgpuSettings};
use bevy_dlss5::{
    ArchTier, DlssNeuralRendering, DlssNeuralRenderingPlugin, DlssNeuralRenderingStatus,
};
use std::path::{Path, PathBuf};

#[derive(Resource)]
struct NeuralControl {
    enabled: bool,
    ready: bool,
    label: String,
}

#[derive(Component)]
struct NeuralLabel;

pub(crate) fn install(app: &mut App, original: WgpuSettings) -> WgpuSettings {
    let requested = std::env::var("METEOR_DLSS5").as_deref() != Ok("0");
    let prepared = requested.then(|| prepare(&original));
    let (settings, ready, label) = match prepared {
        Some(Ok((settings, dll))) => {
            app.add_plugins(DlssNeuralRenderingPlugin::with_dll(dll));
            (settings, true, "DLSS 5 NR：等待场景（F8 开关）".to_owned())
        }
        Some(Err(reason)) => (original, false, format!("DLSS 5 NR：不可用 — {reason}")),
        None => (
            original,
            false,
            "DLSS 5 NR：未启动（METEOR_DLSS5=0）".to_owned(),
        ),
    };
    app.insert_resource(NeuralControl {
        enabled: ready,
        ready,
        label,
    })
    .add_systems(Startup, setup_label)
    .add_systems(PostUpdate, (sync_cameras, update_label).chain());
    settings
}

fn runtime_path(root: &Path, explicit: Option<PathBuf>, adapter: &str) -> Result<PathBuf, String> {
    let path = if let Some(path) = explicit {
        path
    } else if root.join("nvngx_dlssnr.dll").is_file() {
        root.join("nvngx_dlssnr.dll")
    } else {
        let tier = ArchTier::detect(adapter).ok_or("显卡架构未识别")?;
        root.join("dlssnr")
            .join(tier.folder())
            .join("nvngx_dlssnr.dll")
    };
    if !path.is_file() {
        return Err(format!("缺少运行库文件 {}", path.display()));
    }
    path.canonicalize()
        .map_err(|_| format!("缺少运行库 {}", path.display()))
}

fn prepare(original: &WgpuSettings) -> Result<(WgpuSettings, PathBuf), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapters = bevy::tasks::block_on(instance.enumerate_adapters(Backends::VULKAN));
    let infos: Vec<_> = adapters
        .iter()
        .map(|adapter| adapter.get_info())
        .filter(|info| info.vendor == 0x10de)
        .collect();
    let info = infos
        .iter()
        .find(|info| original.adapter_name.as_ref() == Some(&info.name))
        .or_else(|| infos.first())
        .ok_or("未找到 NVIDIA Vulkan 设备")?;
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let dll = runtime_path(
        exe.parent().ok_or("无法定位程序目录")?,
        std::env::var_os("METEOR_DLSS5_DLL").map(PathBuf::from),
        &info.name,
    )?;
    if !exe
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("nvngx.dll"))
    {
        return Err("请使用 run-dlss5.ps1 启动（插件要求进程名 nvngx.dll）".into());
    }

    let mut settings = WgpuSettings::default();
    settings.backends = Some(Backends::VULKAN);
    settings.adapter_name = Some(info.name.clone());
    Ok((settings, dll))
}

fn setup_label(
    mut commands: Commands,
    control: Res<NeuralControl>,
    font: Res<crate::plugins::launcher_ui::UiFont>,
) {
    info!("{}", control.label);
    commands.spawn((
        NeuralLabel,
        Text::new(control.label.clone()),
        TextFont {
            font: font.0.clone().into(),
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
        Node {
            position_type: PositionType::Absolute,
            left: px(8),
            bottom: px(8),
            max_width: percent(95),
            ..default()
        },
        GlobalZIndex(100),
    ));
}

fn scene_active(state: AppState) -> bool {
    matches!(
        state,
        AppState::Playing | AppState::Paused | AppState::Result
    )
}

fn sync_cameras(
    mut commands: Commands,
    mut control: ResMut<NeuralControl>,
    status: Option<Res<DlssNeuralRenderingStatus>>,
    state: Res<State<AppState>>,
    keys: Res<ButtonInput<KeyCode>>,
    console: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    mut cameras: Query<
        (
            Entity,
            &Transform,
            Option<&mut DlssNeuralRendering>,
            Option<&mut TemporalAntiAliasing>,
        ),
        With<GameplayCamera>,
    >,
    mut previous: Local<Option<(AppState, Transform)>>,
) {
    if !control.ready {
        return;
    }
    if status
        .as_ref()
        .is_some_and(|status| status.is_unavailable())
    {
        control.enabled = false;
        control.ready = false;
        control.label = "DLSS 5 NR：运行库执行失败，已回退（详情见日志）".into();
        warn!("{}", control.label);
    } else if keys.just_pressed(KeyCode::F8) && !console.is_some_and(|c| c.captures_input()) {
        control.enabled = !control.enabled;
    }
    let active = control.enabled && scene_active(*state.get());
    for (entity, transform, neural, taa) in &mut cameras {
        let cut = previous.as_ref().is_none_or(|(old_state, old)| {
            old_state != state.get()
                || old.translation.distance(transform.translation) > 5.0
                || old.rotation.angle_between(transform.rotation) > 0.75
        });
        if active {
            if let Some(mut neural) = neural {
                if cut {
                    neural.reset = true;
                }
            } else {
                commands.entity(entity).insert((
                    DlssNeuralRendering {
                        reset: true,
                        ..default()
                    },
                    TemporalAntiAliasing::default(),
                    Msaa::Off,
                ));
            }
            if let Some(mut taa) = taa {
                if cut {
                    taa.reset = true;
                }
            }
        } else if neural.is_some() {
            commands.entity(entity).remove::<DlssNeuralRendering>();
        }
        *previous = Some((*state.get(), *transform));
    }
    if control.ready {
        control.label = if !control.enabled {
            "DLSS 5 NR：已关闭（F8 开启）"
        } else if !active {
            "DLSS 5 NR：等待场景（F8 开关）"
        } else if status.is_some_and(|status| status.is_active()) {
            "DLSS 5 NR：已启用 · Vulkan（F8 关闭）"
        } else {
            "DLSS 5 NR：初始化中（F8 关闭）"
        }
        .into();
    }
}

fn update_label(control: Res<NeuralControl>, mut labels: Query<&mut Text, With<NeuralLabel>>) {
    for mut label in &mut labels {
        if label.0 != control.label {
            label.0.clone_from(&control.label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_affects_only_game_camera_and_reenable_resets_history() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .insert_state(AppState::Playing)
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(NeuralControl {
                enabled: true,
                ready: true,
                label: String::new(),
            })
            .add_systems(PostUpdate, sync_cameras);
        let game = app
            .world_mut()
            .spawn((GameplayCamera, Camera3d::default(), Transform::default()))
            .id();
        let ui = app.world_mut().spawn(Camera2d).id();
        app.update();
        assert!(app.world().get::<DlssNeuralRendering>(game).is_some());
        assert_eq!(*app.world().get::<Msaa>(game).unwrap(), Msaa::Off);
        assert!(app.world().get::<DlssNeuralRendering>(ui).is_none());
        app.world_mut().resource_mut::<NeuralControl>().enabled = false;
        app.update();
        assert!(app.world().get::<DlssNeuralRendering>(game).is_none());
        app.world_mut().resource_mut::<NeuralControl>().enabled = true;
        app.update();
        assert!(app.world().get::<DlssNeuralRendering>(game).unwrap().reset);
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Menu);
        app.update();
        assert!(app.world().get::<DlssNeuralRendering>(game).is_none());
    }

    #[test]
    fn neural_rendering_only_runs_on_visible_game_scenes() {
        for state in [AppState::Playing, AppState::Paused, AppState::Result] {
            assert!(scene_active(state));
        }
        for state in [
            AppState::Menu,
            AppState::Loading,
            AppState::Settings,
            AppState::StudioSplash,
        ] {
            assert!(!scene_active(state));
        }
    }

    #[test]
    fn missing_explicit_dll_does_not_silently_choose_another_runtime() {
        let root = std::env::temp_dir();
        let path = root.join("meteor-missing-dlss5-runtime-test.dll");
        let error = runtime_path(&root, Some(path.clone()), "NVIDIA GeForce RTX 4070").unwrap_err();
        assert!(error.contains(path.to_str().unwrap()));
    }
}
