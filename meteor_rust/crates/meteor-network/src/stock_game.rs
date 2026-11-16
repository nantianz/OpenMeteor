use crate::escort::{
    character_capsule_intersects_flag, position_inside_safe_zone, SN03, STOCK_ESCORT_CATEGORY,
    STOCK_ESCORT_ITEM_ID, STOCK_ESCORT_SCENE_OBJECT_ID,
};
use crate::modes::can_respawn;
use crate::role_state::{RoleState, RoleVec3, ROLE_DOWNLINK_HZ, ROLE_UPLINK_HZ};
use crate::stock_file_transfer::{
    Frame as FileFrame, Receiver as FileReceiver, Sender as FileSender,
};
use crate::stock_protocol::{
    decode_role_state_event, decode_role_state_request, decode_stock_roster,
    encode_role_state_event, encode_role_state_request, stock_base_weapon_item_id,
    stock_resource_checksum, stock_weapon_from_item_id, StockCureRequest, StockDropItemEvent,
    StockEnterRoleRequest, StockEquipWeaponRequest, StockFlagRemovedEvent, StockFlagReturnedEvent,
    StockHitReactionEvent, StockHitRequest, StockKillEvent, StockProtocolError, StockRespawnEvent,
    StockRoleEnteredEvent, StockRoleLeftEvent, StockRoleRate, StockRoleStateRecord,
    StockSceneFrame, StockSceneItemRecord, StockSceneItemSpawnRequest, StockSceneOperation,
    StockSceneOpsEvent, StockSceneStateEvent, StockSceneTakenEvent, StockSceneTransferEvent,
    StockStageConfigEvent, StockTeamScoreEvent, StockVipEvent, StockVitalsEvent,
    StockWeaponEquippedEvent, StockWeaponObjectRequest, STOCK_DISCRETE_RELIABLE_FLAGS,
    STOCK_EVENT_BE_HIT, STOCK_EVENT_DEATH, STOCK_EVENT_DROP_ITEM, STOCK_EVENT_DROP_ITEM_FLAGS,
    STOCK_EVENT_ENVIRONMENT_DEATH, STOCK_EVENT_FLAG_REMOVED, STOCK_EVENT_FLAG_RETURNED,
    STOCK_EVENT_GUARD_HIT, STOCK_EVENT_HIT_FLAGS, STOCK_EVENT_KILL, STOCK_EVENT_MATCH_END,
    STOCK_EVENT_RESPAWN, STOCK_EVENT_ROLE_ENTERED, STOCK_EVENT_ROLE_LEFT, STOCK_EVENT_ROLE_RATE,
    STOCK_EVENT_ROLE_STATE, STOCK_EVENT_ROLE_STATE_FLAGS, STOCK_EVENT_ROSTER,
    STOCK_EVENT_ROUND_RESTART, STOCK_EVENT_ROUND_TIMER, STOCK_EVENT_SCENE_OPS,
    STOCK_EVENT_SCENE_SNAPSHOT, STOCK_EVENT_SCENE_SPAWN, STOCK_EVENT_SCENE_STATE,
    STOCK_EVENT_SCENE_TAKEN, STOCK_EVENT_SCENE_TRANSFER, STOCK_EVENT_STAGE_CONFIG,
    STOCK_EVENT_TEAM_SCORE, STOCK_EVENT_VIP, STOCK_EVENT_VITALS, STOCK_EVENT_WEAPON_EQUIPPED,
    STOCK_EVENT_WEAPON_OBJECT, STOCK_EVENT_WEAPON_OBJECT_FLAGS, STOCK_REQUEST_CURE,
    STOCK_REQUEST_DROP_ITEM, STOCK_REQUEST_ENTER_ROLE, STOCK_REQUEST_EQUIP_WEAPON,
    STOCK_REQUEST_HIT, STOCK_REQUEST_HIT_FLAGS, STOCK_REQUEST_READY, STOCK_REQUEST_ROLE_RATE,
    STOCK_REQUEST_ROLE_STATE, STOCK_REQUEST_ROLE_STATE_FLAGS, STOCK_REQUEST_SPAWN_SCENE_ITEM,
    STOCK_REQUEST_WEAPON_OBJECT, STOCK_SCENE_OPS_PER_PACKET, STOCK_SCENE_OP_EFFECT,
};
use crate::stock_protocol::{
    StockAngerSpend, STOCK_REQUEST_ANGER_COST, STOCK_REQUEST_ANGER_FULL, STOCK_REQUEST_QI_BURST,
};
use crate::stock_protocol::{
    StockStageChecksumRejected, StockStageChecksumRequest, STOCK_EVENT_STAGE_CHECKSUM_REJECTED,
    STOCK_REQUEST_STAGE_CHECKSUM,
};
use crate::stock_session::{
    StockClientTransport, StockServerTransport, StockSessionEvent, StockTransportError,
};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};

fn stock_sequence_reached(ack: u16, sequence: u16) -> bool {
    let modulus = u32::from(crate::stock_udp::STOCK_SEQUENCE_MAX);
    ack != 0 && (u32::from(ack) + modulus - u32::from(sequence)) % modulus < modulus / 2
}
use meteor_asset::pos::AttackBlock;
use meteor_asset::res::{CombatResourceTable, MeteorRes};
use meteor_combat::hit_resolve::{CombatStats, HitOptions};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::path::Path;

#[path = "stock_game/runtime_sync.rs"]
mod runtime_sync;

pub const STOCK_HERO_COUNT: usize = 20;

#[derive(Default)]
pub struct StockRoleSendPacer {
    pending: Option<RoleState>,
    next_us: Option<u64>,
}

impl StockRoleSendPacer {
    pub fn submit(&mut self, state: RoleState) {
        self.pending = Some(state);
    }

    pub fn take_due(&mut self, now_us: u64, hz: u32) -> Option<RoleState> {
        let deadline = *self.next_us.get_or_insert(now_us);
        if now_us < deadline {
            return None;
        }
        let period = 1_000_000 / u64::from(hz.clamp(STOCK_ROLE_RATE_MIN, STOCK_ROLE_RATE_MAX));
        self.next_us = Some(deadline + ((now_us - deadline) / period + 1) * period);
        self.pending.take()
    }
}

pub const STOCK_ROLE_DOWNLINK_INTERVAL_MS: u64 = 1_000 / ROLE_DOWNLINK_HZ as u64;
pub const STOCK_ROLE_RATE_MIN: u32 = 8;
pub const STOCK_ROLE_RATE_MAX: u32 = 20;
pub const STOCK_ROLE_RATE_SAMPLE_MS: u64 = 1_000;
pub const STOCK_CLIENT_RATE_FEEDBACK_THRESHOLD: i32 = 10;
pub const STOCK_HOST_RATE_FEEDBACK_THRESHOLD: i32 = 3;
pub const STOCK_DEFAULT_MAX_HEALTH: f32 = 2_000.0;
pub const STOCK_DEFAULT_GAME_NUM: u32 = 999;
pub const STOCK_DEFAULT_GAME_TIME_MINUTES: u32 = 999;

pub fn stock_max_health_for_life_adjust(percent: u32) -> f32 {
    (1_000.0 * percent.clamp(10, 500) as f32 / 100.0).max(1.0)
}

pub const STOCK_CURE_RANGE: f32 = 80.0 * crate::role_state::ROLE_WORLD_SCALE;
pub const STOCK_CURE_RECOVERY_MS: u64 = 3_034;
pub const STOCK_SCENE_PICKUP_RADIUS: f32 = 16.0 * crate::role_state::ROLE_WORLD_SCALE;
pub const STOCK_SCENE_PICKUP_HEIGHT: f32 = meteor_core::constants::CHARACTER_CONTACT_HEIGHT;
pub const STOCK_SCENE_RECORDS_PER_PACKET: usize = 58;
pub const STOCK_WEAPON_THROW_TIMEOUT_MS: u64 = 5_000;
pub const STOCK_DYNAMIC_ITEM_LIFETIME_MS: u64 = 30_000;

fn retain_scene_write(state: &mut Vec<StockSceneOperation>, operation: StockSceneOperation) {
    if !(1..STOCK_SCENE_OP_EFFECT).contains(&operation.kind) {
        return;
    }
    if operation.kind == crate::stock_protocol::STOCK_SCENE_OP_POSE {
        state.retain(|previous| {
            previous.reference != operation.reference
                || previous.kind != crate::stock_protocol::STOCK_SCENE_OP_FRAME
        });
    }
    if let Some(index) = state.iter().position(|previous| {
        previous.reference == operation.reference && previous.kind == operation.kind
    }) {
        state.remove(index);
    }
    state.push(operation);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockRoomItemGroup {
    Hide,
    Health,
    Update,
    Special,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockRoomItemRules {
    pub hide_item: bool,
    pub health_item: bool,
    pub update_item: bool,
    pub special_item: bool,
}

impl Default for StockRoomItemRules {
    fn default() -> Self {
        Self {
            hide_item: true,
            health_item: true,
            update_item: true,
            special_item: true,
        }
    }
}

impl StockRoomItemRules {
    pub fn allows(self, category: u32, item_id: u32) -> bool {
        match stock_room_item_group(category, item_id) {
            Some(StockRoomItemGroup::Hide) => self.hide_item,
            Some(StockRoomItemGroup::Health) => self.health_item,
            Some(StockRoomItemGroup::Update) => self.update_item,
            Some(StockRoomItemGroup::Special) => self.special_item,
            None => true,
        }
    }

    fn scene_option_bits(self) -> u32 {
        [
            self.health_item,
            self.update_item,
            self.special_item,
            self.hide_item,
        ]
        .into_iter()
        .enumerate()
        .filter(|(_, enabled)| *enabled)
        .filter_map(|(key, _)| crate::stock_protocol::stock_scene_item_option_bit(key as i32))
        .fold(0, |flags, bit| flags | bit)
    }
}

pub fn stock_room_item_group(category: u32, item_id: u32) -> Option<StockRoomItemGroup> {
    match category {
        1 => match stock_weapon_from_item_id(item_id)
            .map(|(weapon, _)| meteor_combat::weapon_strategy::weapon_to_strategy(weapon))
        {
            Some(0) => Some(StockRoomItemGroup::Health),
            Some(1) => Some(StockRoomItemGroup::Update),
            Some(2) => Some(StockRoomItemGroup::Special),
            Some(3) => Some(StockRoomItemGroup::Hide),
            _ => None,
        },
        2 => match item_id {
            1 | 9 | 16..=18 | 23..=27 => Some(StockRoomItemGroup::Health),
            2..=5 | 28..=33 | 39 => Some(StockRoomItemGroup::Update),
            6..=8 => Some(StockRoomItemGroup::Special),
            _ => None,
        },
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StockAuthoredSceneItem {
    pub item_id: u32,
    pub category: u32,
    pub position: RoleVec3,
    pub half_extents: [f32; 3],
    pub respawn_ms: Option<u64>,

    pub scene_reference: Option<u32>,

    pub repeat_on_xstar: bool,

    pub active: bool,
    pub interactive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StockWeaponDropCommand {
    pub item_id: u32,
    pub position: RoleVec3,
    pub forward: RoleVec3,
    pub copy_only: bool,
    pub remaining_item_id: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StockWeaponThrowEvent {
    pub throw_id: u32,
    pub actor_id: u32,
    pub item_id: u32,
    pub position: RoleVec3,
    pub forward: RoleVec3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockHeroCatalog {
    checksums: [u32; STOCK_HERO_COUNT],
    stage_checksums: Option<std::collections::BTreeMap<String, u32>>,
}

impl StockHeroCatalog {
    pub fn bundled_revision() -> Self {
        Self::from_checksums(crate::stock_protocol::STOCK_REVISION_HERO_CHECKSUMS)
    }
    pub fn packed_revision() -> Result<Self, String> {
        Ok(Self {
            checksums: meteor_asset::runtime_assets::stock_checksums()?,
            stage_checksums: Some(meteor_asset::runtime_assets::stock_stage_checksums()?),
        })
    }

    pub fn from_origin_res(origin_res: impl AsRef<Path>) -> Result<Self, StockGameError> {
        let mut checksums = [0u32; STOCK_HERO_COUNT];
        for (hero, checksum) in checksums.iter_mut().enumerate() {
            let path = origin_res
                .as_ref()
                .join("pmodel")
                .join(format!("P{hero}.POS"));
            *checksum = stock_resource_checksum(&fs::read(path)?);
        }
        Ok(Self {
            checksums,
            stage_checksums: None,
        })
    }

    pub fn from_checksums(checksums: [u32; STOCK_HERO_COUNT]) -> Self {
        Self {
            checksums,
            stage_checksums: None,
        }
    }

    pub fn checksum(&self, hero: u32) -> Option<u32> {
        self.checksums.get(hero as usize).copied()
    }

    pub fn accepts(&self, request: StockEnterRoleRequest) -> bool {
        self.checksum(request.hero) == Some(request.hero_checksum)
    }
}

const REQUEST_BATTLE_RULES: u16 = 0x7f06;
const EVENT_BATTLE_RULES: u16 = 0x7f07;
const ACK_BATTLE_RULES: u16 = 0x7f08;
const BATTLE_RULES_V7: &[u8; 4] = b"MBR7";
const BATTLE_RULES_TRANSFER_ID: u32 = 1;
const BATTLE_RULES_ZLIB_KIND: u16 = 2;

const MAX_RULES_COMPRESSED_BYTES: u64 =
    meteor_combat::battle_rules::MAX_RULES_JSON_BYTES as u64 + 64 * 1024;

fn compress_battle_rules(rules: meteor_combat::battle_rules::BattleRules) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(rules.to_json().as_bytes())
        .expect("Vec writer cannot fail");
    encoder.finish().expect("Vec writer cannot fail")
}

fn decode_battle_rules(compressed: &[u8]) -> Option<meteor_combat::battle_rules::BattleRules> {
    if compressed.len() as u64 > MAX_RULES_COMPRESSED_BYTES {
        return None;
    }
    let mut decoder = ZlibDecoder::new(compressed);
    let mut json = String::new();
    decoder
        .by_ref()
        .take((meteor_combat::battle_rules::MAX_RULES_JSON_BYTES + 1) as u64)
        .read_to_string(&mut json)
        .ok()?;
    if json.len() > meteor_combat::battle_rules::MAX_RULES_JSON_BYTES
        || decoder.total_in() != compressed.len() as u64
    {
        return None;
    }
    let rules = meteor_combat::battle_rules::BattleRules::from_json(&json).ok()?;
    (rules.to_json() == json).then_some(rules)
}

const REQUEST_PLAYER_NAMES: u16 = 0x7f02;
const EVENT_PLAYER_NAME: u16 = 0x7f03;
const PLAYER_NAMES_V1: &[u8; 4] = b"MRN1";
const PLAYER_NAMES_V2: &[u8; 4] = b"MRN2";

fn decode_player_name(payload: &[u8]) -> Option<StockGameEvent> {
    if !(9..=776).contains(&payload.len()) || &payload[..4] != PLAYER_NAMES_V1 {
        return None;
    }
    let actor_id = u32::from_le_bytes(payload[4..8].try_into().ok()?);
    let name = std::str::from_utf8(&payload[8..]).ok()?;
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return None;
    }
    Some(StockGameEvent::PlayerName {
        actor_id,
        name: name.into(),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum StockGameEvent {
    BattleRules(meteor_combat::battle_rules::BattleRules),
    PlayerName {
        actor_id: u32,
        name: String,
    },
    RemoteVitals(crate::stock_remote_vitals::RemoteVitals),
    Runtime(crate::runtime_sync::RuntimePacket),
    Latencies(Vec<(u32, u32)>),
    PropertyEffect(crate::stock_protocol::StockPropertyEffectEvent),
    PoseChain(crate::stock_protocol::StockPoseChainEvent),
    TransportConnected {
        peer: SocketAddr,
        transport_id: u32,
    },
    RoleEntered(StockRoleEnteredEvent),
    RoleLeft(StockRoleLeftEvent),

    StageConfig(StockStageConfigEvent),
    StageChecksumRejected(StockStageChecksumRejected),
    RoleRate(u32),
    WeaponEquipped(StockWeaponEquippedEvent),
    RoleStates(Vec<StockRoleStateRecord>),
    HitReaction {
        guarded: bool,
        event: StockHitReactionEvent,
    },
    Kill(StockKillEvent),
    EnvironmentDeath(u32),
    TeamScore(StockTeamScoreEvent),
    Vip(StockVipEvent),
    EscortGrounded {
        record: StockSceneItemRecord,
        dropped: bool,
    },
    EscortTaken(StockSceneTakenEvent),
    EscortReturned,

    SceneItemsGrounded {
        records: Vec<StockSceneItemRecord>,
        spawned: bool,
    },

    SceneItemTaken {
        event: StockSceneTakenEvent,
        record: Option<StockSceneItemRecord>,
    },

    SceneItemRemoved(crate::stock_protocol::StockSceneItemRemovedEvent),
    WeaponThrown(StockWeaponThrowEvent),
    WeaponSettled {
        throw_id: u32,
        record_id: u32,
    },
    SceneOps(StockSceneOpsEvent),
    SceneState(StockSceneStateEvent),
    SceneTransfer(StockSceneTransferEvent),

    RoundEnd,
    RoundRestart,
    MatchEnd,

    RoundTimer {
        remaining_seconds: u32,
        ended: bool,
    },
    LocalDeath,

    LocalRespawn,

    SpawnPad(Option<crate::spawn::SpawnPad>),
    Vitals(StockVitalsEvent),
    Respawn(StockRespawnEvent),
    PeerDisconnected {
        peer: SocketAddr,
        reason: u32,
    },
    RawApplication {
        peer: SocketAddr,
        opcode: u16,
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, Copy)]
struct PendingLocalWeaponDrop {
    throw: StockWeaponThrowEvent,
    expires_at_ms: u64,
    copy_only: bool,
}

pub struct StockGameClient {
    last_latency_request_ms: Option<u64>,
    rules_transfer: Option<FileReceiver<Vec<u8>>>,
    rules_received: Option<u64>,
    runtime_enabled: bool,
    runtime_generation: Option<u32>,
    remote_vitals_enabled: bool,
    player_names_enabled: bool,
    remote_vitals_revisions: HashMap<u32, u32>,
    pending_anger_spends: VecDeque<(u16, StockAngerSpend)>,
    sent_anger_keys: VecDeque<(u32, u16)>,
    last_vitals_sequence: Option<u16>,
    transport: StockClientTransport,
    enter_request: StockEnterRoleRequest,
    weapon_item_id: u32,
    transport_id: Option<u32>,
    actor_id: Option<u32>,
    stage_checksums: Option<std::collections::BTreeMap<String, u32>>,
    checked_stage: Option<String>,
    enter_sent: bool,
    weapon_sent: bool,
    ready_sent: bool,
    camps: HashMap<u32, u32>,
    states: HashMap<u32, RoleState>,
    weapons: HashMap<u32, u32>,
    scene_items: HashMap<u32, StockSceneItemRecord>,
    pending_weapon_drops: Vec<PendingLocalWeaponDrop>,
    observed_weapon_throws: VecDeque<StockWeaponThrowEvent>,
    next_weapon_throw_id: u32,
    dead_actors: HashSet<u32>,
    round_restart_at_ms: Option<u64>,
    pending_spawn_pads: Vec<Option<crate::spawn::SpawnPad>>,
    vips: [Option<u32>; 2],
    role_rate: u32,
    role_rate_sample_ms: Option<u64>,
    role_rate_feedback: i32,
}

impl StockGameClient {
    pub fn new(
        transport: StockClientTransport,
        enter_request: StockEnterRoleRequest,
        weapon_item_id: u32,
    ) -> Self {
        Self {
            pending_anger_spends: VecDeque::new(),
            sent_anger_keys: VecDeque::new(),
            last_vitals_sequence: None,
            last_latency_request_ms: None,
            rules_transfer: None,
            rules_received: None,
            runtime_enabled: false,
            runtime_generation: None,
            remote_vitals_enabled: true,
            player_names_enabled: true,
            remote_vitals_revisions: HashMap::new(),
            transport,
            enter_request,
            weapon_item_id,
            transport_id: None,
            actor_id: None,
            stage_checksums: None,
            checked_stage: None,
            enter_sent: false,
            weapon_sent: false,
            ready_sent: false,
            camps: HashMap::new(),
            states: HashMap::new(),
            weapons: HashMap::new(),
            scene_items: HashMap::new(),
            pending_weapon_drops: Vec::new(),
            observed_weapon_throws: VecDeque::new(),
            next_weapon_throw_id: 1,
            dead_actors: HashSet::new(),
            round_restart_at_ms: None,
            pending_spawn_pads: Vec::new(),
            vips: [None, None],
            role_rate: ROLE_UPLINK_HZ,
            role_rate_sample_ms: None,
            role_rate_feedback: 0,
        }
    }

    pub fn from_origin_res(
        transport: StockClientTransport,
        origin_res: impl AsRef<Path>,
        hero: u32,
        camp: u32,
        weapon: u8,
    ) -> Result<Self, StockGameError> {
        let enter_request = StockEnterRoleRequest::from_origin_res(origin_res, hero, camp)?;
        let weapon_item_id =
            stock_base_weapon_item_id(weapon).ok_or(StockGameError::InvalidWeapon(weapon))?;
        Ok(Self::new(transport, enter_request, weapon_item_id))
    }

    pub fn for_bundled_revision(
        transport: StockClientTransport,
        hero: u32,
        camp: u32,
        weapon: u8,
    ) -> Result<Self, StockGameError> {
        let enter_request = StockEnterRoleRequest::for_bundled_revision(hero, camp)?;
        let weapon_item_id =
            stock_base_weapon_item_id(weapon).ok_or(StockGameError::InvalidWeapon(weapon))?;
        Ok(Self::new(transport, enter_request, weapon_item_id))
    }

    pub fn for_packed_revision(
        transport: StockClientTransport,
        hero: u32,
        camp: u32,
        weapon: u8,
    ) -> Result<Self, StockGameError> {
        let enter_request = StockEnterRoleRequest::for_packed_revision(hero, camp)?;
        let weapon_item_id =
            stock_base_weapon_item_id(weapon).ok_or(StockGameError::InvalidWeapon(weapon))?;
        let mut client = Self::new(transport, enter_request, weapon_item_id);
        client.stage_checksums = Some(
            meteor_asset::runtime_assets::stock_stage_checksums()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
        Ok(client)
    }

    pub fn transport_id(&self) -> Option<u32> {
        self.transport_id
    }

    pub fn transport(&self) -> &StockClientTransport {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut StockClientTransport {
        &mut self.transport
    }

    pub fn actor_id(&self) -> Option<u32> {
        self.actor_id
    }

    pub fn role_rate(&self) -> u32 {
        self.role_rate
    }

    pub fn set_runtime_sync_enabled(&mut self, enabled: bool) {
        self.runtime_enabled = enabled;
    }
    pub fn runtime_generation(&self) -> Option<u32> {
        self.runtime_generation
    }
    pub fn send_runtime(
        &mut self,
        packet: crate::runtime_sync::RuntimePacket,
        now: u64,
    ) -> Result<(), StockGameError> {
        if self.runtime_generation.is_some() {
            self.transport.send_application(
                crate::runtime_sync::REQUEST,
                packet.encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now,
            )?;
        }
        Ok(())
    }

    pub fn set_remote_vitals_enabled(&mut self, enabled: bool) {
        self.remote_vitals_enabled = enabled;
    }

    pub fn set_player_names_enabled(&mut self, enabled: bool) {
        self.player_names_enabled = enabled;
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<Vec<StockGameEvent>, StockGameError> {
        let session_events = self.transport.tick(now_ms)?;
        if self.transport.state() == crate::stock_session::StockClientState::Connected
            && self
                .last_latency_request_ms
                .is_some_and(|at| now_ms.saturating_sub(at) >= 2000)
        {
            self.last_latency_request_ms = Some(now_ms);
            self.transport.send_application(
                crate::stock_latency::REQUEST,
                crate::stock_latency::MAGIC.to_vec(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        let mut events = Vec::new();

        if self
            .round_restart_at_ms
            .is_some_and(|deadline| now_ms >= deadline)
        {
            self.round_restart_at_ms = None;
            self.sent_anger_keys.clear();
            self.pending_anger_spends.clear();
            self.dead_actors.clear();
            self.vips = [None, None];
            self.pending_weapon_drops.clear();
            self.observed_weapon_throws.clear();
            events.push(StockGameEvent::RoundRestart);
            events.extend(
                self.pending_spawn_pads
                    .drain(..)
                    .map(StockGameEvent::SpawnPad),
            );
        }
        for event in session_events {
            match event {
                StockSessionEvent::Connected { peer, transport_id } => {
                    self.transport_id = Some(transport_id);
                    self.rules_transfer = None;
                    self.rules_received = None;
                    self.transport.send_application(
                        REQUEST_BATTLE_RULES,
                        BATTLE_RULES_V7.to_vec(),
                        STOCK_DISCRETE_RELIABLE_FLAGS,
                        now_ms,
                    )?;

                    self.last_latency_request_ms = Some(now_ms);
                    self.runtime_generation = None;
                    if self.runtime_enabled {
                        self.transport.send_application(
                            crate::runtime_sync::REQUEST,
                            crate::runtime_sync::RuntimePacket::Hello.encode(),
                            STOCK_DISCRETE_RELIABLE_FLAGS,
                            now_ms,
                        )?;
                    }
                    if self.remote_vitals_enabled {
                        self.transport.send_application(
                            crate::stock_remote_vitals::REQUEST_REMOTE_VITALS,
                            crate::stock_remote_vitals::REMOTE_VITALS_V1.to_vec(),
                            STOCK_DISCRETE_RELIABLE_FLAGS,
                            now_ms,
                        )?;
                    }
                    if self.player_names_enabled {
                        let mut payload = PLAYER_NAMES_V2.to_vec();
                        payload.extend_from_slice(self.transport.player_name().as_bytes());
                        self.transport.send_application(
                            REQUEST_PLAYER_NAMES,
                            payload,
                            STOCK_DISCRETE_RELIABLE_FLAGS,
                            now_ms,
                        )?;
                        self.transport.send_application(
                            REQUEST_PLAYER_NAMES,
                            PLAYER_NAMES_V1.to_vec(),
                            STOCK_DISCRETE_RELIABLE_FLAGS,
                            now_ms,
                        )?;
                    }
                    if self.stage_checksums.is_none() {
                        self.send_enter_role(now_ms)?;
                        self.send_ready(now_ms)?;
                    }
                    events.push(StockGameEvent::TransportConnected { peer, transport_id });
                }
                StockSessionEvent::Application { peer, packet } => match packet.opcode {
                    EVENT_BATTLE_RULES => {
                        if self.transport.state()
                            != crate::stock_session::StockClientState::Connected
                        {
                            continue;
                        }
                        let frame = match FileFrame::decode(&packet.payload) {
                            Ok(frame) if frame.id() == BATTLE_RULES_TRANSFER_ID => frame,
                            _ => {
                                self.transport.disconnect(0x80)?;
                                continue;
                            }
                        };
                        if let Some(len) = self.rules_received {
                            self.transport.send_application(
                                ACK_BATTLE_RULES,
                                FileFrame::Ack {
                                    id: BATTLE_RULES_TRANSFER_ID,
                                    next: len,
                                }
                                .encode(),
                                STOCK_DISCRETE_RELIABLE_FLAGS,
                                now_ms,
                            )?;
                            continue;
                        }
                        if let FileFrame::Offer {
                            kind: BATTLE_RULES_ZLIB_KIND,
                            ..
                        } = &frame
                        {
                            if self.rules_transfer.is_none() {
                                self.rules_transfer = FileReceiver::new(
                                    &frame,
                                    Vec::new(),
                                    MAX_RULES_COMPRESSED_BYTES,
                                )
                                .ok();
                            }
                            let Some(receiver) = &self.rules_transfer else {
                                self.transport.disconnect(0x80)?;
                                continue;
                            };
                            self.transport.send_application(
                                ACK_BATTLE_RULES,
                                receiver.ack().encode(),
                                STOCK_DISCRETE_RELIABLE_FLAGS,
                                now_ms,
                            )?;
                            continue;
                        }
                        let Some(receiver) = &mut self.rules_transfer else {
                            self.transport.disconnect(0x80)?;
                            continue;
                        };
                        let ack = match receiver.receive(&frame) {
                            Ok(ack) => ack,
                            Err(_) => {
                                self.transport.disconnect(0x80)?;
                                continue;
                            }
                        };
                        if receiver.complete() {
                            let full = self
                                .rules_transfer
                                .take()
                                .expect("completed transfer exists")
                                .into_inner();
                            let rules = decode_battle_rules(&full);
                            let Some(rules) = rules else {
                                self.transport.disconnect(0x80)?;
                                continue;
                            };
                            self.rules_received = Some(full.len() as u64);
                            events.push(StockGameEvent::BattleRules(rules));
                        }
                        self.transport.send_application(
                            ACK_BATTLE_RULES,
                            ack.encode(),
                            STOCK_DISCRETE_RELIABLE_FLAGS,
                            now_ms,
                        )?;
                    }
                    crate::stock_latency::EVENT => {
                        if let Some(samples) = crate::stock_latency::decode(&packet.payload) {
                            events.push(StockGameEvent::Latencies(samples));
                        }
                    }
                    crate::runtime_sync::EVENT if self.runtime_enabled => {
                        if let Some(packet) =
                            crate::runtime_sync::RuntimePacket::decode(&packet.payload)
                        {
                            if let crate::runtime_sync::RuntimePacket::Welcome {
                                generation, ..
                            } = &packet
                            {
                                self.runtime_generation = Some(*generation);
                            }
                            events.push(StockGameEvent::Runtime(packet));
                        }
                    }
                    EVENT_PLAYER_NAME if self.player_names_enabled => {
                        if let Some(event) = decode_player_name(&packet.payload) {
                            events.push(event);
                        }
                    }
                    crate::stock_remote_vitals::EVENT_REMOTE_VITALS
                        if self.remote_vitals_enabled =>
                    {
                        if let Some(sample) =
                            crate::stock_remote_vitals::RemoteVitals::decode(&packet.payload)
                        {
                            let previous = self.remote_vitals_revisions.get(&sample.actor_id);
                            if previous.is_none_or(|previous| sample.newer_than(*previous)) {
                                self.remote_vitals_revisions
                                    .insert(sample.actor_id, sample.revision);
                                events.push(StockGameEvent::RemoteVitals(sample));
                            }
                        }
                    }
                    0 => {
                        let (actor_id, name) =
                            crate::stock_protocol::decode_stock_member_joined(&packet.payload)?;
                        events.push(StockGameEvent::PlayerName { actor_id, name });
                    }
                    STOCK_EVENT_ROSTER => {
                        for member in decode_stock_roster(&packet.payload)? {
                            events.push(StockGameEvent::PlayerName {
                                actor_id: member.actor_id,
                                name: member.name.clone(),
                            });
                            self.camps.insert(member.actor_id, member.camp);
                            if member.entered {
                                events.push(StockGameEvent::RoleEntered(StockRoleEnteredEvent {
                                    actor_id: member.actor_id,
                                    hero: member.hero,
                                    hero_checksum: 0,
                                    camp: member.camp,
                                }));
                                if member.weapon_item_id != 0 {
                                    self.weapons.insert(member.actor_id, member.weapon_item_id);
                                    events.push(StockGameEvent::WeaponEquipped(
                                        StockWeaponEquippedEvent {
                                            actor_id: member.actor_id,
                                            weapon_item_id: member.weapon_item_id,
                                        },
                                    ));
                                }
                            }
                        }
                    }
                    STOCK_EVENT_ROLE_ENTERED => {
                        let entered = StockRoleEnteredEvent::decode(&packet.payload)?;
                        self.camps.insert(entered.actor_id, entered.camp);

                        if Some(entered.actor_id) == self.transport_id
                            && entered.hero == self.enter_request.hero
                        {
                            self.actor_id = Some(entered.actor_id);

                            self.role_rate_sample_ms = Some(now_ms);
                            self.send_initial_weapon(now_ms)?;
                        }
                        events.push(StockGameEvent::RoleEntered(entered));
                    }
                    STOCK_EVENT_ROLE_LEFT => {
                        let left = StockRoleLeftEvent::decode(&packet.payload)?;
                        self.camps.remove(&left.actor_id);
                        self.states.remove(&left.actor_id);
                        self.weapons.remove(&left.actor_id);
                        self.observed_weapon_throws
                            .retain(|throw| throw.actor_id != left.actor_id);
                        self.dead_actors.remove(&left.actor_id);
                        for vip in &mut self.vips {
                            if *vip == Some(left.actor_id) {
                                *vip = None;
                            }
                        }
                        events.push(StockGameEvent::RoleLeft(left));
                    }
                    STOCK_EVENT_ROLE_RATE => {
                        let rate = StockRoleRate::decode(&packet.payload)?;
                        if let Some(hz) = rate.session_hz() {
                            self.role_rate = hz;
                            events.push(StockGameEvent::RoleRate(hz));
                        }
                    }
                    STOCK_EVENT_STAGE_CHECKSUM_REJECTED => {
                        events.push(StockGameEvent::StageChecksumRejected(
                            StockStageChecksumRejected::decode(&packet.payload)?,
                        ));
                    }
                    STOCK_EVENT_STAGE_CONFIG => {
                        let config = StockStageConfigEvent::decode(&packet.payload)?;
                        if let Some(checksums) = &self.stage_checksums {
                            let stage = config.stage_id.to_ascii_lowercase();
                            let checksum = *checksums.get(&stage).ok_or_else(|| {
                                StockGameError::MissingStageChecksum(config.stage_id.clone())
                            })?;
                            if self.checked_stage.as_ref() != Some(&stage) {
                                self.transport.send_application(
                                    STOCK_REQUEST_STAGE_CHECKSUM,
                                    StockStageChecksumRequest { checksum }.encode().to_vec(),
                                    STOCK_DISCRETE_RELIABLE_FLAGS,
                                    now_ms,
                                )?;
                                self.checked_stage = Some(stage);
                            }
                            self.send_enter_role(now_ms)?;
                            self.send_ready(now_ms)?;
                        }
                        events.push(StockGameEvent::StageConfig(config));
                    }
                    STOCK_EVENT_WEAPON_EQUIPPED => {
                        let equipped = StockWeaponEquippedEvent::decode(&packet.payload)?;
                        self.weapons
                            .insert(equipped.actor_id, equipped.weapon_item_id);
                        events.push(StockGameEvent::WeaponEquipped(equipped));
                    }
                    STOCK_EVENT_ROLE_STATE => {
                        let records = decode_role_state_event(&packet.payload)?;
                        for record in &records {
                            self.states.insert(u32::from(record.actor_id), record.state);
                        }
                        events.push(StockGameEvent::RoleStates(records));
                    }
                    crate::stock_protocol::STOCK_EVENT_POSE_CHAIN => {
                        events.push(StockGameEvent::PoseChain(
                            crate::stock_protocol::StockPoseChainEvent::decode(&packet.payload)?,
                        ));
                    }
                    STOCK_EVENT_BE_HIT | STOCK_EVENT_GUARD_HIT => {
                        events.push(StockGameEvent::HitReaction {
                            guarded: packet.opcode == STOCK_EVENT_GUARD_HIT,
                            event: StockHitReactionEvent::decode(&packet.payload)?,
                        });
                    }
                    crate::stock_protocol::STOCK_EVENT_PROPERTY_EFFECT => {
                        events.push(StockGameEvent::PropertyEffect(
                            crate::stock_protocol::StockPropertyEffectEvent::decode(
                                &packet.payload,
                            )?,
                        ));
                    }
                    STOCK_EVENT_ENVIRONMENT_DEATH => {
                        let event = StockRoleLeftEvent::decode(&packet.payload)?;
                        if self.dead_actors.insert(event.actor_id) {
                            events.push(StockGameEvent::EnvironmentDeath(event.actor_id));
                        }
                    }
                    STOCK_EVENT_KILL => {
                        let event = StockKillEvent::decode(&packet.payload)?;
                        self.dead_actors.insert(event.target_actor_id);
                        events.push(StockGameEvent::Kill(event));
                    }
                    STOCK_EVENT_TEAM_SCORE => events.push(StockGameEvent::TeamScore(
                        StockTeamScoreEvent::decode(&packet.payload)?,
                    )),
                    STOCK_EVENT_VIP => {
                        let event = StockVipEvent::decode(&packet.payload)?;
                        if let Some(index) = event
                            .camp
                            .checked_sub(1)
                            .and_then(|camp| self.vips.get_mut(camp as usize))
                        {
                            *index = Some(event.actor_id);
                        }
                        events.push(StockGameEvent::Vip(event));
                    }
                    STOCK_EVENT_SCENE_SNAPSHOT | STOCK_EVENT_SCENE_SPAWN => {
                        let records = StockSceneItemRecord::decode_many(&packet.payload)?;
                        let dropped = packet.opcode == STOCK_EVENT_SCENE_SPAWN;
                        let mut ordinary = Vec::new();
                        for record in records {
                            if dropped && record.category == 1 {
                                if let Some(index) = self
                                    .observed_weapon_throws
                                    .iter()
                                    .position(|throw| throw.item_id == record.item_id)
                                {
                                    if let Some(settled) = self.observed_weapon_throws.remove(index)
                                    {
                                        events.push(StockGameEvent::WeaponSettled {
                                            throw_id: settled.throw_id,
                                            record_id: record.record_id,
                                        });
                                    }
                                }
                            }
                            self.scene_items.insert(record.record_id, record);
                            if is_stock_escort_record(&record) {
                                events.push(StockGameEvent::EscortGrounded { record, dropped });
                            } else if matches!(record.category, 1 | 2) {
                                ordinary.push(record);
                            }
                        }
                        if !ordinary.is_empty() {
                            events.push(StockGameEvent::SceneItemsGrounded {
                                records: ordinary,
                                spawned: dropped,
                            });
                        }
                    }
                    STOCK_EVENT_SCENE_TAKEN => {
                        let event = StockSceneTakenEvent::decode(&packet.payload)?;
                        let record = self.scene_items.remove(&event.record_id);
                        if is_stock_escort_key(event.category, event.item_id) {
                            events.push(StockGameEvent::EscortTaken(event));
                        } else if matches!(event.category, 1 | 2) {
                            events.push(StockGameEvent::SceneItemTaken { event, record });
                        }
                    }
                    crate::stock_protocol::STOCK_EVENT_SCENE_ITEM_REMOVED => {
                        let event = crate::stock_protocol::StockSceneItemRemovedEvent::decode(
                            &packet.payload,
                        )?;
                        self.scene_items.remove(&event.record_id);
                        events.push(StockGameEvent::SceneItemRemoved(event));
                    }
                    STOCK_EVENT_FLAG_RETURNED => {
                        let event = StockFlagReturnedEvent::decode(&packet.payload)?;
                        if is_stock_escort_key(event.category, event.item_id) {
                            events.push(StockGameEvent::EscortReturned);
                        }
                    }
                    STOCK_EVENT_FLAG_REMOVED => {
                        StockFlagRemovedEvent::decode(&packet.payload)?;
                        events.push(StockGameEvent::EscortReturned);
                    }
                    STOCK_EVENT_DROP_ITEM => {
                        let dropped = StockDropItemEvent::decode(&packet.payload)?;
                        if let Some(event) = self.weapon_throw_from_event55(dropped.actor_id) {
                            events.push(StockGameEvent::WeaponThrown(event));
                        }
                    }
                    STOCK_EVENT_SCENE_TRANSFER => events.push(StockGameEvent::SceneTransfer(
                        StockSceneTransferEvent::decode(&packet.payload)?,
                    )),
                    STOCK_EVENT_SCENE_OPS => events.push(StockGameEvent::SceneOps(
                        StockSceneOpsEvent::decode(&packet.payload)?,
                    )),
                    STOCK_EVENT_SCENE_STATE => {
                        let state = StockSceneStateEvent::decode(&packet.payload)?;
                        if !state.objects.is_empty() {
                            events.push(StockGameEvent::SceneState(state));
                        }
                    }
                    STOCK_EVENT_ROUND_RESTART => {
                        require_empty_stock_payload(&packet.payload)?;
                        if self.round_restart_at_ms.is_none() {
                            self.scene_items.clear();
                            self.round_restart_at_ms = Some(now_ms.saturating_add(10_000));
                            events.push(StockGameEvent::RoundEnd);
                        }
                    }
                    STOCK_EVENT_MATCH_END => {
                        require_empty_stock_payload(&packet.payload)?;
                        self.round_restart_at_ms = None;
                        self.pending_spawn_pads.clear();
                        events.push(StockGameEvent::MatchEnd);
                    }
                    STOCK_EVENT_DEATH => events.push(StockGameEvent::LocalDeath),
                    STOCK_EVENT_VITALS => {
                        if self
                            .last_vitals_sequence
                            .is_none_or(|last| stock_sequence_reached(packet.sequence, last))
                        {
                            self.last_vitals_sequence = Some(packet.sequence);
                            let mut vitals = StockVitalsEvent::decode(&packet.payload)?;
                            self.pending_anger_spends.retain(|(sequence, _)| {
                                !stock_sequence_reached(packet.ack, *sequence)
                            });
                            vitals.anger = self
                                .pending_anger_spends
                                .iter()
                                .fold(f32::from(vitals.anger), |anger, (_, spend)| {
                                    spend.remaining(anger)
                                }) as u16;
                            events.push(StockGameEvent::Vitals(vitals));
                        }
                    }
                    STOCK_EVENT_ROUND_TIMER => {
                        if packet.payload.len() < 8 {
                            return Err(StockProtocolError::InvalidLength {
                                expected: 8,
                                actual: packet.payload.len(),
                            }
                            .into());
                        }
                        let remaining_seconds =
                            u32::from_le_bytes(packet.payload[0..4].try_into().unwrap());
                        let ended =
                            u32::from_le_bytes(packet.payload[4..8].try_into().unwrap()) != 0;
                        events.push(StockGameEvent::RoundTimer {
                            remaining_seconds,
                            ended,
                        });
                    }
                    crate::stock_protocol::STOCK_EVENT_SPAWN_PAD => {
                        let pad = crate::stock_protocol::decode_spawn_pad(&packet.payload)?;
                        if self.round_restart_at_ms.is_some() {
                            self.pending_spawn_pads.push(pad);
                        } else {
                            events.push(StockGameEvent::SpawnPad(pad));
                        }
                    }
                    crate::stock_protocol::STOCK_EVENT_LOCAL_RESPAWN => {
                        if !packet.payload.is_empty() {
                            return Err(StockProtocolError::InvalidLength {
                                expected: 0,
                                actual: packet.payload.len(),
                            }
                            .into());
                        }
                        self.sent_anger_keys.clear();
                        self.pending_anger_spends.clear();
                        if let Some(actor) = self.actor_id {
                            self.dead_actors.remove(&actor);
                        }
                        events.push(StockGameEvent::LocalRespawn);
                    }
                    STOCK_EVENT_RESPAWN => {
                        let event = StockRespawnEvent::decode(&packet.payload)?;
                        if Some(event.actor_id) == self.actor_id {
                            self.sent_anger_keys.clear();
                            self.pending_anger_spends.clear();
                        }
                        self.dead_actors.remove(&event.actor_id);
                        events.push(StockGameEvent::Respawn(event));
                    }
                    opcode => events.push(StockGameEvent::RawApplication {
                        peer,
                        opcode,
                        payload: packet.payload,
                    }),
                },
                StockSessionEvent::Disconnected { peer, reason } => {
                    events.push(StockGameEvent::PeerDisconnected { peer, reason });
                }
                StockSessionEvent::PeerConnected { .. }
                | StockSessionEvent::DiscoveryReply { .. } => {}
            }
        }
        let expired: Vec<_> = self
            .pending_weapon_drops
            .iter()
            .filter(|drop| now_ms > drop.expires_at_ms)
            .map(|drop| drop.throw.throw_id)
            .collect();
        self.pending_weapon_drops
            .retain(|drop| now_ms <= drop.expires_at_ms);
        self.observed_weapon_throws
            .retain(|throw| !expired.contains(&throw.throw_id));
        self.maybe_send_role_rate(now_ms)?;
        Ok(events)
    }

    fn maybe_send_role_rate(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        if self.actor_id.is_none() {
            return Ok(());
        }
        let Some(last_sample) = self.role_rate_sample_ms else {
            self.role_rate_sample_ms = Some(now_ms);
            return Ok(());
        };
        if now_ms.saturating_sub(last_sample) < STOCK_ROLE_RATE_SAMPLE_MS {
            return Ok(());
        }
        self.role_rate_sample_ms = Some(now_ms);
        self.role_rate_feedback += if self.transport.take_health_sample() {
            1
        } else {
            -1
        };
        let requested = if self.role_rate_feedback >= STOCK_CLIENT_RATE_FEEDBACK_THRESHOLD {
            Some(self.role_rate + 1)
        } else if self.role_rate_feedback <= -STOCK_CLIENT_RATE_FEEDBACK_THRESHOLD {
            self.role_rate.checked_sub(1)
        } else {
            None
        };
        let Some(requested) = requested else {
            return Ok(());
        };
        self.role_rate_feedback = 0;
        self.transport.send_application(
            STOCK_REQUEST_ROLE_RATE,
            StockRoleRate {
                hz: requested as f32,
            }
            .encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        Ok(())
    }

    pub fn send_pose_chain(
        &mut self,
        request: crate::stock_protocol::StockPoseChainRequest,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.actor_id.ok_or(StockGameError::RoleNotEntered)?;
        self.transport.send_application(
            crate::stock_protocol::STOCK_REQUEST_POSE_CHAIN,
            request.encode(),
            0,
            now_ms,
        )?;
        Ok(())
    }

    pub fn send_role_state(&mut self, state: RoleState, now_ms: u64) -> Result<(), StockGameError> {
        let actor_id = self.actor_id.ok_or(StockGameError::RoleNotEntered)?;
        self.states.insert(actor_id, state);
        self.transport.send_application(
            STOCK_REQUEST_ROLE_STATE,
            encode_role_state_request(state),
            STOCK_REQUEST_ROLE_STATE_FLAGS,
            now_ms,
        )?;
        Ok(())
    }

    pub fn send_self_effect(&mut self, id: u32, now_ms: u64) -> Result<(), StockGameError> {
        self.actor_id.ok_or(StockGameError::RoleNotEntered)?;
        self.transport
            .send_application(42, id.to_le_bytes().to_vec(), 1, now_ms)?;
        Ok(())
    }

    pub fn send_cure(&mut self, now_ms: u64) -> Result<Vec<u8>, StockGameError> {
        let actor_id = self.actor_id.ok_or(StockGameError::RoleNotEntered)?;
        if !self.vips.iter().flatten().any(|vip| *vip == actor_id) {
            return Ok(Vec::new());
        }
        let Some(camp) = self.camps.get(&actor_id).copied() else {
            return Ok(Vec::new());
        };
        let Some(healer_state) = self.states.get(&actor_id).copied() else {
            return Ok(Vec::new());
        };
        let range_sq = STOCK_CURE_RANGE * STOCK_CURE_RANGE;
        let mut target_actor_ids = self
            .dead_actors
            .iter()
            .filter_map(|target_actor_id| {
                let target_state = self.states.get(target_actor_id)?;
                let same_camp = self.camps.get(target_actor_id).copied() == Some(camp);
                let dx = healer_state.position.x - target_state.position.x;
                let dz = healer_state.position.z - target_state.position.z;
                (same_camp && dx * dx + dz * dz <= range_sq)
                    .then(|| u8::try_from(*target_actor_id).ok())
                    .flatten()
            })
            .collect::<Vec<_>>();
        target_actor_ids.sort_unstable();
        if target_actor_ids.is_empty() {
            return Ok(target_actor_ids);
        }
        let request = StockCureRequest {
            target_actor_ids: target_actor_ids.clone(),
        };
        self.transport.send_application(
            STOCK_REQUEST_CURE,
            request.encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        Ok(target_actor_ids)
    }

    pub fn send_hit(&mut self, hit: StockHitRequest, now_ms: u64) -> Result<(), StockGameError> {
        if self.actor_id.is_none() {
            return Err(StockGameError::RoleNotEntered);
        }
        self.transport.send_application(
            STOCK_REQUEST_HIT,
            hit.encode(),
            STOCK_REQUEST_HIT_FLAGS,
            now_ms,
        )?;
        Ok(())
    }

    pub fn send_anger_spend(
        &mut self,
        key: (u32, u16),
        spend: StockAngerSpend,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        if self.actor_id.is_none() {
            return Err(StockGameError::RoleNotEntered);
        }
        if self.sent_anger_keys.contains(&key) {
            return Ok(());
        }
        let sequence = self.transport.send_application_tracked(
            spend.opcode(),
            spend.encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        self.sent_anger_keys.push_back(key);
        if self.sent_anger_keys.len() > 64 {
            self.sent_anger_keys.pop_front();
        }
        self.pending_anger_spends.push_back((sequence, spend));
        Ok(())
    }

    pub fn send_scene_item_spawn(
        &mut self,
        request: StockSceneItemSpawnRequest,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        if self.actor_id.is_none() {
            return Err(StockGameError::RoleNotEntered);
        }
        if !valid_stock_scene_item_key(request.category, request.item_id) {
            return Err(StockGameError::InvalidSceneItem {
                category: request.category,
                item_id: request.item_id,
            });
        }
        self.transport.send_application(
            STOCK_REQUEST_SPAWN_SCENE_ITEM,
            request.encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        Ok(())
    }

    pub fn send_weapon_drop(
        &mut self,
        command: StockWeaponDropCommand,
        now_ms: u64,
    ) -> Result<Option<StockGameEvent>, StockGameError> {
        let actor_id = self.actor_id.ok_or(StockGameError::RoleNotEntered)?;
        if stock_weapon_from_item_id(command.item_id).is_none() {
            return Err(StockGameError::InvalidSceneItem {
                category: 1,
                item_id: command.item_id,
            });
        }
        let request = StockWeaponObjectRequest::from_role_vectors(
            command.item_id,
            u32::from(!command.copy_only),
            command.position,
            command.forward,
        );
        self.transport.send_application(
            STOCK_REQUEST_WEAPON_OBJECT,
            request.encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        if command.copy_only {
            self.transport.send_application(
                STOCK_REQUEST_DROP_ITEM,
                [],
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        } else if let Some(remaining) = command.remaining_item_id {
            self.weapons.insert(actor_id, remaining);
        } else {
            self.weapons.remove(&actor_id);
        }

        let throw = StockWeaponThrowEvent {
            throw_id: self.allocate_weapon_throw_id(),
            actor_id,
            item_id: command.item_id,
            position: command.position,
            forward: command.forward,
        };
        self.observed_weapon_throws.push_back(throw);
        self.pending_weapon_drops.push(PendingLocalWeaponDrop {
            throw,
            expires_at_ms: now_ms.saturating_add(STOCK_WEAPON_THROW_TIMEOUT_MS),
            copy_only: command.copy_only,
        });
        Ok((!command.copy_only).then_some(StockGameEvent::WeaponThrown(throw)))
    }

    pub fn send_weapon_switch(&mut self, item_id: u32, now_ms: u64) -> Result<(), StockGameError> {
        let actor_id = self.actor_id.ok_or(StockGameError::RoleNotEntered)?;
        if stock_weapon_from_item_id(item_id).is_none() {
            return Err(StockGameError::InvalidSceneItem {
                category: 1,
                item_id,
            });
        }

        self.transport.send_application(
            STOCK_REQUEST_EQUIP_WEAPON,
            StockEquipWeaponRequest {
                weapon_item_id: item_id,
            }
            .encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        self.weapons.insert(actor_id, item_id);
        Ok(())
    }

    fn allocate_weapon_throw_id(&mut self) -> u32 {
        let id = self.next_weapon_throw_id.max(1);
        self.next_weapon_throw_id = id.wrapping_add(1).max(1);
        id
    }

    fn weapon_throw_from_event55(&mut self, actor_id: u32) -> Option<StockWeaponThrowEvent> {
        if Some(actor_id) == self.actor_id {
            return self
                .pending_weapon_drops
                .iter()
                .rev()
                .find(|pending| pending.copy_only)
                .map(|pending| pending.throw);
        }
        let item_id = self.weapons.get(&actor_id).copied()?;
        let state = self.states.get(&actor_id).copied()?;
        let throw = StockWeaponThrowEvent {
            throw_id: self.allocate_weapon_throw_id(),
            actor_id,
            item_id,
            position: state.position,
            forward: state.forward,
        };
        self.observed_weapon_throws.push_back(throw);
        Some(throw)
    }

    pub fn confirm_weapon_drop_collision(
        &mut self,
        throw_id: u32,
        position: RoleVec3,
        now_ms: u64,
    ) -> Result<bool, StockGameError> {
        if ![position.x, position.y, position.z]
            .iter()
            .all(|v| v.is_finite())
        {
            return Ok(false);
        }
        let Some(index) = self
            .pending_weapon_drops
            .iter()
            .position(|drop| drop.throw.throw_id == throw_id && now_ms <= drop.expires_at_ms)
        else {
            return Ok(false);
        };
        let drop = self.pending_weapon_drops[index];
        self.transport.send_application(
            STOCK_REQUEST_SPAWN_SCENE_ITEM,
            StockSceneItemSpawnRequest::from_role_position(drop.throw.item_id, 1, position)
                .encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        self.pending_weapon_drops.remove(index);
        Ok(true)
    }

    pub fn send_scene_ops(
        &mut self,
        event: &StockSceneOpsEvent,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        if self.actor_id.is_none() || event.operations.is_empty() {
            return Ok(());
        }
        for operations in event.operations.chunks(STOCK_SCENE_OPS_PER_PACKET) {
            let packet = StockSceneOpsEvent {
                timestamp_ms: event.timestamp_ms,
                operations: operations.to_vec(),
            };
            self.transport.send_application(
                STOCK_EVENT_SCENE_OPS,
                packet.encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        Ok(())
    }

    fn send_enter_role(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        if self.enter_sent {
            return Ok(());
        }
        self.transport.send_application(
            STOCK_REQUEST_ENTER_ROLE,
            self.enter_request.encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        self.enter_sent = true;
        Ok(())
    }

    fn send_initial_weapon(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        if self.weapon_sent {
            return Ok(());
        }
        self.transport.send_application(
            STOCK_REQUEST_EQUIP_WEAPON,
            StockEquipWeaponRequest {
                weapon_item_id: self.weapon_item_id,
            }
            .encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        self.weapon_sent = true;
        Ok(())
    }

    fn send_ready(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        if self.ready_sent {
            return Ok(());
        }
        self.transport.send_application(
            STOCK_REQUEST_READY,
            [],
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        self.ready_sent = true;
        Ok(())
    }
}

fn require_empty_stock_payload(payload: &[u8]) -> Result<(), StockProtocolError> {
    if payload.is_empty() {
        Ok(())
    } else {
        Err(StockProtocolError::InvalidLength {
            expected: 0,
            actual: payload.len(),
        })
    }
}

fn is_stock_escort_key(category: u32, item_id: u32) -> bool {
    category == STOCK_ESCORT_CATEGORY && item_id > 0
}

fn is_stock_escort_record(record: &StockSceneItemRecord) -> bool {
    is_stock_escort_key(record.category, record.item_id)
}

fn valid_authored_scene_item(item: &StockAuthoredSceneItem) -> bool {
    valid_stock_scene_item_key(item.category, item.item_id)
        && [item.position.x, item.position.y, item.position.z]
            .into_iter()
            .all(f32::is_finite)
        && item
            .half_extents
            .into_iter()
            .all(|extent| extent.is_finite() && extent >= 0.0)
}

fn valid_stock_scene_item_key(category: u32, item_id: u32) -> bool {
    match category {
        1 => stock_weapon_from_item_id(item_id).is_some(),
        2 => stock_item_effect(item_id).is_some(),
        _ => false,
    }
}

fn role_can_take_scene_item(role: &StockHostedRole, item: &StockHostedSceneItem) -> bool {
    if item.seed.category == 2 {
        return true;
    }
    let Some((weapon, _)) = stock_weapon_from_item_id(item.seed.item_id) else {
        return false;
    };
    role.weapon_slots.iter().any(Option::is_none)
        && !role.weapon_slots.iter().flatten().any(|held| {
            stock_weapon_from_item_id(*held).is_some_and(|(held_weapon, _)| held_weapon == weapon)
        })
}

fn role_touches_scene_item(position: RoleVec3, item: &StockHostedSceneItem) -> bool {
    let center = item.seed.position;
    if !item.authored {
        let delta = [
            position.x - center.x,
            position.y - center.y,
            position.z - center.z,
        ];
        return delta.iter().map(|axis| axis * axis).sum::<f32>()
            < STOCK_SCENE_PICKUP_RADIUS * STOCK_SCENE_PICKUP_RADIUS;
    }
    let [half_x, half_y, half_z] = item.seed.half_extents;
    (position.x - center.x).abs() <= half_x + STOCK_SCENE_PICKUP_RADIUS
        && position.y + STOCK_SCENE_PICKUP_HEIGHT >= center.y - half_y
        && position.y <= center.y + half_y + STOCK_SCENE_PICKUP_RADIUS
        && (position.z - center.z).abs() <= half_z + STOCK_SCENE_PICKUP_RADIUS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StockItemEffect {
    health: i32,
    anger: i32,
    attack: i32,
    defense: i32,
    max_health: i32,
    duration_ms: u64,
    periodic_health: i32,
    periodic_anger: i32,
    tick_ms: u64,
    refine_variant: Option<u8>,
}

impl StockItemEffect {
    const fn instant(health: i32, anger: i32) -> Self {
        Self {
            health,
            anger,
            attack: 0,
            defense: 0,
            max_health: 0,
            duration_ms: 0,
            periodic_health: 0,
            periodic_anger: 0,
            tick_ms: 0,
            refine_variant: None,
        }
    }
}

fn stock_item_effect(item_id: u32) -> Option<StockItemEffect> {
    let mut effect = StockItemEffect::instant(0, 0);
    match item_id {
        1 => effect.health = 300,
        2 => {
            effect.max_health = 500;
            effect.duration_ms = 600_000;
        }
        3 => {
            effect.defense = 20;
            effect.duration_ms = 90_000;
        }
        4 => {
            effect.attack = 25;
            effect.duration_ms = 60_000;
        }

        5..=7 | 31 => return Some(effect),
        8 => effect.anger = 100,
        9 => effect.refine_variant = Some(0),
        16 => effect.health = 500,
        17 => effect.health = 100,
        18 => {
            effect.duration_ms = 30_000;
            effect.periodic_health = -10;
            effect.tick_ms = 5_000;
        }
        23..=26 => effect.refine_variant = Some((item_id - 22) as u8),
        27 => {
            effect.max_health = 6_999;
            effect.duration_ms = 60_000;
        }
        28 => effect.anger = 25,
        29 => {
            effect.defense = 100;
            effect.duration_ms = 9_999_900;
        }
        30 => {
            effect.attack = 125;
            effect.duration_ms = 9_999_900;
        }
        32 | 33 => {
            effect.duration_ms = 9_999_900;
            effect.tick_ms = 200;
            if item_id == 32 {
                effect.periodic_health = 50;
            } else {
                effect.periodic_anger = 1;
            }
        }
        39 => {
            effect.duration_ms = 9_999_900;
            effect.periodic_anger = 1;
            effect.tick_ms = 1_000;
        }
        _ => return None,
    }
    Some(effect)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StockActiveItemEffect {
    effect: StockItemEffect,
    expires_at_ms: u64,
    next_tick_ms: Option<u64>,
}

fn stock_weapon_item_with_variant(item_id: u32, variant: u8) -> Option<u32> {
    let (weapon, _) = stock_weapon_from_item_id(item_id)?;
    Some(match weapon {
        1..=9 => u32::from(weapon) + u32::from(variant) * 9,
        10 => 47 + u32::from(variant),
        11 => 51 + u32::from(variant),
        12 => 55 + u32::from(variant),
        _ => return None,
    })
}

fn apply_stock_item_effect(role: &mut StockHostedRole, item_id: u32, now_ms: u64) {
    let Some(effect) = stock_item_effect(item_id) else {
        return;
    };
    if effect.health != 0 {
        role.health = (role.health + effect.health as f32).clamp(0.0, role.max_health);
    }
    if effect.anger != 0 {
        role.anger = (role.anger + effect.anger as f32).clamp(0.0, 100.0);
    }
    if let Some(variant) = effect.refine_variant {
        if let Some(current) = role.weapon_item_id {
            if let Some(refined) = stock_weapon_item_with_variant(current, variant) {
                for slot in &mut role.weapon_slots {
                    if *slot == Some(current) {
                        *slot = Some(refined);
                    }
                }
                role.weapon_item_id = Some(refined);
            }
        }
    }
    if effect.duration_ms == 0 {
        return;
    }
    if let Some(active) = role.active_item_effects.get_mut(&item_id) {
        active.expires_at_ms = now_ms.saturating_add(effect.duration_ms);
        return;
    }
    if effect.max_health != 0 {
        role.max_health = (role.max_health + effect.max_health as f32).max(1.0);
        role.health = role.health.min(role.max_health);
    }
    role.active_item_effects.insert(
        item_id,
        StockActiveItemEffect {
            effect,
            expires_at_ms: now_ms.saturating_add(effect.duration_ms),
            next_tick_ms: (effect.tick_ms > 0).then(|| now_ms.saturating_add(effect.tick_ms)),
        },
    );
}

#[derive(Debug, Clone)]
struct StockHostedRole {
    defensive_effects: meteor_combat::rebalance_effects::DefensiveEffects,
    pending_self_health: i32,
    hit_status_effects: meteor_combat::status_effects::HitStatusEffects,
    actor_id: u32,
    hero: u32,
    camp: u32,
    weapon_item_id: Option<u32>,
    weapon_slots: [Option<u32>; 2],
    active_item_effects: HashMap<u32, StockActiveItemEffect>,
    state: Option<RoleState>,
    dirty: bool,
    health: f32,
    max_health: f32,
    anger: f32,
    kills: u32,
    deaths: u32,
    last_hit_ms: Option<u64>,
    respawn_at_ms: Option<u64>,
    spawned_at_ms: u64,
    next_cure_at_ms: u64,
}

impl StockHostedRole {
    fn equipped_item_id(&self) -> u8 {
        self.weapon_item_id
            .filter(|&item_id| stock_weapon_from_item_id(item_id).is_some())
            .and_then(|item_id| u8::try_from(item_id).ok())
            .unwrap_or(6)
    }

    fn entered_event(&self) -> StockRoleEnteredEvent {
        StockRoleEnteredEvent {
            actor_id: self.actor_id,
            hero: self.hero,

            hero_checksum: self.weapon_item_id.unwrap_or(0),
            camp: self.camp,
        }
    }

    fn vitals_event(&self) -> StockVitalsEvent {
        StockVitalsEvent {
            health: self.health.clamp(0.0, f32::from(u16::MAX)).round() as u16,
            max_health: self.max_health.clamp(0.0, f32::from(u16::MAX)).round() as u16,
            anger: self.anger.clamp(0.0, f32::from(u16::MAX)).round() as u16,
        }
    }

    fn item_attack_bonus(&self) -> i32 {
        self.active_item_effects
            .values()
            .map(|active| active.effect.attack)
            .sum::<i32>()
            + self.hit_status_effects.attack_bonus()
    }

    fn item_defense_bonus(&self) -> i32 {
        self.active_item_effects
            .values()
            .map(|active| active.effect.defense)
            .sum()
    }
}

impl CombatStats for StockHostedRole {
    fn try_spend_anger(&mut self, cost: f32) -> bool {
        if cost.is_finite() && cost >= 0.0 && self.anger >= cost {
            self.anger -= cost;
            true
        } else {
            false
        }
    }
    fn hp(&self) -> f32 {
        self.health
    }

    fn alive(&self) -> bool {
        self.health > 0.0
    }

    fn invincible(&self) -> bool {
        false
    }

    fn anger(&self) -> f32 {
        self.anger
    }

    fn anger_max(&self) -> f32 {
        100.0
    }

    fn def(&self) -> f32 {
        self.item_defense_bonus() as f32
    }

    fn attack_bonus(&self) -> f32 {
        self.item_attack_bonus() as f32
    }

    fn take_damage(&mut self, amount: i32) -> i32 {
        let before = self.health;
        self.health = (self.health - amount.max(0) as f32).max(0.0);
        (before - self.health).round() as i32
    }

    fn gain_anger_from_damage(&mut self, damage: i32) -> i32 {
        let gain = (14 * damage.max(0)) / 100;
        let before = self.anger;
        self.anger = (self.anger + gain as f32).min(100.0);
        (self.anger - before).round() as i32
    }

    fn gain_anger_on_guard(&mut self, raw_atk: i32) -> i32 {
        let gain = meteor_combat::hit_resolve::guard_anger_gain(raw_atk);
        let before = self.anger;
        self.anger = (self.anger + gain as f32).min(100.0);
        (self.anger - before).round() as i32
    }

    fn reset_spawn_clock(&mut self) {}
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StockEscortState {
    record_id: u32,
    position: RoleVec3,
    carrier_actor_id: Option<u32>,
    dropped_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct StockHostedSceneItem {
    seed: StockAuthoredSceneItem,
    record_id: u32,
    active: bool,

    script_active: bool,
    interactive: bool,
    respawn_at_ms: Option<u64>,

    expires_at_ms: Option<u64>,
    authored: bool,
}

impl StockHostedSceneItem {
    fn record(self, frame: Option<StockSceneFrame>) -> StockSceneItemRecord {
        let position = self.seed.position;
        StockSceneItemRecord::from_role_position(
            self.record_id,
            self.seed.item_id,
            self.seed.category,
            frame.map_or(position, |frame| frame.position_to_stock(position)),
        )
    }
}

impl StockEscortState {
    fn home(record_id: u32, position: RoleVec3) -> Self {
        Self {
            record_id,
            position,
            carrier_actor_id: None,
            dropped_at_ms: None,
        }
    }

    fn scene_record(self, item_id: u32) -> StockSceneItemRecord {
        StockSceneItemRecord::from_role_position(
            self.record_id,
            item_id,
            STOCK_ESCORT_CATEGORY,
            self.position,
        )
    }
}

pub struct StockGameServer {
    battle_rules: meteor_combat::battle_rules::BattleRules,
    rules_waiting: HashMap<SocketAddr, u64>,
    rules_transfers: HashMap<SocketAddr, FileSender<std::io::Cursor<Vec<u8>>>>,
    rules_verified: HashSet<SocketAddr>,
    pending_rule_entry: HashMap<SocketAddr, Vec<u8>>,
    pending_rule_ready: HashSet<SocketAddr>,
    scene_frame: Option<StockSceneFrame>,
    hazards: meteor_scene::hazards::StageHazards,
    hazard_tick_ms: Option<u64>,

    transport: StockServerTransport,
    heroes: StockHeroCatalog,
    combat_resources: CombatResourceTable,
    rule: i32,
    stage_id: Option<String>,
    spawn_pad_counts: [usize; 3],
    round_limit: u32,
    round_time_minutes: u32,
    base_max_health: f32,
    round_count: u32,
    round_started_ms: Option<u64>,
    team_wins: [u32; 2],
    vips: [Option<u32>; 2],
    escort: Option<StockEscortState>,
    escort_definition: Option<std::sync::Arc<crate::escort_definition::EscortDefinition>>,
    scene_items: BTreeMap<u32, StockHostedSceneItem>,

    scene_state: Vec<StockSceneOperation>,
    runtime: runtime_sync::HostState,
    last_xstar: Option<(SocketAddr, u32, u64)>,
    item_rules: StockRoomItemRules,
    scene_item_option_bits: BTreeMap<(u32, u32), Option<u32>>,
    next_scene_record_id: u32,
    round_ended: bool,
    round_restart_at_ms: Option<u64>,
    roles: HashMap<SocketAddr, StockHostedRole>,
    ready_peers: HashSet<SocketAddr>,
    remote_vitals_peers: HashSet<SocketAddr>,
    player_name_peers: HashSet<SocketAddr>,
    unicode_player_names: HashMap<SocketAddr, String>,
    remote_vitals_revision: u32,
    last_role_downlink_ms: Option<u64>,
    last_vitals_downlink_ms: Option<u64>,
    role_rate: u32,
    requested_role_rates: HashMap<SocketAddr, u32>,
    role_rate_sample_ms: Option<u64>,
    role_rate_feedback: i32,
}

impl StockGameServer {
    fn broadcast_player_names(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        for (owner, role) in &self.roles {
            let Some(name) = self
                .unicode_player_names
                .get(owner)
                .map(String::as_str)
                .or_else(|| self.transport.peer_player_name(*owner))
            else {
                continue;
            };
            let mut payload = PLAYER_NAMES_V1.to_vec();
            payload.extend_from_slice(&role.actor_id.to_le_bytes());
            payload.extend_from_slice(name.as_bytes());
            if decode_player_name(&payload).is_none() {
                continue;
            }
            for peer in &self.player_name_peers {
                self.transport.send_application(
                    *peer,
                    EVENT_PLAYER_NAME,
                    payload.clone(),
                    STOCK_DISCRETE_RELIABLE_FLAGS,
                    now_ms,
                )?;
            }
        }
        Ok(())
    }
    pub fn new(transport: StockServerTransport, heroes: StockHeroCatalog) -> Self {
        Self::with_combat_resources(transport, heroes, CombatResourceTable::default())
    }

    pub fn with_combat_resources(
        transport: StockServerTransport,
        heroes: StockHeroCatalog,
        combat_resources: CombatResourceTable,
    ) -> Self {
        Self::with_rule_and_combat_resources(transport, heroes, 1, combat_resources)
    }

    pub fn with_rule_and_combat_resources(
        transport: StockServerTransport,
        heroes: StockHeroCatalog,
        rule: i32,
        combat_resources: CombatResourceTable,
    ) -> Self {
        Self {
            battle_rules: Default::default(),
            rules_waiting: HashMap::new(),
            rules_transfers: HashMap::new(),
            rules_verified: HashSet::new(),
            pending_rule_entry: HashMap::new(),
            pending_rule_ready: HashSet::new(),
            scene_frame: None,
            hazards: Default::default(),
            hazard_tick_ms: None,
            transport,
            heroes,
            combat_resources,
            rule,
            stage_id: None,
            spawn_pad_counts: [0; 3],
            round_limit: STOCK_DEFAULT_GAME_NUM,
            round_time_minutes: STOCK_DEFAULT_GAME_TIME_MINUTES,
            base_max_health: STOCK_DEFAULT_MAX_HEALTH,
            round_count: 0,
            round_started_ms: None,
            team_wins: [0, 0],
            vips: [None, None],
            escort: (rule == 2).then(|| StockEscortState::home(1, SN03.flag_home)),
            escort_definition: None,
            scene_items: BTreeMap::new(),
            scene_state: Vec::new(),
            runtime: Default::default(),
            last_xstar: None,
            item_rules: StockRoomItemRules::default(),
            scene_item_option_bits: BTreeMap::new(),
            next_scene_record_id: 2,
            round_ended: false,
            round_restart_at_ms: None,
            roles: HashMap::new(),
            ready_peers: HashSet::new(),
            remote_vitals_peers: HashSet::new(),
            player_name_peers: HashSet::new(),
            unicode_player_names: HashMap::new(),
            remote_vitals_revision: 0,
            last_role_downlink_ms: None,
            last_vitals_downlink_ms: None,
            role_rate: ROLE_DOWNLINK_HZ,
            requested_role_rates: HashMap::new(),
            role_rate_sample_ms: None,
            role_rate_feedback: 0,
        }
    }

    pub fn transport(&self) -> &StockServerTransport {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut StockServerTransport {
        &mut self.transport
    }

    pub fn rule(&self) -> i32 {
        self.rule
    }

    pub fn ready_peer_count(&self) -> usize {
        self.ready_peers.len()
    }

    pub fn set_round_limit(&mut self, round_limit: u32) {
        self.round_limit = round_limit.max(1);
    }

    pub fn set_round_time_minutes(&mut self, minutes: u32) {
        self.round_time_minutes = minutes.max(1);
    }

    pub fn set_life_adjust_percent(&mut self, percent: u32) {
        self.base_max_health = stock_max_health_for_life_adjust(percent);
    }

    pub fn set_escort_definition(
        &mut self,
        definition: std::sync::Arc<crate::escort_definition::EscortDefinition>,
    ) {
        assert!(
            self.roles.is_empty(),
            "escort assets cannot change during a round"
        );
        if let Some(state) = self.escort.as_mut() {
            state.position = definition.stage.flag_home;
        }
        self.escort_definition = Some(definition);
    }

    fn escort_stage_data(&self) -> &crate::escort::EscortStage {
        self.escort_definition.as_ref().map_or(&SN03, |d| &d.stage)
    }

    fn escort_item_id(&self) -> u32 {
        self.escort_definition
            .as_ref()
            .map_or(STOCK_ESCORT_ITEM_ID, |d| d.item_id)
    }

    pub fn set_stage_id(&mut self, stage_id: impl Into<String>) {
        let stage_id = stage_id.into().to_ascii_lowercase();
        if self.stage_id.as_deref() != Some(&stage_id) {
            self.scene_state.clear();
            self.runtime.reset_pending = true;
        }
        self.stage_id = Some(stage_id);
        self.transport.set_discovery_stage(self.stage_id.clone());
    }

    pub fn set_spawn_pad_counts(&mut self, counts: [usize; 3]) {
        self.spawn_pad_counts = counts;
    }

    pub fn set_scene_frame(&mut self, frame: StockSceneFrame) {
        self.scene_frame = Some(frame);
    }

    pub fn scene_frame(&self) -> Option<StockSceneFrame> {
        self.scene_frame
    }

    pub fn transfer_scene_actor(
        &mut self,
        actor_id: u32,
        event: StockSceneTransferEvent,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        let event = StockSceneTransferEvent::decode(&event.encode())?;
        let Some(peer) = self.roles.iter().find_map(|(peer, role)| {
            (role.actor_id == actor_id && role.health > 0.0 && role.respawn_at_ms.is_none())
                .then_some(*peer)
        }) else {
            return Ok(());
        };
        if self.round_ended {
            return Ok(());
        }
        self.transport.send_application(
            peer,
            STOCK_EVENT_SCENE_TRANSFER,
            event.encode().to_vec(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        let target = event.role_state();
        if let Some(role) = self.roles.get_mut(&peer) {
            if let Some(state) = &mut role.state {
                state.position = target.position;
                state.forward = target.forward;
                state.velocity = RoleVec3::ZERO;
                role.dirty = true;
            }
        }
        Ok(())
    }

    pub fn set_battle_rules(&mut self, rules: meteor_combat::battle_rules::BattleRules) {
        assert!(
            self.roles.is_empty() && self.rules_waiting.is_empty(),
            "room rules are immutable after admission"
        );
        self.battle_rules = rules;
    }

    pub fn set_item_rules(&mut self, rules: StockRoomItemRules) {
        self.item_rules = rules;
    }

    pub fn set_scene_item_registry(&mut self, resource: &MeteorRes) {
        self.scene_item_option_bits = resource
            .records
            .iter()
            .filter(|record| matches!(record.category, 1 | 2))
            .map(|record| {
                let bit = record
                    .prop_seed_value(11)
                    .and_then(crate::stock_protocol::stock_scene_item_option_bit);
                ((record.category, record.id), bit)
            })
            .collect();
    }

    pub fn team_wins(&self) -> [u32; 2] {
        self.team_wins
    }

    pub fn vips(&self) -> [Option<u32>; 2] {
        self.vips
    }

    pub fn escort_carrier(&self) -> Option<u32> {
        self.escort.and_then(|escort| escort.carrier_actor_id)
    }

    pub fn role_rate(&self) -> u32 {
        self.role_rate
    }

    pub fn replace_authored_scene_items(
        &mut self,
        items: Vec<StockAuthoredSceneItem>,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.scene_items.retain(|_, item| !item.authored);
        let item_rules = self.item_rules;
        let flags = item_rules.scene_option_bits();
        for seed in items.into_iter().filter(valid_authored_scene_item) {
            let allowed = match self
                .scene_item_option_bits
                .get(&(seed.category, seed.item_id))
            {
                Some(bit) => bit.is_none_or(|bit| flags & bit != 0),
                None => item_rules.allows(seed.category, seed.item_id),
            };
            if !allowed {
                continue;
            }
            let record_id = self.allocate_scene_record_id();
            self.scene_items.insert(
                record_id,
                StockHostedSceneItem {
                    seed,
                    record_id,
                    active: seed.active,
                    script_active: seed.active,
                    interactive: seed.interactive,
                    respawn_at_ms: None,
                    expires_at_ms: None,
                    authored: true,
                },
            );
        }
        self.broadcast_scene_snapshot(now_ms)
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<Vec<StockGameEvent>, StockGameError> {
        self.transport
            .set_discovery_rebalance_enabled(self.battle_rules.enabled());
        self.transport.set_discovery_rule(self.rule as u8);
        let mut discovery_camps = [0usize; 2];
        for role in self.roles.values() {
            if let Some(count) = role
                .camp
                .checked_sub(1)
                .and_then(|camp| discovery_camps.get_mut(camp as usize))
            {
                *count += 1;
            }
        }
        self.transport
            .set_discovery_camp_counts(discovery_camps[0], discovery_camps[1]);
        let session_events = self.transport.tick(now_ms)?;
        let mut events = Vec::new();
        for event in session_events {
            match event {
                StockSessionEvent::PeerConnected {
                    peer,
                    transport_id,
                    identity: _,
                } => {
                    if self.battle_rules != meteor_combat::battle_rules::BattleRules::ORIGINAL {
                        self.rules_waiting.insert(peer, now_ms);
                    }
                    let elapsed = self
                        .round_started_ms
                        .map_or(0, |start| now_ms.saturating_sub(start) / 1000);
                    let remaining = (u64::from(self.round_time_minutes) * 60)
                        .saturating_sub(elapsed)
                        .min(u64::from(u32::MAX)) as u32;

                    let mut state = remaining.to_le_bytes().to_vec();
                    state.extend_from_slice(&u32::from(self.round_ended).to_le_bytes());
                    self.transport.send_application(
                        peer,
                        26,
                        state,
                        STOCK_DISCRETE_RELIABLE_FLAGS,
                        now_ms,
                    )?;
                    self.send_initial_stage_config(peer, now_ms)?;
                    self.send_initial_members(peer, transport_id, now_ms)?;
                    events.push(StockGameEvent::TransportConnected { peer, transport_id });
                }
                StockSessionEvent::Application { peer, packet } => {
                    if self.transport.peer_transport_id(peer).is_none() {
                        continue;
                    }
                    if let Err(error) = self.handle_application(
                        peer,
                        packet.opcode,
                        &packet.payload,
                        now_ms,
                        &mut events,
                    ) {
                        match error {
                            StockGameError::Transport(StockTransportError::Io(_))
                            | StockGameError::Io(_) => return Err(error),
                            _ => log::debug!(target: "meteor_network::stock_session",
                                "[stock-lan] request discarded: peer={peer}, opcode={}, {error}",
                                packet.opcode),
                        }
                    }
                }
                StockSessionEvent::Disconnected { peer, reason } => {
                    self.remove_disconnected_peer(peer, reason, now_ms, &mut events)?;
                }
                StockSessionEvent::Connected { .. } | StockSessionEvent::DiscoveryReply { .. } => {}
            }
        }
        let expired: Vec<_> = self
            .rules_waiting
            .iter()
            .filter(|(_, at)| now_ms.saturating_sub(**at) >= 5000)
            .map(|(peer, _)| *peer)
            .collect();
        for peer in expired {
            self.transport.disconnect_peer(peer, 0x80)?;
            self.remove_disconnected_peer(peer, 0x80, now_ms, &mut events)?;
        }
        if self
            .round_restart_at_ms
            .is_some_and(|deadline| now_ms >= deadline)
        {
            self.round_restart_at_ms = None;
            self.reset_roles_for_round(now_ms);
            self.runtime_reset(now_ms)?;
            self.assign_round_spawn_pads(now_ms)?;
            self.round_ended = false;
            self.broadcast_scene_snapshot(now_ms)?;
            events.push(StockGameEvent::RoundRestart);
        }
        if !self.round_ended {
            self.assign_vips_if_ready(now_ms, &mut events)?;
            self.update_scene_items(now_ms, &mut events)?;
            self.update_runtime(now_ms)?;
            self.update_item_effects(now_ms, &mut events)?;
            self.update_environment_hazards(now_ms, &mut events)?;
            self.update_escort(now_ms, &mut events)?;
            self.finish_round_on_timeout(now_ms, &mut events)?;
            self.respawn_due_roles(now_ms, &mut events)?;
        }
        self.update_role_rate(now_ms, &mut events)?;
        self.broadcast_dirty_role_states(now_ms)?;
        self.broadcast_vitals(now_ms)?;
        Ok(events)
    }

    fn send_initial_members(
        &mut self,
        peer: SocketAddr,
        actor_id: u32,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        use crate::stock_protocol::{
            encode_stock_member_joined, encode_stock_roster, StockRosterEntry,
        };
        let mut members = Vec::new();
        for owner in self
            .transport
            .peer_addresses()
            .into_iter()
            .filter(|owner| *owner != peer)
        {
            let Some(id) = self.transport.peer_transport_id(owner) else {
                continue;
            };
            let role = self.roles.get(&owner);
            members.push(StockRosterEntry {
                actor_id: id,
                hero: role.map_or(0, |r| r.hero),
                entered: role.is_some(),
                camp: role.map_or(0, |r| r.camp),
                weapon_item_id: role.and_then(|r| r.weapon_item_id).unwrap_or(0),
                name: self
                    .transport
                    .peer_player_name(owner)
                    .unwrap_or("")
                    .to_owned(),
            });
        }
        members.sort_by_key(|member| member.actor_id);
        self.transport.send_application(
            peer,
            STOCK_EVENT_ROSTER,
            encode_stock_roster(&members)?,
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        let mut score = self.team_wins[0].to_le_bytes().to_vec();
        score.extend_from_slice(&self.team_wins[1].to_le_bytes());
        self.transport.send_application(
            peer,
            STOCK_EVENT_TEAM_SCORE,
            score,
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        self.transport.send_application(
            peer,
            STOCK_EVENT_ROLE_RATE,
            StockRoleRate {
                hz: self.role_rate as f32,
            }
            .encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        let joined = encode_stock_member_joined(
            actor_id,
            self.transport.peer_player_name(peer).unwrap_or(""),
        )?;

        for recipient in self.transport.peer_addresses() {
            self.transport.send_application(
                recipient,
                0,
                joined.clone(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        log::debug!(target: "meteor_network::stock_session",
            "[stock-lan] members initialized: peer={peer}, actor={actor_id}, existing={}", members.len());
        Ok(())
    }

    fn send_initial_stage_config(
        &mut self,
        peer: SocketAddr,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        let Some(stage_id) = self.stage_id.as_ref() else {
            return Ok(());
        };

        let config = StockStageConfigEvent {
            stage_id: stage_id.clone(),
            field_a: self.rule as u32,
            field_b: stage_id
                .strip_prefix("sn")
                .and_then(|id| id.parse().ok())
                .unwrap_or(0),
            field_c: self.item_rules.scene_option_bits(),
        };
        self.transport.send_application(
            peer,
            STOCK_EVENT_STAGE_CONFIG,
            config.encode()?,
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        log::debug!(target: "meteor_network::stock_session",
            "[stock-lan] initial stage sent: peer={peer}, stage={}, rule={}", config.stage_id, self.rule);
        Ok(())
    }

    fn remove_disconnected_peer(
        &mut self,
        peer: SocketAddr,
        reason: u32,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        self.rules_waiting.remove(&peer);
        self.rules_transfers.remove(&peer);
        self.rules_verified.remove(&peer);
        self.pending_rule_entry.remove(&peer);
        self.pending_rule_ready.remove(&peer);
        let disconnected_role = self.roles.remove(&peer);
        self.ready_peers.remove(&peer);
        self.remote_vitals_peers.remove(&peer);
        self.runtime.peers.remove(&peer);
        self.runtime.hit_serials.remove(&peer);
        self.player_name_peers.remove(&peer);
        self.unicode_player_names.remove(&peer);
        self.requested_role_rates.remove(&peer);
        events.push(StockGameEvent::PeerDisconnected { peer, reason });
        if let Some(role) = disconnected_role {
            self.runtime
                .projectiles
                .forget_owner(&format!("stock-{}", role.actor_id));
            if self.rule == 4 && self.vips.iter().flatten().any(|vip| *vip == role.actor_id) {
                self.complete_round(role.camp == 1, role.camp == 2, now_ms, events)?;
            } else if self.rule == 2
                && self
                    .escort
                    .is_some_and(|escort| escort.carrier_actor_id == Some(role.actor_id))
            {
                self.return_escort_after_disconnect(now_ms, events)?;
            }
            let left = StockRoleLeftEvent {
                actor_id: role.actor_id,
            };
            self.broadcast_discrete(STOCK_EVENT_ROLE_LEFT, &left.encode(), now_ms)?;
            events.push(StockGameEvent::RoleLeft(left));
        }
        Ok(())
    }

    fn handle_application(
        &mut self,
        peer: SocketAddr,
        opcode: u16,
        payload: &[u8],
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        if self.round_ended
            && matches!(
                opcode,
                STOCK_REQUEST_ROLE_STATE
                    | STOCK_REQUEST_HIT
                    | STOCK_REQUEST_EQUIP_WEAPON
                    | STOCK_REQUEST_WEAPON_OBJECT
                    | STOCK_REQUEST_DROP_ITEM
                    | STOCK_REQUEST_SPAWN_SCENE_ITEM
                    | STOCK_REQUEST_CURE
                    | crate::stock_protocol::STOCK_REQUEST_POSE_CHAIN
                    | crate::stock_protocol::STOCK_EVENT_SCENE_OPS
            )
        {
            return Ok(());
        }
        if self.battle_rules != meteor_combat::battle_rules::BattleRules::ORIGINAL
            && !self.rules_verified.contains(&peer)
        {
            if opcode == STOCK_REQUEST_ENTER_ROLE {
                StockEnterRoleRequest::decode(payload)?;
                self.pending_rule_entry.insert(peer, payload.to_vec());
                return Ok(());
            }
            if opcode == STOCK_REQUEST_READY {
                if payload.is_empty() {
                    self.pending_rule_ready.insert(peer);
                }
                return Ok(());
            }
        }
        match opcode {
            REQUEST_BATTLE_RULES => {
                if payload == BATTLE_RULES_V7
                    && self.battle_rules != meteor_combat::battle_rules::BattleRules::ORIGINAL
                {
                    let transfer = self.rules_transfers.entry(peer).or_insert_with(|| {
                        FileSender::new(
                            BATTLE_RULES_TRANSFER_ID,
                            BATTLE_RULES_ZLIB_KIND,
                            std::io::Cursor::new(compress_battle_rules(self.battle_rules.clone())),
                            MAX_RULES_COMPRESSED_BYTES,
                        )
                        .expect("validated room rules fit the transfer limit")
                    });
                    self.transport.send_application(
                        peer,
                        EVENT_BATTLE_RULES,
                        transfer.offer().encode(),
                        STOCK_DISCRETE_RELIABLE_FLAGS,
                        now_ms,
                    )?;
                }
            }
            ACK_BATTLE_RULES => {
                let Ok(reply) = FileFrame::decode(payload) else {
                    return Ok(());
                };
                let Some(transfer) = self.rules_transfers.get_mut(&peer) else {
                    return Ok(());
                };
                let Ok(frames) = transfer.accept(&reply) else {
                    return Ok(());
                };
                let complete = transfer.complete();
                for frame in frames {
                    self.transport.send_application(
                        peer,
                        EVENT_BATTLE_RULES,
                        frame.encode(),
                        STOCK_DISCRETE_RELIABLE_FLAGS,
                        now_ms,
                    )?;
                }
                if self.rules_waiting.contains_key(&peer) && complete {
                    self.rules_waiting.remove(&peer);
                    self.rules_transfers.remove(&peer);
                    self.rules_verified.insert(peer);
                    if let Some(entry) = self.pending_rule_entry.remove(&peer) {
                        self.handle_application(
                            peer,
                            STOCK_REQUEST_ENTER_ROLE,
                            &entry,
                            now_ms,
                            events,
                        )?;
                    }
                    if self.pending_rule_ready.remove(&peer) {
                        self.handle_application(peer, STOCK_REQUEST_READY, &[], now_ms, events)?;
                    }
                }
            }
            STOCK_REQUEST_STAGE_CHECKSUM => {
                let request = StockStageChecksumRequest::decode(payload)?;
                let stage = self.stage_id.as_ref().ok_or_else(|| {
                    StockGameError::MissingStageChecksum("room stage unset".into())
                })?;
                let expected = self
                    .heroes
                    .stage_checksums
                    .as_ref()
                    .and_then(|checksums| checksums.get(stage))
                    .ok_or_else(|| StockGameError::MissingStageChecksum(stage.clone()))?;
                if request.checksum != *expected {
                    let actor_id = self
                        .transport
                        .peer_transport_id(peer)
                        .ok_or(StockTransportError::UnknownPeer(peer))?;
                    let rejection = StockStageChecksumRejected { actor_id };
                    self.broadcast_with_flags(
                        STOCK_EVENT_STAGE_CHECKSUM_REJECTED,
                        &rejection.encode(),
                        0,
                        now_ms,
                    )?;
                    self.transport.disconnect_peer(peer, 0x80)?;
                    events.push(StockGameEvent::StageChecksumRejected(rejection));
                    self.remove_disconnected_peer(peer, 0x80, now_ms, events)?;
                }
            }
            crate::stock_latency::REQUEST => {
                if payload == crate::stock_latency::MAGIC {
                    let samples = self.transport.peer_latencies(now_ms);
                    self.transport.send_application(
                        peer,
                        crate::stock_latency::EVENT,
                        crate::stock_latency::encode(&samples),
                        STOCK_DISCRETE_RELIABLE_FLAGS,
                        now_ms,
                    )?;
                }
            }
            crate::runtime_sync::REQUEST => {
                self.handle_runtime_request(peer, payload, now_ms)?;
            }
            crate::stock_remote_vitals::REQUEST_REMOTE_VITALS => {
                if payload == crate::stock_remote_vitals::REMOTE_VITALS_V1 {
                    self.remote_vitals_peers.insert(peer);
                    self.last_vitals_downlink_ms = None;
                }
            }
            REQUEST_PLAYER_NAMES => {
                if let Some(raw) = payload.strip_prefix(PLAYER_NAMES_V2) {
                    if let Ok(name) = std::str::from_utf8(raw) {
                        if !name.trim().is_empty()
                            && name.len() <= 768
                            && !name.chars().any(char::is_control)
                        {
                            self.unicode_player_names.insert(peer, name.to_owned());
                            self.player_name_peers.insert(peer);
                            self.broadcast_player_names(now_ms)?;
                        }
                    }
                }
                if payload == PLAYER_NAMES_V1 {
                    self.player_name_peers.insert(peer);
                    self.broadcast_player_names(now_ms)?;
                }
            }
            STOCK_REQUEST_ANGER_FULL | STOCK_REQUEST_ANGER_COST | STOCK_REQUEST_QI_BURST => {
                let spend = StockAngerSpend::decode(opcode, payload)?;
                let role = self
                    .roles
                    .get_mut(&peer)
                    .ok_or(StockGameError::RoleNotEntered)?;
                let insufficient_full = spend == StockAngerSpend::Full && role.anger < 100.0;
                let actor_id = role.actor_id;

                role.anger = spend.remaining(role.anger);
                if insufficient_full {
                    self.broadcast_with_flags(
                        crate::stock_protocol::STOCK_EVENT_ANGER_INSUFFICIENT,
                        &actor_id.to_le_bytes(),
                        0,
                        now_ms,
                    )?;
                }
            }
            27 if payload.is_empty() => {
                let actor_id = self
                    .transport
                    .peer_transport_id(peer)
                    .ok_or(StockTransportError::UnknownPeer(peer))?;
                self.broadcast_discrete(52, &actor_id.to_le_bytes(), now_ms)?;
                log::debug!(target: "meteor_network::stock_session",
                    "[stock-lan] request 27 replied: peer={peer}, actor={actor_id}, event=52");
            }
            STOCK_REQUEST_ENTER_ROLE => {
                log::debug!(target: "meteor_network::stock_session", "[stock-lan] enter-role received: peer={peer}, bytes={}", payload.len());
                let request = StockEnterRoleRequest::decode(payload)?;
                let expected_checksum = self.heroes.checksum(request.hero);
                log::debug!(target: "meteor_network::stock_session",
                    "[stock-lan] enter-role decoded: peer={peer}, hero={}, camp={}, rule={}, checksum={:#010x}, expected_checksum={expected_checksum:?}",
                    request.hero, request.camp, self.rule, request.hero_checksum);
                if !self.heroes.accepts(request) {
                    log::warn!(target: "meteor_network::stock_session",
                        "[stock-lan] enter-role rejected: peer={peer}, hero={}, checksum={:#010x}, expected_checksum={expected_checksum:?}",
                        request.hero, request.hero_checksum);
                    self.transport.disconnect_peer(peer, 0x80)?;
                    return Ok(());
                }

                if request.camp != 0
                    && self
                        .roles
                        .values()
                        .filter(|role| role.camp == request.camp)
                        .count()
                        >= 8
                {
                    let suggested = if request.camp == 1 { 2u32 } else { 1u32 };
                    self.transport.send_application(
                        peer,
                        crate::stock_protocol::STOCK_EVENT_CAMP_SUGGESTION,
                        suggested.to_le_bytes(),
                        STOCK_DISCRETE_RELIABLE_FLAGS,
                        now_ms,
                    )?;
                }
                let actor_id = self
                    .transport
                    .peer_transport_id(peer)
                    .ok_or(StockTransportError::UnknownPeer(peer))?;
                let existing: Vec<StockHostedRole> = self.roles.values().cloned().collect();
                let role = StockHostedRole {
                    defensive_effects: Default::default(),
                    pending_self_health: 0,
                    actor_id,
                    hero: request.hero,
                    camp: request.camp,
                    weapon_item_id: None,
                    weapon_slots: [None, None],
                    active_item_effects: HashMap::new(),
                    hit_status_effects: Default::default(),
                    state: None,
                    dirty: false,
                    health: self.base_max_health,
                    max_health: self.base_max_health,
                    anger: 0.0,
                    kills: 0,
                    deaths: 0,
                    last_hit_ms: None,
                    respawn_at_ms: None,
                    spawned_at_ms: now_ms,
                    next_cure_at_ms: 0,
                };
                self.roles.insert(peer, role.clone());
                log::debug!(target: "meteor_network::stock_session",
                    "[stock-lan] enter-role accepted: peer={peer}, actor={actor_id}, hero={}, camp={}", role.hero, role.camp);

                for existing_role in &existing {
                    self.send_role_entered(peer, existing_role.entered_event(), now_ms)?;
                    if let Some(weapon_item_id) = existing_role.weapon_item_id {
                        self.send_weapon_equipped(
                            peer,
                            StockWeaponEquippedEvent {
                                weapon_item_id,
                                actor_id: existing_role.actor_id,
                            },
                            now_ms,
                        )?;
                    }
                }
                for (index, actor_id) in self.vips.into_iter().enumerate() {
                    if let Some(actor_id) = actor_id {
                        self.send_vip(
                            peer,
                            StockVipEvent {
                                camp: index as u32 + 1,
                                actor_id,
                            },
                            now_ms,
                        )?;
                    }
                }
                self.broadcast_role_entered(role.entered_event(), now_ms)?;
                self.runtime_role_ready(peer, now_ms)?;
                self.broadcast_player_names(now_ms)?;
                self.send_scene_bootstrap(peer, now_ms)?;
                self.send_scene_state(peer, now_ms)?;

                let mut scene_init = 0u16.to_le_bytes().to_vec();
                scene_init.extend_from_slice(&(now_ms as u32).to_le_bytes());
                self.transport.send_application(
                    peer,
                    STOCK_EVENT_SCENE_STATE,
                    scene_init,
                    STOCK_DISCRETE_RELIABLE_FLAGS,
                    now_ms,
                )?;
                log::debug!(target: "meteor_network::stock_session",
                    "[stock-lan] entry replies sent: peer={peer}, actor={actor_id}, scene_init=48");
                events.push(StockGameEvent::RoleEntered(role.entered_event()));
                self.assign_vips_if_ready(now_ms, events)?;
            }
            STOCK_REQUEST_HIT => {
                if self.round_ended {
                    return Ok(());
                }
                let request = StockHitRequest::decode(payload)?;
                let attacker_role = self
                    .roles
                    .get(&peer)
                    .cloned()
                    .ok_or(StockGameError::RoleNotEntered)?;
                let Some((target_peer, target_role)) = self
                    .roles
                    .iter()
                    .find(|(_, role)| role.actor_id == request.target_actor_id)
                    .map(|(peer, role)| (*peer, role.clone()))
                else {
                    return Ok(());
                };
                let attacker_item = attacker_role.equipped_item_id();
                let attacker_weapon = stock_weapon_from_item_id(u32::from(attacker_item))
                    .map_or(6, |(weapon, _)| weapon);
                let pose_id = (request.hit_word & 0x03ff) as u16;
                log::debug!(target: "meteor_network::stock_hit",
                    "request attacker={} target={} pose={} now_ms={} previous_hit_ms={:?} hp_before={} values_word={:#x}",
                    attacker_role.actor_id, target_role.actor_id, request.hit_word & 0x03ff,
                    now_ms, target_role.last_hit_ms, target_role.health, request.values_word);

                if target_role.health <= 0.0 {
                    return Ok(());
                }
                if now_ms.saturating_sub(target_role.spawned_at_ms) < 2_000 {
                    if let Some(target) = self.roles.get_mut(&target_peer) {
                        target.last_hit_ms = Some(now_ms);
                    }
                    return Ok(());
                }
                let defender_item = target_role.equipped_item_id();
                let attack = AttackBlock {
                    start: 0,
                    end: 0,
                    attack_type: request.force_hit_flags(),
                    check_friend: 0,
                    defense_value: request.defense_value(),
                    defense_move: request.defense_move(),
                    target_value: request.target_value(),
                    target_move: request.target_move(),
                    target_pose: Some(request.target_pose()),
                    target_pose_front: None,
                    target_pose_back: None,
                    target_pose_left: None,
                    target_pose_right: None,
                    bones: Vec::new(),
                };
                let attacker_power = self
                    .combat_resources
                    .weapon_stats(attacker_item)
                    .map(|stats| stats.attack);
                let attacker_item_bonus = attacker_role.item_attack_bonus();
                let defender_weapon_def = self
                    .combat_resources
                    .weapon_stats(defender_item)
                    .map(|stats| stats.defense);
                let combat_resources = &self.combat_resources;
                let result = {
                    let target = self
                        .roles
                        .get_mut(&target_peer)
                        .ok_or(StockGameError::RoleNotEntered)?;
                    target.last_hit_ms = Some(now_ms);
                    let mut effects = std::mem::take(&mut target.defensive_effects);
                    let result = meteor_combat::rebalance_effects::resolve_hit(
                        target,
                        &HitOptions {
                            attacker_team: attacker_role.camp as i32,
                            defender_team: target_role.camp as i32,
                            defender_in_guard: target_role
                                .state
                                .is_some_and(|state| state.state_words.guard_route),
                            defender_weapon_def,
                            weapon_id: attacker_weapon,
                            pose_id,
                            attack_power: attacker_power,
                            attack_bonus: attacker_item_bonus,
                            hit_key: Some(u32::from(pose_id)),
                            force_hit_flags: Some(request.force_hit_flags()),
                            attack_type: request.force_hit_flags() as i32,
                            dword1: Some(request.hit_word),
                            attack: Some(&attack),
                            pose_attack_percent: &|pose| combat_resources.pose_attack_percent(pose),
                        },
                        self.battle_rules.clone(),
                        &mut effects,
                        meteor_combat::rebalance_effects::HitContext {
                            attacker: &attacker_role.actor_id.to_string(),
                            instance: attacker_role
                                .state
                                .map_or(0, |s| u32::from(s.state_words.pose_sequence)),
                            now_seconds: now_ms as f64 / 1000.0,
                        },
                    );
                    target.defensive_effects = effects;
                    result
                };
                log::debug!(target: "meteor_network::stock_hit",
                    "resolved attacker={} target={} pose={} now_ms={} damage={} hp_after={:?} outcome={:?}",
                    attacker_role.actor_id, target_role.actor_id, pose_id, now_ms,
                    result.damage, self.roles.get(&target_peer).map(|target| target.health), result.outcome);
                if result.killed {
                    if let Some(target) = self.roles.get_mut(&target_peer) {
                        target.hit_status_effects.active.clear();
                        target.respawn_at_ms =
                            can_respawn(Some(self.rule)).then_some(now_ms.saturating_add(3_000));
                        target.deaths = target.deaths.saturating_add(1);
                    }
                    if let Some(attacker) = self.roles.get_mut(&peer) {
                        attacker.kills = attacker.kills.saturating_add(1);
                    }
                    self.transport.send_application(
                        target_peer,
                        STOCK_EVENT_DEATH,
                        attacker_role.actor_id.to_le_bytes(),
                        STOCK_DISCRETE_RELIABLE_FLAGS,
                        now_ms,
                    )?;
                    let event = StockKillEvent {
                        attacker_actor_id: attacker_role.actor_id,
                        target_actor_id: target_role.actor_id,
                    };
                    self.broadcast_kill(event, now_ms)?;
                    events.push(StockGameEvent::Kill(event));
                    if self.rule == 2
                        && self.escort.is_some_and(|escort| {
                            escort.carrier_actor_id == Some(target_role.actor_id)
                        })
                    {
                        self.drop_escort_from_role(&target_role, now_ms, events)?;
                    }
                    if self.rule == 4
                        && self
                            .vips
                            .iter()
                            .flatten()
                            .any(|actor_id| *actor_id == target_role.actor_id)
                    {
                        self.complete_round(
                            target_role.camp == 1,
                            target_role.camp == 2,
                            now_ms,
                            events,
                        )?;
                    } else {
                        self.finish_team_round_if_needed(now_ms, events)?;
                    }
                    return Ok(());
                }
                let guarded = result.guarded;
                let (pose_id, move_hundred, value_hundred, event_opcode) = if guarded {
                    (
                        (u32::from(request.target_pose()) << 16) | request.direction_selector(),
                        u32::from((result.defense_move * 100.0).round() as u16),
                        u32::from((result.defense_value * 100.0).round() as u16),
                        STOCK_EVENT_GUARD_HIT,
                    )
                } else {
                    (
                        u32::from(request.target_pose()),
                        u32::from((result.target_move * 100.0).round() as u16),
                        u32::from((result.target_value * 100.0).round() as u16),
                        STOCK_EVENT_BE_HIT,
                    )
                };
                let event = StockHitReactionEvent {
                    pose_id,
                    move_hundred,
                    value_hundred,
                    context: if result.rebalance_effect > 0 {
                        meteor_combat::rebalance_effects::STOCK_EFFECT_CONTEXT
                            + u32::from(result.rebalance_effect)
                    } else {
                        (request.hit_word >> 16) & 0x0f
                    },
                    target_actor_id: request.target_actor_id,
                };
                let mut recipients = vec![peer];
                if target_peer != peer {
                    recipients.push(target_peer);
                }
                if result.rebalance_effect > 0 {
                    recipients = self.roles.keys().copied().collect();
                    self.last_vitals_downlink_ms = None;
                }
                for recipient in recipients {
                    self.transport.send_application(
                        recipient,
                        event_opcode,
                        event.encode(),
                        STOCK_EVENT_HIT_FLAGS,
                        now_ms,
                    )?;
                }
                events.push(StockGameEvent::HitReaction { guarded, event });

                if !result.guarded
                    && result.rebalance_effect != meteor_combat::rebalance_effects::INVALID_HIT
                {
                    if let Some(definition) = self
                        .combat_resources
                        .pose_status_effect((request.hit_word & 0x03ff) as u16)
                    {
                        if let Some(target) = self.roles.get_mut(&target_peer) {
                            let delta = target
                                .hit_status_effects
                                .apply(definition, now_ms as f64 / 1000.0);
                            target.health =
                                (target.health + delta.health as f32).clamp(0.0, target.max_health);
                            target.anger = (target.anger + delta.anger as f32).clamp(0.0, 100.0);
                        }
                        let event = crate::stock_protocol::StockPropertyEffectEvent {
                            actor_id: request.target_actor_id,
                            category: 2,
                            resource_id: definition.id,
                        };
                        self.broadcast_discrete(
                            crate::stock_protocol::STOCK_EVENT_PROPERTY_EFFECT,
                            &event.encode(),
                            now_ms,
                        )?;
                        events.push(StockGameEvent::PropertyEffect(event));
                    }
                }
            }
            STOCK_REQUEST_EQUIP_WEAPON => {
                let request = StockEquipWeaponRequest::decode(payload)?;
                let actor_id = {
                    let role = self
                        .roles
                        .get_mut(&peer)
                        .ok_or(StockGameError::RoleNotEntered)?;
                    role.weapon_item_id = Some(request.weapon_item_id);
                    if let Some(slot) = role
                        .weapon_slots
                        .iter()
                        .position(|item| *item == Some(request.weapon_item_id))
                    {
                        role.weapon_slots.swap(0, slot);
                    } else {
                        role.weapon_slots[0] = Some(request.weapon_item_id);
                    }
                    role.actor_id
                };
                let event = StockWeaponEquippedEvent {
                    weapon_item_id: request.weapon_item_id,
                    actor_id,
                };
                self.broadcast_weapon_equipped(event, now_ms)?;
                events.push(StockGameEvent::WeaponEquipped(event));
            }
            crate::stock_protocol::STOCK_REQUEST_POSE_CHAIN => {
                let Some(role) = self.roles.get(&peer) else {
                    return Ok(());
                };
                if payload.len() != 8 {
                    return Ok(());
                }
                let mut bytes = [0; 10];
                bytes[..2].copy_from_slice(&(role.actor_id as u16).to_le_bytes());
                bytes[2..].copy_from_slice(payload);
                let event = crate::stock_protocol::StockPoseChainEvent::decode(&bytes)?;
                for recipient in self
                    .transport
                    .peer_addresses()
                    .into_iter()
                    .filter(|p| *p != peer)
                {
                    self.transport.send_application(
                        recipient,
                        crate::stock_protocol::STOCK_EVENT_POSE_CHAIN,
                        bytes,
                        0,
                        now_ms,
                    )?;
                }
                events.push(StockGameEvent::PoseChain(event));
            }
            STOCK_REQUEST_ROLE_STATE => {
                let state = decode_role_state_request(payload)?;
                let state = self
                    .scene_frame
                    .map_or(state, |frame| frame.to_scene(state));
                let role = self
                    .roles
                    .get_mut(&peer)
                    .ok_or(StockGameError::RoleNotEntered)?;
                role.state = Some(state);
                role.dirty = true;
                let started = role.defensive_effects.enter_pose(
                    self.battle_rules.clone(),
                    state.state_words.pose_id,
                    u32::from(state.state_words.pose_sequence),
                    &mut role.anger,
                    100.0,
                    role.health > 0.0,
                );
                let event = StockHitReactionEvent {
                    pose_id: u32::from(state.state_words.pose_id),
                    move_hundred: 0,
                    value_hundred: 0,
                    context: meteor_combat::rebalance_effects::STOCK_EFFECT_CONTEXT
                        + u32::from(meteor_combat::rebalance_effects::ARMOR_START),
                    target_actor_id: role.actor_id,
                };
                if started {
                    self.last_vitals_downlink_ms = None;
                    self.broadcast_discrete(STOCK_EVENT_BE_HIT, &event.encode(), now_ms)?;
                    events.push(StockGameEvent::HitReaction {
                        guarded: false,
                        event,
                    });
                }
            }
            STOCK_REQUEST_WEAPON_OBJECT => {
                if payload.is_empty() {
                    let role = self
                        .roles
                        .get_mut(&peer)
                        .ok_or(StockGameError::RoleNotEntered)?;
                    if let Some(current) = role.weapon_item_id {
                        if let Some(other) = role
                            .weapon_slots
                            .iter()
                            .flatten()
                            .find(|item| **item != current)
                            .copied()
                        {
                            role.weapon_item_id = Some(other);
                        }
                    }
                } else {
                    let request = StockWeaponObjectRequest::decode(payload)?;
                    let actor_id = {
                        let role = self
                            .roles
                            .get_mut(&peer)
                            .ok_or(StockGameError::RoleNotEntered)?;
                        if request.selector == 1 {
                            if let Some(current) = role.weapon_item_id {
                                if let Some(active_slot) = role
                                    .weapon_slots
                                    .iter()
                                    .position(|slot| *slot == Some(current))
                                {
                                    role.weapon_slots[1 - active_slot] = None;
                                }
                            }
                        }
                        role.actor_id
                    };
                    let mut event_payload = Vec::with_capacity(4 + payload.len());
                    event_payload.extend_from_slice(&actor_id.to_le_bytes());
                    event_payload.extend_from_slice(payload);
                    self.broadcast_with_flags(
                        STOCK_EVENT_WEAPON_OBJECT,
                        &event_payload,
                        STOCK_EVENT_WEAPON_OBJECT_FLAGS,
                        now_ms,
                    )?;
                }
            }
            STOCK_REQUEST_DROP_ITEM => {
                require_empty_stock_payload(payload)?;
                let actor_id = self
                    .roles
                    .get(&peer)
                    .ok_or(StockGameError::RoleNotEntered)?
                    .actor_id;
                self.broadcast_with_flags(
                    STOCK_EVENT_DROP_ITEM,
                    &StockDropItemEvent { actor_id }.encode(),
                    STOCK_EVENT_DROP_ITEM_FLAGS,
                    now_ms,
                )?;
            }
            STOCK_REQUEST_SPAWN_SCENE_ITEM => {
                if !self.roles.contains_key(&peer) {
                    return Err(StockGameError::RoleNotEntered);
                }
                let request = StockSceneItemSpawnRequest::decode(payload)?;
                if valid_stock_scene_item_key(request.category, request.item_id) {
                    let record_id = self.allocate_scene_record_id();
                    let record = request.into_record(record_id);
                    let position = record.role_position();
                    let position = self
                        .scene_frame
                        .map_or(position, |frame| frame.position_to_scene(position));
                    self.scene_items.insert(
                        record_id,
                        StockHostedSceneItem {
                            seed: StockAuthoredSceneItem {
                                item_id: record.item_id,
                                category: record.category,
                                position,
                                half_extents: [0.0; 3],
                                respawn_ms: None,
                                scene_reference: None,
                                repeat_on_xstar: false,
                                active: true,
                                interactive: true,
                            },
                            record_id,
                            active: true,
                            script_active: true,
                            interactive: true,
                            respawn_at_ms: None,
                            expires_at_ms: Some(
                                now_ms.saturating_add(STOCK_DYNAMIC_ITEM_LIFETIME_MS),
                            ),
                            authored: false,
                        },
                    );
                    self.broadcast_discrete(STOCK_EVENT_SCENE_SPAWN, &record.encode(), now_ms)?;
                    events.push(StockGameEvent::SceneItemsGrounded {
                        records: vec![record],
                        spawned: true,
                    });
                }
            }
            42 => {
                if payload.len() != 4 {
                    return Err(StockProtocolError::InvalidLength {
                        expected: 4,
                        actual: payload.len(),
                    }
                    .into());
                }
                let id = u32::from_le_bytes(payload.try_into().unwrap());
                let definition = [489, 468]
                    .into_iter()
                    .filter_map(|pose| self.combat_resources.pose_self_status_effect(pose))
                    .find(|effect| {
                        effect.id == id
                            && (effect.attack_bonus > 0
                                || (effect.health < 0 && effect.duration == 0.0))
                    });
                if let (Some(definition), Some(role)) = (definition, self.roles.get_mut(&peer)) {
                    if role.health > 0.0 && role.respawn_at_ms.is_none() {
                        let delta = role
                            .hit_status_effects
                            .apply(definition, now_ms as f64 / 1000.0);
                        role.pending_self_health =
                            role.pending_self_health.saturating_add(delta.health);
                        let event = crate::stock_protocol::StockPropertyEffectEvent {
                            actor_id: role.actor_id,
                            category: 2,
                            resource_id: id,
                        };
                        self.broadcast_discrete(
                            crate::stock_protocol::STOCK_EVENT_PROPERTY_EFFECT,
                            &event.encode(),
                            now_ms,
                        )?;
                        events.push(StockGameEvent::PropertyEffect(event));
                        self.update_item_effects(now_ms, events)?;
                    }
                }
            }
            STOCK_REQUEST_CURE => {
                self.handle_cure_request(peer, StockCureRequest::decode(payload), now_ms, events)?;
            }
            STOCK_REQUEST_ROLE_RATE => {
                if self.roles.contains_key(&peer) {
                    if let Some(hz) = StockRoleRate::decode(payload)?.requested_hz() {
                        self.requested_role_rates.insert(peer, hz);
                    }
                }
            }
            STOCK_REQUEST_READY => {
                if !payload.is_empty() {
                    return Err(StockProtocolError::InvalidLength {
                        expected: 0,
                        actual: payload.len(),
                    }
                    .into());
                }
                self.ready_peers.insert(peer);
                log::debug!(target: "meteor_network::stock_session", "[stock-lan] battle ready received: peer={peer}");
            }
            STOCK_EVENT_SCENE_OPS => {
                if self.runtime.peers.contains(&peer) && Some(peer) != self.runtime.owner {
                    return Ok(());
                }
                if !self.roles.contains_key(&peer) {
                    return Err(StockGameError::RoleNotEntered);
                }
                let event = StockSceneOpsEvent::decode(payload)?;
                log::debug!(target: "meteor_network::scene_sync",
                    "event56 relay source={} operations={:?}", peer, event.operations);
                if !event.operations.is_empty() {
                    self.broadcast_discrete_except(peer, STOCK_EVENT_SCENE_OPS, payload, now_ms)?;
                    for operation in &event.operations {
                        retain_scene_write(&mut self.scene_state, *operation);
                    }
                    self.refresh_xstar_items(peer, &event, now_ms, events)?;
                    self.apply_scene_item_ops(&event, now_ms, events)?;
                    events.push(StockGameEvent::SceneOps(event));
                }
            }
            _ => {
                log::debug!(target: "meteor_network::stock_session",
                    "[stock-lan] unhandled game request: peer={peer}, opcode={opcode}, bytes={}", payload.len());
                events.push(StockGameEvent::RawApplication {
                    peer,
                    opcode,
                    payload: payload.to_vec(),
                });
            }
        }
        Ok(())
    }

    fn handle_cure_request(
        &mut self,
        peer: SocketAddr,
        request: StockCureRequest,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        if self.rule != 4 || self.round_ended || request.target_actor_ids.is_empty() {
            return Ok(());
        }
        let healer = self
            .roles
            .get(&peer)
            .cloned()
            .ok_or(StockGameError::RoleNotEntered)?;
        if healer.health <= 0.0
            || healer.next_cure_at_ms > now_ms
            || !self
                .vips
                .iter()
                .flatten()
                .any(|vip| *vip == healer.actor_id)
        {
            return Ok(());
        }
        let Some(healer_state) = healer.state else {
            return Ok(());
        };
        if let Some(role) = self.roles.get_mut(&peer) {
            role.next_cure_at_ms = now_ms.saturating_add(STOCK_CURE_RECOVERY_MS);
        }

        let range_sq = STOCK_CURE_RANGE * STOCK_CURE_RANGE;
        let mut seen = HashSet::new();
        let targets = request
            .target_actor_ids
            .into_iter()
            .filter(|actor_id| seen.insert(*actor_id))
            .filter_map(|actor_id| {
                self.roles
                    .iter()
                    .find(|(_, role)| role.actor_id == u32::from(actor_id))
                    .and_then(|(target_peer, target)| {
                        let state = target.state?;
                        let dx = healer_state.position.x - state.position.x;
                        let dz = healer_state.position.z - state.position.z;
                        (target.actor_id != healer.actor_id
                            && target.camp == healer.camp
                            && target.health <= 0.0
                            && dx * dx + dz * dz <= range_sq)
                            .then_some((*target_peer, target.actor_id))
                    })
            })
            .collect::<Vec<_>>();

        for (target_peer, actor_id) in targets {
            if let Some(target) = self.roles.get_mut(&target_peer) {
                target.defensive_effects = Default::default();
                target.health = target.max_health;
                target.anger = 0.0;
                target.last_hit_ms = None;
                target.respawn_at_ms = None;
                target.spawned_at_ms = now_ms;
                target.dirty = true;
            }
            let event = StockRespawnEvent { actor_id };
            self.broadcast_respawn(event, now_ms)?;
            events.push(StockGameEvent::Respawn(event));
        }
        Ok(())
    }

    fn update_role_rate(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        if self.roles.is_empty() {
            self.role_rate_sample_ms = None;
            self.role_rate_feedback = 0;
            return Ok(());
        }
        let Some(last_sample) = self.role_rate_sample_ms else {
            self.role_rate_sample_ms = Some(now_ms);
            return Ok(());
        };
        let samples = now_ms.saturating_sub(last_sample) / STOCK_ROLE_RATE_SAMPLE_MS;
        if samples == 0 {
            return Ok(());
        }
        self.role_rate_sample_ms =
            Some(last_sample.saturating_add(samples.saturating_mul(STOCK_ROLE_RATE_SAMPLE_MS)));
        for _ in 0..samples {
            let votes = self
                .requested_role_rates
                .iter()
                .filter(|(peer, _)| self.roles.contains_key(peer))
                .fold(0_i32, |votes, (_, requested)| {
                    votes + i32::from(*requested > self.role_rate)
                        - i32::from(*requested < self.role_rate)
                });
            self.role_rate_feedback += votes.signum();
            let candidate = if self.role_rate_feedback > STOCK_HOST_RATE_FEEDBACK_THRESHOLD {
                Some(self.role_rate + 1)
            } else if self.role_rate_feedback < -STOCK_HOST_RATE_FEEDBACK_THRESHOLD {
                self.role_rate.checked_sub(1)
            } else {
                None
            };
            let Some(candidate) = candidate else {
                continue;
            };
            self.role_rate_feedback = 0;
            if !(STOCK_ROLE_RATE_MIN..=STOCK_ROLE_RATE_MAX).contains(&candidate)
                || candidate == self.role_rate
            {
                continue;
            }
            self.role_rate = candidate;
            self.last_role_downlink_ms = Some(now_ms);
            self.last_vitals_downlink_ms = Some(now_ms);
            let rate = StockRoleRate {
                hz: candidate as f32,
            };
            self.broadcast_discrete(STOCK_EVENT_ROLE_RATE, &rate.encode(), now_ms)?;
            events.push(StockGameEvent::RoleRate(candidate));
        }
        Ok(())
    }

    fn broadcast_dirty_role_states(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        let interval_ms = 1_000 / u64::from(self.role_rate);
        if self
            .last_role_downlink_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < interval_ms)
        {
            return Ok(());
        }
        let dirty: Vec<StockRoleStateRecord> = self
            .roles
            .values()
            .filter(|role| role.dirty)
            .filter_map(|role| {
                role.state.map(|state| StockRoleStateRecord {
                    actor_id: role.actor_id as u16,
                    state: self
                        .scene_frame
                        .map_or(state, |frame| frame.to_stock(state)),
                })
            })
            .collect();
        if dirty.is_empty() {
            return Ok(());
        }
        let recipients: Vec<(SocketAddr, u32)> = self
            .roles
            .iter()
            .map(|(peer, role)| (*peer, role.actor_id))
            .collect();
        for (peer, own_actor_id) in recipients {
            let records: Vec<_> = dirty
                .iter()
                .copied()
                .filter(|record| u32::from(record.actor_id) != own_actor_id)
                .collect();
            if !records.is_empty() {
                self.transport.send_application(
                    peer,
                    STOCK_EVENT_ROLE_STATE,
                    encode_role_state_event(&records),
                    STOCK_EVENT_ROLE_STATE_FLAGS,
                    now_ms,
                )?;
            }
        }
        for role in self.roles.values_mut() {
            role.dirty = false;
        }
        self.last_role_downlink_ms = Some(now_ms);
        Ok(())
    }

    fn broadcast_vitals(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        let interval_ms = 1_000 / u64::from(self.role_rate);
        if self
            .last_vitals_downlink_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < interval_ms)
        {
            return Ok(());
        }
        let samples = self
            .roles
            .iter()
            .map(|(peer, role)| (*peer, role.vitals_event()))
            .collect::<Vec<_>>();
        for (peer, vitals) in samples {
            self.transport.send_application(
                peer,
                STOCK_EVENT_VITALS,
                vitals.encode(),
                STOCK_EVENT_ROLE_STATE_FLAGS,
                now_ms,
            )?;
        }

        self.remote_vitals_revision = self.remote_vitals_revision.wrapping_add(1);
        for role in self.roles.values() {
            let payload = crate::stock_remote_vitals::RemoteVitals {
                actor_id: role.actor_id,
                revision: self.remote_vitals_revision,
                vitals: role.vitals_event(),
            }
            .encode();
            for peer in &self.remote_vitals_peers {
                if self
                    .roles
                    .get(peer)
                    .is_some_and(|own| own.actor_id != role.actor_id)
                {
                    self.transport.send_application(
                        *peer,
                        crate::stock_remote_vitals::EVENT_REMOTE_VITALS,
                        payload.clone(),
                        STOCK_EVENT_ROLE_STATE_FLAGS,
                        now_ms,
                    )?;
                }
            }
        }
        self.last_vitals_downlink_ms = Some(now_ms);
        Ok(())
    }

    fn respawn_due_roles(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        let due = self
            .roles
            .iter()
            .filter_map(|(peer, role)| {
                role.respawn_at_ms
                    .is_some_and(|at| now_ms > at)
                    .then_some((*peer, role.actor_id))
            })
            .collect::<Vec<_>>();
        for (peer, actor_id) in due {
            if let Some(role) = self.roles.get_mut(&peer) {
                role.defensive_effects = Default::default();
                role.health = role.max_health;
                role.anger = 0.0;
                role.last_hit_ms = None;
                role.respawn_at_ms = None;
                role.spawned_at_ms = now_ms;
                role.state = None;
                role.dirty = false;
            }
            let event = StockRespawnEvent { actor_id };
            self.broadcast_respawn(event, now_ms)?;
            events.push(StockGameEvent::Respawn(event));
            self.transport.send_application(
                peer,
                crate::stock_protocol::STOCK_EVENT_LOCAL_RESPAWN,
                Vec::new(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        Ok(())
    }

    fn broadcast_respawn(
        &mut self,
        event: StockRespawnEvent,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        for recipient in self.transport.peer_addresses() {
            self.transport.send_application(
                recipient,
                STOCK_EVENT_RESPAWN,
                event.encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        Ok(())
    }

    fn allocate_scene_record_id(&mut self) -> u32 {
        loop {
            let record_id = self.next_scene_record_id;
            self.next_scene_record_id = if record_id >= 0x7ffe {
                1
            } else {
                record_id + 1
            };
            let escort_uses_id = self
                .escort
                .is_some_and(|escort| escort.record_id == record_id);
            if !escort_uses_id && !self.scene_items.contains_key(&record_id) {
                return record_id;
            }
        }
    }

    fn active_scene_records(&self) -> Vec<StockSceneItemRecord> {
        let mut records = self
            .scene_items
            .values()
            .filter(|item| item.active && item.interactive)
            .map(|item| item.record(self.scene_frame))
            .collect::<Vec<_>>();
        if let Some(escort) = self.escort {
            records.push(escort.scene_record(self.escort_item_id()));
        }
        records
    }

    fn send_scene_bootstrap(
        &mut self,
        peer: SocketAddr,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        let active_records = self.active_scene_records();
        for records in active_records.chunks(STOCK_SCENE_RECORDS_PER_PACKET) {
            let payload = records
                .iter()
                .flat_map(|record| record.encode())
                .collect::<Vec<_>>();
            self.transport.send_application(
                peer,
                STOCK_EVENT_SCENE_SNAPSHOT,
                payload,
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        if let Some(escort) = self.escort {
            let Some(actor_id) = escort.carrier_actor_id else {
                return Ok(());
            };
            let taken = StockSceneTakenEvent {
                actor_id,
                record_id: escort.record_id,
                category: STOCK_ESCORT_CATEGORY,
                item_id: self.escort_item_id(),
            };
            self.transport.send_application(
                peer,
                STOCK_EVENT_SCENE_TAKEN,
                taken.encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        Ok(())
    }

    fn send_scene_state(&mut self, peer: SocketAddr, now_ms: u64) -> Result<(), StockGameError> {
        let mut scene_state = self.scene_state.clone();
        for item in self.scene_items.values().filter(|item| item.authored) {
            let Some(reference) = item.seed.scene_reference else {
                continue;
            };
            for (kind, value) in [
                (
                    crate::stock_protocol::STOCK_SCENE_OP_ACTIVE,
                    u32::from(item.active),
                ),
                (
                    crate::stock_protocol::STOCK_SCENE_OP_INTERACTIVE,
                    u32::from(item.active && item.interactive),
                ),
            ] {
                retain_scene_write(
                    &mut scene_state,
                    StockSceneOperation {
                        reference,
                        kind,
                        value,
                        extra: 0,
                    },
                );
            }
        }
        for operations in scene_state.chunks(STOCK_SCENE_OPS_PER_PACKET) {
            self.transport.send_application(
                peer,
                STOCK_EVENT_SCENE_OPS,
                StockSceneOpsEvent {
                    timestamp_ms: now_ms as u32,
                    operations: operations.to_vec(),
                }
                .encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        Ok(())
    }

    fn broadcast_scene_snapshot(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        for peer in self.transport.peer_addresses() {
            self.send_scene_bootstrap(peer, now_ms)?;
        }
        Ok(())
    }

    fn refresh_xstar_items(
        &mut self,
        peer: SocketAddr,
        event: &StockSceneOpsEvent,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        use crate::stock_protocol::{
            STOCK_EFFECT_XSTAR, STOCK_SCENE_OP_EFFECT, STOCK_SCENE_OP_POSE,
        };

        let star_attack = event.operations.windows(2).any(|ops| {
            ops[0].kind == STOCK_SCENE_OP_EFFECT
                && ops[0].value == STOCK_EFFECT_XSTAR
                && ops[1].kind == STOCK_SCENE_OP_POSE
                && ops[1].reference == ops[0].reference
                && ops[1].value == 1
        });
        if self.round_ended || self.stage_id.as_deref() != Some("sn21") || !star_attack {
            return Ok(());
        }

        if self
            .last_xstar
            .is_some_and(|(owner, timestamp, received_at)| {
                if owner == peer {
                    timestamp == event.timestamp_ms
                } else {
                    now_ms.saturating_sub(received_at) < 5_000
                }
            })
        {
            return Ok(());
        }
        self.last_xstar = Some((peer, event.timestamp_ms, now_ms));

        let inactive = self
            .scene_items
            .iter()
            .filter_map(|(&id, item)| (item.seed.repeat_on_xstar && !item.active).then_some(id))
            .collect::<Vec<_>>();
        for id in inactive {
            self.respawn_scene_item(id, now_ms, events)?;
        }
        Ok(())
    }

    fn respawn_scene_item(
        &mut self,
        previous_id: u32,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        let Some(mut item) = self.scene_items.remove(&previous_id) else {
            return Ok(());
        };
        item.record_id = self.allocate_scene_record_id();
        item.active = true;
        item.script_active = true;
        item.interactive = true;
        item.respawn_at_ms = None;
        let record = item.record(self.scene_frame);
        self.scene_items.insert(item.record_id, item);
        self.broadcast_discrete(STOCK_EVENT_SCENE_SPAWN, &record.encode(), now_ms)?;
        events.push(StockGameEvent::SceneItemsGrounded {
            records: vec![record],
            spawned: true,
        });
        Ok(())
    }

    fn apply_scene_item_ops(
        &mut self,
        event: &StockSceneOpsEvent,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        use crate::stock_protocol::{STOCK_SCENE_OP_ACTIVE, STOCK_SCENE_OP_INTERACTIVE};
        let mut released = Vec::new();
        for item in self.scene_items.values_mut().filter(|item| item.authored) {
            let Some(reference) = item.seed.scene_reference else {
                continue;
            };
            let was_ready = item.active && item.interactive;
            for operation in event
                .operations
                .iter()
                .filter(|op| op.reference == reference)
            {
                match operation.kind {
                    STOCK_SCENE_OP_ACTIVE => {
                        let active = operation.value != 0;
                        if active != item.script_active {
                            item.active = active;
                            item.script_active = active;
                            item.respawn_at_ms = None;
                        }
                    }
                    STOCK_SCENE_OP_INTERACTIVE => item.interactive = operation.value != 0,
                    _ => {}
                }
            }
            if !was_ready && item.active && item.interactive {
                item.respawn_at_ms = None;
                released.push(item.record(self.scene_frame));
            }
        }
        for record in released {
            self.broadcast_discrete(STOCK_EVENT_SCENE_SPAWN, &record.encode(), now_ms)?;
            events.push(StockGameEvent::SceneItemsGrounded {
                records: vec![record],
                spawned: true,
            });
        }
        Ok(())
    }

    fn update_scene_items(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        if self.round_ended {
            return Ok(());
        }
        let expired = self
            .scene_items
            .iter()
            .filter_map(|(id, item)| {
                (!item.authored && item.expires_at_ms.is_some_and(|expires| now_ms > expires))
                    .then_some(*id)
            })
            .collect::<Vec<_>>();
        for id in expired {
            let item = self.scene_items.remove(&id).unwrap();
            let event = crate::stock_protocol::StockSceneItemRemovedEvent {
                record_id: id,
                category: item.seed.category,
                item_id: item.seed.item_id,
            };
            self.broadcast_discrete(
                crate::stock_protocol::STOCK_EVENT_SCENE_ITEM_REMOVED,
                &event.encode(),
                now_ms,
            )?;
            events.push(StockGameEvent::SceneItemRemoved(event));
        }
        if self.roles.is_empty() || self.ready_peers.is_empty() {
            return Ok(());
        }

        let respawn_due = self
            .scene_items
            .iter()
            .filter_map(|(record_id, item)| {
                (!item.active && item.respawn_at_ms.is_some_and(|due| now_ms >= due))
                    .then_some(*record_id)
            })
            .collect::<Vec<_>>();
        for old_record_id in respawn_due {
            self.respawn_scene_item(old_record_id, now_ms, events)?;
        }

        let pickup_candidates = self
            .scene_items
            .iter()
            .filter(|(_, item)| item.active && item.interactive)
            .filter_map(|(record_id, item)| {
                self.roles
                    .iter()
                    .filter(|(peer, role)| {
                        self.ready_peers.contains(peer)
                            && role.health > 0.0
                            && role_can_take_scene_item(role, item)
                    })
                    .filter_map(|(peer, role)| {
                        role.state
                            .filter(|state| role_touches_scene_item(state.position, item))
                            .map(|_| (*peer, role.actor_id))
                    })
                    .min_by_key(|(_, actor_id)| *actor_id)
                    .map(|(peer, actor_id)| (*record_id, peer, actor_id))
            })
            .collect::<Vec<_>>();

        for (record_id, peer, actor_id) in pickup_candidates {
            let Some(mut item) = self.scene_items.remove(&record_id) else {
                continue;
            };
            if !item.active
                || !item.interactive
                || !self
                    .roles
                    .get(&peer)
                    .is_some_and(|role| role_can_take_scene_item(role, &item))
            {
                self.scene_items.insert(record_id, item);
                continue;
            }
            let record = item.record(self.scene_frame);
            if item.seed.category == 1 {
                if let Some(role) = self.roles.get_mut(&peer) {
                    if let Some(slot) = role.weapon_slots.iter().position(Option::is_none) {
                        role.weapon_slots[slot] = Some(item.seed.item_id);
                    }
                }
            } else if let Some(role) = self.roles.get_mut(&peer) {
                apply_stock_item_effect(role, item.seed.item_id, now_ms);
            }
            item.active = false;
            item.respawn_at_ms = item
                .authored
                .then_some(item.seed.respawn_ms)
                .flatten()
                .filter(|delay| *delay > 0)
                .map(|delay| now_ms.saturating_add(delay));
            if item.authored {
                self.scene_items.insert(record_id, item);
            }
            let taken = StockSceneTakenEvent {
                actor_id,
                record_id,
                category: record.category,
                item_id: record.item_id,
            };
            self.broadcast_discrete(STOCK_EVENT_SCENE_TAKEN, &taken.encode(), now_ms)?;
            events.push(StockGameEvent::SceneItemTaken {
                event: taken,
                record: Some(record),
            });
        }
        Ok(())
    }

    pub fn replace_environment_geometry(&mut self, geometry: meteor_scene::hazards::StageHazards) {
        self.hazards.volumes = geometry.volumes;
        self.hazards.machines = geometry.machines;
        self.hazards.world_bounds = geometry.world_bounds;
    }

    fn update_environment_hazards(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        let dt = self
            .hazard_tick_ms
            .replace(now_ms)
            .map_or(0.0, |last| now_ms.saturating_sub(last) as f32 / 1000.0);
        if self.round_ended {
            return Ok(());
        }
        let fighters = self
            .roles
            .iter()
            .filter(|(peer, _)| self.ready_peers.contains(peer))
            .filter_map(|(_, role)| {
                role.state.map(|state| {
                    (
                        role.actor_id.to_string(),
                        [state.position.x, state.position.y, state.position.z],
                        role.health <= 0.0,
                        now_ms.saturating_sub(role.spawned_at_ms) <= 3000,
                    )
                })
            })
            .collect::<Vec<_>>();
        let mut hits = Vec::new();
        self.hazards.update(dt, &fighters, &mut |id, _, amount, _| {
            hits.push((id.to_owned(), amount))
        });
        for (id, amount) in hits {
            let Some(peer) = self
                .roles
                .iter()
                .find_map(|(peer, role)| (role.actor_id.to_string() == id).then_some(*peer))
            else {
                continue;
            };
            let role = self.roles.get_mut(&peer).unwrap();
            if amount <= 0 || role.health <= 0.0 || now_ms.saturating_sub(role.spawned_at_ms) < 2000
            {
                continue;
            }
            role.health = (role.health - amount as f32).max(0.0);
            let killed = role.health <= 0.0;
            if killed {
                role.deaths = role.deaths.saturating_add(1);
                role.active_item_effects.clear();
                role.hit_status_effects.active.clear();
                role.max_health = self.base_max_health;
                role.respawn_at_ms =
                    can_respawn(Some(self.rule)).then_some(now_ms.saturating_add(3000));
            }
            let role = role.clone();

            self.last_vitals_downlink_ms = None;
            self.broadcast_vitals(now_ms)?;
            if !killed {
                let event = StockHitReactionEvent {
                    pose_id: 90,
                    move_hundred: 100,
                    value_hundred: 0,
                    context: 0,
                    target_actor_id: role.actor_id,
                };
                self.broadcast_discrete(STOCK_EVENT_BE_HIT, &event.encode(), now_ms)?;
                events.push(StockGameEvent::HitReaction {
                    guarded: false,
                    event,
                });
                continue;
            }
            self.transport.send_application(
                peer,
                STOCK_EVENT_DEATH,
                0u32.to_le_bytes(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;

            for recipient in self.roles.keys().copied().collect::<Vec<_>>() {
                self.transport.send_application(
                    recipient,
                    STOCK_EVENT_ENVIRONMENT_DEATH,
                    role.actor_id.to_le_bytes(),
                    STOCK_EVENT_HIT_FLAGS,
                    now_ms,
                )?;
            }
            events.push(StockGameEvent::EnvironmentDeath(role.actor_id));
            if self.rule == 2
                && self
                    .escort
                    .is_some_and(|escort| escort.carrier_actor_id == Some(role.actor_id))
            {
                self.drop_escort_from_role(&role, now_ms, events)?;
            }
            if self.rule == 4 && self.vips.iter().flatten().any(|id| *id == role.actor_id) {
                self.complete_round(role.camp == 1, role.camp == 2, now_ms, events)?;
                break;
            }
            self.finish_team_round_if_needed(now_ms, events)?;

            if self.round_ended || self.roles.get(&peer).is_none_or(|r| r.health > 0.0) {
                break;
            }
        }
        Ok(())
    }

    fn update_item_effects(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        let mut deaths = Vec::new();
        for role in self.roles.values_mut() {
            let was_alive = role.health > 0.0
                || (role.respawn_at_ms.is_none() && !role.hit_status_effects.active.is_empty());
            let self_health = std::mem::take(&mut role.pending_self_health);
            let status_delta = role.hit_status_effects.tick(now_ms as f64 / 1000.0);
            if role.health > 0.0 {
                role.health = (role.health + status_delta.health as f32 + self_health as f32)
                    .clamp(0.0, role.max_health);
                role.anger = (role.anger + status_delta.anger as f32).clamp(0.0, 100.0);
            }
            let mut periodic_health = 0i32;
            let mut periodic_anger = 0i32;
            for active in role.active_item_effects.values_mut() {
                let Some(mut next_tick) = active.next_tick_ms else {
                    continue;
                };
                while next_tick <= now_ms && next_tick <= active.expires_at_ms {
                    periodic_health = periodic_health.saturating_add(active.effect.periodic_health);
                    periodic_anger = periodic_anger.saturating_add(active.effect.periodic_anger);
                    next_tick = next_tick.saturating_add(active.effect.tick_ms.max(1));
                }
                active.next_tick_ms = Some(next_tick);
            }
            if periodic_health != 0 {
                role.health = (role.health + periodic_health as f32).clamp(0.0, role.max_health);
            }
            if periodic_anger != 0 {
                role.anger = (role.anger + periodic_anger as f32).clamp(0.0, 100.0);
            }

            let expired = role
                .active_item_effects
                .iter()
                .filter_map(|(item_id, active)| {
                    (now_ms >= active.expires_at_ms).then_some(*item_id)
                })
                .collect::<Vec<_>>();
            for item_id in expired {
                let Some(active) = role.active_item_effects.remove(&item_id) else {
                    continue;
                };
                if active.effect.max_health != 0 {
                    role.max_health = (role.max_health - active.effect.max_health as f32).max(1.0);
                    role.health = role.health.min(role.max_health);
                }
            }
            if was_alive && role.health <= 0.0 {
                role.deaths = role.deaths.saturating_add(1);
                role.respawn_at_ms =
                    can_respawn(Some(self.rule)).then_some(now_ms.saturating_add(3_000));
                role.active_item_effects.clear();
                role.hit_status_effects.active.clear();
                role.max_health = self.base_max_health;
                role.health = 0.0;
                deaths.push(role.clone());
            }
        }
        for role in deaths {
            let Some(peer) = self.roles.iter().find_map(|(peer, candidate)| {
                (candidate.actor_id == role.actor_id).then_some(*peer)
            }) else {
                continue;
            };
            self.transport.send_application(
                peer,
                STOCK_EVENT_DEATH,
                0u32.to_le_bytes(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
            if self.rule == 2
                && self
                    .escort
                    .is_some_and(|escort| escort.carrier_actor_id == Some(role.actor_id))
            {
                self.drop_escort_from_role(&role, now_ms, events)?;
            }
            if self.rule == 4
                && self
                    .vips
                    .iter()
                    .flatten()
                    .any(|actor_id| *actor_id == role.actor_id)
            {
                self.complete_round(role.camp == 1, role.camp == 2, now_ms, events)?;
            } else {
                self.finish_team_round_if_needed(now_ms, events)?;
            }
        }
        Ok(())
    }

    fn update_escort(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        if self.rule != 2
            || self.round_ended
            || self.roles.is_empty()
            || !self
                .roles
                .keys()
                .all(|peer| self.ready_peers.contains(peer))
        {
            return Ok(());
        }

        let delivery_enabled = self
            .round_started_ms
            .is_some_and(|start| now_ms.saturating_sub(start) >= 3_000);
        let Some(escort) = self.escort else {
            return Ok(());
        };

        if let Some(actor_id) = escort.carrier_actor_id {
            let carrier = self
                .roles
                .values()
                .find(|role| role.actor_id == actor_id)
                .cloned();
            let Some(carrier) = carrier else {
                return self.return_escort_after_disconnect(now_ms, events);
            };
            if carrier.health <= 0.0 {
                return self.drop_escort_from_role(&carrier, now_ms, events);
            }
            let Some(state) = carrier.state else {
                return Ok(());
            };
            if let Some(current) = self.escort.as_mut() {
                current.position = state.position;
            }
            if delivery_enabled
                && position_inside_safe_zone(
                    state.position,
                    carrier.camp as i32,
                    &self.escort_stage_data(),
                )
            {
                self.complete_round(carrier.camp == 2, carrier.camp == 1, now_ms, events)?;
            }
            return Ok(());
        }

        if escort.dropped_at_ms.is_some_and(|dropped_at| {
            now_ms.saturating_sub(dropped_at)
                >= (self.escort_stage_data().return_seconds * 1_000.0) as u64
        }) {
            return self.return_escort_after_timeout(now_ms, events);
        }

        let pickup = self
            .roles
            .values()
            .filter(|role| role.health > 0.0)
            .filter_map(|role| {
                let state = role.state?;
                self.escort_definition
                    .as_ref()
                    .map_or_else(
                        || character_capsule_intersects_flag(state.position, escort.position),
                        |definition| definition.intersects(state.position, escort.position),
                    )
                    .then_some((role.actor_id, state.position))
            })
            .min_by_key(|(actor_id, _)| *actor_id);
        let Some((actor_id, position)) = pickup else {
            return Ok(());
        };
        if let Some(current) = self.escort.as_mut() {
            current.position = position;
            current.carrier_actor_id = Some(actor_id);
            current.dropped_at_ms = None;
        }
        let taken = StockSceneTakenEvent {
            actor_id,
            record_id: escort.record_id,
            category: STOCK_ESCORT_CATEGORY,
            item_id: self.escort_item_id(),
        };
        self.broadcast_discrete(STOCK_EVENT_SCENE_TAKEN, &taken.encode(), now_ms)?;
        events.push(StockGameEvent::EscortTaken(taken));
        Ok(())
    }

    fn drop_escort_from_role(
        &mut self,
        role: &StockHostedRole,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        let Some(previous) = self.escort else {
            return Ok(());
        };
        if previous.carrier_actor_id != Some(role.actor_id) {
            return Ok(());
        }
        let record_id = self.allocate_scene_record_id();
        let escort = StockEscortState {
            record_id,
            position: role.state.map_or(previous.position, |state| state.position),
            carrier_actor_id: None,
            dropped_at_ms: Some(now_ms),
        };
        self.escort = Some(escort);
        let record = escort.scene_record(self.escort_item_id());
        self.broadcast_discrete(STOCK_EVENT_SCENE_SPAWN, &record.encode(), now_ms)?;
        events.push(StockGameEvent::EscortGrounded {
            record,
            dropped: true,
        });
        Ok(())
    }

    fn return_escort_after_timeout(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        let Some(previous) = self.escort else {
            return Ok(());
        };
        let returned = StockFlagReturnedEvent {
            record_id: previous.record_id,
            category: STOCK_ESCORT_CATEGORY,
            item_id: self.escort_item_id(),
            scene_object_id: self
                .escort_definition
                .as_ref()
                .map_or(STOCK_ESCORT_SCENE_OBJECT_ID, |d| d.scene_object_id),
        };
        self.broadcast_discrete(STOCK_EVENT_FLAG_RETURNED, &returned.encode(), now_ms)?;
        let record_id = self.allocate_scene_record_id();
        self.escort = Some(StockEscortState::home(
            record_id,
            self.escort_stage_data().flag_home,
        ));
        events.push(StockGameEvent::EscortReturned);
        Ok(())
    }

    fn return_escort_after_disconnect(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        let removed = StockFlagRemovedEvent {
            scene_object_id: self
                .escort_definition
                .as_ref()
                .map_or(STOCK_ESCORT_SCENE_OBJECT_ID, |d| d.scene_object_id),
        };
        self.broadcast_discrete(STOCK_EVENT_FLAG_REMOVED, &removed.encode(), now_ms)?;
        let record_id = self.allocate_scene_record_id();
        self.escort = Some(StockEscortState::home(
            record_id,
            self.escort_stage_data().flag_home,
        ));
        events.push(StockGameEvent::EscortReturned);
        Ok(())
    }

    fn broadcast_role_entered(
        &mut self,
        event: StockRoleEnteredEvent,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        for peer in self.transport.peer_addresses() {
            self.send_role_entered(peer, event, now_ms)?;
        }
        Ok(())
    }

    fn broadcast_kill(&mut self, event: StockKillEvent, now_ms: u64) -> Result<(), StockGameError> {
        for peer in self.transport.peer_addresses() {
            self.transport.send_application(
                peer,
                STOCK_EVENT_KILL,
                event.encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        Ok(())
    }

    fn assign_vips_if_ready(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        if self.rule != 4 || self.round_ended || self.vips.iter().all(Option::is_some) {
            return Ok(());
        }

        let mut teams = [Vec::new(), Vec::new()];
        for role in self.roles.values().filter(|role| role.health > 0.0) {
            if let Some(team) = role
                .camp
                .checked_sub(1)
                .and_then(|camp| teams.get_mut(camp as usize))
            {
                team.push(role.actor_id);
            }
        }
        teams.iter_mut().for_each(|actors| actors.sort_unstable());

        let actor_mix = teams
            .iter()
            .flatten()
            .fold(0u64, |seed, actor| seed.rotate_left(7) ^ u64::from(*actor));
        let seed =
            now_ms ^ actor_mix ^ u64::from(self.round_count).wrapping_mul(0x9e37_79b9_7f4a_7c15);

        for (index, team) in teams.iter().enumerate() {
            if self.vips[index].is_some() || team.is_empty() {
                continue;
            }
            let actor_id = team[seed.rotate_left(index as u32 * 29) as usize % team.len()];
            self.vips[index] = Some(actor_id);
            let event = StockVipEvent {
                camp: index as u32 + 1,
                actor_id,
            };
            self.broadcast_discrete(STOCK_EVENT_VIP, &event.encode(), now_ms)?;
            events.push(StockGameEvent::Vip(event));
        }
        Ok(())
    }

    fn finish_team_round_if_needed(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        if (1..=4).contains(&self.rule) || self.round_ended {
            return Ok(());
        }
        let team1 = self
            .roles
            .values()
            .filter(|role| role.camp == 1)
            .collect::<Vec<_>>();
        let team2 = self
            .roles
            .values()
            .filter(|role| role.camp == 2)
            .collect::<Vec<_>>();
        let team1_wiped = !team1.is_empty() && team1.iter().all(|role| role.health <= 0.0);
        let team2_wiped = !team2.is_empty() && team2.iter().all(|role| role.health <= 0.0);
        if !team1_wiped && !team2_wiped {
            return Ok(());
        }
        self.complete_round(team1_wiped, team2_wiped, now_ms, events)
    }

    fn finish_round_on_timeout(
        &mut self,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        if self.round_ended || self.roles.is_empty() || self.ready_peers.len() < self.roles.len() {
            return Ok(());
        }
        let Some(started) = self.round_started_ms else {
            self.round_started_ms = Some(now_ms);
            return Ok(());
        };
        let limit_ms = u64::from(self.round_time_minutes)
            .saturating_mul(60)
            .saturating_mul(1_000);
        if now_ms.saturating_sub(started) < limit_ms {
            return Ok(());
        }

        self.complete_round(false, false, now_ms, events)
    }

    pub fn complete_siege_objective(
        &mut self,
        winner_team: i32,
        now_ms: u64,
    ) -> Result<Vec<StockGameEvent>, StockGameError> {
        if self.rule != 3 || self.round_ended {
            return Ok(Vec::new());
        }
        let mut events = Vec::new();
        self.complete_round(winner_team == 2, winner_team == 1, now_ms, &mut events)?;
        Ok(events)
    }

    fn complete_round(
        &mut self,
        team1_wiped: bool,
        team2_wiped: bool,
        now_ms: u64,
        events: &mut Vec<StockGameEvent>,
    ) -> Result<(), StockGameError> {
        if self.round_ended {
            return Ok(());
        }

        if team2_wiped {
            self.team_wins[0] = self.team_wins[0].saturating_add(1);
        }
        if team1_wiped {
            self.team_wins[1] = self.team_wins[1].saturating_add(1);
        }
        self.round_count = self.round_count.saturating_add(1);
        let score = StockTeamScoreEvent {
            team1_wins: self.team_wins[0],
            team2_wins: self.team_wins[1],
        };
        self.broadcast_discrete(STOCK_EVENT_TEAM_SCORE, &score.encode(), now_ms)?;
        events.push(StockGameEvent::TeamScore(score));

        let match_ended = self.round_limit != 9_999 && self.round_count >= self.round_limit;
        let terminal_opcode = if match_ended {
            STOCK_EVENT_MATCH_END
        } else {
            STOCK_EVENT_ROUND_RESTART
        };
        self.broadcast_discrete(terminal_opcode, &[], now_ms)?;
        self.round_ended = true;
        if match_ended {
            events.push(StockGameEvent::MatchEnd);
        } else {
            self.round_restart_at_ms = Some(now_ms.saturating_add(10_000));
            events.push(StockGameEvent::RoundEnd);
        }
        Ok(())
    }

    fn assign_round_spawn_pads(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        let mut available: [Vec<u32>; 3] = self
            .spawn_pad_counts
            .map(|count| (0..count as u32).collect());
        let mut actors: Vec<_> = self
            .roles
            .iter()
            .map(|(peer, role)| (*peer, role.actor_id, role.camp))
            .collect();
        actors.sort_unstable_by_key(|(_, actor, _)| *actor);
        for (peer, _, camp) in actors {
            let group = crate::spawn::group(camp);
            let candidates = &mut available[group as usize];
            let mut payload = Vec::new();
            if let Some(index) = crate::spawn::random_index(candidates.len()) {
                payload.extend_from_slice(&group.to_le_bytes());
                payload.extend_from_slice(&candidates.remove(index).to_le_bytes());
            }
            self.transport.send_application(
                peer,
                crate::stock_protocol::STOCK_EVENT_SPAWN_PAD,
                payload,
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        Ok(())
    }

    fn reset_roles_for_round(&mut self, now_ms: u64) {
        self.scene_state.clear();
        self.hazards
            .build_from_stage(self.hazards.volumes.clone(), self.hazards.machines.clone());
        self.hazard_tick_ms = Some(now_ms);
        for role in self.roles.values_mut() {
            role.active_item_effects.clear();
            role.hit_status_effects.active.clear();
            role.max_health = self.base_max_health;
            role.health = self.base_max_health;
            role.anger = 0.0;
            role.last_hit_ms = None;
            role.respawn_at_ms = None;
            role.spawned_at_ms = now_ms;
            role.state = None;
            role.dirty = false;
            role.next_cure_at_ms = 0;
        }
        self.vips = [None, None];
        self.last_xstar = None;
        self.scene_items.retain(|_, item| item.authored);
        for item in self.scene_items.values_mut() {
            item.active = item.seed.active;
            item.script_active = item.seed.active;
            item.interactive = item.seed.interactive;
            item.respawn_at_ms = None;
        }
        if self.rule == 2 {
            let record_id = self.allocate_scene_record_id();
            self.escort = Some(StockEscortState::home(
                record_id,
                self.escort_stage_data().flag_home,
            ));
        }
        self.last_vitals_downlink_ms = None;
        self.round_started_ms = Some(now_ms);
    }

    fn broadcast_discrete(
        &mut self,
        opcode: u16,
        payload: &[u8],
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.broadcast_with_flags(opcode, payload, STOCK_DISCRETE_RELIABLE_FLAGS, now_ms)
    }

    fn broadcast_with_flags(
        &mut self,
        opcode: u16,
        payload: &[u8],
        flags: u8,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        for peer in self.transport.peer_addresses() {
            self.transport
                .send_application(peer, opcode, payload, flags, now_ms)?;
        }
        Ok(())
    }

    fn broadcast_discrete_except(
        &mut self,
        excluded: SocketAddr,
        opcode: u16,
        payload: &[u8],
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        for peer in self
            .transport
            .peer_addresses()
            .into_iter()
            .filter(|peer| *peer != excluded)
        {
            self.transport.send_application(
                peer,
                opcode,
                payload,
                STOCK_DISCRETE_RELIABLE_FLAGS,
                now_ms,
            )?;
        }
        Ok(())
    }

    fn send_role_entered(
        &mut self,
        peer: SocketAddr,
        event: StockRoleEnteredEvent,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.transport.send_application(
            peer,
            STOCK_EVENT_ROLE_ENTERED,
            event.encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        Ok(())
    }

    fn broadcast_weapon_equipped(
        &mut self,
        event: StockWeaponEquippedEvent,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        for peer in self.transport.peer_addresses() {
            self.send_weapon_equipped(peer, event, now_ms)?;
        }
        Ok(())
    }

    fn send_weapon_equipped(
        &mut self,
        peer: SocketAddr,
        event: StockWeaponEquippedEvent,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.transport.send_application(
            peer,
            STOCK_EVENT_WEAPON_EQUIPPED,
            event.encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        Ok(())
    }

    fn send_vip(
        &mut self,
        peer: SocketAddr,
        event: StockVipEvent,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.transport.send_application(
            peer,
            STOCK_EVENT_VIP,
            event.encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now_ms,
        )?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StockGameError {
    #[error("stock transport error: {0}")]
    Transport(#[from] StockTransportError),
    #[error("stock protocol error: {0}")]
    Protocol(#[from] StockProtocolError),
    #[error("unable to load stock role resource: {0}")]
    Io(#[from] io::Error),
    #[error("stock role has not entered the room")]
    RoleNotEntered,
    #[error("MPK stage checksum is missing: {0}")]
    MissingStageChecksum(String),
    #[error("stock base weapon id is invalid: {0}")]
    InvalidWeapon(u8),
    #[error("stock scene item key is invalid: category={category}, item={item_id}")]
    InvalidSceneItem { category: u32, item_id: u32 },
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_state::{pack_state_word0, RoleVec3, StateWords};
    use crate::stock_udp::StockConnectRecord;

    #[test]
    fn stock_pickup_reaches_head_without_reaching_the_floor_above() {
        let item = StockHostedSceneItem {
            seed: StockAuthoredSceneItem {
                item_id: 9,
                category: 2,
                position: RoleVec3 {
                    x: 0.0,
                    y: 4.0,
                    z: 0.0,
                },
                half_extents: [0.25; 3],
                respawn_ms: None,
                repeat_on_xstar: false,
                scene_reference: None,
                active: true,
                interactive: true,
            },
            record_id: 1,
            active: true,
            script_active: true,
            interactive: true,
            respawn_at_ms: None,
            expires_at_ms: None,
            authored: true,
        };
        assert!(role_touches_scene_item(RoleVec3::ZERO, &item));
        assert!(!role_touches_scene_item(
            RoleVec3 {
                x: 0.0,
                y: -1.0,
                z: 0.0
            },
            &item
        ));
        assert!(!role_touches_scene_item(
            RoleVec3 {
                x: 0.0,
                y: 7.0,
                z: 0.0
            },
            &item
        ));
        assert!(!role_touches_scene_item(
            RoleVec3 {
                x: 3.0,
                y: 0.0,
                z: 0.0
            },
            &item
        ));
    }

    fn stage_checksum_room(expected: u32, actual: u32) -> (StockGameServer, StockGameClient) {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "stage-checksum").unwrap();
        let address = transport.local_addr().unwrap();
        let mut catalog = StockHeroCatalog::bundled_revision();
        catalog.stage_checksums = Some([("sn01".into(), expected), ("sn02".into(), 123)].into());
        let mut server = StockGameServer::new(transport, catalog);
        server.set_stage_id("sn01");
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        client.stage_checksums = Some([("sn01".into(), actual), ("sn02".into(), 123)].into());
        (server, client)
    }

    #[test]
    fn invalid_or_unentered_requests_do_not_stop_the_host() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "validation").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut client =
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap();
        for now in 0..100 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            client.tick(now).unwrap();
            server.tick(now).unwrap();
        }
        assert!(server.roles.is_empty());
        let state = crate::role_state::encode_request1_state(sample_state(12.0, 1));
        for (opcode, payload) in [
            (STOCK_REQUEST_ROLE_STATE, state.to_vec()),
            (STOCK_REQUEST_ROLE_STATE, vec![0]),
            (STOCK_REQUEST_ANGER_FULL, vec![]),
            (STOCK_REQUEST_QI_BURST, vec![]),
            (STOCK_REQUEST_ANGER_COST, vec![0]),
            (STOCK_REQUEST_ANGER_COST, 10u32.to_le_bytes().to_vec()),
            (STOCK_REQUEST_ENTER_ROLE, vec![0]),
            (27, vec![]),
        ] {
            client
                .send_application(opcode, payload, STOCK_DISCRETE_RELIABLE_FLAGS, 101)
                .unwrap();
        }
        let mut replied = false;
        for now in 101..140 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            server.tick(now).unwrap();
            replied |= client.tick(now).unwrap().iter().any(|event|
                matches!(event, StockSessionEvent::Application { packet, .. } if packet.opcode == 52));
        }
        assert!(
            replied,
            "valid requests after malformed requests must still run"
        );
        let request = StockEnterRoleRequest {
            hero: 0,
            camp: 0,
            hero_checksum: server.heroes.checksum(0).unwrap(),
        };
        client
            .send_application(
                STOCK_REQUEST_ENTER_ROLE,
                request.encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                141,
            )
            .unwrap();
        client
            .send_application(STOCK_REQUEST_ROLE_STATE, state, 0, 141)
            .unwrap();
        for now in 141..180 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            server.tick(now).unwrap();
            client.tick(now).unwrap();
        }
        assert_eq!(server.roles.len(), 1);
        assert_eq!(
            server
                .roles
                .values()
                .next()
                .unwrap()
                .state
                .unwrap()
                .position
                .x,
            12.0
        );
    }

    #[test]
    fn stock_anger_debits_apply_after_death_and_round_end_with_full_warning() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "anger-boundary").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        drive(&mut server, &mut [&mut client], 0, 100);
        let peer = server.transport.peer_addresses()[0];
        let actor = server.roles[&peer].actor_id;
        server.round_ended = true;
        server.roles.get_mut(&peer).unwrap().health = 0.0;
        for (spend, initial, expected) in [
            (StockAngerSpend::Burst, 59.0, 0.0),
            (StockAngerSpend::Cost(40), 20.0, 0.0),
            (StockAngerSpend::Cost(10), 80.0, 70.0),
            (StockAngerSpend::Full, 99.0, 0.0),
        ] {
            server.roles.get_mut(&peer).unwrap().anger = initial;
            server
                .handle_application(peer, spend.opcode(), &spend.encode(), 101, &mut Vec::new())
                .unwrap();
            assert_eq!(server.roles[&peer].anger, expected);
        }
        let mut warned = false;
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            warned |= client.transport.tick(102).unwrap().iter().any(|event| {
                matches!(event, StockSessionEvent::Application { packet, .. }
                    if packet.opcode == crate::stock_protocol::STOCK_EVENT_ANGER_INSUFFICIENT
                    && packet.payload == actor.to_le_bytes() && !packet.reliable())
            });
            if warned {
                break;
            }
        }
        assert!(warned);
    }

    #[test]
    fn assassin_assigns_each_camp_independently_before_ready_and_on_late_entry() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "vips").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::bundled_revision(),
            4,
            CombatResourceTable::default(),
        );
        let mut clients = Vec::new();
        for camp in [1, 1, 2] {
            let mut client = StockClientTransport::bind(
                "127.0.0.1:0",
                address,
                StockConnectRecord::for_open_room(),
            )
            .unwrap();
            let start = clients.len() as u64 * 200;
            for now in start..start + 100 {
                std::thread::sleep(std::time::Duration::from_micros(50));
                client.tick(now).unwrap();
                server.tick(now).unwrap();
            }
            let entry = StockEnterRoleRequest {
                hero: 0,
                camp,
                hero_checksum: server.heroes.checksum(0).unwrap(),
            };
            client
                .send_application(
                    STOCK_REQUEST_ENTER_ROLE,
                    entry.encode(),
                    STOCK_DISCRETE_RELIABLE_FLAGS,
                    start + 100,
                )
                .unwrap();
            for now in start + 100..start + 120 {
                std::thread::sleep(std::time::Duration::from_micros(50));
                server.tick(now).unwrap();
                client.tick(now).unwrap();
            }
            clients.push(client);
            assert!(server.ready_peers.is_empty());
            assert_eq!(server.vips[0], clients[0].transport_id());
            assert_eq!(
                server.vips[1],
                if clients.len() == 3 {
                    clients[2].transport_id()
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn deathmatch_strategy_settles_a_single_nonempty_camp_for_default_rules() {
        for rule in [5, 6, 0, 10] {
            let transport =
                StockServerTransport::bind("127.0.0.1:0", "serverlist", "single-camp").unwrap();
            let address = transport.local_addr().unwrap();
            let mut server = StockGameServer::with_rule_and_combat_resources(
                transport,
                StockHeroCatalog::bundled_revision(),
                rule,
                CombatResourceTable::default(),
            );
            let mut client = StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                0,
                1,
                6,
            )
            .unwrap();
            drive(&mut server, &mut [&mut client], 0, 100);
            assert_eq!(server.rule(), rule);
            server.roles.values_mut().next().unwrap().health = 0.0;
            let mut events = Vec::new();
            server
                .finish_team_round_if_needed(101, &mut events)
                .unwrap();
            assert!(server.round_ended);
            assert_eq!(server.team_wins(), [0, 1]);
        }
    }

    #[test]
    fn stock_camp_limit_is_a_hint_and_does_not_rewrite_or_reject_entry() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "camps").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut clients = Vec::new();
        for index in 0..10u64 {
            let mut client = StockClientTransport::bind(
                "127.0.0.1:0",
                address,
                StockConnectRecord::for_open_room(),
            )
            .unwrap();
            let start = index * 200;
            for now in start..start + 100 {
                std::thread::sleep(std::time::Duration::from_micros(50));
                client.tick(now).unwrap();
                server.tick(now).unwrap();
            }
            let camp = if index < 9 { 1 } else { 7 };
            let request = StockEnterRoleRequest {
                hero: 0,
                camp,
                hero_checksum: server.heroes.checksum(0).unwrap(),
            };
            client
                .send_application(
                    STOCK_REQUEST_ENTER_ROLE,
                    request.encode(),
                    STOCK_DISCRETE_RELIABLE_FLAGS,
                    start + 100,
                )
                .unwrap();
            let mut suggested = false;
            for now in start + 101..start + 140 {
                std::thread::sleep(std::time::Duration::from_micros(50));
                server.tick(now).unwrap();
                suggested |= client.tick(now).unwrap().iter().any(|event| {
                    matches!(event, StockSessionEvent::Application { packet, .. }
                        if packet.opcode == crate::stock_protocol::STOCK_EVENT_CAMP_SUGGESTION
                        && packet.payload == 2u32.to_le_bytes())
                });
            }
            assert_eq!(suggested, index == 8);
            assert_eq!(
                server
                    .roles
                    .values()
                    .find(|role| Some(role.actor_id) == client.transport_id())
                    .unwrap()
                    .camp,
                camp
            );
            clients.push(client);
        }
        assert_eq!(server.roles.len(), 10);
    }

    #[test]
    fn siege_callback_accepts_neutral_winner_on_a_custom_map() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "draw").unwrap();
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::bundled_revision(),
            3,
            CombatResourceTable::default(),
        );
        server.set_stage_id("custom_siege");
        let events = server.complete_siege_objective(0, 0).unwrap();
        assert!(events
            .iter()
            .any(|event| matches!(event, StockGameEvent::RoundEnd)));
        assert_eq!(server.team_wins(), [0, 0]);
    }

    #[test]
    fn stage_checksum_rejection_prevents_queued_role_entry() {
        let (mut server, mut client) = stage_checksum_room(42, 43);
        let events = drive(&mut server, &mut [&mut client], 0, 300);
        assert!(events[0]
            .iter()
            .any(|e| matches!(e, StockGameEvent::StageChecksumRejected(r)
            if Some(r.actor_id) == client.transport_id())));
        assert!(events[0]
            .iter()
            .any(|e| matches!(e, StockGameEvent::PeerDisconnected { reason: 0x80, .. })));
        assert!(server.roles.is_empty());
        assert!(server.transport.peer_addresses().is_empty());
        assert!(server.ready_peers.is_empty());
    }

    #[test]
    fn stage_checksum_success_and_late_rejection_remove_existing_role() {
        let (mut server, mut client) = stage_checksum_room(42, 42);
        drive(&mut server, &mut [&mut client], 0, 300);
        assert_eq!(server.roles.len(), 1);
        client
            .transport
            .send_application(
                STOCK_REQUEST_STAGE_CHECKSUM,
                43u32.to_le_bytes(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                301,
            )
            .unwrap();
        let events = drive(&mut server, &mut [&mut client], 301, 300);
        assert!(events[0]
            .iter()
            .any(|e| matches!(e, StockGameEvent::StageChecksumRejected(_))));
        assert!(server.roles.is_empty());
        assert!(server.ready_peers.is_empty());
        assert!(server.remote_vitals_peers.is_empty());
        assert!(server.player_name_peers.is_empty());
    }

    #[test]
    fn stage_checksum_wire_order_repeated_config_and_stage_change() {
        let (mut server, mut client) = stage_checksum_room(42, 42);
        client.set_remote_vitals_enabled(false);
        client.set_player_names_enabled(false);
        let mut requests = Vec::new();

        for now in 0..300 {
            client.tick(now).unwrap();
            for event in server.transport.tick(now).unwrap() {
                match event {
                    StockSessionEvent::PeerConnected { peer, .. } => {
                        server.send_initial_stage_config(peer, now).unwrap()
                    }
                    StockSessionEvent::Application { packet, .. } => requests.push(packet),
                    _ => {}
                }
            }
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert_eq!(requests.remove(0).opcode, REQUEST_BATTLE_RULES);
        assert_eq!(
            requests.iter().map(|p| p.opcode).collect::<Vec<_>>(),
            [
                STOCK_REQUEST_STAGE_CHECKSUM,
                STOCK_REQUEST_ENTER_ROLE,
                STOCK_REQUEST_READY
            ]
        );
        assert_eq!(requests[0].payload, 42u32.to_le_bytes());
        assert!(requests[0].reliable());
        let peer = server.transport.peer_addresses()[0];
        for (stage, start, expected) in [
            ("SN01", 300, None),
            ("sn02", 600, Some(123)),
            ("SN01", 900, Some(42)),
        ] {
            server
                .transport
                .send_application(
                    peer,
                    STOCK_EVENT_STAGE_CONFIG,
                    StockStageConfigEvent {
                        stage_id: stage.into(),
                        field_a: 0,
                        field_b: 0,
                        field_c: 0,
                    }
                    .encode()
                    .unwrap(),
                    STOCK_DISCRETE_RELIABLE_FLAGS,
                    start,
                )
                .unwrap();
            requests.clear();
            for now in start..start + 300 {
                client.tick(now).unwrap();
                for event in server.transport.tick(now).unwrap() {
                    if let StockSessionEvent::Application { packet, .. } = event {
                        requests.push(packet);
                    }
                }
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
            assert_eq!(requests.len(), usize::from(expected.is_some()));
            if let Some(checksum) = expected {
                assert_eq!(requests[0].opcode, STOCK_REQUEST_STAGE_CHECKSUM);
                assert_eq!(
                    StockStageChecksumRequest::decode(&requests[0].payload)
                        .unwrap()
                        .checksum,
                    checksum
                );
            }
        }
    }

    #[test]
    fn stage_checksum_missing_metadata_never_sends_zero_or_enters() {
        let (mut server, mut client) = stage_checksum_room(42, 42);
        client.stage_checksums.as_mut().unwrap().clear();
        let mut rejected = false;
        for now in 0..300 {
            match client.tick(now) {
                Err(StockGameError::MissingStageChecksum(stage)) => {
                    assert_eq!(stage, "sn01");
                    rejected = true;
                    break;
                }
                result => {
                    result.unwrap();
                }
            }
            server.tick(now).unwrap();
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert!(rejected);
        assert!(!client.enter_sent);
        assert!(server.roles.is_empty());
    }

    #[test]
    fn stage_checksum_success_has_no_ack_and_rejection_broadcast_is_unreliable() {
        let (mut server, mut client) = stage_checksum_room(42, 42);

        client.stage_checksums = None;
        drive(&mut server, &mut [&mut client], 0, 300);
        let peer = client.transport.local_addr().unwrap();
        assert_eq!(server.roles.len(), 1);
        let mut events = Vec::new();
        server
            .handle_application(
                peer,
                STOCK_REQUEST_STAGE_CHECKSUM,
                &42u32.to_le_bytes(),
                301,
                &mut events,
            )
            .unwrap();
        assert!(events.is_empty());
        assert!(!client
            .transport
            .tick(302)
            .unwrap()
            .iter()
            .any(|e| matches!(e, StockSessionEvent::Application { .. })));
        server
            .handle_application(
                peer,
                STOCK_REQUEST_STAGE_CHECKSUM,
                &43u32.to_le_bytes(),
                303,
                &mut events,
            )
            .unwrap();
        let rejected = (0..100)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_micros(50));
                client
                    .transport
                    .tick(304)
                    .unwrap()
                    .into_iter()
                    .find_map(|event| match event {
                        StockSessionEvent::Application { packet, .. }
                            if packet.opcode == STOCK_EVENT_STAGE_CHECKSUM_REJECTED =>
                        {
                            Some(packet)
                        }
                        _ => None,
                    })
            })
            .expect("checksum rejection arrives before disconnect");
        assert!(!rejected.reliable());
        assert_eq!(
            StockStageChecksumRejected::decode(&rejected.payload)
                .unwrap()
                .actor_id,
            client.transport_id().unwrap()
        );
    }

    #[test]
    fn stage_checksum_metadata_matches_all_packed_pst_bytes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
        let repo = meteor_asset::pack_repository::PackRepository::open(&root).unwrap();
        let metadata: serde_json::Value =
            serde_json::from_slice(&repo.read("compatibility/stock.json").unwrap()).unwrap();
        let stages = metadata["stages"].as_array().unwrap();
        assert_eq!(stages.len(), 22);
        for stage in stages {
            let bytes = repo.read(stage["entry"].as_str().unwrap()).unwrap();
            assert_eq!(stage["length"].as_u64(), Some(bytes.len() as u64));
            assert_eq!(
                stage["checksum"].as_u64(),
                Some(u64::from(stock_resource_checksum(&bytes)))
            );
            let id = stage["stage"].as_str().unwrap();
            let original = fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("../../../origin_res/{id}/{id}.pst")),
            )
            .unwrap();
            assert_eq!(
                bytes.as_slice(),
                original.as_slice(),
                "stage PST bytes must not be normalized"
            );
        }
    }

    #[test]
    fn stock_role_pacer_keeps_deadlines_and_only_sends_latest_samples() {
        let mut pacer = StockRoleSendPacer::default();
        pacer.submit(sample_state(1.0, 1));
        assert_eq!(pacer.take_due(0, 20).unwrap().position.x, 1.0);
        pacer.submit(sample_state(2.0, 1));
        assert!(pacer.take_due(49_000, 20).is_none());
        pacer.submit(sample_state(3.0, 1));
        assert_eq!(pacer.take_due(52_000, 20).unwrap().position.x, 3.0);
        pacer.submit(sample_state(4.0, 1));
        assert!(pacer.take_due(99_999, 20).is_none());
        assert!(pacer.take_due(100_000, 20).is_some());
        pacer.submit(sample_state(5.0, 1));
        assert!(pacer.take_due(410_000, 20).is_some());
        assert!(pacer.take_due(410_000, 20).is_none());
        assert!(
            pacer.take_due(450_000, 20).is_none(),
            "no stale state replay during a render stall"
        );
        pacer.submit(sample_state(6.0, 1));
        assert!(pacer.take_due(499_999, 20).is_none());
        assert!(pacer.take_due(500_000, 20).is_some());
    }

    fn hazard_box(name: &str, damage: i32) -> meteor_scene::hazards::HazardVolume {
        meteor_scene::hazards::HazardVolume {
            name: name.into(),
            damage,
            period: 1.0,
            box_: meteor_scene::hazards::Aabb {
                min: [-1.0; 3],
                max: [1.0; 3],
            },
        }
    }

    #[test]
    fn role_entry_downlink_carries_weapon_not_pos_checksum() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "room").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            1,
            0,
            6,
        )
        .unwrap();
        let events = drive(&mut server, &mut [&mut client], 0, 300);
        let entered = events[0]
            .iter()
            .find_map(|event| match event {
                StockGameEvent::RoleEntered(event) => Some(event),
                _ => None,
            })
            .unwrap();
        assert_eq!(&entered.encode()[8..12], &[0, 0, 0, 0]);
        let role = server.roles.values_mut().next().unwrap();
        role.weapon_item_id = Some(6);
        assert_eq!(&role.entered_event().encode()[8..12], &6u32.to_le_bytes());
    }

    #[test]
    fn host_sends_stage_before_client_requests_role_entry() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "room").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        server.set_stage_id("sn07");
        server.rule = 5;

        let mut clients = Vec::new();
        for round in 0..2 {
            clients.push(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
            );
            let mut received = Vec::new();
            for tick in 0..500 {
                let now = round * 500 + tick;
                for (index, client) in clients.iter_mut().enumerate() {
                    for event in client.tick(now).unwrap() {
                        if index == round as usize {
                            if let StockSessionEvent::Application { packet, .. } = event {
                                received.push(packet);
                            }
                        }
                    }
                }
                server.tick(now).unwrap();
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
            let configs: Vec<_> = received
                .iter()
                .filter(|p| p.opcode == STOCK_EVENT_STAGE_CONFIG)
                .collect();
            assert_eq!(
                configs.len(),
                1,
                "initial and late clients each get one config without entering"
            );
            assert_eq!(
                received.iter().map(|p| p.opcode).collect::<Vec<_>>(),
                vec![26, 10, 4, 34, 64, 0]
            );

            let remaining = u32::from(server.round_time_minutes) * 60;
            assert_eq!(
                received[0].payload,
                [remaining, 0]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                StockRoleRate::decode(&received[4].payload).unwrap().hz,
                server.role_rate as f32
            );
            assert!(configs[0].reliable());
            let config = StockStageConfigEvent::decode(&configs[0].payload).unwrap();
            assert_eq!(config.stage_id, "sn07");
            assert_eq!((config.field_a, config.field_b), (5, 7));
            assert_eq!(
                config.field_c, 0x3c,
                "default room admits all four original item groups"
            );
            let roster_index = received
                .iter()
                .position(|p| p.opcode == STOCK_EVENT_ROSTER)
                .unwrap();
            let joined_index = received.iter().position(|p| p.opcode == 0).unwrap();
            assert!(
                received
                    .iter()
                    .position(|p| p.opcode == STOCK_EVENT_STAGE_CONFIG)
                    .unwrap()
                    < roster_index
            );
            assert!(
                roster_index < joined_index,
                "create existing members before the joining member"
            );
            assert!(received[roster_index].reliable() && received[joined_index].reliable());
            let members = decode_stock_roster(&received[roster_index].payload).unwrap();
            assert_eq!(members.len(), round as usize);
            assert!(members.iter().all(|member| !member.entered));
            assert_eq!(received[joined_index].payload[0], round as u8 + 2);
            assert!(server.roles.is_empty());
        }
    }

    #[test]
    fn legacy_pre_entry_request_broadcast_and_late_round_state() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "room").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        server.round_time_minutes = 5;
        server.round_started_ms = Some(0);
        let mut clients: Vec<_> = (0..2)
            .map(|_| {
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap()
            })
            .collect();
        let mut initial = [Vec::new(), Vec::new()];
        for now in 42_000..42_100 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            for (i, client) in clients.iter_mut().enumerate() {
                for event in client.tick(now).unwrap() {
                    if let StockSessionEvent::Application { packet, .. } = event {
                        initial[i].push(packet);
                    }
                }
            }
            server.tick(now).unwrap();
        }
        for packets in &initial {
            let state = packets.iter().find(|p| p.opcode == 26).unwrap();
            assert!(state.reliable());
            assert_eq!(
                state.payload,
                [258u32, 0]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>()
            );
        }
        assert!(server.roles.is_empty());
        let sender = clients[1].local_addr().unwrap();
        let actor = server.transport.peer_transport_id(sender).unwrap();
        clients[1]
            .send_application(27, [], STOCK_DISCRETE_RELIABLE_FLAGS, 42_100)
            .unwrap();
        let mut replies = [Vec::new(), Vec::new()];
        for now in 42_100..42_200 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            server.tick(now).unwrap();
            for (i, client) in clients.iter_mut().enumerate() {
                for event in client.tick(now).unwrap() {
                    if let StockSessionEvent::Application { packet, .. } = event {
                        if packet.opcode == 52 {
                            replies[i].push(packet);
                        }
                    }
                }
            }
        }
        for packets in replies {
            assert_eq!(packets.len(), 1);
            assert!(packets[0].reliable());
            assert_eq!(packets[0].payload, actor.to_le_bytes());
        }
        assert!(
            server.roles.is_empty(),
            "pre-entry requests must not create fighters"
        );

        use crate::stock_protocol::{
            STOCK_SCENE_OP_ACTIVE, STOCK_SCENE_OP_EFFECT, STOCK_SCENE_OP_FRAME, STOCK_SCENE_OP_POSE,
        };
        let op = |kind, value| StockSceneOperation {
            reference: 17,
            kind,
            value,
            extra: 0,
        };
        for operation in [
            op(STOCK_SCENE_OP_ACTIVE, 1),
            op(STOCK_SCENE_OP_FRAME, 25),
            op(STOCK_SCENE_OP_POSE, 2),
            op(STOCK_SCENE_OP_ACTIVE, 0),
            op(STOCK_SCENE_OP_EFFECT, 691),
        ] {
            retain_scene_write(&mut server.scene_state, operation);
        }

        let hidden_mechanism = StockSceneOperation {
            reference: 42,
            kind: crate::stock_protocol::STOCK_SCENE_OP_VISIBLE,
            value: 0,
            extra: 0,
        };
        retain_scene_write(&mut server.scene_state, hidden_mechanism);
        let request = StockEnterRoleRequest {
            hero: 0,
            camp: 0,
            hero_checksum: server.heroes.checksum(0).unwrap(),
        };
        clients[1]
            .send_application(
                STOCK_REQUEST_ENTER_ROLE,
                request.encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                42_200,
            )
            .unwrap();
        let mut entry = [Vec::new(), Vec::new()];
        let mut entered_at = None;
        for now in 42_200..42_300 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            let server_events = server.tick(now).unwrap();
            if server_events
                .iter()
                .any(|event| matches!(event, StockGameEvent::RoleEntered(_)))
            {
                entered_at = Some(now as u32);
            }
            for (i, client) in clients.iter_mut().enumerate() {
                for event in client.tick(now).unwrap() {
                    if let StockSessionEvent::Application { packet, .. } = event {
                        entry[i].push(packet);
                    }
                }
            }
        }
        assert!(
            !entry[0].iter().any(|p| p.opcode == 48),
            "joining must not reset another peer's scene barrier"
        );
        assert!(!entry[0].iter().any(|p| p.opcode == STOCK_EVENT_SCENE_OPS));
        let init_index = entry[1].iter().position(|p| p.opcode == 48).unwrap();
        let scene_index = entry[1]
            .iter()
            .position(|p| p.opcode == STOCK_EVENT_SCENE_OPS)
            .unwrap();
        assert!(
            scene_index < init_index,
            "scene fields must arrive before the join barrier"
        );
        assert!(entry[1][scene_index].reliable());
        assert_eq!(
            StockSceneOpsEvent::decode(&entry[1][scene_index].payload)
                .unwrap()
                .operations,
            vec![
                op(STOCK_SCENE_OP_POSE, 2),
                op(STOCK_SCENE_OP_ACTIVE, 0),
                hidden_mechanism
            ]
        );
        assert!(
            entry[1][..init_index].iter().any(|p| p.opcode == 9),
            "create roles before releasing the join barrier"
        );
        let init = &entry[1][init_index];
        assert!(init.reliable());
        let init = StockSceneStateEvent::decode(&init.payload).unwrap();
        assert!(init.objects.is_empty() && Some(init.timestamp_ms) == entered_at);
        let replay = entry[1][..init_index]
            .iter()
            .find(|p| p.opcode == STOCK_EVENT_SCENE_OPS)
            .expect("restore scene before releasing the join barrier");
        assert!(replay.reliable());
        let replay = StockSceneOpsEvent::decode(&replay.payload).unwrap();
        assert_eq!(
            replay.operations,
            [
                op(STOCK_SCENE_OP_POSE, 2),
                op(STOCK_SCENE_OP_ACTIVE, 0),
                hidden_mechanism
            ]
        );
        assert_eq!(entry[1].iter().filter(|p| p.opcode == 48).count(), 1);
    }

    #[test]
    fn environment_udp_world_boundary_is_authoritative_after_spawn_gate() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "bounds").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        drive(&mut server, &mut [&mut client], 0, 100);
        client.send_role_state(sample_state(10.0, 1), 100).unwrap();
        let mut geometry = meteor_scene::hazards::StageHazards::new();
        geometry.world_bounds = Some(meteor_scene::hazards::Aabb {
            min: [-5.0; 3],
            max: [5.0; 3],
        });
        server.replace_environment_geometry(geometry);
        let protected = drive(&mut server, &mut [&mut client], 100, 100);
        assert!(!protected[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::LocalDeath)));
        let died = drive(&mut server, &mut [&mut client], 3200, 100);
        assert!(died[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::LocalDeath)));
        assert!(died[0].iter().any(|event| {
            matches!(event, StockGameEvent::EnvironmentDeath(id) if *id == client.actor_id().unwrap())
        }));
        assert!(server
            .roles
            .values()
            .all(|role| role.deaths == 1 && role.kills == 0));
    }

    #[test]
    fn environment_udp_protection_vitals_observer_death_and_respawn() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "hazards").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut clients = (0..3)
            .map(|hero| {
                StockGameClient::for_bundled_revision(
                    StockClientTransport::bind(
                        "127.0.0.1:0",
                        address,
                        StockConnectRecord::for_open_room(),
                    )
                    .unwrap(),
                    hero,
                    0,
                    6,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            0,
            100,
        );
        for (i, client) in clients.iter_mut().enumerate() {
            client
                .send_role_state(sample_state(if i == 1 { 10.0 } else { 0.0 }, 1), 100)
                .unwrap();
        }
        let mut geometry = meteor_scene::hazards::StageHazards::new();
        geometry.build_from_stage(vec![hazard_box("damage30", 30)], vec![]);
        geometry.world_bounds = Some(meteor_scene::hazards::Aabb {
            min: [-100.0; 3],
            max: [100.0; 3],
        });
        server.replace_environment_geometry(geometry.clone());
        assert_eq!(server.hazards.world_bounds.unwrap().min, [-100.0; 3]);
        drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            100,
            100,
        );
        assert!(
            server
                .roles
                .values()
                .all(|role| role.health == role.max_health),
            "spawn gate"
        );
        for role in server.roles.values_mut() {
            role.health = 60.0;
        }

        server.replace_environment_geometry(geometry.clone());
        let hit = drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            3200,
            100,
        );
        for i in [0, 2] {
            assert!(hit[i]
                .iter()
                .any(|e| matches!(e, StockGameEvent::Vitals(v) if v.health == 30)));
            assert!(hit[i].iter().any(
                |e| matches!(e, StockGameEvent::HitReaction { event, .. } if event.pose_id == 90)
            ));
        }
        server.replace_environment_geometry(geometry);
        drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            3300,
            100,
        );
        assert_eq!(
            server.roles.values().filter(|r| r.health == 30.0).count(),
            2
        );
        let died = drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            4300,
            100,
        );
        let victims = [
            clients[0].actor_id().unwrap(),
            clients[2].actor_id().unwrap(),
        ];
        for stream in &died {
            assert!(!stream.iter().any(|e| matches!(e, StockGameEvent::Kill(_))));
            for actor in victims {
                assert_eq!(
                    stream
                        .iter()
                        .filter(
                            |e| matches!(e, StockGameEvent::EnvironmentDeath(id) if *id == actor)
                        )
                        .count(),
                    1
                );
            }
        }
        for i in [0, 2] {
            assert!(died[i]
                .iter()
                .any(|e| matches!(e, StockGameEvent::LocalDeath)));
        }
        assert!(server.roles.values().all(|role| role.kills == 0));
        assert_eq!(server.roles.values().filter(|r| r.deaths == 1).count(), 2);
        let repeat = drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            4800,
            100,
        );
        assert!(repeat
            .iter()
            .flatten()
            .all(|e| !matches!(e, StockGameEvent::EnvironmentDeath(_))));
        server.replace_environment_geometry(Default::default());
        let respawned = drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            7400,
            100,
        );
        assert!(respawned
            .iter()
            .flatten()
            .any(|e| matches!(e, StockGameEvent::Respawn(_))));
        assert!(server.roles.values().all(|role| role.health > 0.0));
        assert!(clients.iter().all(|client| client.dead_actors.is_empty()));
    }

    #[test]
    fn environment_udp_machine_geometry_moves_and_active_removal_stops_damage() {
        use meteor_scene::hazards::{Aabb, HazardMachine, StageHazards};
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "machine").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        drive(&mut server, &mut [&mut client], 0, 100);
        client.send_role_state(sample_state(0.0, 1), 3200).unwrap();
        let mut geometry = StageHazards::new();
        geometry.machines.push(HazardMachine {
            name: "roller".into(),
            damage: true,
            damage_value: 300,
            active: true,
            boxes: vec![Aabb {
                min: [10.0; 3],
                max: [11.0; 3],
            }],
            obbs: vec![],
            collider_owners: vec![],
        });
        server.replace_environment_geometry(geometry.clone());
        drive(&mut server, &mut [&mut client], 3200, 100);
        assert_eq!(server.roles.values().next().unwrap().health, 2000.0);

        geometry.machines[0].boxes = vec![Aabb {
            min: [-1.0, -2.0, -1.0],
            max: [1.0, -0.6, 1.0],
        }];
        server.replace_environment_geometry(geometry.clone());
        drive(&mut server, &mut [&mut client], 3350, 40);
        assert_eq!(server.roles.values().next().unwrap().health, 2000.0);
        geometry.machines[0].boxes = vec![Aabb {
            min: [-1.0; 3],
            max: [1.0; 3],
        }];
        server.replace_environment_geometry(geometry.clone());
        drive(&mut server, &mut [&mut client], 3400, 100);
        assert_eq!(server.roles.values().next().unwrap().health, 1700.0);
        server.replace_environment_geometry(geometry.clone());
        drive(&mut server, &mut [&mut client], 4000, 100);
        assert_eq!(
            server.roles.values().next().unwrap().health,
            1700.0,
            "1s shared environment window survives replacement"
        );
        geometry.machines[0].active = false;
        server.replace_environment_geometry(geometry);
        drive(&mut server, &mut [&mut client], 6000, 100);
        assert_eq!(server.roles.values().next().unwrap().health, 1700.0);
    }

    #[test]
    fn environment_udp_lethal_contact_settles_vip_and_drops_escort() {
        for rule in [2, 4] {
            let transport =
                StockServerTransport::bind("127.0.0.1:0", "serverlist", "environment rule")
                    .unwrap();
            let address = transport.local_addr().unwrap();
            let mut server = StockGameServer::with_rule_and_combat_resources(
                transport,
                StockHeroCatalog::bundled_revision(),
                rule,
                Default::default(),
            );
            server.round_limit = 1;
            let mut a = StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                0,
                1,
                6,
            )
            .unwrap();
            let mut b = StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                1,
                2,
                6,
            )
            .unwrap();
            drive(&mut server, &mut [&mut a, &mut b], 0, 100);
            a.send_role_state(sample_state(0.0, 1), 2200).unwrap();
            b.send_role_state(sample_state(10.0, 1), 2200).unwrap();
            if rule == 2 {
                server.escort.as_mut().unwrap().carrier_actor_id = a.actor_id();
            }
            let mut geometry = meteor_scene::hazards::StageHazards::new();
            geometry.volumes.push(hazard_box("hell", 100000));
            server.replace_environment_geometry(geometry);

            let protected = drive(&mut server, &mut [&mut a, &mut b], 2200, 100);
            assert!(server.roles.values().all(|role| role.health > 0.0));
            assert!(protected
                .iter()
                .flatten()
                .all(|event| !matches!(event, StockGameEvent::EnvironmentDeath(_))));
            let events = drive(&mut server, &mut [&mut a, &mut b], 3200, 100);
            if rule == 4 {
                assert!(server.round_ended);
                assert_eq!(server.team_wins, [0, 1]);
                assert!(events
                    .iter()
                    .flatten()
                    .any(|e| matches!(e, StockGameEvent::MatchEnd)));
            } else {
                assert!(
                    !server.round_ended,
                    "escort drop alone does not decide the round"
                );
            }
            assert!(!events
                .iter()
                .flatten()
                .any(|e| matches!(e, StockGameEvent::Kill(_))));
            if rule == 2 {
                assert!(
                    events
                        .iter()
                        .flatten()
                        .any(|e| matches!(e, StockGameEvent::EscortGrounded { dropped: true, .. })),
                    "roles={:?}, carrier={:?}, client actors={:?}, received={:?}",
                    server
                        .roles
                        .values()
                        .map(|role| (role.actor_id, role.health, role.state))
                        .collect::<Vec<_>>(),
                    server.escort,
                    [a.actor_id(), b.actor_id()],
                    events
                );
            }
        }
    }

    #[test]
    fn stock_anger_spend_udp_is_per_actor_deduplicated_and_survives_vitals() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "anger").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut player = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        let mut npc = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            1,
            0,
            6,
        )
        .unwrap();
        drive(&mut server, &mut [&mut player, &mut npc], 0, 100);
        let mut now = 200;
        for (index, spend) in [
            StockAngerSpend::Cost(10),
            StockAngerSpend::Full,
            StockAngerSpend::Burst,
        ]
        .into_iter()
        .enumerate()
        {
            for role in server.roles.values_mut() {
                role.anger = 100.0;
            }
            drive(&mut server, &mut [&mut player, &mut npc], now, 100);
            now += 200;

            server.tick(now).unwrap();
            let key = (index as u32 + 1, 85 + index as u16);
            for client in [&mut player, &mut npc] {
                client.send_anger_spend(key, spend, now).unwrap();
                client.send_anger_spend(key, spend, now).unwrap();
                let old = client.tick(now + 1).unwrap();
                assert!(
                    old.iter()
                        .filter_map(|event| if let StockGameEvent::Vitals(v) = event {
                            Some(v.anger)
                        } else {
                            None
                        })
                        .all(|anger| anger == spend.remaining(100.0) as u16),
                    "old pre-ACK vitals must not refill predicted Qi"
                );
            }
            let events = drive(&mut server, &mut [&mut player, &mut npc], now + 2, 3_100);
            for stream in events {
                let anger: Vec<_> = stream
                    .iter()
                    .filter_map(|event| {
                        if let StockGameEvent::Vitals(v) = event {
                            Some(v.anger)
                        } else {
                            None
                        }
                    })
                    .collect();
                assert!(!anger.is_empty());
                assert!(anger
                    .iter()
                    .all(|anger| *anger == spend.remaining(100.0) as u16));
            }
            for role in server.roles.values() {
                assert_eq!(role.anger, spend.remaining(100.0));
            }
            assert!(player.pending_anger_spends.is_empty() && npc.pending_anger_spends.is_empty());
            now += 3_300;
        }
        for role in server.roles.values_mut() {
            role.anger = 59.0;
        }
        npc.send_anger_spend((99, 85), StockAngerSpend::Burst, now)
            .unwrap();
        npc.send_anger_spend((100, 141), StockAngerSpend::Cost(u32::MAX), now)
            .unwrap();
        drive(&mut server, &mut [&mut player, &mut npc], now, 100);
        assert_eq!(
            server
                .roles
                .values()
                .find(|role| Some(role.actor_id) == npc.actor_id())
                .unwrap()
                .anger,
            0.0,
            "an insufficient debit clamps to zero and a high-bit cost never grants Qi"
        );
        assert_eq!(
            server
                .roles
                .values()
                .find(|role| Some(role.actor_id) == player.actor_id())
                .unwrap()
                .anger,
            59.0
        );
    }

    #[test]
    fn stock_anger_ack_sequence_wrap_matches_transport_ring() {
        assert!(stock_sequence_reached(1, 60_000));
        assert!(stock_sequence_reached(17, 17));
        assert!(!stock_sequence_reached(16, 17));
        assert!(!stock_sequence_reached(60_000, 1));
        assert!(!stock_sequence_reached(0, 60_000));
    }

    #[test]
    fn stock_entry_preserves_transport_address_for_equipment_and_state() {
        let mut server = StockServerTransport::bind("127.0.0.1:0", "serverlist", "test").unwrap();
        let transport = StockClientTransport::bind(
            "127.0.0.1:0",
            server.local_addr().unwrap(),
            StockConnectRecord::for_open_room(),
        )
        .unwrap();
        let connect_address = transport.local_addr().unwrap().port();
        let mut client = StockGameClient::for_bundled_revision(transport, 4, 0, 6).unwrap();
        let mut equipment_seen = false;
        let mut state_seen = false;
        for now in 0..500 {
            client.tick(now).unwrap();
            for event in server.tick(now).unwrap() {
                if let StockSessionEvent::Application { peer, packet } = event {
                    assert_eq!(
                        packet.logical_channel, connect_address,
                        "actor id must never replace the connect address"
                    );
                    if packet.opcode == STOCK_REQUEST_ENTER_ROLE {
                        let actor_id = server.peer_transport_id(peer).unwrap();
                        server
                            .send_application(
                                peer,
                                STOCK_EVENT_ROLE_ENTERED,
                                StockRoleEnteredEvent {
                                    actor_id,
                                    hero: 4,
                                    hero_checksum: 0,
                                    camp: 0,
                                }
                                .encode(),
                                STOCK_DISCRETE_RELIABLE_FLAGS,
                                now,
                            )
                            .unwrap();
                    }
                    equipment_seen |= packet.opcode == STOCK_REQUEST_EQUIP_WEAPON;
                    state_seen |= packet.opcode == STOCK_REQUEST_ROLE_STATE;
                }
            }
            if client.actor_id().is_some() {
                client.send_role_state(sample_state(1.0, 1), now).unwrap();
            }
            if equipment_seen && state_seen {
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
        assert!(equipment_seen && state_seen);
    }

    #[test]
    fn stock_pose_chain_request_reaches_udp_with_original_command27_payload() {
        use crate::stock_protocol::{StockPoseChainRequest, STOCK_REQUEST_POSE_CHAIN};
        let mut server = StockServerTransport::bind("127.0.0.1:0", "serverlist", "test").unwrap();
        let transport = StockClientTransport::bind(
            "127.0.0.1:0",
            server.local_addr().unwrap(),
            StockConnectRecord::for_open_room(),
        )
        .unwrap();
        let mut client = StockGameClient::for_bundled_revision(transport, 4, 0, 6).unwrap();
        let request = StockPoseChainRequest {
            pose_id: 253,
            sequence: 711,
            blend_seconds: 0.3,
        };
        assert!(
            client.send_pose_chain(request, 0).is_err(),
            "cannot send before role entry"
        );
        let mut sent = false;
        let mut received = false;
        for now in 0..500 {
            client.tick(now).unwrap();
            for event in server.tick(now).unwrap() {
                if let StockSessionEvent::Application { peer, packet } = event {
                    if packet.opcode == STOCK_REQUEST_ENTER_ROLE {
                        let actor_id = server.peer_transport_id(peer).unwrap();
                        server
                            .send_application(
                                peer,
                                STOCK_EVENT_ROLE_ENTERED,
                                StockRoleEnteredEvent {
                                    actor_id,
                                    hero: 4,
                                    hero_checksum: 0,
                                    camp: 0,
                                }
                                .encode(),
                                STOCK_DISCRETE_RELIABLE_FLAGS,
                                now,
                            )
                            .unwrap();
                    }
                    if packet.opcode == STOCK_REQUEST_POSE_CHAIN {
                        assert!(!received, "transport retries must not redeliver the entry");
                        assert_eq!(packet.payload, [253, 0, 0, 0, 199, 2, 30, 0]);
                        received = true;
                    }
                }
            }
            if client.actor_id().is_some() && !sent {
                client.send_pose_chain(request, now).unwrap();
                sent = true;
            }
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
        assert!(sent && received);
        assert_eq!(
            StockPoseChainRequest {
                blend_seconds: 0.309,
                ..request
            }
            .encode()[6..],
            [30, 0]
        );
    }

    #[test]
    fn hosted_pose_chain_relays_exact_words_to_other_peers_in_both_directions() {
        use crate::stock_protocol::{
            StockPoseChainRequest, STOCK_EVENT_POSE_CHAIN, STOCK_REQUEST_POSE_CHAIN,
        };
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "chains").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());

        let mut clients = (0..3)
            .map(|hero| {
                StockGameClient::for_bundled_revision(
                    StockClientTransport::bind(
                        "127.0.0.1:0",
                        address,
                        StockConnectRecord::for_open_room(),
                    )
                    .unwrap(),
                    hero,
                    0,
                    6,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            0,
            100,
        );
        let mut now = 100;
        for sender in 0..3 {
            for sequence in [u16::MAX, 0, 1] {
                let mut payload = StockPoseChainRequest {
                    pose_id: 253,
                    sequence,
                    blend_seconds: 0.3,
                }
                .encode();
                payload[2..4].copy_from_slice(&7_u16.to_le_bytes());
                payload[6..8].copy_from_slice(&29_u16.to_le_bytes());
                let actor = clients[sender].actor_id().unwrap() as u16;
                let mut expected = actor.to_le_bytes().to_vec();
                expected.extend_from_slice(&payload);
                clients[sender]
                    .transport_mut()
                    .send_application(STOCK_REQUEST_POSE_CHAIN, payload, 0, now)
                    .unwrap();
                let mut received = [0; 3];
                let mut hosted_events = 0;
                for tick in now..now + 30 {
                    for event in server.tick(tick).unwrap() {
                        if let StockGameEvent::PoseChain(event) = event {
                            assert_eq!(
                                event,
                                crate::stock_protocol::StockPoseChainEvent::decode(&expected)
                                    .unwrap()
                            );
                            hosted_events += 1;
                        }
                    }
                    for (index, client) in clients.iter_mut().enumerate() {
                        for event in client.transport_mut().tick(tick).unwrap() {
                            if let StockSessionEvent::Application { packet, .. } = event {
                                if packet.opcode == STOCK_EVENT_POSE_CHAIN {
                                    assert_eq!(packet.payload, expected);

                                    assert_eq!(
                                        packet.flags & crate::stock_udp::STOCK_FLAG_RELIABLE,
                                        0
                                    );
                                    assert_ne!(
                                        packet.flags & crate::stock_udp::STOCK_FLAG_APPLICATION,
                                        0
                                    );
                                    received[index] += 1;
                                }
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_micros(50));
                }
                assert_eq!(hosted_events, 1);
                for (index, count) in received.into_iter().enumerate() {
                    assert_eq!(
                        count,
                        usize::from(index != sender),
                        "no echo or duplicate replay"
                    );
                }
                now += 30;
            }
        }

        clients[1]
            .send_pose_chain(
                StockPoseChainRequest {
                    pose_id: 249,
                    sequence: 2,
                    blend_seconds: 0.15,
                },
                now,
            )
            .unwrap();
        let output = drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            now,
            30,
        );
        for (index, events) in output.iter().enumerate() {
            let entries: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    StockGameEvent::PoseChain(event) => Some(event),
                    _ => None,
                })
                .collect();
            assert_eq!(entries.len(), usize::from(index != 1));
            for event in entries {
                assert_eq!(event.actor_id, clients[1].actor_id().unwrap() as u16);
                assert_eq!((event.pose_id, event.sequence), (249, 2));
                assert!((event.blend_seconds - 0.15).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn hosted_pose_chain_ignores_unentered_and_malformed_requests() {
        use crate::stock_protocol::STOCK_REQUEST_POSE_CHAIN;
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "chains").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut events = Vec::new();
        server
            .handle_application(
                "127.0.0.1:1".parse().unwrap(),
                STOCK_REQUEST_POSE_CHAIN,
                &[0; 8],
                0,
                &mut events,
            )
            .unwrap();
        assert!(events.is_empty());
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        drive(&mut server, &mut [&mut client], 0, 100);
        let peer = *server.roles.keys().next().unwrap();
        for length in [0, 1, 7, 9, 10] {
            server
                .handle_application(
                    peer,
                    STOCK_REQUEST_POSE_CHAIN,
                    &vec![0; length],
                    100,
                    &mut events,
                )
                .unwrap();
        }
        assert!(events.is_empty());
        server
            .handle_application(peer, STOCK_REQUEST_POSE_CHAIN, &[0; 8], 101, &mut events)
            .unwrap();
        assert!(matches!(events.as_slice(), [StockGameEvent::PoseChain(_)]));
    }

    #[test]
    fn player_names_reach_existing_and_late_peers_without_sending_extensions_to_legacy() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "names").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let connect = |name: &str| {
            let mut record = StockConnectRecord::for_open_room();
            record.player_name = name.into();
            StockGameClient::for_bundled_revision(
                StockClientTransport::bind("127.0.0.1:0", address, record).unwrap(),
                0,
                0,
                6,
            )
            .unwrap()
        };
        let mut first = connect("孟星魂3️⃣");
        let mut legacy = connect("原版来客");
        legacy.set_player_names_enabled(false);
        let initial = drive(&mut server, &mut [&mut first, &mut legacy], 0, 300);
        let first_actor = first.actor_id().unwrap();
        assert_eq!(
            server
                .transport
                .peer_player_name(first.transport.local_addr().unwrap()),
            Some("孟星魂3??")
        );
        let legacy_actor = legacy.actor_id().unwrap();
        assert!(initial[0].iter().any(|event| matches!(event, StockGameEvent::PlayerName { actor_id, name } if *actor_id == legacy_actor && name == "原版来客")));
        assert!(!initial[1].iter().any(|event| matches!(
            event,
            StockGameEvent::RawApplication {
                opcode: EVENT_PLAYER_NAME,
                ..
            }
        )));
        let mut late = connect("德玛西亚🦋");
        let joined = drive(
            &mut server,
            &mut [&mut first, &mut legacy, &mut late],
            300,
            300,
        );
        assert!(joined[2].iter().any(|event| matches!(event, StockGameEvent::PlayerName { actor_id, name } if *actor_id == first_actor && name == "孟星魂3️⃣")));
        assert!(joined[0].iter().any(|event| matches!(event, StockGameEvent::PlayerName { actor_id, name } if Some(*actor_id) == late.actor_id() && name == "德玛西亚🦋")));
        assert!(!joined[1].iter().any(|event| matches!(
            event,
            StockGameEvent::RawApplication {
                opcode: EVENT_PLAYER_NAME,
                ..
            }
        )));
        let late_actor = late.actor_id().unwrap();
        let name_index = joined[1].iter().position(|event| matches!(event,
            StockGameEvent::PlayerName { actor_id, name } if *actor_id == late_actor && name == "德玛西亚?")).unwrap();
        let entry_index = joined[1]
            .iter()
            .position(|event| {
                matches!(event,
            StockGameEvent::RoleEntered(entered) if entered.actor_id == late_actor)
            })
            .unwrap();
        assert!(
            name_index < entry_index,
            "event 0 supplies names before role entry without extensions"
        );
        let first_peer = first.transport.local_addr().unwrap();
        for invalid in [
            b"MRN2".to_vec(),
            b"MRN2\xff".to_vec(),
            b"MRN2bad\nname".to_vec(),
            [PLAYER_NAMES_V2.as_slice(), &vec![b'a'; 769]].concat(),
        ] {
            server
                .handle_application(
                    first_peer,
                    REQUEST_PLAYER_NAMES,
                    &invalid,
                    600,
                    &mut Vec::new(),
                )
                .unwrap();
        }
        assert_eq!(
            server
                .unicode_player_names
                .get(&first_peer)
                .map(String::as_str),
            Some("孟星魂3️⃣")
        );

        let roster = vec![
            2, 2, 5, 2, 1, 1, 5, 1, 1, 1, 1, 1, 1, 1, 1, 7, b'u', b'n', b'n', b'a', b'm', b'e', 1,
            1,
        ];
        server
            .transport
            .send_application(
                legacy.transport.local_addr().unwrap(),
                STOCK_EVENT_ROSTER,
                roster,
                STOCK_DISCRETE_RELIABLE_FLAGS,
                600,
            )
            .unwrap();
        let received = drive(
            &mut server,
            &mut [&mut first, &mut legacy, &mut late],
            600,
            100,
        );
        assert!(received[1].iter().any(|event| matches!(event, StockGameEvent::PlayerName { actor_id: 1, name } if name == "unname")));
        let peer = late.transport.local_addr().unwrap();
        late.transport.disconnect(0).unwrap();
        drive(&mut server, &mut [&mut first, &mut legacy], 701, 100);
        assert!(!server.player_name_peers.contains(&peer));
        assert!(!server.unicode_player_names.contains_key(&peer));
    }

    #[test]
    fn player_name_payload_rejects_malformed_and_invalid_text() {
        for payload in [
            vec![],
            b"MRN1".to_vec(),
            b"MRN1\x01\0\0\0".to_vec(),
            b"MRN1\x01\0\0\0\xff".to_vec(),
            b"MRN1\x01\0\0\0\n".to_vec(),
            b"BAD!\x01\0\0\0hello".to_vec(),
            vec![0; 777],
        ] {
            assert!(decode_player_name(&payload).is_none());
        }
    }

    fn sample_state(x: f32, sequence: u16) -> RoleState {
        RoleState {
            position: RoleVec3 { x, y: 0.0, z: 0.0 },
            forward: RoleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            velocity: RoleVec3::ZERO,
            state_words: StateWords {
                state_word0: pack_state_word0(0, true, false, 0),
                state_word1: sequence,
                pose_id: 0,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: sequence,
            },
        }
    }

    fn drive(
        server: &mut StockGameServer,
        clients: &mut [&mut StockGameClient],
        start: u64,
        ticks: u64,
    ) -> Vec<Vec<StockGameEvent>> {
        let mut output = vec![Vec::new(); clients.len()];
        for now in start..start + ticks {
            for (index, client) in clients.iter_mut().enumerate() {
                output[index].extend(client.tick(now).unwrap());
            }
            server.tick(now).unwrap();
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        output
    }

    #[test]
    fn stock_dagger_double_stomp_host_accepts_distinct_client_hits_inside_200ms() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "stomp").unwrap();
        let address = transport.local_addr().unwrap();
        let checksums = [1234; STOCK_HERO_COUNT];
        let table = meteor_asset::res::parse_meteor_res(include_bytes!(
            "../../../../origin_res/Meteor.res"
        ))
        .unwrap()
        .combat_table();
        let mut server = StockGameServer::with_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            table,
        );
        let client = || {
            StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero: 0,
                    camp: 0,
                    hero_checksum: checksums[0],
                },
                1,
            )
        };
        let mut attacker = client();
        let mut defender = client();
        drive(&mut server, &mut [&mut attacker, &mut defender], 0, 100);
        let target_id = defender.actor_id().unwrap();
        let target_peer = *server
            .roles
            .iter()
            .find(|(_, role)| role.actor_id == target_id)
            .unwrap()
            .0;
        let attacker_peer = *server
            .roles
            .iter()
            .find(|(_, role)| role.actor_id == attacker.actor_id().unwrap())
            .unwrap()
            .0;
        server.roles.get_mut(&attacker_peer).unwrap().weapon_item_id = Some(5);
        let hit = |pose| StockHitRequest {
            target_actor_id: target_id,
            hit_word: 256 | (1 << 16),
            target_pose_word: pose,
            moves_word: 150,
            values_word: 0,
        };
        attacker.send_hit(hit(103), 2100).unwrap();
        let first = drive(&mut server, &mut [&mut attacker, &mut defender], 2100, 60);
        assert!(first
            .iter()
            .all(|events| events.iter().any(|event| matches!(event,
                StockGameEvent::HitReaction { guarded: false, event }
                    if event.target_actor_id == target_id && event.pose_id == 103
            ))));
        let hp_after_first = server.roles[&target_peer].health;
        assert!(hp_after_first > 0.0 && hp_after_first < server.roles[&target_peer].max_health);

        attacker.send_hit(hit(105), 2292).unwrap();
        let second = drive(&mut server, &mut [&mut attacker, &mut defender], 2292, 60);
        assert!(second
            .iter()
            .all(|events| events.iter().any(|event| matches!(event,
                StockGameEvent::HitReaction { guarded: false, event }
                    if event.target_actor_id == target_id && event.pose_id == 105
            ))));
        assert!(server.roles[&target_peer].health < hp_after_first);
    }

    #[test]
    fn json_rules_udp_transmits_custom_configuration() {
        use meteor_combat::battle_rules::BattleRules;
        let overrides: Vec<_> = (0..1024)
            .map(|pose_id| {
                let mut value = serde_json::json!({
                    "poseId": pose_id, "damagePercent": 45, "speedPercent": 125,
                    "hitStunMs": 400, "guardStunMs": 250,
                    "superArmor": {"qiCostPercent": 20},
                    "invalidAttack": {"qiCostPercent": 0, "windowMs": 1200},
                });
                if pose_id == 400 || pose_id == 401 {
                    value["invalidAttack"]["groupId"] = 1.into();
                    value["invalidAttack"]["armOnHit"] = true.into();
                }
                value
            })
            .collect();
        let json = serde_json::json!({"schemaVersion": 2, "rebalance": {
            "guardChipPercent": 15, "guardChipPoses": (0..900).collect::<Vec<u16>>(), "poseOverrides": overrides,
        }}).to_string();
        let rules = BattleRules::from_json(&json).unwrap();
        assert_eq!(
            rules.invalid_attack_key(400).unwrap().1,
            rules.invalid_attack_key(401).unwrap().1
        );
        assert!(rules.to_json().len() > crate::stock_file_transfer::CHUNK_BYTES);
        let compressed = compress_battle_rules(rules.clone());
        assert!(compressed.len() > crate::stock_file_transfer::CHUNK_BYTES);
        assert!(compressed.len() < rules.to_json().len());
        assert_eq!(decode_battle_rules(&compressed), Some(rules.clone()));
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "custom").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        server.set_battle_rules(rules.clone());
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            5,
        )
        .unwrap();
        let events = drive(&mut server, &mut [&mut client], 0, 400);
        assert!(events[0]
            .iter()
            .any(|e| matches!(e, StockGameEvent::BattleRules(received) if *received == rules)));
        assert_eq!(server.roles.len(), 1);
        assert_eq!(server.rules_verified.len(), 1);
        let mut late = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            5,
        )
        .unwrap();
        let events = drive(&mut server, &mut [&mut client, &mut late], 400, 400);
        assert!(events[1]
            .iter()
            .any(|e| matches!(e, StockGameEvent::BattleRules(received) if *received == rules)));
        assert_eq!(server.roles.len(), 2);
        assert_eq!(server.rules_verified.len(), 2);
    }

    #[test]
    fn compressed_rules_reject_oversize_and_trailing_data() {
        let valid = compress_battle_rules(meteor_combat::battle_rules::BattleRules::BALANCED);
        let mut trailing = valid.clone();
        trailing.push(0);
        assert!(decode_battle_rules(&trailing).is_none());
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(&vec![
                b' ';
                meteor_combat::battle_rules::MAX_RULES_JSON_BYTES + 1
            ])
            .unwrap();
        assert!(decode_battle_rules(&encoder.finish().unwrap()).is_none());
    }

    #[test]
    fn balance_udp_requires_ack_and_late_join_receives_rules() {
        const BALANCED: meteor_combat::battle_rules::BattleRules =
            meteor_combat::battle_rules::BattleRules::BALANCED;
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "balance").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        server.set_battle_rules(BALANCED);
        let connect = || {
            StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                0,
                0,
                5,
            )
            .unwrap()
        };
        let mut first = connect();
        let events = drive(&mut server, &mut [&mut first], 0, 400);
        assert!(events[0]
            .iter()
            .any(|e| matches!(e, StockGameEvent::BattleRules(received) if *received == BALANCED)));
        assert_eq!(server.roles.len(), 1);
        assert_eq!(server.rules_verified.len(), 1);
        let mut late = connect();
        let events = drive(&mut server, &mut [&mut first, &mut late], 400, 400);
        assert!(events[1]
            .iter()
            .any(|e| matches!(e, StockGameEvent::BattleRules(received) if *received == BALANCED)));
        assert_eq!(server.roles.len(), 2);
        assert!(server
            .roles
            .keys()
            .all(|peer| server.rules_verified.contains(peer)));

        let mut legacy =
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap();
        for now in 800..1000 {
            legacy.tick(now).unwrap();
            server.tick(now).unwrap();
        }
        let peer = legacy.local_addr().unwrap();
        let request = StockEnterRoleRequest {
            hero: 0,
            camp: 0,
            hero_checksum: server.heroes.checksum(0).unwrap(),
        };
        server
            .handle_application(
                peer,
                STOCK_REQUEST_ENTER_ROLE,
                &request.encode(),
                1000,
                &mut Vec::new(),
            )
            .unwrap();
        server
            .handle_application(peer, STOCK_REQUEST_READY, &[], 1000, &mut Vec::new())
            .unwrap();
        server
            .handle_application(peer, ACK_BATTLE_RULES, b"MBR2\x01", 1000, &mut Vec::new())
            .unwrap();
        let stale = FileFrame::Ack {
            id: BATTLE_RULES_TRANSFER_ID,
            next: 0,
        }
        .encode();
        server
            .handle_application(peer, ACK_BATTLE_RULES, &stale, 1000, &mut Vec::new())
            .unwrap();
        assert!(!server.roles.contains_key(&peer));
        assert!(!server.ready_peers.contains(&peer));
        server.tick(7000).unwrap();
        assert!(!server.rules_waiting.contains_key(&peer));
        assert!(!server.pending_rule_entry.contains_key(&peer));
    }

    #[test]
    fn udp_latency_table_reaches_requesting_players() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "ping").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let connect = || {
            StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                0,
                0,
                6,
            )
            .unwrap()
        };
        let mut first = connect();
        let mut second = connect();
        let events = drive(&mut server, &mut [&mut first, &mut second], 0, 2500);
        for received in events {
            assert!(received.iter().any(
                |event| matches!(event, StockGameEvent::Latencies(samples)
                if samples.iter().any(|(id, _)| Some(*id) == first.transport_id())
                    && samples.iter().any(|(id, _)| Some(*id) == second.transport_id()))
            ));
        }
        assert!(server.transport.peer_latencies(20_000).is_empty());
    }

    #[test]
    fn remote_vitals_udp_negotiation_late_join_and_periodic_repair() {
        use crate::stock_remote_vitals::*;
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "vitals").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let connect = || {
            StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                0,
                0,
                6,
            )
            .unwrap()
        };
        let mut first = connect();
        let mut second = connect();
        let mut legacy = connect();
        legacy.set_remote_vitals_enabled(false);
        let initial = drive(
            &mut server,
            &mut [&mut first, &mut second, &mut legacy],
            0,
            500,
        );
        assert_eq!(server.remote_vitals_peers.len(), 2);
        let actor = second.actor_id().unwrap();
        assert!(initial[0].iter().any(|event| matches!(event,
            StockGameEvent::RemoteVitals(sample) if sample.actor_id == actor && sample.vitals.health == 2000)));
        let peer = second.transport.local_addr().unwrap();
        let role = server.roles.get_mut(&peer).unwrap();
        role.health = 753.0;
        role.max_health = 2500.0;
        role.anger = 42.0;
        let events = drive(
            &mut server,
            &mut [&mut first, &mut second, &mut legacy],
            500,
            200,
        );
        let expected = StockVitalsEvent {
            health: 753,
            max_health: 2500,
            anger: 42,
        };
        assert!(events[0].iter().any(|event| matches!(event,
            StockGameEvent::RemoteVitals(sample) if sample.actor_id == actor && sample.vitals == expected)));
        assert!(events[1]
            .iter()
            .any(|event| matches!(event, StockGameEvent::Vitals(value) if *value == expected)));
        assert!(!events[1].iter().any(|event| matches!(event,
            StockGameEvent::RemoteVitals(sample) if sample.actor_id == actor)));
        for event in initial[2].iter().chain(&events[2]) {
            assert!(!matches!(event, StockGameEvent::RemoteVitals(_)));
            assert!(
                !matches!(event, StockGameEvent::RawApplication { opcode, .. } if *opcode == EVENT_REMOTE_VITALS)
            );
        }

        let mut late = connect();
        let recovered = drive(
            &mut server,
            &mut [&mut first, &mut second, &mut legacy, &mut late],
            700,
            400,
        );
        for index in [0, 3] {
            assert!(recovered[index].iter().any(|event| matches!(event,
                StockGameEvent::RemoteVitals(sample) if sample.actor_id == actor && sample.vitals == expected)));
        }

        legacy
            .transport
            .send_application(
                REQUEST_REMOTE_VITALS,
                b"MRV2".to_vec(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                1100,
            )
            .unwrap();
        drive(
            &mut server,
            &mut [&mut first, &mut second, &mut legacy, &mut late],
            1100,
            100,
        );
        assert!(!server
            .remote_vitals_peers
            .contains(&legacy.transport.local_addr().unwrap()));
        second.transport.disconnect(0).unwrap();
        drive(
            &mut server,
            &mut [&mut first, &mut legacy, &mut late],
            1200,
            100,
        );
        assert!(!server.remote_vitals_peers.contains(&peer));
    }

    #[test]
    fn remote_vitals_udp_rejects_stale_duplicate_and_malformed_samples() {
        use crate::stock_remote_vitals::*;
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "vitals").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        drive(&mut server, &mut [&mut client], 0, 400);
        let peer = client.transport.local_addr().unwrap();
        let sample = RemoteVitals {
            actor_id: 999,
            revision: u32::MAX,
            vitals: StockVitalsEvent {
                health: 300,
                max_health: 1000,
                anger: 12,
            },
        };
        for revision in [u32::MAX, 0, u32::MAX, 0] {
            server
                .transport
                .send_application(
                    peer,
                    EVENT_REMOTE_VITALS,
                    RemoteVitals { revision, ..sample }.encode(),
                    STOCK_EVENT_ROLE_STATE_FLAGS,
                    400,
                )
                .unwrap();
        }
        server
            .transport
            .send_application(
                peer,
                EVENT_REMOTE_VITALS,
                vec![0; 18],
                STOCK_EVENT_ROLE_STATE_FLAGS,
                400,
            )
            .unwrap();
        let events = drive(&mut server, &mut [&mut client], 400, 100);
        let revisions: Vec<_> = events[0]
            .iter()
            .filter_map(|event| match event {
                StockGameEvent::RemoteVitals(sample) => Some(sample.revision),
                _ => None,
            })
            .collect();
        assert_eq!(revisions, [u32::MAX, 0]);
    }

    #[test]
    fn hosted_stage_reaches_initial_and_late_clients_before_role_entry() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "stage").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        server.set_stage_id("SN06");
        let connect = || {
            StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                0,
                0,
                6,
            )
            .unwrap()
        };
        let mut first = connect();
        let events = drive(&mut server, &mut [&mut first], 0, 500);
        let mut late = connect();
        let late_events = drive(&mut server, &mut [&mut first, &mut late], 500, 500);
        for events in [&events[0], &late_events[1]] {
            let config = events
                .iter()
                .position(|event| {
                    matches!(event,
                        StockGameEvent::StageConfig(config) if config.stage_id == "sn06"
                    )
                })
                .expect("host must send its selected stage");
            let entered = events
                .iter()
                .position(|event| matches!(event, StockGameEvent::RoleEntered(_)))
                .expect("client must enter");
            assert!(config < entered, "map must arrive before any role entry");
        }
    }

    #[test]
    fn life_adjust_maps_original_percent_to_room_max_health() {
        assert_eq!(stock_max_health_for_life_adjust(50), 500.0);
        assert_eq!(stock_max_health_for_life_adjust(200), 2_000.0);
        assert_eq!(stock_max_health_for_life_adjust(500), 5_000.0);
        assert_eq!(stock_max_health_for_life_adjust(0), 100.0);
        assert_eq!(stock_max_health_for_life_adjust(999), 5_000.0);
    }

    #[test]
    fn stock_ninja_self_effect_kills_after_queued_hit_and_never_awards_self_kill() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "status").unwrap();
        let addr = transport.local_addr().unwrap();
        let checksums = [1234; STOCK_HERO_COUNT];
        let table = meteor_asset::res::parse_meteor_res(include_bytes!(
            "../../../../origin_res/Meteor.res"
        ))
        .unwrap()
        .combat_table();
        let mut server = StockGameServer::with_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            table,
        );
        let client = |hero| {
            StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero,
                    camp: 0,
                    hero_checksum: checksums[hero as usize],
                },
                1,
            )
        };
        let mut attacker = client(0);
        let mut defender = client(1);
        drive(&mut server, &mut [&mut attacker, &mut defender], 0, 100);
        let target = defender.actor_id().unwrap();
        let source = attacker.actor_id().unwrap();
        attacker
            .send_hit(
                StockHitRequest {
                    target_actor_id: target,
                    hit_word: 468,
                    target_pose_word: 103,
                    moves_word: 150,
                    values_word: 0,
                },
                2100,
            )
            .unwrap();
        attacker.send_self_effect(35, 2100).unwrap();
        drive(&mut server, &mut [&mut attacker, &mut defender], 2100, 100);
        let victim = server
            .roles
            .values()
            .find(|role| role.actor_id == target)
            .unwrap();
        assert!(
            victim.health < victim.max_health,
            "queued damage must land before the caster dies"
        );
        let caster = server
            .roles
            .values()
            .find(|role| role.actor_id == source)
            .unwrap();
        assert_eq!(caster.health, 0.0);
        assert_eq!(caster.deaths, 1);
        assert_eq!(caster.kills, u32::from(victim.health == 0.0));
        assert!(caster.respawn_at_ms.is_some());
        attacker.send_self_effect(35, 2250).unwrap();
        drive(&mut server, &mut [&mut attacker, &mut defender], 2250, 50);
        assert_eq!(
            server
                .roles
                .values()
                .find(|role| role.actor_id == source)
                .unwrap()
                .deaths,
            1
        );
    }

    #[test]
    fn balance_udp_chip_death_reaches_both_peers() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "status").unwrap();
        let addr = transport.local_addr().unwrap();
        let checksums = [1234; STOCK_HERO_COUNT];
        let table = meteor_asset::res::parse_meteor_res(include_bytes!(
            "../../../../origin_res/Meteor.res"
        ))
        .unwrap()
        .combat_table();
        let mut server = StockGameServer::with_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            table,
        );
        server.set_battle_rules(meteor_combat::battle_rules::BattleRules::BALANCED);
        let client = |hero| {
            StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero,
                    camp: 0,
                    hero_checksum: checksums[hero as usize],
                },
                1,
            )
        };
        let mut attacker = client(0);
        let mut defender = client(1);
        drive(&mut server, &mut [&mut attacker, &mut defender], 0, 100);
        let target = defender.actor_id().unwrap();
        let target_peer = *server
            .roles
            .iter()
            .find(|(_, role)| role.actor_id == target)
            .unwrap()
            .0;
        let mut state = sample_state(0.0, 1);
        state.state_words.guard_route = true;
        let role = server.roles.get_mut(&target_peer).unwrap();
        role.state = Some(state);
        role.health = 1.0;
        attacker
            .send_hit(
                StockHitRequest {
                    target_actor_id: target,
                    hit_word: 245 | (1 << 16),
                    target_pose_word: 110,
                    moves_word: 100 | (125 << 16),
                    values_word: 50 | (35 << 16),
                },
                2100,
            )
            .unwrap();
        let received = drive(&mut server, &mut [&mut attacker, &mut defender], 2100, 100);
        assert_eq!(server.roles[&target_peer].health, 0.0);
        for events in received {
            assert!(events.iter().any(
                |e| matches!(e, StockGameEvent::Kill(kill) if kill.target_actor_id == target)
            ));
        }
        assert_eq!(server.roles[&target_peer].deaths, 1);
    }

    #[test]
    fn pose_rules_udp_guard_minimum_uses_host_override() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "status").unwrap();
        let addr = transport.local_addr().unwrap();
        let checksums = [1234; STOCK_HERO_COUNT];
        let table = meteor_asset::res::parse_meteor_res(include_bytes!(
            "../../../../origin_res/Meteor.res"
        ))
        .unwrap()
        .combat_table();
        let mut server = StockGameServer::with_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            table,
        );
        server.set_battle_rules(meteor_combat::battle_rules::BattleRules::from_json(r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":10,"guardChipPoses":"all","poseOverrides":[{"poseId":245,"guardStunMs":450}]}}"#).unwrap());
        let client = |hero| {
            StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero,
                    camp: 0,
                    hero_checksum: checksums[hero as usize],
                },
                1,
            )
        };
        let mut attacker = client(0);
        let mut defender = client(1);
        drive(&mut server, &mut [&mut attacker, &mut defender], 0, 100);
        let target = defender.actor_id().unwrap();
        let target_peer = *server
            .roles
            .iter()
            .find(|(_, role)| role.actor_id == target)
            .unwrap()
            .0;
        for selector in 0..4u32 {
            let now = 2100 + u64::from(selector) * 1000;
            let mut state = sample_state(0.0, 1);
            state.state_words.guard_route = true;
            let role = server.roles.get_mut(&target_peer).unwrap();
            role.state = Some(state);
            role.weapon_item_id = Some(5);
            attacker
                .send_hit(
                    StockHitRequest {
                        target_actor_id: target,
                        hit_word: 245 | (1 << 16),
                        target_pose_word: 110 | (selector << 12),
                        moves_word: 100 | (125 << 16),
                        values_word: 50 | (35 << 16),
                    },
                    now,
                )
                .unwrap();
            let received = drive(&mut server, &mut [&mut attacker, &mut defender], now, 60);
            for events in received {
                let event = events
                    .iter()
                    .find_map(|event| match event {
                        StockGameEvent::HitReaction {
                            guarded: true,
                            event,
                        } => Some(event),
                        _ => None,
                    })
                    .expect("both original-compatible peers receive event 14");

                let wire = event.encode();
                assert_eq!(u16::from_le_bytes([wire[0], wire[1]]), selector as u16);
                assert_eq!(u16::from_le_bytes([wire[2], wire[3]]), 110);
                assert_eq!(event.move_hundred, 125);
                assert_eq!(event.value_hundred, 45);
                assert_eq!(event.context, 1);
                assert_eq!(event.target_actor_id, target);
            }
        }
    }

    #[test]
    fn stock_guard_event_uses_selector_and_target_pose_for_original_client() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "status").unwrap();
        let addr = transport.local_addr().unwrap();
        let checksums = [1234; STOCK_HERO_COUNT];
        let table = meteor_asset::res::parse_meteor_res(include_bytes!(
            "../../../../origin_res/Meteor.res"
        ))
        .unwrap()
        .combat_table();
        let mut server = StockGameServer::with_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            table,
        );
        let client = |hero| {
            StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero,
                    camp: 0,
                    hero_checksum: checksums[hero as usize],
                },
                1,
            )
        };
        let mut attacker = client(0);
        let mut defender = client(1);
        drive(&mut server, &mut [&mut attacker, &mut defender], 0, 100);
        let target = defender.actor_id().unwrap();
        let target_peer = *server
            .roles
            .iter()
            .find(|(_, role)| role.actor_id == target)
            .unwrap()
            .0;
        for selector in 0..4u32 {
            let now = 2100 + u64::from(selector) * 1000;
            let mut state = sample_state(0.0, 1);
            state.state_words.guard_route = true;
            let role = server.roles.get_mut(&target_peer).unwrap();
            role.state = Some(state);
            role.weapon_item_id = Some(5);
            attacker
                .send_hit(
                    StockHitRequest {
                        target_actor_id: target,
                        hit_word: 245 | (1 << 16),
                        target_pose_word: 110 | (selector << 12),
                        moves_word: 100 | (125 << 16),
                        values_word: 50 | (35 << 16),
                    },
                    now,
                )
                .unwrap();
            let received = drive(&mut server, &mut [&mut attacker, &mut defender], now, 60);
            for events in received {
                let event = events
                    .iter()
                    .find_map(|event| match event {
                        StockGameEvent::HitReaction {
                            guarded: true,
                            event,
                        } => Some(event),
                        _ => None,
                    })
                    .expect("both original-compatible peers receive event 14");

                let wire = event.encode();
                assert_eq!(u16::from_le_bytes([wire[0], wire[1]]), selector as u16);
                assert_eq!(u16::from_le_bytes([wire[2], wire[3]]), 110);
                assert_eq!(event.move_hundred, 125);
                assert_eq!(event.value_hundred, 35);
                assert_eq!(event.context, 1);
                assert_eq!(event.target_actor_id, target);
            }
        }
    }

    #[test]
    fn stock_status_effect_hit_broadcast_tick_guard_and_respawn() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "status").unwrap();
        let addr = transport.local_addr().unwrap();
        let checksums = [1234; STOCK_HERO_COUNT];
        let table = meteor_asset::res::parse_meteor_res(include_bytes!(
            "../../../../origin_res/Meteor.res"
        ))
        .unwrap()
        .combat_table();
        let mut server = StockGameServer::with_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            table,
        );
        let client = |hero| {
            StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero,
                    camp: 0,
                    hero_checksum: checksums[hero as usize],
                },
                1,
            )
        };
        let mut attacker = client(0);
        let mut defender = client(1);
        assert!(attacker.send_self_effect(34, 0).is_err());
        drive(&mut server, &mut [&mut attacker, &mut defender], 0, 100);
        attacker.send_self_effect(34, 100).unwrap();
        let self_events = drive(&mut server, &mut [&mut attacker, &mut defender], 100, 60);
        for events in self_events {
            assert!(events.iter().any(|event| matches!(event,
                StockGameEvent::PropertyEffect(effect)
                    if effect.actor_id == attacker.actor_id().unwrap()
                    && effect.category == 2 && effect.resource_id == 34)));
        }
        let caster = server
            .roles
            .values_mut()
            .find(|role| role.actor_id == attacker.actor_id().unwrap())
            .unwrap();
        assert_eq!(caster.item_attack_bonus(), 40);
        assert_eq!(
            caster.hit_status_effects.active[0].definition.duration,
            90.0
        );
        caster.hit_status_effects.tick(91.0);
        assert_eq!(caster.item_attack_bonus(), 0);
        let target = defender.actor_id().unwrap();
        let target_peer = *server
            .roles
            .iter()
            .find(|(_, role)| role.actor_id == target)
            .unwrap()
            .0;
        let hit = |pose| StockHitRequest {
            target_actor_id: target,
            hit_word: pose,
            target_pose_word: 110,
            moves_word: 100,
            values_word: 50,
        };
        attacker.send_hit(hit(204), 2100).unwrap();
        let events = drive(&mut server, &mut [&mut attacker, &mut defender], 2100, 60);
        for events in events {
            let hit_index = events
                .iter()
                .position(|event| {
                    matches!(event,
                        StockGameEvent::HitReaction { guarded: false, event }
                            if event.target_actor_id == target
                    )
                })
                .expect("the target receives the original event 11");
            let property_index = events.iter().position(|event| matches!(event,
                StockGameEvent::PropertyEffect(effect)
                    if effect.actor_id == target && effect.category == 2 && effect.resource_id == 12
            )).expect("the target receives the paired original event 61");
            assert!(hit_index < property_index,
                "Meteor.exe 0x4af983 enqueues event 11 before 0x4afe24 enqueues event 61: {events:?}");
        }
        attacker.send_hit(hit(204), 3000).unwrap();
        drive(&mut server, &mut [&mut attacker, &mut defender], 3000, 30);
        assert_eq!(
            server.roles[&target_peer].hit_status_effects.active.len(),
            1
        );
        let hp = server.roles[&target_peer].health;
        drive(&mut server, &mut [&mut attacker, &mut defender], 4100, 30);
        assert_eq!(server.roles[&target_peer].health, hp - 10.0);
        attacker.send_hit(hit(224), 4500).unwrap();
        drive(&mut server, &mut [&mut attacker, &mut defender], 4500, 30);
        assert!(
            (server.roles[&target_peer]
                .hit_status_effects
                .speed_factor(1.0)
                - 0.3)
                .abs()
                < 1e-6
        );
        let mut state = sample_state(0.0, 1);
        state.state_words.guard_route = true;
        server.roles.get_mut(&target_peer).unwrap().state = Some(state);
        attacker.send_hit(hit(210), 4800).unwrap();
        let guarded = drive(&mut server, &mut [&mut attacker, &mut defender], 4800, 30);
        assert!(!guarded.iter().flatten().any(|event| matches!(event, StockGameEvent::PropertyEffect(effect) if effect.resource_id == 11)));
        assert!(!server.roles[&target_peer]
            .hit_status_effects
            .active
            .iter()
            .any(|effect| effect.definition.id == 11));
        server.roles.get_mut(&target_peer).unwrap().health = 5.0;
        let deaths = drive(&mut server, &mut [&mut attacker, &mut defender], 6100, 30);
        assert!(deaths[1]
            .iter()
            .any(|event| matches!(event, StockGameEvent::LocalDeath)));
        assert!(server.roles[&target_peer]
            .hit_status_effects
            .active
            .is_empty());
        drive(&mut server, &mut [&mut attacker, &mut defender], 9100, 30);
        assert_eq!(
            server.roles[&target_peer].health,
            server.roles[&target_peer].max_health
        );
        assert_eq!(
            server.roles[&target_peer]
                .hit_status_effects
                .speed_factor(1.0),
            1.0
        );
    }

    #[test]
    fn stock_recorded_ultimate_hit_precedes_property_on_both_peers() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "ultimate-order").unwrap();
        let address = transport.local_addr().unwrap();
        let checksums = [1234; STOCK_HERO_COUNT];
        let table = meteor_asset::res::parse_meteor_res(include_bytes!(
            "../../../../origin_res/Meteor.res"
        ))
        .unwrap()
        .combat_table();
        assert_eq!(table.pose_status_effect(473).unwrap().id, 37);
        assert_eq!(table.pose_status_effect(421).unwrap().id, 21);
        assert_eq!(table.pose_status_effect(473).unwrap().anger, -5);
        assert_eq!(table.pose_status_effect(421).unwrap().anger, -10);
        let mut server = StockGameServer::with_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            table,
        );
        let client = || {
            StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero: 0,
                    camp: 0,
                    hero_checksum: checksums[0],
                },
                1,
            )
        };
        let mut attacker = client();
        let mut defender = client();
        drive(&mut server, &mut [&mut attacker, &mut defender], 0, 100);
        let target = defender.actor_id().unwrap();
        for (now_ms, pose, strategy, minimum_hundred, resource_id) in
            [(2100, 473, 11, 300, 37), (2400, 421, 10, 100, 21)]
        {
            attacker
                .send_hit(
                    StockHitRequest {
                        target_actor_id: target,
                        hit_word: pose | (strategy << 16),
                        target_pose_word: 109,
                        moves_word: 0,
                        values_word: minimum_hundred,
                    },
                    now_ms,
                )
                .unwrap();
            for events in drive(&mut server, &mut [&mut attacker, &mut defender], now_ms, 60) {
                let hit = events
                    .iter()
                    .position(|event| {
                        matches!(event,
                            StockGameEvent::HitReaction { guarded: false, event }
                                if event.target_actor_id == target && event.pose_id == 109
                                    && event.value_hundred == minimum_hundred
                                    && event.context == strategy
                        )
                    })
                    .unwrap_or_else(|| panic!("event 11 for pose {pose}: {events:?}"));
                let property = events
                    .iter()
                    .position(|event| {
                        matches!(event,
                            StockGameEvent::PropertyEffect(event)
                                if event.actor_id == target && event.category == 2
                                    && event.resource_id == resource_id
                        )
                    })
                    .expect("event 61");
                assert!(hit < property, "pose={pose} events={events:?}");
            }
        }
    }

    #[test]
    fn stock_scene_transfer_reaches_only_target_and_preserves_pose_and_vitals() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "SN22 transfer").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut clients = (0..2)
            .map(|_| {
                StockGameClient::for_bundled_revision(
                    StockClientTransport::bind(
                        "127.0.0.1:0",
                        address,
                        StockConnectRecord::for_open_room(),
                    )
                    .unwrap(),
                    0,
                    0,
                    6,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            0,
            100,
        );
        let guest = clients[1].actor_id().unwrap();
        clients[1]
            .send_role_state(sample_state(1.2, 7), 100)
            .unwrap();
        drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            100,
            100,
        );
        let before = server
            .roles
            .values()
            .find(|role| role.actor_id == guest)
            .unwrap()
            .clone();
        let mut state = sample_state(90.0, 99);
        state.position.y = 12.0;
        state.position.z = -36.0;
        state.forward = RoleVec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        let event = StockSceneTransferEvent::from_role_state(state);
        server.transfer_scene_actor(guest, event, 200).unwrap();
        let received = drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            200,
            100,
        );
        assert!(!received[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::SceneTransfer(_))));
        assert_eq!(
            received[1]
                .iter()
                .filter(|received| **received == StockGameEvent::SceneTransfer(event))
                .count(),
            1
        );
        let role = server
            .roles
            .values()
            .find(|role| role.actor_id == guest)
            .unwrap();
        assert_eq!(role.state.unwrap().position, state.position);
        assert_eq!(role.state.unwrap().forward, state.forward);
        assert_eq!(role.state.unwrap().velocity, RoleVec3::ZERO);
        assert_eq!(
            role.state.unwrap().state_words,
            before.state.unwrap().state_words
        );
        assert_eq!(role.health, before.health);
        assert_eq!(role.anger, before.anger);

        clients[0]
            .transport_mut()
            .send_application(
                STOCK_EVENT_SCENE_TRANSFER,
                event.encode().to_vec(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                300,
            )
            .unwrap();
        let untrusted = drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            300,
            100,
        );
        assert!(untrusted
            .iter()
            .flatten()
            .all(|event| !matches!(event, StockGameEvent::SceneTransfer(_))));
    }

    #[test]
    fn two_native_clients_enter_equip_and_receive_remote_event2_state() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "Rust stock room").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        let mut client1 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 0,
                hero_checksum: checksums[0],
            },
            6,
        );
        let mut client2 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 1,
                camp: 0,
                hero_checksum: checksums[1],
            },
            5,
        );

        drive(&mut server, &mut [&mut client1, &mut client2], 0, 80);
        assert!(client1.actor_id().is_some());
        assert!(client2.actor_id().is_some());
        assert_eq!(server.ready_peer_count(), 2);

        client1.send_role_state(sample_state(1.2, 1), 100).unwrap();
        client2.send_role_state(sample_state(2.4, 1), 100).unwrap();
        let events = drive(&mut server, &mut [&mut client1, &mut client2], 100, 80);
        let actor1 = client1.actor_id().unwrap() as u16;
        let actor2 = client2.actor_id().unwrap() as u16;
        assert!(events[0].iter().any(|event| matches!(
            event,
            StockGameEvent::RoleStates(records)
                if records.iter().any(|record| record.actor_id == actor2)
                    && records.iter().all(|record| record.actor_id != actor1)
        )));
        assert!(events[1].iter().any(|event| matches!(
            event,
            StockGameEvent::RoleStates(records)
                if records.iter().any(|record| record.actor_id == actor1)
                    && records.iter().all(|record| record.actor_id != actor2)
        )));

        client1
            .send_hit(
                StockHitRequest {
                    target_actor_id: u32::from(actor2),
                    hit_word: 250,
                    target_pose_word: 110 | 0x2000,
                    moves_word: 400 | (200 << 16),
                    values_word: 50 | (125 << 16),
                },
                2_100,
            )
            .unwrap();
        let events = drive(&mut server, &mut [&mut client1, &mut client2], 2_100, 60);
        for client_events in events {
            assert!(client_events.iter().any(|event| matches!(
                event,
                StockGameEvent::HitReaction {
                    guarded: false,
                    event,
                } if event.target_actor_id == u32::from(actor2)
                    && event.pose_id == 110
                    && event.move_hundred == 400
                    && event.value_hundred == 50
            )));
        }
        let events = drive(&mut server, &mut [&mut client1, &mut client2], 2_160, 80);
        assert!(events[1].iter().any(|event| matches!(
            event,
            StockGameEvent::Vitals(StockVitalsEvent {
                health: 1890,
                max_health: 2000,
                anger: 15,
            })
        )));

        let hit = StockHitRequest {
            target_actor_id: u32::from(actor2),
            hit_word: 250,
            target_pose_word: 110,
            moves_word: 400,
            values_word: 50,
        };
        let mut death_seen = false;
        let mut zero_vitals_seen = false;
        let mut kill_seen = [false; 2];
        for index in 0..20 {
            let now = 2_500 + index * 250;
            client1.send_hit(hit, now).unwrap();
            let output = drive(&mut server, &mut [&mut client1, &mut client2], now, 20);
            for (client_index, events) in output.iter().enumerate() {
                kill_seen[client_index] |= events.iter().any(|event| {
                    matches!(
                        event,
                        StockGameEvent::Kill(StockKillEvent {
                            attacker_actor_id,
                            target_actor_id,
                        }) if *attacker_actor_id == u32::from(actor1)
                            && *target_actor_id == u32::from(actor2)
                    )
                });
            }
            death_seen |= output[1]
                .iter()
                .any(|event| matches!(event, StockGameEvent::LocalDeath));
            zero_vitals_seen |= output[1].iter().any(|event| {
                matches!(
                    event,
                    StockGameEvent::Vitals(StockVitalsEvent { health: 0, .. })
                )
            });
        }
        let output = drive(&mut server, &mut [&mut client1, &mut client2], 7_200, 100);
        for (client_index, events) in output.iter().enumerate() {
            kill_seen[client_index] |= events.iter().any(|event| {
                matches!(
                    event,
                    StockGameEvent::Kill(StockKillEvent {
                        attacker_actor_id,
                        target_actor_id,
                    }) if *attacker_actor_id == u32::from(actor1)
                        && *target_actor_id == u32::from(actor2)
                )
            });
        }
        death_seen |= output[1]
            .iter()
            .any(|event| matches!(event, StockGameEvent::LocalDeath));
        zero_vitals_seen |= output[1].iter().any(|event| {
            matches!(
                event,
                StockGameEvent::Vitals(StockVitalsEvent { health: 0, .. })
            )
        });
        assert!(death_seen, "lethal stock hit did not emit event 24");
        assert_eq!(kill_seen, [true, true], "event 23 was not broadcast");
        assert!(
            zero_vitals_seen,
            "lethal stock hit did not emit zero HP event 25"
        );
        let output = drive(&mut server, &mut [&mut client1, &mut client2], 9_800, 100);
        assert!(output[1]
            .iter()
            .any(|event| matches!(event, StockGameEvent::LocalRespawn)));
        assert!(
            !output[0]
                .iter()
                .any(|event| matches!(event, StockGameEvent::LocalRespawn)),
            "event 67 must be private to the actor being respawned"
        );
        assert!(output[1].iter().any(|event| matches!(
            event,
            StockGameEvent::Respawn(StockRespawnEvent { actor_id })
                if *actor_id == u32::from(actor2)
        )));
        let attacker = server
            .roles
            .values()
            .find(|role| role.actor_id == u32::from(actor1))
            .unwrap();
        let target = server
            .roles
            .values()
            .find(|role| role.actor_id == u32::from(actor2))
            .unwrap();
        assert_eq!((attacker.kills, attacker.deaths), (1, 0));
        assert_eq!((target.kills, target.deaths), (0, 1));
        assert!(output[1].iter().any(|event| matches!(
            event,
            StockGameEvent::Vitals(StockVitalsEvent {
                health: 2000,
                max_health: 2000,
                anger: 0,
            })
        )));

        server.rule = 5;
        server.set_round_limit(1);
        for role in server.roles.values_mut() {
            role.camp = if role.actor_id == u32::from(actor1) {
                1
            } else {
                2
            };
        }
        let mut second_death_seen = false;
        let mut score_seen = [false; 2];
        let mut match_end_seen = [false; 2];
        for index in 0..20 {
            let now = 12_000 + index * 250;
            client1.send_hit(hit, now).unwrap();
            let output = drive(&mut server, &mut [&mut client1, &mut client2], now, 20);
            for (client_index, events) in output.iter().enumerate() {
                score_seen[client_index] |= events.iter().any(|event| {
                    matches!(
                        event,
                        StockGameEvent::TeamScore(StockTeamScoreEvent {
                            team1_wins: 1,
                            team2_wins: 0,
                        })
                    )
                });
                match_end_seen[client_index] |= events
                    .iter()
                    .any(|event| matches!(event, StockGameEvent::MatchEnd));
            }
            second_death_seen |= output[1]
                .iter()
                .any(|event| matches!(event, StockGameEvent::LocalDeath));
        }
        assert!(
            second_death_seen,
            "second death missing: {:?}",
            server
                .roles
                .values()
                .map(|role| (role.actor_id, role.health, role.kills, role.deaths))
                .collect::<Vec<_>>()
        );
        assert_eq!(score_seen, [true, true]);
        assert_eq!(match_end_seen, [true, true]);
        assert_eq!(server.team_wins(), [1, 0]);
        let output = drive(&mut server, &mut [&mut client1, &mut client2], 20_000, 100);
        assert!(!output[1]
            .iter()
            .any(|event| matches!(event, StockGameEvent::Respawn(_))));
        let attacker = server
            .roles
            .values()
            .find(|role| role.actor_id == u32::from(actor1))
            .unwrap();
        let target = server
            .roles
            .values()
            .find(|role| role.actor_id == u32::from(actor2))
            .unwrap();
        assert_eq!((attacker.kills, target.deaths), (2, 2));
        assert_eq!(target.health, 0.0);
        assert_eq!(target.respawn_at_ms, None);
    }

    #[test]
    fn stock_client_preserves_ordinary_scene_record_until_event32_pickup() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "items").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        let mut client = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 0,
                hero_checksum: checksums[0],
            },
            6,
        );
        drive(&mut server, &mut [&mut client], 0, 100);
        let peer = server.transport().peer_addresses()[0];
        let actor_id = client.actor_id().unwrap();
        let record = StockSceneItemRecord::from_role_position(
            77,
            1,
            2,
            RoleVec3 {
                x: 1.25,
                y: 2.5,
                z: -3.75,
            },
        );
        server
            .transport_mut()
            .send_application(
                peer,
                STOCK_EVENT_SCENE_SNAPSHOT,
                record.encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                100,
            )
            .unwrap();
        let grounded = drive(&mut server, &mut [&mut client], 100, 20);
        assert!(grounded[0].iter().any(|event| matches!(
            event,
            StockGameEvent::SceneItemsGrounded { records, spawned: false }
                if records == &vec![record]
        )));

        let taken = StockSceneTakenEvent {
            actor_id,
            record_id: record.record_id,
            category: record.category,
            item_id: record.item_id,
        };
        server
            .transport_mut()
            .send_application(
                peer,
                STOCK_EVENT_SCENE_TAKEN,
                taken.encode(),
                STOCK_DISCRETE_RELIABLE_FLAGS,
                120,
            )
            .unwrap();
        let picked = drive(&mut server, &mut [&mut client], 120, 20);
        assert!(picked[0].iter().any(|event| matches!(
            event,
            StockGameEvent::SceneItemTaken {
                event,
                record: Some(observed),
            } if *event == taken && *observed == record
        )));
    }

    #[test]
    fn stock_christmas_star_candies_are_shared_across_transaction_chunks() {
        use crate::stock_protocol::{
            StockSceneOperation, STOCK_SCENE_OP_ACTIVE, STOCK_SCENE_OP_INTERACTIVE,
        };
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "sn21").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let connect = || {
            StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                0,
                0,
                6,
            )
            .unwrap()
        };
        let mut host = connect();
        let mut peer = connect();
        drive(&mut server, &mut [&mut host, &mut peer], 0, 100);

        server
            .replace_authored_scene_items(
                (5..=103)
                    .map(|reference| StockAuthoredSceneItem {
                        item_id: 28,
                        category: 2,
                        position: RoleVec3 {
                            x: reference as f32 * 10.0,
                            y: 0.0,
                            z: 0.0,
                        },
                        half_extents: [0.25; 3],
                        respawn_ms: Some(99_999_000),
                        repeat_on_xstar: true,
                        scene_reference: Some(reference),
                        active: false,
                        interactive: false,
                    })
                    .collect(),
                100,
            )
            .unwrap();
        let hidden = drive(&mut server, &mut [&mut host, &mut peer], 100, 30);
        assert!(server.active_scene_records().is_empty());
        assert!(hidden
            .iter()
            .flatten()
            .all(|event| !matches!(event, StockGameEvent::SceneItemTaken { .. })));
        let operations = (5..=103)
            .flat_map(|reference| {
                [
                    StockSceneOperation {
                        reference,
                        kind: STOCK_SCENE_OP_ACTIVE,
                        value: 1,
                        extra: 0,
                    },
                    StockSceneOperation {
                        reference,
                        kind: STOCK_SCENE_OP_INTERACTIVE,
                        value: 1,
                        extra: 0,
                    },
                ]
            })
            .collect::<Vec<_>>();

        host.send_scene_ops(
            &StockSceneOpsEvent {
                timestamp_ms: 130,
                operations,
            },
            130,
        )
        .unwrap();
        let released = drive(&mut server, &mut [&mut host, &mut peer], 130, 60);
        assert_eq!(server.active_scene_records().len(), 99);
        for events in &released {
            assert_eq!(
                events
                    .iter()
                    .filter_map(|event| match event {
                        StockGameEvent::SceneItemsGrounded {
                            records,
                            spawned: true,
                        } => Some(records.len()),
                        _ => None,
                    })
                    .sum::<usize>(),
                99
            );
        }
        let record = server
            .scene_items
            .values()
            .find(|item| item.seed.scene_reference == Some(5))
            .unwrap()
            .record(None);

        peer.send_role_state(sample_state(50.0, 2), 190).unwrap();
        let picked = drive(&mut server, &mut [&mut host, &mut peer], 190, 30);
        assert_eq!(server.active_scene_records().len(), 98);
        for events in &picked {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event,
                        StockGameEvent::SceneItemTaken { event, record: Some(observed) }
                            if Some(event.actor_id) == peer.actor_id() && *observed == record
                    ))
                    .count(),
                1
            );
        }
        let mut late = connect();
        let joined = drive(
            &mut server,
            &mut [&mut host, &mut peer, &mut late],
            220,
            100,
        );
        let observed = joined[2]
            .iter()
            .filter_map(|event| match event {
                StockGameEvent::SceneItemsGrounded {
                    records,
                    spawned: false,
                } => Some(records.as_slice()),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(observed.len(), 98);
        assert!(observed
            .iter()
            .all(|remaining| remaining.record_id != record.record_id));
    }

    #[test]
    fn stock_box_release_registers_before_pickup_for_host_peer_and_late_join() {
        use crate::stock_protocol::{
            StockSceneOperation, STOCK_SCENE_OP_ACTIVE, STOCK_SCENE_OP_INTERACTIVE,
        };
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "boxes").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let connect = || {
            StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                0,
                0,
                6,
            )
            .unwrap()
        };
        let mut host = connect();
        let mut peer = connect();
        drive(&mut server, &mut [&mut host, &mut peer], 0, 100);
        server
            .replace_authored_scene_items(
                vec![
                    StockAuthoredSceneItem {
                        item_id: 5,
                        category: 1,
                        position: RoleVec3::ZERO,
                        half_extents: [0.25; 3],
                        respawn_ms: Some(1_000),
                        scene_reference: Some(101),
                        repeat_on_xstar: false,
                        active: false,
                        interactive: false,
                    },
                    StockAuthoredSceneItem {
                        item_id: 1,
                        category: 2,
                        position: RoleVec3 {
                            x: 20.0,
                            y: 0.0,
                            z: 0.0,
                        },
                        half_extents: [0.25; 3],
                        respawn_ms: Some(1_000),
                        scene_reference: Some(102),
                        repeat_on_xstar: false,
                        active: false,
                        interactive: false,
                    },
                ],
                100,
            )
            .unwrap();
        host.send_role_state(sample_state(0.0, 1), 100).unwrap();
        peer.send_role_state(sample_state(0.0, 1), 100).unwrap();
        let hidden = drive(&mut server, &mut [&mut host, &mut peer], 100, 30);
        assert!(server.active_scene_records().is_empty());
        assert!(hidden
            .iter()
            .flatten()
            .all(|event| !matches!(event, StockGameEvent::SceneItemTaken { .. })));

        let op = |reference, kind, value| StockSceneOperation {
            reference,
            kind,
            value,
            extra: 0,
        };

        host.send_scene_ops(
            &StockSceneOpsEvent {
                timestamp_ms: 130,
                operations: vec![op(101, STOCK_SCENE_OP_ACTIVE, 1)],
            },
            130,
        )
        .unwrap();
        drive(&mut server, &mut [&mut host, &mut peer], 130, 30);
        assert!(server.active_scene_records().is_empty());
        host.send_role_state(sample_state(-50.0, 2), 160).unwrap();
        peer.send_role_state(sample_state(-50.0, 2), 160).unwrap();
        drive(&mut server, &mut [&mut host, &mut peer], 160, 30);
        let release = StockSceneOpsEvent {
            timestamp_ms: 190,
            operations: vec![
                op(101, STOCK_SCENE_OP_INTERACTIVE, 1),
                op(102, STOCK_SCENE_OP_ACTIVE, 1),
                op(102, STOCK_SCENE_OP_INTERACTIVE, 1),
            ],
        };
        host.send_scene_ops(&release, 190).unwrap();
        let released = drive(&mut server, &mut [&mut host, &mut peer], 190, 30);
        let records = server.active_scene_records();
        assert_eq!(records.len(), 2);
        for events in &released {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        event,
                        StockGameEvent::SceneItemsGrounded { spawned: true, .. }
                    ))
                    .count(),
                2
            );
        }
        host.send_scene_ops(&release, 220).unwrap();
        drive(&mut server, &mut [&mut host, &mut peer], 220, 30);
        assert_eq!(
            server.active_scene_records(),
            records,
            "repeated active state must retain record identities"
        );
        let broken_box = StockSceneOpsEvent {
            timestamp_ms: 240,
            operations: vec![
                op(200, crate::stock_protocol::STOCK_SCENE_OP_POSE, 2),
                op(200, crate::stock_protocol::STOCK_SCENE_OP_COLLISION, 0),
                op(200, crate::stock_protocol::STOCK_SCENE_OP_VISIBLE, 0),
                op(200, crate::stock_protocol::STOCK_SCENE_OP_DAMAGE, 80),
                op(200, crate::stock_protocol::STOCK_SCENE_OP_EFFECT, 691),
                op(200, crate::stock_protocol::STOCK_SCENE_OP_POSE, 3),
            ],
        };
        host.send_scene_ops(&broken_box, 240).unwrap();
        peer.send_scene_ops(
            &StockSceneOpsEvent {
                timestamp_ms: 241,
                operations: vec![op(201, crate::stock_protocol::STOCK_SCENE_OP_FRAME, 4)],
            },
            241,
        )
        .unwrap();
        host.send_scene_ops(
            &StockSceneOpsEvent {
                timestamp_ms: 241,
                operations: (300..370)
                    .map(|reference| {
                        op(reference, crate::stock_protocol::STOCK_SCENE_OP_VISIBLE, 0)
                    })
                    .collect(),
            },
            241,
        )
        .unwrap();
        drive(&mut server, &mut [&mut host, &mut peer], 240, 10);
        let mut late = connect();
        let joined = drive(
            &mut server,
            &mut [&mut host, &mut peer, &mut late],
            250,
            100,
        );
        assert!(joined[2].iter().any(|event| matches!(event,
            StockGameEvent::SceneItemsGrounded { records: observed, spawned: false } if observed == &records)));
        let late_addr = late.transport().local_addr().unwrap();
        assert!(server.ready_peers.remove(&late_addr));
        let bootstrap_ops = joined[2]
            .iter()
            .filter_map(|event| match event {
                StockGameEvent::SceneOps(event) => Some(event.operations.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        assert!(bootstrap_ops.contains(&op(200, crate::stock_protocol::STOCK_SCENE_OP_POSE, 3)));
        assert!(bootstrap_ops.contains(&op(
            200,
            crate::stock_protocol::STOCK_SCENE_OP_COLLISION,
            0
        )));
        assert!(bootstrap_ops.contains(&op(200, crate::stock_protocol::STOCK_SCENE_OP_VISIBLE, 0)));
        assert!(bootstrap_ops.contains(&op(200, crate::stock_protocol::STOCK_SCENE_OP_DAMAGE, 80)));
        assert!(bootstrap_ops.contains(&op(201, crate::stock_protocol::STOCK_SCENE_OP_FRAME, 4)));
        assert!(!bootstrap_ops.contains(&op(200, crate::stock_protocol::STOCK_SCENE_OP_POSE, 2)));
        assert!(!bootstrap_ops
            .iter()
            .any(|op| op.kind == crate::stock_protocol::STOCK_SCENE_OP_EFFECT));
        assert!(bootstrap_ops.contains(&op(101, STOCK_SCENE_OP_ACTIVE, 1)));
        assert!(bootstrap_ops.contains(&op(101, STOCK_SCENE_OP_INTERACTIVE, 1)));
        assert_eq!(
            bootstrap_ops
                .iter()
                .filter(|op| (300..370).contains(&op.reference))
                .count(),
            70,
            "late-join replay must cross the event-56 packet boundary"
        );

        host.send_role_state(sample_state(0.0, 3), 350).unwrap();
        peer.send_role_state(sample_state(0.0, 3), 350).unwrap();
        let picked = drive(&mut server, &mut [&mut host, &mut peer, &mut late], 350, 30);
        for events in &picked {
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event,
                StockGameEvent::SceneItemTaken { event, record: Some(_) }
                    if event.category == 1 && event.item_id == 5))
                    .count(),
                1,
                "all clients must resolve exactly one claim, including its grounded record"
            );
        }
        assert_eq!(
            server
                .roles
                .values()
                .filter(|role| role.weapon_slots.contains(&Some(5)))
                .count(),
            1
        );
        let mut after_pickup = connect();
        let joined_after_pickup = drive(
            &mut server,
            &mut [&mut host, &mut peer, &mut late, &mut after_pickup],
            380,
            30,
        );
        assert!(!joined_after_pickup[3].iter().any(|event| matches!(event,
            StockGameEvent::SceneItemsGrounded { records, .. }
                if records.iter().any(|record| record.category == 1 && record.item_id == 5))));
        let claimed_ops = joined_after_pickup[3]
            .iter()
            .filter_map(|event| match event {
                StockGameEvent::SceneOps(event) => Some(event.operations.as_slice()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        assert!(claimed_ops.contains(&op(101, STOCK_SCENE_OP_ACTIVE, 0)));
        assert!(claimed_ops.contains(&op(101, STOCK_SCENE_OP_INTERACTIVE, 0)));

        peer.send_scene_ops(
            &StockSceneOpsEvent {
                timestamp_ms: 380,
                operations: vec![
                    op(101, STOCK_SCENE_OP_ACTIVE, 1),
                    op(101, STOCK_SCENE_OP_INTERACTIVE, 1),
                ],
            },
            410,
        )
        .unwrap();
        drive(
            &mut server,
            &mut [&mut host, &mut peer, &mut late, &mut after_pickup],
            410,
            20,
        );
        assert!(server
            .active_scene_records()
            .iter()
            .all(|record| record.category == 2));
        assert!(server
            .scene_items
            .values()
            .find(|item| item.seed.category == 1)
            .unwrap()
            .respawn_at_ms
            .is_some());
        for role in server.roles.values_mut() {
            role.health = 1_000.0;
        }
        peer.send_role_state(sample_state(20.0, 4), 430).unwrap();
        let healed = drive(
            &mut server,
            &mut [&mut host, &mut peer, &mut late, &mut after_pickup],
            430,
            30,
        );
        for observed in &healed {
            assert_eq!(observed.iter().filter(|event| matches!(event,
                StockGameEvent::SceneItemTaken { event, record: Some(_) }
                    if event.category == 2 && event.item_id == 1 && Some(event.actor_id) == peer.actor_id()
            )).count(), 1, "food pickup must reach the host, collector and other observer");
        }
        assert_eq!(
            server
                .roles
                .values()
                .find(|role| Some(role.actor_id) == peer.actor_id())
                .unwrap()
                .health,
            1_300.0
        );
        server.reset_roles_for_round(500);
        assert!(
            server.scene_state.is_empty(),
            "next round must not replay the broken box"
        );
        assert!(
            server.active_scene_records().is_empty(),
            "new-round boxes must contain hidden items again"
        );
        assert!(server
            .scene_items
            .values()
            .all(|item| item.respawn_at_ms.is_none()));
    }

    #[test]
    fn stock_sn21_star_reopens_only_authored_star_items_after_pickup() {
        use crate::stock_protocol::{
            StockSceneOperation, STOCK_EFFECT_XSTAR, STOCK_SCENE_OP_EFFECT, STOCK_SCENE_OP_POSE,
        };
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "sn21").unwrap();
        let mut server = StockGameServer::new(
            transport,
            StockHeroCatalog::from_checksums([0x1234_5678; STOCK_HERO_COUNT]),
        );
        server.set_stage_id("sn21");
        let seed = |item_id, repeat_on_xstar| StockAuthoredSceneItem {
            item_id,
            category: 2,
            position: RoleVec3::ZERO,
            half_extents: [0.25; 3],
            respawn_ms: Some(99_999_000),
            scene_reference: Some(item_id),
            repeat_on_xstar,
            active: false,
            interactive: false,
        };
        server
            .replace_authored_scene_items(vec![seed(28, true), seed(33, true), seed(32, false)], 0)
            .unwrap();
        let peer = "127.0.0.1:3000".parse().unwrap();
        let other = "127.0.0.1:3001".parse().unwrap();
        let mut event = StockSceneOpsEvent {
            timestamp_ms: 1,
            operations: vec![
                StockSceneOperation {
                    reference: 9,
                    kind: STOCK_SCENE_OP_EFFECT,
                    value: STOCK_EFFECT_XSTAR,
                    extra: 0,
                },
                StockSceneOperation {
                    reference: 9,
                    kind: STOCK_SCENE_OP_POSE,
                    value: 1,
                    extra: 0,
                },
            ],
        };
        let mut events = Vec::new();
        server
            .refresh_xstar_items(peer, &event, 100, &mut events)
            .unwrap();
        let first = server.active_scene_records();
        assert_eq!(
            first
                .iter()
                .map(|record| record.item_id)
                .collect::<Vec<_>>(),
            [28, 33]
        );
        let item = server.scene_items.get_mut(&first[0].record_id).unwrap();
        item.active = false;
        item.respawn_at_ms = Some(99_999_000);
        server
            .refresh_xstar_items(peer, &event, 200, &mut events)
            .unwrap();
        server
            .refresh_xstar_items(other, &event, 250, &mut events)
            .unwrap();
        assert_eq!(server.active_scene_records(), [first[1]]);
        event.timestamp_ms += 5_000;
        server
            .refresh_xstar_items(other, &event, 5_101, &mut events)
            .unwrap();
        let second = server.active_scene_records();
        assert_eq!(second.len(), 2);
        assert_eq!(second[1].item_id, 28);
        assert_ne!(second[1].record_id, first[0].record_id);
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn stock_host_bootstraps_takes_and_respawns_authored_scene_item() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "items").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        let frame = StockSceneFrame([100.0, 200.0, 50.0]);
        server.set_scene_frame(frame);
        let mut client = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 0,
                hero_checksum: checksums[0],
            },
            6,
        );
        drive(&mut server, &mut [&mut client], 0, 100);
        server
            .replace_authored_scene_items(
                vec![StockAuthoredSceneItem {
                    item_id: 1,
                    category: 2,
                    position: RoleVec3::ZERO,
                    half_extents: [0.25; 3],
                    respawn_ms: Some(50),
                    scene_reference: None,
                    repeat_on_xstar: false,
                    active: true,
                    interactive: true,
                }],
                100,
            )
            .unwrap();
        let bootstrap = drive(&mut server, &mut [&mut client], 100, 20);
        let record = bootstrap[0]
            .iter()
            .find_map(|event| match event {
                StockGameEvent::SceneItemsGrounded {
                    records,
                    spawned: false,
                } => records.first().copied(),
                _ => None,
            })
            .expect("event 29 authored item");
        assert_eq!(record.position_tenths, [1000, 2000, 500]);
        server.roles.values_mut().next().unwrap().health = 1_000.0;

        client
            .send_role_state(frame.to_stock(sample_state(0.0, 1)), 120)
            .unwrap();
        let picked = drive(&mut server, &mut [&mut client], 120, 30);
        assert!(picked[0].iter().any(|event| matches!(
            event,
            StockGameEvent::SceneItemTaken {
                event: StockSceneTakenEvent {
                    actor_id,
                    record_id,
                    category: 2,
                    item_id: 1,
                },
                record: Some(observed),
            } if Some(*actor_id) == client.actor_id()
                && *record_id == record.record_id
                && *observed == record
        )));
        assert_eq!(server.roles.values().next().unwrap().health, 1_300.0);

        client
            .send_role_state(frame.to_stock(sample_state(20.0, 2)), 151)
            .unwrap();
        let respawned = drive(&mut server, &mut [&mut client], 151, 100);
        assert!(respawned[0].iter().any(|event| matches!(
            event,
            StockGameEvent::SceneItemsGrounded { records, spawned: true }
                if records.len() == 1
                    && records[0].category == 2
                    && records[0].item_id == 1
                    && records[0].record_id != record.record_id
        )));
    }

    #[test]
    fn stock_room_item_rules_filter_only_authored_meteor_res_groups() {
        assert_eq!(
            stock_room_item_group(1, 6),
            Some(StockRoomItemGroup::Health)
        );
        assert_eq!(
            stock_room_item_group(1, 5),
            Some(StockRoomItemGroup::Update)
        );
        assert_eq!(
            stock_room_item_group(1, 3),
            Some(StockRoomItemGroup::Special)
        );
        assert_eq!(stock_room_item_group(1, 1), Some(StockRoomItemGroup::Hide));
        assert_eq!(stock_room_item_group(1, 4), None);
        assert_eq!(
            stock_room_item_group(2, 1),
            Some(StockRoomItemGroup::Health)
        );
        assert_eq!(
            stock_room_item_group(2, 4),
            Some(StockRoomItemGroup::Update)
        );
        assert_eq!(
            stock_room_item_group(2, 6),
            Some(StockRoomItemGroup::Special)
        );
        assert_eq!(stock_room_item_group(4, 2), None);

        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "rules").unwrap();
        let mut server = StockGameServer::new(
            transport,
            StockHeroCatalog::from_checksums([0x1234_5678; STOCK_HERO_COUNT]),
        );
        server.set_item_rules(StockRoomItemRules {
            hide_item: false,
            health_item: false,
            update_item: false,
            special_item: true,
        });
        let seed = |category, item_id| StockAuthoredSceneItem {
            item_id,
            category,
            position: RoleVec3::ZERO,
            half_extents: [0.25; 3],
            respawn_ms: None,
            scene_reference: None,
            repeat_on_xstar: false,
            active: true,
            interactive: true,
        };
        server
            .replace_authored_scene_items(vec![seed(1, 6), seed(2, 1), seed(2, 4), seed(2, 6)], 0)
            .unwrap();
        let authored = server
            .scene_items
            .values()
            .map(|item| (item.seed.category, item.seed.item_id))
            .collect::<Vec<_>>();
        assert_eq!(authored, [(2, 6)]);
    }

    #[test]
    fn stock_room_item_rules_follow_original_scene_registry_keys() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "rules").unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let registry = meteor_asset::res::parse_meteor_res(include_bytes!(
            "../../../../origin_res/Meteor.res"
        ))
        .unwrap();
        server.set_scene_item_registry(&registry);
        let seed = |item_id| StockAuthoredSceneItem {
            item_id,
            category: 1,
            position: RoleVec3::ZERO,
            half_extents: [0.25; 3],
            respawn_ms: None,
            scene_reference: None,
            repeat_on_xstar: false,
            active: true,
            interactive: true,
        };
        let seeds = vec![seed(1), seed(4), seed(6)];
        let ids = |server: &StockGameServer| {
            server
                .scene_items
                .values()
                .map(|item| item.seed.item_id)
                .collect::<Vec<_>>()
        };
        server.set_item_rules(StockRoomItemRules {
            hide_item: false,
            ..Default::default()
        });
        server
            .replace_authored_scene_items(seeds.clone(), 0)
            .unwrap();
        assert_eq!(ids(&server), [4, 6]);
        server.set_item_rules(StockRoomItemRules {
            health_item: false,
            ..Default::default()
        });
        server.replace_authored_scene_items(seeds, 1).unwrap();
        assert_eq!(ids(&server), [1, 4]);
    }

    #[test]
    fn stock_request20_creates_event31_runtime_record() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "drop").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        server.set_item_rules(StockRoomItemRules {
            hide_item: false,
            health_item: false,
            update_item: false,
            special_item: false,
        });
        let mut client = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 0,
                hero_checksum: checksums[0],
            },
            6,
        );
        drive(&mut server, &mut [&mut client], 0, 100);
        let request = StockSceneItemSpawnRequest::from_role_position(
            5,
            1,
            RoleVec3 {
                x: 20.0,
                y: 0.0,
                z: 5.0,
            },
        );
        client.send_scene_item_spawn(request, 100).unwrap();
        let output = drive(&mut server, &mut [&mut client], 100, 30);
        assert!(output[0].iter().any(|event| matches!(
            event,
            StockGameEvent::SceneItemsGrounded { records, spawned: true }
                if records.len() == 1
                    && records[0].item_id == 5
                    && records[0].category == 1
                    && records[0].role_position() == request.role_position()
        )));
    }

    #[test]
    fn stock_dynamic_pickup_uses_a_strict_sphere_without_changing_authored_bounds() {
        let radius = STOCK_SCENE_PICKUP_RADIUS;
        let mut item = StockHostedSceneItem {
            seed: StockAuthoredSceneItem {
                item_id: 5,
                category: 1,
                position: RoleVec3 {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                },
                half_extents: [0.0; 3],
                respawn_ms: None,
                scene_reference: None,
                repeat_on_xstar: false,
                active: true,
                interactive: true,
            },
            record_id: 1,
            active: true,
            script_active: true,
            interactive: true,
            respawn_at_ms: None,
            expires_at_ms: Some(30_000),
            authored: false,
        };
        let diagonal = RoleVec3 {
            x: radius * 0.8,
            y: 0.0,
            z: radius * 0.8,
        };
        assert!(!role_touches_scene_item(diagonal, &item));
        assert!(!role_touches_scene_item(
            RoleVec3 {
                x: radius,
                y: 0.0,
                z: 0.0
            },
            &item
        ));
        assert!(role_touches_scene_item(
            RoleVec3 {
                x: 0.0,
                y: radius * 0.99,
                z: 0.0
            },
            &item
        ));
        assert!(!role_touches_scene_item(
            RoleVec3 {
                x: 0.0,
                y: radius,
                z: 0.0
            },
            &item
        ));
        item.authored = true;
        item.expires_at_ms = None;
        assert!(role_touches_scene_item(diagonal, &item));
        assert!(!role_touches_scene_item(
            RoleVec3 {
                x: 0.0,
                y: STOCK_SCENE_PICKUP_HEIGHT,
                z: 0.0
            },
            &item
        ));
        assert!(role_touches_scene_item(
            RoleVec3 {
                x: 0.0,
                y: -STOCK_SCENE_PICKUP_HEIGHT,
                z: 0.0
            },
            &item
        ));
    }

    #[test]
    fn stock_dynamic_drop_expires_after_thirty_seconds_without_a_pickup() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "expiry").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        drive(&mut server, &mut [&mut client], 0, 100);
        let peer = server.transport.peer_addresses()[0];
        let request = StockSceneItemSpawnRequest::from_role_position(
            5,
            1,
            RoleVec3 {
                x: 100.0,
                y: 0.0,
                z: 100.0,
            },
        );
        server
            .handle_application(
                peer,
                STOCK_REQUEST_SPAWN_SCENE_ITEM,
                &request.encode(),
                100,
                &mut Vec::new(),
            )
            .unwrap();
        drive(&mut server, &mut [&mut client], 101, 30);
        let record_id = *server.scene_items.keys().next().unwrap();
        assert!(client.scene_items.contains_key(&record_id));

        let mut authored = server.scene_items[&record_id];
        authored.record_id = 900;
        authored.authored = true;
        authored.expires_at_ms = None;
        server.scene_items.insert(900, authored);
        for now in (5_000..=30_000).step_by(5_000) {
            drive(&mut server, &mut [&mut client], now, 3);
        }
        server.update_scene_items(30_100, &mut Vec::new()).unwrap();
        assert!(
            server.scene_items.contains_key(&record_id),
            "the EXE compares strictly greater than 30s"
        );
        let events = drive(&mut server, &mut [&mut client], 30_101, 100);
        assert!(events[0].iter().any(|event| matches!(event,
            StockGameEvent::SceneItemRemoved(removed)
                if removed.record_id == record_id && removed.category == 1 && removed.item_id == 5)));
        assert!(events[0]
            .iter()
            .all(|event| !matches!(event, StockGameEvent::SceneItemTaken { .. })));
        assert!(!server.scene_items.contains_key(&record_id));
        assert!(!client.scene_items.contains_key(&record_id));
        assert!(server.scene_items.contains_key(&900));
    }

    #[test]
    fn stock_item15_and_item16_pulse_without_an_instant_award_or_refresh_reset() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "periodic").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
        let mut client = StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        drive(&mut server, &mut [&mut client], 0, 100);
        let peer = server.transport.peer_addresses()[0];
        let role = server.roles.get_mut(&peer).unwrap();
        role.health = 1000.0;
        role.anger = 0.0;
        apply_stock_item_effect(role, 32, 100);
        apply_stock_item_effect(role, 33, 100);
        assert_eq!((role.health, role.anger), (1000.0, 0.0));
        assert_eq!(role.active_item_effects[&32].expires_at_ms, 10_000_000);
        server.update_item_effects(299, &mut Vec::new()).unwrap();
        assert_eq!(server.roles[&peer].health, 1000.0);
        server.update_item_effects(300, &mut Vec::new()).unwrap();
        assert_eq!(
            (server.roles[&peer].health, server.roles[&peer].anger),
            (1050.0, 1.0)
        );
        let role = server.roles.get_mut(&peer).unwrap();
        apply_stock_item_effect(role, 32, 350);
        apply_stock_item_effect(role, 33, 350);
        assert_eq!(role.active_item_effects[&32].next_tick_ms, Some(500));
        assert_eq!(role.active_item_effects.len(), 2);
        server.update_item_effects(500, &mut Vec::new()).unwrap();
        assert_eq!(
            (server.roles[&peer].health, server.roles[&peer].anger),
            (1100.0, 2.0)
        );
        let role = server.roles.get_mut(&peer).unwrap();
        role.health = role.max_health - 10.0;
        role.anger = 99.0;
        server.update_item_effects(700, &mut Vec::new()).unwrap();
        assert_eq!(
            (server.roles[&peer].health, server.roles[&peer].anger),
            (server.roles[&peer].max_health, 100.0)
        );
    }

    #[test]
    fn stock_host_item_effects_update_vitals_combat_bonus_and_expire() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "effects").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        let mut client = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 0,
                hero_checksum: checksums[0],
            },
            6,
        );
        drive(&mut server, &mut [&mut client], 0, 100);
        let role = server.roles.values_mut().next().unwrap();
        role.health = 1_000.0;
        role.anger = 10.0;
        apply_stock_item_effect(role, 2, 100);
        apply_stock_item_effect(role, 4, 100);
        apply_stock_item_effect(role, 18, 100);
        assert_eq!((role.health, role.max_health), (1_000.0, 2_500.0));
        assert_eq!(role.item_attack_bonus(), 25);

        server.update_item_effects(5_100, &mut Vec::new()).unwrap();
        let role = server.roles.values().next().unwrap();
        assert_eq!(role.health, 990.0);
        assert_eq!(role.item_attack_bonus(), 25);

        server
            .update_item_effects(600_100, &mut Vec::new())
            .unwrap();
        let role = server.roles.values().next().unwrap();
        assert_eq!((role.health, role.max_health), (940.0, 2_000.0));
        assert_eq!(role.item_attack_bonus(), 0);
        assert!(role.active_item_effects.is_empty());
    }

    #[test]
    fn stock_gun_ultimate_uses_both_equipped_records_and_accepts_ray_and_effect() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "gun").unwrap();
        let address = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let resources =
            meteor_asset::res::parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap())
                .unwrap()
                .combat_table();
        let mut server = StockGameServer::with_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            resources,
        );
        let connect = |hero| {
            StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero,
                    camp: 0,
                    hero_checksum: checksums[hero as usize],
                },
                39,
            )
        };
        let mut attacker = connect(0);
        let mut target = connect(1);
        drive(&mut server, &mut [&mut attacker, &mut target], 0, 100);
        let target_id = target.actor_id().unwrap();
        let hit = StockHitRequest {
            target_actor_id: target_id,
            hit_word: 217,
            target_pose_word: 95,
            moves_word: 0,
            values_word: 0,
        };
        for (now, remaining) in [(2_100, 1_780.0), (2_150, 1_560.0)] {
            attacker.send_hit(hit, now).unwrap();
            drive(&mut server, &mut [&mut attacker, &mut target], now, 20);
            let role = server
                .roles
                .values()
                .find(|role| role.actor_id == target_id)
                .unwrap();

            assert_eq!(role.health, remaining);
        }
    }

    #[test]
    fn stock_g_and_h_weapon_paths_preserve_distinct_slot_and_event55_semantics() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "drop keys").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        let mut client = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 0,
                hero_checksum: checksums[0],
            },
            6,
        );
        drive(&mut server, &mut [&mut client], 0, 100);
        let actor_id = client.actor_id().unwrap();
        let role = server.roles.values_mut().next().unwrap();
        role.weapon_slots = [Some(6), Some(5)];
        role.weapon_item_id = Some(6);

        let g = StockWeaponDropCommand {
            item_id: 5,
            position: RoleVec3::ZERO,
            forward: RoleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            copy_only: false,
            remaining_item_id: Some(6),
        };
        assert!(matches!(
            client.send_weapon_drop(g, 100).unwrap(),
            Some(StockGameEvent::WeaponThrown(StockWeaponThrowEvent {
                actor_id: observed,
                item_id: 5,
                ..
            })) if observed == actor_id
        ));
        let output = drive(&mut server, &mut [&mut client], 100, 30);
        assert!(output[0].iter().any(|event| matches!(
            event,
            StockGameEvent::RawApplication {
                opcode: STOCK_EVENT_WEAPON_OBJECT,
                payload,
                ..
            } if payload.len() == 36
        )));
        assert_eq!(
            server.roles.values().next().unwrap().weapon_item_id,
            Some(6)
        );
        assert_eq!(
            server.roles.values().next().unwrap().weapon_slots,
            [Some(6), None]
        );

        let flying = drive(&mut server, &mut [&mut client], 130, 500);
        assert!(!flying[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::WeaponSettled { .. })));
        let throw_id = client.pending_weapon_drops[0].throw.throw_id;
        let wall = RoleVec3 {
            x: 0.0,
            y: 5.0,
            z: 1.0,
        };
        assert!(client
            .confirm_weapon_drop_collision(throw_id, wall, 630)
            .unwrap());
        assert!(!client
            .confirm_weapon_drop_collision(throw_id, wall, 631)
            .unwrap());
        assert!(!client
            .confirm_weapon_drop_collision(u32::MAX, wall, 631)
            .unwrap());
        let settled = drive(&mut server, &mut [&mut client], 630, 100);
        assert!(settled[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::WeaponSettled { .. })));
        assert!(settled[0].iter().any(|event| matches!(
            event,
            StockGameEvent::SceneItemsGrounded { records, spawned: true }
                if records.iter().any(|record| record.item_id == 5 && record.category == 1 && record.position_tenths == StockSceneItemSpawnRequest::from_role_position(5, 1, wall).position_tenths)
        )));

        let h = StockWeaponDropCommand {
            item_id: 6,
            position: RoleVec3::ZERO,
            forward: RoleVec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
            copy_only: true,
            remaining_item_id: Some(6),
        };
        assert_eq!(client.send_weapon_drop(h, 5_230).unwrap(), None);
        let copied = drive(&mut server, &mut [&mut client], 5_230, 30);
        assert!(copied[0].iter().any(|event| matches!(
            event,
            StockGameEvent::WeaponThrown(StockWeaponThrowEvent {
                actor_id: observed,
                item_id: 6,
                ..
            }) if *observed == actor_id
        )));
        assert_eq!(
            server.roles.values().next().unwrap().weapon_item_id,
            Some(6),
            "H copies the active weapon without consuming its slot"
        );

        let copy_id = client.pending_weapon_drops[0].throw.throw_id;
        assert!(!client
            .confirm_weapon_drop_collision(
                copy_id,
                RoleVec3 {
                    x: f32::NAN,
                    y: 0.0,
                    z: 0.0
                },
                5_259
            )
            .unwrap());
        assert!(client
            .confirm_weapon_drop_collision(copy_id, wall, 5_259)
            .unwrap());
        drive(&mut server, &mut [&mut client], 5_259, 1);
        {
            let role = server.roles.values_mut().next().unwrap();
            role.weapon_slots = [Some(5), Some(6)];
            role.weapon_item_id = Some(5);
        }
        client.send_weapon_switch(6, 5_260).unwrap();
        drive(&mut server, &mut [&mut client], 5_260, 30);
        assert_eq!(
            server.roles.values().next().unwrap().weapon_item_id,
            Some(6),
            "request 6 commits the alternate slot"
        );
        let mut switched_drop = g;
        switched_drop.item_id = 5;
        switched_drop.remaining_item_id = Some(6);
        client.send_weapon_drop(switched_drop, 5_290).unwrap();
        drive(&mut server, &mut [&mut client], 5_290, 30);
        let role = server.roles.values().next().unwrap();
        assert_eq!(role.weapon_slots, [Some(6), None]);
        assert_eq!(
            role.weapon_item_id,
            Some(6),
            "E then G still drops the opposite slot"
        );
        let unlanded_id = client.pending_weapon_drops[0].throw.throw_id;
        drive(&mut server, &mut [&mut client], 10_300, 20);
        assert!(client.pending_weapon_drops.is_empty());
        assert!(!client
            .confirm_weapon_drop_collision(unlanded_id, wall, 10_321)
            .unwrap());
    }

    #[test]
    fn stock_weapon_switch_broadcasts_item_identity_to_observer() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "equip").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        let make_client = || {
            StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    server_addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero: 0,
                    camp: 0,
                    hero_checksum: checksums[0],
                },
                6,
            )
        };
        let mut owner = make_client();
        let mut observer = make_client();
        drive(&mut server, &mut [&mut owner, &mut observer], 0, 100);
        let actor_id = owner.actor_id().unwrap();

        server
            .roles
            .values_mut()
            .find(|role| role.actor_id == actor_id)
            .unwrap()
            .weapon_slots = [Some(6), Some(14)];
        for (time, item, spare) in [(100, 14, 6), (150, 6, 14), (200, 14, 6)] {
            owner.send_weapon_switch(item, time).unwrap();
            let events = drive(&mut server, &mut [&mut owner, &mut observer], time, 30);
            assert!(events[1].iter().any(|event| matches!(event,
                StockGameEvent::WeaponEquipped(e) if e.actor_id == actor_id && e.weapon_item_id == item
            )), "observer must receive reliable equipment identity");
            assert_eq!(observer.weapons[&actor_id], item);
            let role = server
                .roles
                .values()
                .find(|role| role.actor_id == actor_id)
                .unwrap();
            assert_eq!(role.weapon_item_id, Some(item));
            assert_eq!(role.weapon_slots, [Some(item), Some(spare)]);
        }
    }

    #[test]
    fn stock_periodic_item_death_has_no_killer_and_uses_rule_respawn() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "poison").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        let mut client = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 0,
                hero_checksum: checksums[0],
            },
            6,
        );
        drive(&mut server, &mut [&mut client], 0, 100);
        let actor_id = client.actor_id().unwrap();
        let role = server.roles.values_mut().next().unwrap();
        role.health = 10.0;
        apply_stock_item_effect(role, 18, 100);

        let died = drive(&mut server, &mut [&mut client], 5_100, 50);
        assert!(died[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::LocalDeath)));
        assert!(!died[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::Kill(_))));
        let role = server.roles.values().next().unwrap();
        assert_eq!((role.health, role.deaths), (0.0, 1));
        assert_eq!(role.respawn_at_ms, Some(8_100));

        let revived = drive(&mut server, &mut [&mut client], 8_100, 50);
        assert!(revived[0].iter().any(|event| matches!(
            event,
            StockGameEvent::Respawn(StockRespawnEvent { actor_id: observed })
                if *observed == actor_id
        )));
    }

    #[test]
    fn rule1_timeout_advances_round_count_without_team_score() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "timeout").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        server.set_round_limit(1);
        server.set_round_time_minutes(1);
        let mut client = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 0,
                hero_checksum: checksums[0],
            },
            6,
        );
        drive(&mut server, &mut [&mut client], 0, 80);
        assert_eq!(server.ready_peer_count(), 1);

        for now in (5_000..=55_000).step_by(5_000) {
            drive(&mut server, &mut [&mut client], now, 1);
        }
        let output = drive(&mut server, &mut [&mut client], 60_100, 500);
        assert!(
            output[0].iter().any(|event| matches!(
                event,
                StockGameEvent::TeamScore(StockTeamScoreEvent {
                    team1_wins: 0,
                    team2_wins: 0,
                })
            )),
            "events={:?}, server_rate={}, client_rate={}",
            output[0],
            server.role_rate(),
            client.role_rate()
        );
        assert!(output[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::MatchEnd)));
        assert_eq!(server.team_wins(), [0, 0]);
    }

    #[test]
    fn network_round_settlement_delays_respawn_and_rejects_gameplay_for_ten_seconds() {
        for rule in [1, 5] {
            let transport =
                StockServerTransport::bind("127.0.0.1:0", "serverlist", "settlement").unwrap();
            let address = transport.local_addr().unwrap();
            let mut server = StockGameServer::with_rule_and_combat_resources(
                transport,
                StockHeroCatalog::bundled_revision(),
                rule,
                CombatResourceTable::default(),
            );
            server.set_spawn_pad_counts([16, 8, 8]);
            server.set_round_limit(2);
            server.set_round_time_minutes(1);
            let mut a = StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                0,
                if rule == 1 { 0 } else { 1 },
                6,
            )
            .unwrap();
            let mut b = StockGameClient::for_bundled_revision(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    address,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                1,
                if rule == 1 { 0 } else { 2 },
                6,
            )
            .unwrap();
            drive(&mut server, &mut [&mut a, &mut b], 0, 100);
            for now in (5_000..=55_000).step_by(5_000) {
                drive(&mut server, &mut [&mut a, &mut b], now, 10);
            }
            let victim = b.actor_id().unwrap();
            server
                .roles
                .values_mut()
                .find(|r| r.actor_id == victim)
                .unwrap()
                .health = 0.0;
            let mut decisions = Vec::new();
            if rule == 5 {
                server
                    .complete_round(false, true, 60_000, &mut decisions)
                    .unwrap();
            } else {
                server.round_started_ms = Some(0);
                server
                    .finish_round_on_timeout(60_000, &mut decisions)
                    .unwrap();
            }
            assert!(decisions
                .iter()
                .any(|e| matches!(e, StockGameEvent::RoundEnd)));
            assert_eq!(server.round_count, 1);
            let ended = drive(&mut server, &mut [&mut a, &mut b], 60_001, 20);
            for events in &ended {
                assert!(events.iter().any(|e| matches!(e, StockGameEvent::RoundEnd)));
                assert!(!events
                    .iter()
                    .any(|e| matches!(e, StockGameEvent::RoundRestart)));
            }
            a.send_role_state(sample_state(123.0, 1), 65_000).unwrap();
            let waiting = drive(&mut server, &mut [&mut a, &mut b], 65_000, 20);
            assert!(waiting
                .iter()
                .flatten()
                .all(|e| !matches!(e, StockGameEvent::RoundRestart)));
            assert_eq!(
                server
                    .roles
                    .values()
                    .find(|r| r.actor_id == victim)
                    .unwrap()
                    .health,
                0.0
            );
            assert!(server.roles.values().all(|r| r.state.is_none()));
            assert_eq!(
                server.round_count, 1,
                "settlement must not re-evaluate the same round"
            );
            let restarted = drive(&mut server, &mut [&mut a, &mut b], 70_050, 30);
            for events in &restarted {
                assert_eq!(
                    events
                        .iter()
                        .filter(|e| matches!(e, StockGameEvent::RoundRestart))
                        .count(),
                    1
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|e| matches!(e, StockGameEvent::SpawnPad(Some(_))))
                        .count(),
                    1
                );
            }
            assert!(!server.round_ended);
            assert!(server.roles.values().all(|r| r.health == r.max_health));
            assert_eq!(server.team_wins, if rule == 5 { [1, 0] } else { [0, 0] });
        }
    }

    #[test]
    fn request33_feedback_moves_room_rate_and_event64_updates_client_timer() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "pacing").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server =
            StockGameServer::new(transport, StockHeroCatalog::from_checksums(checksums));
        let mut client = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 0,
                hero_checksum: checksums[0],
            },
            6,
        );
        drive(&mut server, &mut [&mut client], 0, 100);
        assert_eq!((server.role_rate(), client.role_rate()), (16, 16));

        let mut seen = Vec::new();
        for now in (100..=15_000).step_by(100) {
            seen.extend(drive(&mut server, &mut [&mut client], now, 1).remove(0));
        }

        seen.extend(drive(&mut server, &mut [&mut client], 15_001, 30).remove(0));

        assert_eq!(server.role_rate(), 17);
        assert_eq!(client.role_rate(), 17);
        assert!(seen
            .iter()
            .any(|event| matches!(event, StockGameEvent::RoleRate(17))));
    }

    #[test]
    fn rule2_pickup_and_safe_zone_delivery_end_the_round() {
        for (custom, stage, camp) in [
            (false, "sn03", 1),
            (true, "sn03", 1),
            (false, "sn02", 1),
            (false, "sn02", 2),
        ] {
            let definition = std::sync::Arc::new(if stage == "sn02" {
                crate::escort_definition::tests::original_definition(stage)
            } else {
                crate::escort_definition::tests::authored_definition(custom)
            });
            let transport =
                StockServerTransport::bind("127.0.0.1:0", "serverlist", "escort").unwrap();
            let server_addr = transport.local_addr().unwrap();
            let checksums = [0x1234_5678; STOCK_HERO_COUNT];
            let mut server = StockGameServer::with_rule_and_combat_resources(
                transport,
                StockHeroCatalog::from_checksums(checksums),
                2,
                CombatResourceTable::default(),
            );
            server.set_round_limit(1);
            server.set_escort_definition(definition.clone());
            let mut client1 = StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    server_addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero: 0,
                    camp,
                    hero_checksum: checksums[0],
                },
                6,
            );
            let mut client2 = StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    server_addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero: 1,
                    camp: 3 - camp,
                    hero_checksum: checksums[1],
                },
                5,
            );

            let entered = drive(&mut server, &mut [&mut client1, &mut client2], 0, 100);
            for events in entered {
                assert!(events.iter().any(|event| matches!(
                    event,
                    StockGameEvent::EscortGrounded {
                        record,
                        dropped: false,
                    } if is_stock_escort_record(record)
                        && record.role_position() == StockSceneItemRecord::from_role_position(
                            record.record_id,
                            definition.item_id,
                            STOCK_ESCORT_CATEGORY,
                            definition.stage.flag_home,
                        ).role_position()
                )));
            }

            let actor1 = client1.actor_id().unwrap();
            let mut home_state = sample_state(0.0, 1);
            home_state.position = definition.stage.flag_home;
            if custom {
                let vertex = definition
                    .vertices
                    .iter()
                    .max_by(|a, b| {
                        (a[0] * a[0] + a[2] * a[2]).total_cmp(&(b[0] * b[0] + b[2] * b[2]))
                    })
                    .unwrap();
                home_state.position.x += vertex[0];
                home_state.position.y += vertex[1] - 0.96;
                home_state.position.z += vertex[2];
            }
            let mut far_state = sample_state(0.0, 1);
            far_state.position.x = definition.stage.flag_home.x + 100.0;
            client1.send_role_state(home_state, 200).unwrap();
            client2.send_role_state(far_state, 200).unwrap();
            let picked_up = drive(&mut server, &mut [&mut client1, &mut client2], 200, 100);
            assert_eq!(server.escort_carrier(), Some(actor1));
            for events in picked_up {
                assert!(events.iter().any(|event| matches!(
                    event,
                    StockGameEvent::EscortTaken(StockSceneTakenEvent { actor_id, .. })
                        if *actor_id == actor1
                )));
            }

            let zone = definition.stage.safe_zones[0];
            let mut delivered_state = home_state;
            delivered_state.position = crate::role_state::RoleVec3 {
                x: zone.center_x,
                y: zone.center_y,
                z: zone.center_z,
            };
            delivered_state.state_words.state_word1 = 2;
            delivered_state.state_words.pose_sequence = 2;
            client1.send_role_state(delivered_state, 400).unwrap();
            let early = drive(&mut server, &mut [&mut client1, &mut client2], 400, 100);
            assert!(
                !server.round_ended,
                "delivery is gated for the first three seconds"
            );
            assert!(early
                .iter()
                .flatten()
                .all(|event| !matches!(event, StockGameEvent::MatchEnd)));
            let delivered = drive(&mut server, &mut [&mut client1, &mut client2], 3_100, 100);
            for events in delivered {
                assert!(events.iter().any(|event| matches!(
                    event,
                    StockGameEvent::TeamScore(StockTeamScoreEvent {
                        team1_wins,
                        team2_wins,
                    }) if *team1_wins == u32::from(camp == 1) && *team2_wins == u32::from(camp == 2)
                )));
                assert!(events
                    .iter()
                    .any(|event| matches!(event, StockGameEvent::MatchEnd)));
            }
            assert_eq!(
                server.team_wins(),
                [u32::from(camp == 1), u32::from(camp == 2)]
            );
        }
    }

    #[test]
    fn rule2_death_drops_then_timeout_and_disconnect_return_the_flag() {
        for (custom, stage) in [(false, "sn03"), (true, "sn03"), (false, "sn02")] {
            let definition = std::sync::Arc::new(if stage == "sn02" {
                crate::escort_definition::tests::original_definition(stage)
            } else {
                crate::escort_definition::tests::authored_definition(custom)
            });
            let transport =
                StockServerTransport::bind("127.0.0.1:0", "serverlist", "escort lifecycle")
                    .unwrap();
            let server_addr = transport.local_addr().unwrap();
            let checksums = [0x1234_5678; STOCK_HERO_COUNT];
            let mut server = StockGameServer::with_rule_and_combat_resources(
                transport,
                StockHeroCatalog::from_checksums(checksums),
                2,
                CombatResourceTable::default(),
            );
            server.set_escort_definition(definition.clone());
            let mut client1 = StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    server_addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero: 0,
                    camp: 1,
                    hero_checksum: checksums[0],
                },
                6,
            );
            let mut client2 = StockGameClient::new(
                StockClientTransport::bind(
                    "127.0.0.1:0",
                    server_addr,
                    StockConnectRecord::for_open_room(),
                )
                .unwrap(),
                StockEnterRoleRequest {
                    hero: 1,
                    camp: 2,
                    hero_checksum: checksums[1],
                },
                5,
            );
            drive(&mut server, &mut [&mut client1, &mut client2], 0, 100);
            let actor1 = client1.actor_id().unwrap();
            let mut home_state = sample_state(0.0, 1);
            home_state.position = definition.stage.flag_home;
            if custom {
                let vertex = definition
                    .vertices
                    .iter()
                    .max_by(|a, b| {
                        (a[0] * a[0] + a[2] * a[2]).total_cmp(&(b[0] * b[0] + b[2] * b[2]))
                    })
                    .unwrap();
                home_state.position.x += vertex[0];
                home_state.position.y += vertex[1] - 0.96;
                home_state.position.z += vertex[2];
            }
            let mut far_state = sample_state(0.0, 1);
            far_state.position.x = definition.stage.flag_home.x + 100.0;
            client1.send_role_state(home_state, 200).unwrap();
            client2.send_role_state(far_state, 200).unwrap();
            drive(&mut server, &mut [&mut client1, &mut client2], 200, 100);
            assert_eq!(server.escort_carrier(), Some(actor1));

            let role = server
                .roles
                .values_mut()
                .find(|role| role.actor_id == actor1)
                .unwrap();
            role.health = 0.0;
            let dropped = drive(&mut server, &mut [&mut client1, &mut client2], 400, 100);
            assert_eq!(server.escort_carrier(), None);
            for events in dropped {
                assert!(events.iter().any(|event| matches!(
                    event,
                    StockGameEvent::EscortGrounded { dropped: true, .. }
                )));
            }

            let returned = drive(&mut server, &mut [&mut client1, &mut client2], 5_500, 100);
            for events in returned {
                assert!(events
                    .iter()
                    .any(|event| matches!(event, StockGameEvent::EscortReturned)));
            }

            let role = server
                .roles
                .values_mut()
                .find(|role| role.actor_id == actor1)
                .unwrap();
            role.health = role.max_health;
            client1.send_role_state(home_state, 5_700).unwrap();
            drive(&mut server, &mut [&mut client1, &mut client2], 5_700, 100);
            assert_eq!(server.escort_carrier(), Some(actor1));
            client1.transport_mut().disconnect(9).unwrap();
            let disconnected = drive(&mut server, &mut [&mut client2], 5_900, 100);
            assert!(disconnected[0]
                .iter()
                .any(|event| matches!(event, StockGameEvent::EscortReturned)));
            assert_eq!(server.escort_carrier(), None);
        }
    }

    #[test]
    fn rule3_relays_event56_and_objective_uses_event34_match_lifecycle() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "siege").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            3,
            CombatResourceTable::default(),
        );
        server.set_round_limit(1);
        let mut client1 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 1,
                hero_checksum: checksums[0],
            },
            6,
        );
        let mut client2 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 1,
                camp: 2,
                hero_checksum: checksums[1],
            },
            5,
        );
        drive(&mut server, &mut [&mut client1, &mut client2], 0, 100);

        let transaction = StockSceneOpsEvent {
            timestamp_ms: 0x1234_5678,
            operations: vec![
                crate::stock_protocol::StockSceneOperation {
                    reference: 42,
                    kind: crate::stock_protocol::STOCK_SCENE_OP_POSE,
                    value: 8,
                    extra: 0,
                },
                crate::stock_protocol::StockSceneOperation {
                    reference: 42,
                    kind: crate::stock_protocol::STOCK_SCENE_OP_COLLISION,
                    value: 0,
                    extra: 0,
                },
            ],
        };
        client1.send_scene_ops(&transaction, 200).unwrap();
        let relayed = drive(&mut server, &mut [&mut client1, &mut client2], 200, 100);
        assert!(!relayed[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::SceneOps(_))));
        assert!(relayed[1].iter().any(
            |event| matches!(event, StockGameEvent::SceneOps(event) if event == &transaction)
        ));

        server.set_stage_id("custom_siege");
        let host_events = server.complete_siege_objective(2, 401).unwrap();
        assert!(host_events.iter().any(|event| matches!(
            event,
            StockGameEvent::TeamScore(StockTeamScoreEvent {
                team1_wins: 0,
                team2_wins: 1,
            })
        )));
        assert!(host_events
            .iter()
            .any(|event| matches!(event, StockGameEvent::MatchEnd)));
        let ended = drive(&mut server, &mut [&mut client1, &mut client2], 401, 100);
        for events in ended {
            assert!(events.iter().any(|event| matches!(
                event,
                StockGameEvent::TeamScore(StockTeamScoreEvent {
                    team1_wins: 0,
                    team2_wins: 1,
                })
            )));
            assert!(events
                .iter()
                .any(|event| matches!(event, StockGameEvent::MatchEnd)));
        }
        assert_eq!(server.team_wins(), [0, 1]);
    }

    #[test]
    fn rule4_assigns_both_vips_and_vip_death_ends_match() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "assassin").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            4,
            CombatResourceTable::default(),
        );
        server.set_round_limit(1);
        let mut client1 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 1,
                hero_checksum: checksums[0],
            },
            6,
        );
        let mut client2 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 1,
                camp: 2,
                hero_checksum: checksums[1],
            },
            5,
        );

        let entered = drive(&mut server, &mut [&mut client1, &mut client2], 0, 100);
        let actor1 = client1.actor_id().unwrap();
        let actor2 = client2.actor_id().unwrap();
        assert_eq!(server.vips(), [Some(actor1), Some(actor2)]);
        for events in entered {
            assert!(events.iter().any(|event| matches!(
                event,
                StockGameEvent::Vip(StockVipEvent { camp: 1, actor_id })
                    if *actor_id == actor1
            )));
            assert!(events.iter().any(|event| matches!(
                event,
                StockGameEvent::Vip(StockVipEvent { camp: 2, actor_id })
                    if *actor_id == actor2
            )));
        }

        let hit = StockHitRequest {
            target_actor_id: actor2,
            hit_word: 250,
            target_pose_word: 110,
            moves_word: 400,
            values_word: 50,
        };
        let mut score_seen = [false; 2];
        let mut match_end_seen = [false; 2];
        for index in 0..20 {
            let now = 2_100 + index * 250;
            client1.send_hit(hit, now).unwrap();
            let output = drive(&mut server, &mut [&mut client1, &mut client2], now, 20);
            for (client_index, events) in output.iter().enumerate() {
                score_seen[client_index] |= events.iter().any(|event| {
                    matches!(
                        event,
                        StockGameEvent::TeamScore(StockTeamScoreEvent {
                            team1_wins: 1,
                            team2_wins: 0,
                        })
                    )
                });
                match_end_seen[client_index] |= events
                    .iter()
                    .any(|event| matches!(event, StockGameEvent::MatchEnd));
            }
        }
        let output = drive(&mut server, &mut [&mut client1, &mut client2], 7_500, 100);
        for (client_index, events) in output.iter().enumerate() {
            score_seen[client_index] |= events.iter().any(|event| {
                matches!(
                    event,
                    StockGameEvent::TeamScore(StockTeamScoreEvent {
                        team1_wins: 1,
                        team2_wins: 0,
                    })
                )
            });
            match_end_seen[client_index] |= events
                .iter()
                .any(|event| matches!(event, StockGameEvent::MatchEnd));
        }
        assert_eq!(score_seen, [true, true]);
        assert_eq!(match_end_seen, [true, true]);
        assert_eq!(server.team_wins(), [1, 0]);
    }

    #[test]
    fn rule4_vip_cure_sends_request31_and_revives_nearby_dead_ally() {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "cure").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            4,
            CombatResourceTable::default(),
        );
        let mut client1 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 1,
                hero_checksum: checksums[0],
            },
            6,
        );
        let mut client2 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 1,
                camp: 1,
                hero_checksum: checksums[1],
            },
            5,
        );
        let mut client3 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 2,
                camp: 2,
                hero_checksum: checksums[2],
            },
            6,
        );
        drive(
            &mut server,
            &mut [&mut client1, &mut client2, &mut client3],
            0,
            120,
        );
        let actor1 = client1.actor_id().unwrap();
        let actor2 = client2.actor_id().unwrap();
        let actor3 = client3.actor_id().unwrap();
        let healer_actor = server.vips()[0].unwrap();
        let target_actor = if healer_actor == actor1 {
            actor2
        } else {
            actor1
        };

        client1.send_role_state(sample_state(0.0, 1), 200).unwrap();
        client2.send_role_state(sample_state(1.0, 1), 200).unwrap();
        client3.send_role_state(sample_state(5.0, 1), 200).unwrap();
        drive(
            &mut server,
            &mut [&mut client1, &mut client2, &mut client3],
            200,
            100,
        );

        let hit = StockHitRequest {
            target_actor_id: target_actor,
            hit_word: 250,
            target_pose_word: 110,
            moves_word: 400,
            values_word: 50,
        };
        let mut kill_seen = false;
        for index in 0..20 {
            let now = 2_100 + index * 250;
            client3.send_hit(hit, now).unwrap();
            let output = drive(
                &mut server,
                &mut [&mut client1, &mut client2, &mut client3],
                now,
                20,
            );
            kill_seen |= output.iter().flatten().any(|event| {
                matches!(
                    event,
                    StockGameEvent::Kill(StockKillEvent {
                        attacker_actor_id,
                        target_actor_id,
                    }) if *attacker_actor_id == actor3 && *target_actor_id == target_actor
                )
            });
        }
        assert!(kill_seen);
        assert!(!server.round_ended);
        let selected = if healer_actor == actor1 {
            client1.send_cure(7_500).unwrap()
        } else {
            client2.send_cure(7_500).unwrap()
        };
        assert_eq!(selected, [target_actor as u8]);

        let output = drive(
            &mut server,
            &mut [&mut client1, &mut client2, &mut client3],
            7_500,
            100,
        );
        for events in output {
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, StockGameEvent::LocalRespawn)),
                "Cure must never request a spawn-pad relocation"
            );
            assert!(events.iter().any(|event| matches!(
                event,
                StockGameEvent::Respawn(StockRespawnEvent { actor_id })
                    if *actor_id == target_actor
            )));
        }
        let target = server
            .roles
            .values()
            .find(|role| role.actor_id == target_actor)
            .unwrap();
        assert_eq!((target.health, target.anger), (2_000.0, 0.0));
        assert_eq!(target.respawn_at_ms, None);
    }

    #[test]
    fn rule4_vip_disconnect_marks_that_camp_defeated() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "disconnect").unwrap();
        let server_addr = transport.local_addr().unwrap();
        let checksums = [0x1234_5678; STOCK_HERO_COUNT];
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::from_checksums(checksums),
            4,
            CombatResourceTable::default(),
        );
        server.set_round_limit(1);
        let mut client1 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 1,
                hero_checksum: checksums[0],
            },
            6,
        );
        let mut client2 = StockGameClient::new(
            StockClientTransport::bind(
                "127.0.0.1:0",
                server_addr,
                StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            StockEnterRoleRequest {
                hero: 1,
                camp: 2,
                hero_checksum: checksums[1],
            },
            5,
        );
        drive(&mut server, &mut [&mut client1, &mut client2], 0, 100);
        assert_eq!(server.vips(), [client1.actor_id(), client2.actor_id()]);
        let client2_actor = client2.actor_id().unwrap();

        client2.transport_mut().disconnect(7).unwrap();
        let output = drive(&mut server, &mut [&mut client1], 100, 100);
        assert!(output[0].iter().any(|event| matches!(
            event,
            StockGameEvent::TeamScore(StockTeamScoreEvent {
                team1_wins: 1,
                team2_wins: 0,
            })
        )));
        assert!(output[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::MatchEnd)));
        assert!(output[0].iter().any(|event| matches!(
            event,
            StockGameEvent::RoleLeft(StockRoleLeftEvent { actor_id })
                if *actor_id == client2_actor
        )));
        assert_eq!(server.team_wins(), [1, 0]);
    }
    include!("stock_game/rebalance_tests.rs");
    include!("stock_game/spawn_tests.rs");
    include!("stock_game/scene_sync_tests.rs");
    include!("stock_game/runtime_sync_tests.rs");
}
