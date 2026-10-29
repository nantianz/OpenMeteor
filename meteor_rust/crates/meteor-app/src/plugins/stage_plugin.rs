#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "stage_plugin/scene_sync_tests.rs"]
mod scene_sync_tests;

use crate::app_state::{AppState, LaunchRequest, LoadedStageAssets, LoadedStageInfo};
use crate::components::fighter::{CombatState, Fighter, Health, MotionState, Qi};
use bevy::asset::RenderAssetUsages;
use bevy::material::AlphaMode;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use meteor_core::MovementMode;
use meteor_scene::mission_runtime::{MissionScriptRuntime, NpcRequest};
use meteor_scene::scene_script_runtime::{
    PstValue, SceneItemInstance, SceneNetOp, SceneScriptRuntime, SceneTransferRequest,
};
use meteor_scene::stage_loader::{
    get_stage_center, load_stage_assets, StageAssetSource, WaypointGraph,
};
use serde_json::{Number as JsonNumber, Value as JsonValue};
use std::path::{Path, PathBuf};

#[derive(Resource, Default)]
pub struct StageScriptRuntime(Option<SceneScriptRuntime>);

impl StageScriptRuntime {
    #[cfg(test)]
    pub(crate) fn from_script(script: SceneScriptRuntime) -> Self {
        Self(Some(script))
    }

    pub fn script(&self) -> Option<&SceneScriptRuntime> {
        self.0.as_ref()
    }

    pub fn script_mut(&mut self) -> Option<&mut SceneScriptRuntime> {
        self.0.as_mut()
    }
}

#[derive(Resource, Default)]
struct SceneTouchState {
    now: f32,
    next_scan: f32,
    cooldowns: std::collections::HashMap<(String, i64), f32>,
}

const SCENE_SNOW_AREA_XZ: f32 = 90.0;
const SCENE_SNOW_AREA_Y: f32 = 40.0;
const SCENE_SNOW_WORLD_SCALE: f32 = 0.03;
const SCENE_SNOW_MAX_PARTICLES: usize = 4_000;

#[derive(Debug, Clone)]
struct SceneSnowConfig {
    enabled: bool,
    density: usize,
    wind: Vec3,
    speed_min: f32,
    speed_max: f32,
    size: f32,
}

impl Default for SceneSnowConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            density: 200,
            wind: Vec3::ZERO,
            speed_min: 20.0,
            speed_max: 100.0,
            size: 5.0,
        }
    }
}

impl SceneSnowConfig {
    fn apply(&mut self, key: &str, args: &[f64]) -> bool {
        match key {
            "snow" => self.enabled = args.first().is_some_and(|value| *value != 0.0),
            "snowdensity" => {
                self.density = args
                    .first()
                    .copied()
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0)
                    .trunc()
                    .max(0.0) as usize;
            }
            "winddir" => {
                self.wind = Vec3::new(
                    snow_arg(args, 0, 0.0),
                    snow_arg(args, 1, 0.0),
                    snow_arg(args, 2, 0.0),
                );
            }
            "snowspeed" => {
                self.speed_min = snow_arg(args, 0, 20.0);
                self.speed_max = snow_arg(args, 1, self.speed_min);
            }
            "snowsize" => self.size = snow_arg(args, 0, 5.0),
            _ => return false,
        }
        true
    }

    fn particle_count(&self) -> usize {
        self.density.min(SCENE_SNOW_MAX_PARTICLES)
    }

    fn flake_size(&self) -> f32 {
        (self.size * SCENE_SNOW_WORLD_SCALE).max(0.06)
    }
}

fn snow_arg(args: &[f64], index: usize, fallback: f32) -> f32 {
    let value = args.get(index).copied().unwrap_or_default() as f32;
    if value.is_finite() && value != 0.0 {
        value
    } else {
        fallback
    }
}

#[derive(Component)]
struct SceneSnow;

#[derive(Resource)]
struct SceneSnowState {
    config: SceneSnowConfig,
    positions: Vec<Vec3>,
    velocities: Vec<f32>,
    entity: Option<Entity>,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    rng: u64,
}

impl Default for SceneSnowState {
    fn default() -> Self {
        Self {
            config: SceneSnowConfig::default(),
            positions: Vec::new(),
            velocities: Vec::new(),
            entity: None,
            mesh: None,
            material: None,
            rng: 0x9e37_79b9_7f4a_7c15,
        }
    }
}

type SceneScriptFighters<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        Option<&'static MissionNpc>,
        Option<&'static LocalFighter>,
    ),
    Or<(With<MissionNpc>, With<LocalFighter>)>,
>;

#[derive(Component, Debug, Clone)]
#[require(NpcPerformance)]
pub struct MissionNpc {
    pub id: i64,

    pub name: String,

    pub team: u8,

    pub in_scene_roster: bool,

    pub view: f32,
    pub think: f32,

    pub forced_behavior: Option<meteor_ai::behavior::BehaviorMode>,

    pub is_vip: bool,

    pub attack_target_slot: Option<i64>,

    pub behavior_args: Vec<meteor_scene::pst_runtime::PstValue>,
    pub attack_target_remaining: i64,
}

#[derive(Component, Debug, Clone, Default)]
#[allow(dead_code)]
pub(crate) struct NpcCombatProfile {
    pub attack1: f32,
    pub attack2: f32,
    pub attack3: f32,
    pub guard: f32,
    pub dodge: f32,
    pub jump: f32,
    pub look: f32,
    pub burst: f32,
    pub aim: f32,
    pub get_item: f32,
}

impl NpcCombatProfile {
    pub(crate) fn web_debug() -> Self {
        Self {
            attack1: 40.0,
            attack2: 30.0,
            attack3: 20.0,
            guard: 20.0,
            dodge: 15.0,
            jump: 0.0,
            look: 0.0,
            burst: 10.0,
            aim: 0.0,
            get_item: 0.0,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NpcPerformCommand {
    pub action: String,
    pub args: Vec<meteor_scene::pst_runtime::PstValue>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct PerformanceClock {
    elapsed: f32,
    pause_ticks: Option<u32>,
    guard_ticks: Option<u32>,
}

impl PerformanceClock {
    pub(crate) fn next(
        &mut self,
        queue: &mut std::collections::VecDeque<NpcPerformCommand>,
        dt: f32,
    ) -> Option<NpcPerformCommand> {
        self.next_eligible(queue, dt, true, true)
    }

    pub(crate) fn guard_held(&self) -> bool {
        self.guard_ticks.is_some()
    }

    pub(crate) fn next_eligible(
        &mut self,
        queue: &mut std::collections::VecDeque<NpcPerformCommand>,
        dt: f32,
        action_ready: bool,
        guard_ready: bool,
    ) -> Option<NpcPerformCommand> {
        self.elapsed += dt.clamp(0.0, 0.1);
        if self.elapsed <= 0.33 {
            return None;
        }
        self.elapsed = 0.0;
        let command = queue.front()?;
        if (command.action == "guard" && !guard_ready)
            || (command.action == "aggress" && !action_ready)
        {
            return None;
        }
        if command.action == "guard" {
            let ticks = self.guard_ticks.get_or_insert_with(|| {
                let value = command.args.first().map(PstValue::as_f64).unwrap_or(1.0);
                (value.max(0.0) as u32).saturating_mul(2)
            });
            if *ticks > 0 {
                *ticks -= 1;
                return None;
            }
            self.guard_ticks = None;
        }
        if command.action == "pause" {
            let ticks = self.pause_ticks.get_or_insert_with(|| {
                let value = command.args.first().map(PstValue::as_f64).unwrap_or(1.0);
                (value.max(0.0) as u32).saturating_mul(2)
            });
            if *ticks > 0 {
                *ticks -= 1;
                return None;
            }
        }
        self.pause_ticks = None;
        queue.pop_front()
    }
}

#[derive(Component, Debug, Clone, Default)]
pub(crate) struct NpcPerformance {
    pub clock: PerformanceClock,
    pub queue: std::collections::VecDeque<NpcPerformCommand>,
    pub pause_remaining: f32,
    pub guard_remaining: f32,
    pub crouching: bool,
    pub jump_requested: bool,
    pub use_items: Vec<u32>,
}

impl NpcPerformance {
    pub(crate) fn guard_hold(&self) -> f32 {
        self.guard_remaining
            .max(if self.clock.guard_held() { 1.0 } else { 0.0 })
    }
}

#[derive(Resource, Debug, Clone, Default)]
pub(crate) struct PlayerPerformance {
    pub clock: PerformanceClock,
    pub queue: std::collections::VecDeque<NpcPerformCommand>,
    pub pause_remaining: f32,
    pub input_blocked: bool,
    pub crouch_requested: Option<bool>,
    pub use_items: Vec<u32>,
}

#[derive(Resource, Default)]
struct PendingMissionNpcCommands {
    behaviors: Vec<(i64, String, Vec<meteor_scene::pst_runtime::PstValue>)>,
    performs: Vec<(i64, String, Vec<meteor_scene::pst_runtime::PstValue>)>,
}

#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub enum ModeObjectiveKind {
    EscortCargo,

    SiegeTarget,
}

fn local_mode_objectives_enabled(status: crate::plugins::network_plugin::NetStatus) -> bool {
    status != crate::plugins::network_plugin::NetStatus::InRoom
}

fn network_escort_visible(state: &str) -> bool {
    state != "carried"
}

#[derive(Component, Debug, Clone)]
pub struct ModeObjective {
    pub kind: ModeObjectiveKind,
}

#[derive(Component, Debug, Clone, Default)]
pub struct EscortCarrying;

#[derive(Component, Default)]
pub(crate) struct LocalEscortState {
    carrier: Option<Entity>,
    return_remaining: Option<f32>,
}

fn local_escort_enabled(request: &LaunchRequest, mission: &StageMissionRuntime) -> bool {
    request.mode == "escort"
        || (request.mode == "single"
            && mission.script().is_some_and(|runtime| {
                runtime
                    .values()
                    .get("Rule")
                    .is_some_and(|rule| rule.as_f64() == 2.0)
            }))
}

#[derive(Component)]
pub(crate) struct NetworkEscortFlag;

#[derive(Component, Debug, Clone, Copy)]
struct NetworkEscortSafeZone {
    team: u8,
}

type ModeObjectiveVisualQuery<'w, 's> = Query<
    'w,
    's,
    Entity,
    Or<(
        With<ModeObjective>,
        With<NetworkEscortFlag>,
        With<NetworkEscortSafeZone>,
    )>,
>;

#[derive(Resource, Default)]
pub struct StageMissionRuntime(Option<MissionScriptRuntime>);

impl StageMissionRuntime {
    #[cfg(test)]
    pub(crate) fn from_script(script: MissionScriptRuntime) -> Self {
        Self(Some(script))
    }

    pub(crate) fn rule(&self) -> i32 {
        self.script()
            .and_then(|script| script.values().get("Rule"))
            .map(|value| value.as_f64() as i32)
            .unwrap_or(meteor_network::modes::DEFAULT_LEVEL_RULE)
    }

    pub fn script(&self) -> Option<&MissionScriptRuntime> {
        self.0.as_ref()
    }

    pub fn script_mut(&mut self) -> Option<&mut MissionScriptRuntime> {
        self.0.as_mut()
    }
}

pub struct StagePlugin;

impl Plugin for StagePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(StageScriptRuntime::default())
            .insert_resource(StageMissionRuntime::default())
            .init_resource::<PendingMissionNpcCommands>()
            .init_resource::<PlayerPerformance>()
            .insert_resource(CustomStages::default())
            .insert_resource(SceneTouchState::default())
            .insert_resource(SceneSnowState::default())
            .init_resource::<LoadedStageAssets>()
            .init_resource::<super::escort_assets::LoadedEscort>()
            .add_systems(OnEnter(AppState::Menu), discover_custom_stages)
            .add_systems(OnEnter(AppState::Loading), reset_player_performance)
            .add_systems(
                OnEnter(AppState::Playing),
                spawn_local_fighter.run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                OnEnter(AppState::Playing),
                spawn_mode_objectives.run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                OnEnter(AppState::Playing),
                start_stage_script
                    .run_if(|script: Res<StageScriptRuntime>| script.0.is_some())
                    .run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                OnEnter(AppState::Playing),
                (start_mission_script, consume_mission_npc_requests)
                    .chain()
                    .run_if(|script: Res<StageMissionRuntime>| script.0.is_some())
                    .run_if(|request: Res<LaunchRequest>| should_load_mission_script(&request))
                    .run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                OnExit(AppState::Playing),
                (
                    close_stage_script,
                    clear_scene_snow,
                    despawn_local_fighter,
                    clear_mode_objective_visuals,
                )
                    .run_if(crate::plugins::ends_playing_session),
            )
            .add_systems(
                Update,
                (
                    tick_stage_script.after(crate::plugins::render_plugin::sync_fmc_with_script),
                    scene_script_touch_and_transfer.after(tick_stage_script),
                    sync_scene_snow
                        .after(scene_script_touch_and_transfer)
                        .run_if(in_state(AppState::Playing)),
                    (
                        sync_mission_spatial,
                        sync_mission_fighters,
                        escort_cargo_pickup,
                        mode_objective_tick,
                        tick_mission_script,
                        consume_mission_npc_requests,
                        consume_mission_attribute_updates,
                        consume_mission_say_requests,
                        assign_assassin_vip,
                    )
                        .chain()
                        .after(crate::plugins::FighterFrameSet::RootMotion)
                        .before(crate::plugins::FighterFrameSet::PoseResolve),
                )
                    .run_if(crate::plugins::simulation_frame),
            )
            .add_systems(
                Update,
                (
                    sync_network_escort_flag,
                    sync_network_escort_safe_zones,
                    super::escort_assets::sync_escort_models
                        .after(sync_network_escort_flag)
                        .after(tick_stage_script)
                        .after(escort_cargo_pickup),
                )
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                Update,
                drive_player_performance
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .before(crate::plugins::input_plugin::capture_local_command_input)
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(in_state(AppState::Playing)),
            );
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(OnEnter(AppState::Loading), begin_stage_load)
            .add_systems(Update, poll_stage_load.run_if(in_state(AppState::Loading)))
            .add_systems(OnExit(AppState::Loading), cancel_stage_load);
        #[cfg(target_arch = "wasm32")]
        app.init_resource::<crate::wasm_assets::WasmAssetLoader>()
            .add_systems(
                OnExit(AppState::Loading),
                crate::wasm_assets::cancel_pending_assets,
            )
            .add_systems(
                Update,
                crate::wasm_assets::prepare_match_assets.run_if(in_state(AppState::Loading)),
            )
            .add_systems(
                Update,
                load_selected_stage
                    .run_if(in_state(AppState::Loading))
                    .run_if(crate::wasm_assets::match_assets_ready)
                    .after(crate::wasm_assets::prepare_match_assets),
            );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct PendingStageLoad(bevy::tasks::Task<PreparedStage>);
#[cfg(not(target_arch = "wasm32"))]
struct PreparedStage {
    characters: Option<super::character_plugin::CharacterAssetsSet>,
    common: Option<super::character_plugin::CommonAmb>,
    info: LoadedStageInfo,
    assets: LoadedStageAssets,
    script: StageScriptRuntime,
    mission: StageMissionRuntime,
    waypoints: WaypointGraph,
    escort: super::escort_assets::LoadedEscort,
    #[cfg(target_os = "android")]
    android: crate::android_assets::AndroidAssetLoader,
}
#[cfg(not(target_arch = "wasm32"))]
fn begin_stage_load(
    mut commands: Commands,
    room: Option<Res<crate::app_state::RoomState>>,
    request: Res<LaunchRequest>,
    custom: Res<CustomStages>,
    net: Res<crate::plugins::network_plugin::NetState>,
    mut info: ResMut<LoadedStageInfo>,
    #[cfg(target_os = "android")] android: Res<crate::android_assets::AndroidAssetLoader>,
) {
    use bevy::ecs::system::RunSystemOnce;
    *info = LoadedStageInfo::default();
    let request = request.clone();
    let custom = custom.clone();
    let room = room.as_deref().cloned().unwrap_or_default();
    let members = net.members.clone();
    let escort_definition = net.escort_definition.clone();
    #[cfg(target_os = "android")]
    let android = android.clone();
    let task = bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let mut world = World::new();
        world.insert_resource(request);
        world.insert_resource(custom);
        world.insert_resource(room);
        let mut net = crate::plugins::network_plugin::NetState::default();
        net.escort_definition = escort_definition;
        net.members = members;
        world.insert_resource(net);
        world.init_resource::<LoadedStageInfo>();
        world.init_resource::<LoadedStageAssets>();
        world.init_resource::<StageScriptRuntime>();
        world.init_resource::<StageMissionRuntime>();
        world.init_resource::<NextState<AppState>>();
        #[cfg(target_os = "android")]
        {
            world.insert_resource(android);
            world
                .run_system_once(crate::android_assets::load_match_assets)
                .expect("asset preparation system");
        }
        world
            .run_system_once(load_selected_stage)
            .expect("stage preparation system");
        if world.resource::<LoadedStageInfo>().ready {
            world
                .run_system_once(super::character_plugin::load_character_assets)
                .expect("character preparation system");
            let id = format!("p{}", world.resource::<LaunchRequest>().character_id);
            let ready = world.contains_resource::<super::character_plugin::CommonAmb>()
                && world
                    .resource::<super::character_plugin::CharacterAssetsSet>()
                    .0
                    .contains_key(&id);
            if !ready {
                let mut info = world.resource_mut::<LoadedStageInfo>();
                info.ready = false;
                info.error = Some("人物或公共动作库加载失败，请返回后重试".into());
            }
        }
        PreparedStage {
            characters: world.remove_resource(),
            common: world.remove_resource(),
            info: world.remove_resource().unwrap(),
            assets: world.remove_resource().unwrap(),
            script: world.remove_resource().unwrap(),
            mission: world.remove_resource().unwrap(),
            waypoints: world.remove_resource().unwrap_or_default(),
            escort: world.remove_resource().unwrap_or_default(),
            #[cfg(target_os = "android")]
            android: world.remove_resource().unwrap(),
        }
    });
    commands.insert_resource(PendingStageLoad(task));
}
#[cfg(not(target_arch = "wasm32"))]
fn poll_stage_load(
    mut commands: Commands,
    pending: Option<ResMut<PendingStageLoad>>,
    mut next: ResMut<NextState<AppState>>,
    presentation: Option<Res<super::loading_ui::LoadingPresentation>>,
) {
    if presentation.is_some_and(|p| !p.can_enter()) {
        return;
    }
    let Some(mut pending) = pending else {
        return;
    };
    let Some(stage) =
        bevy::tasks::block_on(bevy::tasks::futures_lite::future::poll_once(&mut pending.0))
    else {
        return;
    };
    next.set(if stage.info.ready {
        AppState::Playing
    } else {
        AppState::Error
    });
    if let Some(characters) = stage.characters {
        commands.insert_resource(characters);
    }
    if let Some(common) = stage.common {
        commands.insert_resource(common);
    }
    if stage.info.ready {
        meteor_asset::runtime_assets::retain_stage(&stage.info.stage_id);
    }
    commands.insert_resource(stage.info);
    commands.insert_resource(stage.assets);
    commands.insert_resource(stage.script);
    commands.insert_resource(stage.mission);
    commands.insert_resource(stage.waypoints);
    commands.insert_resource(stage.escort);
    #[cfg(target_os = "android")]
    commands.insert_resource(stage.android);
    commands.remove_resource::<PendingStageLoad>();
}
#[cfg(not(target_arch = "wasm32"))]
fn cancel_stage_load(mut commands: Commands) {
    commands.remove_resource::<PendingStageLoad>();
}

fn reset_player_performance(mut performance: ResMut<PlayerPerformance>) {
    *performance = PlayerPerformance::default();
}

pub(crate) fn drive_player_performance(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut performance: ResMut<PlayerPerformance>,
    mut crouch: Option<ResMut<super::input_plugin::LocalCrouchInput>>,
    mut dialogue: Option<ResMut<crate::plugins::ui_plugin::DialogueState>>,
) {
    let dt = crate::plugins::simulation_wall_step(frame_clock.as_deref(), time.delta_secs());
    let spoken = advance_player_performance(&mut performance, dt);
    if let Some(crouch) = crouch.as_deref_mut() {
        if let Some(requested) = performance.crouch_requested.take() {
            crouch.held = requested;
        }
    }
    if let (Some(text), Some(dialogue)) = (spoken, dialogue.as_deref_mut()) {
        dialogue.text = Some(text);
        dialogue.timer = 3.0;
    }
}

fn advance_player_performance(performance: &mut PlayerPerformance, dt: f32) -> Option<String> {
    if performance.pause_remaining > 0.0 {
        performance.pause_remaining = (performance.pause_remaining - dt).max(0.0);
        return None;
    }
    let mut spoken = None;
    if let Some(command) = performance.clock.next(&mut performance.queue, dt) {
        match command.action.as_str() {
            "pause" => {}
            "say" => {
                let text = command
                    .args
                    .first()
                    .map(|value| match value {
                        meteor_scene::pst_runtime::PstValue::Str(text) => text.clone(),
                        meteor_scene::pst_runtime::PstValue::Num(value) => format!("{value}"),
                        meteor_scene::pst_runtime::PstValue::Array(_) => String::new(),
                    })
                    .unwrap_or_default();
                spoken = Some(text);
            }
            "block" => {
                performance.input_blocked = command
                    .args
                    .first()
                    .map(|value| value.as_f64() == 0.0)
                    .unwrap_or(true);
            }
            "crouch" => {
                performance.crouch_requested =
                    Some(command.args.first().map(PstValue::truthy).unwrap_or(true))
            }
            "use" => {
                if let Some(id) = command
                    .args
                    .first()
                    .map(PstValue::as_f64)
                    .filter(|id| id.is_finite() && *id > 0.0 && id.fract() == 0.0)
                {
                    performance.use_items.push(id as u32);
                }
            }
            _ => warn!("unsupported PlayerPerform command: {}", command.action),
        }
    }
    spoken
}

#[derive(Resource, Default, Clone)]
pub struct CustomStages {
    pub stages: Vec<meteor_scene::stage_registry::StageInfo>,

    pub packs: Vec<(String, meteor_asset::pak::PakArchive)>,
}

impl CustomStages {
    pub fn pak_for(&self, stage_id: &str) -> Option<&meteor_asset::pak::PakArchive> {
        self.packs
            .iter()
            .find(|(id, _)| id == stage_id)
            .map(|(_, pak)| pak)
    }
}

enum StageSource {
    File(FileStageSource),
    Mpk {
        pak: meteor_asset::pak::PakArchive,
        path: PathBuf,
    },
}

impl StageSource {
    fn entry_name(path: &str) -> Option<String> {
        let basename = path.rsplit('/').next()?;
        if path.starts_with("Level/") {
            Some(format!("level/{basename}"))
        } else if path.starts_with("PScriptLib/") {
            None
        } else {
            Some(format!("stage/{basename}"))
        }
    }

    fn read_with_path(&self, path: &str) -> (Option<Vec<u8>>, Option<PathBuf>) {
        match self {
            StageSource::File(file) => file.read_with_path(path),
            StageSource::Mpk {
                pak,
                path: pak_path,
            } => {
                let bytes = match Self::entry_name(path) {
                    Some(entry) => pak.read_entry(&entry),
                    None => None,
                };
                (bytes, Some(pak_path.clone()))
            }
        }
    }
}

impl StageAssetSource for StageSource {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_with_path(path).0
    }
}

fn capture_loaded_stage_assets(source: &StageSource, stage_id: &str) -> LoadedStageAssets {
    let paths = meteor_scene::stage_loader::stage_asset_paths(stage_id);
    LoadedStageAssets {
        gmb: source.read_with_path(&paths.gmb).0,
        des: source.read_with_path(&paths.des).0,
        cob: source.read_with_path(&paths.cob).0,
        from_pack: matches!(source, StageSource::Mpk { .. }),
    }
}

pub(crate) fn discover_custom_stages(mut custom: ResMut<CustomStages>) {
    custom.stages = super::escort_assets::custom_stage_entries();
    custom.packs.clear();
}

struct FileStageSource {
    roots: Vec<PathBuf>,
}

impl FileStageSource {
    fn read_with_path(&self, path: &str) -> (Option<Vec<u8>>, Option<PathBuf>) {
        if let Some(bytes) = meteor_asset::runtime_assets::read_legacy_path(path) {
            return (Some(bytes), Some(PathBuf::from("<runtime-mpk>")));
        }
        self.roots
            .iter()
            .map(|root| root.join(path))
            .find_map(|candidate| {
                crate::resource_io::read(&candidate)
                    .ok()
                    .map(|bytes| (Some(bytes), Some(candidate)))
            })
            .unwrap_or((None, None))
    }
}

impl StageAssetSource for FileStageSource {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_with_path(path).0
    }
}

pub(crate) fn resource_roots(workdir: &Path) -> Vec<PathBuf> {
    crate::plugins::audio_plugin::resource_roots(workdir)
}

#[allow(clippy::too_many_arguments)]
fn load_selected_stage(
    mut commands: Commands,
    request: Res<LaunchRequest>,
    net: Res<crate::plugins::network_plugin::NetState>,
    custom: Res<CustomStages>,
    mut loaded: ResMut<LoadedStageInfo>,
    mut loaded_assets: ResMut<LoadedStageAssets>,
    mut script_runtime: ResMut<StageScriptRuntime>,
    mut mission_runtime: ResMut<StageMissionRuntime>,
    mut next_state: ResMut<NextState<AppState>>,
    #[cfg(target_os = "android")] android_loader: Res<crate::android_assets::AndroidAssetLoader>,
) {
    #[cfg(target_os = "android")]
    if let Some(error) = android_loader.error_for(&request) {
        *loaded_assets = LoadedStageAssets::default();
        script_runtime.0 = None;
        mission_runtime.0 = None;
        loaded.stage_id = request.stage_id.clone();
        loaded.ready = false;
        loaded.error = Some(format!("Android 资源加载失败：{error}"));
        next_state.set(AppState::Error);
        return;
    }
    let root = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let roots = resource_roots(&root);
    info!(
        "stage load: cwd={} stage={} roots={:?}",
        root.display(),
        request.stage_id,
        roots
    );
    let escort_result = super::escort_assets::load_escort(&request.stage_id);
    let escort = escort_result.as_ref().ok().cloned();
    if net
        .escort_definition
        .as_ref()
        .is_some_and(|expected| escort.as_ref() != Some(expected))
    {
        loaded.ready = false;
        loaded.error = Some("地图镖物资源在连接后发生变化，请重新进入房间".into());
        next_state.set(AppState::Error);
        return;
    }
    commands.insert_resource(super::escort_assets::LoadedEscort(escort.clone()));

    let source = if !meteor_scene::stage_registry::is_official_stage_id(&request.stage_id) {
        match custom.pak_for(&request.stage_id) {
            Some(pak) => StageSource::Mpk {
                pak: pak.clone(),
                path: PathBuf::new(),
            },
            None => StageSource::File(FileStageSource { roots }),
        }
    } else {
        StageSource::File(FileStageSource { roots })
    };
    let paths = meteor_scene::stage_loader::stage_asset_paths(&request.stage_id);
    *loaded_assets = capture_loaded_stage_assets(&source, &request.stage_id);
    for path in [&paths.gmb, &paths.des, &paths.cob, &paths.pst] {
        let (bytes, hit) = source.read_with_path(path);
        info!(
            "stage asset {}: {}{}",
            path,
            hit.as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "missing".into()),
            bytes
                .as_ref()
                .map(|b| format!(" ({} bytes)", b.len()))
                .unwrap_or_default()
        );
    }

    let wp_path = format!("Level/{}.wp", request.stage_id);
    let (wp_bytes, wp_hit) = source.read_with_path(&wp_path);
    let waypoint_text = wp_bytes.as_deref().map(crate::text_codec::decode_text);
    let center = get_stage_center(&request.stage_id, waypoint_text.as_deref());

    let waypoint_graph = waypoint_text
        .as_deref()
        .map(|text| meteor_scene::stage_loader::parse_waypoint_graph(text, center))
        .unwrap_or_default();
    commands.insert_resource(waypoint_graph);
    info!(
        "stage center: {} waypoint={} center={:?}",
        wp_path,
        wp_hit
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "missing".into()),
        center
    );
    let stage = load_stage_assets(&source, &request.stage_id, center);

    let (pst_bytes, _) = source.read_with_path(&paths.pst);
    match pst_bytes.as_deref().map(crate::text_codec::decode_text) {
        Some(stage_text) => {
            let mut parts = Vec::new();
            for include in ["Def.h", "box.pst", "desk.pst", "chair.pst", "jug.pst"] {
                let (bytes, _) = source.read_with_path(&format!("PScriptLib/{include}"));
                if let Some(bytes) = bytes {
                    parts.push(crate::text_codec::decode_text(&bytes));
                }
            }
            parts.push(stage_text);
            let merged = parts.join("\n");

            let des_path = format!("{}/{}.des", request.stage_id, request.stage_id);
            let instances = source
                .read_with_path(&des_path)
                .0
                .map(|bytes| crate::text_codec::decode_text(&bytes))
                .map(|text| meteor_asset::des::parse_stage_description(&text))
                .map(|desc| {
                    desc.dummies
                        .iter()
                        .enumerate()
                        .map(|(index, dummy)| SceneItemInstance {
                            name: dummy.name.clone(),
                            index,

                            props: [
                                (
                                    "active".to_string(),
                                    PstValue::Num(dummy.visible as i32 as f64),
                                ),
                                (
                                    "collision".to_string(),
                                    PstValue::Num(crate::plugins::render_plugin::is_solid_prop(
                                        dummy,
                                    ) as i32
                                        as f64),
                                ),
                                (
                                    "damagevalue".to_string(),
                                    PstValue::Num(f64::from(dummy.damage_name)),
                                ),
                                ("interactive".to_string(), PstValue::Num(1.0)),
                                ("pose".to_string(), PstValue::Num(0.0)),
                                ("pose_loop".to_string(), PstValue::Num(0.0)),
                                ("frame".to_string(), PstValue::Num(-1.0)),
                                ("state".to_string(), PstValue::Num(3.0)),
                            ]
                            .into_iter()
                            .collect(),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            match SceneScriptRuntime::load(&merged, instances, 250_000) {
                Ok(runtime) => {
                    info!(
                        "stage scene script runtime ready ({} instances)",
                        runtime.host().instances().len()
                    );
                    script_runtime.0 = Some(runtime);
                }
                Err(error) => {
                    warn!("stage scene script parse failed: {error}");
                    script_runtime.0 = None;
                }
            }
        }
        None => info!("stage pst unavailable for {}", request.stage_id),
    }

    mission_runtime.0 = None;
    if should_load_mission_script(&request) {
        let mission_path = format!("Level/{}_.pst", request.stage_id);
        match source
            .read_with_path(&mission_path)
            .0
            .map(|bytes| crate::text_codec::decode_text(&bytes))
        {
            Some(mission_text) => {
                let definition =
                    meteor_scene::mission_runtime::parse_mission_definition(&mission_text);
                info!(
                    "mission script {}: {} npc scripts, {} globals",
                    mission_path,
                    definition.npc_scripts.len(),
                    definition.values.len()
                );
                let mut runtime = MissionScriptRuntime::new(&mission_text, definition.values);
                for npc in definition.npc_scripts {
                    let path = format!("Level/{npc}.pst");
                    if let Some(bytes) = source.read_with_path(&path).0 {
                        let text = crate::text_codec::decode_text(&bytes);
                        if let Err(error) = runtime.register_npc_script(&npc, &text) {
                            warn!("NPC script {path}: {error}");
                        }
                    }
                }
                mission_runtime.0 = Some(runtime);
            }
            None => info!("mission script unavailable for {}", request.stage_id),
        }
    } else {
        info!(
            "mission script skipped for non-solo session: stage={} mode={} room_npcs={}",
            request.stage_id, request.mode, request.spawn_room_npcs
        );
    }
    if local_escort_enabled(&request, &mission_runtime) && escort.is_none() {
        loaded.ready = false;
        loaded.error = Some(format!("劫镖地图配置无效：{}", escort_result.unwrap_err()));
        next_state.set(AppState::Error);
        return;
    }
    loaded.stage_id = stage.stage_id;
    loaded.ready = stage.ready;
    loaded.scene_object_count = stage.scene_object_count;
    loaded.collision_face_count = stage.collision_face_count;
    loaded.center = stage.center;
    loaded.error = (!stage.ready).then(|| "关卡资源不完整：需要 GMB、DES、COB 文件".to_string());
    next_state.set(if stage.ready {
        AppState::Playing
    } else {
        AppState::Error
    });
}

#[derive(Component)]
pub(crate) struct LocalFighter;

#[derive(Debug, Clone, Copy, PartialEq)]
struct PlayerSpawnLoadout {
    weapon: u8,
    weapon_slots: [u8; 2],
    weapon_slot_variants: [u8; 2],
    hp: f32,
}

fn mission_global_i64(runtime: &MissionScriptRuntime, name: &str) -> Option<i64> {
    runtime
        .values()
        .get(name)
        .map(|value| value.as_f64())
        .filter(|value| value.is_finite())
        .map(|value| value as i64)
}

fn should_load_mission_script(request: &LaunchRequest) -> bool {
    request.mode == "single" && !request.spawn_room_npcs
}

fn resolve_player_spawn_loadout(
    request: &LaunchRequest,
    mission: Option<&MissionScriptRuntime>,
) -> PlayerSpawnLoadout {
    let applies_mission_loadout = should_load_mission_script(request)
        && meteor_scene::stage_registry::is_official_stage_id(&request.stage_id);
    let mission_hp = applies_mission_loadout
        .then(|| mission.and_then(|runtime| mission_global_i64(runtime, "PlayerHP")))
        .flatten()
        .filter(|hp| *hp > 0)
        .map(|hp| hp as f32)
        .unwrap_or(meteor_core::DEFAULT_HP);
    let fallback = PlayerSpawnLoadout {
        weapon: request.weapon,

        weapon_slots: [request.weapon, 0],
        weapon_slot_variants: [0, 0],
        hp: mission_hp,
    };
    if !applies_mission_loadout {
        return fallback;
    }
    let Some(runtime) = mission else {
        return fallback;
    };
    let primary = mission_global_i64(runtime, "PlayerWeapon")
        .and_then(crate::plugins::character_plugin::weapon_slot_from_item)
        .unwrap_or((0, 0));
    let secondary = mission_global_i64(runtime, "PlayerWeapon2")
        .and_then(crate::plugins::character_plugin::weapon_slot_from_item)
        .unwrap_or((0, 0));
    PlayerSpawnLoadout {
        weapon: primary.0,
        weapon_slots: [primary.0, secondary.0],
        weapon_slot_variants: [primary.1, secondary.1],
        hp: mission_hp,
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_local_fighter(
    mut commands: Commands,
    request: Res<LaunchRequest>,
    waypoints: Option<Res<WaypointGraph>>,
    loaded: Res<LoadedStageInfo>,
    loaded_assets: Res<LoadedStageAssets>,
    mission: Res<StageMissionRuntime>,
    net: Option<Res<crate::plugins::network_plugin::NetState>>,
    mrc: Option<Res<crate::plugins::replay_plugin::MrcPlayback>>,
) {
    let stage_desc = loaded_assets
        .des
        .as_deref()
        .map(crate::text_codec::decode_text)
        .map(|text| meteor_asset::des::parse_stage_description(&text));
    let (pad_prefix, pad_index) = network_spawn_pad(net.as_deref());
    let (position, facing) = resolve_player_spawn(
        waypoints.as_deref(),
        mission.script(),
        stage_desc.as_ref(),
        loaded.center,
        request.mode == "single",
        pad_prefix,
        pad_index,
    );
    let mut loadout = resolve_player_spawn_loadout(&request, mission.script());
    if request.mode == "replay" {
        if let Some((weapon, variant)) = mrc
            .as_ref()
            .filter(|mrc| mrc.active)
            .and_then(|mrc| mrc.local_weapon())
        {
            loadout.weapon = weapon;
            loadout.weapon_slots = [weapon, 0];
            loadout.weapon_slot_variants = [variant, 0];
        }
    }
    let replay_vitals = (request.mode == "replay")
        .then(|| mrc.as_ref().and_then(|mrc| mrc.spawn_vitals()))
        .flatten();
    let (current_hp, max_hp, anger) = replay_vitals.unwrap_or((loadout.hp, loadout.hp, 0.0));
    info!(
        "player spawn: pos={:?} facing={facing:.3} (stage={})",
        position, request.stage_id
    );

    commands
        .spawn((
            LocalFighter,
            Fighter {
                character_id: request.character_id,
                player_slot: 0,
                weapon: loadout.weapon,
                weapon_slots: loadout.weapon_slots,
                weapon_slot: 0,
                weapon_slot_variants: loadout.weapon_slot_variants,
                weapon_variant: loadout.weapon_slot_variants[0],
            },
            Health {
                current: current_hp,
                max: max_hp,
            },
            Qi {
                current: anger.clamp(0.0, meteor_core::DEFAULT_QI_MAX),
                max: meteor_core::DEFAULT_QI_MAX,
            },
            CombatState::default(),
            MotionState {
                movement_mode: MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing,
            },
            crate::plugins::movement_plugin::FighterAir::default(),
            crate::plugins::movement_plugin::FighterDodge::default(),
            crate::components::fighter::PoseRuntime::idle(),
            crate::components::fighter::SmState::default(),
            crate::components::fighter::DeathRespawn {
                spawn_pos: position,
                ..default()
            },
            crate::components::fighter::AiIntent::default(),
            crate::components::fighter::HeadTrackState::default(),
            Transform::from_translation(position),
        ))
        .insert(Visibility::default())
        .insert(InheritedVisibility::default())
        .insert(crate::plugins::movement_plugin::FighterWall::default())
        .insert(crate::plugins::movement_plugin::FighterAttackMotion::default())
        .insert(crate::components::fighter::VisualRootPin::default())
        .insert(crate::components::fighter::EntryBlend::default())
        .insert(crate::components::fighter::TargetHitClock::default());
}

fn network_spawn_pad(
    net: Option<&crate::plugins::network_plugin::NetState>,
) -> (&'static str, Option<usize>) {
    let Some(net) =
        net.filter(|net| net.status == crate::plugins::network_plugin::NetStatus::InRoom)
    else {
        return ("user", None);
    };
    let Some(local_index) = net
        .members
        .iter()
        .position(|member| member.player_id == net.player_id)
    else {
        return ("user", None);
    };
    let team = net.members[local_index].team;
    let prefix = match team {
        Some(1) => "teama",
        Some(2) => "teamb",
        _ => "user",
    };
    let index = team.map_or(local_index, |team| {
        net.members[..local_index]
            .iter()
            .filter(|member| member.team == Some(team))
            .count()
    });
    (prefix, Some(index))
}

fn stage_spawn_pads(
    desc: &meteor_asset::des::StageDescription,
    prefix: &str,
    center: [f32; 3],
) -> Vec<(Vec3, f32)> {
    let prefix = prefix.to_ascii_lowercase();
    let face_stage_center = prefix == "teama" || prefix == "teamb";
    let mut pads = desc
        .scene_objects
        .iter()
        .chain(&desc.dummies)
        .filter_map(|object| {
            let name = object.name_attr.as_deref()?.to_ascii_lowercase();
            let index = name.strip_prefix(&prefix)?.parse::<usize>().ok()?;
            let p = object.position;
            let q = object.quaternion;
            let rotation = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
                * Quat::from_xyzw(q.x, q.y, q.z, q.w);
            let position = Vec3::new(
                (p.x - center[0]) * meteor_core::constants::MODEL_SCALE,
                (p.z - center[2]) * meteor_core::constants::MODEL_SCALE,
                -(p.y - center[1]) * meteor_core::constants::MODEL_SCALE,
            );
            let facing = if face_stage_center {
                (-position.x).atan2(-position.z)
            } else {
                rotation.to_euler(EulerRot::YXZ).0
            };
            Some((index, position, facing))
        })
        .collect::<Vec<_>>();
    pads.sort_unstable_by_key(|(index, _, _)| *index);
    pads.into_iter()
        .map(|(_, position, facing)| (position, facing))
        .collect()
}

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct RespawnStage<'w> {
    assets: Option<Res<'w, LoadedStageAssets>>,
    info: Option<Res<'w, LoadedStageInfo>>,
    waypoints: Option<Res<'w, WaypointGraph>>,
    collider: Option<Res<'w, super::stage_collider::StageCollider>>,
}

impl RespawnStage<'_> {
    pub(super) fn resolve(
        &self,
        team: Option<u32>,
        pad_index: Option<usize>,
        fallback: Vec3,
        facing: f32,
    ) -> (Vec3, f32) {
        let prefix = match team {
            Some(1) => "teama",
            Some(2) => "teamb",
            _ => "user",
        };
        let mut pads = self
            .assets
            .as_ref()
            .and_then(|assets| assets.des.as_deref())
            .map(|bytes| {
                let desc = meteor_asset::des::parse_stage_description(
                    &crate::text_codec::decode_text(bytes),
                );
                let center = self.info.as_ref().map_or([0.0; 3], |info| info.center);
                if pad_index.is_some() && prefix == "user" {
                    let a = stage_spawn_pads(&desc, "teama", center);
                    let b = stage_spawn_pads(&desc, "teamb", center);
                    (0..a.len().max(b.len()))
                        .flat_map(|index| {
                            [a.get(index).copied(), b.get(index).copied()]
                                .into_iter()
                                .flatten()
                        })
                        .collect()
                } else {
                    stage_spawn_pads(&desc, prefix, center)
                }
            })
            .unwrap_or_default();
        if pads.is_empty() {
            if let Some(graph) = self.waypoints.as_deref() {
                pads.extend(
                    graph
                        .points
                        .iter()
                        .filter(|point| (point.size - 40.0).abs() < 1e-3)
                        .filter_map(|point| graph.world_position(point.id))
                        .map(|position| (Vec3::from_array(position), facing)),
                );
            }
        }
        let (mut position, facing) = if pads.is_empty() {
            (fallback, facing)
        } else {
            let index = pad_index.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |time| time.as_nanos() as usize)
            });
            pads[index % pads.len()]
        };
        if let Some(mesh) = self
            .collider
            .as_ref()
            .and_then(|collider| collider.mesh.as_ref())
        {
            use meteor_physics::collision::MeshCollider;
            if let Some(ground) =
                mesh.sample_ground_hit(position.x, position.z, position.y, position.y)
            {
                position.y = ground.y;
            }
        }
        (position, facing)
    }
}

fn resolve_player_spawn(
    waypoints: Option<&WaypointGraph>,
    mission: Option<&meteor_scene::mission_runtime::MissionScriptRuntime>,
    stage_desc: Option<&meteor_asset::des::StageDescription>,
    center: [f32; 3],
    allow_mission_spawn: bool,
    pad_prefix: &str,
    pad_index: Option<usize>,
) -> (Vec3, f32) {
    let fallback = (Vec3::new(0.0, 0.9, 0.0), 0.0);
    let mission_facing = mission
        .and_then(|m| m.values().get("PlayerSpawnDir"))
        .and_then(|v| match v {
            meteor_scene::pst_runtime::PstValue::Num(n) => Some(*n as f32),
            _ => None,
        })
        .map(|deg| deg.to_radians())
        .unwrap_or(0.0);

    if let Some(graph) = waypoints {
        if let Some(idx) = allow_mission_spawn
            .then(|| {
                mission
                    .and_then(|m| m.values().get("PlayerSpawn"))
                    .and_then(|v| match v {
                        meteor_scene::pst_runtime::PstValue::Num(n) if n.is_finite() => {
                            Some(*n as usize)
                        }
                        _ => None,
                    })
            })
            .flatten()
        {
            if let Some([x, y, z]) = graph.world_position(idx) {
                return (Vec3::new(x, y, z), mission_facing);
            }
        }
    }

    if let Some(desc) = stage_desc {
        let pads = stage_spawn_pads(desc, pad_prefix, center);
        if !pads.is_empty() {
            let index = pad_index.unwrap_or_else(|| {
                (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_nanos() as usize)
                    .unwrap_or(0))
                    % pads.len()
            });
            return pads[index % pads.len()];
        }
    }

    if let Some(graph) = waypoints {
        let pads: Vec<Vec3> = graph
            .points
            .iter()
            .filter(|p| (p.size - 40.0).abs() < 1e-3)
            .filter_map(|p| graph.world_position(p.id))
            .map(|[x, y, z]| Vec3::new(x, y, z))
            .collect();
        if !pads.is_empty() {
            let i = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as usize)
                .unwrap_or(0))
                % pads.len();
            return (pads[i], mission_facing);
        }
    }
    fallback
}

fn despawn_local_fighter(mut commands: Commands, query: Query<Entity, With<LocalFighter>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn close_stage_script(mut script: ResMut<StageScriptRuntime>) {
    if let Some(runtime) = script.script_mut() {
        if let Err(error) = runtime.close() {
            warn!("scene script close failed: {error}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn sync_scene_snow(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut commands: Commands,
    mut script: ResMut<StageScriptRuntime>,
    mut snow: ResMut<SceneSnowState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    camera: Query<&Transform, With<crate::plugins::camera_plugin::GameplayCamera>>,
) {
    let requests = script
        .script_mut()
        .map(|runtime| runtime.host_mut().take_scene_requests())
        .unwrap_or_default();

    let rebuild = requests.iter().fold(false, |changed, (key, args)| {
        snow.config.apply(key, args) || changed
    });
    let (right, up) = scene_snow_camera_axes(camera.single().ok());

    if rebuild {
        rebuild_scene_snow(
            &mut commands,
            &mut snow,
            &mut meshes,
            &mut materials,
            &mut images,
            right,
            up,
        );
    }
    if snow.entity.is_none() {
        return;
    }

    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    if dt <= 0.0 {
        return;
    }
    let wind = Vec3::new(snow.config.wind.x, 0.0, snow.config.wind.y) * SCENE_SNOW_WORLD_SCALE;
    let SceneSnowState {
        positions,
        velocities,
        rng,
        ..
    } = &mut *snow;
    advance_scene_snow(positions, velocities, wind, dt, rng);
    if let Some(mut mesh) = snow.mesh.as_ref().and_then(|handle| meshes.get_mut(handle)) {
        update_scene_snow_mesh(
            &mut mesh,
            &snow.positions,
            snow.config.flake_size(),
            right,
            up,
        );
    }
}

fn load_scene_snow_texture(images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let color = super::ui_plugin::load_ui_texture_bytes(&root, "Asnow.jpg")?;
    let alpha = super::ui_plugin::load_ui_texture_bytes(&root, "Asnow.bmp")?;

    super::pak_textures::image_handle_with_legacy_alpha(images, &color, Some(&alpha), false)
}

fn rebuild_scene_snow(
    commands: &mut Commands,
    snow: &mut SceneSnowState,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    right: Vec3,
    up: Vec3,
) {
    if let Some(entity) = snow.entity.take() {
        commands.entity(entity).despawn();
    }
    snow.mesh = None;
    snow.positions.clear();
    snow.velocities.clear();
    if !snow.config.enabled || snow.config.particle_count() == 0 {
        return;
    }

    if snow.material.is_none() {
        let Some(texture) = load_scene_snow_texture(images) else {
            warn!("Scene snow requires UITexture Asnow.jpg and Asnow.bmp");
            return;
        };
        snow.material = Some(materials.add(StandardMaterial {
            base_color_texture: Some(texture),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        }));
    }
    let count = snow.config.particle_count();
    let rng = &mut snow.rng;
    snow.positions.reserve(count);
    snow.velocities.reserve(count);
    for _ in 0..count {
        snow.positions.push(scene_snow_spawn_position(rng));
        let t = scene_snow_random(rng);
        snow.velocities
            .push(snow.config.speed_min + (snow.config.speed_max - snow.config.speed_min) * t);
    }
    let mesh = meshes.add(build_scene_snow_mesh(
        &snow.positions,
        snow.config.flake_size(),
        right,
        up,
    ));
    let material = snow.material.clone().expect("snow material loaded above");
    let entity = commands
        .spawn((SceneSnow, Mesh3d(mesh.clone()), MeshMaterial3d(material)))
        .id();
    snow.entity = Some(entity);
    snow.mesh = Some(mesh);
}

fn clear_scene_snow(
    mut commands: Commands,
    mut snow: ResMut<SceneSnowState>,
    entities: Query<Entity, With<SceneSnow>>,
) {
    for entity in &entities {
        commands.entity(entity).despawn();
    }
    let material = snow.material.clone();
    *snow = SceneSnowState {
        material,
        ..Default::default()
    };
}

fn scene_snow_camera_axes(camera: Option<&Transform>) -> (Vec3, Vec3) {
    let rotation = camera
        .map(|transform| transform.rotation)
        .unwrap_or_default();
    (rotation * Vec3::X, rotation * Vec3::Y)
}

fn scene_snow_random(rng: &mut u64) -> f32 {
    *rng ^= *rng >> 12;
    *rng ^= *rng << 25;
    *rng ^= *rng >> 27;
    ((*rng).wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1_u32 << 24) as f32
}

fn scene_snow_spawn_position(rng: &mut u64) -> Vec3 {
    Vec3::new(
        (scene_snow_random(rng) * 2.0 - 1.0) * SCENE_SNOW_AREA_XZ,
        scene_snow_random(rng) * SCENE_SNOW_AREA_Y,
        (scene_snow_random(rng) * 2.0 - 1.0) * SCENE_SNOW_AREA_XZ,
    )
}

fn advance_scene_snow(
    positions: &mut [Vec3],
    velocities: &[f32],
    wind: Vec3,
    dt: f32,
    rng: &mut u64,
) {
    for (position, velocity) in positions.iter_mut().zip(velocities) {
        *position += (wind - Vec3::Y * (*velocity * SCENE_SNOW_WORLD_SCALE)) * dt;
        if position.y < -2.0 {
            *position = scene_snow_spawn_position(rng);

            position.y = SCENE_SNOW_AREA_Y;
        }
    }
}

fn build_scene_snow_mesh(positions: &[Vec3], size: f32, right: Vec3, up: Vec3) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    update_scene_snow_mesh(&mut mesh, positions, size, right, up);
    mesh
}

fn update_scene_snow_mesh(mesh: &mut Mesh, positions: &[Vec3], size: f32, right: Vec3, up: Vec3) {
    let half = size * 0.5;
    let right = right.normalize_or_zero() * half;
    let up = up.normalize_or_zero() * half;
    let normal = right.cross(up).normalize_or_zero().to_array();
    let mut vertices = Vec::with_capacity(positions.len() * 6);
    for center in positions {
        let bottom_left = *center - right - up;
        let bottom_right = *center + right - up;
        let top_right = *center + right + up;
        let top_left = *center - right + up;
        vertices.extend_from_slice(&[
            bottom_left.to_array(),
            bottom_right.to_array(),
            top_right.to_array(),
            top_right.to_array(),
            top_left.to_array(),
            bottom_left.to_array(),
        ]);
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vertices);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![normal; positions.len() * 6]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        positions
            .iter()
            .flat_map(|_| {
                [
                    [0.0, 0.0],
                    [1.0, 0.0],
                    [1.0, 1.0],
                    [1.0, 1.0],
                    [0.0, 1.0],
                    [0.0, 0.0],
                ]
            })
            .collect::<Vec<_>>(),
    );
}

pub(crate) fn start_stage_script(
    mut script: ResMut<StageScriptRuntime>,
    mut net: ResMut<crate::plugins::network_plugin::NetState>,
) {
    net.pending_objective_end = None;
    if let Some(runtime) = script.script_mut() {
        runtime
            .host_mut()
            .set_character_team(0, local_scene_player_team(&net));
        runtime.host_mut().set_game_mode(scene_game_mode(&net));
        if let Err(error) = runtime.start() {
            warn!("scene script start failed: {error}");
        }
    }
}

fn script_prop_is_terminally_broken(breakable: bool, active: bool, pose: u32, state: u32) -> bool {
    breakable && !active && pose != 0 && state == 3
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn tick_stage_script(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut script: ResMut<StageScriptRuntime>,
    mut net: ResMut<crate::plugins::network_plugin::NetState>,
    net_tx: Option<Res<crate::plugins::network_plugin::NetTx>>,
    mut scene_effects: MessageWriter<crate::plugins::effect_plugin::SceneEffectEvent>,
    mut collider: ResMut<crate::plugins::stage_collider::StageCollider>,
    mut props: Query<(
        &mut crate::plugins::item_plugin::StageProp,
        &mut Visibility,
        Option<&crate::plugins::render_plugin::PropColliderOwner>,
        &GlobalTransform,
    )>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(10);
    let network_room = net.status == crate::plugins::network_plugin::NetStatus::InRoom;
    let scene_authority = !network_room || net.is_scene_authority();
    if let Some(runtime) = script.script_mut() {
        let pending_ops = std::mem::take(&mut net.pending_scene_ops);
        runtime
            .host_mut()
            .set_character_team(0, local_scene_player_team(&net));
        runtime.host_mut().set_game_mode(scene_game_mode(&net));

        if network_room {
            for op in &pending_ops {
                apply_network_scene_op(runtime, op);
            }
        }
        for hit in std::mem::take(&mut net.pending_scene_prop_hits) {
            apply_network_prop_hit(runtime, &net, hit);
        }
        if scene_authority {
            let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
            if let Err(error) = runtime.update(dt as f64) {
                warn!("scene script tick failed: {error}");
            }
        }

        let forwards_net_ops = network_room && (scene_authority || net.is_stock_udp());
        if forwards_net_ops {
            let batches = runtime.host_mut().take_net_batches();
            for batch in batches {
                let ops = batch.iter().map(scene_op_from_runtime).collect::<Vec<_>>();
                if let Some(net_tx) = net_tx.as_ref() {
                    net_tx.send_control(meteor_network::protocol::Message::SceneOps {
                        room_id: net.room_id.clone(),
                        authority_generated: false,
                        ops,
                    });
                }
            }
        } else if !network_room {
            runtime.host_mut().take_net_ops();
        }
        let effect_requests = runtime
            .host_mut()
            .take_effects()
            .into_iter()
            .filter_map(|op| match op {
                SceneNetOp::Effect {
                    ref_index, effect, ..
                } => runtime
                    .host()
                    .instance(ref_index)
                    .map(|instance| (instance.name.to_ascii_lowercase(), effect)),
                SceneNetOp::Set { .. } => None,
            })
            .collect::<Vec<_>>();

        let states: std::collections::HashMap<String, (bool, bool, bool, u32, u32)> = runtime
            .host()
            .instances()
            .iter()
            .map(|instance| {
                (
                    instance.name.to_ascii_lowercase(),
                    (
                        instance.get("active").as_f64() != 0.0,
                        instance.get("interactive").as_f64() != 0.0,
                        instance.collision_enabled(),
                        instance.get("pose").as_f64().max(0.0) as u32,
                        instance.get("state").as_f64().max(0.0) as u32,
                    ),
                )
            })
            .collect();
        let mut prop_positions = std::collections::HashMap::new();
        for (mut prop, mut visibility, owner, transform) in &mut props {
            let prop_name = prop.name.to_ascii_lowercase();
            prop_positions.insert(prop_name.clone(), transform.translation());
            let Some(&(active, interactive, collision_enabled, pose, state)) =
                states.get(&prop_name)
            else {
                continue;
            };
            prop.interactive = interactive;
            prop.collision_enabled = collision_enabled;

            prop.broken = script_prop_is_terminally_broken(prop.breakable, active, pose, state);

            if prop.respawn_timer <= 0.0 {
                *visibility = if active {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                };
            }
            if let (Some(owner), Some(mesh)) = (owner, collider.mesh.as_mut()) {
                mesh.set_owner_enabled(owner.0, collision_enabled);
            }
        }
        for (prop_name, effect) in effect_requests {
            scene_effects.write(crate::plugins::effect_plugin::SceneEffectEvent {
                effect,
                position: prop_positions
                    .get(&prop_name)
                    .copied()
                    .unwrap_or(Vec3::ZERO),
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn scene_script_touch_and_transfer(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut touch: ResMut<SceneTouchState>,
    mut script: ResMut<StageScriptRuntime>,
    net: Res<crate::plugins::network_plugin::NetState>,
    collider: Res<crate::plugins::stage_collider::StageCollider>,
    props: Query<(
        &crate::plugins::item_plugin::StageProp,
        &GlobalTransform,
        &Visibility,
    )>,
    mut fighters: SceneScriptFighters,
) {
    if net.status == crate::plugins::network_plugin::NetStatus::InRoom
        && !net.is_stock_udp()
        && net.stage == "sn22"
    {
        return;
    }

    if net.status == crate::plugins::network_plugin::NetStatus::InRoom && !net.is_scene_authority()
    {
        return;
    }
    let Some(runtime) = script.script_mut() else {
        return;
    };

    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    touch.now += dt;
    touch.next_scan -= dt;
    if touch.next_scan > 0.0 {
        return;
    }

    touch.next_scan = 0.12;
    let now = touch.now;
    touch.cooldowns.retain(|_, expires| *expires > now);

    let mut positions = std::collections::HashMap::new();
    let mut targets = Vec::new();
    for (prop, transform, visibility) in &props {
        if prop.broken || *visibility == Visibility::Hidden {
            continue;
        }
        let name = prop.name.to_ascii_lowercase();
        positions
            .entry(name.clone())
            .or_insert(transform.translation());
        if !runtime.has_item_callback(&prop.name, "OnTouch")
            || targets.iter().any(|(known, _, _, _)| known == &name)
        {
            continue;
        }
        targets.push((
            name,
            prop.name.clone(),
            prop.global_center(transform),
            world_half_extents(transform, prop.half_extents),
        ));
    }

    let contacts = fighters
        .iter_mut()
        .filter_map(|(transform, npc, local)| {
            local
                .is_some()
                .then_some((0, transform.translation))
                .or_else(|| npc.map(|npc| (npc.id, transform.translation)))
        })
        .collect::<Vec<_>>();
    for (name, callback_name, center, half_extents) in &targets {
        for &(character_id, position) in &contacts {
            if !scene_touch_contains(position, *center, *half_extents) {
                continue;
            }
            let key = (name.clone(), character_id);
            if touch.cooldowns.get(&key).copied().unwrap_or(0.0) > now {
                continue;
            }
            touch.cooldowns.insert(key, now + 0.6);
            if let Err(error) = runtime.call_item(
                callback_name,
                "OnTouch",
                &[PstValue::Num(character_id as f64)],
            ) {
                warn!("script callItem OnTouch failed: {error}");
            }
        }
    }

    let transfers = runtime.host_mut().take_transfer_requests();
    let resolved = transfers
        .into_iter()
        .filter_map(|request| {
            let destination = runtime
                .host()
                .resolve_name(&request.destination)
                .and_then(|index| runtime.host().instance(index))?
                .name
                .to_ascii_lowercase();
            positions
                .get(&destination)
                .copied()
                .map(|position| (request, position))
        })
        .collect::<Vec<(SceneTransferRequest, Vec3)>>();
    for (request, position) in resolved {
        let position = snap_scene_transfer_position(&collider, position);
        for (mut transform, npc, local) in &mut fighters {
            let matches = local.is_some_and(|_| request.character_id == 0)
                || npc.is_some_and(|npc| npc.id == request.character_id);
            if matches {
                transform.translation = position;
                break;
            }
        }
    }
}

fn snap_scene_transfer_position(
    collider: &crate::plugins::stage_collider::StageCollider,
    position: Vec3,
) -> Vec3 {
    use meteor_physics::collision::MeshCollider;

    let Some(mesh) = collider.mesh.as_ref() else {
        return position;
    };

    let Some(hit) = mesh.sample_ground_hit(position.x, position.z, 0.0, position.y + 8.0) else {
        return position;
    };
    Vec3::new(position.x, hit.y, position.z)
}

fn scene_touch_contains(position: Vec3, center: Vec3, half_extents: Vec3) -> bool {
    let delta = position - center;
    delta.x.abs() <= half_extents.x + 0.5
        && delta.z.abs() <= half_extents.z + 0.5
        && delta.y >= -half_extents.y - 0.5
        && delta.y <= half_extents.y + 1.8
}

fn world_half_extents(transform: &GlobalTransform, local_half: Vec3) -> Vec3 {
    let (_, rotation, _) = transform.to_scale_rotation_translation();
    let x = (rotation * Vec3::X).abs();
    let y = (rotation * Vec3::Y).abs();
    let z = (rotation * Vec3::Z).abs();
    Vec3::new(
        x.x * local_half.x + y.x * local_half.y + z.x * local_half.z,
        x.y * local_half.x + y.y * local_half.y + z.y * local_half.z,
        x.z * local_half.x + y.z * local_half.y + z.z * local_half.z,
    )
}

fn local_scene_player_team(net: &crate::plugins::network_plugin::NetState) -> i64 {
    if net.status != crate::plugins::network_plugin::NetStatus::InRoom {
        return 0;
    }
    net.members
        .iter()
        .find(|member| member.player_id == net.player_id)
        .and_then(|member| member.team)
        .map(|team| team as i64)
        .unwrap_or(0)
}

fn scene_game_mode(net: &crate::plugins::network_plugin::NetState) -> i64 {
    if net.status == crate::plugins::network_plugin::NetStatus::InRoom {
        meteor_network::modes::mode_rule_id(&net.mode)
            .map(i64::from)
            .unwrap_or(5)
    } else {
        1
    }
}

pub(crate) fn apply_network_prop_hit(
    runtime: &mut SceneScriptRuntime,
    net: &crate::plugins::network_plugin::NetState,
    hit: meteor_network::protocol::Message,
) {
    let meteor_network::protocol::Message::ScenePropHit {
        room_id,
        match_epoch,
        round_epoch,
        player_id,
        reference,
        damage,
        ..
    } = hit
    else {
        return;
    };
    if net.status != crate::plugins::network_plugin::NetStatus::InRoom
        || !net.round_active
        || net.is_stock_udp()
        || !net.is_scene_authority()
        || room_id != net.room_id
        || net.match_epoch != Some(match_epoch)
        || net.round_epoch != Some(round_epoch)
        || damage <= 0
    {
        return;
    }
    let Some(character) = net
        .members
        .iter()
        .position(|member| member.player_id == player_id)
    else {
        return;
    };
    if runtime.host().resolve_name(&reference).is_none()
        || !super::combat_plugin::script_allows_prop_hit(runtime, &reference)
    {
        return;
    }
    if let Some(team) = net.player_team(&player_id) {
        runtime
            .host_mut()
            .set_character_team(character as i64, i64::from(team));
    }
    if let Err(error) = runtime.call_item(
        &reference,
        "OnAttack",
        &[
            PstValue::Num(character as f64),
            PstValue::Num(damage as f64),
        ],
    ) {
        warn!("network scene OnAttack failed: {error}");
    }
}

fn scene_op_from_runtime(op: &SceneNetOp) -> meteor_network::protocol::SceneOp {
    match op {
        SceneNetOp::Set {
            ref_index,
            field,
            args,
        } => meteor_network::protocol::SceneOp {
            kind: "set".into(),
            reference: JsonValue::from(*ref_index as u64),
            field: field.clone(),
            args: args.iter().map(pst_value_to_json).collect(),
        },
        SceneNetOp::Effect {
            ref_index,
            effect,
            extra,
        } => meteor_network::protocol::SceneOp {
            kind: "effect".into(),
            reference: JsonValue::from(*ref_index as u64),
            field: effect.clone(),
            args: vec![number_json(*extra)],
        },
    }
}

fn apply_network_scene_op(
    runtime: &mut SceneScriptRuntime,
    op: &meteor_network::protocol::SceneOp,
) {
    let ref_index = match &op.reference {
        JsonValue::Number(value) => value.as_u64().and_then(|value| usize::try_from(value).ok()),
        JsonValue::String(name) => runtime.host().resolve_name(name),
        _ => None,
    };
    let Some(ref_index) = ref_index else {
        return;
    };
    match op.kind.as_str() {
        "set" => {
            let Some(args) = op
                .args
                .iter()
                .map(json_to_pst_value)
                .collect::<Option<Vec<_>>>()
            else {
                return;
            };
            runtime.host_mut().apply_net_op(&SceneNetOp::Set {
                ref_index,
                field: op.field.clone(),
                args,
            });
        }
        "effect" => {
            let extra = op.args.first().and_then(JsonValue::as_f64).unwrap_or(0.0);
            runtime.host_mut().apply_net_op(&SceneNetOp::Effect {
                ref_index,
                effect: op.field.clone(),
                extra,
            });
        }
        _ => {}
    }
}

fn pst_value_to_json(value: &PstValue) -> JsonValue {
    match value {
        PstValue::Num(value) => number_json(*value),
        PstValue::Str(value) => JsonValue::String(value.clone()),
        PstValue::Array(values) => JsonValue::Array(values.iter().map(pst_value_to_json).collect()),
    }
}

fn number_json(value: f64) -> JsonValue {
    JsonNumber::from_f64(value).map_or(JsonValue::Null, JsonValue::Number)
}

fn json_to_pst_value(value: &JsonValue) -> Option<PstValue> {
    Some(match value {
        JsonValue::Number(value) => PstValue::Num(value.as_f64()?),
        JsonValue::String(value) => PstValue::Str(value.clone()),
        JsonValue::Bool(value) => PstValue::Num(f64::from(*value)),
        JsonValue::Null => PstValue::Num(0.0),
        JsonValue::Array(values) => PstValue::Array(
            values
                .iter()
                .map(json_to_pst_value)
                .collect::<Option<_>>()?,
        ),
        JsonValue::Object(_) => return None,
    })
}

#[allow(clippy::too_many_arguments)]
fn spawn_mode_objectives(
    mut commands: Commands,
    request: Res<crate::app_state::LaunchRequest>,
    net: Res<crate::plugins::network_plugin::NetState>,
    mission: Res<StageMissionRuntime>,
    escort: Res<super::escort_assets::LoadedEscort>,
    waypoints: Option<Res<WaypointGraph>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !local_mode_objectives_enabled(net.status) {
        return;
    }

    let waypoint_pos = |index: usize| -> Option<Vec3> {
        waypoints
            .as_deref()
            .and_then(|graph| graph.world_position(index))
            .map(|[x, y, z]| Vec3::new(x, y, z))
    };
    let mode = if local_escort_enabled(&request, &mission) {
        "escort"
    } else {
        request.mode.as_str()
    };
    match mode {
        "escort" => {
            let Some(definition) = escort.0.as_deref() else {
                warn!("escort objective unavailable: no valid authored definition");
                return;
            };
            let home = definition.stage.flag_home;
            let position = Vec3::new(home.x, home.y, home.z);
            commands.spawn((
                ModeObjective {
                    kind: ModeObjectiveKind::EscortCargo,
                },
                LocalEscortState::default(),
                Transform::from_translation(position),
                Visibility::Visible,
            ));
            info!("escort cargo spawned at {position:?}");
        }
        "siege" => {
            let position = waypoint_pos(0)
                .map(|mut p| {
                    p.y += 0.6;
                    p
                })
                .unwrap_or_else(|| Vec3::new(0.0, 1.2, -6.0));
            commands.spawn((
                ModeObjective {
                    kind: ModeObjectiveKind::SiegeTarget,
                },
                crate::plugins::item_plugin::StageProp {
                    name: "D_mode_siege".into(),
                    model: "mode_siege".into(),
                    center_offset: Vec3::ZERO,
                    half_extents: Vec3::splat(0.8),
                    hit_count: 0,
                    respawn_timer: 0.0,
                    interactive: false,
                    collision_enabled: true,
                    breakable: true,
                    broken: false,
                },
                Mesh3d(meshes.add(Cuboid::new(1.2, 1.2, 1.2))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgb(0.9, 0.2, 0.2),
                    unlit: true,
                    ..default()
                })),
                Transform::from_translation(position),
            ));
            info!("siege target spawned at {position:?}");
        }
        _ => {}
    }
}

fn sync_network_escort_flag(
    mut commands: Commands,
    net: Res<crate::plugins::network_plugin::NetState>,
    request: Res<crate::app_state::LaunchRequest>,
    mut flags: Query<(Entity, &mut Transform, &mut Visibility), With<NetworkEscortFlag>>,
) {
    let is_network_escort =
        net.status == crate::plugins::network_plugin::NetStatus::InRoom && request.mode == "escort";
    let Some(flag) = is_network_escort
        .then_some(net.escort_flag.as_ref())
        .flatten()
    else {
        for (entity, _, _) in &mut flags {
            commands.entity(entity).despawn();
        }
        return;
    };
    let position = Vec3::from_array(flag.position);
    let visibility = if network_escort_visible(&flag.state) {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    if let Some((_, mut transform, mut current_visibility)) = flags.iter_mut().next() {
        transform.translation = position;
        *current_visibility = visibility;
        return;
    }
    if visibility == Visibility::Hidden {
        return;
    }
    commands.spawn((
        NetworkEscortFlag,
        Transform::from_translation(position),
        visibility,
    ));
}

fn escort_safe_zone_bounds(
    team: u8,
    stage: &meteor_network::escort::EscortStage,
) -> Option<(Vec3, Vec3)> {
    let zone = stage.safe_zones.get(team.checked_sub(1)? as usize)?;
    let min = Vec3::new(zone.min_x, zone.min_y, zone.min_z);
    let max = Vec3::new(zone.max_x, zone.max_y, zone.max_z);
    Some(((min + max) * 0.5, max - min))
}

fn spawn_escort_safe_zone(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    team: u8,
    stage: &meteor_network::escort::EscortStage,
) -> Option<Entity> {
    let (center, size) = escort_safe_zone_bounds(team, stage)?;
    let color = if team == 1 {
        Color::srgb(0.31, 0.65, 0.56)
    } else {
        Color::srgb(0.72, 0.24, 0.26)
    };
    let volume_material = materials.add(StandardMaterial {
        base_color: color.with_alpha(0.11),
        emissive: LinearRgba::from(color) * 0.08,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    let edge_material = materials.add(StandardMaterial {
        base_color: color.with_alpha(0.85),
        emissive: LinearRgba::from(color) * 0.35,
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    let root = commands
        .spawn((
            NetworkEscortSafeZone { team },
            Mesh3d(meshes.add(Cuboid::new(size.x, size.y, size.z))),
            MeshMaterial3d(volume_material),
            Transform::from_translation(center),
        ))
        .id();

    let thickness = 0.12;
    let edge_specs = [
        (Vec3::new(size.x, thickness, thickness), Vec3::X),
        (Vec3::new(thickness, size.y, thickness), Vec3::Y),
        (Vec3::new(thickness, thickness, size.z), Vec3::Z),
    ];
    commands.entity(root).with_children(|parent| {
        for (dimensions, axis) in edge_specs {
            for a in [-1.0, 1.0] {
                for b in [-1.0, 1.0] {
                    let offset = if axis == Vec3::X {
                        Vec3::new(0.0, a * size.y * 0.5, b * size.z * 0.5)
                    } else if axis == Vec3::Y {
                        Vec3::new(a * size.x * 0.5, 0.0, b * size.z * 0.5)
                    } else {
                        Vec3::new(a * size.x * 0.5, b * size.y * 0.5, 0.0)
                    };
                    parent.spawn((
                        Mesh3d(meshes.add(Cuboid::new(dimensions.x, dimensions.y, dimensions.z))),
                        MeshMaterial3d(edge_material.clone()),
                        Transform::from_translation(offset),
                    ));
                }
            }
        }
    });
    Some(root)
}

fn sync_network_escort_safe_zones(
    mut commands: Commands,
    request: Res<crate::app_state::LaunchRequest>,
    mission: Res<StageMissionRuntime>,
    escort: Res<super::escort_assets::LoadedEscort>,
    zones: Query<(Entity, &NetworkEscortSafeZone)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let active = local_escort_enabled(&request, &mission) && escort.0.is_some();
    if !active {
        for (entity, _) in &zones {
            commands.entity(entity).despawn();
        }
        return;
    }
    for team in [1, 2] {
        if zones.iter().any(|(_, zone)| zone.team == team) {
            continue;
        }
        let _ = spawn_escort_safe_zone(
            &mut commands,
            &mut meshes,
            &mut materials,
            team,
            &escort.0.as_ref().expect("active definition").stage,
        );
    }
}

fn clear_mode_objective_visuals(
    mut commands: Commands,
    objectives: ModeObjectiveVisualQuery<'_, '_>,
) {
    for entity in &objectives {
        commands.entity(entity).despawn();
    }
}

type LocalEscortFighters<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Health,
        Option<&'static LocalFighter>,
        Option<&'static MissionNpc>,
        Option<&'static EscortCarrying>,
    ),
    With<Fighter>,
>;

fn escort_position(position: Vec3) -> meteor_network::role_state::RoleVec3 {
    meteor_network::role_state::RoleVec3 {
        x: position.x,
        y: position.y,
        z: position.z,
    }
}

#[allow(clippy::too_many_arguments)]
fn escort_cargo_pickup(
    mut commands: Commands,
    request: Res<crate::app_state::LaunchRequest>,
    net: Res<crate::plugins::network_plugin::NetState>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    end: Res<crate::components::fighter::MatchEnd>,
    escort: Res<super::escort_assets::LoadedEscort>,
    mut mission: ResMut<StageMissionRuntime>,
    fighters: LocalEscortFighters<'_, '_>,
    mut cargos: Query<(&mut LocalEscortState, &mut Transform, &mut Visibility), Without<Fighter>>,
) {
    if !local_mode_objectives_enabled(net.status)
        || !local_escort_enabled(&request, &mission)
        || end.settled
    {
        return;
    }
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let Some(definition) = escort.0.as_deref() else {
        return;
    };
    let home = definition.stage.flag_home;
    let mut carrier_script_id = None;
    for (mut state, mut transform, mut visibility) in &mut cargos {
        let mut can_pick_up = state.carrier.is_none();
        if let Some(carrier) = state.carrier {
            match fighters.get(carrier) {
                Ok((_, pose, health, _, npc, _)) if npc.is_none_or(|npc| npc.in_scene_roster) => {
                    transform.translation = pose.translation;
                    if health.current <= 0.0 {
                        state.carrier = None;
                        state.return_remaining = Some(definition.stage.return_seconds);
                    }
                }
                _ => {
                    state.carrier = None;
                    state.return_remaining = None;
                    transform.translation = Vec3::new(home.x, home.y, home.z);
                }
            }
        } else if let Some(remaining) = state.return_remaining {
            if remaining - dt <= 0.0 {
                state.return_remaining = None;
                transform.translation = Vec3::new(home.x, home.y, home.z);
                can_pick_up = false;
            } else {
                state.return_remaining = Some(remaining - dt);
            }
        }

        if can_pick_up {
            state.carrier = fighters
                .iter()
                .filter(|(_, _, health, local, npc, _)| {
                    health.current > 0.0
                        && (local.is_some() || npc.is_some_and(|npc| npc.in_scene_roster))
                })
                .filter(|(_, pose, _, _, _, _)| {
                    definition.intersects(
                        escort_position(pose.translation),
                        escort_position(transform.translation),
                    )
                })
                .min_by_key(|(_, _, _, _, npc, _)| npc.map_or(0, |npc| npc.id))
                .map(|(entity, _, _, _, _, _)| entity);
        }
        if state.carrier.is_some() {
            state.return_remaining = None;
        }
        *visibility = if state.carrier.is_some() {
            Visibility::Hidden
        } else {
            Visibility::Visible
        };
        for (entity, _, _, _, npc, carrying) in &fighters {
            if Some(entity) == state.carrier {
                carrier_script_id = Some(npc.map_or(0, |npc| npc.id));
                if carrying.is_none() {
                    commands.entity(entity).insert(EscortCarrying);
                }
            } else if carrying.is_some() {
                commands.entity(entity).remove::<EscortCarrying>();
            }
        }
    }
    if let Some(runtime) = mission.script_mut() {
        runtime.host_mut().sync_flag_carrier(carrier_script_id);
    }
}

#[allow(clippy::too_many_arguments)]
fn mode_objective_tick(
    mut end: ResMut<crate::components::fighter::MatchEnd>,
    request: Res<crate::app_state::LaunchRequest>,
    net: Res<crate::plugins::network_plugin::NetState>,
    mission: Res<StageMissionRuntime>,
    escort: Res<super::escort_assets::LoadedEscort>,
    fighters: LocalEscortFighters<'_, '_>,
    objectives: Query<(
        &ModeObjective,
        Option<&crate::plugins::item_plugin::StageProp>,
    )>,
    cargos: Query<&LocalEscortState>,
) {
    if !local_mode_objectives_enabled(net.status) || end.settled || end.result != 0 {
        return;
    }
    let mode = if local_escort_enabled(&request, &mission) {
        "escort"
    } else {
        request.mode.as_str()
    };
    match mode {
        "siege" => {
            let broken = objectives.iter().any(|(objective, prop)| {
                matches!(objective.kind, ModeObjectiveKind::SiegeTarget)
                    && prop.map(|prop| prop.broken).unwrap_or(false)
            });
            if broken {
                end.result = meteor_network::modes::ROUND_RESULT_TEAM2_WIPED;
                end.settled = true;
                end.settle_elapsed = 0.0;
                info!("siege: objective destroyed — victory");
            }
        }
        "escort" => {
            let Some(definition) = escort.0.as_deref() else {
                return;
            };
            let delivered = cargos
                .iter()
                .filter_map(|state| state.carrier)
                .filter_map(|carrier| fighters.get(carrier).ok())
                .find_map(|(_, transform, health, _, npc, _)| {
                    let team = npc.map_or(1, |npc| npc.team as i32);
                    (health.current > 0.0
                        && meteor_network::escort::position_inside_safe_zone(
                            escort_position(transform.translation),
                            team,
                            &definition.stage,
                        ))
                    .then_some(team)
                });
            if let Some(team) = delivered {
                end.result = if team == 1 {
                    meteor_network::modes::ROUND_RESULT_TEAM2_WIPED
                } else {
                    meteor_network::modes::ROUND_RESULT_TEAM1_WIPED
                };
                end.settled = true;
                end.settle_elapsed = 0.0;
                info!("escort: cargo delivered — victory");
            }
        }
        _ => {}
    }
}

fn start_mission_script(mut script: ResMut<StageMissionRuntime>) {
    if let Some(runtime) = script.script_mut() {
        if let Err(error) = runtime.start() {
            warn!("mission script start failed: {error}");
        }
    }
}

fn tick_mission_script(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    request: Res<LaunchRequest>,
    mut script: ResMut<StageMissionRuntime>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(20);
    if request.mode == "replay" {
        return;
    }
    if let Some(runtime) = script.script_mut() {
        let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
        if let Err(error) = runtime.update(dt as f64) {
            warn!("mission script tick failed: {error}");
        }
    }
}

fn consume_mission_npc_requests(
    mut commands: Commands,
    request: Res<LaunchRequest>,
    mut script: ResMut<StageMissionRuntime>,
    mut pending: ResMut<PendingMissionNpcCommands>,
    mut player_performance: ResMut<PlayerPerformance>,
    waypoints: Option<Res<WaypointGraph>>,
    mut npcs: Query<(
        Entity,
        &mut MissionNpc,
        &mut crate::components::fighter::AiIntent,
        &mut NpcPerformance,
        &mut crate::plugins::ai_plugin::NpcPatrol,
        Option<&mut crate::components::fighter::SmState>,
    )>,
) {
    if request.mode == "replay" {
        return;
    }

    let (removes, adds) = {
        let Some(runtime) = script.script_mut() else {
            return;
        };
        split_npc_requests(runtime.host_mut().take_npc_requests())
    };
    for id in removes {
        if let Some(runtime) = script.script_mut() {
            runtime.host_mut().remove_fighter(id);
        }
        for (entity, npc, _intent, _performance, _patrol, _sm) in npcs.iter() {
            if npc.id == id {
                info!("mission RemoveNPC({id}) — despawn");
                commands.entity(entity).despawn();
                break;
            }
        }
    }
    for (npc_script, id) in adds {
        for (entity, npc, _intent, _performance, _patrol, _sm) in npcs.iter() {
            if npc.id == id {
                commands.entity(entity).despawn();
                break;
            }
        }
        spawn_mission_npc(
            &mut commands,
            &mut script,
            waypoints.as_deref(),
            &npc_script,
            id,
        );
    }
    let stops = script
        .script_mut()
        .map(|runtime| runtime.host_mut().take_stop_perform_requests())
        .unwrap_or_default();
    for id in stops {
        if id == 0 {
            player_performance.queue.clear();
            player_performance.clock = default();
            player_performance.pause_remaining = 0.0;
            player_performance.use_items.clear();
        }
        pending.performs.retain(|(target, _, _)| *target != id);
        for (_entity, npc, mut intent, mut performance, _patrol, _sm) in npcs.iter_mut() {
            if npc.id == id {
                intent.want_taunt = false;
                intent.want_guard = false;
                performance.queue.clear();
                performance.clock = default();
                performance.pause_remaining = 0.0;
                performance.guard_remaining = 0.0;
                performance.jump_requested = false;
                performance.use_items.clear();
                break;
            }
        }
    }
    if let Some(runtime) = script.script_mut() {
        for (action, args) in runtime.host_mut().take_player_perform_requests() {
            player_performance.queue.push_back(NpcPerformCommand {
                action: action.trim().to_ascii_lowercase(),
                args,
            });
        }
    }

    let mut behaviors = std::mem::take(&mut pending.behaviors);
    behaviors.extend({
        let Some(runtime) = script.script_mut() else {
            return;
        };
        runtime.host_mut().take_behavior_requests()
    });
    for (id, behavior, args) in behaviors {
        let mut applied = false;
        for (_entity, mut npc, mut intent, _performance, mut patrol, sm) in npcs.iter_mut() {
            if npc.id == id {
                patrol.reset_behavior_state();

                *intent = default();
                npc.forced_behavior = behavior_from_name(&behavior);
                if matches!(
                    npc.forced_behavior,
                    Some(
                        meteor_ai::behavior::BehaviorMode::AttackTarget
                            | meteor_ai::behavior::BehaviorMode::Idle
                            | meteor_ai::behavior::BehaviorMode::Dodge
                    )
                ) {
                    if let Some(mut sm) = sm {
                        sm.attack_queued = None;
                    }
                }
                npc.behavior_args = args.clone();
                if behavior.eq_ignore_ascii_case("patrol") {
                    patrol.route = args
                        .iter()
                        .map(|value| value.as_f64())
                        .filter(|value| value.is_finite() && *value >= 0.0)
                        .map(|value| value as usize)
                        .collect();
                }
                npc.attack_target_remaining = 0;

                if behavior.eq_ignore_ascii_case("attacktarget") {
                    npc.attack_target_slot = args.first().map(|value| value.as_f64() as i64);
                    npc.attack_target_remaining = args
                        .get(1)
                        .map(|value| value.as_f64() as i64)
                        .unwrap_or(0)
                        .max(0);
                } else {
                    npc.attack_target_slot = None;
                }
                if behavior.eq_ignore_ascii_case("patrol") {
                    patrol.route = args
                        .iter()
                        .map(|value| value.as_f64())
                        .filter(|value| value.is_finite() && *value >= 0.0)
                        .map(|value| value as usize)
                        .collect();
                    patrol.index = 0;
                }
                debug!("mission ChangeBehavior({id}, \"{behavior}\", {args:?}) applied");
                applied = true;
                break;
            }
        }
        if !applied {
            pending.behaviors.push((id, behavior, args));
        }
    }

    let mut performs = std::mem::take(&mut pending.performs);
    performs.extend({
        let Some(runtime) = script.script_mut() else {
            return;
        };
        runtime.host_mut().take_perform_requests()
    });
    for (id, action, args) in performs {
        let mut applied = false;
        for (_entity, npc, _intent, mut performance, _patrol, _sm) in npcs.iter_mut() {
            if npc.id != id {
                continue;
            }
            performance.queue.push_back(NpcPerformCommand {
                action: action.trim().to_ascii_lowercase(),
                args: args.clone(),
            });
            applied = true;
            break;
        }
        if !applied {
            pending.performs.push((id, action, args));
        }
    }
}

fn consume_mission_attribute_updates(
    mut script: ResMut<StageMissionRuntime>,
    mut graph: Option<ResMut<WaypointGraph>>,
    mut npcs: Query<(
        &mut MissionNpc,
        &mut NpcCombatProfile,
        &mut crate::plugins::ai_plugin::NpcPatrol,
    )>,
) {
    let Some(runtime) = script.script_mut() else {
        return;
    };
    let updates = runtime.host_mut().take_ai_updates();
    for (id, values) in updates {
        let Some((mut npc, mut profile, _)) = npcs.iter_mut().find(|(npc, _, _)| npc.id == id)
        else {
            runtime.host_mut().defer_ai_update(id, values);
            continue;
        };
        let value = |name: &str| values.get(name).map(PstValue::as_f64).unwrap_or(0.0) as f32;
        npc.view = value("View").max(0.0);
        npc.think = value("Think").clamp(0.0, 100.0);
        *profile = NpcCombatProfile {
            attack1: value("Attack1"),
            attack2: value("Attack2"),
            attack3: value("Attack3"),
            guard: value("Guard"),
            dodge: value("Dodge"),
            jump: value("Jump"),
            look: value("Look"),
            burst: value("Burst"),
            aim: value("Aim"),
            get_item: value("GetItem"),
        };
    }
    let mut changed = false;
    for (id, enabled) in runtime.host_mut().take_waypoint_updates() {
        if let Some(graph) = graph.as_deref_mut() {
            changed |= graph.set_enabled(id, enabled);
        }
    }
    if changed {
        for (_, _, mut patrol) in &mut npcs {
            patrol.invalidate_navigation();
        }
    }
}

fn consume_mission_say_requests(
    mut script: ResMut<StageMissionRuntime>,
    mut dialogue: Option<ResMut<crate::plugins::ui_plugin::DialogueState>>,
) {
    let Some(runtime) = script.script_mut() else {
        return;
    };
    for (name, text) in runtime.host_mut().take_say_requests() {
        if let Some(dialogue) = dialogue.as_deref_mut() {
            dialogue.text = Some(format!("{name}：{text}"));
            dialogue.timer = 3.0;
        }
    }
}

fn assign_assassin_vip(
    request: Res<crate::app_state::LaunchRequest>,
    mission: Option<Res<StageMissionRuntime>>,
    mut npcs: Query<&mut MissionNpc>,
) {
    if request.mode == "single" {
        if let Some(runtime) = mission.as_deref().and_then(StageMissionRuntime::script) {
            for mut npc in &mut npcs {
                npc.is_vip = runtime.host().vip(npc.team) == Some(npc.id);
            }
        }
        return;
    }
    if request.mode != "assassin"
        || npcs
            .iter()
            .any(|npc| npc.in_scene_roster && npc.team == 2 && npc.is_vip)
    {
        return;
    }
    if let Some(mut npc) = npcs
        .iter_mut()
        .find(|npc| npc.in_scene_roster && npc.team == 2)
    {
        npc.is_vip = true;
        info!("assassin VIP assigned: {} (id {})", npc.name, npc.id);
    }
}

fn split_npc_requests(requests: Vec<NpcRequest>) -> (Vec<i64>, Vec<(String, i64)>) {
    let mut removes = Vec::new();
    let mut adds = Vec::new();
    for request in requests {
        match request {
            NpcRequest::Remove { id } => removes.push(id),
            NpcRequest::Add { script, id } => adds.push((script, id)),
        }
    }
    (removes, adds)
}

fn behavior_from_name(name: &str) -> Option<meteor_ai::behavior::BehaviorMode> {
    match name.to_ascii_lowercase().as_str() {
        "idle" | "stop" => Some(meteor_ai::behavior::BehaviorMode::Idle),
        "wait" => Some(meteor_ai::behavior::BehaviorMode::Wait),
        "run" => Some(meteor_ai::behavior::BehaviorMode::Run),
        "patrol" => Some(meteor_ai::behavior::BehaviorMode::Patrol),
        "follow" | "chase" => Some(meteor_ai::behavior::BehaviorMode::Follow),
        "attack" | "fight" => Some(meteor_ai::behavior::BehaviorMode::Attack),
        "attacktarget" => Some(meteor_ai::behavior::BehaviorMode::AttackTarget),
        "kill" => Some(meteor_ai::behavior::BehaviorMode::Kill),
        "dodge" => Some(meteor_ai::behavior::BehaviorMode::Dodge),
        "defend" | "escape" => Some(meteor_ai::behavior::BehaviorMode::Defend),
        _ => None,
    }
}

fn spawn_mission_npc(
    commands: &mut Commands,
    script: &mut ResMut<StageMissionRuntime>,
    waypoints: Option<&WaypointGraph>,
    npc_script: &str,
    id: i64,
) {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let roots = resource_roots(&root);
    let legacy_path = format!("Level/{npc_script}.pst");
    let bytes = meteor_asset::runtime_assets::read_legacy_path(&legacy_path).or_else(|| {
        roots
            .iter()
            .find_map(|root| crate::resource_io::read(root.join(&legacy_path)).ok())
    });
    let Some(bytes) = bytes else {
        warn!("mission AddNPC(\"{npc_script}\") — {npc_script}.pst missing");
        return;
    };
    let text = crate::text_codec::decode_text(&bytes);
    let definition = meteor_scene::mission_runtime::parse_npc_definition(&text, npc_script);

    let initial_forced_behavior = Some(meteor_ai::behavior::BehaviorMode::Wait);
    let initial_behavior_args = Vec::new();
    let initial_attack_target_slot = None;
    let initial_attack_target_remaining = 0;
    let initial_patrol = crate::plugins::ai_plugin::NpcPatrol::default();
    let character_id = definition.model.clamp(0, 19) as u8;
    let primary = crate::plugins::character_plugin::weapon_slot_from_item(definition.weapon)
        .unwrap_or((0, 0));
    let secondary = crate::plugins::character_plugin::weapon_slot_from_item(definition.weapon2)
        .unwrap_or((0, 0));
    let weapon = primary.0;

    let team = definition.team.clamp(0, 2) as u8;
    if let Some(runtime) = script.script_mut() {
        runtime.host_mut().register_vip_candidate(id, team);
    }
    info!(
        "mission AddNPC(\"{npc_script}\") id={id} {} p{character_id} weapon {weapon} team {team}",
        definition.name
    );

    let position = waypoints
        .and_then(|graph| graph.world_position(definition.spawn as usize))
        .map(|[x, y, z]| Vec3::new(x, y, z))
        .unwrap_or_else(|| Vec3::new(0.0, 0.9, 2.0 + (id as f32) * 2.0));
    commands
        .spawn((
            (
                crate::plugins::combat_plugin::NpcFighter,
                MissionNpc {
                    id,
                    name: definition.name.clone(),
                    team,
                    in_scene_roster: true,
                    view: definition.view.max(0) as f32,
                    think: definition.think.clamp(0, 100) as f32,
                    forced_behavior: initial_forced_behavior,
                    is_vip: false,
                    attack_target_slot: initial_attack_target_slot,
                    behavior_args: initial_behavior_args,
                    attack_target_remaining: initial_attack_target_remaining,
                },
                NpcCombatProfile {
                    attack1: definition.attack1.clamp(0, 100) as f32,
                    attack2: definition.attack2.clamp(0, 100) as f32,
                    attack3: definition.attack3.clamp(0, 100) as f32,
                    guard: definition.guard.clamp(0, 100) as f32,
                    dodge: definition.dodge.clamp(0, 100) as f32,
                    jump: definition.jump.clamp(0, 100) as f32,
                    look: definition.look.clamp(0, 100) as f32,
                    burst: definition.burst.clamp(0, 100) as f32,
                    aim: definition.aim.clamp(0, 100) as f32,
                    get_item: definition.get_item.clamp(0, 100) as f32,
                },
                NpcPerformance::default(),
            ),
            Fighter {
                character_id,
                player_slot: 1,
                weapon,
                weapon_slots: [primary.0, secondary.0],
                weapon_slot: 0,
                weapon_slot_variants: [primary.1, secondary.1],
                weapon_variant: primary.1,
            },
            Health {
                current: definition.hp.max(1) as f32,
                max: definition.hp.max(1) as f32,
            },
            Qi::default(),
            CombatState::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,

                facing: std::f32::consts::PI,
            },
            crate::components::fighter::PoseRuntime::idle(),
            crate::components::fighter::SmState::default(),
            crate::components::fighter::DeathRespawn {
                spawn_pos: position,
                ..default()
            },
            crate::plugins::movement_plugin::FighterAir::default(),
            crate::plugins::movement_plugin::FighterDodge::default(),
            crate::components::fighter::AiIntent::default(),
            initial_patrol,
            Transform::from_translation(position),
        ))
        .insert(Visibility::default())
        .insert(InheritedVisibility::default())
        .insert(crate::plugins::movement_plugin::FighterAttackMotion::default())
        .insert(crate::components::fighter::VisualRootPin::default())
        .insert(crate::components::fighter::EntryBlend::default())
        .insert(crate::components::fighter::TargetHitClock::default());

    if let Some(runtime) = script.script_mut() {
        let host = runtime.host_mut();
        host.register_char(&definition.name, id);
        host.sync_fighter(id, definition.hp as f64, definition.hp as f64, team as f64);
        if let Err(error) = runtime.activate_npc_script(id, &text) {
            warn!("mission NPC {npc_script} startup failed: {error}");
        }
    }
}

fn sync_mission_spatial(
    mut script: ResMut<StageMissionRuntime>,
    players: Query<&Transform, With<LocalFighter>>,
    npcs: Query<(&MissionNpc, &Transform)>,
    waypoints: Option<Res<WaypointGraph>>,
) {
    let Some(runtime) = script.script_mut() else {
        return;
    };
    let host = runtime.host_mut();
    host.begin_actor_position_sync();
    if let Ok(transform) = players.single() {
        host.sync_actor_position(0, transform.translation.to_array());
    }
    for (npc, transform) in &npcs {
        if !npc.in_scene_roster {
            continue;
        }
        host.sync_actor_position(npc.id, transform.translation.to_array());
    }
    if let Some(waypoints) = waypoints {
        for id in 0..waypoints.points.len() {
            if let Some(position) = waypoints.world_position(id) {
                host.sync_waypoint_position(id as i64, position);
            }
        }
    }
}

fn sync_mission_fighters(
    mut script: ResMut<StageMissionRuntime>,
    players: Query<&Health, With<LocalFighter>>,
    player_performance: Option<Res<PlayerPerformance>>,
    npcs: Query<(
        &MissionNpc,
        &Health,
        &NpcPerformance,
        &crate::components::fighter::AiIntent,
    )>,
) {
    let Some(runtime) = script.script_mut() else {
        return;
    };
    let host = runtime.host_mut();
    if let Some(performance) = player_performance {
        host.sync_performing(
            0,
            performance.pause_remaining > 0.0 || !performance.queue.is_empty(),
        );
    }
    if let Ok(health) = players.single() {
        host.sync_fighter(0, health.current as f64, health.max as f64, 0.0);
    }
    for (npc, health, performance, intent) in &npcs {
        if !npc.in_scene_roster {
            continue;
        }
        host.sync_fighter(
            npc.id,
            health.current as f64,
            health.max as f64,
            npc.team as f64,
        );
        host.sync_performing(
            npc.id,
            performance.pause_remaining > 0.0
                || performance.guard_remaining > 0.0
                || performance.jump_requested
                || !performance.queue.is_empty(),
        );
        host.sync_enemy(npc.id, intent.enemy_id);
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_guard_counts_eligible_pulses_and_keeps_following_command_queued() {
        let mut clock = PerformanceClock::default();
        let mut queue = std::collections::VecDeque::from([
            NpcPerformCommand {
                action: "guard".into(),
                args: vec![PstValue::Num(3.0)],
            },
            NpcPerformCommand {
                action: "say".into(),
                args: vec![],
            },
        ]);
        for _ in 0..40 {
            assert!(clock.next_eligible(&mut queue, 0.1, false, false).is_none());
        }
        assert!(!clock.guard_held());
        for _ in 0..24 {
            assert!(clock.next_eligible(&mut queue, 0.1, true, true).is_none());
        }
        assert!(clock.guard_held());
        assert_eq!(queue.len(), 2);
        for _ in 0..40 {
            assert!(clock.next_eligible(&mut queue, 0.1, false, false).is_none());
        }
        for _ in 0..3 {
            assert!(clock.next_eligible(&mut queue, 0.1, true, true).is_none());
        }
        assert_eq!(
            clock
                .next_eligible(&mut queue, 0.1, true, true)
                .unwrap()
                .action,
            "guard"
        );
        assert!(!clock.guard_held());
        assert_eq!(queue.len(), 1);
        for _ in 0..3 {
            assert!(clock.next(&mut queue, 0.1).is_none());
        }
        assert_eq!(clock.next(&mut queue, 0.1).unwrap().action, "say");
    }

    fn escort_test_app(network: bool) -> (App, Entity) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let text =
            crate::text_codec::decode_text(&std::fs::read(root.join("Level/sn03_.pst")).unwrap());
        let definition = meteor_scene::mission_runtime::parse_mission_definition(&text);
        let mut mission = MissionScriptRuntime::new(&text, definition.values);
        mission.host_mut().sync_fighter(0, 100.0, 100.0, 0.0);
        let mut app = App::new();
        let net = crate::plugins::network_plugin::NetState {
            status: if network {
                crate::plugins::network_plugin::NetStatus::InRoom
            } else {
                crate::plugins::network_plugin::NetStatus::Disconnected
            },
            ..default()
        };
        app.insert_resource(LaunchRequest {
            stage_id: "sn03".into(),
            mode: "single".into(),
            ..default()
        })
        .insert_resource(net)
        .insert_resource(StageMissionRuntime(Some(mission)))
        .insert_resource(crate::plugins::escort_assets::LoadedEscort(Some(
            crate::plugins::escort_assets::load_escort("sn03").unwrap(),
        )))
        .init_resource::<Time>()
        .init_resource::<crate::plugins::FrameClock>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<crate::components::fighter::MatchEnd>()
        .add_systems(Startup, spawn_mode_objectives)
        .add_systems(Update, (escort_cargo_pickup, mode_objective_tick).chain());
        let home = meteor_network::escort::SN03.flag_home;
        let player = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health {
                    current: 100.0,
                    max: 100.0,
                },
                Transform::from_xyz(home.x, home.y, home.z),
            ))
            .id();
        (app, player)
    }

    #[test]
    fn sn04_reinforcement_order_replaces_old_combat_intent() {
        use crate::components::fighter::{AiIntent, SmState};
        use crate::plugins::ai_plugin::drive_npc_ai;
        use bevy::ecs::system::RunSystemOnce;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
        let read =
            |name: &str| crate::text_codec::decode_text(&std::fs::read(root.join(name)).unwrap());
        for (dead, messenger_id) in [
            (vec![], 7),
            (vec![7], 6),
            (vec![6, 7], 3),
            (vec![3, 6, 7], 2),
        ] {
            let source = read("sn04_.pst");
            let definition = meteor_scene::mission_runtime::parse_mission_definition(&source);
            let mut runtime = MissionScriptRuntime::new(&source, definition.values);
            for npc in definition.npc_scripts {
                runtime
                    .register_npc_script(&npc, &read(&format!("{npc}.pst")))
                    .unwrap();
            }
            runtime.start().unwrap();
            runtime.host_mut().sync_fighter(0, 2000., 2000., 0.);
            runtime.update(0.1).unwrap();
            let mut app = App::new();
            app.insert_resource(StageMissionRuntime(Some(runtime)))
                .insert_resource(LaunchRequest::default())
                .init_resource::<PendingMissionNpcCommands>()
                .init_resource::<PlayerPerformance>()
                .init_resource::<Time>()
                .insert_resource(meteor_scene::stage_loader::parse_waypoint_graph(
                    &std::fs::read_to_string(root.join("sn04.wp")).unwrap(),
                    meteor_scene::stage_loader::get_stage_center("sn04", None),
                ));
            app.world_mut()
                .run_system_once(consume_mission_npc_requests)
                .unwrap();
            app.world_mut()
                .run_system_once(consume_mission_npc_requests)
                .unwrap();
            let player = app
                .world_mut()
                .spawn((
                    LocalFighter,
                    Fighter {
                        character_id: 0,
                        player_slot: 0,
                        weapon: 6,
                        weapon_slots: [6, 0],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    },
                    Health::default(),
                    Transform::default(),
                ))
                .id();
            let mut query = app.world_mut().query::<(Entity, &MissionNpc)>();
            let actors: Vec<_> = query
                .iter(app.world())
                .map(|(entity, npc)| (npc.id, entity))
                .collect();
            let messenger = actors.iter().find(|(id, _)| *id == messenger_id).unwrap().1;

            for &(id, entity) in &actors {
                *app.world_mut().get_mut::<NpcPerformance>(entity).unwrap() = default();
                if dead.contains(&id) {
                    app.world_mut().get_mut::<Health>(entity).unwrap().current = 0.;
                }
            }
            let position = app.world().get::<Transform>(messenger).unwrap().translation;
            app.world_mut()
                .get_mut::<Transform>(player)
                .unwrap()
                .translation = position + Vec3::Z * 2.;
            *app.world_mut().get_mut::<AiIntent>(messenger).unwrap() = AiIntent {
                enemy_id: Some(0),
                enemy_entity: Some(player),
                lock_target: Some(player),
                ranged_aim: Some(Vec3::Z),
                want_attack: true,
                want_guard: true,
                want_qi_burst: true,
                dodge_burst: Some(false),
                ..default()
            };
            {
                let mut sm = app.world_mut().get_mut::<SmState>(messenger).unwrap();
                sm.current = meteor_core::FighterState::Attack1;
                sm.attack_queued = Some(meteor_combat::state_machine::QueuedAttack {
                    source_pose_id: 200,
                    next_pose_id: 201,
                    last_evaluated_frame: 0,
                    transition_time: 0.,
                    anger_cost: 0,
                    action_id: 0,
                });
            }
            {
                let mut script = app.world_mut().resource_mut::<StageMissionRuntime>();
                let rt = script.script_mut().unwrap();
                for id in &dead {
                    rt.host_mut().sync_fighter(*id, 0., 2500., 2.);
                }
                rt.host_mut().sync_enemy(1, Some(0));
                rt.host_mut().sync_fighter(1, 2400., 5000., 2.);
                rt.update(0.1).unwrap();
                assert_eq!(rt.host().target(0), Some(("waypoint", 1.)));
            }
            app.world_mut()
                .run_system_once(consume_mission_npc_requests)
                .unwrap();
            let npc = app.world().get::<MissionNpc>(messenger).unwrap();
            assert_eq!(
                npc.forced_behavior,
                Some(meteor_ai::behavior::BehaviorMode::AttackTarget)
            );
            assert_eq!(npc.attack_target_remaining, 0);
            let intent = app.world().get::<AiIntent>(messenger).unwrap();
            assert!(
                !intent.want_attack
                    && !intent.want_guard
                    && !intent.want_qi_burst
                    && intent.dodge_burst.is_none(),
                "messenger {messenger_id} retained old combat requests: {intent:?}"
            );
            assert!(intent.lock_target.is_none() && intent.enemy_entity.is_none());
            let sm = app.world().get::<SmState>(messenger).unwrap();
            assert!(
                sm.attack_queued.is_none(),
                "old queued combo must not run after the order"
            );
            assert_eq!(
                sm.current,
                meteor_core::FighterState::Attack1,
                "do not truncate the current authored animation"
            );
            app.world_mut().run_system_once(drive_npc_ai).unwrap();
            assert!(!app.world().get::<AiIntent>(messenger).unwrap().want_attack);

            app.world_mut()
                .get_mut::<SmState>(messenger)
                .unwrap()
                .current = meteor_core::FighterState::Name;
            for _ in 0..140 {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_millis(100));
                app.world_mut().run_system_once(drive_npc_ai).unwrap();
                let intent = app.world().get::<AiIntent>(messenger).unwrap();
                assert!(
                    !intent.want_attack && intent.enemy_id.is_none(),
                    "messenger reacquired nearby player"
                );
            }
            assert!(app.world().get::<AiIntent>(messenger).unwrap().move_dir.length()>0.9,"messenger {messenger_id} must depart: intent={:?} npc={:?} sm={:?} performance={:?}", app.world().get::<AiIntent>(messenger),app.world().get::<MissionNpc>(messenger),app.world().get::<SmState>(messenger),app.world().get::<NpcPerformance>(messenger));

            let resource_root = root.parent().unwrap();
            let gmdl = meteor_asset::gmdl::decode_gmdl(
                &std::fs::read(resource_root.join("sn04/sn04.gmb")).unwrap(),
            )
            .unwrap();
            let des = meteor_asset::des::parse_stage_description(
                &std::fs::read_to_string(resource_root.join("sn04/sn04.des")).unwrap(),
            );
            let cob = meteor_asset::cob::parse_cob(
                &std::fs::read(resource_root.join("sn04/sn04.cob")).unwrap(),
            )
            .unwrap();
            let mesh = crate::plugins::stage_collider::StageMeshCollider::from_gmdl(
                &gmdl,
                &des,
                Some(&cob.face_ids),
                meteor_scene::stage_loader::get_stage_center("sn04", None),
            );
            {
                let mut mover = meteor_physics::collision::CollisionSystem::new(
                    crate::plugins::movement_plugin::COLLIDER_RADIUS,
                    crate::plugins::movement_plugin::COLLIDER_HEIGHT,
                );
                mover.set_mesh_collider(&mesh);
                app.world_mut()
                    .get_mut::<Transform>(messenger)
                    .unwrap()
                    .translation
                    .y = mover
                    .sample_ground_hit(position.x, position.z, position.y)
                    .unwrap()
                    .y;
            }
            app.insert_resource(crate::plugins::stage_collider::StageCollider { mesh: Some(mesh) });
            app.world_mut()
                .entity_mut(messenger)
                .insert(crate::components::fighter::EntryBlend::default());
            let target = Vec3::from_array(
                app.world()
                    .resource::<WaypointGraph>()
                    .world_position(1)
                    .unwrap(),
            );
            let mut arrived = false;
            for _ in 0..3000 {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(1. / 60.));
                app.world_mut().run_system_once(drive_npc_ai).unwrap();
                let intent = app.world().get::<AiIntent>(messenger).unwrap();
                assert!(!intent.want_attack && intent.enemy_id.is_none());
                app.world_mut()
                    .run_system_once(crate::plugins::ai_plugin::move_npc_fighters)
                    .unwrap();
                let pos = app.world().get::<Transform>(messenger).unwrap().translation;
                if Vec2::new(pos.x - target.x, pos.z - target.z).length()
                    <= 40. * meteor_core::MODEL_SCALE + 0.05
                {
                    arrived = true;
                    break;
                }
            }
            assert!(
                arrived,
                "messenger {messenger_id} failed to reach authored destination: {:?}",
                app.world().get::<Transform>(messenger)
            );
            for id in [4, 5] {
                let entity = actors.iter().find(|(actor, _)| *actor == id).unwrap().1;
                assert_eq!(
                    app.world()
                        .get::<MissionNpc>(entity)
                        .unwrap()
                        .forced_behavior,
                    Some(meteor_ai::behavior::BehaviorMode::Kill)
                );
            }
        }
    }

    #[test]
    fn pst_dynamic_attributes_and_waypoints_reach_production_ecs() {
        let source = r#"OnStart(){ AddNPC("npc05_03"); Call(1,"Attack",0); DisableWaypoints(1); }"#;
        let mut runtime = MissionScriptRuntime::new(source, Default::default());
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
        let npc_source =
            crate::text_codec::decode_text(&std::fs::read(root.join("npc05_03.pst")).unwrap());
        runtime
            .register_npc_script("npc05_03", &npc_source)
            .unwrap();
        runtime.start().unwrap();
        let mut app = App::new();
        app.insert_resource(StageMissionRuntime(Some(runtime)))
            .insert_resource(LaunchRequest::default())
            .init_resource::<PendingMissionNpcCommands>()
            .init_resource::<PlayerPerformance>()
            .insert_resource(meteor_scene::stage_loader::parse_waypoint_graph(
                "WayPoints 3\nPos 0 0 0\nSize 1\nLink 1\n1 0 10\nPos 10 0 0\nSize 1\nLink 1\n2 0 10\nPos 20 0 0\nSize 1\nLink 0", [0.0;3]))
            .add_systems(Update, (consume_mission_npc_requests, consume_mission_attribute_updates).chain());
        app.update();
        app.update();
        let mut query = app.world_mut().query::<(&MissionNpc, &NpcCombatProfile)>();
        let (npc, profile) = query.single(app.world()).unwrap();
        assert_eq!(npc.id, 1);
        assert_eq!(
            npc.forced_behavior,
            Some(meteor_ai::behavior::BehaviorMode::Follow)
        );
        assert_eq!(
            (profile.attack1, profile.attack2, profile.attack3),
            (0.0, 0.0, 0.0)
        );
        assert!(!app.world().resource::<WaypointGraph>().is_enabled(1));
        assert!(app
            .world()
            .resource::<WaypointGraph>()
            .navigation_links(0)
            .is_empty());
        app.world_mut()
            .resource_mut::<StageMissionRuntime>()
            .script_mut()
            .unwrap()
            .call_npc_script(1, "Attack", &[PstValue::Num(1.0)])
            .unwrap();
        app.update();
        let (_, profile) = query.single(app.world()).unwrap();
        assert_eq!(
            (profile.attack1, profile.attack2, profile.attack3),
            (40.0, 30.0, 10.0)
        );
    }

    #[test]
    fn pst_player_crouch_and_use_respect_pause_order() {
        let mut state = PlayerPerformance::default();
        for (action, value) in [
            ("crouch", 1.0),
            ("pause", 1.0),
            ("use", 15.0),
            ("crouch", 0.0),
        ] {
            state.queue.push_back(NpcPerformCommand {
                action: action.into(),
                args: vec![PstValue::Num(value)],
            });
        }
        player_performance_pulse(&mut state);
        assert_eq!(state.crouch_requested, Some(true));
        for _ in 0..3 {
            player_performance_pulse(&mut state);
            assert!(state.use_items.is_empty());
        }
        player_performance_pulse(&mut state);
        assert_eq!(state.use_items, vec![15]);
        assert_eq!(state.crouch_requested, Some(true));
        player_performance_pulse(&mut state);
        assert_eq!(state.crouch_requested, Some(false));
    }

    #[test]
    fn hazard_script_machine_damage_ignores_collision_and_stops_when_removed() {
        use super::super::hazard_plugin::{sync_hazard_machines, HazardPart, StageHazardRuntime};
        use bevy::ecs::system::RunSystemOnce;
        let instance = SceneItemInstance {
            name: "stone".into(),
            index: 0,
            props: std::collections::HashMap::from([
                ("active".into(), PstValue::Num(1.0)),
                ("damage".into(), PstValue::Num(1.0)),
                ("damagevalue".into(), PstValue::Num(50.0)),
                ("collision".into(), PstValue::Num(0.0)),
            ]),
        };
        let runtime = SceneScriptRuntime::load("", vec![instance], 1000).unwrap();
        let mut app = App::new();
        app.init_resource::<StageHazardRuntime>()
            .insert_resource(StageScriptRuntime(Some(runtime)));
        app.world_mut().spawn((
            super::super::item_plugin::StageProp {
                name: "stone".into(),
                model: "sn01st01".into(),
                center_offset: Vec3::ZERO,
                half_extents: Vec3::ONE,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: false,
                collision_enabled: false,
                breakable: true,
                broken: false,
            },
            Transform::IDENTITY,
            HazardPart(vec![-Vec3::ONE, Vec3::ONE]),
        ));
        app.world_mut()
            .run_system_once(sync_hazard_machines)
            .unwrap();
        let mut dealt = 0;
        app.world_mut()
            .resource_mut::<StageHazardRuntime>()
            .0
            .update(
                0.016,
                &[("p".into(), [0.0; 3], false, false)],
                &mut |_, _, damage, _| dealt += damage,
            );
        assert_eq!(dealt, 50, "broken stone remains damaging with collision=0");
        app.world_mut()
            .resource_mut::<StageScriptRuntime>()
            .script_mut()
            .unwrap()
            .host_mut()
            .instance_mut(0)
            .unwrap()
            .set("active", PstValue::Num(0.0));
        app.world_mut()
            .run_system_once(sync_hazard_machines)
            .unwrap();
        app.world_mut()
            .resource_mut::<StageHazardRuntime>()
            .0
            .update(
                3.0,
                &[("p".into(), [0.0; 3], false, false)],
                &mut |_, _, damage, _| dealt += damage,
            );
        assert_eq!(dealt, 50, "removed machines must stop hurting");
    }

    #[test]
    fn custom_map_uses_its_own_pickup_mesh_home_and_safe_zone() {
        let (mut app, player) = escort_test_app(false);
        {
            let mut request = app.world_mut().resource_mut::<LaunchRequest>();
            request.stage_id = "editor_trial".into();
            request.mode = "escort".into();
        }
        let mut definition = app
            .world()
            .resource::<crate::plugins::escort_assets::LoadedEscort>()
            .0
            .as_ref()
            .unwrap()
            .as_ref()
            .clone();
        definition.stage.flag_home = meteor_network::role_state::RoleVec3 {
            x: 600.0,
            y: 7.0,
            z: 800.0,
        };
        for point in &mut definition.vertices {
            point[0] *= 8.0;
        }
        for zone in &mut definition.stage.safe_zones {
            zone.min_x += 1000.0;
            zone.max_x += 1000.0;
            zone.center_x += 1000.0;
        }
        let home = definition.stage.flag_home;
        let point = definition
            .vertices
            .iter()
            .max_by(|a, b| a[0].total_cmp(&b[0]))
            .unwrap();
        let contact = Vec3::new(
            home.x + point[0],
            home.y + point[1] - 0.96,
            home.z + point[2],
        );
        assert!(!meteor_network::escort::character_capsule_intersects_flag(
            escort_position(contact),
            home
        ));
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = contact;
        let own = definition.stage.safe_zones[0];
        app.world_mut()
            .insert_resource(crate::plugins::escort_assets::LoadedEscort(Some(
                std::sync::Arc::new(definition),
            )));
        app.update();
        assert!(app.world().get::<EscortCarrying>(player).is_some());
        let old = meteor_network::escort::SN03.safe_zones[0];
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = Vec3::new(old.center_x, old.center_y, old.center_z);
        app.update();
        assert!(
            !app.world()
                .resource::<crate::components::fighter::MatchEnd>()
                .settled
        );
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = Vec3::new(own.center_x, own.center_y, own.center_z);
        app.update();
        assert!(
            app.world()
                .resource::<crate::components::fighter::MatchEnd>()
                .settled
        );
    }

    #[test]
    fn campaign_rule2_pickup_drop_return_and_delivery_use_live_objective() {
        let (mut app, player) = escort_test_app(false);
        app.update();
        assert!(
            app.world().get::<EscortCarrying>(player).is_some(),
            "single mode must honor mission Rule=2"
        );
        assert_eq!(
            app.world()
                .resource::<StageMissionRuntime>()
                .script()
                .unwrap()
                .host()
                .char_id("flag"),
            Some(0)
        );
        let cargo = app
            .world_mut()
            .query_filtered::<Entity, With<LocalEscortState>>()
            .single(app.world())
            .unwrap();
        assert_eq!(
            *app.world().get::<Visibility>(cargo).unwrap(),
            Visibility::Hidden
        );

        let dropped = app.world().get::<Transform>(player).unwrap().translation + Vec3::X * 20.0;
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = dropped;
        app.world_mut().get_mut::<Health>(player).unwrap().current = 0.0;
        app.update();
        assert!(app.world().get::<EscortCarrying>(player).is_none());
        assert_eq!(
            app.world()
                .resource::<StageMissionRuntime>()
                .script()
                .unwrap()
                .host()
                .char_id("flag"),
            None
        );
        assert_eq!(
            *app.world().get::<Visibility>(cargo).unwrap(),
            Visibility::Visible
        );
        assert_eq!(
            app.world().get::<Transform>(cargo).unwrap().translation,
            dropped
        );
        app.world_mut()
            .resource_mut::<crate::plugins::FrameClock>()
            .sim_dt = 4.9;
        app.update();
        assert_eq!(
            app.world().get::<Transform>(cargo).unwrap().translation,
            dropped
        );
        app.world_mut()
            .resource_mut::<crate::plugins::FrameClock>()
            .sim_dt = 0.11;
        app.update();
        let home = meteor_network::escort::SN03.flag_home;
        assert_eq!(
            app.world().get::<Transform>(cargo).unwrap().translation,
            Vec3::new(home.x, home.y, home.z)
        );

        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = Vec3::new(home.x, home.y, home.z);
        app.world_mut().get_mut::<Health>(player).unwrap().current = 100.0;
        app.update();
        assert!(app.world().get::<EscortCarrying>(player).is_some());
        let enemy = meteor_network::escort::SN03.safe_zones[1];
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = Vec3::new(enemy.center_x, enemy.center_y, enemy.center_z);
        app.update();
        assert!(
            !app.world()
                .resource::<crate::components::fighter::MatchEnd>()
                .settled
        );
        let own = meteor_network::escort::SN03.safe_zones[0];
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = Vec3::new(own.center_x, own.center_y, own.center_z);
        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::components::fighter::MatchEnd>()
                .result,
            meteor_network::modes::ROUND_RESULT_TEAM2_WIPED
        );
    }

    #[test]
    fn campaign_objective_never_competes_with_network_authority() {
        let (mut app, player) = escort_test_app(true);
        app.update();
        assert!(app.world().get::<EscortCarrying>(player).is_none());
        assert_eq!(
            app.world_mut()
                .query::<&LocalEscortState>()
                .iter(app.world())
                .count(),
            0
        );
    }

    #[test]
    fn campaign_npc_can_retake_dropped_flag_and_removal_returns_it() {
        let (mut app, player) = escort_test_app(false);
        app.update();
        let fighter = app.world().get::<Fighter>(player).unwrap().clone();
        let position = app.world().get::<Transform>(player).unwrap().translation + Vec3::X * 20.0;
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = position;
        app.world_mut().get_mut::<Health>(player).unwrap().current = 0.0;
        let npc = app
            .world_mut()
            .spawn((
                fighter,
                mission_npc_for_test(1, "carrier"),
                Health {
                    current: 100.0,
                    max: 100.0,
                },
                Transform::from_translation(position),
            ))
            .id();
        app.world_mut()
            .resource_mut::<StageMissionRuntime>()
            .script_mut()
            .unwrap()
            .host_mut()
            .sync_fighter(1, 100.0, 100.0, 2.0);
        app.update();
        assert!(
            app.world().get::<EscortCarrying>(npc).is_none(),
            "death first drops the cargo"
        );
        app.update();
        assert!(
            app.world().get::<EscortCarrying>(npc).is_some(),
            "live NPC claims the dropped cargo before timeout"
        );
        assert_eq!(
            app.world()
                .resource::<StageMissionRuntime>()
                .script()
                .unwrap()
                .host()
                .char_id("flag"),
            Some(1)
        );
        app.world_mut().despawn(npc);
        app.update();
        assert_eq!(
            app.world()
                .resource::<StageMissionRuntime>()
                .script()
                .unwrap()
                .host()
                .char_id("flag"),
            None
        );
        let mut cargo = app
            .world_mut()
            .query_filtered::<(&Transform, &Visibility), With<LocalEscortState>>();
        let (transform, visibility) = cargo.single(app.world()).unwrap();
        let home = meteor_network::escort::SN03.flag_home;
        assert_eq!(transform.translation, Vec3::new(home.x, home.y, home.z));
        assert_eq!(*visibility, Visibility::Visible);
    }

    #[test]
    fn network_escort_ground_entity_follows_authoritative_lifecycle() {
        use meteor_network::protocol::EscortFlagSnapshot;
        let mut app = App::new();
        app.insert_resource(LaunchRequest {
            mode: "escort".into(),
            stage_id: "sn03".into(),
            ..default()
        })
        .insert_resource(crate::plugins::network_plugin::NetState {
            status: crate::plugins::network_plugin::NetStatus::InRoom,
            ..default()
        })
        .add_systems(Update, sync_network_escort_flag);
        for (state, position, expected) in [
            (
                "home",
                [26.937681, 0.59184, -155.95084],
                Visibility::Visible,
            ),
            (
                "carried",
                [26.937681, 0.59184, -155.95084],
                Visibility::Hidden,
            ),
            ("dropped", [30.0, 0.0, -150.0], Visibility::Visible),
            (
                "home",
                [26.937681, 0.59184, -155.95084],
                Visibility::Visible,
            ),
        ] {
            app.world_mut()
                .resource_mut::<crate::plugins::network_plugin::NetState>()
                .escort_flag = Some(EscortFlagSnapshot {
                state: state.into(),
                position,
                carrier_id: (state == "carried").then(|| "stock-1".into()),
                return_in: None,
            });
            app.update();
            let mut flags = app
                .world_mut()
                .query_filtered::<(&Transform, &Visibility), With<NetworkEscortFlag>>();
            let (transform, visibility) = flags.single(app.world()).unwrap();
            assert_eq!(transform.translation, Vec3::from_array(position));
            assert_eq!(*visibility, expected);
        }
    }

    fn mission_npc_for_test(id: i64, name: &str) -> MissionNpc {
        MissionNpc {
            id,
            name: name.into(),
            team: 2,
            in_scene_roster: true,
            view: 100.0,
            think: 50.0,
            forced_behavior: None,
            is_vip: false,
            attack_target_slot: None,
            behavior_args: Vec::new(),
            attack_target_remaining: 0,
        }
    }

    #[test]
    fn production_mission_projection_syncs_player_health_and_live_enemy() {
        use meteor_scene::pst_runtime::{PstHost, PstValue};

        let mut app = App::new();
        app.insert_resource(StageMissionRuntime(Some(MissionScriptRuntime::new(
            "",
            Default::default(),
        ))))
        .add_systems(Update, sync_mission_fighters);
        app.world_mut().spawn((
            LocalFighter,
            Health {
                current: 321.0,
                max: 900.0,
            },
        ));
        app.world_mut().spawn((
            mission_npc_for_test(7, "guard"),
            Health {
                current: 500.0,
                max: 1000.0,
            },
            NpcPerformance::default(),
            crate::components::fighter::AiIntent {
                enemy_id: Some(0),
                ..default()
            },
        ));

        app.update();

        let mut runtime = app.world_mut().resource_mut::<StageMissionRuntime>();
        let host = runtime.script_mut().unwrap().host_mut();
        assert_eq!(host.builtin("GetHP", &[PstValue::Num(0.0)]).as_f64(), 321.0);
        assert_eq!(
            host.builtin("GetMaxHP", &[PstValue::Num(0.0)]).as_f64(),
            900.0
        );
        assert_eq!(host.builtin("GetTeam", &[PstValue::Num(0.0)]).as_f64(), 0.0);
        assert_eq!(
            host.builtin("GetEnemy", &[PstValue::Num(7.0)]).as_f64(),
            0.0
        );
    }

    #[test]
    fn mission_tick_reads_current_player_state_on_the_first_update() {
        let source = r#"OnUpdate() { Print(GetHP(0)); }"#;
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(crate::plugins::FrameClock {
                sim_dt: 0.1,
                ..default()
            })
            .insert_resource(LaunchRequest::default())
            .insert_resource(StageMissionRuntime(Some(MissionScriptRuntime::new(
                source,
                Default::default(),
            ))))
            .add_systems(Update, (sync_mission_fighters, tick_mission_script).chain());
        app.world_mut().spawn((
            LocalFighter,
            Health {
                current: 432.0,
                max: 900.0,
            },
        ));

        app.update();

        let runtime = app.world().resource::<StageMissionRuntime>();
        assert_eq!(runtime.script().unwrap().host().output(), &["432"]);
    }

    #[test]
    fn production_mission_spatial_projection_drives_source_unit_distance() {
        use meteor_scene::pst_runtime::{PstHost, PstValue};

        let mut runtime = MissionScriptRuntime::new("", Default::default());
        runtime.host_mut().register_char("guard", 7);
        runtime.host_mut().builtin(
            "SetTarget",
            &[
                PstValue::Num(0.0),
                PstValue::Str("char".into()),
                PstValue::Num(0.0),
            ],
        );
        runtime.host_mut().builtin(
            "SetTarget",
            &[
                PstValue::Num(1.0),
                PstValue::Str("char".into()),
                PstValue::Num(7.0),
            ],
        );
        let mut app = App::new();
        app.insert_resource(StageMissionRuntime(Some(runtime)))
            .add_systems(Update, sync_mission_spatial);
        app.world_mut()
            .spawn((LocalFighter, Transform::from_xyz(0.0, 0.9, 0.0)));
        app.world_mut().spawn((
            mission_npc_for_test(7, "guard"),
            Transform::from_xyz(1.2, 0.9, 0.0),
        ));

        app.update();

        let mut runtime = app.world_mut().resource_mut::<StageMissionRuntime>();
        let distance = runtime
            .script_mut()
            .unwrap()
            .host_mut()
            .builtin("Distance", &[PstValue::Num(0.0), PstValue::Num(1.0)]);
        assert!((distance.as_f64() - 10.0).abs() < 1e-5);
    }

    #[test]
    fn direct_say_request_reaches_dialogue_state() {
        let source = r#"OnStart() { Say(7, "最后一句"); }"#;
        let mut runtime = MissionScriptRuntime::new(source, Default::default());
        runtime.host_mut().register_char("队长", 7);
        runtime.host_mut().sync_fighter(7, 0.0, 1000.0, 2.0);
        runtime.start().expect("Say queues");

        let mut app = App::new();
        app.insert_resource(StageMissionRuntime(Some(runtime)))
            .insert_resource(crate::plugins::ui_plugin::DialogueState::default())
            .add_systems(Update, consume_mission_say_requests);
        app.update();

        let dialogue = app
            .world()
            .resource::<crate::plugins::ui_plugin::DialogueState>();
        assert_eq!(dialogue.text.as_deref(), Some("队长：最后一句"));
        assert_eq!(dialogue.timer, 3.0);
    }

    #[test]
    fn add_npc_reactivation_replaces_entity_without_changing_script_id() {
        use meteor_scene::pst_runtime::{PstHost, PstValue};

        let mut runtime = MissionScriptRuntime::new("", Default::default());
        assert_eq!(
            runtime
                .host_mut()
                .builtin("AddNPC", &[PstValue::Str("npc01_01".into())]),
            PstValue::Num(1.0)
        );
        assert_eq!(
            runtime.host_mut().take_npc_requests(),
            vec![NpcRequest::Add {
                script: "npc01_01".into(),
                id: 1,
            }]
        );
        runtime
            .host_mut()
            .builtin("AddNPC", &[PstValue::Str("npc01_01".into())]);
        let root = std::env::current_dir().expect("test cwd");
        assert!(resource_roots(&root)
            .iter()
            .any(|root| root.join("Level/npc01_01.pst").is_file()));

        let mut app = App::new();
        app.insert_resource(StageMissionRuntime(Some(runtime)))
            .insert_resource(LaunchRequest::default())
            .insert_resource(PendingMissionNpcCommands::default())
            .insert_resource(PlayerPerformance::default())
            .add_systems(Update, consume_mission_npc_requests);
        let old = app
            .world_mut()
            .spawn((
                mission_npc_for_test(1, "stale"),
                crate::components::fighter::AiIntent::default(),
                NpcPerformance::default(),
                crate::plugins::ai_plugin::NpcPatrol::default(),
            ))
            .id();

        app.update();

        assert!(app.world().get_entity(old).is_err());
        let mut query = app.world_mut().query::<(Entity, &MissionNpc, &Health)>();
        let actors = query
            .iter(app.world())
            .filter(|(_, npc, _)| npc.id == 1)
            .map(|(entity, npc, health)| (entity, npc.name.clone(), health.current))
            .collect::<Vec<_>>();
        assert_eq!(actors.len(), 1);
        assert_ne!(actors[0].0, old);
        assert_eq!(actors[0].1, "铁枪哨兵﹒甲");
        assert_eq!(actors[0].2, 1000.0);
    }

    #[test]
    fn spawned_mission_npc_consumes_same_batch_perform_on_the_next_frame() {
        use meteor_scene::pst_runtime::{PstHost, PstValue};

        let mut runtime = MissionScriptRuntime::new("", Default::default());
        let host = runtime.host_mut();
        assert_eq!(
            host.builtin("AddNPC", &[PstValue::Str("npc01_01".into())]),
            PstValue::Num(1.0)
        );
        host.builtin(
            "Perform",
            &[
                PstValue::Num(1.0),
                PstValue::Str("say".into()),
                PstValue::Str("列队".into()),
            ],
        );
        let mut app = App::new();
        app.insert_resource(StageMissionRuntime(Some(runtime)))
            .insert_resource(LaunchRequest::default())
            .insert_resource(PendingMissionNpcCommands::default())
            .insert_resource(PlayerPerformance::default())
            .add_systems(Update, consume_mission_npc_requests);

        app.update();
        app.update();

        let mut query = app.world_mut().query::<(&MissionNpc, &NpcPerformance)>();
        let (npc, performance) = query.single(app.world()).expect("mission NPC queryable");
        assert_eq!(npc.id, 1);
        assert_eq!(performance.queue.len(), 1);
        assert_eq!(performance.queue[0].action, "say");
        assert_eq!(
            performance.queue[0].args,
            vec![PstValue::Str("列队".into())]
        );
    }

    #[test]
    fn official_unaffiliated_npc_keeps_team_zero_in_ecs_and_mission_host() {
        use meteor_scene::pst_runtime::{PstHost, PstValue};

        let mut runtime = MissionScriptRuntime::new("", Default::default());
        runtime
            .host_mut()
            .builtin("AddNPC", &[PstValue::Str("npc03_01".into())]);
        let mut app = App::new();
        app.insert_resource(StageMissionRuntime(Some(runtime)))
            .insert_resource(LaunchRequest::default())
            .insert_resource(PendingMissionNpcCommands::default())
            .insert_resource(PlayerPerformance::default())
            .add_systems(Update, consume_mission_npc_requests);

        app.update();

        let mut query = app.world_mut().query::<(&MissionNpc, &NpcCombatProfile)>();
        let (npc, profile) = query.single(app.world()).expect("npc03_01 spawned");
        assert_eq!(npc.name, "土匪﹒铁胡子");
        assert_eq!(npc.team, 0);
        assert_eq!(
            (
                profile.attack1,
                profile.attack2,
                profile.attack3,
                profile.guard,
                profile.dodge,
                profile.jump,
                profile.look,
                profile.burst,
                profile.aim,
                profile.get_item,
            ),
            (20.0, 25.0, 30.0, 25.0, 60.0, 20.0, 50.0, 20.0, 80.0, 100.0)
        );
        let mut mission = app.world_mut().resource_mut::<StageMissionRuntime>();
        let team = mission
            .script_mut()
            .unwrap()
            .host_mut()
            .builtin("GetTeam", &[PstValue::Num(1.0)]);
        assert_eq!(team, PstValue::Num(0.0));
    }

    #[test]
    fn breakable_becomes_terminal_only_after_script_finishes_and_disables_it() {
        assert!(!script_prop_is_terminally_broken(true, true, 1, 1));
        assert!(!script_prop_is_terminally_broken(true, true, 1, 3));
        assert!(!script_prop_is_terminally_broken(true, false, 1, 1));
        assert!(script_prop_is_terminally_broken(true, false, 1, 3));
        assert!(!script_prop_is_terminally_broken(false, false, 1, 3));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn stock_sn01_boxes_use_original_script_refs_and_apply_weapon_pickups() {
        use crate::plugins::item_plugin::{
            apply_stock_scene_item_pickups, initialize_stock_scene_items, ItemBuffs, StageProp,
            StockSceneManifestState,
        };
        use crate::plugins::network_plugin::{
            stock_scene_manifest_test_channel, stock_scene_ops_from_message, NetState, NetStatus,
            StockSceneItemGrounded, StockSceneItemPickup,
        };
        use bevy::ecs::system::RunSystemOnce;
        use meteor_network::stock_protocol::{STOCK_SCENE_OP_ACTIVE, STOCK_SCENE_OP_INTERACTIVE};
        use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance};
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut sources = Vec::new();
        for include in ["Def.h", "box.pst", "desk.pst", "chair.pst", "jug.pst"] {
            sources.push(crate::text_codec::decode_text(
                &std::fs::read(root.join("PScriptLib").join(include)).unwrap(),
            ));
        }
        sources.push(crate::text_codec::decode_text(
            &std::fs::read(root.join("sn01/sn01.pst")).unwrap(),
        ));
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn01/sn01.des")).unwrap(),
        );
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, dummy)| SceneItemInstance {
                name: dummy.name.clone(),
                index,
                props: std::collections::HashMap::from([
                    ("active".into(), PstValue::Num(f64::from(dummy.visible))),
                    ("interactive".into(), PstValue::Num(1.0)),
                    ("state".into(), PstValue::Num(3.0)),
                ]),
            })
            .collect();
        let mut runtime =
            SceneScriptRuntime::load(&sources.join("\n"), instances, 250_000).unwrap();
        runtime.start().unwrap();
        let (tx, drain) = stock_scene_manifest_test_channel();
        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            address: "udp-host://127.0.0.1:5230".into(),
            player_id: "stock-1".into(),
            ..default()
        })
        .insert_resource(tx)
        .init_resource::<StockSceneManifestState>()
        .init_resource::<ItemBuffs>();
        for dummy in &desc.dummies {
            let Some(model) = &dummy.model else { continue };
            let instance = runtime
                .host()
                .instance(runtime.host().resolve_name(&dummy.name).unwrap())
                .unwrap();
            let active = instance.get("active").as_f64() != 0.0;
            app.world_mut().spawn((
                StageProp {
                    name: dummy.name.clone(),
                    model: model.to_ascii_lowercase(),
                    center_offset: Vec3::ZERO,
                    half_extents: Vec3::splat(0.5),
                    hit_count: 0,
                    respawn_timer: 0.0,
                    interactive: instance.get("interactive").as_f64() != 0.0,
                    collision_enabled: false,
                    breakable: false,
                    broken: false,
                },
                Transform::from_xyz(
                    dummy.position.x * 0.12,
                    dummy.position.z * 0.12,
                    -dummy.position.y * 0.12,
                ),
                if active {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                },
            ));
        }
        app.insert_resource(StageScriptRuntime(Some(runtime)));
        app.world_mut()
            .run_system_once(initialize_stock_scene_items)
            .unwrap();
        let manifests = drain();
        assert_eq!(manifests.len(), 1);
        let player = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                crate::components::fighter::Qi::default(),
                Transform::default(),
            ))
            .id();
        for (box_name, weapon_name) in [("D_BBox01", "D_wpBBox01"), ("D_BBox02", "D_wpBBox02")] {
            let mut script = app.world_mut().resource_mut::<StageScriptRuntime>();
            let runtime = script.script_mut().unwrap();
            let reference = runtime.host().resolve_name(weapon_name).unwrap() as u32;
            let seed = manifests[0]
                .iter()
                .find(|seed| seed.scene_reference == Some(reference))
                .expect("hidden real box weapon must be registered, including W8_0r alias");
            assert!(!seed.active && !seed.interactive);
            runtime
                .call_item(
                    box_name,
                    "OnAttack",
                    &[PstValue::Num(0.0), PstValue::Num(10_000.0)],
                )
                .unwrap();
            let ops = runtime
                .host_mut()
                .take_net_batches()
                .into_iter()
                .flatten()
                .map(|op| scene_op_from_runtime(&op))
                .collect::<Vec<_>>();
            let wire = stock_scene_ops_from_message(&ops, 100);
            for kind in [STOCK_SCENE_OP_ACTIVE, STOCK_SCENE_OP_INTERACTIVE] {
                assert!(
                    wire.operations
                        .iter()
                        .any(|op| op.reference == reference && op.kind == kind && op.value == 1),
                    "original box release must address the registered DES index"
                );
            }
            let position = [seed.position.x, seed.position.y, seed.position.z];
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_stock_scene_grounded
                .push(StockSceneItemGrounded {
                    record_id: reference,
                    category: seed.category,
                    item_id: seed.item_id,
                    position,
                    spawned: true,
                });
            app.world_mut()
                .run_system_once(apply_stock_scene_item_pickups)
                .unwrap();
            assert!(app.world().resource::<NetState>().dynamic_weapon_drops.is_empty(),
                "paired weapon records must reveal their DES prop without creating a duplicate dynamic actor");
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_stock_scene_items
                .push(StockSceneItemPickup {
                    actor_id: 1,
                    record_id: reference,
                    category: seed.category,
                    item_id: seed.item_id,
                    position: Some(position),
                });
            app.world_mut()
                .get_mut::<Fighter>(player)
                .unwrap()
                .weapon_slots = [6, 0];
            app.world_mut()
                .run_system_once(apply_stock_scene_item_pickups)
                .unwrap();
            let expected =
                meteor_network::stock_protocol::stock_weapon_from_item_id(seed.item_id).unwrap();
            let fighter = app.world().get::<Fighter>(player).unwrap();
            assert_eq!(
                (fighter.weapon_slots[1], fighter.weapon_slot_variants[1]),
                expected
            );
            let (_, visibility) = app
                .world_mut()
                .query::<(&StageProp, &Visibility)>()
                .iter(app.world())
                .find(|(prop, _)| prop.name == weapon_name)
                .unwrap();
            assert_eq!(*visibility, Visibility::Hidden);
            assert_eq!(
                app.world()
                    .resource::<StageScriptRuntime>()
                    .script()
                    .unwrap()
                    .host()
                    .instance(reference as usize)
                    .unwrap()
                    .get("active")
                    .as_f64(),
                0.0
            );
        }
    }

    #[test]
    fn real_sn01_stone_pillar_break_clip_finishes_hidden_in_ecs() {
        use meteor_scene::pst_runtime::PstValue;
        use std::collections::HashMap;

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn01/sn01.pst").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mut sources = Vec::new();
        for include in ["Def.h", "box.pst", "desk.pst", "chair.pst", "jug.pst"] {
            sources.push(crate::text_codec::decode_text(
                &std::fs::read(root.join("PScriptLib").join(include)).expect(include),
            ));
        }
        sources.push(crate::text_codec::decode_text(
            &std::fs::read(root.join("sn01/sn01.pst")).expect("sn01.pst"),
        ));
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn01/sn01.des")).expect("sn01.des"),
        );
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(
                |(index, dummy)| meteor_scene::scene_script_runtime::SceneItemInstance {
                    name: dummy.name.clone(),
                    index,
                    props: HashMap::from([
                        ("active".into(), PstValue::Num(dummy.visible as i32 as f64)),
                        ("collision".into(), PstValue::Num(1.0)),
                        ("interactive".into(), PstValue::Num(1.0)),
                        ("pose".into(), PstValue::Num(0.0)),
                        ("pose_loop".into(), PstValue::Num(0.0)),
                        ("frame".into(), PstValue::Num(-1.0)),
                        ("state".into(), PstValue::Num(3.0)),
                    ]),
                },
            )
            .collect();
        let mut runtime = meteor_scene::scene_script_runtime::SceneScriptRuntime::load(
            &sources.join("\n"),
            instances,
            250_000,
        )
        .expect("real sn01 scene script parses");
        runtime.start().expect("real sn01 scene script starts");

        let fmc = meteor_asset::fmc::parse_fmc(
            &std::fs::read_to_string(root.join("cmodel/sn01st02.fmc")).expect("sn01st02.fmc"),
        );
        let poses = meteor_asset::fmc::parse_prop_poses(
            &std::fs::read_to_string(root.join("cmodel/sn01st02.pos")).expect("sn01st02.pos"),
        );
        let idle = poses[0];
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .insert_resource(StageScriptRuntime(Some(runtime)))
            .insert_resource(crate::plugins::network_plugin::NetState::default())
            .insert_resource(crate::plugins::stage_collider::StageCollider::default())
            .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
            .add_systems(
                Update,
                (
                    crate::plugins::render_plugin::update_fmc_clips,
                    crate::plugins::render_plugin::sync_fmc_with_script,
                    tick_stage_script,
                )
                    .chain(),
            );
        let entities = (0..fmc.scene_object_count as usize)
            .map(|part_index| {
                app.world_mut()
                    .spawn((
                        crate::plugins::item_plugin::StageProp {
                            name: "D_St01".into(),
                            model: "sn01st02".into(),
                            center_offset: Vec3::ZERO,
                            half_extents: Vec3::ONE,
                            hit_count: 0,
                            respawn_timer: 0.0,
                            interactive: true,
                            collision_enabled: true,
                            breakable: false,
                            broken: false,
                        },
                        crate::plugins::render_plugin::FmcClip {
                            name: "D_St01".into(),
                            clip: fmc.clone(),
                            part_index,
                            poses: poses.clone(),
                            pose_index: 0,
                            pose_start: idle.start,
                            pose_end: idle.end,
                            pose_loop: 0,
                            pose_revision: 0,
                            anim_time: 0.0,
                            state: 3,
                            frame_set: None,
                            multi: true,
                            base_translation: Vec3::ZERO,
                            base_rotation: Quat::IDENTITY,
                            collider_owner: None,
                            collider_vertex_offset: 0,
                            local_verts: Vec::new(),
                            last_reported_state: 1,
                        },
                        Transform::IDENTITY,
                        GlobalTransform::IDENTITY,
                        Visibility::Visible,
                    ))
                    .id()
            })
            .collect::<Vec<_>>();

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
        app.update();
        app.world_mut()
            .resource_mut::<StageScriptRuntime>()
            .script_mut()
            .expect("runtime")
            .call_item(
                "D_St01",
                "OnAttack",
                &[PstValue::Num(0.0), PstValue::Num(500.0)],
            )
            .expect("stone attack");

        for _ in 0..75 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
            app.update();
        }

        let active = app
            .world()
            .resource::<StageScriptRuntime>()
            .script()
            .expect("runtime")
            .host()
            .instances()
            .iter()
            .find(|instance| instance.name.eq_ignore_ascii_case("D_St01"))
            .expect("D_St01 runtime")
            .get("active");
        assert_eq!(active, PstValue::Num(0.0));
        for entity in entities {
            let prop = app
                .world()
                .entity(entity)
                .get::<crate::plugins::item_plugin::StageProp>()
                .expect("stone prop part");
            assert!(
                !prop.broken,
                "script-driven stone pillars are hidden by active=0, not the box model marker"
            );
            assert_eq!(
                app.world().entity(entity).get::<Visibility>(),
                Some(&Visibility::Hidden),
                "every finished stone pillar material part is hidden"
            );
        }
    }

    fn mission_runtime_with_loadout(primary: i64, secondary: i64, hp: i64) -> MissionScriptRuntime {
        let source = format!(
            "int PlayerWeapon={primary}; int PlayerWeapon2={secondary}; int PlayerHP={hp}; OnStart(){{}}"
        );
        let definition = meteor_scene::mission_runtime::parse_mission_definition(&source);
        MissionScriptRuntime::new(&source, definition.values)
    }

    #[test]
    fn level_mission_script_only_loads_for_solo_missions() {
        assert!(should_load_mission_script(&LaunchRequest::default()));
        assert!(!should_load_mission_script(&LaunchRequest {
            spawn_room_npcs: true,
            ..default()
        }));
        for mode in [
            "leader",
            "escort",
            "siege",
            "assassin",
            "deathmatch",
            "network",
            "replay",
        ] {
            assert!(
                !should_load_mission_script(&LaunchRequest {
                    mode: mode.into(),
                    stage_id: "sn01".into(),
                    ..default()
                }),
                "{mode} must not execute sn01's AddNPC mission lifecycle"
            );
        }
    }

    #[test]
    fn origin_sn22_uses_des_user_and_team_spawn_pads_instead_of_world_origin() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let path = root.join("sn22/sn22.des");
        if !path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(path).expect("sn22.des"),
        );
        let center = meteor_scene::stage_loader::ARENA_CENTER;
        let users = stage_spawn_pads(&desc, "user", center);
        let team_a = stage_spawn_pads(&desc, "teama", center);
        let team_b = stage_spawn_pads(&desc, "teamb", center);
        assert_eq!(users.len(), 16);
        assert_eq!(team_a.len(), 8);
        assert_eq!(team_b.len(), 8);
        assert!((team_a[0].1 - -0.995_013).abs() < 1e-5);
        assert!((team_b[0].1 - 1.126_34).abs() < 1e-5);

        let resolved = resolve_player_spawn(None, None, Some(&desc), center, true, "user", Some(0));
        let expected = Vec3::new(
            (531.942 - center[0]) * meteor_core::constants::MODEL_SCALE,
            12.471 * meteor_core::constants::MODEL_SCALE,
            -(1587.252 - center[1]) * meteor_core::constants::MODEL_SCALE,
        );
        assert!((resolved.0 - expected).length() < 1e-3);
        assert!(resolved.0.distance(Vec3::new(0.0, 0.9, 0.0)) > 10.0);
        assert_eq!(
            resolve_player_spawn(None, None, Some(&desc), center, false, "teama", Some(0)).0,
            team_a[0].0
        );
    }

    #[test]
    fn origin_sn22_transfer_destination_snaps_from_helper_box_to_arena_floor() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn22/sn22.gmb").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let center = meteor_scene::stage_loader::ARENA_CENTER;
        let desc_text = std::fs::read_to_string(root.join("sn22/sn22.des")).expect("sn22.des");
        let desc = meteor_asset::des::parse_stage_description(&desc_text);
        let gmdl = meteor_asset::gmdl::decode_gmdl(
            &std::fs::read(root.join("sn22/sn22.gmb")).expect("sn22.gmb"),
        )
        .expect("decode sn22");
        let cob = meteor_asset::cob::parse_cob(
            &std::fs::read(root.join("sn22/sn22.cob")).expect("sn22.cob"),
        )
        .expect("decode sn22 cob");
        let collider = crate::plugins::stage_collider::StageCollider {
            mesh: Some(
                crate::plugins::stage_collider::StageMeshCollider::from_gmdl(
                    &gmdl,
                    &desc,
                    Some(&cob.face_ids),
                    center,
                ),
            ),
        };
        let destination = desc
            .dummies
            .iter()
            .find(|dummy| dummy.name == "D_tpED01")
            .expect("D_tpED01");
        let raw = Vec3::new(
            (destination.position.x - center[0]) * meteor_core::constants::MODEL_SCALE,
            (destination.position.z - center[2]) * meteor_core::constants::MODEL_SCALE,
            -(destination.position.y - center[1]) * meteor_core::constants::MODEL_SCALE,
        );
        let snapped = snap_scene_transfer_position(&collider, raw);
        assert!(snapped.is_finite());
        assert!(
            snapped.y < raw.y - 0.5,
            "destination helper lies above the arena floor: raw={raw:?} snapped={snapped:?}"
        );
        assert!(
            (snapped.y - -3.483_756).abs() < 1e-4,
            "Rust transfer floor must match Web's original-resource sample: {snapped:?}"
        );
    }

    #[test]
    fn origin_sn22_render_empty_portal_triggers_ecs_transfer() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("sn22/sn22.pst").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let center = meteor_scene::stage_loader::ARENA_CENTER;
        let desc_text = std::fs::read_to_string(root.join("sn22/sn22.des")).expect("sn22.des");
        let desc = meteor_asset::des::parse_stage_description(&desc_text);
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, dummy)| SceneItemInstance {
                name: dummy.name.clone(),
                index,
                props: [
                    ("active".into(), PstValue::Num(dummy.visible as i32 as f64)),
                    ("collision".into(), PstValue::Num(0.0)),
                    ("interactive".into(), PstValue::Num(1.0)),
                    ("pose".into(), PstValue::Num(0.0)),
                    ("pose_loop".into(), PstValue::Num(0.0)),
                    ("frame".into(), PstValue::Num(-1.0)),
                    ("state".into(), PstValue::Num(3.0)),
                ]
                .into_iter()
                .collect(),
            })
            .collect();
        let mut script_parts = ["Def.h", "box.pst", "desk.pst", "chair.pst", "jug.pst"]
            .into_iter()
            .map(|name| {
                std::fs::read_to_string(root.join("PScriptLib").join(name))
                    .unwrap_or_else(|_| String::new())
            })
            .collect::<Vec<_>>();
        script_parts.push(std::fs::read_to_string(root.join("sn22/sn22.pst")).expect("sn22.pst"));
        let mut runtime = SceneScriptRuntime::load(&script_parts.join("\n"), instances, 250_000)
            .expect("sn22 runtime");
        runtime.start().expect("sn22 init");

        let gmdl = meteor_asset::gmdl::decode_gmdl(
            &std::fs::read(root.join("sn22/sn22.gmb")).expect("sn22.gmb"),
        )
        .expect("decode sn22");
        let cob = meteor_asset::cob::parse_cob(
            &std::fs::read(root.join("sn22/sn22.cob")).expect("sn22.cob"),
        )
        .expect("decode sn22 cob");
        let collider = crate::plugins::stage_collider::StageCollider {
            mesh: Some(
                crate::plugins::stage_collider::StageMeshCollider::from_gmdl(
                    &gmdl,
                    &desc,
                    Some(&cob.face_ids),
                    center,
                ),
            ),
        };
        let tp = meteor_asset::gmdl::decode_gmdl(
            &std::fs::read(root.join("cmodel/tp001.gmb")).expect("tp001.gmb"),
        )
        .expect("decode tp001");
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for object in &tp.objects {
            for vertex in object.positions.chunks_exact(3) {
                let vertex = Vec3::from_slice(vertex);
                min = min.min(vertex);
                max = max.max(vertex);
            }
        }
        let local_center = (min + max) * (0.5 * meteor_core::constants::MODEL_SCALE);
        let half_extents = (max - min) * (0.5 * meteor_core::constants::MODEL_SCALE);
        let prop = |name: &str| {
            let dummy = desc
                .dummies
                .iter()
                .find(|dummy| dummy.name == name)
                .unwrap();
            let q = dummy.quaternion;
            let transform = Transform {
                translation: Vec3::new(
                    (dummy.position.x - center[0]) * meteor_core::constants::MODEL_SCALE,
                    (dummy.position.z - center[2]) * meteor_core::constants::MODEL_SCALE,
                    -(dummy.position.y - center[1]) * meteor_core::constants::MODEL_SCALE,
                ),
                rotation: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)
                    * Quat::from_xyzw(q.x, q.y, q.z, q.w),
                scale: Vec3::splat(meteor_core::constants::MODEL_SCALE),
            };
            (
                crate::plugins::item_plugin::StageProp {
                    name: name.into(),
                    model: "tp001".into(),
                    center_offset: local_center,
                    half_extents,
                    hit_count: 0,
                    respawn_timer: 0.0,
                    interactive: true,
                    collision_enabled: false,
                    breakable: false,
                    broken: false,
                },
                transform,
                GlobalTransform::from(transform),
                Visibility::Visible,
            )
        };
        let ec = prop("D_tpEC01");
        let ec_center = ec.2.translation() + ec.2.to_scale_rotation_translation().1 * local_center;
        let ed = prop("D_tpED01");
        let expected = snap_scene_transfer_position(&collider, ed.1.translation);

        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(crate::plugins::FrameClock {
                sim_dt: 0.13,
                ..default()
            })
            .insert_resource(SceneTouchState::default())
            .insert_resource(StageScriptRuntime(Some(runtime)))
            .insert_resource(crate::plugins::network_plugin::NetState::default())
            .insert_resource(collider)
            .add_systems(Update, scene_script_touch_and_transfer);
        app.world_mut().spawn(ec);
        app.world_mut().spawn(ed);
        let fighter = app
            .world_mut()
            .spawn((LocalFighter, Transform::from_translation(ec_center)))
            .id();
        app.update();
        let actual = app
            .world()
            .entity(fighter)
            .get::<Transform>()
            .unwrap()
            .translation;
        assert!(
            actual.distance(expected) < 1e-4,
            "EC01 must transfer the local fighter to snapped ED01: {actual:?} vs {expected:?}"
        );
    }

    #[test]
    fn origin_sn02_looping_axes_consume_frame_seek_once_and_keep_moving() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.join("cmodel/knife01.fmc").exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let fmc = meteor_asset::fmc::parse_fmc(
            &std::fs::read_to_string(root.join("cmodel/knife01.fmc")).expect("knife01.fmc"),
        );
        let poses = meteor_asset::fmc::parse_prop_poses(
            &std::fs::read_to_string(root.join("cmodel/knife01.pos")).expect("knife01.pos"),
        );
        let instance = SceneItemInstance {
            name: "D_knife01".into(),
            index: 0,
            props: [
                ("pose".into(), PstValue::Num(1.0)),
                ("pose_loop".into(), PstValue::Num(1.0)),
                ("frame".into(), PstValue::Num(30.0)),
                ("state".into(), PstValue::Num(3.0)),
            ]
            .into_iter()
            .collect(),
        };
        let runtime = SceneScriptRuntime::load("Scene_OnLoad() {}", vec![instance], 10_000)
            .expect("scene runtime");
        let clip = || crate::plugins::render_plugin::FmcClip {
            name: "D_knife01".into(),
            clip: fmc.clone(),
            part_index: 0,
            poses: poses.clone(),
            pose_index: 0,
            pose_start: 0,
            pose_end: 0,
            pose_loop: 0,
            pose_revision: 0,
            anim_time: 0.0,
            state: 3,
            frame_set: None,
            multi: true,
            base_translation: Vec3::ZERO,
            base_rotation: Quat::IDENTITY,
            collider_owner: None,
            collider_vertex_offset: 0,
            local_verts: Vec::new(),
            last_reported_state: 3,
        };
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(crate::plugins::FrameClock {
                sim_dt: 0.1,
                ..default()
            })
            .insert_resource(crate::plugins::stage_collider::StageCollider::default())
            .insert_resource(StageScriptRuntime(Some(runtime)))
            .add_systems(
                Update,
                (
                    crate::plugins::render_plugin::update_fmc_clips,
                    crate::plugins::render_plugin::sync_fmc_with_script,
                )
                    .chain(),
            );
        app.world_mut().spawn((Transform::default(), clip()));
        app.world_mut().spawn((Transform::default(), clip()));

        app.update();
        assert_eq!(
            app.world()
                .resource::<StageScriptRuntime>()
                .script()
                .unwrap()
                .host()
                .instance(0)
                .unwrap()
                .get("frame"),
            PstValue::Num(-1.0),
            "the authored phase seek is consumed after every clip receives it"
        );
        app.update();
        app.update();
        let times = app
            .world_mut()
            .query::<&crate::plugins::render_plugin::FmcClip>()
            .iter(app.world())
            .map(|clip| clip.anim_time)
            .collect::<Vec<_>>();
        assert_eq!(times.len(), 2);
        assert!(
            times.iter().all(|time| *time > 1.0),
            "frame 30 must seed once, then both axe parts advance: {times:?}"
        );
    }

    #[test]
    fn official_single_mission_replaces_both_weapon_slots_and_hp() {
        let runtime = mission_runtime_with_loadout(24, 21, 1350);
        let request = LaunchRequest {
            mode: "single".into(),
            stage_id: "sn05".into(),
            weapon: 8,
            ..default()
        };
        assert_eq!(
            resolve_player_spawn_loadout(&request, Some(&runtime)),
            PlayerSpawnLoadout {
                weapon: 6,
                weapon_slots: [6, 3],
                weapon_slot_variants: [2, 2],
                hp: 1350.0,
            }
        );
    }

    #[test]
    fn original_sn01_and_sn05_player_loadouts_match_web_item_mapping() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for (stage, expected) in [
            (
                "sn01",
                PlayerSpawnLoadout {
                    weapon: 5,
                    weapon_slots: [5, 1],
                    weapon_slot_variants: [0, 0],
                    hp: 1500.0,
                },
            ),
            (
                "sn05",
                PlayerSpawnLoadout {
                    weapon: 6,
                    weapon_slots: [6, 3],
                    weapon_slot_variants: [2, 2],
                    hp: 2500.0,
                },
            ),
        ] {
            let bytes = std::fs::read(root.join("Level").join(format!("{stage}_.pst")))
                .expect("original mission PST");
            let source = crate::text_codec::decode_text(&bytes);
            let definition = meteor_scene::mission_runtime::parse_mission_definition(&source);
            let runtime = MissionScriptRuntime::new(&source, definition.values);
            let request = LaunchRequest {
                mode: "single".into(),
                stage_id: stage.into(),
                weapon: 8,
                ..default()
            };
            assert_eq!(
                resolve_player_spawn_loadout(&request, Some(&runtime)),
                expected
            );
        }
    }

    #[test]
    fn mission_loadout_never_overrides_network_custom_or_failed_mission_sources() {
        let runtime = mission_runtime_with_loadout(5, 0, 1350);
        let fallback = PlayerSpawnLoadout {
            weapon: 8,
            weapon_slots: [8, 0],
            weapon_slot_variants: [0, 0],
            hp: meteor_core::DEFAULT_HP,
        };
        for request in [
            LaunchRequest {
                mode: "single".into(),
                stage_id: "sn01".into(),
                spawn_room_npcs: true,
                weapon: 8,
                ..default()
            },
            LaunchRequest {
                mode: "network".into(),
                stage_id: "sn01".into(),
                weapon: 8,
                ..default()
            },
            LaunchRequest {
                mode: "single".into(),
                stage_id: "custom01".into(),
                weapon: 8,
                ..default()
            },
        ] {
            assert_eq!(
                resolve_player_spawn_loadout(&request, Some(&runtime)),
                fallback
            );
        }
        let official = LaunchRequest {
            mode: "single".into(),
            stage_id: "sn01".into(),
            weapon: 8,
            ..default()
        };
        assert_eq!(resolve_player_spawn_loadout(&official, None), fallback);
    }

    #[test]
    fn player_perform_keeps_block_say_pause_order_and_releases_input() {
        let mut performance = PlayerPerformance::default();
        for (action, value) in [
            ("block", meteor_scene::pst_runtime::PstValue::Num(0.0)),
            (
                "say",
                meteor_scene::pst_runtime::PstValue::Str("开场".into()),
            ),
            ("pause", meteor_scene::pst_runtime::PstValue::Num(2.0)),
            ("block", meteor_scene::pst_runtime::PstValue::Num(1.0)),
        ] {
            performance.queue.push_back(NpcPerformCommand {
                action: action.into(),
                args: vec![value],
            });
        }

        assert_eq!(player_performance_pulse(&mut performance), None);
        assert!(performance.input_blocked);
        assert_eq!(
            player_performance_pulse(&mut performance),
            Some("开场".into())
        );
        for _ in 0..5 {
            assert_eq!(player_performance_pulse(&mut performance), None);
            assert!(performance.input_blocked);
        }
        assert_eq!(performance.queue.len(), 1);
        assert_eq!(player_performance_pulse(&mut performance), None);
        assert!(!performance.input_blocked);
        assert!(performance.queue.is_empty());
    }

    fn player_performance_pulse(performance: &mut PlayerPerformance) -> Option<String> {
        for _ in 0..3 {
            assert_eq!(advance_player_performance(performance, 0.1), None);
        }
        advance_player_performance(performance, 0.1)
    }

    #[test]
    fn sn13_original_player_perform_releases_on_43rd_pulse_at_each_frame_rate() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
        let read =
            |name: &str| crate::text_codec::decode_text(&std::fs::read(root.join(name)).unwrap());
        let source = read("sn13_.pst");
        let definition = meteor_scene::mission_runtime::parse_mission_definition(&source);
        let mut runtime = MissionScriptRuntime::new(&source, definition.values);
        for npc in definition.npc_scripts {
            runtime
                .register_npc_script(&npc, &read(&format!("{npc}.pst")))
                .unwrap();
        }
        runtime.start().unwrap();
        runtime.host_mut().sync_fighter(0, 2000., 2000., 0.);
        runtime.update(0.1).unwrap();
        let queue: std::collections::VecDeque<_> = runtime
            .host_mut()
            .take_player_perform_requests()
            .into_iter()
            .map(|(action, args)| NpcPerformCommand { action, args })
            .collect();
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[1].action, "pause");
        assert_eq!(queue[1].args[0].as_f64(), 20.0);
        for fps in [30, 60, 125, 165] {
            let dt = 1.0 / fps as f32;

            let frames_per_pulse = (0.33 * fps as f32).floor() as usize + 1;
            let mut state = PlayerPerformance {
                queue: queue.clone(),
                ..default()
            };
            for frame in 1..=43 * frames_per_pulse {
                advance_player_performance(&mut state, dt);
                if frame >= frames_per_pulse && frame < 43 * frames_per_pulse {
                    assert!(state.input_blocked, "fps={fps} frame={frame}");
                }
            }
            assert!(!state.input_blocked, "fps={fps}");
            assert!(state.queue.is_empty());
            assert!(43 * frames_per_pulse < 15 * fps as usize);
        }
    }

    #[test]
    fn perform_clock_does_not_catch_up_or_retain_an_old_pause_after_stop() {
        let mut state = PlayerPerformance::default();
        state.queue.push_back(NpcPerformCommand {
            action: "block".into(),
            args: vec![PstValue::Num(0.0)],
        });
        advance_player_performance(&mut state, 20.0);
        assert!(!state.input_blocked);
        for _ in 0..3 {
            advance_player_performance(&mut state, 0.1);
        }
        assert!(state.input_blocked);
        state.queue.push_back(NpcPerformCommand {
            action: "pause".into(),
            args: vec![PstValue::Num(20.0)],
        });
        player_performance_pulse(&mut state);
        state.queue.clear();
        state.clock = default();
        state.queue.push_back(NpcPerformCommand {
            action: "pause".into(),
            args: vec![PstValue::Num(0.0)],
        });
        player_performance_pulse(&mut state);
        assert!(state.queue.is_empty());
    }

    #[test]
    fn npc_own_on_start_installs_initial_behavior_without_losing_string_target() {
        let source = r#"int g_self;
            OnInit(int self) { g_self = GetSelf(self); }
            OnStart() { ChangeBehavior(g_self, "follow", "vip"); }"#;
        let mut runtime =
            MissionScriptRuntime::new("OnStart(){ AddNPC(\"a\"); }", Default::default());
        runtime.register_npc_script("a", source).unwrap();
        runtime.start().unwrap();
        assert_eq!(
            runtime.host_mut().take_behavior_requests(),
            vec![(1, "follow".into(), vec![PstValue::Str("vip".into())])]
        );
    }

    #[test]
    fn mission_request_split_preserves_adds_when_removes_share_a_tick() {
        let (removes, adds) = split_npc_requests(vec![
            NpcRequest::Add {
                script: "npc_guard".into(),
                id: 7,
            },
            NpcRequest::Remove { id: 3 },
            NpcRequest::Add {
                script: "npc_archer".into(),
                id: 8,
            },
        ]);
        assert_eq!(removes, vec![3]);
        assert_eq!(
            adds,
            vec![("npc_guard".to_string(), 7), ("npc_archer".to_string(), 8)]
        );
    }

    #[test]
    fn behavior_names_preserve_all_eight_web_callbacks() {
        use meteor_ai::behavior::BehaviorMode;
        assert_eq!(behavior_from_name("wait"), Some(BehaviorMode::Wait));
        assert_eq!(behavior_from_name("idle"), Some(BehaviorMode::Idle));
        assert_eq!(behavior_from_name("run"), Some(BehaviorMode::Run));
        assert_eq!(behavior_from_name("follow"), Some(BehaviorMode::Follow));
        assert_eq!(behavior_from_name("patrol"), Some(BehaviorMode::Patrol));
        assert_eq!(
            behavior_from_name("attacktarget"),
            Some(BehaviorMode::AttackTarget)
        );
        assert_eq!(behavior_from_name("kill"), Some(BehaviorMode::Kill));
        assert_eq!(behavior_from_name("dodge"), Some(BehaviorMode::Dodge));
    }

    #[test]
    fn assassin_vip_uses_the_live_launch_mode() {
        let mut app = App::new();
        app.insert_resource(crate::app_state::LaunchRequest {
            mode: "assassin".into(),
            ..default()
        });
        app.add_systems(Update, assign_assassin_vip);
        let debug_enemy = app
            .world_mut()
            .spawn(MissionNpc {
                id: -1,
                name: "debug".into(),
                team: 2,
                in_scene_roster: false,
                view: 400.0,
                think: 50.0,
                forced_behavior: None,
                is_vip: false,
                attack_target_slot: None,
                behavior_args: Vec::new(),
                attack_target_remaining: 0,
            })
            .id();
        let friendly = app
            .world_mut()
            .spawn(MissionNpc {
                id: 1,
                name: "friendly".into(),
                team: 1,
                in_scene_roster: true,
                view: 400.0,
                think: 50.0,
                forced_behavior: None,
                is_vip: false,
                attack_target_slot: None,
                behavior_args: Vec::new(),
                attack_target_remaining: 0,
            })
            .id();
        let enemy = app
            .world_mut()
            .spawn(MissionNpc {
                id: 2,
                name: "enemy".into(),
                team: 2,
                in_scene_roster: true,
                view: 400.0,
                think: 50.0,
                forced_behavior: None,
                is_vip: false,
                attack_target_slot: None,
                behavior_args: Vec::new(),
                attack_target_remaining: 0,
            })
            .id();

        app.update();

        assert!(!app.world().get::<MissionNpc>(debug_enemy).unwrap().is_vip);
        assert!(!app.world().get::<MissionNpc>(friendly).unwrap().is_vip);
        assert!(app.world().get::<MissionNpc>(enemy).unwrap().is_vip);
    }

    #[test]
    fn stage_source_entry_name_maps_web_names() {
        assert_eq!(
            StageSource::entry_name("sn23/sn23.gmb"),
            Some("stage/sn23.gmb".to_string())
        );
        assert_eq!(
            StageSource::entry_name("sn23/sn23.des"),
            Some("stage/sn23.des".to_string())
        );

        assert_eq!(
            StageSource::entry_name("Level/sn23_.pst"),
            Some("level/sn23_.pst".to_string())
        );
        assert_eq!(
            StageSource::entry_name("Level/sn23.wp"),
            Some("level/sn23.wp".to_string())
        );

        assert_eq!(StageSource::entry_name("PScriptLib/Def.h"), None);
    }

    #[test]
    fn custom_pack_core_assets_survive_loading_to_render_handoff() {
        use std::io::Write;
        let mut bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            for (name, payload) in [
                ("stage/sn23.gmb", b"gmb".as_slice()),
                ("stage/sn23.des", b"des".as_slice()),
                ("stage/sn23.cob", b"cob".as_slice()),
            ] {
                zip.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(payload).unwrap();
            }
            zip.finish().unwrap();
        }
        let source = StageSource::Mpk {
            pak: meteor_asset::pak::PakArchive::from_bytes(&bytes).unwrap(),
            path: PathBuf::from("packs/test.mpk"),
        };
        let loaded = capture_loaded_stage_assets(&source, "sn23");
        assert_eq!(loaded.gmb.as_deref(), Some(b"gmb".as_slice()));
        assert_eq!(loaded.des.as_deref(), Some(b"des".as_slice()));
        assert_eq!(loaded.cob.as_deref(), Some(b"cob".as_slice()));
        assert!(loaded.from_pack);
    }

    #[test]
    fn android_pack_set_loads_sn01_without_origin_res_filesystem() {
        struct CurrentWebPacks(Vec<meteor_asset::pak::PakArchive>);

        impl StageAssetSource for CurrentWebPacks {
            fn read(&self, path: &str) -> Option<Vec<u8>> {
                let entry = meteor_asset::runtime_assets::entry_name_for_legacy_path(path)?;
                self.0.iter().rev().find_map(|pack| pack.read_entry(&entry))
            }
        }

        let packs_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
        let index_text =
            std::fs::read_to_string(packs_dir.join("packs.json")).expect("read current packs.json");
        let descriptors =
            crate::pack_manifest::parse_pack_index(&index_text).expect("parse current packs.json");
        let roots = crate::pack_manifest::match_pack_roots("sn01");
        let order = crate::pack_manifest::resolve_pack_order(&descriptors, &roots)
            .expect("resolve Android sn01 pack order");
        let packs = order
            .iter()
            .map(|id| {
                let descriptor = &descriptors[id];
                let path = packs_dir.join(&descriptor.file_name);
                let bytes = std::fs::read(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                meteor_asset::pak::PakArchive::from_mpk1(&bytes)
                    .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
            })
            .collect::<Vec<_>>();
        let source = CurrentWebPacks(packs);
        let waypoint_text = source
            .read("Level/sn01.wp")
            .map(|bytes| crate::text_codec::decode_text(&bytes));
        let center = get_stage_center("sn01", waypoint_text.as_deref());
        let stage = load_stage_assets(&source, "sn01", center);

        assert!(stage.ready, "sn01 must be complete from Android MPKs");
        assert!(stage.scene_object_count > 0);
        assert!(stage.collision_face_count > 0);
    }

    #[test]
    fn resource_roots_resolve_origin_res_from_meteor_rust() {
        let roots = resource_roots(Path::new(r"C:\\project\\meteor_rust"));
        assert!(roots
            .iter()
            .any(|root| root.ends_with(Path::new("origin_res"))));
    }

    #[test]
    fn escort_safe_zone_visuals_use_the_authored_sn03_bounds() {
        for team in [1, 2] {
            let (center, size) = escort_safe_zone_bounds(team, &meteor_network::escort::SN03)
                .expect("authored safe zone");
            let zone = meteor_network::escort::SN03.safe_zones[(team - 1) as usize];
            assert!(
                (center - Vec3::new(zone.center_x, zone.center_y, zone.center_z)).length() < 1e-4
            );
            assert_eq!(
                size,
                Vec3::new(
                    zone.max_x - zone.min_x,
                    zone.max_y - zone.min_y,
                    zone.max_z - zone.min_z,
                )
            );
        }
    }

    #[test]
    fn server_room_disables_local_placeholder_objectives() {
        use crate::plugins::network_plugin::NetStatus;

        assert!(local_mode_objectives_enabled(NetStatus::Disconnected));
        assert!(local_mode_objectives_enabled(NetStatus::Connecting));
        assert!(!local_mode_objectives_enabled(NetStatus::InRoom));
    }

    #[test]
    fn authoritative_escort_flag_hides_only_while_carried() {
        assert!(network_escort_visible("home"));
        assert!(network_escort_visible("dropped"));
        assert!(!network_escort_visible("carried"));
    }

    #[test]
    fn local_scene_team_is_neutral_offline_and_uses_room_assignment() {
        use crate::plugins::network_plugin::{NetState, NetStatus};
        use meteor_network::protocol::RoomMemberWire;

        assert_eq!(local_scene_player_team(&NetState::default()), 0);
        let net = NetState {
            status: NetStatus::InRoom,
            player_id: "player-1".into(),
            members: vec![RoomMemberWire {
                connected: true,
                player_id: "player-1".into(),
                name: "Meteor".into(),
                character_id: 1,
                weapon: 6,
                team: Some(2),
                ready: true,
            }],
            ..Default::default()
        };
        assert_eq!(local_scene_player_team(&net), 2);
        assert_eq!(scene_game_mode(&net), 5);
        assert_eq!(scene_game_mode(&NetState::default()), 1);
    }

    #[test]
    fn scene_snow_config_uses_web_limits_defaults_and_scale() {
        let mut snow = SceneSnowConfig::default();
        assert!(snow.apply("snow", &[1.0]));
        assert!(snow.apply("snowdensity", &[5_000.9]));
        assert!(snow.apply("winddir", &[50.0, -20.0, 9.0]));
        assert!(snow.apply("snowspeed", &[20.0, 100.0]));
        assert!(snow.apply("snowsize", &[5.0, 5.0]));
        assert!(snow.enabled);
        assert_eq!(snow.density, 5_000);
        assert_eq!(snow.particle_count(), 4_000);
        assert_eq!(snow.wind, Vec3::new(50.0, -20.0, 9.0));
        assert_eq!(snow.speed_min, 20.0);
        assert_eq!(snow.speed_max, 100.0);
        assert!((snow.flake_size() - 0.15).abs() < f32::EPSILON);

        snow.apply("snowspeed", &[0.0, 0.0]);
        snow.apply("snowsize", &[1.0]);
        assert_eq!(snow.speed_min, 20.0);
        assert_eq!(snow.speed_max, 20.0);
        assert!((snow.flake_size() - 0.06).abs() < f32::EPSILON);
    }

    #[test]
    fn scene_snow_uses_authored_soft_alpha() {
        let mut images = Assets::<Image>::default();
        let handle = load_scene_snow_texture(&mut images).expect("original snow texture pair");
        let image = images.get(&handle).unwrap();
        let pixels = image.data.as_ref().unwrap();
        let alpha: Vec<_> = pixels.chunks_exact(4).map(|pixel| pixel[3]).collect();
        assert!(alpha.contains(&0), "transparent background");
        assert!(alpha.iter().any(|&value| value > 200), "visible snow core");
        assert!(
            alpha.iter().any(|&value| value > 0 && value < 200),
            "soft edge"
        );
        let width = image.texture_descriptor.size.width as usize;
        let height = image.texture_descriptor.size.height as usize;
        assert_eq!(alpha[0], 0);
        assert_eq!(alpha[width - 1], 0);
        assert_eq!(alpha[(height - 1) * width], 0);
        assert_eq!(alpha[width * height - 1], 0);
    }

    #[test]
    fn scene_snow_moves_in_model_unit_scale_and_recycles_below_floor() {
        let mut positions = vec![Vec3::ZERO, Vec3::new(0.0, -1.9, 0.0)];
        let velocities = vec![20.0, 100.0];
        let mut rng = 0x9e37_79b9_7f4a_7c15;

        advance_scene_snow(
            &mut positions,
            &velocities,
            Vec3::new(50.0, 0.0, 0.0) * SCENE_SNOW_WORLD_SCALE,
            1.0,
            &mut rng,
        );

        assert!((positions[0] - Vec3::new(1.5, -0.6, 0.0)).length() < 1e-5);
        assert_eq!(positions[1].y, SCENE_SNOW_AREA_Y);
        assert!(positions[1].x.abs() <= SCENE_SNOW_AREA_XZ);
        assert!(positions[1].z.abs() <= SCENE_SNOW_AREA_XZ);
    }

    #[test]
    fn scene_touch_uses_expanded_prop_box_and_foot_capsule_height() {
        let center = Vec3::ZERO;
        let half = Vec3::ONE;
        assert!(scene_touch_contains(
            Vec3::new(1.5, -1.5, 0.0),
            center,
            half
        ));
        assert!(scene_touch_contains(Vec3::new(0.0, 2.8, 1.5), center, half));
        assert!(!scene_touch_contains(
            Vec3::new(1.51, 0.0, 0.0),
            center,
            half
        ));
        assert!(!scene_touch_contains(
            Vec3::new(0.0, -1.51, 0.0),
            center,
            half
        ));
        assert!(!scene_touch_contains(
            Vec3::new(0.0, 2.81, 0.0),
            center,
            half
        ));
    }

    #[test]
    fn scene_touch_bounds_do_not_apply_model_scale_twice() {
        let transform = GlobalTransform::from(Transform {
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            scale: Vec3::splat(meteor_core::constants::MODEL_SCALE),
            ..default()
        });
        let half = world_half_extents(&transform, Vec3::new(1.0, 2.0, 3.0));
        assert!((half - Vec3::new(3.0, 2.0, 1.0)).length() < 1e-5);
    }

    #[test]
    fn scene_net_event_preserves_pscript_value_shapes_on_the_wire() {
        let op = scene_op_from_runtime(&SceneNetOp::Set {
            ref_index: 12,
            field: "attribute".into(),
            args: vec![
                PstValue::Str("active".into()),
                PstValue::Num(0.0),
                PstValue::Array(vec![PstValue::Str("extra".into()), PstValue::Num(2.0)]),
            ],
        });
        assert_eq!(op.kind, "set");
        assert_eq!(op.reference, JsonValue::from(12));
        assert_eq!(op.field, "attribute");
        assert_eq!(op.args[0], JsonValue::from("active"));
        assert_eq!(op.args[1].as_f64(), Some(0.0));
        assert_eq!(
            op.args[2],
            JsonValue::Array(vec![JsonValue::from("extra"), number_json(2.0)])
        );
    }

    #[test]
    fn scene_net_event_maps_create_effect_to_wire_shape() {
        let op = scene_op_from_runtime(&SceneNetOp::Effect {
            ref_index: 7,
            effect: "BoxBRK".into(),
            extra: 2.0,
        });
        assert_eq!(op.kind, "effect");
        assert_eq!(op.reference, JsonValue::from(7));
        assert_eq!(op.field, "BoxBRK");
        assert_eq!(op.args, vec![number_json(2.0)]);
    }

    #[test]
    fn server_generated_item_ops_apply_by_des_name_for_the_scene_host_too() {
        let mut runtime = SceneScriptRuntime::load(
            "Scene_OnLoad() {}",
            vec![SceneItemInstance {
                name: "D_It15".into(),
                index: 0,
                ..Default::default()
            }],
            250_000,
        )
        .expect("runtime");
        let op = meteor_network::protocol::SceneOp {
            kind: "set".into(),
            reference: JsonValue::String("D_It15".into()),
            field: "attribute".into(),
            args: vec![JsonValue::String("interactive".into()), JsonValue::from(0)],
        };
        apply_network_scene_op(&mut runtime, &op);
        assert_eq!(
            runtime
                .host()
                .instance(0)
                .expect("instance")
                .get("interactive"),
            PstValue::Num(0.0)
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod loading_worker_tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;
    #[test]
    fn async_loading_failure_returns_error_without_entering_gameplay() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<CustomStages>()
            .init_resource::<crate::plugins::network_plugin::NetState>()
            .init_resource::<LoadedStageInfo>()
            .init_resource::<NextState<AppState>>()
            .insert_resource(LaunchRequest {
                stage_id: "missing-loading-fixture".into(),
                ..default()
            })
            .add_systems(Update, poll_stage_load);
        app.update();
        app.world_mut().run_system_once(begin_stage_load).unwrap();
        assert!(!app.world().resource::<LoadedStageInfo>().ready);
        for _ in 0..400 {
            app.update();
            if !app.world().contains_resource::<PendingStageLoad>() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            !app.world().contains_resource::<PendingStageLoad>(),
            "worker did not complete"
        );
        assert!(app.world().resource::<LoadedStageInfo>().error.is_some());
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Pending(AppState::Error)
        ));
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .reset();
        app.world_mut().run_system_once(begin_stage_load).unwrap();
        app.world_mut().run_system_once(cancel_stage_load).unwrap();
        app.update();
        assert!(!app.world().contains_resource::<PendingStageLoad>());
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Unchanged
        ));
    }
}
