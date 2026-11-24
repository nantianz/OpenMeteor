use super::PreviewLibrary;
use crate::app_state::AppState;
use bevy::prelude::*;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    process::{Child, Command, Stdio},
};

const CACHE_VERSION: &str = "corner-v2";

pub(super) fn library() -> PreviewLibrary {
    let custom = super::super::stage_plugin::CustomStages {
        stages: super::super::escort_assets::custom_stage_entries(),
        ..default()
    };
    let manifest = meteor_asset::runtime_assets::manifest()
        .and_then(|text| meteor_asset::pack_manifest::parse_pack_index(&text).ok());
    let root = cache_root();
    let paths = super::super::launcher_ui::available_room_stages(&custom)
        .into_iter()
        .filter_map(|stage| {
            let mut hash = Sha256::new();
            hash.update(CACHE_VERSION);
            if let Some(revision) =
                meteor_asset::runtime_assets::original_stage_preview_revision(&stage.id)
            {
                hash.update(revision);
            } else {
                let manifest = manifest.as_ref()?;
                let id = format!("stage/{}", stage.id);
                let roots = [id, "common".into(), "textures".into()];
                for dependency in
                    meteor_asset::pack_manifest::resolve_pack_order(manifest, &roots).ok()?
                {
                    hash.update(&manifest[&dependency].hash);
                }
            }
            let revision = format!("{:x}", hash.finalize());
            let path = root.join(format!("{}-{}.jpg", stage.id, &revision[..24]));
            Some((stage.id, path))
        })
        .collect();
    PreviewLibrary(paths)
}

fn cache_root() -> PathBuf {
    if let Some(value) = std::env::var_os("METEOR_STAGE_PREVIEW_CACHE") {
        return value.into();
    }
    if let Some(value) = std::env::var_os("METEOR_USER_DATA_DIR") {
        return PathBuf::from(value).join("cache/map-previews");
    }
    #[cfg(target_os = "windows")]
    if let Some(value) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(value).join("OpenMeteor/cache/map-previews");
    }
    if let Some(value) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(value).join("openmeteor/map-previews");
    }
    if let Some(value) = std::env::var_os("HOME") {
        let suffix = if cfg!(target_os = "macos") {
            "Library/Caches/OpenMeteor/map-previews"
        } else {
            ".cache/openmeteor/map-previews"
        };
        return PathBuf::from(value).join(suffix);
    }
    std::env::temp_dir().join("openmeteor/map-previews")
}

#[derive(Resource)]
struct Worker {
    original_dir: Option<PathBuf>,
    child: Option<Child>,
    error_log: Option<(PathBuf, File)>,
}

impl Worker {
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.take_error();
    }

    fn take_error(&mut self) -> Option<String> {
        let (path, mut file) = self.error_log.take()?;
        let error = (|| {
            let start = file.metadata().ok()?.len().saturating_sub(8192);
            file.seek(SeekFrom::Start(start)).ok()?;
            let mut tail = Vec::new();
            file.by_ref().take(8192).read_to_end(&mut tail).ok()?;
            String::from_utf8_lossy(&tail)
                .lines()
                .rev()
                .find_map(|line| line.strip_prefix("map previews: ").map(str::to_owned))
        })();
        drop(file);
        let _ = std::fs::remove_file(path);
        error
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Resource)]
pub(in crate::plugins) struct StartupProgress {
    done: usize,
    total: usize,
    failure: Option<String>,
    running: bool,
}

impl StartupProgress {
    pub(in crate::plugins) fn complete(&self) -> bool {
        self.done == self.total && self.failure.is_none() && !self.running
    }
}

#[derive(Component)]
struct LoadingRoot;
#[derive(Component)]
struct LoadingSpinner;
#[derive(Component)]
struct LoadingProgress;
#[derive(Component)]
struct Retry;

pub(crate) fn install(app: &mut App, original_dir: Option<PathBuf>) {
    let previews = library();
    app.insert_resource(StartupProgress {
        done: count_cached(&previews),
        total: previews.0.len(),
        failure: None,
        running: false,
    })
    .insert_resource(previews)
    .insert_resource(Worker {
        original_dir,
        child: None,
        error_log: None,
    })
    .add_systems(OnEnter(AppState::StudioSplash), start_worker)
    .add_systems(OnEnter(AppState::MapPreviewLoading), setup_loading)
    .add_systems(OnExit(AppState::MapPreviewLoading), cleanup_loading)
    .add_systems(
        Update,
        check_worker.run_if(|state: Res<State<AppState>>| {
            matches!(
                state.get(),
                AppState::StudioSplash | AppState::MapPreviewLoading
            )
        }),
    )
    .add_systems(
        Update,
        (retry_worker, update_loading)
            .chain()
            .run_if(in_state(AppState::MapPreviewLoading)),
    );
}

fn count_cached(previews: &PreviewLibrary) -> usize {
    previews.0.values().filter(|path| path.is_file()).count()
}

fn start_worker(
    previews: Res<PreviewLibrary>,
    mut worker: ResMut<Worker>,
    mut progress: ResMut<StartupProgress>,
) {
    progress.done = count_cached(&previews);
    if progress.done == progress.total {
        progress.failure = None;
        info!("map previews: using all {} cached maps", progress.total);
        return;
    }
    if worker.child.is_some() {
        return;
    }
    let started = std::env::current_exe().and_then(|exe| {
        let root = cache_root();
        std::fs::create_dir_all(&root)?;
        let path = root.join(format!("preview-worker-{}.log", std::process::id()));
        let log = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        let mut command = Command::new(exe);
        let stderr = log.try_clone();
        worker.error_log = Some((path, log));
        command
            .arg("--bake-map-previews")
            .stdin(Stdio::null())
            .stderr(stderr?);

        command.env_remove("METEOR_STAGE_PREVIEW_STAGE");
        if let Some(path) = &worker.original_dir {
            command.arg("--original-dir").arg(path);
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000 | 0x00004000);
        }
        command.spawn()
    });
    match started {
        Ok(child) => {
            worker.child = Some(child);
            progress.failure = None;
            progress.running = true;
            info!(
                "map previews: baking {} missing maps",
                progress.total - progress.done
            );
        }
        Err(error) => {
            warn!("map previews: {error}");
            worker.take_error();
            progress.failure = Some(error.to_string());
        }
    }
}

fn check_worker(
    previews: Res<PreviewLibrary>,
    mut worker: ResMut<Worker>,
    mut progress: ResMut<StartupProgress>,
    time: Res<Time<Real>>,
    mut next_check: Local<f64>,
) {
    let elapsed = time.elapsed_secs_f64();
    if elapsed < *next_check {
        return;
    }
    *next_check = elapsed + 0.15;
    if let Some(child) = worker.child.as_mut() {
        let status = child.try_wait();
        progress.done = count_cached(&previews);
        match status {
            Ok(None) => {}
            Ok(Some(status)) => {
                worker.child = None;
                progress.running = false;
                let detail = worker.take_error();
                progress.failure = (progress.done != progress.total || !status.success())
                    .then(|| detail.unwrap_or_else(|| format!("预览进程退出：{status}")));
                if progress.failure.is_some() {
                    warn!(
                        "map preview worker returned {status}, {}/{} cached",
                        progress.done, progress.total
                    );
                } else {
                    info!("map previews: all {} ready", progress.total);
                }
            }
            Err(error) => {
                warn!("map preview worker: {error}");
                worker.stop();
                progress.running = false;
                progress.failure = Some(error.to_string());
            }
        }
    }
}

fn setup_loading(mut commands: Commands, font: Res<super::super::launcher_ui::UiFont>) {
    use super::super::ui_theme as theme;
    let font = font.0.clone();
    commands
        .spawn((
            LoadingRoot,
            theme::CinematicBackdrop,
            GlobalZIndex(2100),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(theme::BACKGROUND),
        ))
        .with_children(|backdrop| {
            backdrop
                .spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        flex_direction: FlexDirection::Column,
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        row_gap: Val::Px(16.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.016, 0.018, 0.019, 0.86)),
                ))
                .with_children(|root| {
                    let mut ring_color = BorderColor::all(theme::METAL.with_alpha(0.22));
                    ring_color.top = theme::METAL;
                    ring_color.right = theme::METAL.with_alpha(0.65);
                    root.spawn((
                        LoadingSpinner,
                        Node {
                            width: Val::Px(36.0),
                            height: Val::Px(36.0),
                            border: UiRect::all(Val::Px(2.0)),
                            border_radius: BorderRadius::MAX,
                            ..default()
                        },
                        ring_color,
                    ));
                    root.spawn(super::super::launcher_ui::launcher_text(
                        "生成地图预览",
                        24.0,
                        theme::TEXT,
                        &font,
                    ));
                    root.spawn((
                        LoadingProgress,
                        super::super::launcher_ui::launcher_text("", 14.0, theme::METAL, &font),
                    ));
                    root.spawn((
                        Button,
                        Retry,
                        Node {
                            display: Display::None,
                            padding: UiRect::axes(Val::Px(20.0), Val::Px(9.0)),
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                        },
                        BackgroundColor(theme::CARD),
                        BorderColor::all(theme::METAL),
                    ))
                    .with_children(|button| {
                        button.spawn(super::super::launcher_ui::launcher_text(
                            "重试",
                            15.0,
                            theme::TEXT,
                            &font,
                        ));
                    });
                });
        });
}

fn retry_worker(
    buttons: Query<&Interaction, (Changed<Interaction>, With<Retry>)>,
    previews: Res<PreviewLibrary>,
    worker: ResMut<Worker>,
    progress: ResMut<StartupProgress>,
) {
    if progress.failure.is_some() && buttons.iter().any(|state| *state == Interaction::Pressed) {
        start_worker(previews, worker, progress);
    }
}

fn update_loading(
    time: Res<Time<Real>>,
    progress: Res<StartupProgress>,
    mut next: ResMut<NextState<AppState>>,
    mut spinners: Query<(&mut UiTransform, &mut Visibility), With<LoadingSpinner>>,
    mut labels: Query<&mut Text, With<LoadingProgress>>,
    mut buttons: Query<&mut Node, With<Retry>>,
) {
    if progress.complete() {
        next.set(AppState::Menu);
        return;
    }
    let label = if let Some(error) = &progress.failure {
        format!(
            "生成失败  ·  {} / {}\n{error}",
            progress.done, progress.total
        )
    } else {
        format!("{} / {}", progress.done, progress.total)
    };
    for mut text in &mut labels {
        if text.0 != label {
            text.0.clone_from(&label);
        }
    }
    for (mut spin, mut visibility) in &mut spinners {
        spin.rotation = Rot2::radians(time.elapsed_secs() * 4.6);
        visibility.set_if_neq(if progress.failure.is_some() {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        });
    }
    for mut node in &mut buttons {
        let display = if progress.failure.is_some() {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
}

fn cleanup_loading(mut commands: Commands, root: Query<Entity, With<LoadingRoot>>) {
    for entity in &root {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_stays_on_progress_until_worker_has_finished_all_previews() {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(AppState::MapPreviewLoading)
            .init_resource::<Time<Real>>()
            .insert_resource(StartupProgress {
                done: 2,
                total: 3,
                failure: None,
                running: true,
            })
            .add_systems(
                Update,
                update_loading.run_if(in_state(AppState::MapPreviewLoading)),
            );
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::MapPreviewLoading
        );
        app.world_mut().resource_mut::<StartupProgress>().done = 3;
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::MapPreviewLoading
        );
        app.world_mut().resource_mut::<StartupProgress>().running = false;
        app.update();
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Menu
        );
    }
}
