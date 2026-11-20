#[path = "pst_extensions/actors.rs"]
mod actors;

pub(in crate::plugins) use actors::ScriptFighterOptions;
use actors::{
    actor_entity, actor_id, advance_attack_sequences, apply_actor_action, update_script_actors,
};

use std::collections::{BTreeMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Write};
use std::sync::Arc;

use bevy::audio::{AudioPlayer, AudioSource, PlaybackSettings, Volume};
use bevy::prelude::*;
use meteor_physics::collision::Point3;
use meteor_scene::pst_game::{GameRequest, PstGame, ScriptEvent, ScriptStorage};
use meteor_scene::pst_music::PstMusicRequests;
use meteor_scene::pst_runtime::PstValue;
use meteor_scene::pst_startup::StartupScripts;

use super::*;
use crate::plugins::{combat_plugin, pst_ui, render_plugin, stage_collider};

#[derive(Resource, Clone)]
pub(in crate::plugins) struct PstSession(pub PstGame);

impl Default for PstSession {
    fn default() -> Self {
        Self(PstGame::new(ScriptStorage {
            read: read_save,
            write: write_save,
            read_asset: read_installed_script_asset,
        }))
    }
}

#[derive(Resource, Default)]
pub(in crate::plugins) struct GlobalPst(pub StartupScripts);

#[derive(Component)]
struct ScriptSound {
    name: String,
    actor: Option<i64>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<PstSession>()
        .init_resource::<GlobalPst>()
        .add_systems(OnEnter(AppState::Menu), load_startup_scripts)
        .add_systems(
            OnEnter(AppState::Playing),
            begin_script_stage
                .after(super::spawn_local_fighter)
                .before(super::start_stage_script)
                .run_if(crate::plugins::starts_fresh_playing_session),
        )
        .add_systems(
            OnEnter(AppState::Playing),
            capture_stage_geometry
                .after(render_plugin::setup_stage_scene)
                .before(super::start_mission_script)
                .run_if(crate::plugins::starts_fresh_playing_session),
        )
        .add_systems(
            OnExit(AppState::Playing),
            end_script_stage
                .after(super::close_stage_script)
                .run_if(crate::plugins::ends_playing_session),
        )
        .add_systems(PreUpdate, sample_input.after(bevy::input::InputSystems))
        .add_systems(Last, stop_startup_scripts)
        .add_systems(
            Update,
            (
                update_script_actors.before(super::tick_stage_script),
                refresh_stage_geometry
                    .after(render_plugin::sync_fmc_with_script)
                    .before(super::tick_stage_script),
                tick_startup_scripts.after(super::tick_mission_script),
                apply_game_requests
                    .after(tick_startup_scripts)
                    .after(combat_plugin::stage_hazard_damage)
                    .before(super::consume_pst_actor_requests),
                advance_attack_sequences
                    .before(super::drive_player_performance)
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(in_state(AppState::Playing)),
                sync_bound_sounds,
                apply_item_placements
                    .after(super::tick_stage_script)
                    .before(super::scene_script_touch_and_transfer),
            ),
        );
    #[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
    app.add_systems(Update, reload_pst_sources.before(tick_startup_scripts));
}

#[cfg(not(target_arch = "wasm32"))]
fn save_path(slot: &str) -> std::path::PathBuf {
    #[cfg(target_os = "android")]
    if let Some(dir) = bevy::android::ANDROID_APP
        .get()
        .and_then(|app| app.internal_data_path())
    {
        return dir
            .join("UserData")
            .join("Saves")
            .join(format!("{slot}.pst-save"));
    }
    std::path::Path::new("UserData/Saves").join(format!("{slot}.pst-save"))
}

fn read_save(slot: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()?
            .local_storage()
            .ok()??
            .get_item(&format!("meteor.pst.save.{slot}"))
            .ok()?
            .filter(|value| value.len() <= 1024 * 1024)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let file = std::fs::File::open(save_path(slot)).ok()?;
        let mut text = String::new();
        file.take(1024 * 1024 + 1).read_to_string(&mut text).ok()?;
        (text.len() <= 1024 * 1024).then_some(text)
    }
}

fn write_save(slot: &str, data: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|window| window.local_storage().ok().flatten())
            .is_some_and(|storage| {
                storage
                    .set_item(&format!("meteor.pst.save.{slot}"), data)
                    .is_ok()
            })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path = save_path(slot);
        let temporary = path.with_extension("pst-save.tmp");
        let result = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().expect("script save directory"))?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let commit = file
                .write_all(data.as_bytes())
                .and_then(|_| file.sync_all());
            drop(file);
            let commit = commit.and_then(|_| std::fs::rename(&temporary, &path));
            if commit.is_err() {
                let _ = std::fs::remove_file(&temporary);
            }
            commit
        })();
        result.is_ok()
    }
}

pub(in crate::plugins) fn read_script_asset(path: &str) -> Option<Vec<u8>> {
    meteor_asset::runtime_assets::read_legacy_path(path).or_else(|| {
        let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
        resource_roots(&cwd)
            .iter()
            .find_map(|root| meteor_asset::runtime_assets::read_file(root.join(path)).ok())
    })
}

pub(in crate::plugins) fn read_installed_script_asset(path: &str) -> Option<Vec<u8>> {
    meteor_asset::runtime_assets::read_legacy_path(path)
        .or_else(|| meteor_asset::runtime_assets::read_entry(path))
}

fn load_startup_scripts(
    mut commands: Commands,
    game: Res<PstSession>,
    mut startup: ResMut<GlobalPst>,
    mut loaded: Local<bool>,
    old_ui: Option<Res<pst_ui::StagePstUi>>,
    old_music: Option<Res<StageScriptMusic>>,
) {
    if *loaded {
        return;
    }
    let Some(index) = read_script_asset("PScriptLib/Startup/index.txt") else {
        return;
    };
    *loaded = true;
    let ui = old_ui
        .as_ref()
        .and_then(|ui| ui.0.clone())
        .unwrap_or_default();
    let music = old_music.as_ref().map_or_else(
        || PstMusicRequests::new(read_script_asset),
        |music| music.0.clone(),
    );
    commands.insert_resource(pst_ui::StagePstUi(Some(ui.clone())));
    commands.insert_resource(StageScriptMusic(music.clone()));
    let mut seen = HashSet::new();
    let text = crate::text_codec::decode_text(&index);
    for name in text
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.starts_with('#'))
        .take(32)
    {
        if name.len() > 64
            || !name.ends_with(".pst")
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-. ".contains(&byte))
            || !seen.insert(name.to_ascii_lowercase())
        {
            continue;
        }
        let path = format!("PScriptLib/Startup/{name}");
        if let Some(source) = read_script_asset(&path) {
            if let Err(error) = startup.0.add(
                name,
                &crate::text_codec::decode_text(&source),
                game.0.clone(),
                ui.clone(),
                music.clone(),
            ) {
                warn!("{path}: {error}");
            }
        }
    }
    game.0.set_startup_events(startup.0.event_names());
    let event = ScriptEvent {
        name: "OnStart".into(),
        args: vec![],
    };
    if let Err(error) = startup.0.dispatch(None, &[event], None, None) {
        warn!("startup PST: {error}");
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android")))]
fn reload_pst_sources(
    time: Res<Time<Real>>,
    mut next_poll: Local<f64>,
    game: Res<PstSession>,
    mut startup: ResMut<GlobalPst>,
    mut scene: ResMut<StageScriptRuntime>,
    mut mission: ResMut<StageMissionRuntime>,
    loaded: Res<LoadedStageInfo>,
) {
    let now = time.elapsed_secs_f64();
    if (startup.0.is_empty() && !game.0.active())
        || now < *next_poll
        || !meteor_asset::runtime_assets::original_directory_active()
    {
        return;
    }
    *next_poll = now + 0.5;
    let report = |path: &str, result: Result<bool, String>| match result {
        Ok(true) => info!("PST reloaded {path}"),
        Ok(false) => {}
        Err(error) => warn!("{path}: {error}"),
    };
    if game.0.active() {
        let scene_path = format!("{0}/{0}.pst", loaded.stage_id);
        if let (Some(runtime), Some(bytes)) =
            (scene.script_mut(), read_installed_script_asset(&scene_path))
        {
            let source = crate::text_codec::decode_text(&bytes);
            let (source, _, missing) = super::expand_scene_pst_includes(
                &scene_path,
                &source,
                &mut read_installed_script_asset,
            );
            if missing.is_empty() {
                report(&scene_path, runtime.reload_source(&source));
            }
        }
        if let Some(runtime) = mission.script_mut() {
            let path = format!("Level/{}_.pst", loaded.stage_id);
            if let Some(bytes) = read_installed_script_asset(&path) {
                report(
                    &path,
                    runtime.reload_source(&crate::text_codec::decode_text(&bytes)),
                );
            }
            for (id, name) in runtime.npc_source_names() {
                let path = format!("Level/{name}.pst");
                if let Some(bytes) = read_installed_script_asset(&path) {
                    report(
                        &path,
                        runtime.reload_npc_source(id, &crate::text_codec::decode_text(&bytes)),
                    );
                }
            }
        }
    }
    let names: Vec<_> = startup.0.names().map(str::to_owned).collect();
    for (index, name) in names.into_iter().enumerate() {
        let path = format!("PScriptLib/Startup/{name}");
        let Some(bytes) = read_script_asset(&path) else {
            continue;
        };
        let (scene, mission) = if game.0.active() {
            (scene.script_mut(), mission.script_mut())
        } else {
            (None, None)
        };
        match startup.0.reload(
            index,
            &crate::text_codec::decode_text(&bytes),
            scene,
            mission,
        ) {
            Ok(true) => {
                game.0.set_startup_events(startup.0.event_names());
                info!("PST reloaded {path}");
            }
            Ok(false) => {}
            Err(error) => warn!("{path}: {error}"),
        }
    }
}

fn begin_script_stage(world: &mut World) {
    let game = world.resource::<PstSession>().0.clone();
    {
        let mut net = world.resource_mut::<crate::plugins::network_plugin::NetState>();
        net.offline_script_player_team = None;
        net.offline_script_player_name = None;
    }
    let offline = {
        let request = world.resource::<LaunchRequest>();
        should_load_mission_script(request)
            && world
                .resource::<crate::plugins::network_plugin::NetState>()
                .status
                != crate::plugins::network_plugin::NetStatus::InRoom
    };
    game.begin_stage(offline);
    if !offline {
        return;
    }
    if let (Some(ui), Some(music)) = (
        world
            .get_resource::<pst_ui::StagePstUi>()
            .and_then(|ui| ui.0.clone()),
        world
            .get_resource::<StageScriptMusic>()
            .map(|music| music.0.clone()),
    ) {
        world
            .resource_mut::<GlobalPst>()
            .0
            .update_channels(ui, music);
    }
    update_script_actors(world);
    game.publish("Level_OnStart", &[]);
}

fn capture_stage_geometry(
    game: Res<PstSession>,
    loaded: Res<LoadedStageInfo>,
    collider: Option<Res<stage_collider::StageCollider>>,
    hazards: Option<Res<crate::plugins::hazard_plugin::StageHazardRuntime>>,
) {
    if !game.0.active() {
        return;
    }
    if let Some(mesh) = collider
        .as_deref()
        .and_then(|collider| collider.mesh.as_ref())
    {
        set_stage_geometry(&game.0, mesh, loaded.center, hazards.as_deref());
    }
}

fn refresh_stage_geometry(
    game: Res<PstSession>,
    loaded: Res<LoadedStageInfo>,
    collider: Option<Res<stage_collider::StageCollider>>,
    hazards: Option<Res<crate::plugins::hazard_plugin::StageHazardRuntime>>,
    mut revision: Local<Option<u64>>,
) {
    if !game.0.raycast_used() {
        *revision = None;
        return;
    }
    if let Some(mesh) = collider
        .as_deref()
        .and_then(|collider| collider.mesh.as_ref())
    {
        if *revision != Some(mesh.geometry_revision()) {
            *revision = Some(mesh.geometry_revision());
            set_stage_geometry(&game.0, mesh, loaded.center, hazards.as_deref());
        }
    }
}

fn set_stage_geometry(
    game: &PstGame,
    mesh: &stage_collider::StageMeshCollider,
    center: [f32; 3],
    hazards: Option<&crate::plugins::hazard_plugin::StageHazardRuntime>,
) {
    use meteor_physics::collision::MeshCollider;
    let bounds = hazards
        .and_then(|hazards| hazards.0.world_bounds.as_ref())
        .map(|box_| {
            let a = Vec3::from_array(world_to_source(Vec3::from_array(box_.min), center));
            let b = Vec3::from_array(world_to_source(Vec3::from_array(box_.max), center));
            [a.min(b).to_array(), a.max(b).to_array()]
        });
    let mesh = mesh.clone();
    game.set_map(
        bounds,
        Some(Arc::new(move |a, b| {
            let a = source_to_world(a, center);
            let b = source_to_world(b, center);
            let delta = b - a;
            let direction = delta.try_normalize()?;
            let hit = mesh.raycast_hit(
                Point3::new(a.x, a.y, a.z),
                Point3::new(direction.x, direction.y, direction.z),
                delta.length(),
            )?;
            Some(world_to_source(a + direction * hit.t, center))
        })),
    );
}

fn stop_startup_scripts(
    mut exit: MessageReader<AppExit>,
    mut startup: ResMut<GlobalPst>,
    mut scene: ResMut<StageScriptRuntime>,
    mut mission: ResMut<StageMissionRuntime>,
) {
    if exit.read().next().is_none() || startup.0.is_empty() {
        return;
    }
    let event = ScriptEvent {
        name: "OnStop".into(),
        args: vec![],
    };
    if let Err(error) = startup
        .0
        .dispatch(None, &[event], scene.script_mut(), mission.script_mut())
    {
        warn!("startup PST quit: {error}");
    }
}

fn end_script_stage(
    game: Res<PstSession>,
    mut startup: ResMut<GlobalPst>,
    mut scene: ResMut<StageScriptRuntime>,
    mut mission: ResMut<StageMissionRuntime>,
) {
    if !startup.0.is_empty() {
        let mut events = game.0.take_events();
        events.push(ScriptEvent {
            name: "Level_OnStop".into(),
            args: vec![],
        });
        if let Err(error) =
            startup
                .0
                .dispatch(None, &events, scene.script_mut(), mission.script_mut())
        {
            warn!("startup PST close: {error}");
        }
    }
    game.0.begin_stage(false);
}

fn sample_input(
    game: Res<PstSession>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    time: Res<Time>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    let name = |key: &KeyCode| {
        let name = format!("{key:?}");
        name.strip_prefix("Key")
            .or_else(|| name.strip_prefix("Digit"))
            .unwrap_or(&name)
            .to_ascii_uppercase()
    };
    let mut pressed = keys.as_ref().map_or_else(HashSet::new, |keys| {
        keys.get_just_pressed().map(name).collect()
    });
    let mut down = keys
        .as_ref()
        .map_or_else(HashSet::new, |keys| keys.get_pressed().map(name).collect());
    if let Some(mouse) = mouse {
        for button in mouse.get_just_pressed() {
            pressed.insert(format!("MOUSE{button:?}").to_ascii_uppercase());
        }
        for button in mouse.get_pressed() {
            down.insert(format!("MOUSE{button:?}").to_ascii_uppercase());
        }
    }
    let size = windows
        .single()
        .map_or([0.0; 2], |window| [window.width(), window.height()]);
    game.0
        .set_input(pressed, down, size, time.elapsed_secs_f64());
}

pub(in crate::plugins) fn source_to_world(point: [f32; 3], center: [f32; 3]) -> Vec3 {
    Vec3::new(
        point[0] - center[0],
        point[2] - center[2],
        center[1] - point[1],
    ) * meteor_core::MODEL_SCALE
}

pub(in crate::plugins) fn world_to_source(point: Vec3, center: [f32; 3]) -> [f32; 3] {
    let point = point / meteor_core::MODEL_SCALE;
    [
        point.x + center[0],
        center[1] - point.z,
        point.y + center[2],
    ]
}

fn apply_item_placements(
    mut scene: ResMut<StageScriptRuntime>,
    stage: Res<LoadedStageInfo>,
    game: Res<PstSession>,
    mut collider: Option<ResMut<stage_collider::StageCollider>>,
    mut props: Query<(
        &crate::plugins::item_plugin::StageProp,
        &mut Transform,
        Option<&render_plugin::PropColliderOwner>,
        Option<&mut render_plugin::FmcClip>,
    )>,
) {
    if !game.0.active() {
        return;
    }
    let Some(runtime) = scene.script_mut() else {
        return;
    };
    let placements = runtime.host_mut().take_item_placements();
    if placements.is_empty() {
        return;
    }
    for (name, position, yaw) in placements {
        for (prop, mut transform, owner, vertex_clip) in &mut props {
            if !prop.name.eq_ignore_ascii_case(&name) {
                continue;
            }
            let (previous, previous_rotation) = vertex_clip
                .as_ref()
                .map_or((transform.translation, transform.rotation), |clip| {
                    (clip.base_translation, clip.base_rotation)
                });
            let position =
                position.map_or(previous, |position| source_to_world(position, stage.center));
            let forward = previous_rotation * Vec3::NEG_Y;
            let delta = yaw.map_or(Quat::IDENTITY, |yaw| {
                Quat::from_rotation_y(yaw.to_radians() - forward.x.atan2(forward.z))
            });
            transform.translation = position + delta * (transform.translation - previous);
            transform.rotation = delta * transform.rotation;
            if let Some(mut clip) = vertex_clip {
                clip.base_translation = position;
                clip.base_rotation = delta * previous_rotation;
            }
            if let (Some(owner), Some(mesh)) = (
                owner,
                collider
                    .as_deref_mut()
                    .and_then(|collider| collider.mesh.as_mut()),
            ) {
                mesh.place_owner(owner.0, previous, position, delta);
            }
        }
    }
}

fn tick_startup_scripts(
    state: Res<State<AppState>>,
    game: Res<PstSession>,
    mut startup: ResMut<GlobalPst>,
    mut scene: ResMut<StageScriptRuntime>,
    mut mission: ResMut<StageMissionRuntime>,
    end: Option<Res<crate::components::fighter::MatchEnd>>,
    mut net: Option<ResMut<crate::plugins::network_plugin::NetState>>,
) {
    let active =
        *state.get() == AppState::Playing && game.0.active() && !end.is_some_and(|end| end.settled);
    if active {
        if let Some(runtime) = scene.script_mut() {
            if let Err(error) = runtime.run_due(mission.script_mut()) {
                warn!("scene PST timer: {error}");
            }
        }
        if let Some(runtime) = mission.script_mut() {
            if let Err(error) = runtime.run_due(scene.script_mut()) {
                warn!("mission PST timer: {error}");
            }
        }
    }
    let chat = net.as_deref_mut().map_or_else(Default::default, |net| {
        std::mem::take(&mut net.pst_chat_messages)
    });
    if startup.0.is_empty() || !(active || *state.get() == AppState::Menu || !chat.is_empty()) {
        return;
    }
    let mut events = game.0.take_events();
    events.extend(chat.into_iter().map(|text| ScriptEvent {
        name: "OnChat".into(),
        args: vec![PstValue::Str(text), PstValue::Num(0.0)],
    }));
    if active || *state.get() == AppState::Menu {
        events.push(ScriptEvent {
            name: "OnUpdate".into(),
            args: vec![],
        });
    }
    let result = if active {
        startup
            .0
            .dispatch(None, &events, scene.script_mut(), mission.script_mut())
    } else {
        startup.0.dispatch(None, &events, None, None)
    };
    if let Err(error) = result {
        warn!("startup PST: {error}");
    }
}

fn apply_game_requests(world: &mut World) {
    let requests = world.resource::<PstSession>().0.take_requests();
    if requests.is_empty()
        || world
            .resource::<crate::plugins::network_plugin::NetState>()
            .status
            == crate::plugins::network_plugin::NetStatus::InRoom
    {
        return;
    }
    let playing = *world.resource::<State<AppState>>().get() == AppState::Playing;
    let center = world.resource::<LoadedStageInfo>().center;
    for request in requests {
        match request {
            GameRequest::Enter(id) => {
                let stage = format!("sn{id:02}");
                if let Some(mission) = crate::plugins::campaign_ui::mission(&stage) {
                    let mut request = world.resource_mut::<LaunchRequest>();
                    request.stage_id = stage;
                    request.stage_name = mission.name.into();
                    request.mode = "single".into();
                    request.character_id = 0;
                    request.spawn_room_npcs = false;
                    world
                        .resource_mut::<NextState<AppState>>()
                        .set(AppState::Loading);
                    break;
                }
            }
            GameRequest::Exit if playing => {
                world
                    .resource_mut::<NextState<AppState>>()
                    .set(AppState::Menu);
                break;
            }
            GameRequest::Actor(id, action) if playing => {
                if let Some(entity) = actor_entity(world, id) {
                    apply_actor_action(world, entity, id, action, center);
                }
            }
            GameRequest::TempItem {
                actor,
                resource,
                position,
                yaw,
                thrown,
            } if playing => {
                let position_yaw = if thrown {
                    yaw.or_else(|| {
                        actor_entity(world, actor)
                            .and_then(|entity| world.get::<MotionState>(entity))
                            .map(|motion| motion.facing.to_degrees())
                    })
                } else {
                    None
                };
                let Some(model) = item_model(world, resource) else {
                    continue;
                };
                if let Some(runtime) = world.resource_mut::<StageScriptRuntime>().script_mut() {
                    runtime.host_mut().create_temp_item(
                        actor,
                        resource,
                        &model,
                        position,
                        position_yaw,
                    );
                }
            }
            GameRequest::SoundPlay(name, path, looping, spatial) if playing => {
                play_sound(world, name, &path, looping, spatial)
            }
            GameRequest::SoundStop(name) => {
                if let Some(entity) = sound_entity(world, &name) {
                    world.despawn(entity);
                }
            }
            GameRequest::SoundPosition(name, position) => {
                if let Some(entity) = sound_entity(world, &name) {
                    if let Some(mut sound) = world.get_mut::<ScriptSound>(entity) {
                        sound.actor = None;
                    }
                    if let Some(mut transform) = world.get_mut::<Transform>(entity) {
                        transform.translation = source_to_world(position, center);
                    }
                }
            }
            GameRequest::SoundBind(name, actor) => {
                if let Some(entity) = sound_entity(world, &name) {
                    if let Some(mut sound) = world.get_mut::<ScriptSound>(entity) {
                        sound.actor = Some(actor);
                    }
                }
            }
            GameRequest::SoundUnbind(name) => {
                if let Some(entity) = sound_entity(world, &name) {
                    if let Some(mut sound) = world.get_mut::<ScriptSound>(entity) {
                        sound.actor = None;
                    }
                }
            }
            GameRequest::Notice(text) if playing => {
                if let Some(mut feed) =
                    world.get_resource_mut::<crate::plugins::system_notice::SystemNoticeFeed>()
                {
                    feed.push(text);
                }
            }
            _ => {}
        }
    }
}

#[derive(Resource, Default)]
struct ScriptItemModels(Option<BTreeMap<[i64; 2], String>>);

fn item_model(world: &mut World, key: [i64; 2]) -> Option<String> {
    let mut cache = world.get_resource_or_init::<ScriptItemModels>();
    let items = cache.0.get_or_insert_with(|| {
        read_script_asset("Meteor.res")
            .and_then(|bytes| meteor_asset::res::parse_meteor_res(&bytes).ok())
            .into_iter()
            .flat_map(|table| table.records)
            .filter(|record| matches!(record.category, 1 | 2 | 4))
            .map(|record| {
                (
                    [i64::from(record.category), i64::from(record.id)],
                    record.name,
                )
            })
            .collect()
    });
    items.get(&key).cloned()
}

fn sound_entity(world: &mut World, name: &str) -> Option<Entity> {
    world
        .query::<(Entity, &ScriptSound)>()
        .iter(world)
        .find_map(|(entity, sound)| (sound.name == name).then_some(entity))
}

fn play_sound(world: &mut World, name: String, path: &str, looping: bool, spatial: bool) {
    let Some(bytes) = read_script_asset(path).filter(|bytes| {
        bytes.len() <= 16 * 1024 * 1024
            && bytes.get(..4) == Some(b"RIFF")
            && bytes.get(8..12) == Some(b"WAVE")
    }) else {
        return;
    };
    let previous = sound_entity(world, &name);
    if previous.is_none() && world.query::<&ScriptSound>().iter(world).count() >= 32 {
        return;
    }
    let (actor, position) = previous.map_or((None, Vec3::ZERO), |entity| {
        (
            world
                .get::<ScriptSound>(entity)
                .and_then(|sound| sound.actor),
            world
                .get::<Transform>(entity)
                .map_or(Vec3::ZERO, |transform| transform.translation),
        )
    });
    if let Some(previous) = previous {
        world.despawn(previous);
    }
    let volume = world
        .resource::<crate::app_state::Settings>()
        .audio_volume_linear();
    let handle = world
        .resource_mut::<Assets<AudioSource>>()
        .add(AudioSource {
            bytes: bytes.into(),
        });
    let settings = (if looping {
        PlaybackSettings::LOOP
    } else {
        PlaybackSettings::DESPAWN
    })
    .with_spatial(spatial)
    .with_volume(Volume::Linear(volume));
    world.spawn((
        AudioPlayer::new(handle),
        settings,
        Transform::from_translation(position),
        ScriptSound { name, actor },
        render_plugin::PlayingSceneEntity,
    ));
}

fn sync_bound_sounds(
    mut sounds: Query<(&ScriptSound, &mut Transform), Without<Fighter>>,
    actors: Query<(&Transform, Option<&LocalFighter>, Option<&MissionNpc>), With<Fighter>>,
) {
    for (sound, mut position) in &mut sounds {
        if let Some(actor) = sound.actor {
            if let Some((transform, _, _)) = actors
                .iter()
                .find(|(_, local, npc)| actor_id(*local, *npc) == Some(actor))
            {
                position.translation = transform.translation;
            }
        }
    }
}

#[cfg(all(test, not(any(target_arch = "wasm32", target_os = "android"))))]
#[path = "pst_extensions/tests.rs"]
mod tests;
