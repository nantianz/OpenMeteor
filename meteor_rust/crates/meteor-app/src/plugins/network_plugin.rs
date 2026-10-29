use bevy::prelude::*;
use meteor_asset::pos::AttackBlock;
use meteor_combat::hit_reaction::{
    begin_be_hit, begin_death, begin_guard_hit, facing_bucket, ReactionFighter,
};
use meteor_combat::weapon_strategy::weapon_to_strategy;
#[cfg(not(target_arch = "wasm32"))]
use meteor_network::escort::SN03;
use meteor_network::modes::is_team_mode;
use meteor_network::protocol::V4Payload;
use meteor_network::protocol::{
    escort_flag_from_v4, CombatReactionWire, EscortFlagSnapshot, FighterSnapshot, ItemEffectWire,
    ItemWeaponSlotWire, Message, RoomMemberWire, RoomSummary, SceneOp,
};
#[cfg(not(target_arch = "wasm32"))]
use meteor_network::role_state::decode_request1_state;
use meteor_network::role_state::{
    decode_event2_state, encode_request1_state, forward_from_yaw, role_state_from_runtime,
    yaw_from_forward, RemotePoseRuntime, RoleState, RoleStateRuntime, RoleVec3, ROLE_UPLINK_HZ,
};
#[cfg(all(test, not(target_arch = "wasm32")))]
use meteor_network::stock_game::StockHeroCatalog;
#[cfg(not(target_arch = "wasm32"))]
use meteor_network::stock_game::{
    StockAuthoredSceneItem, StockGameClient, StockGameEvent, StockGameServer, StockRoomItemRules,
    StockWeaponDropCommand,
};
#[cfg(not(target_arch = "wasm32"))]
use meteor_network::stock_protocol::{
    stock_rule3_effect_id, stock_rule3_effect_name, stock_weapon_from_item_id,
    stock_weapon_item_id, StockHitRequest, StockRoleStateRecord, StockSceneFrame,
    StockSceneOperation, StockSceneOpsEvent, STOCK_SCENE_OP_ACTIVE, STOCK_SCENE_OP_COLLISION,
    STOCK_SCENE_OP_DAMAGE, STOCK_SCENE_OP_EFFECT, STOCK_SCENE_OP_FRAME, STOCK_SCENE_OP_INTERACTIVE,
    STOCK_SCENE_OP_POSE, STOCK_SCENE_OP_VISIBLE,
};
#[cfg(not(target_arch = "wasm32"))]
use meteor_network::stock_session::{
    StockClientTransport, StockDiscoveredRoom, StockDiscoveryClient, StockServerTransport,
    STOCK_LAN_DEFAULT_PORT,
};
use meteor_network::wire_v4::{
    CombatJudge as V4CombatJudge, Input as V4Input, InputVisual as V4InputVisual, PackedObb,
    RoleSnapshot as V4RoleSnapshot, Snapshot as V4Snapshot, Vec2 as V4Vec2, Vec3 as V4Vec3,
};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use std::{cell::Cell, rc::Rc};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{closure::Closure, JsCast};
#[cfg(target_arch = "wasm32")]
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

use crate::app_state::{AppState, LaunchRequest, MenuNetButton, MenuNetStatusText};
use room_browser::RoomTarget;
#[path = "network_asset_barrier.rs"]
mod asset_barrier;
#[path = "room_browser.rs"]
pub(crate) mod room_browser;
#[path = "room_lobby.rs"]
pub(crate) mod room_lobby;
#[cfg(not(target_arch = "wasm32"))]
use crate::app_state::{
    MenuPublicBattleButton, MenuScreen, MenuStockJoinButton, MenuStockLoadoutCancel,
    MenuStockLoadoutConfirm, MenuStockLoadoutDialog, MenuStockRefreshButton,
};
use crate::app_state::{
    MenuStockCampButton, MenuStockHostButton, MenuStockItemRuleButton, MenuStockRuleButton,
    MenuStockSelectOption, MenuStockSelectText, MenuStockStageButton, MenuStockStageText,
    StockConfigField, StockItemRule,
};
use crate::components::fighter::{
    AiIntent, CombatState, DeathRespawn, EntryBlend, EscortFlagCarrier, Fighter,
    FighterHitReaction, GunAimState, Health, LockOnController, MatchEnd, MotionState, PoseRuntime,
    Qi, SmState, TargetHitClock, WeaponSwitchActive,
};
use crate::plugins::character_plugin::{CharacterAssets, CharacterAssetsSet};
use crate::plugins::movement_plugin::{FighterAir, FighterAttackMotion, FighterDodge, FighterWall};
#[cfg(not(target_arch = "wasm32"))]
use crate::plugins::native_web_p2p::{NativeP2pOutput, NativeWebP2p};
use crate::plugins::stage_plugin::LocalFighter;
use crate::plugins::ui_components::UiSelectionState;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetStatus {
    #[default]
    Disconnected,
    Connecting,
    InRoom,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetWireFormat {
    Json,
    #[default]
    V4,
}

impl NetWireFormat {
    fn from_environment() -> Self {
        let enabled = |value: &str| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        };
        if std::env::var("METEOR_PROTOCOL_JSON").is_ok_and(|value| enabled(&value))
            || std::env::var("METEOR_PROTOCOL_V4").is_ok_and(|value| !enabled(&value))
        {
            Self::Json
        } else {
            Self::V4
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum EncodedNetFrame {
    Text(String),
    Binary(Vec<u8>),
}

fn encode_control_frame(
    wire_format: NetWireFormat,
    message: Message,
) -> Result<EncodedNetFrame, String> {
    match wire_format {
        NetWireFormat::Json => Ok(EncodedNetFrame::Text(Message::to_json(&message))),
        NetWireFormat::V4 => V4Payload::Control(message)
            .to_v4_binary()
            .map(EncodedNetFrame::Binary)
            .map_err(|error| error.to_string()),
    }
}

fn encode_input_frame(
    wire_format: NetWireFormat,
    input: V4Input,
) -> Result<Option<EncodedNetFrame>, String> {
    match wire_format {
        NetWireFormat::Json => Ok(None),
        NetWireFormat::V4 => V4Payload::Input(input)
            .to_v4_binary()
            .map(EncodedNetFrame::Binary)
            .map(Some)
            .map_err(|error| error.to_string()),
    }
}

fn decode_text_frame(text: &str) -> Option<NetInbound> {
    Message::from_json(text).map(NetInbound::Control)
}

fn decode_binary_frame(
    wire_format: NetWireFormat,
    bytes: &[u8],
) -> Result<Option<NetInbound>, String> {
    match wire_format {
        NetWireFormat::Json => Ok(std::str::from_utf8(bytes)
            .ok()
            .and_then(Message::from_json)
            .map(NetInbound::Control)),
        NetWireFormat::V4 => Ok(v4_payload_to_inbound(
            Message::from_v4_payload(bytes).map_err(|error| error.to_string())?,
        )),
    }
}

fn v4_payload_to_inbound(payload: V4Payload) -> Option<NetInbound> {
    match payload {
        V4Payload::Control(message) => Some(NetInbound::Control(message)),
        V4Payload::Snapshot(snapshot) => Some(NetInbound::Snapshot(snapshot)),
        V4Payload::RoleSnapshot(snapshot) => Some(NetInbound::RoleSnapshot(snapshot)),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[path = "stock_npc_bridge.rs"]
mod stock_npc_bridge;

#[cfg(not(target_arch = "wasm32"))]
#[path = "stock_connection_dialog.rs"]
pub(crate) mod stock_connection_dialog;
#[cfg(not(target_arch = "wasm32"))]
use stock_connection_dialog::StockConnectionDialog;

#[cfg(not(target_arch = "wasm32"))]
use meteor_network::stock_protocol::{StockAngerSpend, StockVitalsEvent};

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct StockAngerActor {
    revision: u64,
    pending: std::collections::VecDeque<(u64, StockAngerSpend)>,
    keys: std::collections::VecDeque<(u32, u16)>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
pub(crate) struct PendingStockAnger(std::collections::HashMap<Option<Entity>, StockAngerActor>);

#[cfg(not(target_arch = "wasm32"))]
impl PendingStockAnger {
    pub(crate) fn send(
        &mut self,
        tx: &NetTx,
        actor: Option<Entity>,
        generation: u32,
        action: u16,
        cost: u16,
    ) {
        if !(1..=100).contains(&cost)
            || (action == meteor_combat::qi_burst::QI_BURST_ACTION_ID && cost != 60)
        {
            return;
        }
        let Some(sender) = &tx.active else {
            return;
        };
        let spend = if action == meteor_combat::qi_burst::QI_BURST_ACTION_ID {
            StockAngerSpend::Burst
        } else if cost == 100 {
            StockAngerSpend::Full
        } else {
            StockAngerSpend::Cost(u32::from(cost))
        };
        let state = self.0.entry(actor).or_default();
        let key = (generation, action);
        if state.keys.contains(&key) {
            return;
        }
        let revision = state.revision + 1;
        if sender
            .send(NetOutbound::StockAngerSpend {
                actor,
                key,
                spend,
                revision,
            })
            .is_err()
        {
            return;
        }
        state.revision = revision;
        state.pending.push_back((revision, spend));
        state.keys.push_back(key);
        if state.keys.len() > 64 {
            state.keys.pop_front();
        }
    }

    fn project(&mut self, actor: Option<Entity>, revision: u64, vitals: &mut StockVitalsEvent) {
        if let Some(state) = self.0.get_mut(&actor) {
            state.pending.retain(|(pending, _)| *pending > revision);
            vitals.anger = state
                .pending
                .iter()
                .fold(f32::from(vitals.anger), |anger, (_, spend)| {
                    spend.remaining(anger)
                }) as u16;
        }
    }
}

enum NetOutbound {
    Control(Message),
    Input(V4Input),

    OrderedInput(V4Input),
    #[cfg(not(target_arch = "wasm32"))]
    StockRoleState(RoleState),
    #[cfg(not(target_arch = "wasm32"))]
    StockPoseChain(meteor_network::stock_protocol::StockPoseChainRequest),
    #[cfg(not(target_arch = "wasm32"))]
    StockSelfEffect(Option<Entity>, u32),
    #[cfg(not(target_arch = "wasm32"))]
    StockHit(StockHitRequest),
    #[cfg(not(target_arch = "wasm32"))]
    StockNpcRoster(Vec<stock_npc_bridge::NpcRole>),
    #[cfg(not(target_arch = "wasm32"))]
    StockNpcHit(Entity, StockHitRequest),
    #[cfg(not(target_arch = "wasm32"))]
    StockAngerSpend {
        actor: Option<Entity>,
        key: (u32, u16),
        spend: StockAngerSpend,
        revision: u64,
    },
    #[cfg(not(target_arch = "wasm32"))]
    StockSceneManifest(Vec<StockAuthoredSceneItem>),
    #[cfg(not(target_arch = "wasm32"))]
    StockEnvironmentGeometry(meteor_scene::hazards::StageHazards),
    #[cfg(not(target_arch = "wasm32"))]
    StockWeaponDrop(StockWeaponDropCommand),
    #[cfg(not(target_arch = "wasm32"))]
    StockWeaponDropRest(u32, [f32; 3]),
    #[cfg(not(target_arch = "wasm32"))]
    StockWeaponSwitch(u32),
    Quit,
}

enum NetInbound {
    #[cfg(not(target_arch = "wasm32"))]
    StockLatency(Option<f64>),
    Control(Message),
    Snapshot(V4Snapshot),
    RoleSnapshot(V4RoleSnapshot),
    #[cfg(not(target_arch = "wasm32"))]
    StockGame(StockGameEvent),
    #[cfg(not(target_arch = "wasm32"))]
    StockNpcEvent(Entity, StockGameEvent),
    #[cfg(not(target_arch = "wasm32"))]
    StockVitals {
        actor: Option<Entity>,
        vitals: StockVitalsEvent,
        spend_revision: u64,
    },
    Closed {
        code: u16,
        reason: String,
    },
}

pub(super) fn default_web_address() -> String {
    #[cfg(target_arch = "wasm32")]
    if env!("METEOR_BUILD_WS") == "/ws" {
        if let Some(window) = web_sys::window() {
            let location = window.location();
            if let (Ok(protocol), Ok(host)) = (location.protocol(), location.host()) {
                let scheme = if protocol == "https:" { "wss" } else { "ws" };
                return format!("{scheme}://{host}/ws");
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(address) = std::env::var("METEOR_WS") {
        let address = address.trim();
        if !address.is_empty() {
            return address.to_owned();
        }
    }
    env!("METEOR_BUILD_WS").to_owned()
}

fn default_net_address() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(address) = std::env::var("METEOR_STOCK_UDP") {
        let address = address.trim();
        if !address.is_empty() {
            return format!("udp://{address}");
        }
    }
    default_web_address()
}

#[cfg(target_arch = "wasm32")]
const SESSION_STORAGE_KEY: &str = "meteor.network.session.v1";
#[cfg(target_arch = "wasm32")]
const INPUT_SEQ_STORAGE_KEY: &str = "meteor.network.inputSeq.v1";

#[cfg(target_arch = "wasm32")]
#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserSessionState {
    session_token: String,
    input_seq: u32,
    room_id: Option<String>,
    match_epoch: Option<u32>,
    round_epoch: Option<u32>,
}

#[cfg(target_arch = "wasm32")]
fn browser_session_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

#[cfg(target_arch = "wasm32")]
fn load_browser_session() -> BrowserSessionState {
    let Some(storage) = browser_session_storage() else {
        return BrowserSessionState::default();
    };
    let session_token = storage
        .get_item(SESSION_STORAGE_KEY)
        .ok()
        .flatten()
        .unwrap_or_default();
    if session_token.is_empty() {
        return BrowserSessionState::default();
    }
    let Some(json) = storage.get_item(INPUT_SEQ_STORAGE_KEY).ok().flatten() else {
        return BrowserSessionState {
            session_token,
            ..Default::default()
        };
    };
    let Ok(mut restored) = serde_json::from_str::<BrowserSessionState>(&json) else {
        return BrowserSessionState {
            session_token,
            ..Default::default()
        };
    };
    if restored.session_token != session_token
        || restored.room_id.as_deref().is_none_or(str::is_empty)
        || restored.match_epoch.is_none_or(|epoch| epoch == 0)
    {
        restored.input_seq = 0;
        restored.room_id = None;
        restored.match_epoch = None;
        restored.round_epoch = None;
    }
    restored.session_token = session_token;
    restored
}

#[cfg(target_arch = "wasm32")]
fn persist_browser_session(net: &NetState, sequence: u32) {
    let Some(storage) = browser_session_storage() else {
        return;
    };
    if net.session_token.is_empty() {
        let _ = storage.remove_item(SESSION_STORAGE_KEY);
        let _ = storage.remove_item(INPUT_SEQ_STORAGE_KEY);
        return;
    }
    let _ = storage.set_item(SESSION_STORAGE_KEY, &net.session_token);
    let state = BrowserSessionState {
        session_token: net.session_token.clone(),
        input_seq: sequence,
        room_id: (!net.room_id.is_empty()).then(|| net.room_id.clone()),
        match_epoch: net.match_epoch,
        round_epoch: net.round_epoch,
    };
    if let Ok(json) = serde_json::to_string(&state) {
        let _ = storage.set_item(INPUT_SEQ_STORAGE_KEY, &json);
    }
}

#[derive(Resource)]
pub struct NetState {
    pub(crate) latency: super::scoreboard::Latency,

    pub battle_pending: bool,
    pub late_join_team: Option<u32>,

    pub(crate) stock_npc_actors: std::collections::HashMap<Entity, u32>,
    pub(crate) stock_npc_ready: std::collections::HashSet<Entity>,
    pub(crate) pending_npc_hits: Vec<(Entity, AuthorityHit)>,
    pub(crate) pending_npc_vitals: Vec<(Entity, [f32; 3])>,
    pub(crate) stock_room_max_health: f32,

    pub(crate) pending_web_join: Option<Message>,
    pub(crate) lobby: meteor_network::protocol::RoomLobbyWire,
    pub(crate) room_latency:
        std::collections::HashMap<String, meteor_network::protocol::RoomLatencyWire>,
    pub(crate) asset_prepare: Option<asset_barrier::Request>,
    pub(crate) asset_error: Option<String>,
    pub status: NetStatus,
    pub address: String,
    pub player_id: String,
    pub client_id: String,
    pub session_token: String,

    pub transport_connected: bool,
    pub reconnect_attempt: u8,
    pub reconnect_at: Option<f64>,
    pub intentional_disconnect: bool,
    pub resume_requested: bool,
    pub room_id: String,
    pub members: Vec<RoomMemberWire>,
    pub mode: String,
    pub stage: String,

    pub friendly_fire: bool,
    pub match_epoch: Option<u32>,
    pub round_epoch: Option<u32>,
    pub last_full_snapshot_tick: Option<u32>,
    pub last_role_snapshot_tick: Option<u32>,
    pub wire_format: NetWireFormat,

    pub pending_objective_end: Option<i32>,

    pub round_active: bool,
    pub battle_result: Option<super::battle_result::NetworkBattleResult>,
    pub stock_round_number: u32,
    pub stock_round_start_wins: [u32; 2],

    pub role_uplink_hz: u32,
    pub error: Option<String>,

    pub pending_snapshots: Vec<(String, FighterSnapshot, Option<u32>)>,

    pub pending_role_states: Vec<(String, RoleState, u32)>,

    pub pending_pose_chains: Vec<(String, meteor_network::role_state::PoseChain, u32)>,
    pub remote_chain_ticks: std::collections::HashMap<String, u32>,

    pub pending_role_entries: Vec<String>,

    pub pending_stock_vitals: Option<[f32; 3]>,
    pub pending_remote_vitals: std::collections::HashMap<String, [f32; 3]>,

    pub pending_stock_kills: Vec<(String, String)>,

    pub pending_stock_scene_items: Vec<StockSceneItemPickup>,
    pub pending_status_effects: Vec<(Option<Entity>, meteor_network::role_state::PropertyEffect)>,

    pub pending_stock_scene_grounded: Vec<StockSceneItemGrounded>,

    pub remote_pose_sequences: std::collections::HashMap<String, (u16, u16)>,

    pub remote_pose_ids: std::collections::HashMap<String, u16>,

    pub remote_actor_profiles: std::collections::HashMap<String, (u8, u8)>,

    pub stock_equipped_weapons: std::collections::HashMap<String, (u8, u8)>,
    pub remote_actor_teams: std::collections::HashMap<String, i32>,
    pub remote_actor_names: std::collections::HashMap<String, String>,
    pub network_npc_ids: std::collections::HashSet<String>,
    pub pending_remote_despawns: Vec<String>,

    pub pending_hits: Vec<AuthorityHit>,

    pub pending_remote_hits: Vec<(String, AuthorityHit)>,

    pub pending_respawns: Vec<String>,

    pub pending_spawn_relocations: std::collections::HashSet<String>,

    pub escort_flag: Option<EscortFlagSnapshot>,
    pub(crate) escort_definition:
        Option<std::sync::Arc<meteor_network::escort_definition::EscortDefinition>>,

    pub pending_scene_ops: Vec<SceneOp>,
    pub(crate) pending_scene_prop_hits: Vec<Message>,
    pub(crate) pending_scene_transfers: Vec<SceneTransfer>,
    pub(crate) applied_scene_transfers: std::collections::HashMap<String, SceneTransfer>,

    pub pending_item_claims: Vec<Message>,

    pub pending_item_states: Vec<AuthorityItemState>,

    pub pending_weapon_states: Vec<AuthorityWeaponState>,

    pub pending_item_effect_states: Vec<AuthorityItemEffectState>,
    pub pending_status_snapshots:
        std::collections::HashMap<String, Vec<meteor_network::protocol::ItemEffectWire>>,

    pub dynamic_weapon_drops: std::collections::HashMap<String, AuthorityWeaponDrop>,

    pub pending_weapon_drops: Vec<AuthorityWeaponDrop>,

    pub remote_kills: std::collections::HashMap<String, u32>,

    pub stock_deaths: std::collections::HashMap<String, u32>,

    pub stock_team_wins: [u32; 2],

    pub stock_vips: [Option<String>; 2],

    pub radar_star_players: std::collections::HashSet<String>,

    pub chat_messages: std::collections::VecDeque<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct AuthorityHit {
    stock_guard_bucket: Option<meteor_combat::hit_reaction::FacingBucket>,
    damage: f32,
    health: Option<f32>,
    anger: Option<f32>,
    guarded: bool,
    attacker_id: Option<String>,

    effect_strategy: Option<u8>,
    motion_scale: Option<f32>,
    reaction: Option<CombatReactionWire>,
    tick: Option<u32>,
    impact: bool,
    terminal: bool,
}

#[derive(Debug, Clone)]
pub struct AuthorityItemState {
    pub player_id: String,
    pub health: f32,
    pub max_health: f32,
    pub anger: f32,
    pub weapon: String,
    pub weapon_model: Option<String>,
    pub weapon_slot: u8,
    pub weapon_slots: Vec<ItemWeaponSlotWire>,
    pub item_effects: Vec<ItemEffectWire>,

    pub worn_effects: Vec<ItemEffectWire>,
}

#[derive(Debug, Clone)]
pub struct AuthorityWeaponState {
    pub player_id: String,
    pub weapon: String,
    pub weapon_model: Option<String>,
    pub weapon_slot: u8,
    pub weapon_slots: Vec<ItemWeaponSlotWire>,
}

#[derive(Debug, Clone)]
pub struct AuthorityItemEffectState {
    pub player_id: String,
    pub health: f32,
    pub max_health: f32,
    pub anger: f32,
    pub item_effects: Vec<ItemEffectWire>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StockSceneItemPickup {
    pub actor_id: u32,
    pub record_id: u32,
    pub category: u32,
    pub item_id: u32,
    pub position: Option<[f32; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StockSceneItemGrounded {
    pub record_id: u32,
    pub category: u32,
    pub item_id: u32,
    pub position: [f32; 3],
    pub spawned: bool,
}

#[derive(Debug, Clone)]
pub struct AuthorityWeaponDrop {
    pub reference: String,
    pub model: String,
    pub position: [f32; 3],
    pub velocity: Option<[f32; 3]>,

    pub settled: bool,
    pub pickup_ready: bool,
}

impl Default for NetState {
    fn default() -> Self {
        #[cfg(target_arch = "wasm32")]
        let restored = load_browser_session();
        Self {
            battle_pending: false,
            late_join_team: None,
            pending_web_join: None,
            lobby: Default::default(),
            room_latency: Default::default(),
            latency: Default::default(),
            asset_prepare: None,
            asset_error: None,
            status: NetStatus::Disconnected,
            address: default_net_address(),
            player_id: String::new(),
            client_id: String::new(),
            #[cfg(target_arch = "wasm32")]
            session_token: restored.session_token.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            session_token: String::new(),
            transport_connected: false,
            reconnect_attempt: 0,
            reconnect_at: None,
            intentional_disconnect: false,
            resume_requested: false,
            #[cfg(target_arch = "wasm32")]
            room_id: restored.room_id.clone().unwrap_or_default(),
            #[cfg(not(target_arch = "wasm32"))]
            room_id: String::new(),
            members: Vec::new(),
            mode: String::new(),
            stage: String::new(),
            friendly_fire: false,
            stock_npc_actors: Default::default(),
            stock_npc_ready: Default::default(),
            pending_npc_hits: Vec::new(),
            pending_npc_vitals: Vec::new(),
            stock_room_max_health: 2_000.0,
            #[cfg(target_arch = "wasm32")]
            match_epoch: restored.match_epoch,
            #[cfg(not(target_arch = "wasm32"))]
            match_epoch: None,
            #[cfg(target_arch = "wasm32")]
            round_epoch: restored.round_epoch,
            #[cfg(not(target_arch = "wasm32"))]
            round_epoch: None,
            last_full_snapshot_tick: None,
            last_role_snapshot_tick: None,
            wire_format: NetWireFormat::from_environment(),
            pending_objective_end: None,
            round_active: false,
            battle_result: None,
            stock_round_number: 1,
            stock_round_start_wins: [0, 0],
            role_uplink_hz: ROLE_UPLINK_HZ,
            error: None,
            pending_snapshots: Vec::new(),
            pending_role_states: Vec::new(),
            pending_pose_chains: Vec::new(),
            remote_chain_ticks: Default::default(),
            pending_role_entries: Vec::new(),
            pending_stock_vitals: None,
            pending_remote_vitals: Default::default(),
            pending_stock_kills: Vec::new(),
            pending_stock_scene_items: Vec::new(),
            pending_status_effects: Vec::new(),
            pending_stock_scene_grounded: Vec::new(),
            remote_pose_sequences: std::collections::HashMap::new(),
            remote_pose_ids: std::collections::HashMap::new(),
            remote_actor_profiles: std::collections::HashMap::new(),
            stock_equipped_weapons: std::collections::HashMap::new(),
            remote_actor_teams: std::collections::HashMap::new(),
            remote_actor_names: std::collections::HashMap::new(),
            network_npc_ids: std::collections::HashSet::new(),
            pending_remote_despawns: Vec::new(),
            pending_hits: Vec::new(),
            pending_remote_hits: Vec::new(),
            pending_respawns: Vec::new(),
            pending_spawn_relocations: Default::default(),
            escort_flag: None,
            escort_definition: None,
            pending_scene_ops: Vec::new(),
            pending_scene_prop_hits: Vec::new(),
            pending_scene_transfers: Vec::new(),
            applied_scene_transfers: std::collections::HashMap::new(),
            pending_item_claims: Vec::new(),
            pending_item_states: Vec::new(),
            pending_weapon_states: Vec::new(),
            pending_item_effect_states: Vec::new(),
            pending_status_snapshots: Default::default(),
            dynamic_weapon_drops: std::collections::HashMap::new(),
            pending_weapon_drops: Vec::new(),
            remote_kills: std::collections::HashMap::new(),
            stock_deaths: std::collections::HashMap::new(),
            stock_team_wins: [0, 0],
            stock_vips: [None, None],
            radar_star_players: std::collections::HashSet::new(),
            chat_messages: std::collections::VecDeque::new(),
        }
    }
}

impl NetState {
    pub fn is_scene_authority(&self) -> bool {
        self.members
            .first()
            .is_some_and(|member| member.player_id == self.player_id)
    }

    pub fn friendly_fire_enabled(&self) -> bool {
        self.status == NetStatus::InRoom && self.friendly_fire
    }

    pub fn player_team(&self, player_id: &str) -> Option<u8> {
        self.stock_vips
            .iter()
            .position(|vip| vip.as_deref() == Some(player_id))
            .map(|index| index as u8 + 1)
            .or_else(|| {
                self.members
                    .iter()
                    .find(|member| member.player_id == player_id)
                    .and_then(|member| member.team)
                    .and_then(|team| u8::try_from(team).ok())
            })
            .or_else(|| {
                self.remote_actor_teams
                    .get(player_id)
                    .and_then(|team| u8::try_from(*team).ok())
            })
            .filter(|team| matches!(team, 1 | 2))
    }

    pub fn vip_team(&self, player_id: &str) -> Option<u8> {
        let stock = self
            .stock_vips
            .iter()
            .position(|vip| vip.as_deref() == Some(player_id))
            .map(|index| index as u8 + 1);
        stock.or_else(|| {
            self.radar_star_players
                .contains(player_id)
                .then(|| self.player_team(player_id))
                .flatten()
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_stock_udp(&self) -> bool {
        self.address.starts_with("udp://") || self.address.starts_with("udp-host://")
    }

    #[cfg(target_arch = "wasm32")]
    pub fn is_stock_udp(&self) -> bool {
        false
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn is_stock_host(&self) -> bool {
        self.address.starts_with("udp-host://")
    }

    #[cfg(target_arch = "wasm32")]
    pub fn is_stock_host(&self) -> bool {
        false
    }
}

#[derive(Resource, Default)]
pub struct NetTx {
    active: Option<Sender<NetOutbound>>,
    connecting: Option<Sender<NetOutbound>>,
}

impl NetTx {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn send_stock_self_effect(&self, actor: Option<Entity>, id: u32) {
        if let Some(tx) = &self.active {
            let _ = tx.send(NetOutbound::StockSelfEffect(actor, id));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn send_stock_actor_hit(&self, npc: Option<Entity>, hit: StockHitRequest) {
        if let Some(tx) = &self.active {
            let message = npc.map_or(NetOutbound::StockHit(hit), |entity| {
                NetOutbound::StockNpcHit(entity, hit)
            });
            let _ = tx.send(message);
        }
    }
    pub fn send_control(&self, message: Message) {
        if let Some(tx) = &self.active {
            let _ = tx.send(NetOutbound::Control(message));
        }
    }

    fn send_input(&self, input: V4Input) {
        if let Some(tx) = &self.active {
            let _ = tx.send(NetOutbound::Input(input));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn send_stock_hit(&self, hit: StockHitRequest) {
        if let Some(tx) = &self.active {
            let _ = tx.send(NetOutbound::StockHit(hit));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn send_stock_environment_geometry(
        &self,
        geometry: meteor_scene::hazards::StageHazards,
    ) {
        if let Some(tx) = &self.active {
            let _ = tx.send(NetOutbound::StockEnvironmentGeometry(geometry));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn send_stock_scene_manifest(&self, items: Vec<StockAuthoredSceneItem>) {
        if let Some(tx) = &self.active {
            let _ = tx.send(NetOutbound::StockSceneManifest(items));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn send_stock_weapon_drop_rest(&self, id: u32, position: [f32; 3]) {
        if let Some(tx) = &self.active {
            let _ = tx.send(NetOutbound::StockWeaponDropRest(id, position));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn send_stock_weapon_drop(&self, command: StockWeaponDropCommand) {
        if let Some(tx) = &self.active {
            let _ = tx.send(NetOutbound::StockWeaponDrop(command));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn send_stock_weapon_switch(&self, item_id: u32) {
        if let Some(tx) = &self.active {
            let _ = tx.send(NetOutbound::StockWeaponSwitch(item_id));
        }
    }

    fn disconnect(&self) {
        if let Some(tx) = self.active.as_ref().or(self.connecting.as_ref()) {
            let _ = tx.send(NetOutbound::Quit);
        }
    }

    fn begin_connect(&mut self, sender: Sender<NetOutbound>) {
        self.active = None;
        self.connecting = Some(sender);
    }

    fn activate(&mut self) {
        self.active = self.connecting.take();
    }

    fn clear(&mut self) {
        self.active = None;
        self.connecting = None;
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn control_test_channel() -> (NetTx, impl Fn() -> Vec<Message>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (
        NetTx {
            active: Some(tx),
            connecting: None,
        },
        move || {
            rx.try_iter()
                .filter_map(|outbound| match outbound {
                    NetOutbound::Control(message) => Some(message),
                    _ => None,
                })
                .collect()
        },
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn stock_scene_manifest_test_channel(
) -> (NetTx, impl Fn() -> Vec<Vec<StockAuthoredSceneItem>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (
        NetTx {
            active: Some(tx),
            connecting: None,
        },
        move || {
            rx.try_iter()
                .filter_map(|message| match message {
                    NetOutbound::StockSceneManifest(items) => Some(items),
                    _ => None,
                })
                .collect()
        },
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn stock_hit_test_channel(
) -> (NetTx, impl Fn() -> Vec<(Option<Entity>, StockHitRequest)>) {
    let (tx, rx) = std::sync::mpsc::channel();
    (
        NetTx {
            active: Some(tx),
            connecting: None,
        },
        move || {
            rx.try_iter()
                .filter_map(|message| match message {
                    NetOutbound::StockHit(hit) => Some((None, hit)),
                    NetOutbound::StockNpcHit(entity, hit) => Some((Some(entity), hit)),
                    _ => None,
                })
                .collect()
        },
    )
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn stock_anger_test_channel(
) -> (NetTx, impl Fn() -> Vec<(Option<Entity>, StockAngerSpend)>) {
    let (sender, receiver) = std::sync::mpsc::channel();
    (
        NetTx {
            active: Some(sender),
            connecting: None,
        },
        move || {
            receiver
                .try_iter()
                .filter_map(|message| {
                    if let NetOutbound::StockAngerSpend { actor, spend, .. } = message {
                        Some((actor, spend))
                    } else {
                        None
                    }
                })
                .collect()
        },
    )
}

#[derive(Resource, Default)]
pub struct NetRx(Option<std::sync::Mutex<Receiver<NetInbound>>>);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct NativeNetThread {
    handle: Option<std::thread::JoinHandle<()>>,
    release_required: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeNetThread {
    fn reap_finished(&mut self) {
        if self
            .handle
            .as_ref()
            .is_some_and(|handle| handle.is_finished())
        {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
            self.release_required = false;
        }
    }

    fn prepare_new(&mut self) -> Result<(), String> {
        self.reap_finished();
        if self.handle.is_some() {
            Err("上一局域网会话正在关闭，请稍后重试".into())
        } else {
            Ok(())
        }
    }

    fn install(&mut self, address: &str, handle: std::thread::JoinHandle<()>) {
        self.handle = Some(handle);
        self.release_required = address.starts_with("udp://") || address.starts_with("udp-host://");
    }

    fn shutdown(&mut self) {
        let wait_until = std::time::Instant::now()
            + std::time::Duration::from_millis(if self.release_required { 300 } else { 0 });
        while self
            .handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
            && std::time::Instant::now() < wait_until
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        self.reap_finished();
        if self.handle.is_some() && !self.release_required {
            self.handle.take();
        }
    }
}

fn reset_network_session(
    net: &mut NetState,
    net_tx: &mut NetTx,
    net_rx: &mut NetRx,
    sequence: &mut NetInputSequence,
    pending: &mut PendingNetActions,
    chat: &mut NetChatInput,
) {
    let error = net.error.take();
    *net = NetState::default();

    net.session_token.clear();
    net.room_id.clear();
    net.match_epoch = None;
    net.round_epoch = None;
    net.error = error;
    net_tx.clear();
    net_rx.0 = None;
    sequence.0 = 0;
    pending.take();
    chat.text.clear();
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn cleanup_network_session(
    mut net: ResMut<NetState>,
    mut net_tx: ResMut<NetTx>,
    mut net_rx: ResMut<NetRx>,
    mut sequence: ResMut<NetInputSequence>,
    mut pending: ResMut<PendingNetActions>,
    mut chat: ResMut<NetChatInput>,
    mut native_thread: ResMut<NativeNetThread>,
    mut discovery: ResMut<StockLanDiscoveryState>,
) {
    info!(
        "net: leaving session status={:?} address={} worker={}",
        net.status,
        net.address,
        native_thread.handle.is_some()
    );
    if net.status == NetStatus::InRoom || !net.room_id.is_empty() {
        net_tx.send_control(Message::RoomLeave);
    }
    net_tx.disconnect();
    native_thread.shutdown();
    reset_network_session(
        &mut net,
        &mut net_tx,
        &mut net_rx,
        &mut sequence,
        &mut pending,
        &mut chat,
    );
    *discovery = StockLanDiscoveryState::default();
}

#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments)]
fn cleanup_network_session(
    mut net: ResMut<NetState>,
    mut net_tx: ResMut<NetTx>,
    mut net_rx: ResMut<NetRx>,
    mut sequence: ResMut<NetInputSequence>,
    mut pending: ResMut<PendingNetActions>,
    mut chat: ResMut<NetChatInput>,
    mut transport: NonSendMut<WasmNetTransport>,
) {
    if (net.status == NetStatus::InRoom || !net.room_id.is_empty())
        && transport
            .socket
            .as_ref()
            .is_some_and(|socket| socket.ready_state() == WebSocket::OPEN)
    {
        if let Ok(frame) = encode_control_frame(transport.wire_format, Message::RoomLeave) {
            if let Some(socket) = transport.socket.as_ref() {
                let _ = send_wasm_frame(socket, &frame);
            }
        }
    }
    transport.teardown(true);
    reset_network_session(
        &mut net,
        &mut net_tx,
        &mut net_rx,
        &mut sequence,
        &mut pending,
        &mut chat,
    );
    persist_browser_session(&net, sequence.0);
}

#[cfg(target_arch = "wasm32")]
const WASM_SOCKET_BACKPRESSURE_BYTES: f64 = 256.0 * 1024.0;

#[cfg(target_arch = "wasm32")]
struct WasmNetTransport {
    socket: Option<WebSocket>,
    wire_format: NetWireFormat,
    outbound: Option<Receiver<NetOutbound>>,
    inbound: Option<Sender<NetInbound>>,
    closed: Rc<Cell<bool>>,
    on_open: Option<Closure<dyn FnMut(Event)>>,
    on_message: Option<Closure<dyn FnMut(MessageEvent)>>,
    on_error: Option<Closure<dyn FnMut(Event)>>,
    on_close: Option<Closure<dyn FnMut(CloseEvent)>>,
}

#[cfg(target_arch = "wasm32")]
impl Default for WasmNetTransport {
    fn default() -> Self {
        Self {
            socket: None,
            wire_format: NetWireFormat::V4,
            outbound: None,
            inbound: None,
            closed: Rc::new(Cell::new(false)),
            on_open: None,
            on_message: None,
            on_error: None,
            on_close: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl WasmNetTransport {
    fn connect(
        &mut self,
        address: &str,
        wire_format: NetWireFormat,
        hello: Message,
        outbound: Receiver<NetOutbound>,
        inbound: Sender<NetInbound>,
    ) -> Result<(), String> {
        self.teardown(true);
        let socket = WebSocket::new(address)
            .map_err(|error| format!("无法创建 WebSocket {address}: {error:?}"))?;
        socket.set_binary_type(BinaryType::Arraybuffer);
        let hello_frame = encode_control_frame(wire_format, hello)
            .map_err(|error| format!("无法编码 hello: {error}"))?;
        let closed = Rc::new(Cell::new(false));

        let open_socket = socket.clone();
        let open_inbound = inbound.clone();
        let open_closed = closed.clone();
        let on_open = Closure::<dyn FnMut(Event)>::new(move |_| {
            if let Err(error) = send_wasm_frame(&open_socket, &hello_frame) {
                let _ = open_inbound.send(NetInbound::Control(Message::Error {
                    code: "connect".into(),
                    text: error,
                }));
                open_closed.set(true);
            }
        });
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));

        let message_inbound = inbound.clone();
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let data = event.data();
            let decoded = if let Some(text) = data.as_string() {
                Ok(decode_text_frame(&text))
            } else if data.is_instance_of::<js_sys::ArrayBuffer>() {
                let bytes = js_sys::Uint8Array::new(&data).to_vec();
                decode_binary_frame(wire_format, &bytes)
            } else {
                Err("WebSocket 收到非 ArrayBuffer/文本帧".to_string())
            };
            match decoded {
                Ok(Some(message)) => {
                    let _ = message_inbound.send(message);
                }
                Ok(None) => {}
                Err(error) => {
                    let _ = message_inbound.send(NetInbound::Control(Message::Error {
                        code: "v4_decode".into(),
                        text: format!("无法解码服务端消息: {error}"),
                    }));
                }
            }
        });
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        let error_inbound = inbound.clone();
        let on_error = Closure::<dyn FnMut(Event)>::new(move |_| {
            let _ = error_inbound.send(NetInbound::Control(Message::Error {
                code: "connect".into(),
                text: "WebSocket 连接失败".into(),
            }));
        });
        socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        let close_inbound = inbound.clone();
        let close_closed = closed.clone();
        let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |event: CloseEvent| {
            close_closed.set(true);
            let reason = if event.reason().is_empty() {
                format!("WebSocket 已关闭（code {}）", event.code())
            } else {
                event.reason()
            };
            let _ = close_inbound.send(NetInbound::Closed {
                code: event.code(),
                reason,
            });
        });
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));

        self.socket = Some(socket);
        self.wire_format = wire_format;
        self.outbound = Some(outbound);
        self.inbound = Some(inbound);
        self.closed = closed;
        self.on_open = Some(on_open);
        self.on_message = Some(on_message);
        self.on_error = Some(on_error);
        self.on_close = Some(on_close);
        Ok(())
    }

    fn teardown(&mut self, close_socket: bool) {
        if let Some(socket) = self.socket.take() {
            socket.set_onopen(None);
            socket.set_onmessage(None);
            socket.set_onerror(None);
            socket.set_onclose(None);
            if close_socket
                && matches!(
                    socket.ready_state(),
                    WebSocket::CONNECTING | WebSocket::OPEN
                )
            {
                let _ = socket.close_with_code_and_reason(1000, "client close");
            }
        }
        self.outbound = None;
        self.wire_format = NetWireFormat::V4;
        self.inbound = None;
        self.on_open = None;
        self.on_message = None;
        self.on_error = None;
        self.on_close = None;
        self.closed = Rc::new(Cell::new(false));
    }
}

#[cfg(target_arch = "wasm32")]
fn send_wasm_frame(socket: &WebSocket, frame: &EncodedNetFrame) -> Result<(), String> {
    match frame {
        EncodedNetFrame::Text(text) => socket.send_with_str(text),
        EncodedNetFrame::Binary(bytes) => socket.send_with_u8_array(bytes),
    }
    .map_err(|error| format!("WebSocket 发送失败: {error:?}"))
}

#[derive(Resource)]
pub(crate) struct NetInputSequence(u32);

#[allow(clippy::derivable_impls)]
impl Default for NetInputSequence {
    fn default() -> Self {
        #[cfg(target_arch = "wasm32")]
        let input_seq = load_browser_session().input_seq;
        #[cfg(not(target_arch = "wasm32"))]
        let input_seq = 0;
        Self(input_seq)
    }
}

impl NetInputSequence {
    pub(crate) fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(1).max(1);
        self.0
    }
}

#[derive(Resource, Default)]
pub struct NetChatInput {
    pub text: String,
}

#[derive(Resource, Default)]
pub struct PendingNetActions {
    actions: Vec<String>,
}

impl PendingNetActions {
    pub fn push(&mut self, action: impl Into<String>) {
        if self.actions.len() < 8 {
            self.actions.push(action.into());
        }
    }

    fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.actions)
    }
}

#[derive(Component, Debug, Clone)]
pub struct RemoteFighter {
    pub player_id: String,
}

#[derive(Component, Debug, Clone, Copy)]
pub struct RemoteVitalsStatus {
    pub health_known: bool,
    pub anger_known: bool,
}

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct RemoteTransformTarget {
    pub(crate) translation: Vec3,
    pub(crate) yaw: f32,
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct RemoteAuthorityGeneration {
    pub(crate) role_state_tick: Option<u32>,
    pub(crate) last_hit_tick: Option<u32>,
}

#[derive(Component, Debug, Clone, Copy, Default)]
pub(crate) struct RemoteHitMotionWindow {
    pub(crate) remaining: f32,
}

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct RemoteContactSegment {
    pub(crate) from: Vec3,
    pub(crate) to: Vec3,
    pub(crate) valid: bool,

    pub(crate) role_driven: bool,
    pub(crate) state_age: f32,
    pub(crate) hit_age: f32,
}

impl Default for RemoteContactSegment {
    fn default() -> Self {
        Self {
            from: Vec3::ZERO,
            to: Vec3::ZERO,
            valid: false,
            role_driven: false,
            state_age: 0.0,
            hit_age: 10.0,
        }
    }
}

#[derive(Component, Debug, Clone)]
pub struct PendingRemoteHit {
    pub damage: f32,
    pub pose_id: u16,
    pub attack_index: u32,

    pub frame: f32,
    pub cursor: f32,
    pub attack_origin: Vec3,
    pub attack_forward: Vec3,
}

const SNAPSHOT_INTERVAL: f32 = 0.05;

pub struct NetworkPlugin;

fn entered_menu_after_transition(previous: Option<Res<PreviousState<AppState>>>) -> bool {
    previous.is_some()
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct StockLanDiscoveryState {
    browser: Option<StockDiscoveryClient>,
    rooms: Vec<StockDiscoveredRoom>,
    error: Option<String>,
    started_at: Instant,
    last_bind_attempt_ms: Option<u64>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for StockLanDiscoveryState {
    fn default() -> Self {
        Self {
            browser: None,
            rooms: Vec::new(),
            error: None,
            started_at: Instant::now(),
            last_bind_attempt_ms: None,
        }
    }
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NetworkFrameApply;

impl Plugin for NetworkPlugin {
    fn build(&self, app: &mut App) {
        super::native_text_input::install(app);
        room_browser::install(app);
        asset_barrier::install(app);
        app.init_resource::<NetState>()
            .init_resource::<NetTx>()
            .init_resource::<NetRx>()
            .init_resource::<NetInputSequence>()
            .init_resource::<NetChatInput>()
            .init_resource::<PendingNetActions>()
            .add_systems(
                Update,
                (
                    menu_stock_rule_buttons,
                    menu_stock_select_options,
                    menu_stock_camp_buttons,
                    menu_stock_item_rule_buttons,
                    menu_stock_stage_buttons,
                )
                    .chain()
                    .in_set(crate::plugins::UiTouchActionSet)
                    .before(menu_net_button)
                    .run_if(in_state(AppState::Menu)),
            )
            .add_systems(
                Update,
                (
                    net_poll,
                    menu_net_button,
                    net_room_start_button,
                    room_lobby::profile_input,
                    room_lobby::ready_input,
                    net_room_team_select_buttons,
                    net_update_team_select_buttons,
                    net_room_leave_button,
                    net_update_room_members,
                )
                    .in_set(crate::plugins::UiTouchActionSet),
            )
            .add_systems(
                Update,
                (net_chat_input, net_update_chat)
                    .chain()
                    .after(super::native_text_input::NativeTextInputSet)
                    .run_if(in_state(AppState::NetRoom)),
            )
            .add_systems(
                Update,
                room_lobby::update
                    .after(net_poll)
                    .after(room_lobby::profile_input),
            )
            .add_systems(Update, net_reconnect_tick.after(net_poll))
            .add_systems(
                Update,
                (
                    collect_net_key_actions,
                    net_apply_stock_vitals,
                    net_apply_stock_scores,
                    net_apply_snapshot,
                    sync_network_escort_carrier,
                    net_smooth_remote_fighters,
                    net_handle_hit,
                    net_handle_remote_hits,
                    net_apply_authority_respawns,
                    net_apply_remote_vitals,
                    net_apply_scene_transfers,
                )
                    .chain()
                    .in_set(NetworkFrameApply)
                    .before(crate::plugins::FighterFrameSet::SkeletonSample)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                Update,
                (
                    net_send_pending_actions,
                    net_send_pose_chains,
                    net_send_snapshot,
                    net_send_hits,
                    net_send_self_effects,
                )
                    .chain()
                    .after(collect_net_key_actions)
                    .after(crate::plugins::combat_plugin::CombatPoseResolveSet)
                    .after(crate::plugins::character_contact_plugin::CharacterContactFinishSet)
                    .before(crate::plugins::FighterFrameSet::CameraFinalize)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                OnEnter(AppState::NetRoom),
                (
                    reset_net_room,
                    crate::plugins::ui_plugin::setup_net_room_panel,
                ),
            )
            .add_systems(OnExit(AppState::NetRoom), despawn_root_net)
            .add_systems(
                OnExit(AppState::Playing),
                despawn_remote_fighters.run_if(crate::plugins::ends_playing_session),
            )
            .add_systems(
                OnEnter(AppState::Menu),
                cleanup_network_session.run_if(entered_menu_after_transition),
            );
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<StockLanDiscoveryState>()
            .init_resource::<PendingStockConnection>()
            .init_resource::<StockConnectionDialog>()
            .init_resource::<PendingStockAnger>()
            .add_systems(
                OnExit(AppState::Menu),
                |mut dialog: ResMut<StockConnectionDialog>| {
                    *dialog = StockConnectionDialog::default();
                },
            )
            .add_systems(
                Update,
                stock_connection_dialog::input
                    .after(crate::plugins::UiTouchGestureSet)
                    .after(crate::plugins::launcher_ui::launcher_ui_scroll)
                    .before(crate::plugins::UiTouchActionSet)
                    .run_if(in_state(AppState::Menu)),
            )
            .add_systems(
                Update,
                stock_connection_dialog::refresh
                    .after(menu_stock_loadout_confirmation)
                    .after(net_poll)
                    .before(net_reconnect_tick)
                    .run_if(in_state(AppState::Menu)),
            )
            .add_systems(
                OnEnter(AppState::Menu),
                |mut pending: ResMut<PendingStockAnger>,
                 mut connection: ResMut<PendingStockConnection>| {
                    *pending = PendingStockAnger::default();
                    connection.0 = None;
                },
            )
            .add_systems(
                Update,
                stock_npc_bridge::sync_npc_roles
                    .after(net_poll)
                    .after(crate::plugins::combat_plugin::CombatPoseResolveSet)
                    .run_if(in_state(AppState::Playing)),
            )
            .init_resource::<NativeNetThread>()
            .add_systems(
                Update,
                menu_public_battle_button
                    .in_set(crate::plugins::UiTouchActionSet)
                    .before(menu_net_button)
                    .run_if(in_state(AppState::Menu)),
            )
            .add_systems(
                Update,
                (
                    menu_stock_lan_discovery,
                    menu_stock_connection_buttons
                        .after(menu_stock_lan_discovery)
                        .after(menu_stock_stage_buttons)
                        .after(crate::plugins::UiTouchActionSet),
                    menu_stock_loadout_confirmation.after(menu_stock_connection_buttons),
                )
                    .after(crate::plugins::UiTouchActionSet)
                    .run_if(in_state(AppState::Menu)),
            )
            .add_systems(
                Update,
                net_send_stock_weapon_switch
                    .after(net_send_pending_actions)
                    .after(crate::plugins::combat_plugin::CombatPoseResolveSet)
                    .before(crate::plugins::FighterFrameSet::CameraFinalize)
                    .run_if(in_state(AppState::Playing)),
            );
        #[cfg(target_arch = "wasm32")]
        app.init_non_send::<WasmNetTransport>().add_systems(
            Update,
            (
                wasm_flush_outbound.after(net_reconnect_tick),
                persist_wasm_network_session,
            ),
        );
    }
}

fn collect_net_key_actions(
    net: Res<NetState>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<crate::app_state::Keymap>,
    combat_input: Option<Res<crate::plugins::input_plugin::LocalCombatInput>>,
    performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    mut pending: ResMut<PendingNetActions>,
) {
    if net.status != NetStatus::InRoom
        || !net.round_active
        || performance.is_some_and(|performance| performance.input_blocked)
    {
        return;
    }
    use crate::plugins::input_plugin::LocalCombatInputEdge as Edge;
    let bindings = [
        (Edge::QiBurst, keymap.qi_burst),
        (Edge::Cure, keymap.cure),
        (Edge::WeaponSwitch, keymap.weapon_switch),
        (Edge::DropWeapon, keymap.drop_weapon),
        (Edge::DropItem, keymap.drop_item),
    ];
    for edge in crate::plugins::input_plugin::ordered_local_discrete_actions(
        combat_input.as_deref(),
        &bindings,
        &keys,
        &mouse,
    ) {
        let action = match edge {
            Edge::QiBurst => "qi_burst",
            Edge::Cure => "cure",
            Edge::WeaponSwitch => "switch_weapon",
            Edge::DropWeapon => "drop_weapon",
            Edge::DropItem => "drop_item",
            _ => continue,
        };
        pending.push(action);
    }
}

fn despawn_root_net(
    mut commands: Commands,
    query: Query<Entity, With<crate::app_state::NetRoomRoot>>,
) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn reset_net_room(net: Res<NetState>, mut next_state: ResMut<NextState<AppState>>) {
    if net.status != NetStatus::InRoom {
        next_state.set(AppState::Menu);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
struct StockConnectionConfig {
    password: String,
    room_name: String,
    rule: i32,
    game_num: u32,
    game_time_minutes: u32,
    life_adjust_percent: u32,
    item_rules: StockRoomItemRules,
    camp: u32,
    stage_id: String,
    escort_definition: Option<std::sync::Arc<meteor_network::escort_definition::EscortDefinition>>,
}

#[cfg(not(target_arch = "wasm32"))]
fn begin_network_connection(
    net: &mut NetState,
    net_tx: &mut NetTx,
    net_rx: &mut NetRx,
    native_thread: &mut NativeNetThread,
    request: &LaunchRequest,
) -> Result<(), String> {
    info!(
        "net: begin connection address={} stage={} character={} weapon={} worker={}",
        net.address,
        request.stage_id,
        request.character_id,
        request.weapon,
        native_thread.handle.is_some()
    );
    let escort_definition = if request.stock_rule == 2 && net.address.starts_with("udp") {
        Some(super::escort_assets::load_escort(&request.stage_id)?)
    } else {
        None
    };
    net.escort_definition = escort_definition.clone();
    native_thread.prepare_new()?;
    let (to_thread_tx, to_thread_rx) = mpsc::channel();
    let (to_app_tx, to_app_rx) = mpsc::channel();
    net_tx.begin_connect(to_thread_tx);
    net_rx.0 = Some(std::sync::Mutex::new(to_app_rx));
    net.transport_connected = false;
    net.resume_requested = !net.session_token.is_empty();
    net.intentional_disconnect = false;
    let address = net.address.clone();
    let hello = Message::Hello {
        name: request.player_name.clone(),
        resume_token: (!net.session_token.is_empty()).then(|| net.session_token.clone()),
        character_id: request.character_id,
        weapon: request.weapon,
    };
    let wire_format = net.wire_format;
    let stock = StockConnectionConfig {
        password: request.stock_password.clone(),
        room_name: request.room_name.clone(),
        rule: request.stock_rule,
        game_num: request.stock_game_num,
        game_time_minutes: request.stock_game_time_minutes,
        life_adjust_percent: request.stock_life_adjust_percent,
        item_rules: StockRoomItemRules {
            hide_item: request.stock_hide_item,
            health_item: request.stock_health_item,
            update_item: request.stock_update_item,
            special_item: request.stock_special_item,
        },
        camp: request.stock_camp,
        stage_id: request.stage_id.clone(),
        escort_definition,
    };
    let thread_address = address.clone();
    let handle = std::thread::spawn(move || {
        run_connection(address, hello, stock, wire_format, to_thread_rx, to_app_tx);
    });
    native_thread.install(&thread_address, handle);
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn begin_network_connection(
    net: &mut NetState,
    net_tx: &mut NetTx,
    net_rx: &mut NetRx,
    request: &LaunchRequest,
    wasm_transport: &mut WasmNetTransport,
) -> Result<(), String> {
    let (to_socket_tx, to_socket_rx) = mpsc::channel();
    let (to_app_tx, to_app_rx) = mpsc::channel();
    let hello = Message::Hello {
        name: request.player_name.clone(),
        resume_token: (!net.session_token.is_empty()).then(|| net.session_token.clone()),
        character_id: request.character_id,
        weapon: request.weapon,
    };
    wasm_transport.connect(
        &net.address,
        net.wire_format,
        hello,
        to_socket_rx,
        to_app_tx,
    )?;
    net_tx.begin_connect(to_socket_tx);
    net_rx.0 = Some(std::sync::Mutex::new(to_app_rx));
    net.transport_connected = false;
    net.resume_requested = !net.session_token.is_empty();
    net.intentional_disconnect = false;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn menu_net_button(
    route: Option<Res<super::server_route::ServerRoute>>,
    query: Query<&Interaction, (Changed<Interaction>, With<MenuNetButton>)>,
    host: Query<&Interaction, (Changed<Interaction>, With<MenuStockHostButton>)>,
    mut net: ResMut<NetState>,
    mut net_tx: ResMut<NetTx>,
    mut net_rx: ResMut<NetRx>,
    #[cfg(not(target_arch = "wasm32"))] mut native_thread: ResMut<NativeNetThread>,
    rooms: Query<(&Interaction, &RoomTarget), Changed<Interaction>>,
    browser: Res<room_browser::RoomBrowser>,
    request: Res<LaunchRequest>,
    #[cfg(target_arch = "wasm32")] mut wasm_transport: NonSendMut<WasmNetTransport>,
    #[cfg(not(target_arch = "wasm32"))] dialog: Option<Res<StockConnectionDialog>>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    if dialog.is_some_and(|dialog| dialog.is_open()) {
        return;
    }
    if net.status != NetStatus::Disconnected {
        return;
    }
    let selected = rooms.iter().find_map(|(interaction, target)| {
        if *interaction != Interaction::Pressed {
            return None;
        }
        match target {
            RoomTarget::Web(room) => Some(room),
            #[cfg(not(target_arch = "wasm32"))]
            RoomTarget::Lan(_) => None,
        }
    });
    let create = query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed);
    let create = create
        || ((cfg!(target_arch = "wasm32") || request.public_battle)
            && host.iter().any(|i| *i == Interaction::Pressed));
    if create && (cfg!(target_arch = "wasm32") || request.public_battle) {
        if let Err(error) = validate_stock_selection(&request) {
            net.error = Some(error);
            return;
        }
    }
    let action = if let Some(selected) = selected {
        let Some(room) = browser
            .rooms
            .iter()
            .find(|room| room.room_id == selected.room_id)
        else {
            net.error = Some("该 Web 房间已离线，请刷新后重试".into());
            return;
        };
        if let Some(reason) = room_browser::web_room_unavailable(room) {
            net.error = Some(reason.into());
            return;
        }
        web_room_join(Some(room), &request)
    } else if create {
        web_room_join(None, &request)
    } else {
        return;
    };
    if route.as_ref().is_some_and(|route| route.pending()) {
        net.error = Some("正在选择联机服务器，请稍候再试".into());
        return;
    }

    net.session_token.clear();
    net.room_id.clear();
    net.match_epoch = None;
    net.round_epoch = None;
    net.asset_error = None;
    net.asset_prepare = None;
    net.pending_web_join = Some(action);
    net.address = super::server_route::address(route.as_deref());
    #[cfg(not(target_arch = "wasm32"))]
    let result = begin_network_connection(
        &mut net,
        &mut net_tx,
        &mut net_rx,
        &mut native_thread,
        &request,
    );
    #[cfg(target_arch = "wasm32")]
    let result = begin_network_connection(
        &mut net,
        &mut net_tx,
        &mut net_rx,
        &request,
        &mut wasm_transport,
    );
    match result {
        Ok(()) => {
            net.status = NetStatus::Connecting;
            net.reconnect_attempt = 0;
            net.reconnect_at = None;
            net.error = None;
        }
        Err(error) => {
            net.status = NetStatus::Disconnected;
            net.error = Some(error);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn menu_stock_lan_discovery(
    screen: Res<MenuScreen>,
    refresh: Query<&Interaction, (Changed<Interaction>, With<MenuStockRefreshButton>)>,
    mut state: ResMut<StockLanDiscoveryState>,
) {
    if *screen != MenuScreen::Network {
        return;
    }
    let now_ms = state
        .started_at
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let refresh_pressed = refresh
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed);

    if state.browser.is_none()
        && state
            .last_bind_attempt_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= 1_000)
    {
        state.last_bind_attempt_ms = Some(now_ms);
        match StockDiscoveryClient::bind_lan() {
            Ok(browser) => {
                state.browser = Some(browser);
                state.error = None;
            }
            Err(error) => {
                state.error = Some(format!("无法启动局域网搜索: {error}"));
            }
        }
    }
    if refresh_pressed {
        if let Some(browser) = state.browser.as_mut() {
            browser.refresh();
        }
    }
    if let Some(browser) = state.browser.as_mut() {
        match browser.tick(now_ms) {
            Ok(rooms) => {
                state.rooms = rooms;
                state.error = None;
            }
            Err(error) => {
                state.error = Some(format!("局域网搜索失败: {error}"));
                state.browser = None;
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct PendingStockConnection(Option<(String, LaunchRequest)>);

#[derive(Component)]
pub(crate) struct StockLoadoutCamp(pub u32);

#[derive(Component)]
pub(crate) struct StockLoadoutCampSection;

#[derive(Component)]
pub(crate) struct StockPasswordSection;

#[derive(Component)]
pub(crate) struct StockLoadoutModeText;

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn menu_stock_connection_buttons(
    join: Query<&Interaction, (Changed<Interaction>, With<MenuStockJoinButton>)>,
    host: Query<&Interaction, (Changed<Interaction>, With<MenuStockHostButton>)>,
    rooms: Query<(&Interaction, &RoomTarget), Changed<Interaction>>,
    discovery: Res<StockLanDiscoveryState>,
    mut net: ResMut<NetState>,
    mut request: ResMut<LaunchRequest>,
    mut pending: ResMut<PendingStockConnection>,
    mut dialog: ResMut<StockConnectionDialog>,
) {
    let join_pressed = join
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed);
    let host_pressed = !request.public_battle
        && host
            .iter()
            .any(|interaction| *interaction == Interaction::Pressed);
    let room_pressed = rooms.iter().find_map(|(interaction, target)| {
        if *interaction != Interaction::Pressed {
            return None;
        }
        match target {
            RoomTarget::Lan(room) => Some(room),
            RoomTarget::Web(_) => None,
        }
    });
    if !join_pressed && !host_pressed && room_pressed.is_none() {
        return;
    }
    if net.status != NetStatus::Disconnected || pending.0.is_some() || dialog.is_open() {
        return;
    }
    let discovered_room = room_pressed
        .and_then(|selected| {
            discovery
                .rooms
                .iter()
                .find(|room| room.endpoint == selected.endpoint)
        })
        .cloned();
    if room_pressed.is_some() && discovered_room.is_none() {
        dialog.fail("该局域网房间已离线，请刷新后重试".into());
        return;
    }
    request.stock_password.clear();
    let mut request = request.clone();
    if !host_pressed {
        request.stock_rule = 0;
    }
    if let Some(room) = &discovered_room {
        apply_stock_discovered_room(&mut request, room);
    }
    if host_pressed {
        if let Err(error) = validate_stock_selection(&request) {
            dialog.fail(error);
            return;
        }
    }

    let address = if let Some(room) = discovered_room {
        format!("udp://{}", room.endpoint)
    } else if join_pressed {
        let target = request.stock_server_address.trim();
        if target.is_empty() {
            dialog.fail("请输入原版服务器 IP:端口".into());
            return;
        }
        let target = target
            .strip_prefix("udp://")
            .or_else(|| target.strip_prefix("udp-host://"))
            .unwrap_or(target);
        format!("udp://{target}")
    } else {
        format!("udp-host://{}", stock_host_bind_address())
    };

    pending.0 = Some((address, request));
    net.error = None;
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
fn menu_stock_loadout_confirmation(
    mut commands: Commands,
    mut probe: Local<Option<(String, StockDiscoveryClient, std::time::Instant)>>,
    mut sections: Query<
        (&mut Node, Option<&StockPasswordSection>),
        (
            Or<(With<StockLoadoutCampSection>, With<StockPasswordSection>)>,
            Without<MenuStockLoadoutDialog>,
        ),
    >,
    mut mode_labels: Query<&mut Text, With<StockLoadoutModeText>>,
    mut camps: Query<(&Interaction, &StockLoadoutCamp, &mut UiSelectionState)>,
    confirm: Query<(Entity, &Interaction), With<MenuStockLoadoutConfirm>>,
    cancel: Query<&Interaction, (Changed<Interaction>, With<MenuStockLoadoutCancel>)>,
    mut dialogs: Query<&mut Node, With<MenuStockLoadoutDialog>>,
    mut pending: ResMut<PendingStockConnection>,
    mut net: ResMut<NetState>,
    mut net_tx: ResMut<NetTx>,
    mut net_rx: ResMut<NetRx>,
    mut native_thread: ResMut<NativeNetThread>,
    mut request: ResMut<LaunchRequest>,
    mut dialog: ResMut<StockConnectionDialog>,
) {
    let mode_result = (|| -> Result<(), String> {
        let Some((address, selection)) = pending.0.as_mut() else {
            *probe = None;
            return Ok(());
        };
        if !(1..=5).contains(&selection.stock_rule) {
            if probe
                .as_ref()
                .is_none_or(|(target, _, _)| target != address)
            {
                let endpoint = address
                    .strip_prefix("udp://")
                    .unwrap_or(address)
                    .parse::<std::net::SocketAddr>()
                    .map_err(|_| "请输入有效的服务器 IP:端口".to_string())?;
                let browser = StockDiscoveryClient::bind_with_destination(
                    "0.0.0.0:0",
                    endpoint,
                    "serverlist",
                )
                .map_err(|error| error.to_string())?;
                *probe = Some((address.clone(), browser, std::time::Instant::now()));
            }
            let (_, browser, started) = probe.as_mut().unwrap();
            let rooms = browser
                .tick(started.elapsed().as_millis() as u64)
                .map_err(|error| error.to_string())?;
            if let Some(room) = rooms.iter().find(|room| room.info.rule().is_some()) {
                apply_stock_discovered_room(selection, room);
            } else if started.elapsed().as_secs() >= 8 {
                return Err("未能读取房间模式，请刷新重试或更新房主版本".into());
            }
        }
        Ok(())
    })();
    if let Err(error) = mode_result {
        pending.0 = None;
        *probe = None;
        dialog.fail(error);
    }
    let rule = pending
        .0
        .as_ref()
        .map(|(_, selection)| selection.stock_rule)
        .unwrap_or(0);
    let team_mode = (2..=5).contains(&rule);

    let hosting = pending
        .0
        .as_ref()
        .is_some_and(|(address, _)| address.starts_with("udp-host://"));
    for (mut node, password) in &mut sections {
        let visible = if password.is_some() {
            pending
                .0
                .as_ref()
                .is_some_and(|(address, _)| address.starts_with("udp://"))
        } else {
            team_mode && !hosting
        };
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (entity, _) in &confirm {
        if (1..=5).contains(&rule) {
            commands
                .entity(entity)
                .remove::<bevy::ui::InteractionDisabled>();
        } else {
            commands
                .entity(entity)
                .insert(bevy::ui::InteractionDisabled);
        }
    }
    for mut label in &mut mode_labels {
        label.0 = match rule {
            1 => "盟主模式 · 无需选择阵营",
            2..=5 if hosting => "阵营沿用建房设置 · 请选择人物与兵器",
            2..=5 => "请选择流星或蝴蝶阵营",
            _ => "正在读取房间模式…",
        }
        .into();
    }
    if let Some((_, selection)) = pending.0.as_mut() {
        if rule == 1 {
            selection.stock_camp = 0;
        }
        for (interaction, camp, _) in &mut camps {
            if !hosting
                && team_mode
                && *interaction == Interaction::Pressed
                && matches!(camp.0, 1 | 2)
            {
                selection.stock_camp = camp.0;
            }
        }
        for (_, camp, mut state) in &mut camps {
            state.selected = team_mode && camp.0 == selection.stock_camp;
        }
    }
    if cancel
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
        || net.status != NetStatus::Disconnected
    {
        pending.0 = None;
        request.stock_password.clear();
    }
    let confirmed = (1..=5).contains(&rule)
        && confirm
            .iter()
            .any(|(_, interaction)| *interaction == Interaction::Pressed);
    for mut node in &mut dialogs {
        node.display = if pending.0.is_some() && !confirmed {
            Display::Flex
        } else {
            Display::None
        };
    }
    if !confirmed {
        return;
    }
    let Some((address, mut selection)) = pending.0.take() else {
        return;
    };
    selection.character_id = request.character_id;
    selection.weapon = request.weapon;
    selection.stock_password = if address.starts_with("udp://") {
        std::mem::take(&mut request.stock_password)
    } else {
        String::new()
    };
    *request = selection;
    net.address = address;
    stock_connection_dialog::start(
        &mut dialog,
        &mut net,
        &mut net_tx,
        &mut net_rx,
        &mut native_thread,
        &request,
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_stock_discovered_room(request: &mut LaunchRequest, room: &StockDiscoveredRoom) {
    request.stock_rule = room.info.rule().unwrap_or(0);
    request.stock_camp = if request.stock_rule == 1 {
        0
    } else if room.info.field_0e <= room.info.field_0f {
        1
    } else {
        2
    };
}

fn validate_stock_selection(request: &LaunchRequest) -> Result<(), String> {
    if !(1..=5).contains(&request.stock_rule) {
        return Err("原版规则必须是 Rule 1..5".into());
    }
    if !(1..=9_999).contains(&request.stock_game_num) {
        return Err("局数必须是 1..9999".into());
    }
    if !(1..=999).contains(&request.stock_game_time_minutes) {
        return Err("单场时限必须是 1..999 分钟".into());
    }
    if !(10..=500).contains(&request.stock_life_adjust_percent) {
        return Err("生命调整必须是 10%..500%".into());
    }
    if request.stock_rule == 1 && request.stock_camp != 0 {
        return Err("盟主模式必须选择无阵营".into());
    }
    if request.stock_rule != 1 && !(1..=2).contains(&request.stock_camp) {
        return Err("劫镖/护城/暗杀/死斗必须选择流星队或蝴蝶队".into());
    }
    if request.stock_rule == 2 {
        #[cfg(not(target_arch = "wasm32"))]
        super::escort_assets::load_escort(&request.stage_id)?;
        #[cfg(target_arch = "wasm32")]
        if !request.stage_id.eq_ignore_ascii_case("sn03") {
            return Err("公网劫镖只支持 sn03".into());
        }
    }
    if request.stock_rule == 3 && !matches!(request.stage_id.as_str(), "sn04" | "sn11" | "sn12") {
        return Err("原版护城只支持 sn04、sn11、sn12".into());
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn stock_host_bind_address() -> String {
    format!("0.0.0.0:{STOCK_LAN_DEFAULT_PORT}")
}

fn menu_stock_rule_buttons(
    pressed: Query<(&Interaction, &MenuStockRuleButton), Changed<Interaction>>,
    mut buttons: Query<(&MenuStockRuleButton, &mut UiSelectionState)>,
    mut request: ResMut<LaunchRequest>,
) {
    for (interaction, rule) in &pressed {
        if *interaction == Interaction::Pressed && (1..=5).contains(&rule.0) {
            request.stock_rule = rule.0;
            if rule.0 == 1 {
                request.stock_camp = 0;
            } else if request.stock_camp == 0 {
                request.stock_camp = 1;
            }
            normalize_stock_stage_for_rule(&mut request);
        }
    }
    for (rule, mut selection) in &mut buttons {
        selection.selected = rule.0 == request.stock_rule;
    }
}

fn menu_stock_select_options(
    pressed: Query<(&Interaction, &MenuStockSelectOption), Changed<Interaction>>,
    mut options: Query<(&MenuStockSelectOption, &mut UiSelectionState)>,
    mut request: ResMut<LaunchRequest>,
    mut texts: Query<(&MenuStockSelectText, &mut Text)>,
) {
    for (interaction, option) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match option.field {
            StockConfigField::GameNum => request.stock_game_num = option.value.clamp(1, 9_999),
            StockConfigField::GameTime => {
                request.stock_game_time_minutes = option.value.clamp(1, 999)
            }
            StockConfigField::LifeAdjust => {
                request.stock_life_adjust_percent = option.value.clamp(10, 500)
            }
        }
    }
    for (option, mut selection) in &mut options {
        selection.selected = match option.field {
            StockConfigField::GameNum => request.stock_game_num == option.value,
            StockConfigField::GameTime => request.stock_game_time_minutes == option.value,
            StockConfigField::LifeAdjust => request.stock_life_adjust_percent == option.value,
        };
    }
    for (field, mut text) in &mut texts {
        text.0 = stock_config_value_label(field.0, &request);
    }
}

fn menu_stock_camp_buttons(
    pressed: Query<(&Interaction, &MenuStockCampButton), Changed<Interaction>>,
    mut buttons: Query<(&MenuStockCampButton, &mut Node, &mut UiSelectionState)>,
    mut request: ResMut<LaunchRequest>,
) {
    for (interaction, camp) in &pressed {
        let valid = if request.stock_rule == 1 {
            camp.0 == 0
        } else {
            (1..=2).contains(&camp.0)
        };
        if *interaction == Interaction::Pressed && valid {
            request.stock_camp = camp.0;
        }
    }
    for (camp, mut node, mut selection) in &mut buttons {
        let valid = if request.stock_rule == 1 {
            camp.0 == 0
        } else {
            (1..=2).contains(&camp.0)
        };
        node.display = if valid { Display::Flex } else { Display::None };
        selection.selected = valid && camp.0 == request.stock_camp;
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn menu_public_battle_button(
    pressed: Query<&Interaction, (Changed<Interaction>, With<MenuPublicBattleButton>)>,
    mut selections: Query<&mut UiSelectionState, With<MenuPublicBattleButton>>,
    mut request: ResMut<LaunchRequest>,
) {
    if pressed
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        request.public_battle = !request.public_battle;
    }
    for mut selection in &mut selections {
        selection.selected = request.public_battle;
    }
}

fn menu_stock_item_rule_buttons(
    pressed: Query<(&Interaction, &MenuStockItemRuleButton), Changed<Interaction>>,
    mut buttons: Query<(&MenuStockItemRuleButton, &mut UiSelectionState)>,
    mut request: ResMut<LaunchRequest>,
) {
    for (interaction, rule) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let enabled = match rule.0 {
            StockItemRule::Hide => &mut request.stock_hide_item,
            StockItemRule::Health => &mut request.stock_health_item,
            StockItemRule::Update => &mut request.stock_update_item,
            StockItemRule::Special => &mut request.stock_special_item,
        };
        *enabled = !*enabled;
    }
    for (rule, mut selection) in &mut buttons {
        selection.selected = match rule.0 {
            StockItemRule::Hide => request.stock_hide_item,
            StockItemRule::Health => request.stock_health_item,
            StockItemRule::Update => request.stock_update_item,
            StockItemRule::Special => request.stock_special_item,
        };
    }
}

fn menu_stock_stage_buttons(
    pressed: Query<(&Interaction, &MenuStockStageButton), Changed<Interaction>>,
    mut buttons: Query<(&MenuStockStageButton, &mut Node, &mut UiSelectionState)>,
    mut labels: Query<&mut Text, With<MenuStockStageText>>,
    mut request: ResMut<LaunchRequest>,
) {
    if !request.is_changed()
        && !pressed
            .iter()
            .any(|(interaction, _)| *interaction == Interaction::Pressed)
    {
        return;
    }
    for (interaction, stage) in &pressed {
        if *interaction == Interaction::Pressed
            && stock_stage_allowed_for_rule(request.stock_rule, &stage.id)
        {
            request.stage_id.clone_from(&stage.id);
            request.stage_name.clone_from(&stage.name);
        }
    }
    for (stage, mut node, mut selection) in &mut buttons {
        let allowed = stock_stage_allowed_for_rule(request.stock_rule, &stage.id);
        node.display = if allowed {
            Display::Flex
        } else {
            Display::None
        };
        selection.selected = allowed && request.stage_id == stage.id;
    }
    for mut label in &mut labels {
        label.0 = format!(
            "当前：{} · {}",
            request.stage_id.to_uppercase(),
            request.stage_name
        );
    }
}

fn stock_config_value_label(field: StockConfigField, request: &LaunchRequest) -> String {
    match field {
        StockConfigField::GameNum if request.stock_game_num == 9_999 => "无限局".into(),
        StockConfigField::GameNum => format!("{} 局", request.stock_game_num),
        StockConfigField::GameTime if request.stock_game_time_minutes == 999 => "不限时".into(),
        StockConfigField::GameTime => format!("{} 分钟", request.stock_game_time_minutes),
        StockConfigField::LifeAdjust => format!("{}%", request.stock_life_adjust_percent),
    }
}

fn stock_stage_allowed_for_rule(rule: i32, stage_id: &str) -> bool {
    match rule {
        2 => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                super::escort_assets::load_escort(stage_id).is_ok()
            }
            #[cfg(target_arch = "wasm32")]
            {
                stage_id.eq_ignore_ascii_case("sn03")
            }
        }
        3 => matches!(
            stage_id.to_ascii_lowercase().as_str(),
            "sn04" | "sn11" | "sn12"
        ),
        _ => meteor_scene::stage_registry::is_official_stage_id(stage_id),
    }
}

fn normalize_stock_stage_for_rule(request: &mut LaunchRequest) {
    if stock_stage_allowed_for_rule(request.stock_rule, &request.stage_id) {
        return;
    }
    let fallback = match request.stock_rule {
        2 => "sn03",
        3 => "sn04",
        _ => "sn01",
    };
    if let Some(stage) = meteor_scene::stage_registry::official_stages()
        .into_iter()
        .find(|stage| stage.id == fallback)
    {
        request.stage_id = stage.id;
        request.stage_name = stage.name;
    }
}

#[cfg(target_arch = "wasm32")]
fn wasm_flush_outbound(mut transport: NonSendMut<WasmNetTransport>) {
    flush_wasm_transport(&mut transport);
}

#[cfg(target_arch = "wasm32")]
fn flush_wasm_transport(transport: &mut WasmNetTransport) {
    if transport.closed.get() {
        transport.teardown(true);
        return;
    }
    let Some(socket) = transport.socket.as_ref() else {
        return;
    };
    if socket.ready_state() != WebSocket::OPEN
        || socket.buffered_amount() as f64 > WASM_SOCKET_BACKPRESSURE_BYTES
    {
        return;
    }
    let mut close = false;
    let mut failure = None;
    let wire_format = transport.wire_format;
    if let Some(outbound) = transport.outbound.as_ref() {
        loop {
            let message = match outbound.try_recv() {
                Ok(message) => message,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    close = true;
                    break;
                }
            };
            let frame = match message {
                NetOutbound::Control(message) => {
                    encode_control_frame(wire_format, message).map(Some)
                }
                NetOutbound::Input(input) | NetOutbound::OrderedInput(input) => {
                    encode_input_frame(wire_format, input)
                }
                NetOutbound::Quit => {
                    close = true;
                    break;
                }
            };
            match frame {
                Ok(Some(frame)) => {
                    if let Err(error) = send_wasm_frame(socket, &frame) {
                        failure = Some(error);
                        break;
                    }
                    if socket.buffered_amount() as f64 > WASM_SOCKET_BACKPRESSURE_BYTES {
                        break;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    failure = Some(format!("无法编码 WebSocket 消息: {error}"));
                    break;
                }
            }
        }
    }
    if let Some(error) = failure {
        if let Some(inbound) = transport.inbound.as_ref() {
            let _ = inbound.send(NetInbound::Control(Message::Error {
                code: "send".into(),
                text: error,
            }));
        }
        close = true;
    }
    if close {
        transport.teardown(true);
    }
}

#[derive(bevy::ecs::system::SystemParam)]
struct ReconnectResources<'w> {
    net: ResMut<'w, NetState>,
    net_tx: ResMut<'w, NetTx>,
    net_rx: ResMut<'w, NetRx>,
    request: Res<'w, LaunchRequest>,
    current: Res<'w, State<AppState>>,
    next_state: ResMut<'w, NextState<AppState>>,
    #[cfg(not(target_arch = "wasm32"))]
    native_thread: ResMut<'w, NativeNetThread>,
}

fn net_reconnect_tick(
    time: Res<Time>,
    mut resources: ReconnectResources<'_>,
    #[cfg(target_arch = "wasm32")] mut wasm_transport: NonSendMut<WasmNetTransport>,
) {
    let Some(reconnect_at) = resources.net.reconnect_at else {
        return;
    };
    if resources.net.transport_connected || time.elapsed_secs_f64() < reconnect_at {
        return;
    }
    resources.net.reconnect_at = None;
    #[cfg(not(target_arch = "wasm32"))]
    let result = begin_network_connection(
        &mut resources.net,
        &mut resources.net_tx,
        &mut resources.net_rx,
        &mut resources.native_thread,
        &resources.request,
    );
    #[cfg(target_arch = "wasm32")]
    let result = begin_network_connection(
        &mut resources.net,
        &mut resources.net_tx,
        &mut resources.net_rx,
        &resources.request,
        &mut wasm_transport,
    );
    if let Err(error) = result {
        resources.net.error = Some(error);
        if !schedule_reconnect(&mut resources.net, time.elapsed_secs_f64()) {
            let current = *resources.current.get();
            hard_disconnect(&mut resources.net, &mut resources.next_state, &current);
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn persist_wasm_network_session(
    time: Res<Time>,
    net: Res<NetState>,
    sequence: Res<NetInputSequence>,
    mut last_persisted: Local<f64>,
) {
    if time.elapsed_secs_f64() - *last_persisted < 0.25 {
        return;
    }
    *last_persisted = time.elapsed_secs_f64();
    persist_browser_session(&net, sequence.0);
}

#[cfg(not(target_arch = "wasm32"))]
async fn flush_native_p2p_outputs<S>(
    sink: &mut S,
    to_app: &Sender<NetInbound>,
    wire_format: NetWireFormat,
    outputs: Vec<NativeP2pOutput>,
) -> Result<(), ()>
where
    S: futures_util::Sink<tokio_tungstenite::tungstenite::Message> + Unpin,
{
    use futures_util::SinkExt;
    for output in outputs {
        match output {
            NativeP2pOutput::WebSocketControl(message) => {
                let frame = encode_control_frame(wire_format, *message).map_err(|error| {
                    let _ = to_app.send(NetInbound::Control(Message::Error {
                        code: "v4_encode".into(),
                        text: format!("无法编码 P2P 信令: {error}"),
                    }));
                })?;
                let frame = match frame {
                    EncodedNetFrame::Text(text) => {
                        tokio_tungstenite::tungstenite::Message::Text(text.into())
                    }
                    EncodedNetFrame::Binary(bytes) => {
                        tokio_tungstenite::tungstenite::Message::Binary(bytes.into())
                    }
                };
                sink.send(frame).await.map_err(|_| ())?;
            }
            NativeP2pOutput::InboundFrame(bytes) => {
                match decode_binary_frame(NetWireFormat::V4, &bytes) {
                    Ok(Some(message)) => {
                        let _ = to_app.send(message);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "p2p_decode".into(),
                            text: format!("无法解码 P2P 消息: {error}"),
                        }));
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
fn run_stock_connection(
    address: &str,
    hello: Message,
    camp: u32,
    to_thread: Receiver<NetOutbound>,
    to_app: Sender<NetInbound>,
) {
    run_stock_connection_with_password(address, hello, camp, "", to_thread, to_app);
}

#[cfg(not(target_arch = "wasm32"))]
fn run_stock_connection_with_password(
    address: &str,
    hello: Message,
    camp: u32,
    password: &str,
    to_thread: Receiver<NetOutbound>,
    to_app: Sender<NetInbound>,
) {
    let Message::Hello { name, .. } = &hello else {
        let _ = to_app.send(NetInbound::Control(Message::Error {
            code: "stock_hello".into(),
            text: "原版 UDP 连接缺少角色身份".into(),
        }));
        return;
    };
    let local = std::env::var("METEOR_STOCK_BIND").unwrap_or_else(|_| "0.0.0.0:0".into());
    let transport = match StockClientTransport::bind(local.as_str(), address, {
        let mut record = stock_connect_record(name);
        record.password = password.to_owned();
        record
    }) {
        Ok(transport) => transport,
        Err(error) => {
            let _ = to_app.send(NetInbound::Control(Message::Error {
                code: "stock_bind".into(),
                text: format!("无法创建原版 UDP 连接: {error}"),
            }));
            return;
        }
    };
    run_stock_client_loop(transport, None, hello, camp, to_thread, to_app);
}

#[cfg(not(target_arch = "wasm32"))]
fn run_stock_host_connection(
    bind: &str,
    hello: Message,
    stock: StockConnectionConfig,
    to_thread: Receiver<NetOutbound>,
    to_app: Sender<NetInbound>,
) {
    let Message::Hello { name, .. } = &hello else {
        let _ = to_app.send(NetInbound::Control(Message::Error {
            code: "stock_hello".into(),
            text: "原版 UDP 房主缺少角色身份".into(),
        }));
        return;
    };
    let room_name = stock.room_name.clone();

    let server_transport = match StockServerTransport::bind(bind, "serverlist", room_name) {
        Ok(transport) => transport,
        Err(error) => {
            let _ = to_app.send(NetInbound::Control(Message::Error {
                code: "stock_host_bind".into(),
                text: format!("无法监听原版 UDP 房间 {bind}: {error}"),
            }));
            return;
        }
    };
    let local_addr = match server_transport.local_addr() {
        Ok(address) => address,
        Err(error) => {
            let _ = to_app.send(NetInbound::Control(Message::Error {
                code: "stock_host_addr".into(),
                text: format!("无法读取原版 UDP 房主地址: {error}"),
            }));
            return;
        }
    };
    let loopback = match local_addr {
        std::net::SocketAddr::V4(address) => std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            address.port(),
        ),
        std::net::SocketAddr::V6(address) => std::net::SocketAddr::new(
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            address.port(),
        ),
    };
    let client_bind = if loopback.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let client_transport =
        match StockClientTransport::bind(client_bind, loopback, stock_connect_record(name)) {
            Ok(transport) => transport,
            Err(error) => {
                let _ = to_app.send(NetInbound::Control(Message::Error {
                    code: "stock_host_client".into(),
                    text: format!("无法把房主角色接入本地原版 UDP 房间: {error}"),
                }));
                return;
            }
        };
    let mut server = StockGameServer::with_rule_and_combat_resources(
        server_transport,
        stock_asset_catalog(),
        stock.rule,
        stock_combat_resources(),
    );
    server.set_round_limit(stock.game_num);
    server.set_round_time_minutes(stock.game_time_minutes);
    server.set_life_adjust_percent(stock.life_adjust_percent);
    server.set_item_rules(stock.item_rules);
    server.set_scene_frame(stock_scene_frame(&stock.stage_id));
    server.set_stage_id(stock.stage_id);
    if let Some(definition) = stock.escort_definition {
        server.set_escort_definition(definition);
    }
    run_stock_client_loop(
        client_transport,
        Some(server),
        hello,
        stock.camp,
        to_thread,
        to_app,
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn stock_connect_record(name: &str) -> meteor_network::stock_udp::StockConnectRecord {
    let password = std::env::var("METEOR_STOCK_PASSWORD").unwrap_or_default();
    let checksum = meteor_network::stock_udp::stock_integrity_checksum(stock_read_legacy_file);
    let mut record = meteor_network::stock_udp::StockConnectRecord::new(password, checksum);
    record.player_name = name.to_owned();
    record
}

#[cfg(not(target_arch = "wasm32"))]
fn stock_read_legacy_file(path: &str) -> Option<Vec<u8>> {
    meteor_asset::runtime_assets::read_legacy_path(path).or_else(|| {
        let current = std::env::current_dir().ok()?;
        let relative = path.replace('\\', "/");
        crate::plugins::audio_plugin::resource_roots(&current)
            .into_iter()
            .find_map(|root| crate::resource_io::read(root.join(&relative)).ok())
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn stock_scene_frame(stage_id: &str) -> StockSceneFrame {
    let waypoints = stock_read_legacy_file(&format!("Level/{stage_id}.wp"))
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    StockSceneFrame(meteor_scene::stage_loader::get_stage_center(
        stage_id,
        waypoints.as_deref(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
fn stock_combat_resources() -> meteor_asset::res::CombatResourceTable {
    meteor_asset::runtime_assets::read_legacy_path("Meteor.res")
        .and_then(|bytes| meteor_asset::res::parse_meteor_res(&bytes).ok())
        .map(|resource| resource.combat_table())
        .or_else(|| {
            let current = std::env::current_dir().ok()?;
            crate::plugins::audio_plugin::resource_roots(&current)
                .into_iter()
                .find_map(|root| {
                    crate::resource_io::read(root.join("Meteor.res"))
                        .ok()
                        .and_then(|bytes| meteor_asset::res::parse_meteor_res(&bytes).ok())
                        .map(|resource| resource.combat_table())
                })
        })
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn stock_scene_ops_from_message(ops: &[SceneOp], now_ms: u64) -> StockSceneOpsEvent {
    let operations = ops
        .iter()
        .filter_map(|op| {
            let reference = op
                .reference
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())?;
            match op.kind.as_str() {
                "set" if op.field.eq_ignore_ascii_case("pose") => Some(StockSceneOperation {
                    reference,
                    kind: STOCK_SCENE_OP_POSE,
                    value: scene_json_u32(op.args.first())?,
                    extra: scene_json_u32(op.args.get(1)).unwrap_or(0),
                }),
                "set" if op.field.eq_ignore_ascii_case("frame") => Some(StockSceneOperation {
                    reference,
                    kind: STOCK_SCENE_OP_FRAME,
                    value: scene_json_u32(op.args.first())?,
                    extra: 0,
                }),
                "set" if op.field.eq_ignore_ascii_case("attribute") => {
                    let attribute = op.args.first()?.as_str()?.to_ascii_lowercase();
                    let kind = match attribute.as_str() {
                        "collision" => STOCK_SCENE_OP_COLLISION,
                        "visible" => STOCK_SCENE_OP_VISIBLE,
                        "damage" => STOCK_SCENE_OP_DAMAGE,
                        "active" => STOCK_SCENE_OP_ACTIVE,
                        "interactive" => STOCK_SCENE_OP_INTERACTIVE,
                        _ => return None,
                    };
                    Some(StockSceneOperation {
                        reference,
                        kind,
                        value: scene_json_u32(op.args.get(1))?,
                        extra: 0,
                    })
                }
                "effect" => Some(StockSceneOperation {
                    reference,
                    kind: STOCK_SCENE_OP_EFFECT,
                    value: stock_rule3_effect_id(&op.field)?,
                    extra: scene_json_u32(op.args.first()).unwrap_or(0),
                }),
                _ => None,
            }
        })
        .collect();
    StockSceneOpsEvent {
        timestamp_ms: now_ms as u32,
        operations,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn scene_json_u32(value: Option<&serde_json::Value>) -> Option<u32> {
    match value? {
        serde_json::Value::Number(number) => number
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .or_else(|| {
                number
                    .as_f64()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .map(|value| value.trunc() as u32)
            }),
        serde_json::Value::Bool(value) => Some(u32::from(*value)),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn scene_ops_from_stock(event: &StockSceneOpsEvent) -> Vec<SceneOp> {
    event
        .operations
        .iter()
        .filter_map(|operation| {
            let reference = serde_json::Value::from(operation.reference);
            let number = |value: u32| serde_json::Value::from(value);
            let attribute = |name: &str| SceneOp {
                kind: "set".into(),
                reference: reference.clone(),
                field: "attribute".into(),
                args: vec![
                    serde_json::Value::String(name.into()),
                    number(operation.value),
                ],
            };
            match operation.kind {
                STOCK_SCENE_OP_POSE => Some(SceneOp {
                    kind: "set".into(),
                    reference,
                    field: "pose".into(),
                    args: vec![number(operation.value), number(operation.extra)],
                }),
                STOCK_SCENE_OP_FRAME => Some(SceneOp {
                    kind: "set".into(),
                    reference,
                    field: "frame".into(),
                    args: vec![number(operation.value)],
                }),
                STOCK_SCENE_OP_COLLISION => Some(attribute("collision")),
                STOCK_SCENE_OP_VISIBLE => Some(attribute("visible")),
                STOCK_SCENE_OP_DAMAGE => Some(attribute("damage")),
                STOCK_SCENE_OP_ACTIVE => Some(attribute("active")),
                STOCK_SCENE_OP_INTERACTIVE => Some(attribute("interactive")),
                STOCK_SCENE_OP_EFFECT => Some(SceneOp {
                    kind: "effect".into(),
                    reference,
                    field: stock_rule3_effect_name(operation.value)?.into(),
                    args: vec![number(operation.extra)],
                }),
                _ => None,
            }
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn run_stock_client_loop(
    transport: StockClientTransport,
    mut hosted_server: Option<StockGameServer>,
    hello: Message,
    camp: u32,
    to_thread: Receiver<NetOutbound>,
    to_app: Sender<NetInbound>,
) {
    let Message::Hello {
        character_id,
        weapon,
        ..
    } = hello
    else {
        let _ = to_app.send(NetInbound::Control(Message::Error {
            code: "stock_hello".into(),
            text: "原版 UDP 连接缺少角色身份".into(),
        }));
        return;
    };
    let camp = std::env::var("METEOR_STOCK_CAMP")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|camp| *camp <= 2)
        .unwrap_or(camp.min(2));
    let mut stage_checksum_rejected = false;
    let mut client = match stock_asset_client(transport, u32::from(character_id), camp, weapon) {
        Ok(client) => client,
        Err(error) => {
            let _ = to_app.send(NetInbound::Control(Message::Error {
                code: "stock_profile".into(),
                text: format!("无法创建原版角色身份: {error}"),
            }));
            return;
        }
    };
    let mut npc_bridge = stock_npc_bridge::Bridge::default();
    let mut scene_frame = hosted_server
        .as_ref()
        .and_then(StockGameServer::scene_frame);
    let mut anger_revision = 0;
    let mut role_pacer = meteor_network::stock_game::StockRoleSendPacer::default();
    let started = std::time::Instant::now();
    let mut last_latency_report = 0;
    loop {
        let now_ms = started.elapsed().as_millis() as u64;
        loop {
            let message = match to_thread.try_recv() {
                Ok(message) => message,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = client.transport_mut().disconnect(0);
                    return;
                }
            };
            match message {
                NetOutbound::StockRoleState(state) => {
                    role_pacer.submit(state);
                }
                NetOutbound::StockSelfEffect(actor, id) => {
                    let result = if let Some(actor) = actor {
                        npc_bridge.send_self_effect(actor, id, now_ms)
                    } else {
                        client.send_self_effect(id, now_ms)
                    };
                    if let Err(error) = result {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "stock_self_effect".into(),
                            text: error.to_string(),
                        }));
                    }
                }
                NetOutbound::StockPoseChain(request) => {
                    if let Err(error) = client.send_pose_chain(request, now_ms) {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "stock_pose_chain".into(),
                            text: format!("无法发送原版连招动作: {error}"),
                        }));
                    }
                }
                NetOutbound::Input(input) | NetOutbound::OrderedInput(input) => {
                    if !input.role_state.is_empty() {
                        match decode_request1_state(&input.role_state) {
                            Ok(state) => {
                                let state =
                                    scene_frame.map_or(state, |frame| frame.to_stock(state));
                                if let Err(error) = client.send_role_state(state, now_ms) {
                                    let _ = to_app.send(NetInbound::Control(Message::Error {
                                        code: "stock_send".into(),
                                        text: format!("无法发送原版角色状态: {error}"),
                                    }));
                                }
                            }
                            Err(error) => {
                                let _ = to_app.send(NetInbound::Control(Message::Error {
                                    code: "stock_role_state".into(),
                                    text: format!("无法编码原版角色状态: {error}"),
                                }));
                            }
                        }
                    }
                    if input.actions.iter().any(|action| action == "cure") {
                        if let Err(error) = client.send_cure(now_ms) {
                            let _ = to_app.send(NetInbound::Control(Message::Error {
                                code: "stock_cure".into(),
                                text: format!("无法发送原版救治请求: {error}"),
                            }));
                        }
                    }
                }
                NetOutbound::StockHit(hit) => {
                    if let Err(error) = client.send_hit(hit, now_ms) {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "stock_hit".into(),
                            text: format!("无法发送原版命中请求: {error}"),
                        }));
                    }
                }
                NetOutbound::StockNpcRoster(roster) => {
                    if let Some(server) = hosted_server.as_ref() {
                        if let Err(error) = npc_bridge.sync(server, roster, now_ms) {
                            let _ = to_app.send(NetInbound::Control(Message::Error {
                                code: "stock_npc".into(),
                                text: format!("NPC 接入房间失败: {error}"),
                            }));
                        }
                    }
                }
                NetOutbound::StockNpcHit(entity, hit) => {
                    if let Err(error) = npc_bridge.send_hit(entity, hit, now_ms) {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "stock_npc_hit".into(),
                            text: format!("NPC 命中请求失败: {error}"),
                        }));
                    }
                }
                NetOutbound::StockAngerSpend {
                    actor,
                    key,
                    spend,
                    revision,
                } => {
                    let result = if let Some(entity) = actor {
                        npc_bridge.send_anger_spend(entity, key, spend, revision, now_ms)
                    } else {
                        client.send_anger_spend(key, spend, now_ms).map(|()| {
                            anger_revision = revision;
                        })
                    };
                    if let Err(error) = result {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "stock_anger".into(),
                            text: format!("原版耗气请求失败: {error}"),
                        }));
                    }
                }
                NetOutbound::StockEnvironmentGeometry(geometry) => {
                    if let Some(server) = hosted_server.as_mut() {
                        server.replace_environment_geometry(geometry);
                    }
                }
                NetOutbound::StockSceneManifest(items) => {
                    if let Some(server) = hosted_server.as_mut() {
                        if let Err(error) = server.replace_authored_scene_items(items, now_ms) {
                            let _ = to_app.send(NetInbound::Control(Message::Error {
                                code: "stock_scene_manifest".into(),
                                text: format!("无法注册原版场景道具: {error}"),
                            }));
                        }
                    }
                }
                NetOutbound::StockWeaponDropRest(id, position) => {
                    if let Err(error) = client.confirm_weapon_drop_collision(
                        id,
                        RoleVec3 {
                            x: position[0],
                            y: position[1],
                            z: position[2],
                        },
                        now_ms,
                    ) {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "stock_weapon_drop_rest".into(),
                            text: error.to_string(),
                        }));
                    }
                }
                NetOutbound::StockWeaponDrop(command) => {
                    match client.send_weapon_drop(command, now_ms) {
                        Ok(Some(event)) => {
                            let _ = to_app.send(NetInbound::StockGame(event));
                        }
                        Ok(None) => {}
                        Err(error) => {
                            let _ = to_app.send(NetInbound::Control(Message::Error {
                                code: "stock_weapon_drop".into(),
                                text: format!("无法发送原版丢兵请求: {error}"),
                            }));
                        }
                    }
                }
                NetOutbound::StockWeaponSwitch(item_id) => {
                    if let Err(error) = client.send_weapon_switch(item_id, now_ms) {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "stock_weapon_switch".into(),
                            text: format!("无法发送原版换兵请求: {error}"),
                        }));
                    }
                }
                NetOutbound::Control(Message::RoomLeave) | NetOutbound::Quit => {
                    let _ = client.transport_mut().disconnect(0);
                    let _ = to_app.send(NetInbound::Closed {
                        code: 1000,
                        reason: "已退出原版 UDP 对战".into(),
                    });
                    return;
                }
                NetOutbound::Control(Message::SceneOps { ops, .. }) => {
                    let event = stock_scene_ops_from_message(&ops, now_ms);
                    if let Err(error) = client.send_scene_ops(&event, now_ms) {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "stock_scene_ops".into(),
                            text: format!("无法发送原版场景事务: {error}"),
                        }));
                    }
                }
                NetOutbound::Control(Message::ObjectiveEnd { winner_team }) => {
                    if let Some(server) = hosted_server.as_mut() {
                        if let Err(error) = server.complete_siege_objective(winner_team, now_ms) {
                            let _ = to_app.send(NetInbound::Control(Message::Error {
                                code: "stock_objective".into(),
                                text: format!("无法结算原版护城目标: {error}"),
                            }));
                        }
                    }
                }
                NetOutbound::Control(_) => {}
            }
        }
        if let Some(state) =
            role_pacer.take_due(started.elapsed().as_micros() as u64, client.role_rate())
        {
            let state = scene_frame.map_or(state, |frame| frame.to_stock(state));
            if let Err(error) = client.send_role_state(state, now_ms) {
                let _ = to_app.send(NetInbound::Control(Message::Error {
                    code: "stock_send".into(),
                    text: format!("无法发送原版角色状态: {error}"),
                }));
            }
        }
        if let Some(server) = hosted_server.as_mut() {
            if let Err(error) = server.tick(now_ms) {
                let _ = to_app.send(NetInbound::Closed {
                    code: 1006,
                    reason: format!("原版 UDP 房主运行中断: {error}"),
                });
                return;
            }
        }

        if let Err(error) = npc_bridge.tick(now_ms, &to_app) {
            let _ = to_app.send(NetInbound::Control(Message::Error {
                code: "stock_npc_tick".into(),
                text: format!("NPC 连接异常: {error}"),
            }));
        }
        match client.tick(now_ms) {
            Ok(events) => {
                for mut event in events {
                    match &mut event {
                        StockGameEvent::StageConfig(config) => {
                            scene_frame =
                                Some(stock_scene_frame(&config.stage_id.to_ascii_lowercase()));
                        }
                        StockGameEvent::RoleStates(records) => {
                            if let Some(frame) = scene_frame {
                                for record in records {
                                    record.state = frame.to_scene(record.state);
                                }
                            }
                        }
                        _ => {}
                    }
                    if let StockGameEvent::Vitals(vitals) = event {
                        let _ = to_app.send(NetInbound::StockVitals {
                            actor: None,
                            vitals,
                            spend_revision: anger_revision,
                        });
                        continue;
                    }
                    if let StockGameEvent::StageChecksumRejected(rejected) = &event {
                        if client.transport_id() == Some(rejected.actor_id) {
                            stage_checksum_rejected = true;
                        }
                    }
                    let disconnect_reason = match &event {
                        StockGameEvent::PeerDisconnected { .. } if stage_checksum_rejected => {
                            Some("地图 PST 资源与房主不一致，请使用相同版本的资源包")
                        }
                        StockGameEvent::PeerDisconnected { reason, .. }
                            if *reason == meteor_network::stock_session::STOCK_REJECT_PASSWORD =>
                        {
                            Some("房间密码错误，请重试并重新输入密码")
                        }
                        StockGameEvent::PeerDisconnected { .. } => Some("原版 UDP 房主已断开"),
                        _ => None,
                    };
                    let _ = to_app.send(NetInbound::StockGame(event));
                    if let Some(reason) = disconnect_reason {
                        let _ = to_app.send(NetInbound::Closed {
                            code: 1006,
                            reason: reason.into(),
                        });
                        return;
                    }
                }
            }
            Err(error) => {
                let _ = to_app.send(NetInbound::Closed {
                    code: 1006,
                    reason: format!("原版 UDP 连接中断: {error}"),
                });
                return;
            }
        }
        if now_ms.saturating_sub(last_latency_report) >= 1000 {
            last_latency_report = now_ms;
            let _ = to_app.send(NetInbound::StockLatency(
                client
                    .transport()
                    .measured_round_trip_ms(now_ms)
                    .map(f64::from),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run_connection(
    address: String,
    hello: Message,
    stock: StockConnectionConfig,
    wire_format: NetWireFormat,
    to_thread: Receiver<NetOutbound>,
    to_app: Sender<NetInbound>,
) {
    if let Some(bind) = address.strip_prefix("udp-host://") {
        run_stock_host_connection(bind, hello, stock, to_thread, to_app);
        return;
    }
    if let Some(stock_address) = address.strip_prefix("udp://") {
        run_stock_connection_with_password(
            stock_address,
            hello,
            stock.camp,
            &stock.password,
            to_thread,
            to_app,
        );
        return;
    }
    use futures_util::{SinkExt, StreamExt};
    fn tungstenite_frame(frame: EncodedNetFrame) -> tokio_tungstenite::tungstenite::Message {
        match frame {
            EncodedNetFrame::Text(text) => {
                tokio_tungstenite::tungstenite::Message::Text(text.into())
            }
            EncodedNetFrame::Binary(bytes) => {
                tokio_tungstenite::tungstenite::Message::Binary(bytes.into())
            }
        }
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            let _ = to_app.send(NetInbound::Control(Message::Error {
                code: "runtime".into(),
                text: format!("无法创建网络线程: {e}"),
            }));
            return;
        }
    };
    runtime.block_on(async move {
        let connected = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio_tungstenite::connect_async(&address),
        ).await;
        let ws = match connected {
            Ok(Ok((ws, _))) => ws,
            result => {
                let e = match result {
                    Ok(Err(error)) => error.to_string(),
                    Err(_) => "连接超时".to_owned(),
                    Ok(Ok(_)) => unreachable!(),
                };
                let _ = to_app.send(NetInbound::Control(Message::Error {
                    code: "connect".into(),
                    text: format!("无法连接服务器 {address}: {e}"),
                }));
                return;
            }
        };
        let (mut sink, mut stream) = ws.split();
        let mut p2p = NativeWebP2p::new();

        let hello_frame = match encode_control_frame(wire_format, hello) {
            Ok(frame) => tungstenite_frame(frame),
            Err(error) => {
                let _ = to_app.send(NetInbound::Control(Message::Error {
                    code: "v4_encode".into(),
                    text: format!("无法编码 hello: {error}"),
                }));
                return;
            }
        };
        if sink.send(hello_frame).await.is_err() {
            return;
        }

        loop {
            while let Ok(message) = to_thread.try_recv() {
                let encoded = match message {
                    NetOutbound::Control(message) => {
                        encode_control_frame(wire_format, message).map(Some)
                    }
                    NetOutbound::Input(input) if p2p.is_active_peer() => {
                        match p2p.send_input(input).await {
                            Ok(Some(relay)) => Ok(Some(EncodedNetFrame::Binary(relay))),
                            Ok(None) => continue,
                            Err(error) => {
                                let _ = to_app.send(NetInbound::Control(Message::Error {
                                    code: "p2p_send".into(),
                                    text: error,
                                }));
                                continue;
                            }
                        }
                    }
                    NetOutbound::Input(input) | NetOutbound::OrderedInput(input) => encode_input_frame(wire_format, input),
                    NetOutbound::StockRoleState(_)
                    | NetOutbound::StockPoseChain(_)
                    | NetOutbound::StockSelfEffect(_, _)
                    | NetOutbound::StockHit(_)
                    | NetOutbound::StockNpcRoster(_)
                    | NetOutbound::StockNpcHit(_, _)
                    | NetOutbound::StockAngerSpend { .. }
                    | NetOutbound::StockEnvironmentGeometry(_)
                    | NetOutbound::StockSceneManifest(_)
                    | NetOutbound::StockWeaponDropRest(_, _)
                    | NetOutbound::StockWeaponDrop(_)
                    | NetOutbound::StockWeaponSwitch(_) => continue,
                    NetOutbound::Quit => {
                        p2p.teardown().await;
                        return;
                    }
                };
                let frame = match encoded {
                    Ok(Some(frame)) => tungstenite_frame(frame),
                    Ok(None) => continue,
                    Err(error) => {
                        let _ = to_app.send(NetInbound::Control(Message::Error {
                            code: "v4_encode".into(),
                            text: format!("无法编码 WebSocket 消息: {error}"),
                        }));
                        return;
                    }
                };
                if sink.send(frame).await.is_err() {
                    return;
                }
            }
            let p2p_outputs = p2p.drain_outputs().await;
            if flush_native_p2p_outputs(&mut sink, &to_app, wire_format, p2p_outputs)
                .await
                .is_err()
            {
                return;
            }
            tokio::select! {
                incoming = stream.next() => {
                    match incoming {
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => {
                            let (code, reason) = frame
                                .map(|frame| (u16::from(frame.code), frame.reason.to_string()))
                                .unwrap_or((1000, "与服务器连接已关闭".into()));
                            let _ = to_app.send(NetInbound::Closed { code, reason });
                            return;
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                            if let Some(message) = decode_text_frame(&text) {
                                let _ = to_app.send(message);
                            }
                        }
                        Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes))) => {
                            if wire_format == NetWireFormat::V4 {
                                match Message::from_v4_payload(&bytes) {
                                    Ok(V4Payload::Control(message)) => {
                                        if let Message::Welcome { player_id, .. } = &message {
                                            p2p.set_player_id(player_id.clone());
                                        }
                                        match p2p.handle_control(&message).await {
                                            Ok(outputs) => {
                                                if flush_native_p2p_outputs(
                                                    &mut sink,
                                                    &to_app,
                                                    wire_format,
                                                    outputs,
                                                )
                                                .await
                                                .is_err()
                                                {
                                                    return;
                                                }
                                            }
                                            Err(error) => {
                                                let _ = to_app.send(NetInbound::Control(Message::Error {
                                                    code: "p2p_signal".into(),
                                                    text: error,
                                                }));
                                            }
                                        }
                                        let _ = to_app.send(NetInbound::Control(message));
                                    }
                                    Ok(V4Payload::RelayFromHost(relay)) => {
                                        if p2p.accept_relay_from_host(&relay) {
                                            match decode_binary_frame(NetWireFormat::V4, &relay.payload) {
                                                Ok(Some(message)) => {
                                                    let _ = to_app.send(message);
                                                }
                                                Ok(None) => {}
                                                Err(error) => {
                                                    let _ = to_app.send(NetInbound::Control(Message::Error {
                                                        code: "p2p_relay_decode".into(),
                                                        text: format!("无法解码 P2P 中继消息: {error}"),
                                                    }));
                                                }
                                            }
                                        }
                                    }
                                    Ok(payload) => {
                                        if let Some(message) = v4_payload_to_inbound(payload) {
                                            let _ = to_app.send(message);
                                        }
                                    }
                                    Err(error) => {
                                        let _ = to_app.send(NetInbound::Control(Message::Error {
                                            code: "v4_decode".into(),
                                            text: format!("无法解码服务端消息: {error}"),
                                        }));
                                    }
                                }
                            } else {
                                match decode_binary_frame(wire_format, &bytes) {
                                    Ok(Some(message)) => {
                                        let _ = to_app.send(message);
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        let _ = to_app.send(NetInbound::Control(Message::Error {
                                            code: "v4_decode".into(),
                                            text: format!("无法解码服务端消息: {error}"),
                                        }));
                                    }
                                }
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(e)) => {
                            let _ = to_app.send(NetInbound::Control(Message::Error {
                                code: "closed".into(),
                                text: format!("连接断开: {e}"),
                            }));
                            return;
                        }
                        None => {
                            let _ = to_app.send(NetInbound::Control(Message::Error {
                                code: "closed".into(),
                                text: "与服务器连接已关闭".into(),
                            }));
                            return;
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
            }
        }

    });
}

#[allow(clippy::too_many_arguments)]
fn net_poll(
    #[cfg(not(target_arch = "wasm32"))] mut stock_anger: ResMut<PendingStockAnger>,
    mut net: ResMut<NetState>,
    mut rx: ResMut<NetRx>,
    mut tx: ResMut<NetTx>,
    mut sequence: ResMut<NetInputSequence>,
    mut pending_actions: ResMut<PendingNetActions>,
    mut request: ResMut<LaunchRequest>,
    mut end: ResMut<MatchEnd>,
    mut next_state: ResMut<NextState<AppState>>,
    current: Res<State<AppState>>,
    time: Res<Time>,
    real_time: Option<Res<Time<bevy::time::Real>>>,
) {
    net.latency.clock += real_time
        .as_ref()
        .map_or(time.delta_secs_f64(), |t| t.delta_secs_f64());
    if !net.transport_connected {
        net.latency.pending = None;
        net.latency.sample = None;
        net.latency.received.clear();
    } else if !net.is_stock_udp()
        && net.latency.clock - net.latency.last_sent >= 2.0
        && net
            .latency
            .pending
            .is_none_or(|at| net.latency.clock - at >= 10.0)
    {
        let now = net.latency.clock;
        net.latency.last_sent = now;
        net.latency.pending = Some(now);
        tx.send_control(Message::Ping {
            client_time: Some(now),
        });
    }
    let mut closed = false;
    let mut close_code = None;
    if let Some(receiver) = rx.0.as_mut() {
        let receiver = receiver.get_mut().unwrap();
        loop {
            match receiver.try_recv() {
                #[cfg(not(target_arch = "wasm32"))]
                Ok(NetInbound::StockLatency(value)) => {
                    net.latency.sample = value.map(|ms| (ms, net.latency.clock));
                }
                Ok(NetInbound::Control(Message::Pong {
                    client_time: Some(token),
                    ..
                })) => {
                    if net.latency.receive(token) && !net.room_id.is_empty() {
                        tx.send_control(Message::RoomLatency {
                            room_id: net.room_id.clone(),
                            player_id: net.player_id.clone(),
                            latency: meteor_network::protocol::RoomLatencyWire {
                                server_rtt: net.latency.value(),
                                host_rtt: None,
                                p2p_state: "server".into(),
                            },
                        });
                    }
                }
                Ok(NetInbound::Control(message)) => handle_net_message(
                    &mut net,
                    &mut request,
                    &mut end,
                    &mut next_state,
                    message,
                    &mut tx,
                    &mut sequence,
                    &mut pending_actions,
                    *current.get(),
                ),
                Ok(NetInbound::Snapshot(snapshot)) => handle_v4_snapshot(&mut net, snapshot),
                Ok(NetInbound::RoleSnapshot(snapshot)) => {
                    handle_v4_role_snapshot(&mut net, snapshot)
                }
                #[cfg(not(target_arch = "wasm32"))]
                Ok(NetInbound::StockVitals {
                    actor,
                    mut vitals,
                    spend_revision,
                }) => {
                    stock_anger.project(actor, spend_revision, &mut vitals);
                    if let Some(entity) = actor {
                        stock_npc_bridge::apply_event(
                            &mut net,
                            entity,
                            StockGameEvent::Vitals(vitals),
                        );
                    } else {
                        handle_stock_game_event(
                            &mut net,
                            &mut request,
                            &mut end,
                            &mut tx,
                            &mut next_state,
                            *current.get(),
                            StockGameEvent::Vitals(vitals),
                        );
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                Ok(NetInbound::StockNpcEvent(entity, event)) => {
                    stock_npc_bridge::apply_event(&mut net, entity, event);
                }
                #[cfg(not(target_arch = "wasm32"))]
                Ok(NetInbound::StockGame(event)) => {
                    match &event {
                        StockGameEvent::RoundRestart => {
                            for state in stock_anger.0.values_mut() {
                                state.pending.clear();
                                state.keys.clear();
                            }
                        }
                        StockGameEvent::LocalRespawn => {
                            if let Some(state) = stock_anger.0.get_mut(&None) {
                                state.pending.clear();
                                state.keys.clear();
                            }
                        }
                        StockGameEvent::Respawn(respawn) => {
                            let actor = if net.player_id == format!("stock-{}", respawn.actor_id) {
                                Some(None)
                            } else {
                                net.stock_npc_actors
                                    .iter()
                                    .find(|(_, id)| **id == respawn.actor_id)
                                    .map(|(entity, _)| Some(*entity))
                            };
                            if let Some(state) =
                                actor.and_then(|actor| stock_anger.0.get_mut(&actor))
                            {
                                state.pending.clear();
                                state.keys.clear();
                            }
                        }
                        _ => {}
                    }
                    handle_stock_game_event(
                        &mut net,
                        &mut request,
                        &mut end,
                        &mut tx,
                        &mut next_state,
                        *current.get(),
                        event,
                    );
                }
                Ok(NetInbound::Closed { code, reason }) => {
                    net.error = Some(reason);
                    close_code = Some(code);
                    closed = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
    }
    if closed {
        info!(
            "net: connection closed address={} status={:?} code={close_code:?} retry={} error={:?}",
            net.address, net.status, net.reconnect_attempt, net.error
        );
        rx.0 = None;
        tx.clear();
        net.transport_connected = false;
        if net.address.starts_with("udp://") || net.address.starts_with("udp-host://") {
            let error = net
                .error
                .take()
                .unwrap_or_else(|| "局域网连接已断开".into());
            hard_disconnect(&mut net, &mut next_state, current.get());
            net.error = Some(error);
            net.reconnect_at = None;
            net.intentional_disconnect = true;
            net.session_token.clear();
            net.resume_requested = false;
            sequence.0 = 0;
            pending_actions.take();
            #[cfg(not(target_arch = "wasm32"))]
            stock_anger.0.clear();
            request.stock_password.clear();
            if *current.get() != AppState::Menu {
                next_state.set(AppState::Menu);
            }
        } else if close_code == Some(4000) {
            net.intentional_disconnect = true;
            net.session_token.clear();
            net.resume_requested = false;
            sequence.0 = 0;
            pending_actions.take();
            hard_disconnect(&mut net, &mut next_state, current.get());
            net.error = Some("当前会话已在另一个客户端恢复，本端停止重连".into());
        } else if close_code == Some(1002) {
            net.intentional_disconnect = true;
            hard_disconnect(&mut net, &mut next_state, current.get());
            net.error = Some("协议版本不匹配，请刷新或同步客户端与服务端版本".into());
        } else if net.intentional_disconnect {
            let reason = net.error.take();
            hard_disconnect(&mut net, &mut next_state, current.get());
            if let Some(reason) = reason {
                net.error = Some(reason);
            }
        } else if schedule_reconnect(&mut net, time.elapsed_secs_f64()) {
            net.error = Some("连接中断，正在保留战局并重连".into());
        } else {
            hard_disconnect(&mut net, &mut next_state, current.get());
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn handle_stock_game_event(
    net: &mut NetState,
    request: &mut LaunchRequest,
    end: &mut MatchEnd,
    net_tx: &mut NetTx,
    next_state: &mut NextState<AppState>,
    current: AppState,
    event: StockGameEvent,
) {
    let actor_key = |actor_id: u32| format!("stock-{actor_id}");
    match event {
        StockGameEvent::PlayerName { actor_id, name } => {
            if !name.trim().is_empty() {
                let player_id = actor_key(actor_id);
                net.remote_actor_names
                    .insert(player_id.clone(), name.clone());
                if let Some(member) = net
                    .members
                    .iter_mut()
                    .find(|member| member.player_id == player_id)
                {
                    member.name = name;
                }
            }
        }
        StockGameEvent::Latencies(samples) => {
            for (actor, ms) in samples {
                let id = actor_key(actor);
                let now = net.latency.clock;
                net.latency.received.insert(id.clone(), now);
                net.room_latency.insert(
                    id,
                    meteor_network::protocol::RoomLatencyWire {
                        server_rtt: None,
                        host_rtt: Some(f64::from(ms)),
                        p2p_state: "udp".into(),
                    },
                );
            }
        }
        StockGameEvent::RemoteVitals(sample) => {
            let player_id = actor_key(sample.actor_id);
            if player_id != net.player_id
                && net.remote_actor_profiles.contains_key(&player_id)
                && !net
                    .stock_npc_actors
                    .values()
                    .any(|actor| *actor == sample.actor_id)
            {
                net.pending_remote_vitals.insert(
                    player_id,
                    [
                        f32::from(sample.vitals.health),
                        f32::from(sample.vitals.max_health),
                        f32::from(sample.vitals.anger),
                    ],
                );
            }
        }
        StockGameEvent::TransportConnected {
            peer: _,
            transport_id,
        } => {
            info!("net: stock transport connected id={transport_id}");
            net_tx.activate();
            net.transport_connected = true;
            net.player_id = actor_key(transport_id);
            net.client_id = net.player_id.clone();
            net.error = None;
        }
        StockGameEvent::RoleEntered(entered) => {
            let player_id = actor_key(entered.actor_id);
            let is_local = player_id == net.player_id;
            let name = net
                .remote_actor_names
                .get(&player_id)
                .cloned()
                .unwrap_or_else(|| {
                    if is_local {
                        request.player_name.clone()
                    } else {
                        format!("原版玩家 {}", entered.actor_id)
                    }
                });
            let profile = (entered.hero.min(u32::from(u8::MAX)) as u8, 0);
            net.remote_actor_profiles.insert(player_id.clone(), profile);
            net.remote_actor_teams
                .insert(player_id.clone(), entered.camp as i32);
            net.remote_actor_names
                .insert(player_id.clone(), name.clone());
            if !is_local {
                info!(
                    "net: stock remote role entered actor={} hero={} camp={} checksum={:#x}",
                    entered.actor_id, entered.hero, entered.camp, entered.hero_checksum
                );

                net.pending_role_entries.retain(|id| id != &player_id);
                net.pending_role_entries.push(player_id.clone());
            }
            if let Some(member) = net
                .members
                .iter_mut()
                .find(|member| member.player_id == player_id)
            {
                member.name = name;
                member.character_id = profile.0;
                member.team = (request.stock_rule != 1).then_some(entered.camp);
            } else {
                net.members.push(RoomMemberWire {
                    connected: true,
                    player_id: player_id.clone(),
                    name,
                    character_id: profile.0,
                    weapon: 0,
                    team: (request.stock_rule != 1).then_some(entered.camp),
                    ready: true,
                });
            }
            if is_local {
                info!(
                    "net: stock role entered actor={} hero={} stage={} current={current:?}",
                    entered.actor_id, entered.hero, request.stage_id
                );
                request.mode = stock_mode_from_rule(request.stock_rule).into();
                request.spawn_room_npcs = false;
                net.status = NetStatus::InRoom;
                net.room_id = "stock-udp".into();
                net.mode = request.mode.clone();
                net.stage = request.stage_id.clone();
                net.round_active = true;

                if matches!(current, AppState::Menu | AppState::NetRoom) {
                    next_state.set(AppState::Loading);
                }
            }
        }
        StockGameEvent::RoleLeft(left) => {
            let player_id = actor_key(left.actor_id);
            if player_id != net.player_id
                && !net
                    .pending_remote_despawns
                    .iter()
                    .any(|id| id == &player_id)
            {
                net.pending_remote_despawns.push(player_id.clone());
            }
            net.members.retain(|member| member.player_id != player_id);
            net.radar_star_players.remove(&player_id);
            net.stock_deaths.remove(&player_id);
            for vip in &mut net.stock_vips {
                if vip.as_deref() == Some(player_id.as_str()) {
                    *vip = None;
                }
            }
        }
        StockGameEvent::StageChecksumRejected(rejected) => {
            if actor_key(rejected.actor_id) == net.player_id {
                net.error = Some("地图 PST 资源与房主不一致，请使用相同版本的资源包".into());
            }
        }
        StockGameEvent::StageConfig(config) => {
            let stage_id = config.stage_id.to_ascii_lowercase();
            info!(
                "net: stock stage config stage={stage_id} fields={}/{}/{} local={}",
                config.field_a, config.field_b, config.field_c, request.stage_id
            );
            if meteor_scene::stage_registry::is_official_stage_id(&stage_id)
                && request.stage_id != stage_id
            {
                request.stage_name = meteor_scene::stage_registry::official_stages()
                    .into_iter()
                    .find(|stage| stage.id == stage_id)
                    .map(|stage| stage.name)
                    .unwrap_or_else(|| stage_id.to_uppercase());
                request.stage_id.clone_from(&stage_id);
                net.stage = stage_id;
                if request.stock_rule == 2 {
                    if let Ok(escort) = super::escort_assets::load_escort(&request.stage_id) {
                        net.escort_definition = Some(escort.clone());
                    }
                }
            }
        }
        StockGameEvent::RoleRate(hz) => apply_session_role_rate(net, hz),
        StockGameEvent::WeaponEquipped(equipped) => {
            let player_id = actor_key(equipped.actor_id);
            if let Some((weapon, variant)) = stock_weapon_from_item_id(equipped.weapon_item_id) {
                net.stock_equipped_weapons
                    .insert(player_id.clone(), (weapon, variant));
                if let Some(member) = net
                    .members
                    .iter_mut()
                    .find(|member| member.player_id == player_id)
                {
                    member.weapon = weapon;
                }
                if let Some(profile) = net.remote_actor_profiles.get_mut(&player_id) {
                    profile.1 = weapon;
                }
            }
        }
        StockGameEvent::PoseChain(event) => {
            let player_id = actor_key(u32::from(event.actor_id));
            if player_id != net.player_id
                && !net
                    .stock_npc_actors
                    .values()
                    .any(|id| *id == u32::from(event.actor_id))
            {
                let tick = net.last_role_snapshot_tick.unwrap_or(0).wrapping_add(1);
                net.last_role_snapshot_tick = Some(tick);
                net.pending_pose_chains
                    .push((player_id, event.into(), tick));
            }
        }
        StockGameEvent::RoleStates(records) => {
            let tick = net.last_role_snapshot_tick.unwrap_or(0).wrapping_add(1);
            net.last_role_snapshot_tick = Some(tick);
            for StockRoleStateRecord { actor_id, state } in records {
                let player_id = actor_key(u32::from(actor_id));
                if player_id == net.player_id
                    || net
                        .stock_npc_actors
                        .values()
                        .any(|id| *id == u32::from(actor_id))
                {
                    continue;
                }
                if let Some(existing) = net
                    .pending_role_states
                    .iter_mut()
                    .find(|(id, _, _)| id == &player_id)
                {
                    *existing = (player_id, state, tick);
                } else {
                    net.pending_role_states.push((player_id, state, tick));
                }
            }
        }
        StockGameEvent::HitReaction { guarded, event } => {
            let target_id = actor_key(event.target_actor_id);
            let motion = event.motion_scale();
            let minimum = event.minimum_seconds();
            let reaction = if guarded {
                CombatReactionWire {
                    defense_move: motion,
                    defense_value: minimum,
                    target_pose: Some((event.pose_id >> 16) as u16),
                    ..Default::default()
                }
            } else {
                CombatReactionWire {
                    target_move: motion,
                    target_value: minimum,
                    target_pose: Some(event.pose_id as u16),
                    ..Default::default()
                }
            };
            let hit = AuthorityHit {
                stock_guard_bucket: guarded.then(|| stock_guard_bucket(event.pose_id)),
                damage: 0.0,
                health: None,
                anger: None,
                guarded,
                attacker_id: None,
                effect_strategy: u8::try_from(event.context)
                    .ok()
                    .filter(|strategy| *strategy < 12),
                motion_scale: Some(motion),
                reaction: Some(reaction),
                tick: net.last_role_snapshot_tick,
                impact: true,
                terminal: false,
            };
            if target_id == net.player_id {
                net.pending_hits.push(hit);
            } else if net
                .stock_npc_actors
                .values()
                .any(|id| actor_key(*id) == target_id)
            {
            } else {
                net.pending_remote_hits.push((target_id, hit));
            }
        }
        StockGameEvent::EnvironmentDeath(actor_id) => {
            let target = actor_key(actor_id);
            let deaths = net.stock_deaths.entry(target.clone()).or_default();
            *deaths = deaths.saturating_add(1);
            let hit = AuthorityHit {
                stock_guard_bucket: None,
                damage: 0.0,
                health: Some(0.0),
                anger: None,
                guarded: false,
                attacker_id: None,
                effect_strategy: None,
                motion_scale: None,
                reaction: None,
                tick: None,
                impact: false,
                terminal: true,
            };
            if target == net.player_id {
                net.pending_hits.push(hit);
            } else if let Some(entity) = net
                .stock_npc_actors
                .iter()
                .find_map(|(entity, id)| (*id == actor_id).then_some(*entity))
            {
                net.pending_npc_hits.push((entity, hit));
            } else {
                net.pending_remote_hits.push((target, hit));
            }
        }
        StockGameEvent::Kill(event) => {
            net.pending_stock_kills.push((
                actor_key(event.attacker_actor_id),
                actor_key(event.target_actor_id),
            ));
        }
        StockGameEvent::TeamScore(score) => {
            net.stock_team_wins = [score.team1_wins, score.team2_wins];
            if !net.round_active && net.battle_result.is_none() {
                net.stock_round_start_wins = net.stock_team_wins;
            }
        }
        StockGameEvent::Vip(vip) => {
            if let Some(index) = vip
                .camp
                .checked_sub(1)
                .and_then(|camp| usize::try_from(camp).ok())
                .filter(|index| *index < net.stock_vips.len())
            {
                if let Some(previous) = net.stock_vips[index].take() {
                    net.radar_star_players.remove(&previous);
                }
                let player_id = actor_key(vip.actor_id);
                net.radar_star_players.insert(player_id.clone());
                net.stock_vips[index] = Some(player_id);
            }
        }
        StockGameEvent::EscortGrounded { record, dropped } => {
            if let Some(previous) = net
                .escort_flag
                .as_ref()
                .and_then(|flag| flag.carrier_id.clone())
            {
                net.radar_star_players.remove(&previous);
            }
            let position = record.role_position();
            net.escort_flag = Some(EscortFlagSnapshot {
                position: [position.x, position.y, position.z],
                carrier_id: None,
                state: if dropped { "dropped" } else { "home" }.into(),
                return_in: dropped.then_some(SN03.return_seconds),
            });
        }
        StockGameEvent::EscortTaken(taken) => {
            if let Some(previous) = net
                .escort_flag
                .as_ref()
                .and_then(|flag| flag.carrier_id.clone())
            {
                net.radar_star_players.remove(&previous);
            }
            let player_id = actor_key(taken.actor_id);
            let position = net
                .escort_flag
                .as_ref()
                .map_or(stock_escort_home(net), |flag| flag.position);
            net.radar_star_players.insert(player_id.clone());
            net.escort_flag = Some(EscortFlagSnapshot {
                position,
                carrier_id: Some(player_id),
                state: "carried".into(),
                return_in: None,
            });
        }
        StockGameEvent::EscortReturned => {
            if let Some(previous) = net
                .escort_flag
                .as_ref()
                .and_then(|flag| flag.carrier_id.clone())
            {
                net.radar_star_players.remove(&previous);
            }
            net.escort_flag = Some(EscortFlagSnapshot {
                position: stock_escort_home(net),
                carrier_id: None,
                state: "home".into(),
                return_in: None,
            });
        }
        StockGameEvent::SceneItemsGrounded { records, spawned } => {
            net.pending_stock_scene_grounded
                .extend(records.into_iter().map(|record| {
                    let position = record.role_position();
                    StockSceneItemGrounded {
                        record_id: record.record_id,
                        category: record.category,
                        item_id: record.item_id,
                        position: [position.x, position.y, position.z],
                        spawned,
                    }
                }));
            if net.pending_stock_scene_grounded.len() > 512 {
                let excess = net.pending_stock_scene_grounded.len() - 512;
                net.pending_stock_scene_grounded.drain(..excess);
            }
        }
        StockGameEvent::PropertyEffect(event) => {
            if !net
                .stock_npc_actors
                .values()
                .any(|id| *id == event.actor_id)
            {
                net.pending_status_effects.push((None, event));
            }
        }
        StockGameEvent::SceneItemTaken { event, record } => {
            let position = record.map(|record| {
                let position = record.role_position();
                [position.x, position.y, position.z]
            });
            net.pending_stock_scene_items.push(StockSceneItemPickup {
                actor_id: event.actor_id,
                record_id: event.record_id,
                category: event.category,
                item_id: event.item_id,
                position,
            });
            if net.pending_stock_scene_items.len() > 128 {
                let excess = net.pending_stock_scene_items.len() - 128;
                net.pending_stock_scene_items.drain(..excess);
            }
        }
        StockGameEvent::WeaponThrown(thrown) => {
            if let Some((weapon, variant)) = stock_weapon_from_item_id(thrown.item_id) {
                if let Some(model) =
                    crate::plugins::character_plugin::weapon_model_stem(weapon, variant)
                {
                    let reference = format!("D_wpStockThrow{}", thrown.throw_id);
                    let position = [
                        thrown.position.x,
                        thrown.position.y + 6.0,
                        thrown.position.z,
                    ];
                    let velocity = [
                        thrown.forward.x * 12.0,
                        thrown.forward.y * 12.0,
                        thrown.forward.z * 12.0,
                    ];
                    let drop = AuthorityWeaponDrop {
                        reference: reference.clone(),
                        model,
                        position,
                        velocity: Some(velocity),
                        settled: false,
                        pickup_ready: false,
                    };
                    net.dynamic_weapon_drops.insert(reference, drop.clone());
                    if net.pending_weapon_drops.len() >= 64 {
                        net.pending_weapon_drops.remove(0);
                    }
                    net.pending_weapon_drops.push(drop);
                }
            }
        }
        StockGameEvent::WeaponSettled { throw_id, .. } => {
            net.dynamic_weapon_drops
                .remove(&format!("D_wpStockThrow{throw_id}"));
        }
        StockGameEvent::SceneOps(event) => {
            if !net.is_stock_host() || net.mode != "siege" {
                net.pending_scene_ops.extend(scene_ops_from_stock(&event));
            }
        }
        StockGameEvent::RoundEnd => {
            net.round_active = false;
            net.battle_result = Some(stock_battle_result(net, false));
        }
        StockGameEvent::RoundRestart => {
            net.battle_result = None;
            net.stock_round_number = net.stock_round_number.saturating_add(1);
            net.stock_round_start_wins = net.stock_team_wins;
            net.round_active = true;
            net.pending_hits.clear();
            net.pending_remote_hits.clear();
            net.pending_npc_hits.clear();
            net.pending_scene_ops.clear();
            net.pending_scene_prop_hits.clear();
            net.pending_scene_transfers.clear();
            net.applied_scene_transfers.clear();
            net.pending_stock_scene_items.clear();
            net.pending_status_effects.clear();
            net.pending_stock_scene_grounded.clear();
            net.dynamic_weapon_drops.clear();
            net.pending_weapon_drops.clear();
            net.stock_vips = [None, None];
            net.radar_star_players.clear();
            net.escort_flag = (request.stock_rule == 2).then(|| EscortFlagSnapshot {
                position: stock_escort_home(net),
                carrier_id: None,
                state: "home".into(),
                return_in: None,
            });
            if !net.player_id.is_empty() {
                net.pending_respawns.push(net.player_id.clone());
                net.pending_spawn_relocations.insert(net.player_id.clone());
            }
            net.pending_respawns.extend(
                net.stock_npc_actors
                    .values()
                    .map(|id| format!("stock-{id}")),
            );
            net.pending_spawn_relocations
                .extend(net.pending_respawns.iter().cloned());
            *end = MatchEnd::default();

            if current == AppState::Playing && request.stock_rule == 3 {
                next_state.set(AppState::Loading);
            }
        }
        StockGameEvent::MatchEnd => {
            net.round_active = false;
            let result = stock_battle_result(net, true);
            end.result = super::battle_result::local_result_bits(net, request, &result);
            net.battle_result = Some(result);
            end.settled = true;
            end.settle_elapsed = 0.0;
        }
        StockGameEvent::LocalDeath => {
            net.pending_hits.push(AuthorityHit {
                stock_guard_bucket: None,
                damage: 0.0,
                health: Some(0.0),
                anger: None,
                guarded: false,
                attacker_id: None,
                effect_strategy: None,
                motion_scale: None,
                reaction: None,
                tick: net.last_role_snapshot_tick,
                impact: false,
                terminal: true,
            });
        }
        StockGameEvent::Vitals(vitals) => {
            net.stock_room_max_health = f32::from(vitals.max_health).max(1.0);
            net.pending_stock_vitals = Some([
                f32::from(vitals.health),
                f32::from(vitals.max_health),
                f32::from(vitals.anger),
            ]);
        }
        StockGameEvent::LocalRespawn => {
            if !net.player_id.is_empty() {
                net.pending_respawns.push(net.player_id.clone());
                net.pending_spawn_relocations.insert(net.player_id.clone());
            }
        }
        StockGameEvent::Respawn(respawn) => {
            let player_id = actor_key(respawn.actor_id);

            if request.stock_rule == 4
                && (player_id == net.player_id
                    || net
                        .stock_npc_actors
                        .values()
                        .any(|id| *id == respawn.actor_id))
            {
                net.pending_respawns.push(player_id);
            }
        }
        StockGameEvent::PeerDisconnected { peer: _, reason } => {
            net.transport_connected = false;
            if reason != 0x80 || net.error.is_none() {
                net.error = Some(format!("原版 UDP 对端已断开（reason={reason}）"));
            }
        }
        StockGameEvent::RawApplication {
            peer: _,
            opcode: _,
            payload: _,
        } => {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn stock_escort_home(net: &NetState) -> [f32; 3] {
    let home = net
        .escort_definition
        .as_ref()
        .map_or(SN03.flag_home, |d| d.stage.flag_home);
    [home.x, home.y, home.z]
}

fn stock_mode_from_rule(rule: i32) -> &'static str {
    match rule {
        1 => "leader",
        2 => "escort",
        3 => "siege",
        4 => "assassin",
        5 => "deathmatch",
        _ => "deathmatch",
    }
}

fn reconnect_delay_seconds(attempt: u8) -> Option<f64> {
    (attempt < 6).then(|| (0.4 * 2_f64.powi(i32::from(attempt))).min(3.0))
}

fn schedule_reconnect(net: &mut NetState, now: f64) -> bool {
    if net.address.starts_with("udp://") || net.address.starts_with("udp-host://") {
        net.reconnect_at = None;
        return false;
    }
    let Some(delay) = reconnect_delay_seconds(net.reconnect_attempt) else {
        net.reconnect_at = None;
        return false;
    };
    net.reconnect_attempt += 1;
    net.reconnect_at = Some(now + delay);
    true
}

fn hard_disconnect(net: &mut NetState, next_state: &mut NextState<AppState>, current: &AppState) {
    net.late_join_team = None;
    net.battle_pending = false;
    net.status = NetStatus::Disconnected;
    net.transport_connected = false;
    net.friendly_fire = false;
    net.match_epoch = None;
    net.round_epoch = None;
    net.last_full_snapshot_tick = None;
    net.last_role_snapshot_tick = None;
    net.pending_stock_vitals = None;
    net.pending_remote_vitals.clear();
    net.pending_stock_kills.clear();
    net.pending_stock_scene_items.clear();
    net.pending_status_effects.clear();
    net.pending_stock_scene_grounded.clear();
    net.stock_deaths.clear();
    net.stock_team_wins = [0, 0];
    net.stock_vips = [None, None];
    net.radar_star_players.clear();
    net.round_active = false;
    net.pending_pose_chains.clear();
    net.remote_chain_ticks.clear();
    net.error = Some("自动重连已停止，请重新进入联机大厅".into());
    if matches!(current, AppState::NetRoom | AppState::Playing) {
        next_state.set(AppState::Menu);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_net_message(
    net: &mut NetState,
    request: &mut LaunchRequest,
    end: &mut MatchEnd,
    next_state: &mut NextState<AppState>,
    message: Message,
    net_tx: &mut NetTx,
    sequence: &mut NetInputSequence,
    pending_actions: &mut PendingNetActions,
    current_state: AppState,
) {
    match message {
        Message::Welcome {
            player_id,
            room_id,
            members,
            client_id,
            session_token,
            resumed,
        } => {
            let resume_rejected = !resumed
                && net.resume_requested
                && (!net.room_id.is_empty() || net.match_epoch.is_some());
            net_tx.activate();
            net.transport_connected = true;
            net.reconnect_attempt = 0;
            net.reconnect_at = None;
            net.client_id = client_id;
            net.session_token = session_token;
            if !player_id.is_empty() {
                net.player_id = player_id;
            }
            if !room_id.is_empty() {
                net.room_id = room_id;
            }
            if !members.is_empty() || !resumed {
                net.members = members;
            }
            if !resumed {
                sequence.0 = 0;
                pending_actions.take();
            }
            if resume_rejected {
                net_tx.disconnect();
                net_tx.clear();
                net.transport_connected = false;
                net.intentional_disconnect = true;
                net.session_token.clear();
                net.status = NetStatus::Disconnected;
                net.error = Some("原房间会话已失效，请从列表重新选择房间".into());
                net.room_id.clear();
                net.match_epoch = None;
                net.round_epoch = None;
                net.round_active = false;
                net.pending_scene_ops.clear();
                net.pending_scene_prop_hits.clear();
                net.pending_scene_transfers.clear();
                net.applied_scene_transfers.clear();
                net.pending_role_states.clear();
                net.pending_pose_chains.clear();
                net.remote_chain_ticks.clear();
                net.pending_role_entries.clear();
                net.pending_hits.clear();
                net.pending_remote_hits.clear();
                net.remote_pose_sequences.clear();
                net.remote_pose_ids.clear();
                net.last_full_snapshot_tick = None;
                net.last_role_snapshot_tick = None;
                if current_state == AppState::Playing {
                    next_state.set(AppState::Menu);
                }
            }
            net.resume_requested = false;

            if !resumed {
                if let Some(join) = net.pending_web_join.take() {
                    let select_team = matches!(join, Message::RoomCreate { .. })
                        && (cfg!(target_arch = "wasm32") || request.public_battle)
                        && request.stock_rule != 1
                        && matches!(request.stock_camp, 1 | 2);
                    net_tx.send_control(join);
                    if select_team {
                        net_tx.send_control(Message::TeamSelect {
                            team: request.stock_camp,
                        });
                    }
                }
            }
            #[cfg(target_arch = "wasm32")]
            persist_browser_session(net, sequence.0);
        }
        Message::RoomList { .. } => {}
        Message::RoomJoined {
            room_id,
            player_id,
            match_epoch,
            round_epoch,
            room,
            scene_state,
            resumed,
            last_processed_input_seq,
            ..
        } => {
            let resumes_same_input_epoch = resumed
                && net.room_id == room_id
                && match_epoch.is_some()
                && net.match_epoch == match_epoch;
            sequence.0 = if resumes_same_input_epoch {
                sequence.0.max(last_processed_input_seq)
            } else {
                pending_actions.take();
                last_processed_input_seq
            };
            if net.room_id != room_id {
                net.last_full_snapshot_tick = None;
                net.last_role_snapshot_tick = None;
            }
            net.room_id = room_id;
            net.player_id = player_id;
            net.match_epoch = match_epoch.or_else(|| {
                room.get("matchEpoch")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
            });
            net.round_epoch = round_epoch.or_else(|| {
                room.get("roundEpoch")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
            });
            net.friendly_fire = room
                .get("rules")
                .and_then(|rules| rules.get("friendlyFire"))
                .or_else(|| room.get("friendlyFire"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            net.battle_pending = room
                .get("players")
                .and_then(|v| v.as_array())
                .and_then(|players| {
                    players.iter().find(|p| {
                        p.get("id").and_then(|v| v.as_str()) == Some(net.player_id.as_str())
                    })
                })
                .and_then(|p| p.get("battlePending"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            net.lobby = serde_json::from_value(room.clone()).unwrap_or_default();
            net.pending_scene_ops = scene_state;
            #[cfg(target_arch = "wasm32")]
            persist_browser_session(net, sequence.0);
        }
        Message::RoomLatency {
            room_id,
            player_id,
            latency,
        } => {
            if !net.is_stock_udp()
                && room_id == net.room_id
                && net
                    .members
                    .iter()
                    .any(|member| member.player_id == player_id)
            {
                let now = net.latency.clock;
                net.latency.received.insert(player_id.clone(), now);
                net.room_latency.insert(player_id, latency);
            }
        }
        Message::RoomPlayerLeft { player_id } => {
            if player_id != net.player_id && !net.pending_remote_despawns.contains(&player_id) {
                net.pending_remote_despawns.push(player_id.clone());
            }
            net.members.retain(|member| member.player_id != player_id);
            net.room_latency.remove(&player_id);
            net.radar_star_players.remove(&player_id);
            net.stock_deaths.remove(&player_id);

            net.pending_snapshots.retain(|(id, _, _)| id != &player_id);
            net.pending_role_states
                .retain(|(id, _, _)| id != &player_id);
            net.pending_pose_chains
                .retain(|(id, _, _)| id != &player_id);
        }
        Message::RoomState {
            room_id,
            match_epoch,
            round_epoch,
            mode,
            stage,
            members,
            started,
            rules,
            mut lobby,
        } => {
            if net.room_id != room_id {
                net.room_latency.clear();
            }
            net.room_id = room_id;
            net.match_epoch = match_epoch.or(net.match_epoch);
            net.round_epoch = round_epoch.or(net.round_epoch);
            net.mode = mode;
            net.stage = stage;
            net.members = members;
            net.room_latency
                .retain(|id, _| net.members.iter().any(|member| &member.player_id == id));
            if lobby.max_players.is_none() {
                lobby.max_players = rules.max_players.map(|n| n as usize);
            }
            if lobby.state.is_none() {
                lobby.state = Some(if started { "running" } else { "lobby" }.into());
            }
            if lobby.asset_loading == Some(false) {
                net.asset_prepare = None;
            }
            net.lobby = lobby;
            net.error = net.asset_error.clone();
            if let Some(me) = net.members.iter().find(|m| m.player_id == net.player_id) {
                request.character_id = me.character_id;
                request.weapon = me.weapon;
            }
            if let Some(friendly_fire) = rules.friendly_fire {
                net.friendly_fire = friendly_fire;
            }
            if net.status == NetStatus::Connecting {
                net.status = NetStatus::InRoom;
                net.error = None;
                next_state.set(AppState::NetRoom);
            }
        }
        Message::MatchAssetPrepare {
            room_id,
            attempt_id,
            players,
            purpose,
            match_epoch,
            round_epoch,
            ..
        } => {
            let current_epoch = if purpose == "match-start" {
                !net.round_active && round_epoch == 0 && match_epoch > net.match_epoch.unwrap_or(0)
            } else {
                (net.round_active || net.lobby.state.as_deref() == Some("running"))
                    && Some(match_epoch) == net.match_epoch
                    && Some(round_epoch) == net.round_epoch
            };
            if current_epoch
                && !net.is_stock_udp()
                && net.room_id == room_id
                && !attempt_id.is_empty()
                && matches!(purpose.as_str(), "match-start" | "late-join")
                && players.iter().any(|p| p.player_id == net.player_id)
            {
                net.asset_error = None;
                net.error = None;
                net.asset_prepare = Some(asset_barrier::Request {
                    room_id,
                    attempt_id,
                    players,
                });
            }
        }
        Message::MatchStart {
            match_epoch,
            round_epoch,
            mode,
            stage,
            players,
            resumed,
            late_joined,
            ..
        } => {
            net.battle_pending = false;
            net.late_join_team = None;
            if !resumed && !late_joined {
                net.battle_result = None;
                net.remote_kills.clear();
                net.stock_deaths.clear();
                net.stock_round_number = 1;
                net.stock_team_wins = [0, 0];
                sequence.0 = 0;
                pending_actions.take();
            }

            request.mode = mode;
            net.match_epoch = match_epoch.or(net.match_epoch);
            net.round_epoch = round_epoch.or(net.round_epoch);
            request.stage_id = stage.clone();
            request.stage_name = stage;

            let roster = if players.is_empty() {
                &net.members
            } else {
                &players
            };
            if let Some(me) = roster.iter().find(|m| m.player_id == net.player_id) {
                request.player_name.clone_from(&me.name);
                request.character_id = me.character_id;
                request.weapon = me.weapon;
            }
            if !players.is_empty() {
                net.members = players;
            }
            net.round_active = true;
            net.role_uplink_hz = ROLE_UPLINK_HZ;
            net.pending_objective_end = None;
            let in_place_resume = resumed && current_state == AppState::Playing;
            if !in_place_resume {
                if !resumed && !late_joined {
                    net.pending_scene_ops.clear();
                    net.pending_scene_prop_hits.clear();
                    net.pending_scene_transfers.clear();
                    net.applied_scene_transfers.clear();
                }
                net.pending_role_states.clear();
                net.pending_pose_chains.clear();
                net.remote_chain_ticks.clear();
                net.pending_role_entries.clear();
                net.pending_hits.clear();
                net.pending_remote_hits.clear();
                net.remote_pose_sequences.clear();
                net.remote_pose_ids.clear();
                net.remote_actor_profiles.clear();
                net.stock_equipped_weapons.clear();
                net.remote_actor_teams.clear();
                net.remote_actor_names.clear();
                net.network_npc_ids.clear();
                net.pending_remote_despawns.clear();
                net.last_full_snapshot_tick = None;
                net.last_role_snapshot_tick = None;
                net.pending_item_claims.clear();
                net.pending_item_states.clear();
                net.pending_weapon_states.clear();
                net.pending_item_effect_states.clear();
                net.pending_status_snapshots.clear();
                net.dynamic_weapon_drops.clear();
                net.pending_weapon_drops.clear();
                next_state.set(AppState::Loading);
            }
            #[cfg(target_arch = "wasm32")]
            persist_browser_session(net, sequence.0);
        }
        Message::RoundStart {
            match_epoch,
            round_epoch,
            ..
        } => {
            let restart = net.round_epoch.is_some()
                && round_epoch.is_some_and(|epoch| Some(epoch) != net.round_epoch);
            if restart && !net.battle_pending && !net.player_id.is_empty() {
                if !net.pending_respawns.contains(&net.player_id) {
                    net.pending_respawns.push(net.player_id.clone());
                }
                net.pending_spawn_relocations.insert(net.player_id.clone());
            }
            net.battle_result = None;
            *end = MatchEnd::default();
            if match_epoch.is_some_and(|epoch| Some(epoch) != net.match_epoch)
                || round_epoch.is_some_and(|epoch| Some(epoch) != net.round_epoch)
            {
                net.last_full_snapshot_tick = None;
                net.last_role_snapshot_tick = None;
                net.pending_role_states.clear();
                net.pending_pose_chains.clear();
                net.remote_chain_ticks.clear();
                net.pending_role_entries.clear();
                net.pending_hits.clear();
                net.pending_remote_hits.clear();
                net.remote_pose_sequences.clear();
                net.remote_pose_ids.clear();
            }
            net.match_epoch = match_epoch.or(net.match_epoch);
            net.round_epoch = round_epoch.or(net.round_epoch);
            net.round_active = !net.battle_pending;
            net.escort_flag = None;
            net.pending_objective_end = None;
        }
        Message::RoundEnd {
            match_epoch,
            round_epoch,
            round,
            winner_team,
            mut winner_teams,
            winner_player_id,
            reason,
            team_wins,
        } => {
            if let Some(team) = winner_team {
                if winner_teams.is_empty() {
                    winner_teams.push(team);
                }
            }
            if let Some(wins) = team_wins {
                net.stock_team_wins = [*wins.get("1").unwrap_or(&0), *wins.get("2").unwrap_or(&0)];
            }
            net.battle_result = Some(super::battle_result::NetworkBattleResult {
                round,
                winner_teams,
                winner_player_id,
                reason,
                ..Default::default()
            });
            net.match_epoch = match_epoch.or(net.match_epoch);
            net.round_epoch = round_epoch.or(net.round_epoch);
            net.round_active = false;
            net.pending_pose_chains.clear();

            net.escort_flag = None;
            net.pending_objective_end = None;
        }
        Message::SceneTransfer {
            room_id,
            match_epoch,
            round_epoch,
            serial,
            tick,
            player_id,
            position,
            yaw,
        } => {
            queue_scene_transfer(
                net,
                net_tx,
                SceneTransfer {
                    room_id,
                    match_epoch,
                    round_epoch,
                    serial,
                    tick,
                    player_id,
                    position,
                    yaw,
                },
            );
        }
        Message::RoleRate { rate } => {
            apply_session_role_rate(net, rate);
        }
        Message::RolePoseChain {
            room_id,
            player_id,
            match_epoch,
            round_epoch,
            tick,
            chain,
            ..
        } => {
            if net.status == NetStatus::InRoom
                && net.round_active
                && room_id == net.room_id
                && player_id != net.player_id
                && Some(match_epoch) == net.match_epoch
                && Some(round_epoch) == net.round_epoch
                && chain.is_valid()
                && net
                    .members
                    .iter()
                    .any(|member| member.player_id == player_id)
                && net
                    .remote_chain_ticks
                    .get(&player_id)
                    .map_or(true, |previous| tick > *previous)
            {
                net.remote_chain_ticks.insert(player_id.clone(), tick);
                net.pending_pose_chains.push((player_id, chain, tick));
                if net.pending_pose_chains.len() > 1024 {
                    net.pending_pose_chains.remove(0);
                }
            }
        }
        Message::ScenePropHit {
            room_id,
            match_epoch,
            round_epoch,
            player_id,
            reference,
            pose_id,
            frame,
            damage,
        } => {
            if net.status == NetStatus::InRoom
                && net.round_active
                && !net.is_stock_udp()
                && net.is_scene_authority()
                && room_id == net.room_id
                && Some(match_epoch) == net.match_epoch
                && Some(round_epoch) == net.round_epoch
                && damage > 0
                && net
                    .members
                    .iter()
                    .any(|member| member.player_id == player_id)
            {
                net.pending_scene_prop_hits.push(Message::ScenePropHit {
                    room_id,
                    match_epoch,
                    round_epoch,
                    player_id,
                    reference,
                    pose_id,
                    frame,
                    damage,
                });
                if net.pending_scene_prop_hits.len() > 1024 {
                    net.pending_scene_prop_hits.remove(0);
                }
            }
        }
        Message::SceneOps { room_id, ops, .. } if room_id.is_empty() || room_id == net.room_id => {
            net.pending_scene_ops.extend(ops);
            if net.pending_scene_ops.len() > 1024 {
                let excess = net.pending_scene_ops.len() - 1024;
                net.pending_scene_ops.drain(..excess);
            }
        }
        message @ Message::ItemClaimed { .. } => {
            let reference = match &message {
                Message::ItemClaimed { reference, .. } => reference,
                _ => unreachable!("matched item.claimed above"),
            };
            net.dynamic_weapon_drops.remove(reference.as_str());
            net.pending_item_claims.push(message);
            if net.pending_item_claims.len() > 128 {
                let excess = net.pending_item_claims.len() - 128;
                net.pending_item_claims.drain(..excess);
            }
        }
        Message::ItemEffect {
            player_id,
            health,
            max_health,
            anger,
            item_effects,
        } => {
            queue_authority_item_effect_state(
                net,
                AuthorityItemEffectState {
                    player_id,
                    health,
                    max_health,
                    anger,
                    item_effects,
                },
            );
        }
        Message::WeaponSwitched {
            player_id,
            weapon,
            weapon_model,
            weapon_slot,
            weapon_slots,
        } => queue_authority_weapon_state(
            net,
            AuthorityWeaponState {
                player_id,
                weapon,
                weapon_model,
                weapon_slot,
                weapon_slots,
            },
        ),
        Message::WeaponDropped {
            player_id,
            reference,
            model,
            position,
            velocity,
            weapon,
            weapon_model,
            weapon_slot,
            weapon_slots,
        } => {
            queue_authority_weapon_state(
                net,
                AuthorityWeaponState {
                    player_id,
                    weapon,
                    weapon_model,
                    weapon_slot,
                    weapon_slots,
                },
            );
            let drop = AuthorityWeaponDrop {
                reference: reference.clone(),
                model,
                position,
                velocity: Some(velocity),
                settled: false,
                pickup_ready: false,
            };
            net.dynamic_weapon_drops.insert(reference, drop.clone());
            if net.pending_weapon_drops.len() >= 64 {
                net.pending_weapon_drops.remove(0);
            }
            net.pending_weapon_drops.push(drop);
        }
        Message::WeaponDropRest {
            match_epoch,
            round_epoch,
            reference,
            position,
        } => {
            if !authority_epoch_matches(net.match_epoch, match_epoch)
                || !authority_epoch_matches(net.round_epoch, round_epoch)
                || !position.iter().all(|value| value.is_finite())
            {
                return;
            }
            if let Some(drop) = net.dynamic_weapon_drops.get_mut(&reference) {
                drop.position = position;
                drop.velocity = None;
                drop.settled = true;
                drop.pickup_ready = true;

                let settled = drop.clone();
                if net.pending_weapon_drops.len() >= 64 {
                    net.pending_weapon_drops.remove(0);
                }
                net.pending_weapon_drops.push(settled);
            }
        }
        Message::Snapshot {
            player_id: Some(remote_id),
            fighter,
        } => {
            if remote_id != net.player_id {
                net.pending_snapshots.push((remote_id, fighter, None));
            }
        }
        Message::Snapshot {
            player_id: None, ..
        } => {}
        Message::CombatHit {
            target,
            damage,
            health,
            anger,
            guarded,
            attacker_id,
            motion_scale,
            reaction,
            tick,
        } => {
            let hit = AuthorityHit {
                stock_guard_bucket: None,
                damage,
                health,
                anger,
                guarded: guarded.unwrap_or(false),
                attacker_id,
                effect_strategy: None,
                motion_scale,
                reaction,
                tick,
                impact: true,
                terminal: false,
            };
            if target == net.player_id {
                net.pending_hits.push(hit);
            } else {
                net.pending_remote_hits.push((target, hit));
            }
        }
        Message::CombatDeath {
            target_id,
            attacker_id,
            tick,
            ..
        } => {
            let hit = AuthorityHit {
                stock_guard_bucket: None,
                damage: 0.0,
                health: Some(0.0),
                anger: None,
                guarded: false,
                attacker_id,
                effect_strategy: None,
                motion_scale: None,
                reaction: None,
                tick,
                impact: false,
                terminal: true,
            };
            if target_id == net.player_id {
                net.pending_hits.push(hit);
            } else {
                net.pending_remote_hits.push((target_id, hit));
            }
        }
        Message::CombatRespawn {
            player_id, reason, ..
        } if player_id == net.player_id => {
            if reason != "cure" {
                net.pending_spawn_relocations.insert(player_id.clone());
            }
            net.pending_respawns.push(player_id);
        }
        Message::MatchEnd {
            winner_team,
            winner_player_id,
            result,
            reason,
            ..
        } => {
            net.round_active = false;
            net.pending_pose_chains.clear();

            let presentation = super::battle_result::NetworkBattleResult {
                round: net.battle_result.as_ref().map_or(1, |r| r.round),
                terminal: true,
                winner_teams: if result.as_deref() == Some("draw") {
                    Vec::new()
                } else {
                    winner_team.into_iter().collect()
                },
                winner_player_id,
                reason: if result.as_deref() == Some("aborted") {
                    "aborted".into()
                } else {
                    reason
                },
                ..Default::default()
            };
            end.result = super::battle_result::local_result_bits(net, request, &presentation);
            net.battle_result = Some(presentation);
            end.settled = true;
            end.settle_elapsed = 0.0;
            info!("net: match end received (winner_team={winner_team:?})");
        }
        Message::MatchReset => {
            net.battle_result = None;
            net.remote_kills.clear();
            net.stock_deaths.clear();
            net.battle_pending = false;
            net.round_active = false;
            net.pending_pose_chains.clear();
            net.remote_chain_ticks.clear();
            net.pending_hits.clear();
            net.pending_remote_hits.clear();
            net.escort_flag = None;
            net.pending_objective_end = None;
            net.pending_scene_ops.clear();
            net.pending_scene_prop_hits.clear();
            net.pending_scene_transfers.clear();
            net.applied_scene_transfers.clear();
            net.pending_item_claims.clear();
            net.pending_item_states.clear();
            net.pending_weapon_states.clear();
            net.pending_item_effect_states.clear();
            net.pending_status_snapshots.clear();
            net.dynamic_weapon_drops.clear();
            net.pending_weapon_drops.clear();
            *end = MatchEnd::default();
            next_state.set(AppState::NetRoom);
        }
        Message::ChatMessage {
            player_id,
            name,
            text,
        } => {
            let name = if name.is_empty() {
                player_id.unwrap_or_else(|| "?".into())
            } else {
                name
            };
            net.chat_messages.push_back((name, text));
            if net.chat_messages.len() > 20 {
                net.chat_messages.pop_front();
            }
        }
        Message::Error { code, text } => {
            warn!("net: {code}: {text}");
            if net.status == NetStatus::Connecting && !net.is_stock_udp() {
                net_tx.disconnect();
                net_tx.clear();
                net.transport_connected = false;
                net.intentional_disconnect = true;
                net.reconnect_at = None;
                net.session_token.clear();
                net.pending_web_join = None;
                net.status = NetStatus::Disconnected;
            }
            if current_state == AppState::NetRoom {
                if let Some(me) = net.local_room_member() {
                    request.character_id = me.character_id;
                    request.weapon = me.weapon;
                }
            }
            if code == "ASSET_LOAD_FAILED" {
                if net.asset_error.is_none() {
                    net.asset_error = Some(text.clone());
                }
                net.error = net.asset_error.clone();
            } else {
                net.error = Some(text);
            }
        }
        _ => {}
    }
}

fn apply_session_role_rate(net: &mut NetState, rate: u32) {
    if (8..=20).contains(&rate) {
        net.role_uplink_hz = rate;
    }
}

fn handle_v4_snapshot(net: &mut NetState, snapshot: V4Snapshot) {
    if !net.room_id.is_empty() && snapshot.room_id != net.room_id {
        return;
    }
    if !authority_epoch_matches(net.match_epoch, snapshot.match_epoch)
        || !authority_epoch_matches(net.round_epoch, snapshot.round_epoch)
    {
        return;
    }
    if net
        .last_full_snapshot_tick
        .is_some_and(|last_tick| snapshot.tick <= last_tick)
    {
        return;
    }
    net.last_full_snapshot_tick = Some(snapshot.tick);
    let apply_role_state = net
        .last_role_snapshot_tick
        .is_none_or(|tick| snapshot.tick > tick);
    net.last_role_snapshot_tick = Some(
        net.last_role_snapshot_tick
            .map_or(snapshot.tick, |tick| tick.max(snapshot.tick)),
    );
    net.match_epoch = snapshot.match_epoch.or(net.match_epoch);
    net.round_epoch = snapshot.round_epoch.or(net.round_epoch);
    net.radar_star_players = snapshot
        .players
        .iter()
        .filter(|player| player.is_leader)
        .map(|player| player.id.clone())
        .collect();
    net.escort_flag = escort_flag_from_v4(snapshot.flag.as_ref());

    net.dynamic_weapon_drops = snapshot
        .drops
        .iter()
        .filter_map(authority_weapon_drop_from_v4)
        .map(|drop| (drop.reference.clone(), drop))
        .collect();
    let incoming_npc_ids = snapshot
        .npcs
        .iter()
        .map(|npc| npc.id.clone())
        .collect::<std::collections::HashSet<_>>();
    let removed_npcs = net
        .network_npc_ids
        .difference(&incoming_npc_ids)
        .cloned()
        .collect::<Vec<_>>();
    net.pending_remote_despawns.extend(removed_npcs);
    net.network_npc_ids = incoming_npc_ids;
    for player in snapshot.players {
        net.stock_deaths.insert(player.id.clone(), player.deaths);
        net.remote_kills.insert(player.id.clone(), player.kills);
        net.remote_actor_profiles.insert(
            player.id.clone(),
            (
                meteor_network::protocol::character_id_from_web_name(&player.character),
                weapon_id_from_web_name(&player.weapon),
            ),
        );
        net.remote_actor_names
            .insert(player.id.clone(), player.name.clone());
        if let Some(team) = player.team {
            net.remote_actor_teams
                .insert(player.id.clone(), team as i32);
        } else {
            net.remote_actor_teams.remove(&player.id);
        }
        queue_authority_item_state(net, authority_item_state_from_v4(&player));
        if player.id == net.player_id {
            net.remote_kills.insert(player.id, player.kills);
            net.pending_hits.push(AuthorityHit {
                stock_guard_bucket: None,
                damage: 0.0,
                health: Some(player.health),
                anger: Some(player.anger),
                guarded: false,
                attacker_id: None,
                effect_strategy: None,
                motion_scale: None,
                reaction: None,
                tick: Some(snapshot.tick),
                impact: false,
                terminal: false,
            });
            continue;
        }
        let Ok(role) = decode_event2_state(&player.role_state) else {
            continue;
        };
        if apply_role_state {
            queue_role_state(net, player.id.clone(), role, snapshot.tick);
        }
        net.pending_snapshots.push((
            player.id.clone(),
            FighterSnapshot {
                x: role.position.x,
                y: role.position.y,
                z: role.position.z,
                yaw: yaw_from_forward(role.forward),
                pose_id: role.state_words.pose_id,
                pose_frame: 0,
                pose_start: 0,
                pose_end: 1,
                pose_loop_start: 0,
                pose_loop_end: 1,
                pose_loops: true,
                hp: player.health,
                max_hp: player.max_health.unwrap_or(player.health.max(1.0)),
                weapon: weapon_id_from_web_name(&player.weapon),
                movement_mode: if role.state_words.grounded { 0 } else { 4 },
                kills: player.kills,
            },
            Some(snapshot.tick),
        ));
    }
    for npc in snapshot.npcs {
        let character_id = meteor_network::protocol::character_id_from_web_name(&npc.character);
        let weapon = weapon_id_from_web_name(&npc.weapon);
        net.remote_actor_profiles
            .insert(npc.id.clone(), (character_id, weapon));
        net.remote_actor_names
            .insert(npc.id.clone(), npc.name.clone());
        if let Some(team) = npc.team {
            net.remote_actor_teams.insert(npc.id.clone(), team as i32);
        } else {
            net.remote_actor_teams.remove(&npc.id);
        }
        if npc.vip {
            net.radar_star_players.insert(npc.id.clone());
        }
        let role = decode_event2_state(&npc.role_state).ok();
        if let Some(role) = role.filter(|_| apply_role_state) {
            queue_role_state(net, npc.id.clone(), role, snapshot.tick);
        }
        let Some((position, yaw, grounded)) = role
            .map(|role| {
                (
                    role.position,
                    yaw_from_forward(role.forward),
                    role.state_words.grounded,
                )
            })
            .or_else(|| {
                npc.position.as_ref().map(|position| {
                    (
                        RoleVec3 {
                            x: position.x,
                            y: position.y,
                            z: position.z,
                        },
                        npc.yaw,
                        true,
                    )
                })
            })
        else {
            continue;
        };
        net.remote_kills.insert(npc.id.clone(), npc.kills);
        net.stock_deaths.insert(npc.id.clone(), npc.deaths);
        net.pending_snapshots.push((
            npc.id,
            FighterSnapshot {
                x: position.x,
                y: position.y,
                z: position.z,
                yaw,
                pose_id: role.map_or(npc.pose_id as u16, |role| role.state_words.pose_id),
                pose_frame: role.map_or(npc.pose_cursor.max(0.0) as u32, |_| 0),
                pose_start: 0,
                pose_end: 1,
                pose_loop_start: 0,
                pose_loop_end: 1,
                pose_loops: true,
                hp: npc.health,
                max_hp: npc.max_health.max(1.0),
                weapon,
                movement_mode: if grounded { 0 } else { 4 },
                kills: npc.kills,
            },
            Some(snapshot.tick),
        ));
    }
}

fn handle_v4_role_snapshot(net: &mut NetState, snapshot: V4RoleSnapshot) {
    if !net.room_id.is_empty() && snapshot.room_id != net.room_id {
        return;
    }
    if !authority_epoch_matches(net.match_epoch, snapshot.match_epoch)
        || !authority_epoch_matches(net.round_epoch, snapshot.round_epoch)
    {
        return;
    }
    if net
        .last_role_snapshot_tick
        .is_some_and(|last_tick| snapshot.tick <= last_tick)
    {
        return;
    }
    net.last_role_snapshot_tick = Some(snapshot.tick);
    net.match_epoch = snapshot.match_epoch.or(net.match_epoch);
    net.round_epoch = snapshot.round_epoch.or(net.round_epoch);
    for record in snapshot.roles {
        if record.id == net.player_id {
            continue;
        }
        let Ok(state) = decode_event2_state(&record.role_state) else {
            continue;
        };
        queue_role_state(net, record.id, state, snapshot.tick);
    }
}

#[derive(Clone, Debug)]
pub struct SceneTransfer {
    room_id: String,
    match_epoch: u32,
    round_epoch: u32,
    serial: u32,
    tick: u32,
    player_id: String,
    position: [f32; 3],
    yaw: f32,
}

impl SceneTransfer {
    fn current(&self, net: &NetState) -> bool {
        net.status == NetStatus::InRoom
            && !net.is_stock_udp()
            && self.room_id == net.room_id
            && Some(self.match_epoch) == net.match_epoch
            && Some(self.round_epoch) == net.round_epoch
    }

    fn ack(&self) -> Message {
        Message::SceneTransferAck {
            room_id: self.room_id.clone(),
            match_epoch: self.match_epoch,
            round_epoch: self.round_epoch,
            serial: self.serial,
        }
    }
}

fn queue_scene_transfer(net: &mut NetState, tx: &NetTx, transfer: SceneTransfer) {
    if !transfer.current(net)
        || transfer.serial == 0
        || !transfer.yaw.is_finite()
        || !transfer.position.into_iter().all(f32::is_finite)
    {
        return;
    }
    if net
        .applied_scene_transfers
        .get(&transfer.player_id)
        .is_some_and(|last| last.current(net) && last.serial >= transfer.serial)
    {
        if transfer.player_id == net.player_id {
            tx.send_control(transfer.ack());
        }
        return;
    }
    if net
        .pending_scene_transfers
        .iter()
        .any(|p| p.player_id == transfer.player_id && p.current(net) && p.serial >= transfer.serial)
    {
        return;
    }
    net.pending_scene_transfers
        .retain(|p| p.player_id != transfer.player_id);
    net.pending_role_states
        .retain(|(id, _, tick)| id != &transfer.player_id || *tick > transfer.tick);
    net.pending_snapshots
        .retain(|(id, _, _)| id != &transfer.player_id);
    net.pending_scene_transfers.push(transfer);
}

type TransferFighters<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut MotionState,
        Option<&'static LocalFighter>,
        Option<&'static RemoteFighter>,
        Option<&'static mut FighterAir>,
        Option<&'static mut RemoteTransformTarget>,
        Option<&'static mut RemoteContactSegment>,
    ),
    Or<(With<LocalFighter>, With<RemoteFighter>)>,
>;

fn net_apply_scene_transfers(
    mut net: ResMut<NetState>,
    tx: Res<NetTx>,
    mut fighters: TransferFighters,
) {
    for transfer in std::mem::take(&mut net.pending_scene_transfers) {
        if !transfer.current(&net) {
            continue;
        }
        let mut applied = false;
        for (mut transform, mut motion, local, remote, air, target, segment) in &mut fighters {
            if !(local.is_some() && net.player_id == transfer.player_id)
                && !remote.is_some_and(|r| r.player_id == transfer.player_id)
            {
                continue;
            }

            transform.translation = Vec3::from_array(transfer.position);
            transform.rotation = Quat::from_rotation_y(transfer.yaw);
            motion.facing = transfer.yaw;
            motion.velocity = Vec3::ZERO;
            if let Some(mut air) = air {
                air.air.vertical_speed = 0.0;
                air.air.planar_x = 0.0;
                air.air.planar_z = 0.0;
                air.ground_external_velocity = Vec3::ZERO;
                air.landed_planar = Vec2::ZERO;
                air.ground_y = transfer.position[1];
            }
            if let Some(mut target) = target {
                target.translation = transform.translation;
                target.yaw = transfer.yaw;
            }
            if let Some(mut segment) = segment {
                segment.from = transform.translation;
                segment.to = transform.translation;
                segment.valid = false;
                segment.state_age = 0.0;
            }
            applied = true;
            break;
        }
        if applied {
            if transfer.player_id == net.player_id {
                tx.send_control(transfer.ack());
            }
            net.applied_scene_transfers
                .insert(transfer.player_id.clone(), transfer);
        } else {
            net.pending_scene_transfers.push(transfer);
        }
    }
}

fn queue_role_state(net: &mut NetState, id: String, state: RoleState, tick: u32) {
    if net
        .applied_scene_transfers
        .get(&id)
        .is_some_and(|t| t.current(net) && tick <= t.tick)
        || net
            .pending_scene_transfers
            .iter()
            .any(|t| t.player_id == id && t.current(net) && tick <= t.tick)
    {
        return;
    }

    if let Some(existing) = net
        .pending_role_states
        .iter_mut()
        .find(|(player_id, _, _)| player_id == &id)
    {
        if tick > existing.2 {
            *existing = (id, state, tick);
        }
    } else {
        net.pending_role_states.push((id, state, tick));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod scene_transfer_tests {
    use super::*;

    #[test]
    fn scene_transfer_applies_only_target_and_acks_after_ecs_then_ignores_replay() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            room_id: "r".into(),
            player_id: "guest".into(),
            match_epoch: Some(1),
            round_epoch: Some(2),
            ..Default::default()
        })
        .insert_resource(NetTx {
            active: Some(sender),
            connecting: None,
        })
        .add_systems(Update, net_apply_scene_transfers);
        let motion = || MotionState {
            movement_mode: meteor_core::MovementMode::Idle,
            velocity: Vec3::ONE,
            facing: 0.0,
        };
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                Transform::default(),
                motion(),
                Health {
                    current: 123.0,
                    max: 1000.0,
                },
                PoseRuntime::idle(),
                FighterAir::default(),
            ))
            .id();
        let remote = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "host".into(),
                },
                Transform::default(),
                motion(),
                RemoteTransformTarget {
                    translation: Vec3::ZERO,
                    yaw: 0.0,
                },
            ))
            .id();
        let transfer = SceneTransfer {
            room_id: "r".into(),
            match_epoch: 1,
            round_epoch: 2,
            serial: 1,
            tick: 10,
            player_id: "guest".into(),
            position: [10.0, 2.0, -30.0],
            yaw: 1.0,
        };
        app.world_mut()
            .resource_scope(|world, mut net: Mut<NetState>| {
                queue_scene_transfer(&mut net, world.resource::<NetTx>(), transfer.clone());
            });
        assert!(
            receiver.try_recv().is_err(),
            "must not acknowledge unapplied transform"
        );
        app.update();
        assert_eq!(
            app.world().get::<Transform>(local).unwrap().translation,
            Vec3::from_array(transfer.position)
        );
        assert_eq!(
            app.world().get::<MotionState>(local).unwrap().velocity,
            Vec3::ZERO
        );
        assert_eq!(app.world().get::<Health>(local).unwrap().current, 123.0);
        assert_eq!(app.world().get::<PoseRuntime>(local).unwrap().pose_id, 0);
        assert_eq!(
            app.world().get::<Transform>(remote).unwrap().translation,
            Vec3::ZERO
        );
        let NetOutbound::Control(message) = receiver.try_recv().unwrap() else {
            panic!("expected applied ack")
        };
        assert_eq!(message, transfer.ack());
        assert_eq!(
            Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap(),
            message
        );
        app.world_mut()
            .get_mut::<Transform>(local)
            .unwrap()
            .translation
            .x += 2.0;
        app.world_mut()
            .resource_scope(|world, mut net: Mut<NetState>| {
                queue_scene_transfer(&mut net, world.resource::<NetTx>(), transfer.clone());
                let mut stale = transfer.clone();
                stale.round_epoch = 1;
                stale.serial = 50;
                queue_scene_transfer(&mut net, world.resource::<NetTx>(), stale);
                let role = meteor_network::role_state::decode_request1_state(&[0; 20]).unwrap();
                queue_role_state(&mut net, "guest".into(), role, 9);
                assert!(net.pending_role_states.is_empty());
            });
        app.update();
        assert_eq!(
            app.world().get::<Transform>(local).unwrap().translation.x,
            12.0
        );
        assert!(matches!(
            receiver.try_recv(),
            Ok(NetOutbound::Control(Message::SceneTransferAck { .. }))
        ));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn scene_transfer_waits_for_actor_bootstrap_and_resets_remote_interpolation() {
        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            room_id: "r".into(),
            player_id: "host".into(),
            match_epoch: Some(1),
            round_epoch: Some(1),
            pending_scene_transfers: vec![SceneTransfer {
                room_id: "r".into(),
                match_epoch: 1,
                round_epoch: 1,
                serial: 1,
                tick: 1,
                player_id: "guest".into(),
                position: [10.0, 0.0, 20.0],
                yaw: 0.5,
            }],
            ..Default::default()
        })
        .init_resource::<NetTx>()
        .add_systems(Update, net_apply_scene_transfers);
        app.update();
        assert_eq!(
            app.world()
                .resource::<NetState>()
                .pending_scene_transfers
                .len(),
            1
        );
        let remote = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "guest".into(),
                },
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ONE,
                    facing: 0.0,
                },
                RemoteTransformTarget {
                    translation: Vec3::ZERO,
                    yaw: 0.0,
                },
            ))
            .id();
        app.update();
        let transform = app.world().get::<Transform>(remote).unwrap();
        let target = app.world().get::<RemoteTransformTarget>(remote).unwrap();
        assert_eq!(transform.translation, Vec3::new(10.0, 0.0, 20.0));
        assert_eq!(target.translation, transform.translation);
        assert_eq!(target.yaw, 0.5);
        assert!(app
            .world()
            .resource::<NetState>()
            .pending_scene_transfers
            .is_empty());
    }
}

fn authority_epoch_matches(current: Option<u32>, incoming: Option<u32>) -> bool {
    current.is_none_or(|current| incoming == Some(current))
}

fn queue_authority_item_state(net: &mut NetState, state: AuthorityItemState) {
    if let Some(existing) = net
        .pending_item_states
        .iter_mut()
        .find(|existing| existing.player_id == state.player_id)
    {
        *existing = state;
    } else {
        net.pending_item_states.push(state);
    }
}

fn queue_authority_weapon_state(net: &mut NetState, state: AuthorityWeaponState) {
    if let Some(existing) = net
        .pending_weapon_states
        .iter_mut()
        .find(|existing| existing.player_id == state.player_id)
    {
        *existing = state;
    } else {
        net.pending_weapon_states.push(state);
    }
}

fn queue_authority_item_effect_state(net: &mut NetState, state: AuthorityItemEffectState) {
    if let Some(existing) = net
        .pending_item_effect_states
        .iter_mut()
        .find(|existing| existing.player_id == state.player_id)
    {
        *existing = state;
    } else {
        net.pending_item_effect_states.push(state);
    }
}

fn authority_weapon_drop_from_v4(
    drop: &meteor_network::wire_v4::SnapshotDrop,
) -> Option<AuthorityWeaponDrop> {
    let position = drop.position.as_ref()?;
    [position.x, position.y, position.z]
        .into_iter()
        .all(f32::is_finite)
        .then(|| AuthorityWeaponDrop {
            reference: drop.r#ref.clone(),
            model: drop.model.clone(),
            position: [position.x, position.y, position.z],
            velocity: None,
            settled: drop.settled,
            pickup_ready: drop.settled,
        })
}

fn authority_item_state_from_v4(
    player: &meteor_network::wire_v4::SnapshotPlayer,
) -> AuthorityItemState {
    AuthorityItemState {
        player_id: player.id.clone(),
        health: player.health,
        max_health: player.max_health.unwrap_or(player.health.max(1.0)),
        anger: player.anger,
        weapon: player.weapon.clone(),
        weapon_model: player.weapon_model.clone(),
        weapon_slot: u8::try_from(player.weapon_slot).unwrap_or(u8::MAX),
        weapon_slots: player
            .weapon_slots
            .iter()
            .map(|slot| ItemWeaponSlotWire {
                index: u8::try_from(slot.index).unwrap_or(u8::MAX),
                present: slot.present,
                weapon: slot.weapon.clone(),
                weapon_id: slot.weapon_id.and_then(|id| u8::try_from(id).ok()),
                model: slot.model.clone(),
                resource_class: u8::try_from(slot.resource_class).unwrap_or(u8::MAX),
                variant: u8::try_from(slot.variant).unwrap_or(u8::MAX),
                attack_power: slot.attack_power,
                defense_power: slot.defense_power,
            })
            .collect(),
        item_effects: player
            .item_effects
            .iter()
            .map(|effect| ItemEffectWire {
                model: effect.model.clone(),
                remaining: effect.remaining,
            })
            .collect(),
        worn_effects: player
            .worn_effects
            .iter()
            .map(|effect| ItemEffectWire {
                model: effect.model.clone(),
                remaining: effect.remaining,
            })
            .collect(),
    }
}

fn weapon_id_from_web_name(weapon: &str) -> u8 {
    meteor_network::protocol::weapon_id_from_web_name(weapon)
}

fn web_room_join(room: Option<&RoomSummary>, request: &LaunchRequest) -> Message {
    let Some(room) = room else {
        let public_battle = cfg!(target_arch = "wasm32") || request.public_battle;
        return Message::RoomCreate {
            mode: if public_battle {
                stock_mode_from_rule(request.stock_rule)
            } else {
                "deathmatch"
            }
            .into(),
            stage: request.stage_id.clone(),
            name: request.player_name.clone(),
            character_id: request.character_id,
            weapon: request.weapon,
            room_name: request.room_name.clone(),
            password: String::new(),
            rules: if public_battle {
                meteor_network::protocol::RoomRulesWire {
                    round_time: Some(request.stock_game_time_minutes),
                    game_num: Some(request.stock_game_num),
                    life_adjust: Some(request.stock_life_adjust_percent),
                    hide_item: Some(request.stock_hide_item),
                    health_item: Some(request.stock_health_item),
                    update_item: Some(request.stock_update_item),
                    special_item: Some(request.stock_special_item),
                    transport: Some("server".into()),
                    ..Default::default()
                }
            } else {
                Default::default()
            },
        };
    };
    Message::RoomJoin {
        room_id: room.room_id.clone(),
        mode: room.mode.clone(),
        stage: room.stage_id.clone(),
        name: request.player_name.clone(),
        character_id: request.character_id,
        weapon: request.weapon,
        password: String::new(),
    }
}

type LocalSnapshotQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Fighter,
        &'static PoseRuntime,
        &'static Transform,
        &'static Health,
        &'static MotionState,
        &'static FighterAir,
        Option<&'static SmState>,
        Option<&'static crate::components::fighter::FighterHitReaction>,
    ),
    With<LocalFighter>,
>;

type LocalWeaponActionQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut Fighter,
        &'static Transform,
        &'static MotionState,
    ),
    (With<LocalFighter>, Without<RemoteFighter>),
>;

fn local_request1_runtime(
    fighter_pose: &PoseRuntime,
    transform: &Transform,
    motion: &MotionState,
    air: &FighterAir,
    guard_route: bool,
) -> RoleStateRuntime {
    RoleStateRuntime {
        position: RoleVec3 {
            x: transform.translation.x,
            y: transform.translation.y,
            z: transform.translation.z,
        },
        forward: forward_from_yaw(motion.facing),
        velocity: RoleVec3 {
            x: motion.velocity.x,
            y: motion.velocity.y,
            z: motion.velocity.z,
        },
        pose_id: fighter_pose.pose_id,
        grounded: !air.air.airborne,
        guard_route,

        playback_arg: i8::from(fighter_pose.loops),
        pose_sequence: fighter_pose.pose_instance_id as u16,
    }
}

fn local_guard_route(
    state: Option<&SmState>,
    reaction: Option<&crate::components::fighter::FighterHitReaction>,
) -> bool {
    state.is_some_and(|state| state.current == meteor_core::FighterState::Guard)
        || reaction
            .and_then(|reaction| reaction.current.as_ref())
            .is_some_and(|reaction| {
                reaction.kind == meteor_combat::hit_reaction::HitReactKind::Guard
            })
}

fn v6_action_input(
    seq: u32,
    actions: Vec<String>,
    match_epoch: Option<u32>,
    round_epoch: Option<u32>,
) -> V4Input {
    V4Input {
        seq,
        role_state: Vec::new(),
        role_rate_request: None,
        r#move: Some(V4Vec2 { x: 0.0, z: 0.0 }),
        yaw: 0.0,
        run: false,
        guard: false,
        actions,
        combat: Vec::new(),
        visual: None,
        match_epoch,
        round_epoch,
    }
}

fn v6_role_input(
    seq: u32,
    role_state: Vec<u8>,
    role_rate_request: Option<u32>,
    match_epoch: Option<u32>,
    round_epoch: Option<u32>,
) -> V4Input {
    V4Input {
        seq,
        role_state,
        role_rate_request,
        r#move: Some(V4Vec2 { x: 0.0, z: 0.0 }),
        yaw: 0.0,
        run: false,
        guard: false,
        actions: Vec::new(),
        combat: Vec::new(),
        visual: None,
        match_epoch,
        round_epoch,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn net_send_stock_weapon_switch(
    net: Res<NetState>,
    net_tx: Option<Res<NetTx>>,
    mut previous_slot: Local<Option<u8>>,
    local: Query<&Fighter, (With<LocalFighter>, Without<RemoteFighter>)>,
) {
    if net.status != NetStatus::InRoom || !net.round_active || !net.is_stock_udp() {
        *previous_slot = None;
        return;
    }
    let Ok(fighter) = local.single() else {
        return;
    };
    let slot = fighter.weapon_slot.min(1);
    let changed = previous_slot.is_some_and(|previous| previous != slot);
    *previous_slot = Some(slot);
    if !changed {
        return;
    }
    let Some(item_id) = stock_weapon_item_id(fighter.weapon, fighter.weapon_variant) else {
        return;
    };
    if let Some(tx) = net_tx {
        tx.send_stock_weapon_switch(item_id);
    }
}

fn net_send_pending_actions(
    mut commands: Commands,
    net: Res<NetState>,
    net_tx: Option<Res<NetTx>>,
    mut sequence: ResMut<NetInputSequence>,
    mut pending: ResMut<PendingNetActions>,
    mut local: LocalWeaponActionQuery<'_, '_>,
    mut dialogue: Option<ResMut<crate::plugins::ui_plugin::DialogueState>>,
) {
    #[cfg(target_arch = "wasm32")]
    let _ = (&mut commands, &mut local, &mut dialogue);
    if net.status != NetStatus::InRoom
        || !net.round_active
        || net.wire_format != NetWireFormat::V4
        || pending.actions.is_empty()
    {
        return;
    }
    let Some(tx) = net_tx else {
        return;
    };
    #[cfg(not(target_arch = "wasm32"))]
    let mut actions = pending.take();
    #[cfg(target_arch = "wasm32")]
    let actions = pending.take();
    #[cfg(not(target_arch = "wasm32"))]
    if net.is_stock_udp() {
        if let Ok((entity, mut fighter, transform, motion)) = local.single_mut() {
            let mut stock_actions = Vec::new();
            actions.retain(|action| {
                if matches!(action.as_str(), "drop_weapon" | "drop_item") {
                    stock_actions.push(action.clone());
                    false
                } else {
                    action != "switch_weapon"
                }
            });
            for action in stock_actions {
                let copy_only = action == "drop_item";
                let spare_slot = 1 - usize::from(fighter.weapon_slot.min(1));
                let (weapon, variant) = if copy_only {
                    (fighter.weapon, fighter.weapon_variant)
                } else {
                    (
                        fighter.weapon_slots[spare_slot],
                        fighter.weapon_slot_variants[spare_slot],
                    )
                };
                let Some(item_id) = stock_weapon_item_id(weapon, variant) else {
                    if let Some(dialogue) = dialogue.as_deref_mut() {
                        dialogue.text = Some(
                            if copy_only {
                                "当前没有可丢弃的手持武器"
                            } else {
                                "丢武器需要先拾取一把备用武器"
                            }
                            .into(),
                        );
                        dialogue.timer = 2.5;
                    }
                    continue;
                };
                if !copy_only
                    && crate::plugins::item_plugin::drop_spare_weapon(&mut fighter).is_none()
                {
                    if let Some(dialogue) = dialogue.as_deref_mut() {
                        dialogue.text = Some("丢武器需要先拾取一把备用武器".into());
                        dialogue.timer = 2.5;
                    }
                    continue;
                }
                let forward = meteor_network::role_state::forward_from_yaw(motion.facing);
                let remaining_item_id = (!copy_only)
                    .then(|| stock_weapon_item_id(fighter.weapon, fighter.weapon_variant))
                    .flatten();
                tx.send_stock_weapon_drop(StockWeaponDropCommand {
                    item_id,
                    position: RoleVec3 {
                        x: transform.translation.x,
                        y: transform.translation.y,
                        z: transform.translation.z,
                    },
                    forward: RoleVec3 {
                        x: forward.x,
                        y: forward.y,
                        z: forward.z,
                    },
                    copy_only,
                    remaining_item_id,
                });
                if !copy_only {
                    commands
                        .entity(entity)
                        .insert(crate::components::fighter::WeaponVariantChanged);
                }
            }
        }
    }
    if actions.is_empty() {
        return;
    }
    tx.send_input(v6_action_input(
        sequence.next(),
        actions,
        net.match_epoch,
        net.round_epoch,
    ));
}

fn net_send_pose_chains(
    mut sequence: ResMut<NetInputSequence>,
    net: Res<NetState>,
    tx: Option<Res<NetTx>>,
    mut fighters: Query<
        (&PoseRuntime, &mut crate::components::fighter::EntryBlend),
        With<LocalFighter>,
    >,
) {
    for (pose, mut blend) in &mut fighters {
        let Some((instance, pose_id, blend_seconds)) = blend.pending_local_chain.take() else {
            continue;
        };
        if pose.pose_instance_id != instance || pose.pose_id != pose_id {
            continue;
        }
        if !net.is_stock_udp() && net.status == NetStatus::InRoom && net.round_active {
            if let (Some(tx), Some(match_epoch), Some(round_epoch)) =
                (tx.as_deref(), net.match_epoch, net.round_epoch)
            {
                tx.send_control(Message::RolePoseChain {
                    seq: sequence.next(),
                    room_id: net.room_id.clone(),
                    player_id: String::new(),
                    match_epoch,
                    round_epoch,
                    tick: 0,
                    chain: meteor_network::role_state::PoseChain {
                        pose_id,
                        sequence: instance as u16,
                        minimum_seconds: 0.0,
                        blend_seconds,
                    },
                });
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        if net.is_stock_udp() && net.status == NetStatus::InRoom && net.round_active {
            if let Some(sender) = tx.as_deref().and_then(|tx| tx.active.as_ref()) {
                let _ = sender.send(NetOutbound::StockPoseChain(
                    meteor_network::stock_protocol::StockPoseChainRequest {
                        pose_id,
                        sequence: instance as u16,
                        blend_seconds,
                    },
                ));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn net_send_snapshot(
    time: Res<Time>,
    net: Res<NetState>,
    net_tx: Option<Res<NetTx>>,
    mut sequence: ResMut<NetInputSequence>,
    mut last_sent: Local<f32>,
    query: LocalSnapshotQuery<'_, '_>,
    score: Res<crate::components::fighter::KillScore>,
) {
    if net.status != NetStatus::InRoom || !net.round_active {
        return;
    }
    let Some(tx) = net_tx else {
        return;
    };
    #[cfg(not(target_arch = "wasm32"))]
    if net.is_stock_udp() {
        if let Some(sender) = &tx.active {
            for (_, pose, transform, _, motion, air, state, reaction) in &query {
                let _ = sender.send(NetOutbound::StockRoleState(role_state_from_runtime(
                    local_request1_runtime(
                        pose,
                        transform,
                        motion,
                        air,
                        local_guard_route(state, reaction),
                    ),
                )));
            }
        }
        return;
    }
    let interval = if net.wire_format == NetWireFormat::V4 {
        1.0 / net.role_uplink_hz as f32
    } else {
        SNAPSHOT_INTERVAL
    };
    if time.elapsed_secs() - *last_sent < interval {
        return;
    }
    *last_sent = time.elapsed_secs();
    for (fighter, pose, transform, health, motion, air, state, reaction) in &query {
        let yaw = motion.facing;
        if net.wire_format == NetWireFormat::V4 {
            let role_state =
                encode_request1_state(role_state_from_runtime(local_request1_runtime(
                    pose,
                    transform,
                    motion,
                    air,
                    local_guard_route(state, reaction),
                )));
            tx.send_input(v6_role_input(
                sequence.next(),
                role_state.to_vec(),
                Some(net.role_uplink_hz),
                net.match_epoch,
                net.round_epoch,
            ));
            continue;
        }
        let snapshot = FighterSnapshot {
            x: transform.translation.x,
            y: transform.translation.y,
            z: transform.translation.z,
            yaw,
            pose_id: pose.pose_id,
            pose_frame: pose.frame as u32,
            pose_start: pose.start,
            pose_end: pose.end,
            pose_loop_start: pose.loop_start,
            pose_loop_end: pose.loop_end,
            pose_loops: pose.loops,
            hp: health.current,
            max_hp: health.max,
            weapon: fighter.weapon,
            movement_mode: movement_mode_id(motion.movement_mode),
            kills: score.player_kills,
        };
        tx.send_control(Message::Snapshot {
            player_id: None,
            fighter: snapshot,
        });
    }
}

fn movement_mode_id(mode: meteor_core::MovementMode) -> u8 {
    match mode {
        meteor_core::MovementMode::Idle => 0,
        meteor_core::MovementMode::Walk => 1,
        meteor_core::MovementMode::Run => 2,
        meteor_core::MovementMode::Crouch => 3,
        meteor_core::MovementMode::Airborne => 4,
        meteor_core::MovementMode::WallRun => 5,
        meteor_core::MovementMode::Dodge => 6,
    }
}

fn movement_mode_from_id(id: u8) -> meteor_core::MovementMode {
    match id {
        1 => meteor_core::MovementMode::Walk,
        2 => meteor_core::MovementMode::Run,
        3 => meteor_core::MovementMode::Crouch,
        4 => meteor_core::MovementMode::Airborne,
        5 => meteor_core::MovementMode::WallRun,
        6 => meteor_core::MovementMode::Dodge,
        _ => meteor_core::MovementMode::Idle,
    }
}

type RemoteFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut PoseRuntime,
        &'static mut Health,
        &'static mut Fighter,
        &'static mut MotionState,
        &'static mut DeathRespawn,
        &'static mut FighterAir,
        Option<&'static mut RemoteAuthorityGeneration>,
        &'static mut FighterHitReaction,
        Option<&'static mut RemoteHitMotionWindow>,
    ),
    With<RemoteFighter>,
>;

type EscortCarrierFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static RemoteFighter>,
        Option<&'static LocalFighter>,
        Option<&'static EscortFlagCarrier>,
    ),
    With<Fighter>,
>;

fn net_apply_stock_vitals(
    mut net: ResMut<NetState>,
    mut local: Query<(&mut Health, &mut Qi), With<LocalFighter>>,
    mut npcs: Query<(&mut Health, &mut Qi), Without<LocalFighter>>,
) {
    for (entity, [health, max_health, anger]) in std::mem::take(&mut net.pending_npc_vitals) {
        if let Ok((mut hp, mut qi)) = npcs.get_mut(entity) {
            hp.max = max_health.max(1.0);
            hp.current = health.clamp(0.0, hp.max);
            qi.current = anger.clamp(0.0, qi.max);
        }
    }
    let Some([health, max_health, anger]) = net.pending_stock_vitals.take() else {
        return;
    };
    let Ok((mut local_health, mut qi)) = local.single_mut() else {
        net.pending_stock_vitals = Some([health, max_health, anger]);
        return;
    };
    local_health.max = max_health.max(1.0);
    local_health.current = health.clamp(0.0, local_health.max);
    qi.current = anger.clamp(0.0, qi.max);
}

fn net_apply_remote_vitals(
    mut net: ResMut<NetState>,
    mut remotes: Query<(
        &RemoteFighter,
        &mut Health,
        &mut Qi,
        &mut RemoteVitalsStatus,
    )>,
) {
    for (remote, mut health, mut qi, mut status) in &mut remotes {
        let Some([current, max, anger]) = net.pending_remote_vitals.remove(&remote.player_id)
        else {
            continue;
        };
        health.max = max.max(1.0);
        health.current = current.clamp(0.0, health.max);
        qi.current = anger.clamp(0.0, qi.max);
        status.health_known = true;
        status.anger_known = true;
    }
}

fn stock_battle_result(
    net: &NetState,
    terminal: bool,
) -> super::battle_result::NetworkBattleResult {
    let winner_teams = if net.mode == "leader" {
        Vec::new()
    } else if terminal {
        match net.stock_team_wins[0].cmp(&net.stock_team_wins[1]) {
            std::cmp::Ordering::Greater => vec![1],
            std::cmp::Ordering::Less => vec![2],
            _ => Vec::new(),
        }
    } else {
        (0..2)
            .filter(|&i| net.stock_team_wins[i] > net.stock_round_start_wins[i])
            .map(|i| i as i32 + 1)
            .collect()
    };
    let winner_player_id = if terminal && net.mode == "leader" {
        let mut kills = net.remote_kills.clone();

        for (attacker, _) in &net.pending_stock_kills {
            let count = kills.entry(attacker.clone()).or_default();
            *count = count.saturating_add(1);
        }
        let mut ids = net
            .members
            .iter()
            .map(|m| m.player_id.clone())
            .chain(kills.keys().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        if !net.player_id.is_empty() {
            ids.insert(net.player_id.clone());
        }
        let max = ids
            .iter()
            .map(|id| kills.get(id).copied().unwrap_or(0))
            .max()
            .unwrap_or(0);
        let leaders = ids
            .into_iter()
            .filter(|id| kills.get(id).copied().unwrap_or(0) == max)
            .collect::<Vec<_>>();
        if leaders.len() == 1 {
            leaders.into_iter().next()
        } else {
            None
        }
    } else {
        None
    };
    super::battle_result::NetworkBattleResult {
        round: net.stock_round_number,
        terminal,
        winner_teams,
        winner_player_id,
        ..Default::default()
    }
}

#[cfg(test)]
mod battle_result_tests {
    use super::*;

    #[test]
    fn stock_leader_final_kill_is_counted_before_ecs_score_application() {
        let mut net = NetState {
            mode: "leader".into(),
            player_id: "me".into(),
            remote_kills: [("me".into(), 2), ("other".into(), 2)]
                .into_iter()
                .collect(),
            pending_stock_kills: vec![("me".into(), "other".into())],
            ..Default::default()
        };
        assert_eq!(
            stock_battle_result(&net, true).winner_player_id.as_deref(),
            Some("me")
        );
        assert!(stock_battle_result(&net, false).winner_player_id.is_none());
        net.pending_stock_kills.clear();
        assert!(stock_battle_result(&net, true).winner_player_id.is_none());
        net.mode = "deathmatch".into();
        net.stock_round_start_wins = [3, 2];
        net.stock_team_wins = [3, 3];
        assert_eq!(stock_battle_result(&net, false).winner_teams, [2]);
        assert!(stock_battle_result(&net, true).winner_teams.is_empty());
    }

    #[test]
    fn round_message_opens_results_next_round_clears_and_final_uses_local_team() {
        let mut net = NetState {
            player_id: "me".into(),
            status: NetStatus::InRoom,
            remote_actor_teams: [("me".into(), 2)].into_iter().collect(),
            ..Default::default()
        };
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut next = NextState::default();
        let mut tx = NetTx::default();
        let mut sequence = NetInputSequence::default();
        let mut actions = PendingNetActions::default();
        let mut dispatch = |net: &mut NetState, end: &mut MatchEnd, message| {
            handle_net_message(
                net,
                &mut request,
                end,
                &mut next,
                message,
                &mut tx,
                &mut sequence,
                &mut actions,
                AppState::Playing,
            );
        };
        dispatch(
            &mut net,
            &mut end,
            Message::RoundEnd {
                match_epoch: Some(1),
                round_epoch: Some(3),
                round: 3,
                winner_team: Some(2),
                winner_teams: vec![2],
                winner_player_id: None,
                reason: "eliminated".into(),
                team_wins: Some([("1".into(), 1), ("2".into(), 2)].into_iter().collect()),
            },
        );
        assert!(
            !end.settled,
            "a round result must not enter terminal AppState::Result"
        );
        assert!(!net.round_active);
        assert_eq!(net.battle_result.as_ref().unwrap().round, 3);
        assert_eq!(net.stock_team_wins, [1, 2]);
        dispatch(
            &mut net,
            &mut end,
            Message::MatchEnd {
                winner_team: Some(2),
                winner_player_id: None,
                result: Some("victory".into()),
                reason: "eliminated".into(),
                lobby_return_seconds: Some(10.0),
            },
        );
        assert_eq!(end.result, 4);
        assert!(end.settled);
        dispatch(&mut net, &mut end, Message::MatchReset);
        assert!(net.battle_result.is_none());
        assert!(!end.settled);
    }
}

fn net_apply_stock_scores(
    mut net: ResMut<NetState>,
    mut score: ResMut<crate::components::fighter::KillScore>,
) {
    let events = std::mem::take(&mut net.pending_stock_kills);
    for (attacker_id, target_id) in events {
        if attacker_id == net.player_id {
            score.player_kills = score.player_kills.saturating_add(1);
        }
        let kills = net.remote_kills.entry(attacker_id).or_default();
        *kills = kills.saturating_add(1);
        let deaths = net.stock_deaths.entry(target_id).or_default();
        *deaths = deaths.saturating_add(1);
    }
}

#[allow(clippy::too_many_arguments)]
fn net_apply_snapshot(
    mut commands: Commands,
    mut net: ResMut<NetState>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    remotes: Query<(Entity, &RemoteFighter)>,

    local_spawn: Query<&DeathRespawn, (With<LocalFighter>, Without<RemoteFighter>)>,
    mut fighters: RemoteFighterQuery<'_, '_>,
    mut targets: Query<&mut RemoteTransformTarget>,
    mut contact_segments: Query<&mut RemoteContactSegment>,
    mut entry_blends: Query<&mut crate::components::fighter::EntryBlend>,
    #[cfg(not(target_arch = "wasm32"))] mut trace: Option<ResMut<super::combat_trace::CombatTrace>>,
) {
    if net.is_stock_udp() {
        let local_spawn_pos = local_spawn
            .single()
            .map(|respawn| respawn.spawn_pos)
            .unwrap_or(Vec3::new(0.0, 0.9, 0.0));
        let entry_bootstrap: Vec<_> = net
            .pending_role_entries
            .iter()
            .filter(|id| {
                !net.stock_npc_actors
                    .values()
                    .any(|actor| **id == format!("stock-{actor}"))
                    && !remotes.iter().any(|(_, remote)| &remote.player_id == *id)
                    && !net
                        .pending_snapshots
                        .iter()
                        .any(|(pending, _, _)| pending == *id)
                    && !net
                        .pending_role_states
                        .iter()
                        .any(|(pending, _, _)| pending == *id)
                    && !net.pending_remote_despawns.contains(id)
            })
            .map(|id| {
                let weapon = net
                    .remote_actor_profiles
                    .get(id)
                    .map_or(6, |profile| profile.1);
                (
                    id.clone(),
                    FighterSnapshot {
                        x: local_spawn_pos.x,
                        y: local_spawn_pos.y,
                        z: local_spawn_pos.z,
                        yaw: 0.0,
                        pose_id: 0,
                        pose_frame: 0,
                        pose_start: 0,
                        pose_end: 1,
                        pose_loop_start: 0,
                        pose_loop_end: 1,
                        pose_loops: true,
                        hp: net.stock_room_max_health,
                        max_hp: net.stock_room_max_health,
                        weapon,
                        movement_mode: 0,
                        kills: 0,
                    },
                    None,
                )
            })
            .collect();

        net.pending_role_entries.clear();
        net.pending_snapshots.extend(entry_bootstrap);
        let bootstrap: Vec<_> = net
            .pending_role_states
            .iter()
            .filter(|(id, _, _)| {
                net.remote_actor_profiles.contains_key(id)
                    && !net
                        .stock_npc_actors
                        .values()
                        .any(|actor| *id == format!("stock-{actor}"))
                    && !remotes.iter().any(|(_, remote)| remote.player_id == *id)
                    && !net
                        .pending_snapshots
                        .iter()
                        .any(|(pending, _, _)| pending == id)
                    && !net.pending_remote_despawns.contains(id)
            })
            .map(|(id, state, tick)| {
                let weapon = net
                    .remote_actor_profiles
                    .get(id)
                    .map_or(6, |profile| profile.1);
                (
                    id.clone(),
                    FighterSnapshot {
                        x: state.position.x,
                        y: state.position.y,
                        z: state.position.z,
                        yaw: yaw_from_forward(state.forward),
                        pose_id: state.state_words.pose_id,
                        pose_frame: 0,
                        pose_start: 0,
                        pose_end: 1,
                        pose_loop_start: 0,
                        pose_loop_end: 1,
                        pose_loops: true,
                        hp: net.stock_room_max_health,
                        max_hp: net.stock_room_max_health,
                        weapon,
                        movement_mode: if state.state_words.grounded { 0 } else { 4 },
                        kills: 0,
                    },
                    Some(*tick),
                )
            })
            .collect();
        net.pending_snapshots.extend(bootstrap);

        for (entity, remote) in &remotes {
            let Some(&(weapon, variant)) = net.stock_equipped_weapons.get(&remote.player_id) else {
                continue;
            };
            if let Ok((_, _, mut fighter, _, _, _, _, _, _)) = fighters.get_mut(entity) {
                if fighter.weapon != weapon || fighter.weapon_variant != variant {
                    let slot = usize::from(fighter.weapon_slot.min(1));
                    fighter.weapon = weapon;
                    fighter.weapon_variant = variant;
                    fighter.weapon_slots[slot] = weapon;
                    fighter.weapon_slot_variants[slot] = variant;
                    commands
                        .entity(entity)
                        .insert(crate::components::fighter::WeaponVariantChanged);
                }
            }
        }
    }
    let despawns = std::mem::take(&mut net.pending_remote_despawns);
    for remote_id in despawns {
        if let Some((entity, _)) = remotes
            .iter()
            .find(|(_, remote)| remote.player_id == remote_id)
        {
            commands.entity(entity).despawn();
        }
        net.remote_actor_profiles.remove(&remote_id);
        net.pending_remote_vitals.remove(&remote_id);
        net.stock_equipped_weapons.remove(&remote_id);
        net.remote_actor_teams.remove(&remote_id);
        net.remote_actor_names.remove(&remote_id);
        net.remote_pose_sequences.remove(&remote_id);
        net.remote_pose_ids.remove(&remote_id);
        net.remote_chain_ticks.remove(&remote_id);
        net.pending_pose_chains
            .retain(|(id, _, _)| id != &remote_id);
        net.remote_kills.remove(&remote_id);
        net.pending_role_states
            .retain(|(actor_id, _, _)| actor_id != &remote_id);
    }
    let snapshots = std::mem::take(&mut net.pending_snapshots);

    let latest_snapshot: std::collections::HashMap<_, _> = snapshots
        .iter()
        .enumerate()
        .map(|(index, (id, _, _))| (id.clone(), index))
        .collect();
    for (index, (remote_id, snap, snapshot_tick)) in snapshots.into_iter().enumerate() {
        if let Some((entity, _)) = remotes
            .iter()
            .find(|(_, remote)| remote.player_id == remote_id)
        {
            let role_driven = contact_segments
                .get(entity)
                .is_ok_and(|segment| segment.role_driven)
                || net
                    .pending_role_states
                    .iter()
                    .any(|(id, _, _)| id == &remote_id);
            if let Ok((
                mut pose,
                mut health,
                mut fighter,
                mut motion,
                mut death,
                mut air,
                generation,
                mut reaction,
                mut window,
            )) = fighters.get_mut(entity)
            {
                fighter.weapon = snap.weapon;
                let last_hit_tick = generation
                    .as_deref()
                    .and_then(|generation| generation.last_hit_tick);
                let hit_owns_generation = snapshot_tick
                    .zip(last_hit_tick)
                    .is_some_and(|(snapshot_tick, hit_tick)| hit_tick >= snapshot_tick);
                if !hit_owns_generation {
                    if !role_driven {
                        if let Ok(mut target) = targets.get_mut(entity) {
                            target.translation = Vec3::new(snap.x, snap.y, snap.z);
                            target.yaw = snap.yaw;
                        }
                        pose.pose_id = snap.pose_id;
                        net.remote_pose_ids.insert(remote_id.clone(), snap.pose_id);
                        pose.frame = snap.pose_frame as f32;
                        pose.start = snap.pose_start;
                        pose.end = snap.pose_end;
                        pose.loop_start = snap.pose_loop_start;
                        pose.loop_end = snap.pose_loop_end;
                        pose.loops = snap.pose_loops;
                        motion.movement_mode = movement_mode_from_id(snap.movement_mode);
                        air.air.airborne = matches!(
                            motion.movement_mode,
                            meteor_core::MovementMode::Airborne
                                | meteor_core::MovementMode::WallRun
                        );
                        air.ground_contact = !air.air.airborne;
                    }
                    health.current = snap.hp;
                    health.max = snap.max_hp;

                    death.active = snap.hp <= 0.0;
                    if !role_driven
                        && snapshot_tick
                            .zip(last_hit_tick)
                            .is_some_and(|(snapshot_tick, hit_tick)| snapshot_tick > hit_tick)
                    {
                        reaction.clear();
                        if let Some(window) = window.as_deref_mut() {
                            window.remaining = 0.0;
                        }
                    }
                }
                if let Some(mut generation) = generation {
                    if !role_driven {
                        generation.role_state_tick = snapshot_tick.or(generation.role_state_tick);
                    }
                }
            }
            net.remote_kills.insert(remote_id.clone(), snap.kills);
            continue;
        }
        if latest_snapshot.get(&remote_id) != Some(&index) {
            continue;
        }

        let character_id = net
            .remote_actor_profiles
            .get(&remote_id)
            .map(|profile| profile.0)
            .or_else(|| {
                net.members
                    .iter()
                    .find(|m| m.player_id == remote_id)
                    .map(|m| m.character_id)
            })
            .unwrap_or(0);
        let slot = net
            .members
            .iter()
            .position(|m| m.player_id == remote_id)
            .map(|index| index as u8)
            .unwrap_or(1);
        let health = Health {
            current: snap.hp,
            max: snap.max_hp,
        };
        let mut pose = PoseRuntime {
            pose_id: snap.pose_id,
            frame: snap.pose_frame as f32,
            start: snap.pose_start,
            end: snap.pose_end,
            loop_start: snap.pose_loop_start,
            loop_end: snap.pose_loop_end,
            loops: snap.pose_loops,
            pose_instance_id: 0,
        };
        if let Some(character_assets) = assets
            .as_deref()
            .and_then(|assets| assets.0.get(&format!("p{character_id}")))
        {
            character_assets.apply_pose(&mut pose, snap.pose_id, true);
            pose.frame = (pose.start as f32 + snap.pose_frame as f32)
                .clamp(pose.start as f32, pose.end as f32);
        }
        net.remote_kills.insert(remote_id.clone(), snap.kills);
        let movement_mode = movement_mode_from_id(snap.movement_mode);
        let mut fighter_air = FighterAir::default();
        fighter_air.air.airborne = matches!(
            movement_mode,
            meteor_core::MovementMode::Airborne | meteor_core::MovementMode::WallRun
        );
        fighter_air.ground_contact = !fighter_air.air.airborne;
        net.remote_pose_ids.insert(remote_id.clone(), snap.pose_id);
        commands.spawn((
            (
                RemoteFighter {
                    player_id: remote_id.clone(),
                },
                RemoteVitalsStatus {
                    health_known: !net.is_stock_udp(),
                    anger_known: false,
                },
                Fighter {
                    character_id,
                    player_slot: slot,
                    weapon: snap.weapon,
                    weapon_slots: [snap.weapon, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                health,
                Qi::default(),
                CombatState::default(),
                MotionState {
                    movement_mode,
                    velocity: Vec3::ZERO,
                    facing: snap.yaw,
                },
                pose,
                crate::components::fighter::EntryBlend::default(),
            ),
            (
                SmState::default(),
                DeathRespawn {
                    spawn_pos: Vec3::new(0.0, 0.9, 0.0),
                    active: snap.hp <= 0.0,
                    ..default()
                },
                fighter_air,
                FighterDodge::default(),
                Transform::from_xyz(snap.x, snap.y, snap.z),
                RemoteTransformTarget {
                    translation: Vec3::new(snap.x, snap.y, snap.z),
                    yaw: snap.yaw,
                },
                RemoteAuthorityGeneration {
                    role_state_tick: snapshot_tick,
                    last_hit_tick: None,
                },
                RemoteHitMotionWindow::default(),
                RemoteContactSegment::default(),
                crate::components::fighter::TargetHitClock::default(),
                crate::components::fighter::VisualRootPin::default(),
            ),
        ));
        info!("net: spawned remote fighter {remote_id} (slot {slot})");
    }

    enum PoseInput {
        State(String, RoleState, u32),
        Chain(String, meteor_network::role_state::PoseChain, u32),
    }
    let mut inputs: Vec<_> = std::mem::take(&mut net.pending_role_states)
        .into_iter()
        .map(|(id, state, tick)| PoseInput::State(id, state, tick))
        .collect();
    inputs.extend(
        std::mem::take(&mut net.pending_pose_chains)
            .into_iter()
            .map(|(id, event, tick)| PoseInput::Chain(id, event, tick)),
    );
    inputs.sort_by_key(|input| match input {
        PoseInput::State(_, _, tick) => *tick,
        PoseInput::Chain(_, _, tick) => *tick,
    });
    let mut waiting_for_bootstrap = Vec::new();
    for input in inputs {
        let (remote_id, state, state_tick) = match input {
            PoseInput::State(id, state, tick) => (id, state, tick),
            PoseInput::Chain(id, event, tick) => {
                let Some((entity, _)) = remotes.iter().find(|(_, remote)| remote.player_id == id)
                else {
                    net.pending_pose_chains.push((id, event, tick));
                    continue;
                };
                let Ok((mut pose, _, fighter, _, _, _, generation, _, _)) =
                    fighters.get_mut(entity)
                else {
                    continue;
                };
                if generation
                    .as_deref()
                    .and_then(|g| g.last_hit_tick)
                    .is_some_and(|hit| hit >= tick)
                {
                    continue;
                }

                if !net.is_stock_udp()
                    && generation
                        .as_deref()
                        .and_then(|g| g.role_state_tick)
                        .is_some_and(|state_tick| state_tick > tick)
                    && net
                        .remote_pose_sequences
                        .get(&id)
                        .is_some_and(|(_, applied)| {
                            meteor_network::role_state::pose_sequence_is_newer(
                                *applied,
                                event.sequence,
                            )
                        })
                {
                    continue;
                }
                let Some(character) = assets
                    .as_deref()
                    .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
                else {
                    net.pending_pose_chains.push((id, event, tick));
                    continue;
                };
                let Ok(mut blend) = entry_blends.get_mut(entity) else {
                    continue;
                };
                if !character.poses.contains_key(&event.pose_id) {
                    continue;
                }

                character.apply_pose_with_blend(
                    &mut pose,
                    &mut blend,
                    event.pose_id,
                    true,
                    Some(event.blend_seconds),
                    crate::components::fighter::EntryBlendKind::Chained,
                    false,
                    true,
                    None,
                );

                blend.duration = event.blend_seconds.max(0.0);

                pose.loops = false;
                net.remote_pose_ids.insert(id.clone(), event.pose_id);
                let sequences = net.remote_pose_sequences.entry(id).or_default();
                sequences.0 = sequences.0.wrapping_add(1);
                sequences.1 = event.sequence;
                continue;
            }
        };
        if net
            .stock_npc_actors
            .values()
            .any(|id| remote_id == format!("stock-{id}"))
        {
            continue;
        }
        let Some((entity, _)) = remotes
            .iter()
            .find(|(_, remote)| remote.player_id == remote_id)
        else {
            waiting_for_bootstrap.push((remote_id, state, state_tick));
            continue;
        };
        let Ok((
            mut pose,
            _,
            fighter,
            mut motion,
            _,
            mut air,
            generation,
            mut reaction,
            mut window,
        )) = fighters.get_mut(entity)
        else {
            continue;
        };
        let last_hit_tick = generation
            .as_deref()
            .and_then(|generation| generation.last_hit_tick);
        let hit_owns_generation = last_hit_tick.is_some_and(|hit_tick| hit_tick >= state_tick);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(trace) = trace.as_deref_mut() {
            trace.event(serde_json::json!({"kind":"role-state-apply","entity":entity.to_bits(),"remoteId":remote_id,"tick":state_tick,"lastHitTick":last_hit_tick,"blockedByHitGeneration":hit_owns_generation,"position":[state.position.x,state.position.y,state.position.z],"velocity":[state.velocity.x,state.velocity.y,state.velocity.z],"pose":state.state_words.pose_id}));
        }
        let (ground_sequence, applied_sequence) = net
            .remote_pose_sequences
            .get(&remote_id)
            .copied()
            .unwrap_or_default();
        let transition = meteor_network::role_state::decide_pose_transition(
            RemotePoseRuntime {
                pose_id: *net
                    .remote_pose_ids
                    .entry(remote_id.clone())
                    .or_insert(pose.pose_id),
                ground_sequence,
                applied_sequence,
            },
            state,
        );
        if transition.apply && !hit_owns_generation {
            net.remote_pose_ids
                .insert(remote_id.clone(), state.state_words.pose_id);
            if let Some(character_assets) = assets
                .as_deref()
                .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
            {
                if let Ok(mut blend) = entry_blends.get_mut(entity) {
                    character_assets.apply_pose_with_blend(
                        &mut pose,
                        &mut blend,
                        state.state_words.pose_id,
                        true,
                        Some(0.1),
                        crate::components::fighter::EntryBlendKind::Generic,
                        false,
                        true,
                        None,
                    );
                } else {
                    character_assets.apply_pose(&mut pose, state.state_words.pose_id, true);
                }
            } else {
                pose.pose_id = state.state_words.pose_id;
                pose.frame = 0.0;
            }
        }
        net.remote_pose_sequences.insert(
            remote_id.clone(),
            (transition.ground_sequence, transition.applied_sequence),
        );
        if !hit_owns_generation {
            if let Ok(mut target) = targets.get_mut(entity) {
                target.translation =
                    Vec3::new(state.position.x, state.position.y, state.position.z);
                target.yaw = yaw_from_forward(state.forward);
            }
            motion.velocity = Vec3::new(state.velocity.x, state.velocity.y, state.velocity.z);

            motion.movement_mode = if state.state_words.grounded {
                meteor_core::MovementMode::Idle
            } else {
                meteor_core::MovementMode::Airborne
            };
            air.air.airborne = !state.state_words.grounded;
            air.ground_contact = state.state_words.grounded;
            if let Ok(mut segment) = contact_segments.get_mut(entity) {
                segment.role_driven = true;
                segment.state_age = 0.0;
            }

            if last_hit_tick.is_some_and(|hit_tick| state_tick > hit_tick)
                && !window
                    .as_deref()
                    .is_some_and(|window| window.remaining > 0.0)
            {
                reaction.clear();
                if let Some(window) = window.as_deref_mut() {
                    window.remaining = 0.0;
                }
            }
        }
        if let Some(mut generation) = generation {
            generation.role_state_tick = Some(
                generation
                    .role_state_tick
                    .map_or(state_tick, |tick| tick.max(state_tick)),
            );
        }
    }
    net.pending_role_states = waiting_for_bootstrap;
}

fn sync_network_escort_carrier(
    mut commands: Commands,
    net: Res<NetState>,
    fighters: EscortCarrierFighterQuery<'_, '_>,
) {
    let carrier_id = escort_carrier_id(&net);
    for (entity, remote, local, carrying) in &fighters {
        let is_carrier = carrier_id.is_some_and(|carrier_id| {
            local.is_some_and(|_| carrier_id == net.player_id)
                || remote.is_some_and(|remote| carrier_id == remote.player_id)
        });
        match (is_carrier, carrying.is_some()) {
            (true, false) => {
                commands.entity(entity).insert(EscortFlagCarrier);
            }
            (false, true) => {
                commands.entity(entity).remove::<EscortFlagCarrier>();
            }
            _ => {}
        }
    }
}

fn escort_carrier_id(net: &NetState) -> Option<&str> {
    let flag = net.escort_flag.as_ref()?;
    (net.status == NetStatus::InRoom && flag.state.eq_ignore_ascii_case("carried"))
        .then_some(flag.carrier_id.as_deref())
        .flatten()
}

fn net_smooth_remote_fighters(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    #[cfg(not(target_arch = "wasm32"))] mut trace: Option<ResMut<super::combat_trace::CombatTrace>>,
    mut remotes: Query<
        (
            Entity,
            &mut Transform,
            &mut MotionState,
            &mut RemoteTransformTarget,
            &mut RemoteContactSegment,
        ),
        With<RemoteFighter>,
    >,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let legacy_factor = (dt.max(0.0) * 12.0).min(1.0);
    for (entity, mut transform, mut motion, mut target, mut segment) in &mut remotes {
        let display_start = transform.translation;
        let target_start = target.translation;
        let (contact_start, contact_end) = if segment.role_driven {
            segment.state_age += dt.max(0.0);
            segment.hit_age += dt.max(0.0);
            let correction = target.translation - transform.translation;
            let correction_factor = meteor_network::role_state::role_position_correction_factor(
                correction.length_squared(),
                motion.velocity.length_squared(),
                dt,
                segment.hit_age,
            );
            transform.translation = transform
                .translation
                .lerp(target.translation, correction_factor);
            motion.facing = lerp_remote_yaw(
                motion.facing,
                target.yaw,
                meteor_network::role_state::role_yaw_correction_factor(dt, segment.hit_age),
            );

            let physical_start = transform.translation;
            let delta = meteor_network::role_state::remote_velocity_delta(
                segment.state_age,
                segment.hit_age,
                meteor_network::role_state::RoleVec3 {
                    x: motion.velocity.x,
                    y: motion.velocity.y,
                    z: motion.velocity.z,
                },
                dt,
            );
            let physical_delta = Vec3::new(delta.x, delta.y, delta.z);
            transform.translation += physical_delta;

            target.translation += physical_delta;
            (physical_start, transform.translation)
        } else {
            transform.translation = transform
                .translation
                .lerp(target.translation, legacy_factor);
            motion.facing = lerp_remote_yaw(motion.facing, target.yaw, legacy_factor);
            (display_start, transform.translation)
        };
        if (contact_end.x - contact_start.x).abs() > 1e-6
            || (contact_end.z - contact_start.z).abs() > 1e-6
        {
            if !segment.valid {
                segment.from = contact_start;
            }
            segment.to = contact_end;
            segment.valid = true;
        }

        transform.rotation = Quat::IDENTITY;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(trace) = trace.as_deref_mut() {
            trace.event(serde_json::json!({"kind":"remote-smooth","entity":entity.to_bits(),"dt":dt,"roleDriven":segment.role_driven,"stateAge":segment.state_age,"hitAge":segment.hit_age,"before":display_start.to_array(),"after":transform.translation.to_array(),"targetBefore":target_start.to_array(),"targetAfter":target.translation.to_array(),"correctionDelta":(contact_start-display_start).to_array(),"contactDelta":(contact_end-contact_start).to_array(),"velocity":motion.velocity.to_array()}));
        }
    }
}

fn lerp_remote_yaw(current: f32, target: f32, factor: f32) -> f32 {
    let mut delta = target - current;
    while delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    }
    while delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    current + delta * factor.clamp(0.0, 1.0)
}

struct NetworkReactionView<'a> {
    pose: &'a mut PoseRuntime,
    blend: &'a mut EntryBlend,
    combat: &'a mut CombatState,
    health: &'a mut Health,
    air: &'a FighterAir,
    assets: &'a CharacterAssets,
    strategy_id: u8,
}

impl NetworkReactionView<'_> {
    fn apply_pose(&mut self, pose_id: u16, hard_cut: bool) {
        self.assets.apply_pose_with_blend(
            self.pose,
            self.blend,
            pose_id,
            true,
            None,
            crate::components::fighter::EntryBlendKind::Generic,
            hard_cut,
            false,
            Some(self.strategy_id),
        );
    }
}

impl ReactionFighter for NetworkReactionView<'_> {
    fn pose_id(&self) -> u16 {
        self.pose.pose_id
    }
    fn is_pose_finished(&self) -> bool {
        self.pose.finished()
    }
    fn has_pose(&self, pose_id: u16) -> bool {
        self.assets.poses.contains_key(&pose_id)
    }
    fn play_pose(&mut self, pose_id: u16, _hard: bool) {
        self.apply_pose(pose_id, false);
    }
    fn play_pose_hard_cut(&mut self, pose_id: u16) {
        self.apply_pose(pose_id, true);
    }
    fn set_in_guard(&mut self, _v: bool) {}
    fn current_state(&self) -> meteor_core::FighterState {
        self.combat.state
    }
    fn set_state(&mut self, state: meteor_core::FighterState, _locked: bool) {
        self.combat.state = state;
    }
    fn alive(&self) -> bool {
        self.health.current > 0.0
    }
    fn guard_held(&self) -> bool {
        false
    }
    fn pose_link(&self, pose_id: u16) -> Option<u16> {
        self.assets.poses.get(&pose_id).and_then(|pose| pose.link)
    }
    fn is_airborne(&self) -> bool {
        self.air.air.airborne
    }
    fn pose_range(&self, pose_id: u16) -> Option<(u32, u32)> {
        self.assets.pose_frames.get(&pose_id).copied()
    }
    fn release_pose_loop_to_end(&mut self, pose_id: u16) {
        if self.pose.pose_id == pose_id {
            self.pose.loops = false;
        }
    }
    fn rewind_reaction_pose(&mut self, pose_id: u16) {
        if self.pose.pose_id == pose_id {
            self.pose.frame = self.pose.loop_start as f32;
            self.pose.loops = true;
        }
    }
}

fn finite_reaction_value(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn stock_guard_bucket(word: u32) -> meteor_combat::hit_reaction::FacingBucket {
    use meteor_combat::hit_reaction::FacingBucket;
    match word & 0xffff {
        1 => FacingBucket::Right,
        2 => FacingBucket::Left,
        3 => FacingBucket::Back,
        _ => FacingBucket::Front,
    }
}

fn authority_attack_block(hit: &AuthorityHit) -> AttackBlock {
    let reaction = hit.reaction.as_ref();
    let fallback_scale = finite_reaction_value(hit.motion_scale.unwrap_or(0.0));
    AttackBlock {
        start: 0,
        end: 0,
        attack_type: 0,
        check_friend: 0,
        defense_value: reaction.map_or(0.0, |reaction| {
            finite_reaction_value(reaction.defense_value)
        }),
        defense_move: reaction.map_or(fallback_scale, |reaction| {
            finite_reaction_value(reaction.defense_move)
        }),
        target_value: reaction.map_or(0.0, |reaction| finite_reaction_value(reaction.target_value)),
        target_move: reaction.map_or(fallback_scale, |reaction| {
            finite_reaction_value(reaction.target_move)
        }),
        target_pose: reaction.and_then(|reaction| reaction.target_pose),
        target_pose_front: reaction.and_then(|reaction| reaction.target_pose_front),
        target_pose_back: reaction.and_then(|reaction| reaction.target_pose_back),
        target_pose_left: reaction.and_then(|reaction| reaction.target_pose_left),
        target_pose_right: reaction.and_then(|reaction| reaction.target_pose_right),
        bones: Vec::new(),
    }
}

fn set_network_reaction_state(
    sm: &mut SmState,
    combat: &mut CombatState,
    state: meteor_core::FighterState,
) {
    sm.previous = sm.current;
    sm.current = state;
    sm.time_in_state = 0.0;
    sm.locked = true;
    sm.attack_stage = 0;
    sm.attack_queued = None;
    sm.linked_recovery_pose_id = None;
    sm.exe_ground_mode_ticks = 0;
    combat.state = state;
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn net_handle_hit(
    assets: Option<Res<CharacterAssetsSet>>,
    mut net: ResMut<NetState>,
    mut impacts: MessageWriter<crate::plugins::impact_fx_plugin::CombatImpactEvent>,
    mut query: Query<
        (
            Entity,
            Option<&LocalFighter>,
            &Fighter,
            &Transform,
            &mut Health,
            &mut Qi,
            &mut DeathRespawn,
            (
                &mut PoseRuntime,
                &mut EntryBlend,
                &mut CombatState,
                &mut SmState,
                &mut MotionState,
                &mut FighterAir,
                &mut FighterHitReaction,
                &mut FighterAttackMotion,
            ),
        ),
        (With<Fighter>, Without<RemoteFighter>),
    >,
    remotes: Query<(Entity, &RemoteFighter, &Transform), Without<LocalFighter>>,
) {
    let mut hits: Vec<_> = std::mem::take(&mut net.pending_hits)
        .into_iter()
        .map(|hit| (None, hit))
        .collect();
    hits.extend(
        std::mem::take(&mut net.pending_npc_hits)
            .into_iter()
            .map(|(entity, hit)| (Some(entity), hit)),
    );
    if hits.is_empty() || (!net.round_active && net.battle_result.is_none()) {
        return;
    }
    for (target, hit) in hits {
        for (
            entity,
            local,
            fighter,
            transform,
            mut health,
            mut qi,
            mut death,
            (
                mut pose,
                mut blend,
                mut combat,
                mut sm,
                mut motion,
                air,
                mut reaction_runtime,
                mut attack_motion,
            ),
        ) in &mut query
        {
            if !target.map_or(local.is_some(), |target| target == entity) {
                continue;
            }
            let was_dead = death.active;
            health.current = hit
                .health
                .unwrap_or_else(|| health.current - hit.damage)
                .clamp(0.0, health.max);
            if let Some(anger) = hit.anger {
                qi.current = anger.clamp(0.0, qi.max);
            }

            if !hit.impact && !hit.terminal {
                continue;
            }
            let attacker = hit.attacker_id.as_deref().and_then(|attacker_id| {
                remotes.iter().find_map(|(entity, remote, transform)| {
                    (remote.player_id == attacker_id).then_some((entity, transform.translation))
                })
            });
            let (attacker_entity, attacker_position) =
                attacker.unwrap_or((entity, transform.translation));
            if hit.impact {
                let kind = if hit.guarded {
                    crate::plugins::impact_fx_plugin::CombatImpactKind::Guard
                } else if health.current <= 0.0 {
                    crate::plugins::impact_fx_plugin::CombatImpactKind::Kill
                } else {
                    crate::plugins::impact_fx_plugin::CombatImpactKind::Hit
                };
                impacts.write(crate::plugins::impact_fx_plugin::CombatImpactEvent {
                    kind,
                    attacker: attacker_entity,
                    defender: entity,
                    reaction_pose: None,
                    effect_strategy: hit.effect_strategy,
                    contact: (attacker_position + transform.translation + Vec3::Y * 2.2) * 0.5,
                    direction: transform.translation - attacker_position,
                });
            }
            if was_dead {
                continue;
            }
            let killed = health.current <= 0.0;
            let Some(character_assets) = assets
                .as_deref()
                .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
            else {
                if killed {
                    *attack_motion = FighterAttackMotion::default();
                    motion.velocity = Vec3::ZERO;
                    set_network_reaction_state(
                        &mut sm,
                        &mut combat,
                        meteor_core::FighterState::Dead,
                    );
                    death.active = true;
                    death.timer = 0.0;
                }
                continue;
            };
            let attack = authority_attack_block(&hit);
            let attacker_position = if attacker_entity == entity {
                transform.translation + Vec3::new(motion.facing.sin(), 0.0, motion.facing.cos())
            } else {
                attacker_position
            };
            let bucket = facing_bucket(
                attacker_position.x,
                attacker_position.z,
                transform.translation.x,
                transform.translation.z,
                motion.facing,
            );
            *attack_motion = FighterAttackMotion::default();
            let strategy_id = weapon_to_strategy(fighter.weapon);
            let poses: Vec<u16> = character_assets.poses.keys().copied().collect();
            let mut view = NetworkReactionView {
                pose: &mut pose,
                blend: &mut blend,
                combat: &mut combat,
                health: &mut health,
                air: &air,
                assets: character_assets,
                strategy_id,
            };
            if killed {
                let reaction = begin_death(&mut view, Some(&attack));
                reaction_runtime.begin(reaction, air.air.airborne);
                motion.velocity = Vec3::ZERO;
                set_network_reaction_state(&mut sm, &mut combat, meteor_core::FighterState::Dead);
                death.active = true;

                death.timer = 0.0;
                info!("net: local fighter defeated");
            } else if hit.guarded {
                let reaction = begin_guard_hit(
                    &mut view,
                    Some(&attack),
                    hit.stock_guard_bucket.unwrap_or(bucket),
                    &poses,
                    strategy_id,
                    meteor_physics::locomotion::guard_pose_for_weapon(strategy_id),
                    entity.to_bits() as u32 & 3,
                );
                if let Some(reaction) = reaction {
                    reaction_runtime.begin(reaction, air.air.airborne);
                }
                set_network_reaction_state(&mut sm, &mut combat, meteor_core::FighterState::Guard);
            } else {
                let reaction = begin_be_hit(&mut view, Some(&attack), bucket, &poses);
                if let Some(reaction) = reaction {
                    reaction_runtime.begin(reaction, air.air.airborne);
                    reaction_runtime.bank_be_hit_carry(motion.velocity, local.is_some());
                    set_network_reaction_state(
                        &mut sm,
                        &mut combat,
                        meteor_core::FighterState::BeHit,
                    );
                }
            }
        }
    }
}

const REMOTE_HIT_ROOT_SECONDS: f32 = 0.4;

#[allow(clippy::type_complexity)]
fn net_handle_remote_hits(
    assets: Option<Res<CharacterAssetsSet>>,
    mut net: ResMut<NetState>,
    mut impacts: MessageWriter<crate::plugins::impact_fx_plugin::CombatImpactEvent>,
    attackers: Query<
        (
            Entity,
            Option<&LocalFighter>,
            Option<&RemoteFighter>,
            &Transform,
        ),
        With<Fighter>,
    >,
    mut remotes: Query<
        (
            Entity,
            &RemoteFighter,
            &Fighter,
            &Transform,
            &MotionState,
            &mut Health,
            &mut PoseRuntime,
            &mut EntryBlend,
            &mut CombatState,
            &mut DeathRespawn,
            &FighterAir,
            &mut FighterHitReaction,
            &mut RemoteHitMotionWindow,
            &mut RemoteAuthorityGeneration,
            Option<&mut RemoteContactSegment>,
        ),
        Without<LocalFighter>,
    >,
) {
    let hits = std::mem::take(&mut net.pending_remote_hits);
    if hits.is_empty() || (!net.round_active && net.battle_result.is_none()) {
        return;
    }
    for (target_id, hit) in hits {
        let attacker = hit.attacker_id.as_deref().and_then(|attacker_id| {
            attackers
                .iter()
                .find_map(|(entity, local, remote, transform)| {
                    let matches = local.is_some_and(|_| attacker_id == net.player_id)
                        || remote.is_some_and(|remote| remote.player_id == attacker_id);
                    matches.then_some((entity, transform.translation))
                })
        });
        let attacker_is_local = hit.attacker_id.as_deref() == Some(net.player_id.as_str());
        for (
            entity,
            remote,
            fighter,
            transform,
            motion,
            mut health,
            mut pose,
            mut blend,
            mut combat,
            mut death,
            air,
            mut reaction_runtime,
            mut window,
            mut generation,
            mut contact_segment,
        ) in &mut remotes
        {
            if remote.player_id != target_id {
                continue;
            }
            let (attacker_entity, attacker_position) =
                attacker.unwrap_or((entity, transform.translation + Vec3::NEG_Z));
            let lethal = hit.health.is_some_and(|health| health <= 0.0);

            if hit.impact && !attacker_is_local {
                impacts.write(crate::plugins::impact_fx_plugin::CombatImpactEvent {
                    kind: if hit.guarded {
                        crate::plugins::impact_fx_plugin::CombatImpactKind::Guard
                    } else if lethal {
                        crate::plugins::impact_fx_plugin::CombatImpactKind::Kill
                    } else {
                        crate::plugins::impact_fx_plugin::CombatImpactKind::Hit
                    },
                    attacker: attacker_entity,
                    defender: entity,
                    reaction_pose: None,
                    effect_strategy: hit.effect_strategy,
                    contact: (attacker_position + transform.translation + Vec3::Y * 2.2) * 0.5,
                    direction: transform.translation - attacker_position,
                });
            }
            let superseded = hit.tick.is_some_and(|tick| {
                generation
                    .role_state_tick
                    .is_some_and(|snapshot_tick| snapshot_tick > tick)
                    || generation
                        .last_hit_tick
                        .is_some_and(|last_hit_tick| last_hit_tick > tick)
            });
            if superseded {
                continue;
            }
            if let Some(tick) = hit.tick {
                generation.last_hit_tick = Some(
                    generation
                        .last_hit_tick
                        .map_or(tick, |last_hit_tick| last_hit_tick.max(tick)),
                );
            }
            if hit.impact {
                let sequence = net
                    .remote_pose_sequences
                    .entry(target_id.clone())
                    .or_default();
                sequence.0 = sequence.0.wrapping_add(1);
                sequence.1 = sequence.1.wrapping_add(1);
                if let Some(segment) = contact_segment.as_deref_mut() {
                    segment.hit_age = 0.0;
                }
            }
            if let Some(authority_health) = hit.health {
                health.current = authority_health.clamp(0.0, health.max);
            }
            if death.active {
                continue;
            }
            let Some(character_assets) = assets
                .as_deref()
                .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
            else {
                if lethal {
                    death.active = true;
                }
                continue;
            };
            let attack = authority_attack_block(&hit);
            let strategy_id = weapon_to_strategy(fighter.weapon);
            let poses: Vec<u16> = character_assets.poses.keys().copied().collect();
            let mut view = NetworkReactionView {
                pose: &mut pose,
                blend: &mut blend,
                combat: &mut combat,
                health: &mut health,
                air,
                assets: character_assets,
                strategy_id,
            };
            if lethal {
                let _ = begin_death(&mut view, Some(&attack));
                reaction_runtime.clear();
                window.remaining = 0.0;
                death.active = true;
            } else if hit.guarded {
                if let Some(reaction) = begin_guard_hit(
                    &mut view,
                    Some(&attack),
                    hit.stock_guard_bucket
                        .unwrap_or(meteor_combat::hit_reaction::FacingBucket::Front),
                    &poses,
                    strategy_id,
                    meteor_physics::locomotion::guard_pose_for_weapon(strategy_id),
                    entity.to_bits() as u32 & 3,
                ) {
                    reaction_runtime.begin(reaction, air.air.airborne);
                    window.remaining = REMOTE_HIT_ROOT_SECONDS;
                }
            } else {
                let bucket = facing_bucket(
                    attacker_position.x,
                    attacker_position.z,
                    transform.translation.x,
                    transform.translation.z,
                    motion.facing,
                );
                if let Some(reaction) = begin_be_hit(&mut view, Some(&attack), bucket, &poses) {
                    reaction_runtime.begin(reaction, air.air.airborne);
                    window.remaining = REMOTE_HIT_ROOT_SECONDS;
                }
            }
            net.remote_pose_ids.insert(target_id.clone(), pose.pose_id);
            break;
        }
    }
}

type AuthorityRespawnQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        (
            Option<&'static LocalFighter>,
            Option<&'static super::stage_plugin::MissionNpc>,
        ),
        &'static Fighter,
        &'static mut Health,
        &'static mut Qi,
        &'static mut CombatState,
        &'static mut PoseRuntime,
        &'static mut SmState,
        &'static mut DeathRespawn,
        &'static mut TargetHitClock,
        &'static mut Transform,
        (
            &'static mut EntryBlend,
            &'static mut MotionState,
            &'static mut FighterAir,
            &'static mut FighterDodge,
            &'static mut FighterAttackMotion,
            &'static mut FighterHitReaction,
            &'static mut GunAimState,
            Option<&'static mut AiIntent>,
            Option<&'static mut FighterWall>,
        ),
    ),
    (With<Fighter>, Without<RemoteFighter>),
>;

#[allow(clippy::too_many_arguments)]
fn net_apply_authority_respawns(
    mut commands: Commands,
    mut net: ResMut<NetState>,
    assets: Option<Res<crate::plugins::character_plugin::CharacterAssetsSet>>,
    mut lock: ResMut<LockOnController>,
    mut camera_basis: ResMut<crate::plugins::camera_plugin::CameraBasis>,
    stage: super::stage_plugin::RespawnStage,
    mut query: AuthorityRespawnQuery<'_, '_>,
    mut statuses: Query<(
        &mut crate::components::fighter::FighterStatusEffects,
        &mut crate::components::fighter::WornEffect,
    )>,
) {
    if net.wire_format != NetWireFormat::V4 || net.pending_respawns.is_empty() {
        return;
    }
    let mut respawns = std::mem::take(&mut net.pending_respawns);
    let mut relocations = std::mem::take(&mut net.pending_spawn_relocations);
    for (
        entity,
        (local, mission),
        fighter,
        mut health,
        mut qi,
        mut combat,
        mut pose,
        mut sm,
        mut death,
        mut target_hit_clock,
        mut transform,
        (
            mut blend,
            mut motion,
            mut air,
            mut dodge,
            mut attack_motion,
            mut hit_reaction,
            mut gun_aim,
            mut ai_intent,
            mut wall,
        ),
    ) in &mut query
    {
        let actor = if local.is_some() {
            Some(net.player_id.clone())
        } else {
            net.stock_npc_actors
                .get(&entity)
                .map(|id| format!("stock-{id}"))
        };
        let Some(actor) = actor.filter(|id| respawns.contains(id)) else {
            continue;
        };
        if relocations.contains(&actor) || death.active {
            if let Ok((mut status, mut worn)) = statuses.get_mut(entity) {
                status.clear(&mut worn);
            }
        }
        health.current = health.max;
        qi.current = 0.0;
        let character_assets = assets
            .as_deref()
            .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)));
        crate::plugins::combat_plugin::reset_fighter_runtime_for_respawn(
            character_assets,
            &mut combat,
            &mut pose,
            &mut blend,
            &mut sm,
            &mut motion,
            &mut air,
            &mut dodge,
            &mut attack_motion,
            &mut hit_reaction,
            &mut gun_aim,
            ai_intent.as_deref_mut(),
            wall.as_deref_mut(),
        );
        commands.entity(entity).remove::<WeaponSwitchActive>();
        if relocations.contains(&actor) {
            let member_index = net
                .members
                .iter()
                .position(|member| member.player_id == actor);
            let team = member_index
                .and_then(|index| net.members[index].team)
                .or_else(|| mission.map(|mission| u32::from(mission.team)));
            let pad_index = (!net.is_stock_udp())
                .then(|| {
                    member_index.map(|index| {
                        net.members[..index]
                            .iter()
                            .filter(|member| member.team == team)
                            .count()
                    })
                })
                .flatten();
            let (position, facing) = stage.resolve(team, pad_index, death.spawn_pos, motion.facing);
            transform.translation = position;

            death.spawn_pos = position;
            motion.facing = facing;
            air.ground_y = position.y;
        }
        death.active = false;
        death.timer = 0.0;
        target_hit_clock.reset_spawn();
        target_hit_clock.reset();
        if local.is_some() {
            lock.system.clear();
            lock.lock_entity = None;
            lock.track_entity = None;
            camera_basis.yaw = motion.facing;
        }
        respawns.retain(|id| id != &actor);
        relocations.remove(&actor);
    }

    net.pending_respawns = respawns;
    net.pending_spawn_relocations = relocations;
}

#[derive(Component)]
pub(crate) struct PendingSelfEffect {
    pub resource_id: u32,
    pub pose_id: u16,
    pub pose_instance: u32,
}

fn net_send_self_effects(
    mut commands: Commands,
    net: Res<NetState>,
    tx: Option<Res<NetTx>>,
    mut sequence: ResMut<NetInputSequence>,
    pending: Query<(Entity, &PendingSelfEffect, Option<&LocalFighter>), Without<RemoteFighter>>,
) {
    for (entity, effect, local) in &pending {
        commands.entity(entity).remove::<PendingSelfEffect>();
        if net.status != NetStatus::InRoom || !net.round_active {
            continue;
        }
        let Some(tx) = tx.as_ref() else {
            continue;
        };
        #[cfg(not(target_arch = "wasm32"))]
        if net.is_stock_udp() {
            tx.send_stock_self_effect(local.is_none().then_some(entity), effect.resource_id);
            continue;
        }
        if local.is_some() {
            if let (Some(match_epoch), Some(round_epoch)) = (net.match_epoch, net.round_epoch) {
                tx.send_control(Message::RoleSelfEffect {
                    seq: sequence.next(),
                    room_id: net.room_id.clone(),
                    match_epoch,
                    round_epoch,
                    pose_id: effect.pose_id,
                    pose_instance: effect.pose_instance,
                });
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn net_send_hits(
    mut commands: Commands,
    net: Res<NetState>,
    net_tx: Option<Res<NetTx>>,
    mut sequence: ResMut<NetInputSequence>,
    assets: Option<Res<CharacterAssetsSet>>,
    self_effects: Query<(), With<PendingSelfEffect>>,
    local: Query<(&Fighter, &Transform), (With<LocalFighter>, Without<RemoteFighter>)>,
    hits: Query<
        (
            Entity,
            &RemoteFighter,
            &Fighter,
            &Transform,
            &MotionState,
            &PendingRemoteHit,
        ),
        (Changed<PendingRemoteHit>, Without<LocalFighter>),
    >,
) {
    #[cfg(target_arch = "wasm32")]
    let _ = (&assets, &local);
    if !net.round_active {
        for (entity, _, _, _, _, _) in &hits {
            commands.entity(entity).remove::<PendingRemoteHit>();
        }
        return;
    }
    let Some(tx) = net_tx else {
        return;
    };
    for (entity, remote, remote_fighter, remote_transform, remote_motion, hit) in &hits {
        #[cfg(not(target_arch = "wasm32"))]
        if net.is_stock_udp() {
            let stock_hit = (|| {
                let target_actor_id = remote.player_id.strip_prefix("stock-")?.parse().ok()?;
                let (local_fighter, local_transform) = local.single().ok()?;
                let assets = assets.as_deref()?;
                let local_assets = assets.0.get(&format!("p{}", local_fighter.character_id))?;
                let attack = local_assets
                    .poses
                    .get(&hit.pose_id)?
                    .attacks
                    .get(usize::try_from(hit.attack_index).ok()?)?;
                let remote_poses = assets
                    .0
                    .get(&format!("p{}", remote_fighter.character_id))?
                    .poses
                    .keys()
                    .copied()
                    .collect::<Vec<_>>();
                let bucket = facing_bucket(
                    local_transform.translation.x,
                    local_transform.translation.z,
                    remote_transform.translation.x,
                    remote_transform.translation.z,
                    remote_motion.facing,
                );
                Some(StockHitRequest::from_attack(
                    target_actor_id,
                    hit.pose_id,
                    local_fighter.weapon,
                    attack,
                    bucket,
                    &remote_poses,
                ))
            })();
            if let Some(stock_hit) = stock_hit {
                tx.send_stock_hit(stock_hit);
            } else {
                warn!(
                    "stock UDP hit skipped: missing actor/POS attack mapping for {} pose {} attack {}",
                    remote.player_id, hit.pose_id, hit.attack_index
                );
            }
            commands.entity(entity).remove::<PendingRemoteHit>();
            continue;
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (remote_fighter, remote_transform, remote_motion);
        send_web_hit(
            &tx,
            &mut sequence,
            &net,
            remote,
            hit,
            !self_effects.is_empty(),
        );
        commands.entity(entity).remove::<PendingRemoteHit>();
    }
}

fn send_web_hit(
    tx: &NetTx,
    sequence: &mut NetInputSequence,
    net: &NetState,
    remote: &RemoteFighter,
    hit: &PendingRemoteHit,
    ordered: bool,
) {
    if net.wire_format == NetWireFormat::V4 {
        let forward = hit.attack_forward.normalize_or_zero();
        let center = hit.attack_origin + forward * (1.7 * 0.5);
        let right = Vec3::new(forward.z, 0.0, -forward.x);
        let send_input = |input| {
            if ordered {
                if let Some(sender) = tx.active.as_ref() {
                    let _ = sender.send(NetOutbound::OrderedInput(input));
                }
            } else {
                tx.send_input(input);
            }
        };
        send_input(V4Input {
            seq: sequence.next(),
            role_state: Vec::new(),
            role_rate_request: None,
            r#move: None,
            yaw: 0.0,
            run: false,
            guard: false,
            actions: Vec::new(),
            combat: vec![V4CombatJudge {
                kind: "precise".to_string(),
                pose_id: hit.pose_id as u32,
                attack_index: hit.attack_index,
                pose_instance_id: 0,
                frame: hit.frame,
                cursor: hit.cursor,
                boxes: vec![PackedObb {
                    values: vec![
                        center.x,
                        center.y,
                        center.z,
                        right.x,
                        right.y,
                        right.z,
                        0.0,
                        1.0,
                        0.0,
                        forward.x,
                        forward.y,
                        forward.z,
                        0.6,
                        0.9,
                        1.7 * 0.5,
                    ],
                }],
                ranged_kind: String::new(),
                start: None,
                end: None,
                target_ids: vec![remote.player_id.clone()],
            }],
            visual: Some(V4InputVisual {
                pose_id: hit.pose_id as u32,
                cursor: hit.cursor,
                lift: 0.0,
                character: String::new(),
                weapon: String::new(),
                world_position: Some(V4Vec3 {
                    x: hit.attack_origin.x,
                    y: hit.attack_origin.y,
                    z: hit.attack_origin.z,
                }),
                carries_hurt_obbs: false,
                hurt_obbs: Vec::new(),
                carries_hurt_whole_obb: false,
                hurt_whole_obb: None,
            }),
            match_epoch: net.match_epoch,
            round_epoch: net.round_epoch,
        });
    } else {
        tx.send_control(Message::CombatHit {
            target: remote.player_id.clone(),
            damage: hit.damage,
            attacker_id: None,
            guarded: None,
            health: None,
            anger: None,
            motion_scale: None,
            reaction: None,
            tick: None,
        });
    }
}

fn net_room_start_button(
    query: Query<
        &Interaction,
        (
            Changed<Interaction>,
            With<crate::app_state::NetRoomStartButton>,
        ),
    >,
    net: Res<NetState>,
    net_tx: Option<Res<NetTx>>,
) {
    if !query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        return;
    }
    let Some(tx) = net_tx else {
        return;
    };
    if net.room_start_blocker().is_none() {
        tx.send_control(Message::RoomStart);
    }
}

fn net_room_team_select_buttons(
    query: Query<(&Interaction, &crate::app_state::NetRoomTeamSelectButton), Changed<Interaction>>,
    mut net: ResMut<NetState>,
    net_tx: Option<Res<NetTx>>,
) {
    if !is_team_mode(&net.mode) || !net.can_select_room_team() {
        return;
    }
    let Some(tx) = net_tx else {
        return;
    };
    for (interaction, button) in &query {
        if *interaction == Interaction::Pressed && matches!(button.team, 1 | 2) {
            let count = net
                .members
                .iter()
                .filter(|m| m.team == Some(button.team))
                .count();
            let selected = net
                .local_room_member()
                .is_some_and(|m| m.team == Some(button.team));
            if selected || count < net.room_capacity().div_ceil(2) {
                if net.battle_pending {
                    net.late_join_team = Some(button.team);
                } else {
                    tx.send_control(Message::TeamSelect { team: button.team });
                }
            }
        }
    }
}

pub(crate) fn net_update_team_select_buttons(
    mut commands: Commands,
    net: Res<NetState>,
    mut buttons: Query<(
        Entity,
        &crate::app_state::NetRoomTeamSelectButton,
        &mut Node,
        &Children,
        Has<bevy::ui::InteractionDisabled>,
        Option<&mut UiSelectionState>,
    )>,
    mut labels: Query<&mut Text>,
) {
    for (entity, button, mut node, children, disabled, selection) in &mut buttons {
        node.display = if is_team_mode(&net.mode) {
            Display::Flex
        } else {
            Display::None
        };
        let selected = net.selected_room_team() == Some(button.team);
        let count = net
            .members
            .iter()
            .filter(|m| m.team == Some(button.team))
            .count();
        let enabled =
            net.can_select_room_team() && (selected || count < net.room_capacity().div_ceil(2));
        if enabled == disabled {
            if enabled {
                commands
                    .entity(entity)
                    .remove::<bevy::ui::InteractionDisabled>();
            } else {
                commands
                    .entity(entity)
                    .insert(bevy::ui::InteractionDisabled);
            }
        }
        if let Some(mut selection) = selection {
            selection.selected = selected;
        }
        let label = format!(
            "{} {}/{}",
            if button.team == 1 { "流星" } else { "蝴蝶" },
            count,
            net.room_capacity().div_ceil(2)
        );
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child) {
                if text.0 != label {
                    text.0 = label.clone();
                }
            }
        }
    }
}

fn net_room_leave_button(
    query: Query<
        &Interaction,
        (
            Changed<Interaction>,
            With<crate::app_state::NetRoomLeaveButton>,
        ),
    >,
    mut net: ResMut<NetState>,
    mut net_tx: ResMut<NetTx>,
    mut net_rx: ResMut<NetRx>,
    mut next_state: ResMut<NextState<AppState>>,
) {
    if !query
        .iter()
        .any(|interaction| *interaction == Interaction::Pressed)
    {
        return;
    }
    net_tx.send_control(Message::RoomLeave);
    net_tx.disconnect();
    net_tx.clear();
    net_rx.0 = None;
    net.intentional_disconnect = true;
    net.transport_connected = false;
    net.reconnect_attempt = 0;
    net.reconnect_at = None;
    net.status = NetStatus::Disconnected;
    net.friendly_fire = false;
    net.match_epoch = None;
    net.round_epoch = None;
    net.last_full_snapshot_tick = None;
    net.last_role_snapshot_tick = None;
    net.error = None;
    net.room_latency.clear();
    net.members.clear();
    net.room_id.clear();
    net.escort_flag = None;
    net.pending_objective_end = None;
    next_state.set(AppState::Menu);
}

fn net_update_room_members(
    net: Res<NetState>,
    mut texts: Query<&mut Text, With<crate::app_state::NetRoomMembersText>>,
) {
    for mut text in &mut texts {
        text.0 = net_room_members_text(&net);
    }
}

fn net_room_members_text(net: &NetState) -> String {
    let mut lines = Vec::with_capacity(net.members.len() + 2);
    lines.push(format!(
        "联机房间（{} · {}{}）",
        if net.mode.is_empty() {
            "deathmatch"
        } else {
            &net.mode
        },
        if net.stage.is_empty() {
            "sn01"
        } else {
            &net.stage
        },
        if net.room_id.is_empty() {
            String::new()
        } else {
            format!(" · {}", net.room_id)
        }
    ));
    if net.members.is_empty() {
        lines.push("  等待玩家加入…".to_string());
    }
    for (index, member) in net.members.iter().enumerate() {
        let host = if index == 0 { "（房主）" } else { "" };
        let team = match member.team {
            Some(1) => " · 流星队",
            Some(2) => " · 蝴蝶队",
            _ => "",
        };
        lines.push(format!(
            "  [{}] {} · 角色 P{} · 武器 {}{}{}{host}",
            index,
            member.name,
            member.character_id,
            member.weapon,
            team,
            if member.ready { " · 就绪" } else { "" }
        ));
    }
    lines.push(format!("我的 ID：{}", net.player_id));
    lines.join("\n")
}

fn net_chat_input(
    mut submitted: MessageReader<super::native_text_input::TextSubmitted>,
    mut net: ResMut<NetState>,
    net_tx: Option<Res<NetTx>>,
    mut input: ResMut<NetChatInput>,
) {
    for event in submitted.read() {
        if event.field != super::native_text_input::TextField::Chat || event.value.is_empty() {
            continue;
        }
        let text = event.value.clone();
        input.text.clear();
        let my_id = net.player_id.clone();
        net.chat_messages
            .push_back((format!("{my_id}（我）"), text.clone()));
        if net.chat_messages.len() > 20 {
            net.chat_messages.pop_front();
        }
        if let Some(tx) = net_tx.as_ref() {
            tx.send_control(Message::ChatSend { text });
        }
    }
}

fn net_update_chat(
    net: Res<NetState>,
    input: Res<NetChatInput>,
    mut chat_texts: Query<
        &mut Text,
        (
            With<crate::app_state::NetRoomChatText>,
            Without<crate::app_state::NetRoomChatInputText>,
        ),
    >,
    mut input_texts: Query<
        &mut Text,
        (
            With<crate::app_state::NetRoomChatInputText>,
            Without<crate::app_state::NetRoomChatText>,
        ),
    >,
) {
    for mut text in &mut chat_texts {
        text.0 = net
            .chat_messages
            .iter()
            .rev()
            .take(8)
            .map(|(name, text)| format!("{name}: {text}"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    for mut text in &mut input_texts {
        text.0 = if input.text.is_empty() {
            "点击输入消息".into()
        } else {
            format!("> {}", input.text)
        };
    }
}

fn despawn_remote_fighters(mut commands: Commands, query: Query<Entity, With<RemoteFighter>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn stock_asset_catalog() -> meteor_network::stock_game::StockHeroCatalog {
    #[cfg(test)]
    {
        meteor_network::stock_game::StockHeroCatalog::bundled_revision()
    }
    #[cfg(not(test))]
    {
        meteor_network::stock_game::StockHeroCatalog::packed_revision()
            .expect("validated MPK stock profile")
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn stock_asset_client(
    transport: meteor_network::stock_session::StockClientTransport,
    hero: u32,
    camp: u32,
    weapon: u8,
) -> Result<meteor_network::stock_game::StockGameClient, meteor_network::stock_game::StockGameError>
{
    #[cfg(test)]
    {
        meteor_network::stock_game::StockGameClient::for_bundled_revision(
            transport, hero, camp, weapon,
        )
    }
    #[cfg(not(test))]
    {
        meteor_network::stock_game::StockGameClient::for_packed_revision(
            transport, hero, camp, weapon,
        )
    }
}
#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_environment_death_routes_player_npc_observer_without_kill_credit() {
        use super::*;
        let npc = Entity::from_bits(123);
        let mut net = NetState {
            player_id: "stock-1".into(),
            ..Default::default()
        };
        net.stock_npc_actors.insert(npc, 2);
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next = NextState::default();
        for actor in [1, 2, 3] {
            handle_stock_game_event(
                &mut net,
                &mut request,
                &mut end,
                &mut tx,
                &mut next,
                AppState::Playing,
                StockGameEvent::EnvironmentDeath(actor),
            );
        }
        assert_eq!(net.pending_hits.len(), 1);
        assert_eq!(net.pending_npc_hits[0].0, npc);
        assert_eq!(net.pending_remote_hits[0].0, "stock-3");
        assert!(net.pending_stock_kills.is_empty());
        assert!(net.remote_kills.is_empty());
        assert!(net.stock_deaths.values().all(|count| *count == 1));
        assert_eq!(net.pending_hits[0].health, Some(0.0));
        assert!(net.pending_hits[0].terminal);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_environment_geometry_is_only_published_by_host() {
        use super::*;
        use crate::plugins::hazard_plugin::{
            publish_stock_environment_geometry, StageHazardRuntime,
        };
        use bevy::ecs::system::RunSystemOnce;
        for (address, expected) in [("udp-host://0.0.0.0:5230", 1), ("udp://127.0.0.1:5230", 0)] {
            let (tx, rx) = std::sync::mpsc::channel();
            let mut app = App::new();
            app.init_resource::<StageHazardRuntime>()
                .insert_resource(NetState {
                    address: address.into(),
                    ..Default::default()
                })
                .insert_resource(NetTx {
                    active: Some(tx),
                    connecting: None,
                });
            app.world_mut()
                .run_system_once(publish_stock_environment_geometry)
                .unwrap();
            assert_eq!(
                rx.try_iter()
                    .filter(|m| matches!(m, NetOutbound::StockEnvironmentGeometry(_)))
                    .count(),
                expected
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_anger_main_thread_gap_projects_old_vitals_per_actor_once() {
        use super::{stock_anger_test_channel, PendingStockAnger, StockVitalsEvent};
        let (tx, take) = stock_anger_test_channel();
        let npc = bevy::prelude::World::new().spawn_empty().id();
        let mut ledger = PendingStockAnger::default();
        ledger.send(&tx, None, 1, 141, 10);
        ledger.send(&tx, None, 1, 141, 10);
        ledger.send(&tx, Some(npc), 1, 85, 60);
        ledger.send(&tx, Some(npc), 1, 85, 60);
        assert_eq!(take().len(), 2);
        for (actor, expected) in [(None, 90), (Some(npc), 40)] {
            let mut old = StockVitalsEvent {
                health: 900,
                max_health: 1000,
                anger: 100,
            };
            ledger.project(actor, 0, &mut old);
            assert_eq!(old.anger, expected);
            assert_eq!(old.health, 900);
            ledger.project(actor, 1, &mut old);
            assert_eq!(
                old.anger, expected,
                "client projection already includes the debit"
            );
            assert!(ledger.0[&actor].pending.is_empty());
        }
    }
    use super::{
        apply_session_role_rate, authority_attack_block, collect_net_key_actions,
        decode_binary_frame, encode_control_frame, escort_carrier_id, handle_net_message,
        handle_v4_role_snapshot, handle_v4_snapshot, lerp_remote_yaw, local_request1_runtime,
        net_apply_authority_respawns, net_apply_snapshot, net_apply_stock_scores,
        net_apply_stock_vitals, net_handle_hit, net_handle_remote_hits, net_room_members_text,
        net_send_hits, net_send_pending_actions, net_smooth_remote_fighters,
        reconnect_delay_seconds, v6_action_input, v6_role_input, weapon_id_from_web_name,
        web_room_join, AuthorityHit, AuthorityWeaponDrop, EncodedNetFrame, NetChatInput,
        NetInbound, NetInputSequence, NetOutbound, NetRx, NetState, NetStatus, NetTx,
        NetWireFormat, PendingNetActions, RemoteAuthorityGeneration, RemoteContactSegment,
        RemoteFighter, RemoteHitMotionWindow, RemoteTransformTarget,
    };
    #[cfg(not(target_arch = "wasm32"))]
    use super::{
        apply_stock_discovered_room, cleanup_network_session, menu_stock_lan_discovery,
        net_send_stock_weapon_switch, normalize_stock_stage_for_rule, stock_config_value_label,
        stock_mode_from_rule, stock_stage_allowed_for_rule, validate_stock_selection,
        NativeNetThread, StockDiscoveredRoom, StockLanDiscoveryState, StockSceneItemPickup,
        StockServerTransport,
    };
    use crate::app_state::{AppState, LaunchRequest, MenuScreen, StockConfigField};
    use crate::components::fighter::{
        AiIntent, CombatState, DeathRespawn, EntryBlend, Fighter, FighterHitReaction, GunAimState,
        Health, LockOnController, MatchEnd, MotionState, PoseRuntime, Qi, SmState, TargetHitClock,
        WeaponSwitchActive,
    };
    use crate::plugins::camera_plugin::CameraBasis;
    use crate::plugins::movement_plugin::{
        FighterAir, FighterAttackMotion, FighterDodge, FighterWall,
    };
    use crate::plugins::stage_plugin::LocalFighter;
    use bevy::prelude::{
        App, ButtonInput, Entity, IntoScheduleConfigs, KeyCode, MouseButton, NextState, Quat, Time,
        Transform, Update, Vec3, With,
    };
    #[cfg(not(target_arch = "wasm32"))]
    use meteor_network::escort::SN03;
    use meteor_network::protocol::{
        CombatReactionWire, EscortFlagSnapshot, FighterSnapshot, Message, RoomMemberWire,
        RoomSummary, SceneOp, V4Payload,
    };
    use meteor_network::role_state::{
        encode_event2_state, role_state_from_runtime, yaw_from_forward, RoleStateRuntime, RoleVec3,
    };
    use meteor_network::wire_v4::{
        RoleSnapshot, RoleStateRecord, Snapshot, SnapshotDrop, SnapshotNpc, SnapshotPlayer,
        Vec2 as V4Vec2, Vec3 as V4Vec3,
    };

    #[cfg(not(target_arch = "wasm32"))]
    use super::{
        handle_stock_game_event, run_stock_host_connection, scene_ops_from_stock,
        stock_host_bind_address, stock_scene_ops_from_message, StockConnectionConfig,
        StockGameEvent, STOCK_SCENE_OP_COLLISION, STOCK_SCENE_OP_POSE,
    };

    #[test]
    fn vip_team_prefers_stock_camp_and_uses_v6_authority_metadata() {
        let mut net = NetState::default();
        net.stock_vips[1] = Some("stock-7".into());
        assert_eq!(net.vip_team("stock-7"), Some(2));

        net.members.push(RoomMemberWire {
            connected: true,
            player_id: "p1".into(),
            name: "VIP".into(),
            character_id: 0,
            weapon: 6,
            team: Some(1),
            ready: true,
        });
        net.radar_star_players.insert("p1".into());
        assert_eq!(net.player_team("p1"), Some(1));
        assert_eq!(net.vip_team("p1"), Some(1));
        assert_eq!(net.vip_team("ordinary"), None);
    }

    #[test]
    fn scene_prop_hit_receive_requires_current_scene_host() {
        let mut net = NetState {
            status: NetStatus::InRoom,
            room_id: "room1".into(),
            player_id: "p1".into(),
            match_epoch: Some(1),
            round_epoch: Some(2),
            round_active: true,
            members: ["p1", "p2"]
                .into_iter()
                .map(|id| RoomMemberWire {
                    player_id: id.into(),
                    name: id.into(),
                    character_id: 0,
                    weapon: 6,
                    team: Some(1),
                    connected: true,
                    ready: true,
                })
                .collect(),
            ..Default::default()
        };
        let hit = Message::ScenePropHit {
            room_id: "room1".into(),
            match_epoch: 1,
            round_epoch: 2,
            player_id: "p2".into(),
            reference: "D_BBox01".into(),
            pose_id: 262,
            frame: 9770.0,
            damage: 126,
        };
        let receive = |net: &mut NetState, message| {
            super::handle_net_message(
                net,
                &mut LaunchRequest::default(),
                &mut MatchEnd::default(),
                &mut NextState::default(),
                message,
                &mut NetTx::default(),
                &mut NetInputSequence::default(),
                &mut PendingNetActions::default(),
                AppState::Playing,
            );
        };
        receive(&mut net, hit.clone());
        assert_eq!(net.pending_scene_prop_hits, vec![hit.clone()]);
        net.pending_scene_prop_hits.clear();
        net.round_epoch = Some(3);
        receive(&mut net, hit.clone());
        assert!(net.pending_scene_prop_hits.is_empty());
        net.round_epoch = Some(2);
        net.player_id = "p2".into();
        receive(&mut net, hit.clone());
        assert!(net.pending_scene_prop_hits.is_empty());
        net.player_id = "p1".into();
        net.round_active = false;
        receive(&mut net, hit);
        assert!(net.pending_scene_prop_hits.is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_player_names_survive_both_event_orders_and_repeated_role_entry() {
        use meteor_network::stock_protocol::StockRoleEnteredEvent;
        for name_first in [true, false] {
            let mut net = NetState {
                player_id: "stock-1".into(),
                ..Default::default()
            };
            let mut request = LaunchRequest::default();
            let mut end = MatchEnd::default();
            let mut tx = NetTx::default();
            let mut next = NextState::default();
            let name = StockGameEvent::PlayerName {
                actor_id: 2,
                name: "德玛西亚".into(),
            };
            let entered = StockGameEvent::RoleEntered(StockRoleEnteredEvent {
                actor_id: 2,
                hero: 4,
                hero_checksum: 0,
                camp: 1,
            });
            let events = if name_first {
                vec![name, entered.clone()]
            } else {
                vec![entered.clone(), name]
            };
            for event in events.into_iter().chain([entered]) {
                handle_stock_game_event(
                    &mut net,
                    &mut request,
                    &mut end,
                    &mut tx,
                    &mut next,
                    AppState::Playing,
                    event,
                );
            }
            assert_eq!(net.remote_actor_names["stock-2"], "德玛西亚");
            assert_eq!(
                net.members
                    .iter()
                    .find(|m| m.player_id == "stock-2")
                    .unwrap()
                    .name,
                "德玛西亚"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_state_before_roster_waits_for_the_real_character() {
        let state = role_state_from_runtime(RoleStateRuntime {
            position: RoleVec3 {
                x: 12.0,
                y: 3.0,
                z: -9.0,
            },
            forward: RoleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            velocity: RoleVec3::ZERO,
            pose_id: 0,
            grounded: true,
            guard_route: false,
            playback_arg: 0,
            pose_sequence: 10,
        });
        let mut app = App::new();
        app.insert_resource(NetState {
            address: "udp://127.0.0.1:5230".into(),
            pending_role_states: vec![("stock-1".into(), state, 1)],
            ..NetState::default()
        })
        .add_systems(Update, net_apply_snapshot);
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&RemoteFighter>().iter(world).count(), 0);
        {
            let mut net = world.resource_mut::<NetState>();
            net.remote_actor_profiles.insert("stock-1".into(), (4, 6));
            net.pending_role_entries.push("stock-1".into());
        }
        app.update();
        app.update();
        let world = app.world_mut();
        let fighters: Vec<_> = world
            .query::<(&RemoteFighter, &Fighter, &Transform)>()
            .iter(world)
            .collect();
        assert_eq!(fighters.len(), 1);
        assert_eq!(fighters[0].1.character_id, 4);
        assert_eq!(fighters[0].2.translation, Vec3::new(12.0, 3.0, -9.0));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_weapon_event_updates_an_existing_idle_remote() {
        let mut app = App::new();
        app.insert_resource(NetState {
            address: "udp://127.0.0.1:5230".into(),
            pending_role_entries: vec!["stock-1".into()],
            remote_actor_profiles: [("stock-1".into(), (4, 0))].into(),
            ..NetState::default()
        })
        .add_systems(Update, net_apply_snapshot);
        app.update();

        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next = NextState::default();
        for (weapon, variant) in [(6, 0), (7, 1)] {
            super::handle_stock_game_event(
                &mut app.world_mut().resource_mut::<NetState>(),
                &mut request,
                &mut end,
                &mut tx,
                &mut next,
                AppState::Playing,
                meteor_network::stock_game::StockGameEvent::WeaponEquipped(
                    meteor_network::stock_protocol::StockWeaponEquippedEvent {
                        actor_id: 1,
                        weapon_item_id: meteor_network::stock_protocol::stock_weapon_item_id(
                            weapon, variant,
                        )
                        .unwrap(),
                    },
                ),
            );
            app.update();
            let world = app.world_mut();
            let fighter = world.query::<&Fighter>().single(world).unwrap();
            assert_eq!((fighter.weapon, fighter.weapon_variant), (weapon, variant));
            assert_eq!(fighter.weapon_slots[0], weapon);
            assert_eq!(fighter.weapon_slot_variants[0], variant);
            assert_eq!(
                world
                    .query::<&crate::components::fighter::WeaponVariantChanged>()
                    .iter(world)
                    .count(),
                1,
                "changing inventory must request the actual weapon model rebuild"
            );
        }
    }

    #[test]
    fn stock_remote_pose_uses_wall_morph_and_keeps_display_yaw_until_smoothing() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p0").unwrap();
        let start = character.pose_frames[&24].0 as f32;
        let mut app = App::new();
        app.insert_resource(NetState {
            address: "udp://127.0.0.1:5230".into(),
            pending_role_entries: vec!["stock-1".into()],
            remote_actor_profiles: [("stock-1".into(), (0, 5))].into(),
            ..NetState::default()
        })
        .insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
            [("p0".into(), character)].into(),
        ))
        .add_systems(Update, net_apply_snapshot);
        app.update();
        let state = role_state_from_runtime(RoleStateRuntime {
            position: RoleVec3::ZERO,
            forward: RoleVec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            velocity: RoleVec3 {
                x: 3.0,
                y: 0.0,
                z: 0.0,
            },
            pose_id: 24,
            grounded: true,
            guard_route: false,
            playback_arg: 1,
            pose_sequence: 5,
        });
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_role_states
            .push(("stock-1".into(), state, 1));
        app.update();
        let world = app.world_mut();
        let (pose, blend, motion, target) = world
            .query::<(
                &PoseRuntime,
                &crate::components::fighter::EntryBlend,
                &MotionState,
                &RemoteTransformTarget,
            )>()
            .single(world)
            .unwrap();
        assert_eq!(pose.pose_id, 24);
        assert_eq!(pose.frame, start);
        assert_eq!(blend.to_frame, start);
        assert_eq!(blend.duration, 0.1);
        assert_eq!(
            motion.facing, 0.0,
            "incoming quantized yaw must not snap the visible model"
        );
        assert!((target.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        let instance = pose.pose_instance_id;
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_role_states
            .push(("stock-1".into(), state, 2));
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query::<&PoseRuntime>()
                .single(world)
                .unwrap()
                .pose_instance_id,
            instance,
            "a repeated state must not restart the 0.1s transition"
        );
    }

    #[test]
    fn stock_dagger_forward_chain_keeps_both_network_poses_and_distinct_local_entry() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p4").unwrap();

        let mut local_pose = PoseRuntime::idle();
        character.apply_pose(&mut local_pose, 250, true);
        let mut local_blend = crate::components::fighter::EntryBlend::default();
        character.apply_pose_with_blend(
            &mut local_pose,
            &mut local_blend,
            253,
            true,
            character.poses[&250]
                .next_pose
                .as_ref()
                .map(|next| next.time),
            crate::components::fighter::EntryBlendKind::Chained,
            false,
            false,
            None,
        );
        assert_eq!(local_blend.duration, 0.3);
        assert_eq!(local_blend.to_frame, 9211.0);

        let mut app = App::new();
        app.insert_resource(NetState {
            address: "udp://127.0.0.1:5230".into(),
            pending_role_entries: vec!["stock-1".into()],
            remote_actor_profiles: [("stock-1".into(), (4, 5))].into(),
            ..NetState::default()
        })
        .insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
            [("p4".into(), character)].into(),
        ))
        .add_systems(Update, net_apply_snapshot);
        app.update();
        for (tick, pose_id, sequence, start) in [(1, 250, 710, 9070.0), (2, 253, 711, 9193.0)] {
            let state = role_state_from_runtime(RoleStateRuntime {
                position: RoleVec3::ZERO,
                forward: RoleVec3 {
                    x: 0.0,
                    y: 0.0,
                    z: 1.0,
                },
                velocity: RoleVec3::ZERO,
                pose_id,
                grounded: false,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: sequence,
            });
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_role_states
                .push(("stock-1".into(), state, tick));
            app.update();
            let world = app.world_mut();
            let (pose, blend) = world
                .query::<(&PoseRuntime, &crate::components::fighter::EntryBlend)>()
                .single(world)
                .unwrap();
            assert_eq!(
                pose.pose_id, pose_id,
                "captured chain edge must be accepted"
            );
            assert_eq!(blend.duration, 0.1);
            assert_eq!(
                blend.to_frame, start,
                "EXE network third playback argument is 1, not local chained 3"
            );
        }
    }

    #[test]
    fn stock_dagger_capture_replays_event51_and_continuous_state_fallback() {
        use crate::components::fighter::EntryBlend;
        use crate::plugins::character_plugin::{
            drive_fighter_animation_for_test, load_character_asset_for_test,
            spawn_fighter_model_for_test, CharacterAssetsSet, CommonAmb, FighterBone,
        };
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).unwrap(),
        )
        .unwrap();

        for (fps, mode) in [30_u32, 60, 144]
            .into_iter()
            .flat_map(|fps| (0..5).map(move |mode| (fps, mode)))
        {
            let discrete = mode != 0;
            let character =
                load_character_asset_for_test(std::slice::from_ref(&root), "p4").unwrap();
            let mut app = App::new();
            app.init_resource::<Time>()
                .init_resource::<Time<bevy::time::Real>>()
                .insert_resource(NetState {
                    address: "udp://127.0.0.1:5230".into(),
                    pending_role_entries: vec!["stock-1".into()],
                    remote_actor_profiles: [("stock-1".into(), (4, 5))].into(),
                    ..NetState::default()
                })
                .insert_resource(CharacterAssetsSet([("p4".into(), character)].into()))
                .insert_resource(CommonAmb(common.clone()))
                .add_systems(Update, net_apply_snapshot);
            app.update();
            let world = app.world_mut();
            let fighter = world
                .query_filtered::<Entity, With<RemoteFighter>>()
                .single(world)
                .unwrap();
            spawn_fighter_model_for_test(world, fighter);
            drive_fighter_animation_for_test(world);
            app.add_systems(
                Update,
                (
                    super::super::combat_plugin::advance_poses,
                    drive_fighter_animation_for_test,
                )
                    .chain()
                    .after(net_apply_snapshot),
            );
            let mut next_packet_ms = 0.0;
            let mut last_pose = 0;
            let mut previous_bones = std::collections::HashMap::new();
            let mut before_245 = 0.0;
            let mut blend_samples = 0;
            for step in 0..=(fps * 3 / 2) {
                let now_ms = step as f64 * 1000.0 / fps as f64;
                let (pose_id, sequence) = if now_ms < 499.0 {
                    (250, 710)
                } else if now_ms < 1300.0 {
                    (253, 711)
                } else {
                    (245, 712)
                };
                let edge = pose_id != last_pose;
                if edge || now_ms >= next_packet_ms {
                    if edge && pose_id == 245 {
                        before_245 = app.world().get::<PoseRuntime>(fighter).unwrap().frame;
                    }
                    let state = role_state_from_runtime(RoleStateRuntime {
                        position: RoleVec3::ZERO,
                        forward: RoleVec3 {
                            x: 0.0,
                            y: 0.0,
                            z: 1.0,
                        },
                        velocity: RoleVec3::ZERO,
                        pose_id,
                        grounded: false,
                        guard_route: false,
                        playback_arg: 0,
                        pose_sequence: sequence,
                    });
                    if discrete && edge && pose_id != 250 {
                        if mode == 2 || mode == 4 {
                            let mut net = app.world_mut().resource_mut::<NetState>();
                            let tick = net.last_role_snapshot_tick.unwrap_or(0).wrapping_add(1);
                            net.last_role_snapshot_tick = Some(tick);
                            net.pending_role_states
                                .push(("stock-1".into(), state, tick));
                        }
                        let bytes = if pose_id == 253 {
                            [1, 0, 253, 0, 0, 0, 199, 2, 30, 0]
                        } else {
                            [1, 0, 245, 0, 0, 0, 200, 2, 10, 0]
                        };
                        let event =
                            meteor_network::stock_protocol::StockPoseChainEvent::decode(&bytes)
                                .unwrap();
                        if mode >= 3 {
                            let mut net = app.world_mut().resource_mut::<NetState>();
                            let tick = net.last_role_snapshot_tick.unwrap_or(0).wrapping_add(1);
                            net.last_role_snapshot_tick = Some(tick);
                            let message = Message::RolePoseChain {
                                seq: tick,
                                room_id: "r".into(),
                                player_id: "stock-1".into(),
                                match_epoch: 1,
                                round_epoch: 1,
                                tick,
                                chain: event.into(),
                            };
                            let Message::RolePoseChain { chain, .. } =
                                Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap()
                            else {
                                panic!()
                            };
                            net.pending_pose_chains
                                .push(("stock-1".into(), chain, tick));
                        } else {
                            super::handle_stock_game_event(
                                &mut app.world_mut().resource_mut::<NetState>(),
                                &mut LaunchRequest::default(),
                                &mut MatchEnd::default(),
                                &mut NetTx::default(),
                                &mut NextState::default(),
                                AppState::Playing,
                                StockGameEvent::PoseChain(event),
                            );
                        }
                    }
                    let mut net = app.world_mut().resource_mut::<NetState>();
                    let tick = net.last_role_snapshot_tick.unwrap_or(0).wrapping_add(1);
                    net.last_role_snapshot_tick = Some(tick);
                    net.pending_role_states
                        .push(("stock-1".into(), state, tick));
                    next_packet_ms = now_ms + 50.0;
                }
                let dt = std::time::Duration::from_secs_f64(1.0 / fps as f64);
                app.world_mut().resource_mut::<Time>().advance_by(dt);
                app.world_mut()
                    .resource_mut::<Time<bevy::time::Real>>()
                    .advance_by(dt);
                app.update();
                let world = app.world_mut();
                let current_bones: std::collections::HashMap<_, _> = world
                    .query_filtered::<(Entity, &Transform), With<FighterBone>>()
                    .iter(world)
                    .map(|(entity, transform)| (entity, transform.rotation))
                    .collect();
                assert!(
                    !current_bones.is_empty(),
                    "real p4 skeleton must be sampled"
                );
                if edge && pose_id == 253 && fps == 60 {
                    let character = &world.resource::<CharacterAssetsSet>().0["p4"];
                    let amb = character.amb_for_pose(&world.resource::<CommonAmb>().0, 253);
                    let start = amb.sample_at(9193.0);
                    let action = amb.sample_at(9211.0);
                    let mut turns: Vec<_> = world
                        .query::<(Entity, &FighterBone)>()
                        .iter(world)
                        .filter_map(|(entity, bone)| {
                            let from = current_bones[&entity].normalize();
                            let start = Quat::from_array(*start.bone_quats.get(bone.skin_index)?)
                                .normalize();
                            let action = Quat::from_array(*action.bone_quats.get(bone.skin_index)?)
                                .normalize();
                            Some((
                                bone.name.clone(),
                                from.angle_between(start).to_degrees(),
                                from.angle_between(action).to_degrees(),
                            ))
                        })
                        .collect();
                    turns.sort_by(|a, b| b.1.total_cmp(&a.1));
                    for (bone, to_start, to_action) in turns.iter().take(4) {
                        println!("250->253 bone {bone}: network Start turn={to_start:.1}deg in 0.1s; Action target turn={to_action:.1}deg (same source sample)");
                    }
                }
                if edge && !previous_bones.is_empty() {
                    for (entity, rotation) in &current_bones {
                        let previous: &Quat = &previous_bones[entity];
                        assert!(rotation.normalize().dot(previous.normalize()).abs() > 0.99999,
                            "first morph sample must preserve displayed bones at {fps} fps, pose {pose_id}");
                    }
                }
                let blend = world.get::<EntryBlend>(fighter).unwrap();
                if edge && discrete && pose_id == 253 {
                    assert!((blend.duration - 0.3).abs() < 0.00001);
                    assert_eq!(
                        world.resource::<NetState>().remote_pose_sequences["stock-1"].1,
                        711
                    );
                }
                if pose_id == 253 && blend.duration > 0.0 {
                    blend_samples += 1;
                    assert_eq!(
                        world.get::<PoseRuntime>(fighter).unwrap().frame,
                        if discrete { 9211.0 } else { 9193.0 }
                    );
                }
                previous_bones = current_bones;
                last_pose = pose_id;
            }
            println!("dagger capture {fps} fps mode={mode}: 253 last frame before 245={before_245:.3}, blend samples={blend_samples}");
            assert!(blend_samples >= 3);
            if discrete {
                assert!(
                    before_245 >= 9225.0,
                    "event51 must reach the Action tail at {fps} fps, got {before_245}"
                );
            } else {
                assert!(
                    before_245 >= 9215.0 && before_245 < 9225.0,
                    "event2 alone reproduces the interrupted Action tail"
                );
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_join_sends_menu_name_in_the_connect_datagram() {
        use meteor_network::stock_udp::{StockConnectRecord, StockControlPacket};
        let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let address = server.local_addr().unwrap().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        let (to_app, _from_net) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            super::run_stock_connection_with_password(
                &address,
                Message::Hello {
                    name: "Rust测试".into(),
                    resume_token: None,
                    character_id: 4,
                    weapon: 6,
                },
                0,
                " secret123 ",
                rx,
                to_app,
            )
        });
        let mut buffer = [0u8; 1400];
        let cookie = 0x1122_3344;
        let received = (|| -> std::io::Result<usize> {
            let (_, peer) = server.recv_from(&mut buffer)?;
            server.send_to(
                &StockControlPacket::Challenge { cookie }.encode(0).unwrap(),
                peer,
            )?;
            server.recv_from(&mut buffer).map(|(length, _)| length)
        })();
        let _ = tx.send(NetOutbound::Quit);
        worker.join().unwrap();
        let length = received.expect("production client must answer the challenge");
        let StockControlPacket::Connect { record, .. } =
            StockControlPacket::decode(&buffer[..length], Some(cookie)).unwrap()
        else {
            panic!("expected type-2 connect");
        };
        let decoded = StockConnectRecord::decode(&record).unwrap();
        assert_eq!(decoded.player_name, "Rust测试");
        assert_eq!(decoded.password, " secret123 ");
        assert_eq!(decoded.cdkey, meteor_network::stock_udp::STOCK_CDKEY_BYPASS);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "requires an explicitly supplied live Meteor.exe LAN host"]
    fn stock_live_original_host_keeps_the_rust_player_name() {
        use meteor_network::stock_protocol::{decode_stock_roster, STOCK_EVENT_ROSTER};
        use meteor_network::stock_session::{StockClientTransport, StockSessionEvent};
        let address =
            std::env::var("METEOR_STOCK_TEST_ADDRESS").expect("supply live server address");
        let target = address.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let (to_app, from_net) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            super::run_stock_connection(
                &target,
                Message::Hello {
                    name: "Rust测试".into(),
                    resume_token: None,
                    character_id: 4,
                    weapon: 6,
                },
                0,
                rx,
                to_app,
            )
        });
        let started = std::time::Instant::now();
        let mut actor = None;
        let mut entered = false;
        let mut observer: Option<StockClientTransport> = None;
        let mut found = None;
        let mut errors = Vec::new();
        while started.elapsed().as_secs() < 10 && found.is_none() && errors.is_empty() {
            while let Ok(event) = from_net.try_recv() {
                match event {
                    NetInbound::StockGame(StockGameEvent::TransportConnected {
                        transport_id,
                        ..
                    }) => actor = Some(transport_id),
                    NetInbound::StockGame(StockGameEvent::RoleEntered(role)) => {
                        entered |= Some(role.actor_id) == actor
                    }
                    NetInbound::Closed { reason, .. } => errors.push(reason),
                    _ => {}
                }
            }
            if entered && observer.is_none() {
                match StockClientTransport::bind(
                    "127.0.0.1:0",
                    &address,
                    super::stock_connect_record("名字观察"),
                ) {
                    Ok(client) => observer = Some(client),
                    Err(error) => errors.push(error.to_string()),
                }
            }
            if let Some(client) = observer.as_mut() {
                match client.tick(started.elapsed().as_millis() as u64) {
                    Ok(events) => {
                        for event in events {
                            if let StockSessionEvent::Application { packet, .. } = event {
                                if packet.opcode == STOCK_EVENT_ROSTER {
                                    match decode_stock_roster(&packet.payload) {
                                        Ok(members) => {
                                            found = members
                                                .into_iter()
                                                .find(|member| Some(member.actor_id) == actor)
                                        }
                                        Err(error) => errors.push(error.to_string()),
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => errors.push(error.to_string()),
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        if let Some(client) = observer.as_mut() {
            let _ = client.disconnect(0);
        }
        let _ = tx.send(NetOutbound::Quit);
        worker.join().unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let member = found
            .expect("original host must include the first Rust client in the observer's roster");
        println!(
            "original host roster: actor={} hero={} name={:?}",
            member.actor_id, member.hero, member.name
        );
        assert_eq!(member.name, "Rust测试");
        assert_eq!(member.hero, 4);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    #[ignore = "requires an explicitly supplied live Meteor.exe LAN host"]
    fn stock_live_original_host_preserves_scene_coordinates_between_clients() {
        let address =
            std::env::var("METEOR_STOCK_TEST_ADDRESS").expect("supply live server address");
        let mut senders = Vec::new();
        let mut receivers = Vec::new();
        let mut handles = Vec::new();
        for hero in [0, 9] {
            let (tx, rx) = std::sync::mpsc::channel();
            let (to_app, from_net) = std::sync::mpsc::channel();
            let target = address.clone();
            handles.push(std::thread::spawn(move || {
                super::run_stock_connection(
                    &target,
                    Message::Hello {
                        name: "LAN coordinate check".into(),
                        resume_token: None,
                        character_id: hero,
                        weapon: 6,
                    },
                    0,
                    rx,
                    to_app,
                )
            }));
            senders.push(tx);
            receivers.push(from_net);
        }
        let start = std::time::Instant::now();
        let mut actors = [None, None];
        let mut ready = [false, false];
        let mut seen = [false; 3];
        let mut observed_positions = [false; 3];
        let mut monk_seen = false;
        let mut host_state = None;
        let mut points = Vec::new();
        let mut last_send = 0;
        let mut errors = Vec::new();
        while start.elapsed().as_secs() < 12 && !seen.iter().all(|seen| *seen) {
            for (index, receiver) in receivers.iter().enumerate() {
                while let Ok(message) = receiver.try_recv() {
                    match message {
                        NetInbound::StockGame(StockGameEvent::TransportConnected {
                            transport_id,
                            ..
                        }) => actors[index] = Some(transport_id),
                        NetInbound::StockGame(StockGameEvent::RoleEntered(role)) => {
                            monk_seen |= role.hero == 4;
                            ready[index] |= Some(role.actor_id) == actors[index];
                        }
                        NetInbound::StockGame(StockGameEvent::RoleStates(records)) => {
                            for record in records {
                                if record.actor_id == 1 {
                                    host_state = Some(record.state);
                                }
                                if index == 0 && Some(u32::from(record.actor_id)) == actors[1] {
                                    for (n, expected) in points.iter().enumerate() {
                                        let expected: &meteor_network::role_state::RoleState =
                                            expected;

                                        let source_x =
                                            (expected.forward.x * 1000.0 + 0.5).trunc() * 0.001;
                                        let source_y =
                                            (-expected.forward.z * 1000.0 + 0.5).trunc() * 0.001;
                                        let angle = source_x.clamp(-1.0, 1.0).acos()
                                            * if source_y > 0.0 { 1.0 } else { -1.0 };
                                        let wire_angle =
                                            (((angle * 10.0 + 0.5) as i32) & 63) as f32 * 0.1;
                                        let expected_forward = RoleVec3 {
                                            x: wire_angle.cos(),
                                            y: 0.0,
                                            z: -wire_angle.sin(),
                                        };
                                        let actual = record.state;
                                        if (actual.position.x - expected.position.x).abs() < 0.1
                                            && (actual.position.y - expected.position.y).abs() < 0.1
                                            && (actual.position.z - expected.position.z).abs() < 0.1
                                        {
                                            if !observed_positions[n] {
                                                println!("point {n}: {:?} facing {:?}; expected {:?} facing {:?}", actual.position, actual.forward, expected.position, expected_forward);
                                                observed_positions[n] = true;
                                            }
                                            seen[n] |= actual.forward.x * expected_forward.x
                                                + actual.forward.z * expected_forward.z
                                                > 0.9999;
                                        }
                                    }
                                }
                            }
                        }
                        NetInbound::Closed { reason, .. } => errors.push(reason),
                        _ => {}
                    }
                }
            }
            if points.is_empty() && ready.iter().all(|ready| *ready) {
                if let Some(base) = host_state {
                    for (x, y, z, yaw) in [
                        (6.0, 0.0, 0.0, 0.0),
                        (9.0, 1.2, -6.0, 1.5),
                        (-6.0, 0.0, 9.0, 3.0),
                    ] {
                        let mut point = base;
                        point.position.x += x;
                        point.position.y += y;
                        point.position.z += z;
                        point.forward = meteor_network::role_state::forward_from_yaw(yaw);
                        point.velocity = RoleVec3::ZERO;
                        points.push(point);
                    }
                }
            }
            let now = start.elapsed().as_millis() as u64;
            if !points.is_empty() && now - last_send >= 50 {
                last_send = now;
                let point = points[(now / 500 % 3) as usize];
                let _ = senders[1].send(NetOutbound::StockRoleState(point));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        for tx in senders {
            let _ = tx.send(NetOutbound::Quit);
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(errors.is_empty(), "{errors:?}");
        assert!(
            monk_seen,
            "original monk must be identified from the member roster"
        );
        assert!(
            seen.iter().all(|seen| *seen),
            "confirmed={seen:?}, actors={actors:?}, ready={ready:?}, host_state={host_state:?}"
        );
    }

    fn fighter_snapshot(movement_mode: u8) -> FighterSnapshot {
        FighterSnapshot {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            yaw: 0.75,
            pose_id: 200,
            pose_frame: 12,
            pose_start: 0,
            pose_end: 30,
            pose_loop_start: 0,
            pose_loop_end: 30,
            pose_loops: false,
            hp: 900.0,
            max_hp: 1000.0,
            weapon: 6,
            movement_mode,
            kills: 0,
        }
    }

    #[test]
    fn discrete_network_keys_queue_current_round_actions() {
        let mut app = App::new();
        let net = NetState {
            status: NetStatus::InRoom,
            round_active: true,
            ..NetState::default()
        };
        app.insert_resource(net)
            .insert_resource(crate::app_state::Keymap::default())
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<PendingNetActions>()
            .add_systems(Update, collect_net_key_actions);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyH);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyF);
        app.update();
        assert_eq!(
            app.world_mut().resource_mut::<PendingNetActions>().take(),
            vec!["qi_burst", "drop_item"]
        );
    }

    #[test]
    fn ordered_mobile_drop_edges_keep_original_h_then_g_order() {
        use crate::plugins::input_plugin::{LocalCombatInput, LocalCombatInputEdge};

        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            round_active: true,
            ..NetState::default()
        })
        .insert_resource(crate::app_state::Keymap::default())
        .insert_resource(LocalCombatInput {
            edges: vec![
                LocalCombatInputEdge::DropItem,
                LocalCombatInputEdge::DropWeapon,
            ],
            ..Default::default()
        })
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<PendingNetActions>()
        .add_systems(Update, collect_net_key_actions);

        app.update();

        assert_eq!(
            app.world_mut().resource_mut::<PendingNetActions>().take(),
            vec!["drop_item", "drop_weapon"]
        );
    }

    #[test]
    fn network_input_and_character_contact_schedule_has_no_cycle() {
        use bevy::ecs::schedule::Schedules;
        use bevy::state::app::AppExtStates;

        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<AppState>();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.add_plugins((
            crate::plugins::InputPlugin,
            super::NetworkPlugin,
            crate::plugins::CharacterContactPlugin,
        ));

        let mut update = app
            .world_mut()
            .resource_mut::<Schedules>()
            .remove(Update)
            .expect("Update schedule must exist");
        let result = update.initialize(app.world_mut());
        app.world_mut().resource_mut::<Schedules>().insert(update);

        if let Err(error) = result {
            panic!("Update schedule must be acyclic: {error}");
        }
    }

    #[test]
    fn current_web_protobuf_is_the_default_transport() {
        assert_eq!(NetWireFormat::default(), NetWireFormat::V4);
    }

    #[test]
    fn browser_and_desktop_share_the_v6_frame_codec() {
        let frame = encode_control_frame(
            NetWireFormat::V4,
            Message::Ping {
                client_time: Some(12.5),
            },
        )
        .unwrap();
        let EncodedNetFrame::Binary(bytes) = frame else {
            panic!("v6 control must be binary");
        };
        let decoded = decode_binary_frame(NetWireFormat::V4, &bytes).unwrap();
        assert!(matches!(
            decoded,
            Some(NetInbound::Control(Message::Ping {
                client_time: Some(value)
            })) if (value - 12.5).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn reconnect_backoff_matches_web_and_stops_after_six_attempts() {
        assert_eq!(
            (0..=6).map(reconnect_delay_seconds).collect::<Vec<_>>(),
            vec![
                Some(0.4),
                Some(0.8),
                Some(1.6),
                Some(3.0),
                Some(3.0),
                Some(3.0),
                None,
            ]
        );
    }

    #[test]
    fn resumed_room_keeps_the_higher_local_input_watermark() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut tx = NetTx {
            active: None,
            connecting: Some(sender),
        };
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: "p1".into(),
            room_id: "room1".into(),
            match_epoch: Some(7),
            round_epoch: Some(3),
            session_token: "old-token".into(),
            resume_requested: true,
            ..NetState::default()
        };
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut next = NextState::<AppState>::default();
        let mut sequence = NetInputSequence(12);
        let mut pending = PendingNetActions::default();
        pending.push("drop_item");

        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::Welcome {
                player_id: String::new(),
                room_id: String::new(),
                members: Vec::new(),
                client_id: "c1".into(),
                session_token: "new-token".into(),
                resumed: true,
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::Playing,
        );
        assert!(net.transport_connected);
        assert_eq!(net.player_id, "p1");
        assert_eq!(net.session_token, "new-token");
        assert!(tx.active.is_some());
        assert!(tx.connecting.is_none());
        assert!(
            receiver.try_recv().is_err(),
            "resumed welcome skips room.list"
        );

        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::RoomJoined {
                room_id: "room1".into(),
                player_id: "p1".into(),
                match_epoch: Some(7),
                round_epoch: Some(3),
                mode: Some("deathmatch".into()),
                resumed: true,
                last_processed_input_seq: 10,
                room: serde_json::Value::Null,
                scene_state: Vec::new(),
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::Playing,
        );
        assert_eq!(sequence.0, 12);
        net.dynamic_weapon_drops.insert(
            "D_wpNet1".into(),
            AuthorityWeaponDrop {
                reference: "D_wpNet1".into(),
                model: "w1_0".into(),
                position: [1.0, 0.0, 0.0],
                velocity: None,
                settled: true,
                pickup_ready: true,
            },
        );
        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: Some(7),
                round_epoch: Some(3),
                mode: "deathmatch".into(),
                stage: "sn01".into(),
                players: Vec::new(),
                slots: Vec::new(),
                resumed: true,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::Playing,
        );
        assert_eq!(sequence.0, 12);
        assert_eq!(pending.take(), vec!["drop_item"]);
        assert!(net.dynamic_weapon_drops.contains_key("D_wpNet1"));
    }

    #[test]
    fn fresh_match_resets_input_sequence_and_pending_actions() {
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: "p1".into(),
            ..NetState::default()
        };
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut next = NextState::<AppState>::default();
        let mut tx = NetTx::default();
        let mut sequence = NetInputSequence(91);
        let mut pending = PendingNetActions::default();
        pending.push("cure");
        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: Some(8),
                round_epoch: Some(1),
                mode: "deathmatch".into(),
                stage: "sn01".into(),
                players: Vec::new(),
                slots: Vec::new(),
                resumed: false,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::NetRoom,
        );
        assert_eq!(sequence.0, 0);
        assert!(pending.take().is_empty());
    }

    #[test]
    fn ws_player_left_despawns_remote_and_discards_queued_snapshot() {
        let mut app = App::new();
        app.insert_resource(NetState::default())
            .add_systems(Update, net_apply_snapshot);
        let remote = app
            .world_mut()
            .spawn(RemoteFighter {
                player_id: "p2".into(),
            })
            .id();
        let local = app.world_mut().spawn(LocalFighter).id();
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut next = NextState::default();
        let mut tx = NetTx::default();
        let mut sequence = NetInputSequence::default();
        let mut actions = PendingNetActions::default();
        {
            let mut net = app.world_mut().resource_mut::<NetState>();
            net.player_id = "p1".into();
            net.pending_snapshots.push((
                "p2".into(),
                FighterSnapshot {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    yaw: 0.0,
                    pose_id: 0,
                    pose_frame: 0,
                    pose_start: 0,
                    pose_end: 1,
                    pose_loop_start: 0,
                    pose_loop_end: 1,
                    pose_loops: true,
                    hp: 100.0,
                    max_hp: 100.0,
                    weapon: 6,
                    movement_mode: 0,
                    kills: 0,
                },
                None,
            ));
            net.remote_actor_names.insert("p2".into(), "remote".into());
            for _ in 0..2 {
                handle_net_message(
                    &mut net,
                    &mut request,
                    &mut end,
                    &mut next,
                    Message::RoomPlayerLeft {
                        player_id: "p2".into(),
                    },
                    &mut tx,
                    &mut sequence,
                    &mut actions,
                    AppState::Playing,
                );
            }
            assert_eq!(net.pending_remote_despawns, ["p2"]);
            assert!(net.pending_snapshots.is_empty());
        }
        app.update();
        assert!(app.world().get_entity(remote).is_err());
        assert!(app.world().get_entity(local).is_ok());
        assert!(!app
            .world()
            .resource::<NetState>()
            .remote_actor_names
            .contains_key("p2"));
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&RemoteFighter>()
                .iter(app.world())
                .count(),
            0
        );
    }

    #[test]
    fn rejected_resume_drops_stale_epoch_and_returns_to_room_browser() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut tx = NetTx {
            active: None,
            connecting: Some(sender),
        };
        let mut net = NetState {
            status: NetStatus::InRoom,
            room_id: "old-room".into(),
            match_epoch: Some(9),
            round_epoch: Some(2),
            session_token: "expired".into(),
            resume_requested: true,
            round_active: true,
            ..NetState::default()
        };
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut next = NextState::<AppState>::default();
        let mut sequence = NetInputSequence(77);
        let mut pending = PendingNetActions::default();
        pending.push("drop_weapon");
        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::Welcome {
                player_id: "p-new".into(),
                room_id: String::new(),
                members: Vec::new(),
                client_id: "c-new".into(),
                session_token: "fresh".into(),
                resumed: false,
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::Playing,
        );
        assert_eq!(net.status, NetStatus::Disconnected);
        assert!(net.room_id.is_empty());
        assert_eq!(net.match_epoch, None);
        assert_eq!(net.round_epoch, None);
        assert!(!net.round_active);
        assert_eq!(sequence.0, 0);
        assert!(pending.take().is_empty());
        assert!(matches!(receiver.try_recv(), Ok(NetOutbound::Quit)));
    }

    #[test]
    fn v6_action_and_role_inputs_are_strictly_separated() {
        let action = v6_action_input(4, vec!["cure".into()], Some(7), Some(3));
        assert!(action.role_state.is_empty());
        assert_eq!(action.actions, ["cure"]);
        assert!(action.combat.is_empty());
        assert!(action.visual.is_none());
        assert_eq!(action.r#move, Some(V4Vec2 { x: 0.0, z: 0.0 }));
        assert_eq!(action.match_epoch, Some(7));
        assert_eq!(action.round_epoch, Some(3));

        assert_eq!(
            V4Payload::Input(action.clone()).to_v4_binary().unwrap(),
            decode_hex("0806520e0804220042046375726558076003")
        );

        let role = v6_role_input(5, vec![1; 20], Some(17), Some(7), Some(3));
        assert_eq!(role.role_state, vec![1; 20]);
        assert_eq!(role.role_rate_request, Some(17));
        assert!(role.actions.is_empty());
        assert!(role.combat.is_empty());
        assert!(role.visual.is_none());
        assert_eq!(role.r#move, Some(V4Vec2 { x: 0.0, z: 0.0 }));
        assert_eq!(role.yaw, 0.0);
        assert!(!role.run);
        assert!(!role.guard);
        assert_eq!(
            V4Payload::Input(role).to_v4_binary().unwrap(),
            decode_hex("080652200805121401010101010101010101010101010101010101011811220058076003")
        );
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = (pair[0] as char).to_digit(16).unwrap() as u8;
                let low = (pair[1] as char).to_digit(16).unwrap() as u8;
                (high << 4) | low
            })
            .collect()
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_pose_chain_uplink_consumes_commit_once_and_discards_old_entries() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            address: "udp://127.0.0.1:5230".into(),
            round_active: true,
            ..NetState::default()
        })
        .insert_resource(NetTx {
            active: Some(sender),
            connecting: None,
        })
        .init_resource::<NetInputSequence>()
        .add_systems(Update, super::net_send_pose_chains);
        let entity = app
            .world_mut()
            .spawn((
                LocalFighter,
                PoseRuntime {
                    pose_id: 253,
                    pose_instance_id: 711,
                    ..PoseRuntime::default()
                },
                EntryBlend {
                    pending_local_chain: Some((711, 253, 0.3)),
                    ..EntryBlend::default()
                },
            ))
            .id();
        app.update();
        let NetOutbound::StockPoseChain(request) = receiver.try_recv().unwrap() else {
            panic!("expected a discrete pose entry");
        };
        assert_eq!(request.encode(), [253, 0, 0, 0, 199, 2, 30, 0]);
        app.update();
        assert!(
            receiver.try_recv().is_err(),
            "same instance must not restart every frame"
        );

        app.world_mut()
            .get_mut::<PoseRuntime>(entity)
            .unwrap()
            .pose_instance_id = 65536;
        app.world_mut()
            .get_mut::<EntryBlend>(entity)
            .unwrap()
            .pending_local_chain = Some((65536, 253, 0.15));
        app.update();
        let NetOutbound::StockPoseChain(request) = receiver.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(request.encode(), [253, 0, 0, 0, 0, 0, 15, 0]);
        app.world_mut()
            .get_mut::<EntryBlend>(entity)
            .unwrap()
            .pending_local_chain = Some((711, 253, 0.3));
        app.update();
        assert!(
            receiver.try_recv().is_err(),
            "an interrupted instance must not be replayed"
        );
        app.world_mut().resource_mut::<NetState>().round_active = false;
        app.world_mut()
            .get_mut::<EntryBlend>(entity)
            .unwrap()
            .pending_local_chain = Some((65536, 253, 0.3));
        app.update();
        app.world_mut().resource_mut::<NetState>().round_active = true;
        app.update();
        assert!(
            receiver.try_recv().is_err(),
            "entering a round must not flush an old commit"
        );
    }

    #[test]
    fn web_self_effect_sends_once_at_action_and_waits_for_authority() {
        use crate::components::fighter::FighterStatusEffects;
        let (sender, receiver) = std::sync::mpsc::channel();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p0").unwrap();
        let marker = character.poses[&489].action_entry_frame as f32;
        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, 489, true);
        pose.frame = marker - 1.0;
        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            room_id: "room1".into(),
            round_active: true,
            match_epoch: Some(7),
            round_epoch: Some(3),
            ..NetState::default()
        })
        .insert_resource(NetTx {
            active: Some(sender),
            connecting: None,
        })
        .insert_resource(NetInputSequence(100))
        .insert_resource(crate::plugins::combat_plugin::CombatResourceData(
            meteor_asset::res::parse_meteor_res(include_bytes!(
                "../../../../../origin_res/Meteor.res"
            ))
            .unwrap()
            .combat_table(),
        ))
        .insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
            std::collections::HashMap::from([("p0".into(), character)]),
        ))
        .add_systems(
            Update,
            crate::plugins::status_plugin::apply_self_attack_effect,
        );
        let entity = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 11,
                    weapon_slots: [11, 6],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                pose,
                Health {
                    current: 1000.0,
                    max: 1000.0,
                },
            ))
            .id();
        app.update();
        assert!(receiver.try_recv().is_err());
        app.world_mut()
            .get_mut::<PoseRuntime>(entity)
            .unwrap()
            .frame = marker;
        app.update();
        let NetOutbound::Control(message) = receiver.try_recv().unwrap() else {
            panic!("missing self effect request")
        };
        assert!(matches!(
            message,
            Message::RoleSelfEffect {
                seq: 101,
                match_epoch: 7,
                round_epoch: 3,
                pose_id: 489,
                ..
            }
        ));
        assert_eq!(
            app.world()
                .get::<FighterStatusEffects>(entity)
                .unwrap()
                .effects
                .attack_bonus(),
            0,
            "only the server response may activate online state"
        );
        app.update();
        assert!(
            receiver.try_recv().is_err(),
            "one request per Action instance"
        );
    }

    #[test]
    fn web_pose_chain_uplink_and_receive_preserve_fields_and_reject_stale_events() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            room_id: "room1".into(),
            round_active: true,
            match_epoch: Some(7),
            round_epoch: Some(3),
            ..NetState::default()
        })
        .insert_resource(NetTx {
            active: Some(sender),
            connecting: None,
        })
        .insert_resource(NetInputSequence(100))
        .add_systems(Update, super::net_send_pose_chains);
        let entity = app
            .world_mut()
            .spawn((
                LocalFighter,
                PoseRuntime {
                    pose_id: 253,
                    pose_instance_id: 65536,
                    ..PoseRuntime::default()
                },
                EntryBlend {
                    pending_local_chain: Some((65536, 253, 0.3)),
                    ..EntryBlend::default()
                },
            ))
            .id();
        app.update();
        let NetOutbound::Control(mut message) = receiver.try_recv().unwrap() else {
            panic!("missing Web chain");
        };
        assert!(matches!(&message, Message::RolePoseChain {
            seq: 101, match_epoch: 7, round_epoch: 3, chain, ..
        } if chain.sequence == 0 && chain.pose_id == 253 && chain.blend_seconds == 0.3));
        app.update();
        assert!(receiver.try_recv().is_err());
        app.world_mut().resource_mut::<NetState>().round_active = false;
        app.world_mut()
            .get_mut::<EntryBlend>(entity)
            .unwrap()
            .pending_local_chain = Some((65536, 253, 0.3));
        app.update();
        app.world_mut().resource_mut::<NetState>().round_active = true;
        app.update();
        assert!(receiver.try_recv().is_err());

        let mut net = NetState {
            status: NetStatus::InRoom,
            room_id: "room1".into(),
            player_id: "p1".into(),
            round_active: true,
            match_epoch: Some(7),
            round_epoch: Some(3),
            members: vec![serde_json::from_value(serde_json::json!({"player_id":"p2"})).unwrap()],
            ..NetState::default()
        };
        if let Message::RolePoseChain {
            player_id, tick, ..
        } = &mut message
        {
            *player_id = "p2".into();
            *tick = 15;
        }
        let deliver = |net: &mut NetState, message: Message| {
            handle_net_message(
                net,
                &mut LaunchRequest::default(),
                &mut MatchEnd::default(),
                &mut NextState::default(),
                Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap(),
                &mut NetTx {
                    active: None,
                    connecting: None,
                },
                &mut NetInputSequence::default(),
                &mut PendingNetActions::default(),
                AppState::Playing,
            );
        };
        deliver(&mut net, message.clone());
        deliver(&mut net, message.clone());
        assert_eq!(net.pending_pose_chains.len(), 1);
        assert_eq!(net.pending_pose_chains[0].1.blend_seconds, 0.3);
        for bad in 0..4 {
            let mut bad_message = message.clone();
            if let Message::RolePoseChain {
                room_id,
                player_id,
                match_epoch,
                round_epoch,
                tick,
                ..
            } = &mut bad_message
            {
                *tick = 16;
                match bad {
                    0 => *room_id = "other".into(),
                    1 => *player_id = "unknown".into(),
                    2 => *match_epoch = 6,
                    _ => *round_epoch = 2,
                }
            }
            deliver(&mut net, bad_message);
        }
        assert_eq!(net.pending_pose_chains.len(), 1);

        if let Message::RolePoseChain { tick, chain, .. } = &mut message {
            *tick = 16;
            chain.sequence = 1;
        }
        deliver(&mut net, message);
        assert_eq!(net.pending_pose_chains.len(), 2);
    }

    #[test]
    fn production_action_uplink_flushes_without_waiting_for_request1_timer() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut pending = PendingNetActions::default();
        pending.push("drop_item");
        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            round_active: true,
            wire_format: NetWireFormat::V4,
            match_epoch: Some(7),
            round_epoch: Some(3),
            ..NetState::default()
        })
        .insert_resource(NetTx {
            active: Some(sender),
            connecting: None,
        })
        .insert_resource(NetInputSequence::default())
        .insert_resource(pending)
        .add_systems(Update, net_send_pending_actions);

        app.update();

        let NetOutbound::Input(input) = receiver.try_recv().expect("action sent this update")
        else {
            panic!("expected v6 input half");
        };
        assert_eq!(input.seq, 1);
        assert_eq!(input.actions, ["drop_item"]);
        assert!(input.role_state.is_empty());
        assert!(input.visual.is_none());
        assert!(app
            .world()
            .resource::<PendingNetActions>()
            .actions
            .is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_drop_weapon_action_sends_spare_and_keeps_active_slot() {
        for degrees in (0..360).step_by(15) {
            let yaw = (degrees as f32).to_radians();
            let (sender, receiver) = std::sync::mpsc::channel();
            let mut pending = PendingNetActions::default();
            pending.push("drop_weapon");
            let mut app = App::new();
            app.insert_resource(NetState {
                status: NetStatus::InRoom,
                address: "udp://127.0.0.1:5230".into(),
                round_active: true,
                wire_format: NetWireFormat::V4,
                ..NetState::default()
            })
            .insert_resource(NetTx {
                active: Some(sender),
                connecting: None,
            })
            .insert_resource(NetInputSequence::default())
            .insert_resource(pending)
            .add_systems(Update, net_send_pending_actions);
            let player = app
                .world_mut()
                .spawn((
                    Fighter {
                        character_id: 0,
                        player_slot: 0,
                        weapon: 6,
                        weapon_slots: [6, 5],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    },
                    Transform::default(),
                    MotionState {
                        movement_mode: meteor_core::MovementMode::Idle,
                        velocity: Vec3::ZERO,
                        facing: yaw,
                    },
                    LocalFighter,
                ))
                .id();

            app.update();

            let NetOutbound::StockWeaponDrop(command) =
                receiver.try_recv().expect("stock weapon drop sent")
            else {
                panic!("expected stock request-19 command");
            };
            assert!((command.forward.x - yaw.sin()).abs() < 1e-6);
            assert_eq!(command.forward.y, 0.0);
            assert!((command.forward.z - yaw.cos()).abs() < 1e-6);
            assert_eq!(command.item_id, 5);
            assert!(!command.copy_only);
            assert_eq!(command.remaining_item_id, Some(6));
            let fighter = app.world().get::<Fighter>(player).unwrap();
            assert_eq!(
                (fighter.weapon, fighter.weapon_slots, fighter.weapon_slot),
                (6, [6, 0], 0)
            );
            assert!(app
                .world()
                .get::<crate::components::fighter::WeaponVariantChanged>(player)
                .is_some());

            app.world_mut()
                .resource_mut::<PendingNetActions>()
                .push("drop_item");
            app.update();
            let NetOutbound::StockWeaponDrop(copy) =
                receiver.try_recv().expect("stock drop-item copy sent")
            else {
                panic!("expected stock H copy command");
            };
            assert_eq!(copy.item_id, 6);
            assert_eq!(copy.forward, command.forward);
            assert!(copy.copy_only);
            assert_eq!(copy.remaining_item_id, None);
            let fighter = app.world().get::<Fighter>(player).unwrap();
            assert_eq!((fighter.weapon, fighter.weapon_slots), (6, [6, 0]));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_committed_slot_change_emits_zero_length_request19_command() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            address: "udp://127.0.0.1:5230".into(),
            round_active: true,
            ..NetState::default()
        })
        .insert_resource(NetTx {
            active: Some(sender),
            connecting: None,
        })
        .add_systems(Update, net_send_stock_weapon_switch);
        let player = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                LocalFighter,
            ))
            .id();
        app.update();
        assert!(
            receiver.try_recv().is_err(),
            "initial slot only seeds the edge"
        );

        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(player).unwrap();
            fighter.weapon_slot = 1;
            fighter.weapon = 5;
        }
        app.update();
        assert!(matches!(
            receiver.try_recv(),
            Ok(NetOutbound::StockWeaponSwitch(5))
        ));
    }

    #[test]
    fn friendly_fire_rule_is_scoped_to_an_active_network_room() {
        let mut net = NetState {
            friendly_fire: true,
            ..NetState::default()
        };
        assert!(!net.friendly_fire_enabled());
        net.status = NetStatus::InRoom;
        assert!(net.friendly_fire_enabled());
        net.status = NetStatus::Disconnected;
        assert!(!net.friendly_fire_enabled());
    }

    #[test]
    fn web_late_join_prepares_models_in_room_and_keeps_scene_bootstrap() {
        let member = RoomMemberWire {
            connected: true,
            player_id: "p2".into(),
            name: "joiner".into(),
            character_id: 3,
            weapon: 7,
            team: Some(2),
            ready: false,
        };
        let mut net = NetState {
            status: NetStatus::InRoom,
            transport_connected: true,
            room_id: "room1".into(),
            player_id: "p2".into(),
            mode: "deathmatch".into(),
            members: vec![member.clone()],
            match_epoch: Some(2),
            round_epoch: Some(3),
            battle_pending: true,
            lobby: meteor_network::protocol::RoomLobbyWire {
                state: Some("running".into()),
                ..Default::default()
            },
            pending_scene_ops: vec![SceneOp {
                kind: "set".into(),
                reference: serde_json::json!("D_test"),
                field: "active".into(),
                args: vec![serde_json::json!(0)],
            }],
            ..Default::default()
        };
        assert!(net.can_select_room_team());
        assert!(!net.can_edit_room());
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut next = NextState::default();
        let mut tx = NetTx::default();
        let mut sequence = NetInputSequence(0);
        let mut pending = PendingNetActions::default();
        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::MatchAssetPrepare {
                room_id: "room1".into(),
                match_epoch: 2,
                round_epoch: 3,
                attempt_id: "late-2".into(),
                purpose: "late-join".into(),
                joiner_id: Some("p2".into()),
                players: vec![member],
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::NetRoom,
        );
        assert!(net.asset_prepare.is_some());
        assert!(!net.round_active);
        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::MatchStart {
                room_id: "room1".into(),
                match_epoch: Some(2),
                round_epoch: Some(3),
                mode: "deathmatch".into(),
                stage: "sn01".into(),
                players: vec![],
                slots: vec![],
                resumed: false,
                late_joined: true,
                authority: "server".into(),
                host_id: String::new(),
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::NetRoom,
        );
        assert!(net.round_active);
        assert!(!net.battle_pending);
        assert!(!net.can_select_room_team());
        assert_eq!(net.pending_scene_ops.len(), 1);
        assert_eq!(request.character_id, 3);
        assert!(matches!(next, NextState::Pending(AppState::Loading)));
    }

    #[test]
    fn web_match_start_preserves_room_state_roster_and_loadout() {
        let member = RoomMemberWire {
            connected: true,
            player_id: "web-local".into(),
            name: "Web Player".into(),
            character_id: 3,
            weapon: 10,
            team: Some(2),
            ready: true,
        };
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: member.player_id.clone(),
            members: vec![member.clone()],
            ..NetState::default()
        };
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut next = NextState::<AppState>::default();
        let mut tx = NetTx::default();
        let mut sequence = NetInputSequence(17);
        let mut pending = PendingNetActions::default();

        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: Some(4),
                round_epoch: Some(1),
                mode: "deathmatch".into(),
                stage: "sn03".into(),
                players: Vec::new(),
                slots: Vec::new(),
                resumed: false,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::NetRoom,
        );

        assert_eq!(net.members, vec![member]);
        assert_eq!(request.player_name, "Web Player");
        assert_eq!(request.character_id, 3);
        assert_eq!(request.weapon, 10);
        assert_eq!(request.stage_id, "sn03");
        assert_eq!(net.match_epoch, Some(4));
        assert_eq!(net.round_epoch, Some(1));
    }

    #[test]
    fn weapon_drop_rest_updates_live_actor_only_for_current_epoch() {
        let mut net = NetState {
            match_epoch: Some(7),
            round_epoch: Some(3),
            dynamic_weapon_drops: std::collections::HashMap::from([(
                "D_wpNet7".into(),
                AuthorityWeaponDrop {
                    reference: "D_wpNet7".into(),
                    model: "w1_0".into(),
                    position: [0.0, 1.0, 0.0],
                    velocity: Some([0.0, 0.0, 20.0]),
                    settled: false,
                    pickup_ready: false,
                },
            )]),
            ..NetState::default()
        };
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut next = NextState::<AppState>::default();
        let mut tx = NetTx::default();
        let mut sequence = NetInputSequence::default();
        let mut pending = PendingNetActions::default();

        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::WeaponDropRest {
                match_epoch: Some(6),
                round_epoch: Some(3),
                reference: "D_wpNet7".into(),
                position: [1.0, 2.0, 3.0],
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::Playing,
        );
        assert_eq!(
            net.dynamic_weapon_drops["D_wpNet7"].position,
            [0.0, 1.0, 0.0]
        );

        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::WeaponDropRest {
                match_epoch: Some(7),
                round_epoch: Some(3),
                reference: "D_wpNet7".into(),
                position: [1.0, 2.0, 3.0],
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::Playing,
        );
        assert_eq!(
            net.dynamic_weapon_drops["D_wpNet7"].position,
            [1.0, 2.0, 3.0]
        );
        assert_eq!(net.dynamic_weapon_drops["D_wpNet7"].velocity, None);
        assert!(
            net.dynamic_weapon_drops["D_wpNet7"].pickup_ready,
            "reliable wall contact admits pickup"
        );
        assert!(matches!(
            net.pending_weapon_drops.as_slice(),
            [AuthorityWeaponDrop { reference, position, velocity: None, .. }]
                if reference == "D_wpNet7" && *position == [1.0, 2.0, 3.0]
        ));
    }

    #[test]
    fn remote_yaw_uses_shortest_wrap_arc() {
        let current = std::f32::consts::PI - 0.1;
        let target = -std::f32::consts::PI + 0.1;
        let next = lerp_remote_yaw(current, target, 0.5);
        assert!((next - std::f32::consts::PI).abs() < 0.001);
    }

    #[test]
    fn production_remote_smoothing_keeps_yaw_in_motion_state_only() {
        let mut app = App::new();
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(1.0 / 24.0));
        app.insert_resource(time)
            .add_systems(Update, net_smooth_remote_fighters);
        let entity = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "peer".into(),
                },
                Transform::from_xyz(0.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(1.7)),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                RemoteTransformTarget {
                    translation: Vec3::new(2.0, 0.0, 0.0),
                    yaw: 1.0,
                },
                RemoteContactSegment::default(),
            ))
            .id();

        app.update();

        let transform = app.world().get::<Transform>(entity).unwrap();
        let motion = app.world().get::<MotionState>(entity).unwrap();
        let segment = app.world().get::<RemoteContactSegment>(entity).unwrap();
        assert!((transform.translation.x - 1.0).abs() < 1e-6);
        assert_eq!(transform.rotation, Quat::IDENTITY);
        assert!((motion.facing - 0.5).abs() < 1e-6);
        assert!(segment.valid);
        assert_eq!(segment.from, Vec3::ZERO);
        assert_eq!(segment.to, Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn role_correction_never_pushes_but_velocity_extrapolation_does() {
        let mut app = App::new();
        let dt = 1.0 / 30.0;
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(dt));
        app.insert_resource(time)
            .add_systems(Update, net_smooth_remote_fighters);
        let correction_only = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "correction".into(),
                },
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                RemoteTransformTarget {
                    translation: Vec3::new(2.0, 0.0, 0.0),
                    yaw: 0.0,
                },
                RemoteContactSegment {
                    role_driven: true,
                    ..RemoteContactSegment::default()
                },
            ))
            .id();
        let physical = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "physical".into(),
                },
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Run,
                    velocity: Vec3::new(3.0, 0.0, 0.0),
                    facing: 0.0,
                },
                RemoteTransformTarget {
                    translation: Vec3::new(2.0, 0.0, 0.0),
                    yaw: 0.0,
                },
                RemoteContactSegment {
                    role_driven: true,
                    ..RemoteContactSegment::default()
                },
            ))
            .id();

        app.update();

        let correction_segment = app
            .world()
            .get::<RemoteContactSegment>(correction_only)
            .unwrap();
        assert!(!correction_segment.valid);
        assert!(
            app.world()
                .get::<Transform>(correction_only)
                .unwrap()
                .translation
                .x
                > 0.0,
            "位置纠偏仍应更新显示，只是不能产生人物推力"
        );

        let physical_segment = app.world().get::<RemoteContactSegment>(physical).unwrap();
        assert!(physical_segment.valid);
        assert!((physical_segment.to.x - physical_segment.from.x - 3.0 * dt).abs() < 1e-5);
        assert_eq!(physical_segment.state_age, dt);
    }

    #[test]
    fn queued_snapshot_burst_spawns_one_remote_per_player_with_latest_state() {
        let mut app = App::new();
        app.insert_resource(NetState::default())
            .add_systems(Update, net_apply_snapshot);

        for tick in 1..=20 {
            for id in ["peer-a", "peer-b"] {
                let mut snap = fighter_snapshot(2);
                snap.x = tick as f32;
                snap.hp = 100.0 - tick as f32;
                snap.kills = tick;
                app.world_mut()
                    .resource_mut::<NetState>()
                    .pending_snapshots
                    .push((id.into(), snap, Some(tick)));
            }
        }
        app.update();
        let mut query = app.world_mut().query::<(
            Entity,
            &RemoteFighter,
            &Transform,
            &Health,
            &RemoteAuthorityGeneration,
        )>();
        assert_eq!(
            query.iter(app.world()).count(),
            2,
            "one entity per player, regardless of backlog"
        );
        let initial: Vec<_> = query
            .iter(app.world())
            .map(|(entity, remote, transform, health, generation)| {
                assert_eq!(transform.translation.x, 20.0);
                assert_eq!(health.current, 80.0);
                assert_eq!(generation.role_state_tick, Some(20));
                assert_eq!(
                    app.world().resource::<NetState>().remote_kills[&remote.player_id],
                    20
                );
                (remote.player_id.clone(), entity)
            })
            .collect();
        assert_eq!(
            initial.len(),
            2,
            "one entity per player, regardless of backlog"
        );
        assert_ne!(initial[0].0, initial[1].0);

        for tick in 21..=24 {
            for id in ["peer-a", "peer-b"] {
                let mut snap = fighter_snapshot(2);
                snap.x = tick as f32;
                snap.hp = 100.0 - tick as f32;
                app.world_mut()
                    .resource_mut::<NetState>()
                    .pending_snapshots
                    .push((id.into(), snap, Some(tick)));
            }
        }
        app.update();
        assert_eq!(query.iter(app.world()).count(), 2);
        for (id, entity) in initial {
            let (_, remote, _, health, generation) = query.get(app.world(), entity).unwrap();
            assert_eq!(remote.player_id, id);
            assert_eq!(health.current, 76.0);
            assert_eq!(generation.role_state_tick, Some(24));
            assert_eq!(
                app.world()
                    .get::<RemoteTransformTarget>(entity)
                    .unwrap()
                    .translation
                    .x,
                24.0
            );
        }
    }

    #[test]
    fn production_snapshot_spawn_has_single_yaw_owner_and_visual_pin() {
        let yaw = 0.75;
        let mut net = NetState::default();
        net.pending_snapshots.push((
            "peer".into(),
            FighterSnapshot {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                yaw,
                pose_id: 200,
                pose_frame: 12,
                pose_start: 0,
                pose_end: 30,
                pose_loop_start: 0,
                pose_loop_end: 30,
                pose_loops: false,
                hp: 900.0,
                max_hp: 1000.0,
                weapon: 6,
                movement_mode: 2,
                kills: 0,
            },
            None,
        ));
        let mut app = App::new();
        app.insert_resource(net)
            .add_systems(Update, net_apply_snapshot);
        app.update();

        let mut query = app.world_mut().query_filtered::<(
            Entity,
            &Transform,
            &MotionState,
            &crate::components::fighter::VisualRootPin,
        ), With<RemoteFighter>>();
        let (_, transform, motion, _) = query.single(app.world()).expect("single local fighter");
        assert_eq!(transform.rotation, Quat::IDENTITY);
        assert!((motion.facing - yaw).abs() < 1e-6);
    }

    #[test]
    fn production_snapshot_projects_remote_airborne_state_for_character_contacts() {
        let mut net = NetState::default();
        net.pending_snapshots
            .push(("peer".into(), fighter_snapshot(4), None));
        let mut app = App::new();
        app.insert_resource(net)
            .add_systems(Update, net_apply_snapshot);
        app.update();

        let mut query = app.world_mut().query_filtered::<(
            &MotionState,
            &crate::plugins::movement_plugin::FighterAir,
        ), With<RemoteFighter>>();
        let (motion, air) = query.single(app.world()).unwrap();
        assert_eq!(motion.movement_mode, meteor_core::MovementMode::Airborne);
        assert!(air.air.airborne);

        app.world_mut()
            .resource_mut::<NetState>()
            .pending_snapshots
            .push(("peer".into(), fighter_snapshot(2), None));
        app.update();
        let (motion, air) = query.single(app.world()).unwrap();
        assert_eq!(motion.movement_mode, meteor_core::MovementMode::Run);
        assert!(!air.air.airborne);
    }

    #[test]
    fn lobby_join_uses_explicit_room_or_create_action() {
        let request = LaunchRequest::default();
        let rooms = vec![
            RoomSummary {
                room_id: "running".into(),
                name: "running".into(),
                mode: Some("leader".into()),
                stage_id: Some("sn01".into()),
                configured: true,
                state: "running".into(),
                has_password: false,
                player_count: 1,
                max_players: 16,
                team_counts: None,
                owner_name: None,
            },
            RoomSummary {
                room_id: "full".into(),
                name: "full".into(),
                mode: Some("leader".into()),
                stage_id: Some("sn01".into()),
                configured: true,
                state: "lobby".into(),
                has_password: false,
                player_count: 16,
                max_players: 16,
                team_counts: None,
                owner_name: None,
            },
            RoomSummary {
                room_id: "open".into(),
                name: "open".into(),
                mode: Some("deathmatch".into()),
                stage_id: Some("sn02".into()),
                configured: true,
                state: "lobby".into(),
                has_password: false,
                player_count: 2,
                max_players: 16,
                team_counts: None,
                owner_name: None,
            },
        ];
        assert!(matches!(
            web_room_join(Some(&rooms[2]), &request),
            Message::RoomJoin { room_id, mode, stage, .. }
                if room_id == "open"
                    && mode.as_deref() == Some("deathmatch")
                    && stage.as_deref() == Some("sn02")
        ));
        assert!(matches!(
            web_room_join(None, &request),
            Message::RoomCreate { mode, stage, .. }
                if mode == "deathmatch" && stage == request.stage_id
        ));
    }

    #[test]
    fn native_text_profile_reaches_create_and_join_requests() {
        use super::*;
        let request = LaunchRequest {
            player_name: "孟星魂🦋".into(),
            room_name: "好友切磋".into(),
            ..default()
        };
        assert!(
            matches!(web_room_join(None, &request), Message::RoomCreate { name, room_name, .. }
            if name == "孟星魂🦋" && room_name == "好友切磋")
        );
        let room = RoomSummary {
            room_id: "room".into(),
            name: "房间".into(),
            mode: None,
            stage_id: None,
            configured: true,
            state: "lobby".into(),
            has_password: false,
            player_count: 1,
            max_players: 16,
            team_counts: None,
            owner_name: None,
        };
        assert!(
            matches!(web_room_join(Some(&room), &request), Message::RoomJoin { name, .. } if name == "孟星魂🦋")
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn public_battle_toggle_excludes_udp_host_and_preserves_private_creation() {
        use super::*;
        let mut app = App::new();
        app.init_resource::<LaunchRequest>()
            .init_resource::<NetState>()
            .init_resource::<StockLanDiscoveryState>()
            .init_resource::<super::PendingStockConnection>()
            .init_resource::<super::StockConnectionDialog>()
            .add_systems(
                Update,
                (
                    super::menu_public_battle_button,
                    super::menu_stock_connection_buttons,
                )
                    .chain(),
            );
        let toggle = app
            .world_mut()
            .spawn((
                super::MenuPublicBattleButton,
                Interaction::Pressed,
                UiSelectionState::new(false),
            ))
            .id();
        let host = app
            .world_mut()
            .spawn((MenuStockHostButton, Interaction::Pressed))
            .id();
        app.update();
        assert!(app.world().resource::<LaunchRequest>().public_battle);
        assert!(app
            .world()
            .resource::<super::PendingStockConnection>()
            .0
            .is_none());
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap() = Interaction::None;
        *app.world_mut().get_mut::<Interaction>(host).unwrap() = Interaction::None;
        app.update();
        *app.world_mut().get_mut::<Interaction>(toggle).unwrap() = Interaction::Pressed;
        *app.world_mut().get_mut::<Interaction>(host).unwrap() = Interaction::Pressed;
        app.update();
        let pending = app.world().resource::<super::PendingStockConnection>();
        assert_eq!(pending.0.as_ref().unwrap().0, "udp-host://0.0.0.0:5230");
    }

    #[test]
    fn public_creation_transmits_selected_rules_and_camp_to_rust_authority() {
        use super::*;
        let mut request = LaunchRequest {
            public_battle: true,
            room_name: "公网切磋".into(),
            stock_rule: 4,
            stock_camp: 2,
            stock_game_num: 5,
            stock_game_time_minutes: 10,
            stock_life_adjust_percent: 150,
            stock_hide_item: false,
            stock_health_item: false,
            stock_update_item: false,
            stock_special_item: false,
            ..Default::default()
        };
        let mut server = meteor_network::server::RoomServer::new();
        server.connect(1);
        let hello = Message::from_json(r#"{"type":"hello","name":"tester","weapon":6}"#).unwrap();
        let welcome = server.handle(1, hello).to_sender.remove(0);
        let mut net = NetState {
            pending_web_join: Some(web_room_join(None, &request)),
            ..NetState::default()
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut tx = NetTx {
            active: None,
            connecting: Some(sender),
        };
        handle_net_message(
            &mut net,
            &mut request,
            &mut MatchEnd::default(),
            &mut NextState::default(),
            welcome,
            &mut tx,
            &mut NetInputSequence::default(),
            &mut PendingNetActions::default(),
            AppState::Menu,
        );
        let mut states = Vec::new();
        for _ in 0..2 {
            let NetOutbound::Control(message) = receiver.try_recv().unwrap() else {
                panic!()
            };
            let message = Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap();
            let dispatch = server.handle(1, message);
            states.extend(dispatch.to_sender);
        }
        assert!(states.iter().any(|message| matches!(message,
            Message::RoomState { mode, rules, members, .. }
                if mode == "assassin" && rules.game_num == Some(5)
                && rules.round_time == Some(10) && rules.life_adjust == Some(150)
                && rules.hide_item == Some(false) && rules.health_item == Some(false)
                && rules.update_item == Some(false) && rules.special_item == Some(false)
                && rules.transport.as_deref() == Some("server")
                && members[0].team == Some(2)
        )));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn shared_create_form_applies_map_mode_camp_and_rule_selections_before_request() {
        use super::*;
        let mut app = App::new();
        app.insert_resource(LaunchRequest {
            public_battle: true,
            ..Default::default()
        })
        .add_systems(
            Update,
            (
                menu_stock_rule_buttons,
                menu_stock_select_options,
                menu_stock_camp_buttons,
                menu_stock_item_rule_buttons,
                menu_stock_stage_buttons,
            )
                .chain(),
        );
        app.world_mut().spawn((
            Interaction::Pressed,
            MenuStockRuleButton(3),
            UiSelectionState::new(false),
        ));
        app.world_mut().spawn((
            Interaction::Pressed,
            MenuStockCampButton(2),
            UiSelectionState::new(false),
            Node::default(),
        ));
        app.world_mut().spawn((
            Interaction::Pressed,
            MenuStockStageButton {
                id: "sn12".into(),
                name: "峰火雷".into(),
            },
            UiSelectionState::new(false),
            Node::default(),
        ));
        for (field, value) in [
            (StockConfigField::GameNum, 10),
            (StockConfigField::GameTime, 15),
            (StockConfigField::LifeAdjust, 300),
        ] {
            app.world_mut().spawn((
                Interaction::Pressed,
                MenuStockSelectOption { field, value },
                UiSelectionState::new(false),
            ));
        }
        app.world_mut().spawn((
            Interaction::Pressed,
            MenuStockItemRuleButton(StockItemRule::Health),
            UiSelectionState::new(true),
        ));
        app.update();
        let request = app.world().resource::<LaunchRequest>();
        assert_eq!(request.stock_camp, 2);
        let message = web_room_join(None, request);
        let Message::RoomCreate {
            mode, stage, rules, ..
        } = Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap()
        else {
            panic!()
        };
        assert_eq!((mode.as_str(), stage.as_str()), ("siege", "sn12"));
        assert_eq!(
            (rules.game_num, rules.round_time, rules.life_adjust),
            (Some(10), Some(15), Some(300))
        );
        assert_eq!(rules.health_item, Some(false));
        assert_eq!(rules.special_item, Some(true));
    }

    #[test]
    fn native_text_chat_commits_unicode_once_and_ignores_profile_edits() {
        use super::super::native_text_input::{TextField, TextSubmitted};
        use super::*;
        let mut app = App::new();
        app.init_resource::<NetState>()
            .init_resource::<NetChatInput>()
            .add_message::<TextSubmitted>()
            .add_systems(Update, net_chat_input);
        app.world_mut().write_message(TextSubmitted {
            field: TextField::PlayerName,
            value: "名字".into(),
        });
        app.world_mut().write_message(TextSubmitted {
            field: TextField::Chat,
            value: "你好，江湖！🦋".into(),
        });
        app.update();
        app.update();
        let net = app.world().resource::<NetState>();
        assert_eq!(net.chat_messages.len(), 1);
        assert_eq!(net.chat_messages[0].1, "你好，江湖！🦋");
    }

    #[test]
    fn net_room_members_show_team_assignment() {
        assert!(!NetState::default().round_active);
        let net = NetState {
            members: vec![
                RoomMemberWire {
                    connected: true,
                    player_id: "p1".into(),
                    name: "Meteor".into(),
                    character_id: 1,
                    weapon: 0,
                    team: Some(1),
                    ready: true,
                },
                RoomMemberWire {
                    connected: true,
                    player_id: "p2".into(),
                    name: "Butterfly".into(),
                    character_id: 2,
                    weapon: 1,
                    team: Some(2),
                    ready: false,
                },
            ],
            ..Default::default()
        };

        let text = net_room_members_text(&net);
        assert!(text.contains("流星队"));
        assert!(text.contains("蝴蝶队"));
        assert!(!net.is_scene_authority(), "empty player id is not the host");
        let mut host = net;
        host.player_id = "p1".into();
        assert!(host.is_scene_authority());
        host.player_id = "p2".into();
        assert!(!host.is_scene_authority());
    }

    #[test]
    fn reconciliation_snapshots_preserve_authored_remote_idle_playback() {
        use meteor_network::wire_v4::{RoleSnapshot, RoleStateRecord, Snapshot, SnapshotPlayer};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p0").unwrap();
        let mut app = App::new();
        app.insert_resource(NetState {
            player_id: "local".into(),
            ..Default::default()
        })
        .insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
            [("p0".into(), character)].into(),
        ))
        .add_systems(Update, net_apply_snapshot);
        let mut role = role_state_from_runtime(RoleStateRuntime {
            position: RoleVec3::ZERO,
            forward: RoleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            velocity: RoleVec3::ZERO,
            pose_id: 0,
            grounded: true,
            guard_route: false,
            playback_arg: 1,
            pose_sequence: 1,
        });
        let full = |tick, role| Snapshot {
            tick,
            players: vec![SnapshotPlayer {
                id: "peer".into(),
                character: "p0".into(),
                weapon: "sword".into(),
                health: 750.0,
                max_health: Some(1000.0),
                alive: true,

                pose_id: 200,
                pose_cursor: 0.0,
                role_state: encode_event2_state(role).to_vec(),
                ..Default::default()
            }],
            ..Default::default()
        };
        handle_v4_snapshot(
            &mut app.world_mut().resource_mut::<NetState>(),
            full(1, role),
        );
        app.update();
        app.update();
        let entity = app
            .world_mut()
            .query_filtered::<Entity, With<RemoteFighter>>()
            .single(app.world())
            .unwrap();
        let baseline = {
            let mut pose = app.world_mut().get_mut::<PoseRuntime>(entity).unwrap();
            assert_eq!(pose.pose_id, 0);
            assert!(pose.end > pose.start + 5);
            pose.frame = pose.start as f32 + 5.0;
            (
                pose.start,
                pose.end,
                pose.loop_start,
                pose.loop_end,
                pose.frame,
                pose.pose_instance_id,
            )
        };
        for tick in 2..=6 {
            handle_v4_snapshot(
                &mut app.world_mut().resource_mut::<NetState>(),
                full(tick, role),
            );
            app.update();
            let pose = app.world().get::<PoseRuntime>(entity).unwrap();
            assert_eq!(pose.pose_id, 0);
            assert_eq!(
                (
                    pose.start,
                    pose.end,
                    pose.loop_start,
                    pose.loop_end,
                    pose.frame,
                    pose.pose_instance_id
                ),
                baseline
            );
            assert_eq!(app.world().get::<Health>(entity).unwrap().current, 750.0);
        }

        role.state_words = meteor_network::role_state::unpack_state_words(
            meteor_network::role_state::pack_state_word0(24, true, false, 1),
            2,
        );
        handle_v4_snapshot(
            &mut app.world_mut().resource_mut::<NetState>(),
            full(7, role),
        );
        app.update();
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 24);
        assert!(app.world().get::<PoseRuntime>(entity).unwrap().end > 1);

        role.position.x = 6.0;
        handle_v4_role_snapshot(
            &mut app.world_mut().resource_mut::<NetState>(),
            RoleSnapshot {
                tick: 10,
                roles: vec![RoleStateRecord {
                    id: "peer".into(),
                    role_state: encode_event2_state(role).to_vec(),
                }],
                ..Default::default()
            },
        );
        app.update();
        role.position.x = 0.0;
        handle_v4_snapshot(
            &mut app.world_mut().resource_mut::<NetState>(),
            full(8, role),
        );
        app.update();
        assert!(
            (app.world()
                .get::<RemoteTransformTarget>(entity)
                .unwrap()
                .translation
                .x
                - 6.0)
                .abs()
                < 0.04
        );
    }

    #[test]
    fn v4_snapshot_uses_original_event2_state_for_remote_interpolation() {
        let mut net = NetState {
            room_id: "room1".into(),
            player_id: "p1".into(),
            ..Default::default()
        };
        let role_state = encode_event2_state(role_state_from_runtime(RoleStateRuntime {
            position: RoleVec3 {
                x: 1.25,
                y: 2.0,
                z: -3.5,
            },
            forward: RoleVec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            velocity: RoleVec3::ZERO,
            pose_id: 200,
            grounded: false,
            guard_route: false,
            playback_arg: 0,
            pose_sequence: 0,
        }));
        let remote = SnapshotPlayer {
            id: "p2".into(),
            name: "remote".into(),
            character: "p1".into(),
            weapon: "blade".into(),
            weapon_model: None,
            weapon_slot: 0,
            team: Some(2),
            is_leader: true,
            connected: true,
            ready: true,
            health: 600.0,
            max_health: Some(1000.0),
            anger: 0.0,
            kills: 3,
            round_kills: 1,
            round_wins: 0,
            deaths: 2,
            guard: false,
            alive: true,
            pose_id: 200,
            pose_cursor: 6.5,
            lift: 0.0,
            role_state: role_state.to_vec(),
            position: None,
            yaw: None,
            weapon_slots: Vec::new(),
            item_effects: Vec::new(),
            worn_effects: vec![meteor_network::wire_v4::ItemEffect {
                model: "Item15".into(),
                remaining: 5.0,
            }],
        };
        let mut own = remote.clone();
        own.id = "p1".into();
        own.is_leader = false;
        own.health = 750.0;
        own.anger = 45.0;
        own.kills = 4;
        own.role_state.clear();
        handle_v4_snapshot(
            &mut net,
            Snapshot {
                room_id: "room1".into(),
                tick: 7,
                server_time: 1.0,
                players: vec![own, remote],
                npcs: Vec::new(),
                drops: Vec::new(),
                flag: None,
                r#match: None,
                match_epoch: None,
                round_epoch: None,
            },
        );
        let (_, snapshot, _) = net.pending_snapshots.pop().expect("remote snapshot");
        assert!((snapshot.x - 1.25).abs() < 0.02);
        assert!((snapshot.z + 3.5).abs() < 0.02);
        assert_eq!(snapshot.pose_id, 200);
        assert_eq!(snapshot.weapon, 8);
        assert_eq!(snapshot.kills, 3);
        assert_eq!(snapshot.movement_mode, 4);
        assert_eq!(net.remote_kills.get("p1"), Some(&4));
        assert_eq!(
            net.radar_star_players,
            std::collections::HashSet::from(["p2".to_string()])
        );
        assert!(matches!(
            net.pending_hits.as_slice(),
            [AuthorityHit {
                stock_guard_bucket: None,
                damage,
                health: Some(750.0),
                anger: Some(45.0),
                ..
            }] if *damage == 0.0
        ));
        assert_eq!(net.pending_item_states.len(), 2);
        assert!(net.pending_item_states.iter().any(|state| {
            state.player_id == "p1"
                && state.weapon == "blade"
                && (state.max_health - 1000.0).abs() < f32::EPSILON
        }));
        assert!(net.pending_item_states.iter().any(|state| {
            state.player_id == "p2"
                && state.weapon == "blade"
                && (state.max_health - 1000.0).abs() < f32::EPSILON
                && state.worn_effects.first().is_some_and(|effect| {
                    effect.model == "Item15" && (effect.remaining - 5.0).abs() < f32::EPSILON
                })
        }));
    }

    #[test]
    fn v4_snapshot_replaces_settled_dynamic_weapon_drops() {
        let mut net = NetState::default();
        handle_v4_snapshot(
            &mut net,
            Snapshot {
                room_id: "room1".into(),
                tick: 42,
                server_time: 1.0,
                players: Vec::new(),
                npcs: Vec::new(),
                drops: vec![SnapshotDrop {
                    r#ref: "D_wpNet7".into(),
                    model: "w1_0".into(),
                    position: Some(V4Vec3 {
                        x: 1.0,
                        y: 0.0,
                        z: 4.0406,
                    }),
                    settled: true,
                }],
                flag: None,
                r#match: None,
                match_epoch: None,
                round_epoch: None,
            },
        );
        let drop = net
            .dynamic_weapon_drops
            .get("D_wpNet7")
            .expect("snapshot drop is retained for render reconciliation");
        assert_eq!(drop.model, "w1_0");
        assert_eq!(drop.position, [1.0, 0.0, 4.0406]);
        assert_eq!(drop.velocity, None);

        handle_v4_snapshot(
            &mut net,
            Snapshot {
                room_id: "room1".into(),
                tick: 43,
                server_time: 1.1,
                players: Vec::new(),
                npcs: Vec::new(),
                drops: Vec::new(),
                flag: None,
                r#match: None,
                match_epoch: None,
                round_epoch: None,
            },
        );
        assert!(net.dynamic_weapon_drops.is_empty());
    }

    #[test]
    fn protocol_v6_role_snapshot_queues_only_current_round_remote_state() {
        let state = role_state_from_runtime(RoleStateRuntime {
            position: RoleVec3 {
                x: 2.0,
                y: 1.0,
                z: -3.0,
            },
            forward: RoleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            velocity: RoleVec3::ZERO,
            pose_id: 24,
            grounded: true,
            guard_route: false,
            playback_arg: 0,
            pose_sequence: 9,
        });
        let record = RoleStateRecord {
            id: "p2".into(),
            role_state: encode_event2_state(state).to_vec(),
        };
        let mut net = NetState {
            room_id: "room1".into(),
            player_id: "p1".into(),
            match_epoch: Some(7),
            round_epoch: Some(3),
            ..NetState::default()
        };

        handle_v4_role_snapshot(
            &mut net,
            RoleSnapshot {
                room_id: "room1".into(),
                tick: 10,
                server_time: 1.0,
                roles: vec![record.clone()],
                match_epoch: Some(6),
                round_epoch: Some(3),
            },
        );
        assert!(
            net.pending_role_states.is_empty(),
            "stale match is rejected"
        );

        handle_v4_role_snapshot(
            &mut net,
            RoleSnapshot {
                room_id: "room1".into(),
                tick: 10,
                server_time: 1.0,
                roles: vec![record.clone()],
                match_epoch: None,
                round_epoch: None,
            },
        );
        assert!(
            net.pending_role_states.is_empty(),
            "v6 frames without mandatory generations are rejected"
        );

        handle_v4_role_snapshot(
            &mut net,
            RoleSnapshot {
                room_id: "room1".into(),
                tick: 11,
                server_time: 1.1,
                roles: vec![record],
                match_epoch: Some(7),
                round_epoch: Some(3),
            },
        );
        assert_eq!(net.pending_role_states.len(), 1);
        assert_eq!(net.pending_role_states[0].0, "p2");
        assert_eq!(net.pending_role_states[0].1.state_words.pose_id, 24);
        handle_v4_role_snapshot(
            &mut net,
            RoleSnapshot {
                room_id: "room1".into(),
                tick: 11,
                server_time: 1.2,
                roles: Vec::new(),
                match_epoch: Some(7),
                round_epoch: Some(3),
            },
        );
        assert_eq!(
            net.pending_role_states.len(),
            1,
            "duplicate tick cannot erase a newer queued actor state"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_dodge_to_guard_sends_looping_stance_and_actual_guard_eligibility() {
        use crate::components::fighter::{FighterHitReaction, KillScore};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p4").unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<NetInputSequence>()
            .init_resource::<KillScore>()
            .insert_resource(NetState {
                status: NetStatus::InRoom,
                round_active: true,
                address: "udp://127.0.0.1:5230".into(),
                ..Default::default()
            })
            .insert_resource(NetTx {
                active: Some(sender),
                connecting: None,
            })
            .add_systems(Update, super::net_send_snapshot);
        let mut pose = PoseRuntime::default();
        character.apply_pose(&mut pose, 501, true);
        let entity = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 4,
                    player_slot: 0,
                    weapon: 9,
                    weapon_slots: [9, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                pose,
                Transform::default(),
                Health::new(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Dodge,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                SmState {
                    current: meteor_core::FighterState::Dodge,
                    ..Default::default()
                },
                FighterHitReaction::default(),
            ))
            .id();
        app.update();
        let NetOutbound::StockRoleState(dodge) = receiver.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(dodge.state_words.pose_id, 501);
        assert!(
            !dodge.state_words.guard_route,
            "dodge cannot acquire guard eligibility from held input"
        );
        assert_eq!(dodge.state_words.playback_arg, 0);
        for guard_pose in [480, 481] {
            character.apply_pose(
                &mut app.world_mut().get_mut::<PoseRuntime>(entity).unwrap(),
                guard_pose,
                true,
            );
            app.world_mut().get_mut::<SmState>(entity).unwrap().current =
                meteor_core::FighterState::Guard;
            app.world_mut()
                .get_mut::<MotionState>(entity)
                .unwrap()
                .movement_mode = meteor_core::MovementMode::Idle;
            app.update();
            let NetOutbound::StockRoleState(guard) = receiver.try_recv().unwrap() else {
                panic!()
            };
            let packet = meteor_network::role_state::encode_request1_state(guard);
            let decoded = meteor_network::role_state::decode_request1_state(&packet).unwrap();
            assert_eq!(decoded.state_words.pose_id, guard_pose);
            assert!(decoded.state_words.guard_route);
            assert_eq!(
                decoded.state_words.playback_arg, 1,
                "original must keep looping its guard stance"
            );
            assert!(decoded.state_words.pose_sequence > dodge.state_words.pose_sequence);
        }
        character.apply_pose(
            &mut app.world_mut().get_mut::<PoseRuntime>(entity).unwrap(),
            0,
            true,
        );
        app.world_mut().get_mut::<SmState>(entity).unwrap().current =
            meteor_core::FighterState::Name;
        app.update();
        let NetOutbound::StockRoleState(released) = receiver.try_recv().unwrap() else {
            panic!()
        };
        assert!(!released.state_words.guard_route);
    }

    #[test]
    fn soft_locomotion_switches_reach_remote_through_request1() {
        use meteor_network::role_state::{
            decide_pose_transition, decode_request1_state, encode_request1_state,
            role_state_from_runtime, RemotePoseRuntime,
        };
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p0").unwrap();
        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, 140, true);
        let mut remote = RemotePoseRuntime {
            pose_id: pose.pose_id,
            ground_sequence: pose.pose_instance_id as u16,
            applied_sequence: pose.pose_instance_id as u16,
        };
        let motion = MotionState {
            movement_mode: meteor_core::MovementMode::Run,
            velocity: Vec3::ZERO,
            facing: 0.0,
        };
        for target in [141, 142, 143, 140, 140] {
            assert!(meteor_physics::locomotion::can_keep_locomotion_phase(
                pose.pose_id,
                target
            ));
            let changed = pose.pose_id != target;
            let instance = pose.pose_instance_id;
            pose.frame = pose.start as f32 + 2.0;
            character.apply_pose(&mut pose, target, false);
            assert_eq!(pose.frame, pose.start as f32 + 2.0);
            assert_eq!(pose.pose_instance_id, instance + u32::from(changed));
            let state = role_state_from_runtime(local_request1_runtime(
                &pose,
                &Transform::default(),
                &motion,
                &FighterAir::default(),
                false,
            ));
            let decoded = decode_request1_state(&encode_request1_state(state)).unwrap();
            let transition = decide_pose_transition(remote, decoded);
            assert_eq!(transition.apply, changed, "direction pose {target}");
            if transition.apply {
                remote.pose_id = decoded.state_words.pose_id;
            }
            remote.ground_sequence = transition.ground_sequence;
            remote.applied_sequence = transition.applied_sequence;
            assert_eq!(remote.pose_id, target);
            assert!(
                !decide_pose_transition(remote, decoded).apply,
                "duplicates must not restart animation"
            );
        }
    }

    #[test]
    fn request1_uses_motion_facing_and_pose_instance_generation() {
        let mut pose = PoseRuntime::idle();
        pose.pose_id = 24;
        pose.pose_instance_id = 65_540;
        let transform =
            Transform::from_xyz(1.0, 2.0, 3.0).with_rotation(Quat::from_rotation_y(-1.0));
        let motion = MotionState {
            movement_mode: meteor_core::MovementMode::Run,
            velocity: Vec3::new(4.0, 0.5, -2.0),
            facing: std::f32::consts::FRAC_PI_2,
        };
        let mut air = FighterAir::default();
        air.air.airborne = true;

        let runtime = local_request1_runtime(&pose, &transform, &motion, &air, true);

        assert_eq!(
            runtime.position,
            RoleVec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0
            }
        );
        assert!((yaw_from_forward(runtime.forward) - motion.facing).abs() < 1e-5);
        assert_eq!(
            runtime.velocity,
            RoleVec3 {
                x: 4.0,
                y: 0.5,
                z: -2.0
            }
        );
        assert_eq!(runtime.pose_id, 24);
        assert_eq!(runtime.pose_sequence, 4);
        assert!(!runtime.grounded);
        assert!(runtime.guard_route);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_hit_reactions_preserve_effect_strategy_for_players_and_remotes() {
        for guarded in [false, true] {
            for strategy in 0..12 {
                let mut net = NetState {
                    player_id: "stock-1".into(),
                    ..NetState::default()
                };
                for target_actor_id in [1, 2] {
                    handle_stock_game_event(
                        &mut net,
                        &mut LaunchRequest::default(),
                        &mut MatchEnd::default(),
                        &mut NetTx::default(),
                        &mut NextState::<AppState>::default(),
                        AppState::Playing,
                        StockGameEvent::HitReaction {
                            guarded,
                            event: meteor_network::stock_protocol::StockHitReactionEvent {
                                pose_id: if guarded {
                                    (93 << 16) | (strategy % 4)
                                } else {
                                    93
                                },
                                move_hundred: 100,
                                value_hundred: 0,
                                context: strategy,
                                target_actor_id,
                            },
                        },
                    );
                }
                assert_eq!(net.pending_hits.len(), 1);
                assert_eq!(net.pending_remote_hits.len(), 1);
                for hit in [&net.pending_hits[0], &net.pending_remote_hits[0].1] {
                    assert_eq!(hit.effect_strategy, Some(strategy as u8));
                    assert_eq!(hit.guarded, guarded);
                    assert_eq!(hit.reaction.as_ref().unwrap().target_pose, Some(93));
                    use meteor_combat::hit_reaction::FacingBucket;
                    let expected = [
                        FacingBucket::Front,
                        FacingBucket::Right,
                        FacingBucket::Left,
                        FacingBucket::Back,
                    ][strategy as usize % 4];
                    assert_eq!(hit.stock_guard_bucket, guarded.then_some(expected));
                    assert!(hit.attacker_id.is_none());
                }
            }
        }
    }

    #[test]
    fn authority_hit_installs_the_authored_local_behit_controller() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p0",
        )
        .expect("p0 assets");
        let mut app = App::new();
        app.insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
            std::collections::HashMap::from([("p0".to_string(), character)]),
        ))
        .insert_resource(NetState {
            round_active: true,
            pending_hits: vec![AuthorityHit {
                stock_guard_bucket: None,
                damage: 100.0,
                health: Some(900.0),
                anger: Some(14.0),
                guarded: false,
                attacker_id: None,
                effect_strategy: None,
                motion_scale: Some(1.5),
                reaction: Some(CombatReactionWire {
                    target_move: 1.5,
                    target_value: 0.5,
                    target_pose: Some(93),
                    target_pose_front: Some(93),
                    target_pose_back: Some(94),
                    target_pose_left: Some(92),
                    target_pose_right: Some(91),
                    ..CombatReactionWire::default()
                }),
                tick: Some(12),
                impact: true,
                terminal: false,
            }],
            ..NetState::default()
        })
        .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
        .add_systems(Update, net_handle_hit);
        let entity = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::default(),
                Health::default(),
                Qi::default(),
                DeathRespawn::default(),
                (
                    PoseRuntime::idle(),
                    EntryBlend {
                        duration: 0.4,
                        ..EntryBlend::default()
                    },
                    CombatState {
                        state: meteor_core::FighterState::Attack1,
                    },
                    SmState {
                        current: meteor_core::FighterState::Attack1,
                        ..SmState::default()
                    },
                    MotionState {
                        movement_mode: meteor_core::MovementMode::Idle,
                        velocity: Vec3::new(6.0, 0.0, -2.0),
                        facing: 0.0,
                    },
                    FighterAir::default(),
                    FighterAttackMotion {
                        active: true,
                        ..FighterAttackMotion::default()
                    },
                ),
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<Health>(entity).unwrap().current, 900.0);
        assert_eq!(app.world().get::<Qi>(entity).unwrap().current, 14.0);
        assert_eq!(
            app.world().get::<SmState>(entity).unwrap().current,
            meteor_core::FighterState::BeHit
        );
        let pose = app.world().get::<PoseRuntime>(entity).unwrap();
        assert_eq!(pose.pose_id, 93);
        assert_eq!(pose.frame, pose.start as f32);
        assert_eq!(
            app.world().get::<EntryBlend>(entity).unwrap().duration,
            0.12
        );
        let reaction = app.world().get::<FighterHitReaction>(entity).unwrap();
        assert_eq!(reaction.current.as_ref().unwrap().pose_id, Some(93));
        assert_eq!(reaction.current.as_ref().unwrap().motion_scale, 1.5);
        assert_eq!((reaction.carry_x, reaction.carry_z), (6.0, -2.0));
        assert!(
            !app.world()
                .get::<FighterAttackMotion>(entity)
                .unwrap()
                .active
        );

        let attack =
            &app.world().resource::<super::CharacterAssetsSet>().0["p0"].poses[&417].attacks[0];
        let event = meteor_network::stock_protocol::StockHitReactionEvent {
            pose_id: u32::from(attack.target_pose.unwrap()),
            move_hundred: (attack.target_move * 100.0).round() as u32,
            value_hundred: (attack.target_value * 100.0).round() as u32,
            context: 0,
            target_actor_id: 2,
        };
        assert_eq!(event.pose_id, 100);
        assert_eq!(event.motion_scale(), 1.0);
        app.world_mut()
            .entity_mut(entity)
            .remove::<LocalFighter>()
            .insert(crate::plugins::combat_plugin::NpcFighter);
        app.world_mut()
            .get_mut::<MotionState>(entity)
            .unwrap()
            .velocity = Vec3::new(6.0, 0.0, -2.0);
        super::stock_npc_bridge::apply_event(
            &mut app.world_mut().resource_mut::<NetState>(),
            entity,
            meteor_network::stock_game::StockGameEvent::HitReaction {
                guarded: false,
                event,
            },
        );
        app.update();
        let reaction = app.world().get::<FighterHitReaction>(entity).unwrap();
        assert_eq!(reaction.current.as_ref().unwrap().pose_id, Some(100));
        assert_eq!(reaction.current.as_ref().unwrap().motion_scale, 1.0);
        assert_eq!(
            (reaction.carry_x, reaction.carry_z),
            (0.0, 0.0),
            "LAN NPCs must match offline NPCs: no local-player carry on fist 417"
        );
        app.world_mut().get_mut::<Fighter>(entity).unwrap().weapon = 5;
        for (selector, expected) in [(0, None), (1, Some(83)), (2, Some(84)), (3, Some(85))] {
            super::stock_npc_bridge::apply_event(
                &mut app.world_mut().resource_mut::<NetState>(),
                entity,
                StockGameEvent::HitReaction {
                    guarded: true,
                    event: meteor_network::stock_protocol::StockHitReactionEvent {
                        pose_id: (110 << 16) | selector,
                        move_hundred: 125,
                        value_hundred: 35,
                        context: 1,
                        target_actor_id: 2,
                    },
                },
            );
            app.update();
            let runtime = app.world().get::<FighterHitReaction>(entity).unwrap();
            let reaction = runtime.current.as_ref().unwrap();
            assert_eq!(reaction.selector, selector);
            let pose = reaction.pose_id.unwrap();
            if let Some(expected) = expected {
                assert_eq!(pose, expected);
            } else {
                assert!((56..=59).contains(&pose));
            }
            assert_eq!(
                app.world().get::<PoseRuntime>(entity).unwrap().pose_id,
                pose
            );
            assert_eq!(reaction.motion_scale, 1.25);
            assert!((reaction.authored_minimum - 0.35).abs() < 1e-6);
        }
    }

    #[test]
    fn legacy_authority_hit_keeps_motion_scale_when_reaction_record_is_absent() {
        let hit = AuthorityHit {
            stock_guard_bucket: None,
            damage: 10.0,
            health: None,
            anger: None,
            guarded: false,
            attacker_id: None,
            effect_strategy: None,
            motion_scale: Some(2.5),
            reaction: None,
            tick: None,
            impact: true,
            terminal: false,
        };

        let attack = authority_attack_block(&hit);

        assert_eq!(attack.target_move, 2.5);
        assert_eq!(attack.defense_move, 2.5);
        assert_eq!(attack.target_value, 0.0);
        assert_eq!(attack.defense_value, 0.0);
        assert_eq!(attack.target_pose, None);
    }

    #[test]
    fn authority_hit_installs_one_authored_remote_reaction_window() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let character = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root),
            "p0",
        )
        .expect("p0 assets");
        let mut app = App::new();
        app.insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
            std::collections::HashMap::from([("p0".to_string(), character)]),
        ))
        .insert_resource(NetState {
            player_id: "local".into(),
            round_active: true,
            pending_remote_hits: vec![(
                "remote".into(),
                AuthorityHit {
                    stock_guard_bucket: None,
                    damage: 100.0,
                    health: Some(900.0),
                    anger: None,
                    guarded: false,
                    attacker_id: Some("local".into()),
                    effect_strategy: None,
                    motion_scale: Some(1.5),
                    reaction: Some(CombatReactionWire {
                        target_move: 1.5,
                        target_value: 0.5,
                        target_pose: Some(93),
                        ..CombatReactionWire::default()
                    }),
                    tick: Some(12),
                    impact: true,
                    terminal: false,
                },
            )],
            ..NetState::default()
        })
        .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
        .add_systems(Update, (net_apply_snapshot, net_handle_remote_hits).chain());
        app.world_mut().spawn((
            LocalFighter,
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Transform::default(),
        ));
        let entity = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "remote".into(),
                },
                Fighter {
                    character_id: 0,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_xyz(0.0, 0.0, 1.0),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Health::default(),
                PoseRuntime::idle(),
                EntryBlend {
                    duration: 0.4,
                    ..EntryBlend::default()
                },
                CombatState::default(),
                DeathRespawn::default(),
                FighterAir::default(),
                RemoteHitMotionWindow::default(),
                RemoteAuthorityGeneration {
                    role_state_tick: Some(12),
                    last_hit_tick: None,
                },
                RemoteTransformTarget {
                    translation: Vec3::new(0.0, 0.0, 1.0),
                    yaw: 0.0,
                },
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<Health>(entity).unwrap().current, 900.0);
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 93);
        assert_eq!(
            app.world().get::<EntryBlend>(entity).unwrap().duration,
            0.12
        );
        let reaction = app.world().get::<FighterHitReaction>(entity).unwrap();
        assert_eq!(reaction.current.as_ref().unwrap().pose_id, Some(93));
        assert_eq!(reaction.current.as_ref().unwrap().motion_scale, 1.5);
        assert_eq!(
            app.world()
                .get::<RemoteHitMotionWindow>(entity)
                .unwrap()
                .remaining,
            super::REMOTE_HIT_ROOT_SECONDS
        );
        assert_eq!(
            app.world()
                .get_resource::<NetState>()
                .unwrap()
                .remote_pose_sequences
                .get("remote"),
            Some(&(1, 1))
        );

        let mut same_tick_pre_hit = fighter_snapshot(0);
        same_tick_pre_hit.x = 99.0;
        same_tick_pre_hit.hp = 1000.0;
        same_tick_pre_hit.pose_id = 0;
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_snapshots
            .push(("remote".into(), same_tick_pre_hit, Some(12)));
        app.update();

        assert_eq!(app.world().get::<Health>(entity).unwrap().current, 900.0);
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 93);
        assert_eq!(
            app.world()
                .get::<RemoteTransformTarget>(entity)
                .unwrap()
                .translation,
            Vec3::new(0.0, 0.0, 1.0),
            "a same-tick pre-hit snapshot must not pull the reaction back"
        );

        let continuing_role = role_state_from_runtime(RoleStateRuntime {
            position: RoleVec3 {
                x: 0.1,
                y: 0.2,
                z: 1.0,
            },
            velocity: RoleVec3 {
                x: 0.0,
                y: 2.0,
                z: 0.0,
            },
            pose_id: 93,
            pose_sequence: 1,
            forward: RoleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            grounded: false,
            guard_route: false,
            playback_arg: 0,
        });
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_role_states
            .push(("remote".into(), continuing_role, 13));
        app.update();
        assert_eq!(
            app.world()
                .get::<RemoteHitMotionWindow>(entity)
                .unwrap()
                .remaining,
            super::REMOTE_HIT_ROOT_SECONDS,
            "new role state must preserve the active root window"
        );
        assert!(app
            .world()
            .get::<FighterHitReaction>(entity)
            .unwrap()
            .current
            .is_some());
        assert_eq!(
            app.world()
                .get::<RemoteTransformTarget>(entity)
                .unwrap()
                .translation,
            Vec3::new(0.1, 0.2, 1.0)
        );

        let mut newer_state = fighter_snapshot(0);
        newer_state.x = 77.0;
        newer_state.hp = 850.0;
        newer_state.pose_id = 0;
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_snapshots
            .push(("remote".into(), newer_state, Some(13)));
        app.update();

        assert_eq!(app.world().get::<Health>(entity).unwrap().current, 850.0);
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 0);
        assert_eq!(
            app.world()
                .get::<RemoteTransformTarget>(entity)
                .unwrap()
                .translation
                .x,
            77.0
        );
        assert!(app
            .world()
            .get::<FighterHitReaction>(entity)
            .unwrap()
            .current
            .is_none());
        assert_eq!(
            app.world()
                .get::<RemoteHitMotionWindow>(entity)
                .unwrap()
                .remaining,
            0.0
        );

        let hit = AuthorityHit {
            stock_guard_bucket: None,
            damage: 100.0,
            health: Some(750.0),
            anger: None,
            guarded: false,
            attacker_id: Some("local".into()),
            effect_strategy: None,
            motion_scale: Some(1.0),
            reaction: Some(CombatReactionWire {
                target_move: 1.0,
                target_value: 0.5,
                target_pose: Some(114),
                ..Default::default()
            }),
            tick: Some(20),
            impact: true,
            terminal: false,
        };
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_remote_hits
            .push(("remote".into(), hit.clone()));
        app.update();
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 114);

        let mut state = continuing_role;
        state.state_words = meteor_network::role_state::unpack_state_words(
            meteor_network::role_state::pack_state_word0(114, true, false, 0),
            500,
        );
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_role_states
            .push(("remote".into(), state, 21));
        app.update();
        let instance = app
            .world()
            .get::<PoseRuntime>(entity)
            .unwrap()
            .pose_instance_id;

        app.world_mut().resource_scope(|world, assets: bevy::prelude::Mut<crate::plugins::character_plugin::CharacterAssetsSet>| {
            let character = &assets.0["p0"];
            let link = character.poses[&114].link.unwrap();
            assert_eq!(link, 112);
            let mut pose = world.get_mut::<PoseRuntime>(entity).unwrap();
            character.apply_pose(&mut pose, link, true);
            pose.pose_instance_id = instance;
        });
        for tick in 22..=26 {
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_role_states
                .push(("remote".into(), state, tick));
            app.update();
            let pose = app.world().get::<PoseRuntime>(entity).unwrap();
            assert_eq!(
                pose.pose_id, 112,
                "the same network reaction must not undo its natural Link"
            );
            assert_eq!(pose.pose_instance_id, instance);
        }

        app.world_mut()
            .resource_mut::<NetState>()
            .pending_remote_hits
            .push((
                "remote".into(),
                AuthorityHit {
                    stock_guard_bucket: None,
                    tick: Some(27),
                    health: Some(650.0),
                    ..hit
                },
            ));
        app.update();
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 114);
        assert_ne!(
            app.world()
                .get::<PoseRuntime>(entity)
                .unwrap()
                .pose_instance_id,
            instance
        );
    }

    #[test]
    fn newer_remote_role_state_prevents_a_late_hit_pose_rewind() {
        let mut app = App::new();
        app.insert_resource(NetState {
            player_id: "local".into(),
            round_active: true,
            pending_remote_hits: vec![(
                "remote".into(),
                AuthorityHit {
                    stock_guard_bucket: None,
                    damage: 100.0,
                    health: Some(900.0),
                    anger: None,
                    guarded: false,
                    attacker_id: None,
                    effect_strategy: None,
                    motion_scale: Some(1.5),
                    reaction: Some(CombatReactionWire {
                        target_pose: Some(93),
                        ..CombatReactionWire::default()
                    }),
                    tick: Some(12),
                    impact: true,
                    terminal: false,
                },
            )],
            ..NetState::default()
        })
        .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
        .add_systems(Update, net_handle_remote_hits);
        let entity = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "remote".into(),
                },
                Fighter {
                    character_id: 0,
                    player_slot: 1,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Health::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                CombatState::default(),
                DeathRespawn::default(),
                FighterAir::default(),
                RemoteHitMotionWindow::default(),
                RemoteAuthorityGeneration {
                    role_state_tick: Some(13),
                    last_hit_tick: None,
                },
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<Health>(entity).unwrap().current, 1000.0);
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 0);
        assert!(app
            .world()
            .get::<FighterHitReaction>(entity)
            .unwrap()
            .current
            .is_none());
        assert!(!app
            .world()
            .get_resource::<NetState>()
            .unwrap()
            .remote_pose_sequences
            .contains_key("remote"));
    }

    #[test]
    fn authority_respawn_clears_previous_life_motion_owners_and_lock() {
        let mut app = App::new();
        app.insert_resource(NetState {
            player_id: "local".into(),
            wire_format: NetWireFormat::V4,
            pending_respawns: vec!["local".into()],
            ..NetState::default()
        })
        .init_resource::<LockOnController>()
        .init_resource::<CameraBasis>()
        .add_systems(Update, net_apply_authority_respawns);

        let mut pose = PoseRuntime::idle();
        pose.pose_id = 117;
        pose.frame = 700.0;
        pose.pose_instance_id = 41;
        let mut air = FighterAir::default();
        air.air.airborne = true;
        air.jump_charge_active = true;
        let mut dodge = FighterDodge::default();
        dodge.state.active = true;
        dodge.pose = 176;
        dodge.pose_entered = true;
        let attack_motion = FighterAttackMotion {
            active: true,
            carry_x: 8.0,
            ..FighterAttackMotion::default()
        };
        let reaction = FighterHitReaction {
            carry_x: 5.0,
            airborne: true,
            ..FighterHitReaction::default()
        };
        let intent = AiIntent {
            want_attack: true,
            ..AiIntent::default()
        };
        let wall = FighterWall {
            cling_active: true,
            ..FighterWall::default()
        };
        let entity = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_xyz(30.0, -12.0, 40.0),
                Health {
                    current: 0.0,
                    max: 1_000.0,
                },
                Qi {
                    current: 80.0,
                    max: 100.0,
                },
                CombatState {
                    state: meteor_core::FighterState::Dead,
                },
                pose,
                SmState {
                    current: meteor_core::FighterState::Dead,
                    locked: true,
                    ..SmState::default()
                },
                DeathRespawn {
                    active: true,
                    timer: 1.0,
                    spawn_pos: Vec3::ZERO,
                },
                TargetHitClock::default(),
                (
                    EntryBlend {
                        duration: 0.4,
                        ..EntryBlend::default()
                    },
                    MotionState {
                        movement_mode: meteor_core::MovementMode::Airborne,
                        velocity: Vec3::new(9.0, -4.0, 3.0),
                        facing: 1.25,
                    },
                    air,
                    dodge,
                    attack_motion,
                    reaction,
                    GunAimState { aiming: true },
                    intent,
                    wall,
                    WeaponSwitchActive::default(),
                ),
            ))
            .id();
        {
            let mut lock = app.world_mut().resource_mut::<LockOnController>();
            lock.system.lock_target = Some(entity.to_bits());
            lock.lock_entity = Some(entity);
            lock.track_entity = Some(entity);
        }

        app.update();

        assert_eq!(app.world().get::<Health>(entity).unwrap().current, 1_000.0);
        assert_eq!(app.world().get::<Qi>(entity).unwrap().current, 0.0);
        assert_eq!(
            app.world().get::<SmState>(entity).unwrap().current,
            meteor_core::FighterState::Name
        );
        let pose = app.world().get::<PoseRuntime>(entity).unwrap();
        assert_eq!(pose.pose_id, 0);
        assert_eq!(pose.pose_instance_id, 42);
        let motion = app.world().get::<MotionState>(entity).unwrap();
        assert_eq!(motion.velocity, Vec3::ZERO);
        assert_eq!(motion.movement_mode, meteor_core::MovementMode::Idle);
        assert_eq!(motion.facing, 1.25);
        assert!(!app.world().get::<FighterAir>(entity).unwrap().air.airborne);
        assert!(
            !app.world()
                .get::<FighterDodge>(entity)
                .unwrap()
                .state
                .active
        );
        assert!(
            !app.world()
                .get::<FighterAttackMotion>(entity)
                .unwrap()
                .active
        );
        assert_eq!(
            app.world()
                .get::<FighterHitReaction>(entity)
                .unwrap()
                .carry_x,
            0.0
        );
        assert!(!app.world().get::<GunAimState>(entity).unwrap().aiming);
        assert!(!app.world().get::<AiIntent>(entity).unwrap().want_attack);
        assert!(!app.world().get::<FighterWall>(entity).unwrap().cling_active);
        assert!(app.world().get::<WeaponSwitchActive>(entity).is_none());
        let lock = app.world().resource::<LockOnController>();
        assert_eq!(lock.system.lock_target, None);
        assert_eq!(lock.lock_entity, None);
        assert_eq!(lock.track_entity, None);
        assert_eq!(app.world().resource::<CameraBasis>().yaw, 1.25);
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            Vec3::new(30.0, -12.0, 40.0),
            "in-place revival preserves position"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_owner_respawn_uses_real_stage_pads_while_cure_stays_in_place() {
        use crate::app_state::{LoadedStageAssets, LoadedStageInfo};
        use crate::plugins::stage_collider::{StageCollider, StageMeshCollider};
        use meteor_network::stock_game::StockGameEvent;
        use meteor_network::stock_protocol::StockRespawnEvent;
        use meteor_physics::collision::MeshCollider;
        for stage_id in ["sn03", "sn06", "sn22"] {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../origin_res")
                .join(stage_id);
            let des = std::fs::read(root.join(format!("{stage_id}.des"))).unwrap();
            let desc =
                meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&des));
            let gmdl = meteor_asset::gmdl::decode_gmdl(
                &std::fs::read(root.join(format!("{stage_id}.gmb"))).unwrap(),
            )
            .unwrap();
            let cob = meteor_asset::cob::parse_cob(
                &std::fs::read(root.join(format!("{stage_id}.cob"))).unwrap(),
            )
            .unwrap();
            let center = meteor_scene::stage_loader::get_stage_center(stage_id, None);
            let corpse = Vec3::new(95.0, -25.0, 93.0);
            for (team, prefix) in [(None, "user"), (Some(1), "teama"), (Some(2), "teamb")] {
                let mut app = App::new();
                app.init_resource::<LockOnController>()
                    .init_resource::<CameraBasis>()
                    .insert_resource(LoadedStageAssets {
                        des: Some(des.clone()),
                        ..Default::default()
                    })
                    .insert_resource(LoadedStageInfo {
                        center,
                        ..Default::default()
                    })
                    .insert_resource(StageCollider {
                        mesh: Some(StageMeshCollider::from_gmdl(
                            &gmdl,
                            &desc,
                            Some(&cob.face_ids),
                            center,
                        )),
                    })
                    .insert_resource(NetState {
                        status: NetStatus::InRoom,
                        wire_format: NetWireFormat::V4,
                        address: "udp://127.0.0.1:5230".into(),
                        player_id: "stock-1".into(),
                        members: vec![RoomMemberWire {
                            player_id: "stock-1".into(),
                            team,
                            name: "Player".into(),
                            character_id: 0,
                            weapon: 6,
                            ready: true,
                            connected: true,
                        }],
                        ..Default::default()
                    })
                    .add_systems(Update, net_apply_authority_respawns);
                let entity = app
                    .world_mut()
                    .spawn((
                        (
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
                            Transform::from_translation(corpse),
                            Health {
                                current: 0.0,
                                ..Default::default()
                            },
                            Qi::default(),
                            DeathRespawn {
                                active: true,
                                spawn_pos: Vec3::new(-80.0, 0.0, -80.0),
                                ..Default::default()
                            },
                            PoseRuntime::idle(),
                            EntryBlend::default(),
                            CombatState::default(),
                            SmState::default(),
                            TargetHitClock::default(),
                        ),
                        (
                            MotionState {
                                movement_mode: meteor_core::MovementMode::Idle,
                                velocity: Vec3::ZERO,
                                facing: 0.0,
                            },
                            FighterAir::default(),
                            FighterDodge::default(),
                            FighterAttackMotion::default(),
                            FighterHitReaction::default(),
                            GunAimState::default(),
                        ),
                    ))
                    .id();
                let mut request = LaunchRequest {
                    stock_rule: if team.is_none() { 1 } else { 2 },
                    ..Default::default()
                };
                let mut end = MatchEnd::default();
                let mut tx = NetTx::default();
                let mut next = NextState::default();
                super::handle_stock_game_event(
                    &mut app.world_mut().resource_mut::<NetState>(),
                    &mut request,
                    &mut end,
                    &mut tx,
                    &mut next,
                    AppState::Playing,
                    StockGameEvent::Respawn(StockRespawnEvent { actor_id: 1 }),
                );
                app.update();
                assert_eq!(
                    app.world().get::<Health>(entity).unwrap().current,
                    0.0,
                    "ordinary revival waits for the owner event, not observer event 46"
                );
                for _ in 0..2 {
                    super::handle_stock_game_event(
                        &mut app.world_mut().resource_mut::<NetState>(),
                        &mut request,
                        &mut end,
                        &mut tx,
                        &mut next,
                        AppState::Playing,
                        StockGameEvent::LocalRespawn,
                    );
                }
                app.update();
                let actual = app.world().get::<Transform>(entity).unwrap().translation;
                let mesh = app
                    .world()
                    .resource::<StageCollider>()
                    .mesh
                    .as_ref()
                    .unwrap();
                let authored = desc.scene_objects.iter().chain(&desc.dummies).any(|pad| {
                    let name = pad.name_attr.as_deref().unwrap_or("").to_ascii_lowercase();
                    if !name
                        .strip_prefix(prefix)
                        .is_some_and(|suffix| suffix.parse::<usize>().is_ok())
                    {
                        return false;
                    }
                    let x = (pad.position.x - center[0]) * meteor_core::constants::MODEL_SCALE;
                    let z = -(pad.position.y - center[1]) * meteor_core::constants::MODEL_SCALE;
                    (actual.x - x).abs() < 1e-4 && (actual.z - z).abs() < 1e-4
                });
                assert!(
                    authored,
                    "{prefix} respawn at {actual:?} must come from the original DES"
                );
                assert!(actual.distance(corpse) > 10.0);
                assert_eq!(
                    app.world().get::<Transform>(entity).unwrap().rotation,
                    Quat::IDENTITY,
                    "the model child remains the only facing owner after respawn"
                );
                assert!(
                    (mesh
                        .sample_ground_hit(actual.x, actual.z, actual.y, actual.y)
                        .unwrap()
                        .y
                        - actual.y)
                        .abs()
                        < 1e-4
                );
                assert_eq!(
                    app.world()
                        .get::<PoseRuntime>(entity)
                        .unwrap()
                        .pose_instance_id,
                    1,
                    "duplicate owner events in one poll reset the actor once"
                );
                assert_eq!(
                    app.world().resource::<CameraBasis>().yaw,
                    app.world().get::<MotionState>(entity).unwrap().facing
                );
                request.stock_rule = 4;
                app.world_mut()
                    .get_mut::<Transform>(entity)
                    .unwrap()
                    .translation = corpse;
                super::handle_stock_game_event(
                    &mut app.world_mut().resource_mut::<NetState>(),
                    &mut request,
                    &mut end,
                    &mut tx,
                    &mut next,
                    AppState::Playing,
                    StockGameEvent::Respawn(StockRespawnEvent { actor_id: 1 }),
                );
                app.update();
                assert_eq!(
                    app.world().get::<Transform>(entity).unwrap().translation,
                    corpse,
                    "Rule 4 Cure preserves the target's position"
                );

                app.world_mut().resource_mut::<NetState>().address = "ws://localhost".into();
                handle_net_message(
                    &mut app.world_mut().resource_mut::<NetState>(),
                    &mut request,
                    &mut end,
                    &mut next,
                    Message::CombatRespawn {
                        player_id: "stock-1".into(),
                        reason: "timer".into(),
                        healer_id: None,
                    },
                    &mut tx,
                    &mut NetInputSequence::default(),
                    &mut PendingNetActions::default(),
                    AppState::Playing,
                );
                app.update();
                let expected_prefix = if team == Some(2) { "teamb" } else { "teama" };
                let expected = desc
                    .scene_objects
                    .iter()
                    .chain(&desc.dummies)
                    .find(|pad| {
                        pad.name_attr
                            .as_deref()
                            .unwrap_or("")
                            .eq_ignore_ascii_case(&format!("{expected_prefix}01"))
                    })
                    .unwrap();
                let actual = app.world().get::<Transform>(entity).unwrap().translation;
                assert!(
                    (actual.x
                        - (expected.position.x - center[0]) * meteor_core::constants::MODEL_SCALE)
                        .abs()
                        < 1e-4
                );
                assert!(
                    (actual.z
                        + (expected.position.y - center[1]) * meteor_core::constants::MODEL_SCALE)
                        .abs()
                        < 1e-4
                );

                for dead in [false, true] {
                    app.world_mut()
                        .get_mut::<Transform>(entity)
                        .unwrap()
                        .translation = corpse;
                    app.world_mut()
                        .get_mut::<DeathRespawn>(entity)
                        .unwrap()
                        .active = dead;
                    app.world_mut().get_mut::<Health>(entity).unwrap().current =
                        if dead { 0.0 } else { 50.0 };
                    {
                        let mut net = app.world_mut().resource_mut::<NetState>();
                        net.match_epoch = Some(1);
                        net.round_epoch = Some(1);
                        net.round_active = false;
                    }
                    let start = Message::RoundStart {
                        match_epoch: Some(1),
                        round_epoch: Some(2),
                        round: 2,
                        round_time: 15,
                        game_num: 3,
                        team_wins: None,
                    };
                    for _ in 0..2 {
                        handle_net_message(
                            &mut app.world_mut().resource_mut::<NetState>(),
                            &mut request,
                            &mut end,
                            &mut next,
                            start.clone(),
                            &mut tx,
                            &mut NetInputSequence::default(),
                            &mut PendingNetActions::default(),
                            AppState::Playing,
                        );
                        app.update();
                    }
                    assert_eq!(
                        app.world().get::<Transform>(entity).unwrap().translation,
                        actual
                    );
                    let health = app.world().get::<Health>(entity).unwrap();
                    assert_eq!(health.current, health.max);
                    assert!(!app.world().get::<DeathRespawn>(entity).unwrap().active);
                    assert!(app
                        .world()
                        .resource::<NetState>()
                        .pending_respawns
                        .is_empty());
                }
            }
        }
    }

    #[test]
    fn authority_death_survives_local_respawn_tick_until_server_revives_player() {
        for with_assets in [false, true] {
            let mut app = App::new();
            app.init_resource::<Time>()
                .insert_resource(LaunchRequest {
                    mode: "leader".into(),
                    ..LaunchRequest::default()
                })
                .init_resource::<MatchEnd>()
                .init_resource::<LockOnController>()
                .init_resource::<CameraBasis>()
                .insert_resource(NetState {
                    status: NetStatus::InRoom,
                    player_id: "local".into(),
                    wire_format: NetWireFormat::V4,
                    round_active: true,
                    pending_hits: vec![AuthorityHit {
                        stock_guard_bucket: None,
                        damage: 1_000.0,
                        health: Some(0.0),
                        anger: None,
                        guarded: false,
                        attacker_id: Some("npc".into()),
                        effect_strategy: None,
                        motion_scale: None,
                        reaction: None,
                        tick: None,
                        impact: true,
                        terminal: true,
                    }],
                    ..NetState::default()
                })
                .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
                .add_systems(
                    Update,
                    (
                        super::net_handle_hit,
                        crate::plugins::combat_plugin::death_respawn,
                        net_apply_authority_respawns,
                    )
                        .chain(),
                );
            if with_assets {
                let root =
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
                let character =
                    crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p0")
                        .expect("p0 death pose assets");
                app.insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
                    std::collections::HashMap::from([("p0".into(), character)]),
                ));
            }
            let position = Vec3::new(10.0, 0.0, 5.0);
            let actor = app
                .world_mut()
                .spawn((
                    (
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
                        Transform::from_translation(position),
                        Health::default(),
                        Qi::default(),
                        DeathRespawn::default(),
                        PoseRuntime::idle(),
                        EntryBlend::default(),
                        CombatState::default(),
                        SmState::default(),
                        TargetHitClock::default(),
                    ),
                    (
                        MotionState {
                            movement_mode: meteor_core::MovementMode::Idle,
                            velocity: Vec3::ZERO,
                            facing: 0.0,
                        },
                        FighterAir::default(),
                        FighterDodge::default(),
                        FighterAttackMotion::default(),
                        FighterHitReaction::default(),
                        GunAimState::default(),
                    ),
                ))
                .id();

            for _ in 0..80 {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_millis(50));
                app.update();
                assert_eq!(app.world().get::<Health>(actor).unwrap().current, 0.0);
                assert!(app.world().get::<DeathRespawn>(actor).unwrap().active);
                assert_eq!(
                    app.world().get::<Transform>(actor).unwrap().translation,
                    position
                );
                assert_eq!(
                    app.world().get::<SmState>(actor).unwrap().current,
                    meteor_core::FighterState::Dead
                );
                if with_assets {
                    assert_eq!(app.world().get::<PoseRuntime>(actor).unwrap().pose_id, 117);
                    assert_eq!(
                        app.world()
                            .get::<FighterHitReaction>(actor)
                            .unwrap()
                            .current
                            .as_ref()
                            .unwrap()
                            .kind,
                        meteor_combat::hit_reaction::HitReactKind::Dead
                    );
                }
            }
            app.world_mut()
                .resource_mut::<NetState>()
                .pending_respawns
                .push("local".into());
            app.update();
            for _ in 0..2 {
                assert!(!app.world().get::<DeathRespawn>(actor).unwrap().active);
                let health = app.world().get::<Health>(actor).unwrap();
                assert_eq!(health.current, health.max);
                let sm = app.world().get::<SmState>(actor).unwrap();
                assert!(!sm.locked);
                assert!(crate::plugins::movement_plugin::ordinary_locomotion_allowed(sm, false));
                assert!(app
                    .world()
                    .get::<FighterHitReaction>(actor)
                    .unwrap()
                    .current
                    .is_none());
                app.update();
            }
        }
    }

    #[test]
    fn production_role_snapshot_updates_motion_without_overwriting_metadata() {
        let state = role_state_from_runtime(RoleStateRuntime {
            position: RoleVec3 {
                x: 4.0,
                y: 1.5,
                z: -2.0,
            },
            forward: RoleVec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            velocity: RoleVec3 {
                x: 3.0,
                y: 0.5,
                z: -1.0,
            },
            pose_id: 24,
            grounded: false,
            guard_route: false,
            playback_arg: 0,
            pose_sequence: 9,
        });
        let mut app = App::new();
        app.insert_resource(NetState {
            pending_role_states: vec![("p2".into(), state, 1)],
            ..NetState::default()
        })
        .add_systems(Update, net_apply_snapshot);
        let entity = app
            .world_mut()
            .spawn((
                RemoteFighter {
                    player_id: "p2".into(),
                },
                Fighter {
                    character_id: 3,
                    player_slot: 1,
                    weapon: 10,
                    weapon_slots: [10, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health {
                    current: 600.0,
                    max: 1000.0,
                },
                PoseRuntime::idle(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                DeathRespawn::default(),
                FighterAir::default(),
                RemoteTransformTarget {
                    translation: Vec3::ZERO,
                    yaw: 0.0,
                },
                Transform::default(),
            ))
            .id();

        app.update();

        let target = app.world().get::<RemoteTransformTarget>(entity).unwrap();
        assert_eq!(target.translation, Vec3::new(4.0, 1.5, -2.0));
        assert!((target.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 24);
        assert_eq!(app.world().get::<Health>(entity).unwrap().current, 600.0);
        assert_eq!(app.world().get::<Fighter>(entity).unwrap().weapon, 10);
        let motion = app.world().get::<MotionState>(entity).unwrap();
        assert_eq!(motion.velocity, Vec3::new(3.0, 0.5, -1.0));
        assert_eq!(motion.movement_mode, meteor_core::MovementMode::Airborne);
        assert!(app.world().get::<FighterAir>(entity).unwrap().air.airborne);
    }

    #[test]
    fn protocol_v6_snapshot_spawns_and_removes_authoritative_npc_actor() {
        let role = role_state_from_runtime(RoleStateRuntime {
            position: RoleVec3 {
                x: 2.0,
                y: 1.0,
                z: 5.0,
            },
            forward: RoleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            velocity: RoleVec3::ZERO,
            pose_id: 144,
            grounded: true,
            guard_route: false,
            playback_arg: 0,
            pose_sequence: 3,
        });
        let mut net = NetState::default();
        handle_v4_snapshot(
            &mut net,
            Snapshot {
                room_id: "room1".into(),
                tick: 1,
                server_time: 1.0,
                players: Vec::new(),
                npcs: vec![SnapshotNpc {
                    id: "npc:7".into(),
                    kind: "npc".into(),
                    name: "夜猫子".into(),
                    character: "p3".into(),
                    weapon: "spear".into(),
                    team: Some(2),
                    vip: true,
                    position: Some(V4Vec3 {
                        x: 2.0,
                        y: 1.0,
                        z: 5.0,
                    }),
                    yaw: 0.0,
                    health: 800.0,
                    max_health: 900.0,
                    kills: 2,
                    deaths: 1,
                    alive: true,
                    pose_id: 144,
                    pose_cursor: 8.0,
                    role_state: encode_event2_state(role).to_vec(),
                }],
                drops: Vec::new(),
                flag: None,
                r#match: None,
                match_epoch: None,
                round_epoch: None,
            },
        );
        assert_eq!(net.remote_actor_profiles.get("npc:7"), Some(&(3, 7)));
        assert_eq!(net.remote_actor_teams.get("npc:7"), Some(&2));
        assert_eq!(
            net.remote_actor_names.get("npc:7").map(String::as_str),
            Some("夜猫子")
        );
        assert!(net.radar_star_players.contains("npc:7"));

        let mut app = App::new();
        app.insert_resource(net)
            .add_systems(Update, net_apply_snapshot);
        app.update();
        let npc_entity = app
            .world_mut()
            .query::<(Entity, &RemoteFighter)>()
            .iter(app.world())
            .find_map(|(entity, remote)| (remote.player_id == "npc:7").then_some(entity))
            .expect("snapshot NPC spawns a remote actor");
        assert_eq!(
            app.world().get::<Fighter>(npc_entity).unwrap().character_id,
            3
        );
        assert_eq!(app.world().get::<Fighter>(npc_entity).unwrap().weapon, 7);
        assert_eq!(
            app.world().get::<Health>(npc_entity).unwrap().current,
            800.0
        );

        {
            let mut net = app.world_mut().resource_mut::<NetState>();
            handle_v4_snapshot(
                &mut net,
                Snapshot {
                    room_id: "room1".into(),
                    tick: 2,
                    server_time: 2.0,
                    players: Vec::new(),
                    npcs: Vec::new(),
                    drops: Vec::new(),
                    flag: None,
                    r#match: None,
                    match_epoch: None,
                    round_epoch: None,
                },
            );
        }
        app.update();
        assert!(app.world().get_entity(npc_entity).is_err());
    }

    #[test]
    fn web_weapon_names_cover_original_loadout_ids() {
        assert_eq!(weapon_id_from_web_name("dart"), 1);
        assert_eq!(weapon_id_from_web_name("staffBlade"), 10);
        assert_eq!(weapon_id_from_web_name("ninjaBlade"), 12);
        assert_eq!(weapon_id_from_web_name("unknown"), 6);
    }

    #[test]
    fn escort_carrier_is_only_taken_from_authoritative_carried_snapshot() {
        let mut net = NetState {
            status: NetStatus::InRoom,
            escort_flag: Some(EscortFlagSnapshot {
                position: [1.0, 2.0, 3.0],
                carrier_id: Some("p2".into()),
                state: "carried".into(),
                return_in: None,
            }),
            ..Default::default()
        };
        assert_eq!(escort_carrier_id(&net), Some("p2"));
        net.escort_flag.as_mut().expect("flag").state = "dropped".into();
        assert_eq!(escort_carrier_id(&net), None);
        net.escort_flag.as_mut().expect("flag").state = "carried".into();
        net.status = NetStatus::Disconnected;
        assert_eq!(escort_carrier_id(&net), None);
    }

    #[test]
    fn role_rate_event_changes_request1_cadence_only_within_original_bounds() {
        let mut net = NetState::default();
        apply_session_role_rate(&mut net, 17);
        assert_eq!(net.role_uplink_hz, 17);
        apply_session_role_rate(&mut net, 7);
        assert_eq!(net.role_uplink_hz, 17);
        apply_session_role_rate(&mut net, 21);
        assert_eq!(net.role_uplink_hz, 17);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_host_channel_drop_releases_port_and_next_room_is_discoverable() {
        let mut bind = "127.0.0.1:0".to_owned();
        for generation in 0..2 {
            let (tx, rx) = std::sync::mpsc::channel();
            let (in_tx, in_rx) = std::sync::mpsc::channel();
            let address = bind.clone();
            let handle = std::thread::spawn(move || {
                run_stock_host_connection(
                    &address,
                    Message::Hello {
                        name: "host".into(),
                        resume_token: None,
                        character_id: 0,
                        weapon: 6,
                    },
                    StockConnectionConfig {
                        password: String::new(),
                        room_name: format!("generation-{generation}"),
                        rule: 1,
                        game_num: 1,
                        game_time_minutes: 999,
                        life_adjust_percent: 200,
                        item_rules: meteor_network::stock_game::StockRoomItemRules::default(),
                        camp: 0,
                        stage_id: "sn06".into(),
                        escort_definition: None,
                    },
                    rx,
                    in_tx,
                )
            });
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            let mut endpoint = None;
            while std::time::Instant::now() < deadline {
                if let Ok(NetInbound::StockGame(StockGameEvent::TransportConnected {
                    peer, ..
                })) = in_rx.recv_timeout(std::time::Duration::from_millis(20))
                {
                    endpoint = Some(peer);
                    break;
                }
            }
            let endpoint = endpoint.expect("host connects on each generation");
            bind = endpoint.to_string();
            let mut browser =
                meteor_network::stock_session::StockDiscoveryClient::bind_with_destination(
                    "127.0.0.1:0",
                    endpoint,
                    "serverlist",
                )
                .unwrap();
            let start = std::time::Instant::now();
            let mut found = false;
            while start.elapsed().as_secs() < 2 {
                if browser
                    .tick(start.elapsed().as_millis() as u64)
                    .unwrap()
                    .iter()
                    .any(|room| room.info.server_name == format!("generation-{generation}"))
                {
                    found = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }

            drop(tx);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !handle.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(
                handle.is_finished(),
                "orphaned host must release its socket"
            );
            handle.join().unwrap();
            assert!(found, "new room must answer discovery");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_stock_host_binds_and_its_local_player_enters() {
        assert_eq!(stock_host_bind_address(), "0.0.0.0:5230");

        let (to_thread_tx, to_thread_rx) = std::sync::mpsc::channel();
        let (to_app_tx, to_app_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            run_stock_host_connection(
                "127.0.0.1:0",
                Message::Hello {
                    name: "host".into(),
                    resume_token: None,
                    character_id: 0,
                    weapon: 6,
                },
                StockConnectionConfig {
                    password: String::new(),
                    room_name: "自定义房间".into(),
                    rule: 3,
                    game_num: 1,
                    game_time_minutes: 999,
                    life_adjust_percent: 200,
                    item_rules: meteor_network::stock_game::StockRoomItemRules::default(),
                    camp: 1,
                    stage_id: "sn04".into(),
                    escort_definition: None,
                },
                to_thread_rx,
                to_app_tx,
            );
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut entered = false;
        let mut endpoint = None;
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            match to_app_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(NetInbound::StockGame(StockGameEvent::RoleEntered(event)))
                    if event.actor_id == 1 && event.hero == 0 =>
                {
                    entered = true;
                    break;
                }
                Ok(NetInbound::StockGame(StockGameEvent::TransportConnected { peer, .. })) => {
                    endpoint = Some(peer)
                }
                Ok(NetInbound::StockGame(event)) => seen.push(format!("stock:{event:?}")),
                Ok(NetInbound::Closed { code, reason }) => {
                    seen.push(format!("closed:{code}:{reason}"))
                }
                Ok(_) => seen.push("other".into()),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let mut advertised_name = None;
        if let Some(endpoint) = endpoint {
            let mut discovery =
                meteor_network::stock_session::StockDiscoveryClient::bind_with_destination(
                    "127.0.0.1:0",
                    endpoint,
                    "serverlist",
                )
                .unwrap();
            let start = std::time::Instant::now();
            while start.elapsed().as_secs() < 2 {
                let rooms = discovery.tick(start.elapsed().as_millis() as u64).unwrap();
                if let Some(room) = rooms.first() {
                    advertised_name = Some(room.info.server_name.clone());
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let _ = to_thread_tx.send(NetOutbound::Control(Message::ObjectiveEnd {
            winner_team: 2,
        }));
        let objective_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut score_seen = false;
        let mut match_end_seen = false;
        while std::time::Instant::now() < objective_deadline && !match_end_seen {
            match to_app_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(NetInbound::StockGame(StockGameEvent::TeamScore(score))) => {
                    score_seen = score.team1_wins == 0 && score.team2_wins == 1;
                }
                Ok(NetInbound::StockGame(StockGameEvent::MatchEnd)) => match_end_seen = true,
                Ok(NetInbound::StockGame(event)) => seen.push(format!("stock:{event:?}")),
                Ok(NetInbound::Closed { code, reason }) => {
                    seen.push(format!("closed:{code}:{reason}"))
                }
                Ok(_) => seen.push("other".into()),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        let _ = to_thread_tx.send(NetOutbound::Quit);
        handle.join().unwrap();
        assert_eq!(advertised_name.as_deref(), Some("自定义房间"));
        let rebound = StockServerTransport::bind(
            endpoint.expect("host must announce its actual bound endpoint"),
            "Meteor Butterfly v1.08 Room",
            "rebind-check",
        )
        .expect("host exit must release its bound UDP port for the next room");
        drop(rebound);
        assert!(
            entered,
            "host local player never completed stock role entry: {seen:?}"
        );
        assert!(score_seen, "host objective did not emit event 34: {seen:?}");
        assert!(
            match_end_seen,
            "host objective did not emit event 40: {seen:?}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_session_cleanup_leaves_room_and_clears_reconnect_identity() {
        use bevy::state::app::AppExtStates;
        let (outbound, outbound_rx) = std::sync::mpsc::channel();
        let (_inbound_tx, inbound) = std::sync::mpsc::channel();
        let (seen_tx, seen_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            while let Ok(message) = outbound_rx.recv() {
                if matches!(message, NetOutbound::Control(Message::RoomLeave)) {
                    let _ = seen_tx.send(message);
                    return;
                }
                if matches!(message, NetOutbound::Quit) {
                    return;
                }
            }
        });
        let mut native_thread = NativeNetThread::default();
        native_thread.install("udp-host://0.0.0.0:5230", handle);

        let net = NetState {
            status: NetStatus::InRoom,
            player_id: "player-1".into(),
            client_id: "client-1".into(),
            session_token: "resume-me".into(),
            room_id: "room-1".into(),
            error: Some("keep visible".into()),
            ..Default::default()
        };
        let net_tx = NetTx {
            active: Some(outbound),
            connecting: None,
        };
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<AppState>()
            .insert_resource(net)
            .insert_resource(net_tx)
            .insert_resource(NetRx(Some(std::sync::Mutex::new(inbound))))
            .insert_resource(NetInputSequence(41))
            .insert_resource(PendingNetActions {
                actions: vec!["switch_weapon".into()],
            })
            .insert_resource(NetChatInput {
                text: "unfinished message".into(),
            })
            .insert_resource(native_thread)
            .init_resource::<StockLanDiscoveryState>()
            .add_systems(
                bevy::state::prelude::OnEnter(AppState::Menu),
                cleanup_network_session.run_if(super::entered_menu_after_transition),
            );
        app.update();
        for state in [AppState::Playing, AppState::Paused] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(state);
            app.update();
            assert_eq!(app.world().resource::<NetState>().status, NetStatus::InRoom);
            assert!(app.world().resource::<NativeNetThread>().handle.is_some());
        }
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::Menu);
        app.update();

        let net = app.world().resource::<NetState>();
        assert_eq!(net.status, NetStatus::Disconnected);
        assert!(net.player_id.is_empty());
        assert!(net.client_id.is_empty());
        assert!(net.session_token.is_empty());
        assert!(net.room_id.is_empty());
        assert_eq!(net.error.as_deref(), Some("keep visible"));
        assert!(matches!(
            seen_rx.try_recv(),
            Ok(NetOutbound::Control(Message::RoomLeave))
        ));
        let net_tx = app.world().resource::<NetTx>();
        assert!(net_tx.active.is_none() && net_tx.connecting.is_none());
        let net_rx = app.world().resource::<NetRx>();
        assert!(net_rx.0.is_none());
        let sequence = app.world().resource::<NetInputSequence>();
        assert_eq!(sequence.0, 0);
        let pending = app.world().resource::<PendingNetActions>();
        assert!(pending.actions.is_empty());
        let chat = app.world().resource::<NetChatInput>();
        assert!(chat.text.is_empty());
        let native_thread = app.world().resource::<NativeNetThread>();
        assert!(native_thread.handle.is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_rule_selection_matches_original_mode_and_stage_gates() {
        let mut request = LaunchRequest::default();
        assert_eq!((request.stock_rule, request.stock_camp), (1, 0));
        assert_eq!(request.stock_life_adjust_percent, 200);
        assert!(
            request.stock_hide_item
                && request.stock_health_item
                && request.stock_update_item
                && request.stock_special_item
        );
        assert_eq!(stock_mode_from_rule(request.stock_rule), "leader");
        assert!(validate_stock_selection(&request).is_ok());

        request.stock_rule = 2;
        request.stock_camp = 1;
        assert!(validate_stock_selection(&request).is_err());
        normalize_stock_stage_for_rule(&mut request);
        assert_eq!(
            (request.stage_id.as_str(), request.stage_name.as_str()),
            ("sn03", "一线天")
        );
        assert!(stock_stage_allowed_for_rule(2, &request.stage_id));
        assert_eq!(stock_mode_from_rule(request.stock_rule), "escort");
        assert!(validate_stock_selection(&request).is_ok());

        request.stock_rule = 3;
        normalize_stock_stage_for_rule(&mut request);
        assert_eq!(request.stage_id, "sn04");
        assert!(validate_stock_selection(&request).is_ok());
        request.stage_id = "sn11".into();
        assert_eq!(stock_mode_from_rule(request.stock_rule), "siege");
        assert!(validate_stock_selection(&request).is_ok());

        request.stock_rule = 5;
        request.stage_id = "sn01".into();
        assert_eq!(stock_mode_from_rule(request.stock_rule), "deathmatch");
        assert!(validate_stock_selection(&request).is_ok());
        assert_eq!(
            stock_config_value_label(StockConfigField::LifeAdjust, &request),
            "200%"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_preparation_reads_direct_host_rule_before_offering_teams() {
        use bevy::prelude::*;
        use meteor_network::{
            stock_game::{StockGameServer, StockHeroCatalog},
            stock_session::StockServerTransport,
        };
        for rule in [1, 5] {
            let transport =
                StockServerTransport::bind("127.0.0.1:0", "serverlist", "rules").unwrap();
            let address = transport.local_addr().unwrap();
            let mut server = StockGameServer::with_rule_and_combat_resources(
                transport,
                StockHeroCatalog::bundled_revision(),
                rule,
                Default::default(),
            );
            let mut app = App::new();
            app.insert_resource(super::PendingStockConnection(Some((
                format!("udp://{address}"),
                LaunchRequest {
                    stock_rule: 0,
                    stock_camp: 2,
                    ..default()
                },
            ))))
            .init_resource::<LaunchRequest>()
            .init_resource::<NetState>()
            .init_resource::<NetTx>()
            .init_resource::<NetRx>()
            .init_resource::<NativeNetThread>()
            .init_resource::<super::StockConnectionDialog>()
            .add_systems(Update, super::menu_stock_loadout_confirmation);
            let section = app
                .world_mut()
                .spawn((Node::default(), super::StockLoadoutCampSection))
                .id();
            app.world_mut().spawn((
                Interaction::Pressed,
                super::StockLoadoutCamp(2),
                crate::plugins::ui_components::UiSelectionState::new(false),
            ));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                app.update();
                server.tick(1).unwrap();
                if app
                    .world()
                    .resource::<super::PendingStockConnection>()
                    .0
                    .as_ref()
                    .unwrap()
                    .1
                    .stock_rule
                    != 0
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let pending = app.world().resource::<super::PendingStockConnection>();
            let selection = &pending.0.as_ref().unwrap().1;
            assert_eq!(selection.stock_rule, rule);
            assert_eq!(selection.stock_camp, if rule == 1 { 0 } else { 2 });
            assert_eq!(
                app.world().get::<Node>(section).unwrap().display,
                if rule == 1 {
                    Display::None
                } else {
                    Display::Flex
                }
            );
            assert_eq!(
                server.transport().peer_count(),
                0,
                "reading room rules must not enter the battle"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_host_confirms_loadout_without_reselecting_camp() {
        use super::{menu_stock_loadout_confirmation, PendingStockConnection};
        use crate::app_state::MenuStockLoadoutConfirm;
        use bevy::prelude::{Display, Interaction, Node};

        let mut app = App::new();
        let selection = LaunchRequest {
            stage_id: "sn04".into(),
            stage_name: "炽雪城".into(),
            stock_rule: 5,
            stock_camp: 1,
            ..Default::default()
        };
        app.insert_resource(PendingStockConnection(Some((
            "udp-host://127.0.0.1:0".into(),
            selection,
        ))))
        .insert_resource(LaunchRequest {
            character_id: 1,
            weapon: 8,
            ..Default::default()
        })
        .init_resource::<NetState>()
        .init_resource::<NetTx>()
        .init_resource::<NetRx>()
        .init_resource::<NativeNetThread>()
        .init_resource::<super::StockConnectionDialog>()
        .add_systems(Update, menu_stock_loadout_confirmation);
        let confirm = app
            .world_mut()
            .spawn((Interaction::None, MenuStockLoadoutConfirm))
            .id();
        let camp_section = app
            .world_mut()
            .spawn((Node::default(), super::StockLoadoutCampSection))
            .id();
        let loadout_dialog = app
            .world_mut()
            .spawn((Node::default(), crate::app_state::MenuStockLoadoutDialog))
            .id();

        app.world_mut().spawn((
            Interaction::Pressed,
            super::StockLoadoutCamp(2),
            crate::plugins::ui_components::UiSelectionState::new(false),
        ));
        app.update();
        assert_eq!(
            app.world().get::<Node>(loadout_dialog).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(camp_section).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().resource::<NetState>().status,
            NetStatus::Disconnected
        );
        assert!(app.world().resource::<NetRx>().0.is_none());

        app.world_mut().resource_mut::<LaunchRequest>().character_id = 2;
        app.world_mut().resource_mut::<LaunchRequest>().weapon = 6;
        *app.world_mut().get_mut::<Interaction>(confirm).unwrap() = Interaction::Pressed;
        app.update();
        assert_eq!(
            app.world().get::<Node>(loadout_dialog).unwrap().display,
            Display::None
        );
        assert!(app.world().resource::<PendingStockConnection>().0.is_none());
        assert!(app
            .world()
            .resource::<super::StockConnectionDialog>()
            .is_open());
        assert_eq!(
            app.world().resource::<NetState>().status,
            NetStatus::Connecting
        );
        let request = app.world().resource::<LaunchRequest>();
        assert_eq!((request.character_id, request.weapon), (2, 6));
        assert_eq!(request.stage_id, "sn04");
        assert_eq!(request.stock_camp, 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut entered = false;
        {
            let receiver = app
                .world()
                .resource::<NetRx>()
                .0
                .as_ref()
                .unwrap()
                .lock()
                .unwrap();
            while std::time::Instant::now() < deadline {
                if let Ok(NetInbound::StockGame(StockGameEvent::RoleEntered(event))) =
                    receiver.recv_timeout(std::time::Duration::from_millis(50))
                {
                    entered = event.hero == 2 && event.camp == 1;
                    break;
                }
            }
        }
        app.world().resource::<NetTx>().disconnect();
        if let Some(handle) = app
            .world_mut()
            .resource_mut::<NativeNetThread>()
            .handle
            .take()
        {
            handle.join().unwrap();
        }
        assert!(
            entered,
            "confirmed character must reach the host through UDP role entry"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_entry_buttons_wait_for_loadout_confirmation_and_cancel_without_connecting() {
        use super::{
            menu_stock_connection_buttons, menu_stock_loadout_confirmation, PendingStockConnection,
        };
        use crate::app_state::{
            MenuStockHostButton, MenuStockJoinButton, MenuStockLoadoutCancel,
            MenuStockLoadoutDialog,
        };
        use bevy::prelude::{Display, Interaction, Node};

        for entry in 0..3 {
            let mut app = App::new();
            let request = LaunchRequest {
                stock_rule: 1,
                stock_camp: 0,
                stock_server_address: "192.168.1.30:5230".into(),
                ..Default::default()
            };
            let initial_stage = request.stage_id.clone();
            let mut discovery = StockLanDiscoveryState::default();
            let mut info = meteor_network::stock_udp::StockDiscoveryInfo::default_room(
                5230,
                "LAN",
                "192.168.2.203",
            );
            info.field_0e = 2;
            info.field_0f = 1;
            discovery.rooms.push(StockDiscoveredRoom {
                endpoint: "192.168.1.20:5230".parse().unwrap(),
                info,
                last_seen_ms: 10,
            });
            app.insert_resource(request)
                .insert_resource(discovery)
                .init_resource::<NetState>()
                .init_resource::<NetTx>()
                .init_resource::<NetRx>()
                .init_resource::<NativeNetThread>()
                .init_resource::<PendingStockConnection>()
                .init_resource::<super::StockConnectionDialog>()
                .add_systems(
                    Update,
                    (
                        menu_stock_connection_buttons,
                        menu_stock_loadout_confirmation,
                    )
                        .chain(),
                );
            let room_target = super::RoomTarget::Lan(
                app.world().resource::<StockLanDiscoveryState>().rooms[0].clone(),
            );
            let button = match entry {
                0 => app
                    .world_mut()
                    .spawn((Interaction::Pressed, MenuStockHostButton))
                    .id(),
                1 => app
                    .world_mut()
                    .spawn((Interaction::Pressed, MenuStockJoinButton))
                    .id(),
                _ => app
                    .world_mut()
                    .spawn((Interaction::Pressed, room_target))
                    .id(),
            };
            let dialog = app
                .world_mut()
                .spawn((Node::default(), MenuStockLoadoutDialog))
                .id();
            let cancel = app
                .world_mut()
                .spawn((Interaction::None, MenuStockLoadoutCancel))
                .id();
            app.update();
            assert_eq!(
                app.world().get::<Node>(dialog).unwrap().display,
                Display::Flex
            );
            assert_eq!(
                app.world().resource::<NetState>().status,
                NetStatus::Disconnected
            );
            assert!(app.world().resource::<NetRx>().0.is_none());
            let (address, selection) = app
                .world()
                .resource::<PendingStockConnection>()
                .0
                .as_ref()
                .unwrap();
            assert_eq!(
                address,
                match entry {
                    0 => "udp-host://0.0.0.0:5230",
                    1 => "udp://192.168.1.30:5230",
                    _ => "udp://192.168.1.20:5230",
                }
            );
            if entry == 2 {
                assert_eq!(selection.stage_id, initial_stage);
                assert_eq!(
                    selection.stock_camp, 2,
                    "join the less populated advertised camp"
                );
            }
            let captured_address = address.clone();
            app.world_mut()
                .resource_mut::<StockLanDiscoveryState>()
                .rooms
                .clear();
            *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::None;
            app.update();
            assert_eq!(
                app.world()
                    .resource::<PendingStockConnection>()
                    .0
                    .as_ref()
                    .unwrap()
                    .0,
                captured_address
            );
            *app.world_mut().get_mut::<Interaction>(cancel).unwrap() = Interaction::Pressed;
            app.update();
            assert!(app.world().resource::<PendingStockConnection>().0.is_none());
            assert_eq!(
                app.world().get::<Node>(dialog).unwrap().display,
                Display::None
            );
            assert_eq!(
                app.world().resource::<NetState>().status,
                NetStatus::Disconnected
            );
            assert!(app.world().resource::<NetRx>().0.is_none());
            assert_eq!(
                app.world().resource::<LaunchRequest>().stage_id,
                initial_stage
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn discovered_stock_room_syncs_rule_and_camp_and_keeps_local_stage() {
        let mut request = LaunchRequest {
            stage_id: "sn03".into(),
            stage_name: "一线天".into(),
            ..Default::default()
        };
        let mut room = StockDiscoveredRoom {
            endpoint: "192.168.2.203:5230".parse().unwrap(),
            info: meteor_network::stock_udp::StockDiscoveryInfo::default_room(
                5230,
                "LAN 房间",
                "192.168.2.203",
            ),
            last_seen_ms: 10,
        };
        room.info.fields_0_to_6[5] = 1;
        apply_stock_discovered_room(&mut request, &room);
        assert_eq!(
            (request.stage_id.as_str(), request.stage_name.as_str()),
            ("sn03", "一线天"),
            "discovery record carries no stage; local selection is kept"
        );
        assert_eq!(request.stock_camp, 0);

        room.info.fields_0_to_6[5] = 5;
        room.info.field_0e = 2;
        room.info.field_0f = 1;
        apply_stock_discovered_room(&mut request, &room);
        assert_eq!(
            request.stock_camp, 2,
            "join the less populated advertised camp"
        );

        room.info.field_0e = 1;
        room.info.field_0f = 3;
        apply_stock_discovered_room(&mut request, &room);
        assert_eq!(request.stock_camp, 1);
        assert_eq!(request.stock_rule, 5);
        room.info.field_0e = 0;
        room.info.field_0f = 0;
        apply_stock_discovered_room(&mut request, &room);
        assert_eq!((request.stock_rule, request.stock_camp), (5, 1));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_stage_config_replaces_default_map_before_loading() {
        let mut net = NetState {
            player_id: "stock-1".into(),
            ..Default::default()
        };
        let mut request = LaunchRequest {
            stage_id: "sn01".into(),
            stage_name: "钟乳洞".into(),
            ..Default::default()
        };
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next = NextState::Unchanged;
        for event in [
            StockGameEvent::StageConfig(meteor_network::stock_protocol::StockStageConfigEvent {
                stage_id: "sn06".into(),
                field_a: 0,
                field_b: 0,
                field_c: 0,
            }),
            StockGameEvent::RoleEntered(meteor_network::stock_protocol::StockRoleEnteredEvent {
                actor_id: 1,
                hero: 0,
                hero_checksum: 0,
                camp: 0,
            }),
        ] {
            handle_stock_game_event(
                &mut net,
                &mut request,
                &mut end,
                &mut tx,
                &mut next,
                AppState::Menu,
                event,
            );
        }
        assert_eq!(request.stage_id, "sn06");
        assert_eq!(request.stage_name, "四方阵");
        assert_eq!(net.stage, "sn06");
        assert!(matches!(next, NextState::Pending(AppState::Loading)));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_event56_conversion_preserves_rule3_scene_operations() {
        let message_ops = vec![
            SceneOp {
                kind: "set".into(),
                reference: serde_json::Value::from(42),
                field: "pose".into(),
                args: vec![serde_json::Value::from(8), serde_json::Value::from(0)],
            },
            SceneOp {
                kind: "set".into(),
                reference: serde_json::Value::from(42),
                field: "attribute".into(),
                args: vec![
                    serde_json::Value::String("collision".into()),
                    serde_json::Value::from(0),
                ],
            },
            SceneOp {
                kind: "effect".into(),
                reference: serde_json::Value::from(42),
                field: "GiMaBRK".into(),
                args: vec![serde_json::Value::from(0)],
            },
        ];
        let stock = stock_scene_ops_from_message(&message_ops, 0x1234_5678);
        assert_eq!(stock.timestamp_ms, 0x1234_5678);
        assert_eq!(stock.operations.len(), 3);
        assert_eq!(stock.operations[0].kind, STOCK_SCENE_OP_POSE);
        assert_eq!(stock.operations[1].kind, STOCK_SCENE_OP_COLLISION);
        assert_eq!(stock.operations[2].value, 701);
        assert_eq!(scene_ops_from_stock(&stock), message_ops);

        let mut net = NetState {
            address: "udp-host://0.0.0.0:37105".into(),
            mode: "siege".into(),
            ..Default::default()
        };
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next_state = NextState::<AppState>::default();
        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::SceneOps(stock.clone()),
        );
        assert!(net.pending_scene_ops.is_empty());
        net.mode = "deathmatch".into();
        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::SceneOps(stock.clone()),
        );
        assert_eq!(
            net.pending_scene_ops, message_ops,
            "ordinary host must apply remote transactions"
        );
        net.pending_scene_ops.clear();
        net.address = "udp://127.0.0.1:37105".into();
        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::SceneOps(stock),
        );
        assert_eq!(net.pending_scene_ops, message_ops);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_event34_and_terminal_events_drive_match_state() {
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: "stock-1".into(),
            round_active: true,
            ..Default::default()
        };
        let mut request = LaunchRequest {
            stock_rule: 5,
            stock_camp: 1,
            ..Default::default()
        };
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next_state = NextState::<AppState>::default();
        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::TeamScore(meteor_network::stock_protocol::StockTeamScoreEvent {
                team1_wins: 1,
                team2_wins: 0,
            }),
        );
        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::MatchEnd,
        );
        assert_eq!(net.stock_team_wins, [1, 0]);
        assert!(!net.round_active);
        assert!(end.settled);
        assert_eq!(end.result, meteor_network::modes::ROUND_RESULT_TEAM2_WIPED);

        request.stock_rule = 3;
        net.pending_objective_end = Some(2);
        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::RoundRestart,
        );
        assert!(net.round_active);
        assert_eq!(net.pending_objective_end, Some(2));
        assert_eq!(net.pending_respawns, ["stock-1"]);
        assert!(!end.settled);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_escort_events_drive_flag_view_and_radar_carrier() {
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: "stock-1".into(),
            round_active: true,
            ..Default::default()
        };
        let mut request = LaunchRequest {
            stock_rule: 2,
            stock_camp: 1,
            ..Default::default()
        };
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next_state = NextState::<AppState>::default();
        let record = meteor_network::stock_protocol::StockSceneItemRecord::from_role_position(
            7,
            2,
            4,
            SN03.flag_home,
        );

        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::EscortGrounded {
                record,
                dropped: false,
            },
        );
        assert_eq!(
            net.escort_flag.as_ref().map(|flag| flag.state.as_str()),
            Some("home")
        );

        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::EscortTaken(meteor_network::stock_protocol::StockSceneTakenEvent {
                actor_id: 2,
                record_id: 7,
                category: 4,
                item_id: 2,
            }),
        );
        assert_eq!(escort_carrier_id(&net), Some("stock-2"));
        assert!(net.radar_star_players.contains("stock-2"));

        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::EscortGrounded {
                record,
                dropped: true,
            },
        );
        let flag = net.escort_flag.as_ref().unwrap();
        assert_eq!(flag.state, "dropped");
        assert_eq!(flag.return_in, Some(5.0));
        assert!(!net.radar_star_players.contains("stock-2"));

        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::EscortReturned,
        );
        let flag = net.escort_flag.as_ref().unwrap();
        assert_eq!(flag.state, "home");
        assert_eq!(
            flag.position,
            [SN03.flag_home.x, SN03.flag_home.y, SN03.flag_home.z]
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_ordinary_scene_pickup_is_queued_with_event29_position() {
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: "stock-1".into(),
            round_active: true,
            ..Default::default()
        };
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next_state = NextState::<AppState>::default();
        let record = meteor_network::stock_protocol::StockSceneItemRecord::from_role_position(
            77,
            1,
            2,
            RoleVec3 {
                x: 1.25,
                y: 2.5,
                z: -3.75,
            },
        );

        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::SceneItemsGrounded {
                records: vec![record],
                spawned: false,
            },
        );
        assert_eq!(net.pending_stock_scene_grounded.len(), 1);
        let observed = record.role_position();
        let observed = [observed.x, observed.y, observed.z];
        assert_eq!(net.pending_stock_scene_grounded[0].position, observed);

        let taken = meteor_network::stock_protocol::StockSceneTakenEvent {
            actor_id: 1,
            record_id: 77,
            category: 2,
            item_id: 1,
        };
        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::SceneItemTaken {
                event: taken,
                record: Some(record),
            },
        );
        assert_eq!(
            net.pending_stock_scene_items,
            [StockSceneItemPickup {
                actor_id: 1,
                record_id: 77,
                category: 2,
                item_id: 1,
                position: Some(observed),
            }]
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_event35_replaces_each_camp_vip_and_restart_clears_markers() {
        let mut net = NetState::default();
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next_state = NextState::<AppState>::default();
        for event in [
            meteor_network::stock_protocol::StockVipEvent {
                camp: 1,
                actor_id: 11,
            },
            meteor_network::stock_protocol::StockVipEvent {
                camp: 2,
                actor_id: 22,
            },
            meteor_network::stock_protocol::StockVipEvent {
                camp: 1,
                actor_id: 33,
            },
        ] {
            handle_stock_game_event(
                &mut net,
                &mut request,
                &mut end,
                &mut tx,
                &mut next_state,
                AppState::Playing,
                StockGameEvent::Vip(event),
            );
        }
        assert_eq!(
            net.stock_vips,
            [Some("stock-33".into()), Some("stock-22".into())]
        );
        assert_eq!(
            net.radar_star_players,
            ["stock-22".to_owned(), "stock-33".to_owned()]
                .into_iter()
                .collect::<std::collections::HashSet<_>>()
        );

        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::RoundRestart,
        );
        assert_eq!(net.stock_vips, [None, None]);
        assert!(net.radar_star_players.is_empty());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_event6_removes_remote_member_and_queues_fighter_despawn() {
        let mut net = NetState {
            player_id: "stock-1".into(),
            members: vec![
                RoomMemberWire {
                    connected: true,
                    player_id: "stock-1".into(),
                    name: "local".into(),
                    character_id: 0,
                    weapon: 6,
                    team: Some(1),
                    ready: true,
                },
                RoomMemberWire {
                    connected: true,
                    player_id: "stock-2".into(),
                    name: "remote".into(),
                    character_id: 1,
                    weapon: 5,
                    team: Some(2),
                    ready: true,
                },
            ],
            stock_vips: [None, Some("stock-2".into())],
            radar_star_players: ["stock-2".to_owned()].into_iter().collect(),
            stock_deaths: [("stock-2".to_owned(), 3)].into_iter().collect(),
            ..Default::default()
        };
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next_state = NextState::<AppState>::default();

        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::RoleLeft(meteor_network::stock_protocol::StockRoleLeftEvent {
                actor_id: 2,
            }),
        );

        assert_eq!(net.pending_remote_despawns, ["stock-2"]);
        assert_eq!(
            net.members
                .iter()
                .map(|member| member.player_id.as_str())
                .collect::<Vec<_>>(),
            ["stock-1"]
        );
        assert_eq!(net.stock_vips, [None, None]);
        assert!(!net.radar_star_players.contains("stock-2"));
        assert!(!net.stock_deaths.contains_key("stock-2"));

        handle_stock_game_event(
            &mut net,
            &mut request,
            &mut end,
            &mut tx,
            &mut next_state,
            AppState::Playing,
            StockGameEvent::RoleRate(18),
        );
        assert_eq!(net.role_uplink_hz, 18);
    }

    #[test]
    fn ninja_self_effect_uplink_follows_damage_on_the_same_reliable_route() {
        use super::{net_send_self_effects, PendingSelfEffect};
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut app = App::new();
        app.insert_resource(NetState {
            status: NetStatus::InRoom,
            room_id: "room1".into(),
            round_active: true,
            match_epoch: Some(7),
            round_epoch: Some(3),
            ..NetState::default()
        })
        .insert_resource(NetTx {
            active: Some(sender),
            connecting: None,
        })
        .insert_resource(NetInputSequence(100))
        .add_systems(Update, (net_send_hits, net_send_self_effects).chain());
        let fighter = || Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 12,
            weapon_slots: [12, 6],
            weapon_slot: 0,
            weapon_variant: 0,
            weapon_slot_variants: [0, 0],
        };
        app.world_mut().spawn((
            LocalFighter,
            fighter(),
            Transform::default(),
            PendingSelfEffect {
                resource_id: 35,
                pose_id: 468,
                pose_instance: 17,
            },
        ));
        app.world_mut().spawn((
            RemoteFighter {
                player_id: "p2".into(),
            },
            fighter(),
            Transform::default(),
            MotionState {
                facing: 0.0,
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
            },
            super::PendingRemoteHit {
                damage: 650.0,
                pose_id: 468,
                attack_index: 0,
                frame: 15820.0,
                cursor: 34.0,
                attack_origin: Vec3::ZERO,
                attack_forward: Vec3::Z,
            },
        ));
        app.update();
        let NetOutbound::OrderedInput(hit) = receiver.try_recv().unwrap() else {
            panic!("damage must be first and bypass unordered P2P");
        };
        let NetOutbound::Control(Message::RoleSelfEffect { seq, pose_id, .. }) =
            receiver.try_recv().unwrap()
        else {
            panic!("self effect must follow damage");
        };
        assert_eq!(pose_id, 468);
        assert!(seq > hit.seq);
        assert_eq!(hit.combat[0].pose_id, 468);
        app.update();
        assert!(
            receiver.try_recv().is_err(),
            "neither hit nor self effect repeats"
        );
    }

    #[test]
    fn stock_hit_sender_queries_are_schedule_compatible() {
        let mut app = App::new();
        app.insert_resource(NetState::default())
            .insert_resource(NetTx::default())
            .insert_resource(NetInputSequence::default())
            .add_systems(Update, net_send_hits);
        app.update();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_lan_discovery_menu_queries_are_schedule_compatible() {
        let mut app = App::new();
        app.insert_resource(MenuScreen::Main)
            .init_resource::<StockLanDiscoveryState>()
            .add_systems(Update, menu_stock_lan_discovery);

        app.update();
    }

    #[test]
    fn stock_event25_updates_only_local_health_and_anger() {
        let mut app = App::new();
        app.insert_resource(NetState {
            pending_stock_vitals: Some([875.0, 2000.0, 42.0]),
            ..Default::default()
        })
        .add_systems(Update, net_apply_stock_vitals);
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                Health {
                    current: 1.0,
                    max: 1.0,
                },
                Qi::default(),
            ))
            .id();
        app.update();
        let health = app.world().get::<Health>(local).unwrap();
        assert_eq!((health.current, health.max), (875.0, 2000.0));
        assert_eq!(app.world().get::<Qi>(local).unwrap().current, 42.0);
        assert!(app
            .world()
            .resource::<NetState>()
            .pending_stock_vitals
            .is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_remote_vitals_apply_after_spawn_and_preserve_local_authority() {
        use meteor_network::stock_protocol::StockVitalsEvent;
        use meteor_network::stock_remote_vitals::RemoteVitals;
        let mut app = App::new();
        app.insert_resource(NetState {
            address: "udp://127.0.0.1:5230".into(),
            player_id: "stock-1".into(),
            pending_role_entries: vec!["stock-2".into()],
            remote_actor_profiles: [("stock-2".into(), (4, 0))].into(),
            ..Default::default()
        })
        .add_systems(
            Update,
            (net_apply_snapshot, super::net_apply_remote_vitals).chain(),
        );
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                Health {
                    current: 1800.0,
                    max: 2000.0,
                },
                Qi::default(),
            ))
            .id();
        let mut request = LaunchRequest::default();
        let mut end = MatchEnd::default();
        let mut tx = NetTx::default();
        let mut next = NextState::default();
        for (health, max_health, anger) in [
            (753, 2500, 42),
            (0, 2500, 0),
            (2500, 2500, 0),
            (2700, 3000, 100),
        ] {
            for actor_id in [1, 2] {
                handle_stock_game_event(
                    &mut app.world_mut().resource_mut::<NetState>(),
                    &mut request,
                    &mut end,
                    &mut tx,
                    &mut next,
                    AppState::Playing,
                    StockGameEvent::RemoteVitals(RemoteVitals {
                        actor_id,
                        revision: 1,
                        vitals: StockVitalsEvent {
                            health,
                            max_health,
                            anger,
                        },
                    }),
                );
            }
            app.update();
            let world = app.world_mut();
            let (hp, qi, status) = world
                .query_filtered::<(&Health, &Qi, &super::RemoteVitalsStatus), With<RemoteFighter>>()
                .single(world)
                .unwrap();
            assert_eq!(
                (hp.current, hp.max, qi.current),
                (health as f32, max_health as f32, anger as f32)
            );
            assert!(status.health_known && status.anger_known);
            assert_eq!(world.get::<Health>(local).unwrap().current, 1800.0);
            assert!(world
                .resource::<NetState>()
                .pending_remote_vitals
                .is_empty());
        }
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_remote_despawns
            .push("stock-2".into());
        app.world_mut()
            .resource_mut::<NetState>()
            .pending_remote_vitals
            .insert("stock-2".into(), [1.0; 3]);
        app.update();
        assert!(app
            .world()
            .resource::<NetState>()
            .pending_remote_vitals
            .is_empty());
    }

    #[test]
    fn stock_event23_updates_local_and_remote_score_once() {
        let mut app = App::new();
        app.insert_resource(NetState {
            player_id: "stock:1".into(),
            pending_stock_kills: vec![
                ("stock:1".into(), "stock:2".into()),
                ("stock:2".into(), "stock:1".into()),
            ],
            ..Default::default()
        })
        .insert_resource(crate::components::fighter::KillScore::default())
        .add_systems(Update, net_apply_stock_scores);
        app.update();

        assert_eq!(
            app.world()
                .resource::<crate::components::fighter::KillScore>()
                .player_kills,
            1
        );
        let net = app.world().resource::<NetState>();
        assert_eq!(net.remote_kills.get("stock:2"), Some(&1));
        assert_eq!(net.stock_deaths.get("stock:1"), Some(&1));
        assert_eq!(net.stock_deaths.get("stock:2"), Some(&1));
        assert!(net.pending_stock_kills.is_empty());

        app.update();
        assert_eq!(
            app.world()
                .resource::<crate::components::fighter::KillScore>()
                .player_kills,
            1,
            "a reliable event edge must not be applied twice"
        );
    }
}
