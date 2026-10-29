use crate::escort::{
    character_capsule_intersects_flag, position_inside_safe_zone, SN03, STOCK_ESCORT_CATEGORY,
    STOCK_ESCORT_ITEM_ID, STOCK_ESCORT_SCENE_OBJECT_ID,
};
use crate::modes::{can_respawn, is_siege_stage};
use crate::role_state::{RoleState, RoleVec3, ROLE_DOWNLINK_HZ, ROLE_UPLINK_HZ};
use crate::stock_protocol::{
    decode_role_state_event, decode_role_state_request, decode_stock_roster,
    encode_role_state_event, encode_role_state_request, stock_base_weapon_item_id,
    stock_resource_checksum, stock_weapon_from_item_id, StockCureRequest, StockDropItemEvent,
    StockEnterRoleRequest, StockEquipWeaponRequest, StockFlagRemovedEvent, StockFlagReturnedEvent,
    StockHitReactionEvent, StockHitRequest, StockKillEvent, StockProtocolError, StockRespawnEvent,
    StockRoleEnteredEvent, StockRoleLeftEvent, StockRoleRate, StockRoleStateRecord,
    StockSceneFrame, StockSceneItemRecord, StockSceneItemSpawnRequest, StockSceneOpsEvent,
    StockSceneTakenEvent, StockStageConfigEvent, StockTeamScoreEvent, StockVipEvent,
    StockVitalsEvent, StockWeaponEquippedEvent, StockWeaponObjectRequest,
    STOCK_DISCRETE_RELIABLE_FLAGS, STOCK_EVENT_BE_HIT, STOCK_EVENT_DEATH, STOCK_EVENT_DROP_ITEM,
    STOCK_EVENT_DROP_ITEM_FLAGS, STOCK_EVENT_ENVIRONMENT_DEATH, STOCK_EVENT_FLAG_REMOVED,
    STOCK_EVENT_FLAG_RETURNED, STOCK_EVENT_GUARD_HIT, STOCK_EVENT_HIT_FLAGS, STOCK_EVENT_KILL,
    STOCK_EVENT_MATCH_END, STOCK_EVENT_RESPAWN, STOCK_EVENT_ROLE_ENTERED, STOCK_EVENT_ROLE_LEFT,
    STOCK_EVENT_ROLE_RATE, STOCK_EVENT_ROLE_STATE, STOCK_EVENT_ROLE_STATE_FLAGS,
    STOCK_EVENT_ROSTER, STOCK_EVENT_ROUND_RESTART, STOCK_EVENT_SCENE_OPS,
    STOCK_EVENT_SCENE_SNAPSHOT, STOCK_EVENT_SCENE_SPAWN, STOCK_EVENT_SCENE_TAKEN,
    STOCK_EVENT_STAGE_CONFIG, STOCK_EVENT_TEAM_SCORE, STOCK_EVENT_VIP, STOCK_EVENT_VITALS,
    STOCK_EVENT_WEAPON_EQUIPPED, STOCK_EVENT_WEAPON_OBJECT, STOCK_EVENT_WEAPON_OBJECT_FLAGS,
    STOCK_REQUEST_CURE, STOCK_REQUEST_DROP_ITEM, STOCK_REQUEST_ENTER_ROLE,
    STOCK_REQUEST_EQUIP_WEAPON, STOCK_REQUEST_HIT, STOCK_REQUEST_HIT_FLAGS, STOCK_REQUEST_READY,
    STOCK_REQUEST_ROLE_RATE, STOCK_REQUEST_ROLE_STATE, STOCK_REQUEST_ROLE_STATE_FLAGS,
    STOCK_REQUEST_SPAWN_SCENE_ITEM, STOCK_REQUEST_WEAPON_OBJECT, STOCK_SCENE_OPS_PER_PACKET,
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

fn stock_sequence_reached(ack: u16, sequence: u16) -> bool {
    let modulus = u32::from(crate::stock_udp::STOCK_SEQUENCE_MAX);
    ack != 0 && (u32::from(ack) + modulus - u32::from(sequence)) % modulus < modulus / 2
}
use meteor_asset::pos::AttackBlock;
use meteor_asset::res::CombatResourceTable;
use meteor_combat::hit_resolve::{resolve_hit, CombatStats, HitOptions};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::Path;

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
pub const STOCK_SCENE_PICKUP_HEIGHT: f32 = 1.8;
pub const STOCK_SCENE_RECORDS_PER_PACKET: usize = 58;
pub const STOCK_WEAPON_THROW_TIMEOUT_MS: u64 = 5_000;

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
}

pub fn stock_room_item_group(category: u32, item_id: u32) -> Option<StockRoomItemGroup> {
    match category {
        1 if stock_weapon_from_item_id(item_id).is_some() => Some(StockRoomItemGroup::Hide),
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
    PlayerName {
        actor_id: u32,
        name: String,
    },
    RemoteVitals(crate::stock_remote_vitals::RemoteVitals),
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
    WeaponThrown(StockWeaponThrowEvent),
    WeaponSettled {
        throw_id: u32,
        record_id: u32,
    },
    SceneOps(StockSceneOpsEvent),

    RoundEnd,
    RoundRestart,
    MatchEnd,
    LocalDeath,

    LocalRespawn,
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
        }
        for event in session_events {
            match event {
                StockSessionEvent::Connected { peer, transport_id } => {
                    self.transport_id = Some(transport_id);

                    self.last_latency_request_ms = Some(now_ms);
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
                    crate::stock_latency::EVENT => {
                        if let Some(samples) = crate::stock_latency::decode(&packet.payload) {
                            events.push(StockGameEvent::Latencies(samples));
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
                    STOCK_EVENT_SCENE_OPS => events.push(StockGameEvent::SceneOps(
                        StockSceneOpsEvent::decode(&packet.payload)?,
                    )),
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
            STOCK_REQUEST_WEAPON_OBJECT,
            [],
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
    let [half_x, half_y, half_z] = item.seed.half_extents;
    (position.x - center.x).abs() <= half_x + STOCK_SCENE_PICKUP_RADIUS
        && (position.y - center.y).abs() <= half_y + STOCK_SCENE_PICKUP_HEIGHT
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
        32 => effect.health = 50,
        33 => effect.anger = 1,
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
    let already_active = role.active_item_effects.contains_key(&item_id);
    if effect.max_health != 0 && !already_active {
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
    authored: bool,
}

impl StockHostedSceneItem {
    fn record(self) -> StockSceneItemRecord {
        StockSceneItemRecord::from_role_position(
            self.record_id,
            self.seed.item_id,
            self.seed.category,
            self.seed.position,
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
    scene_frame: Option<StockSceneFrame>,
    hazards: meteor_scene::hazards::StageHazards,
    hazard_tick_ms: Option<u64>,

    transport: StockServerTransport,
    heroes: StockHeroCatalog,
    combat_resources: CombatResourceTable,
    rule: i32,
    stage_id: Option<String>,
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
    item_rules: StockRoomItemRules,
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
            scene_frame: None,
            hazards: Default::default(),
            hazard_tick_ms: None,
            transport,
            heroes,
            combat_resources,
            rule: rule.clamp(1, 5),
            stage_id: None,
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
            item_rules: StockRoomItemRules::default(),
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

    fn escort_stage_data(&self) -> crate::escort::EscortStage {
        self.escort_definition.as_ref().map_or(SN03, |d| d.stage)
    }

    fn escort_item_id(&self) -> u32 {
        self.escort_definition
            .as_ref()
            .map_or(STOCK_ESCORT_ITEM_ID, |d| d.item_id)
    }

    pub fn set_stage_id(&mut self, stage_id: impl Into<String>) {
        self.stage_id = Some(stage_id.into().to_ascii_lowercase());
        self.transport.set_discovery_stage(self.stage_id.clone());
    }

    pub fn set_scene_frame(&mut self, frame: StockSceneFrame) {
        self.scene_frame = Some(frame);
    }

    pub fn scene_frame(&self) -> Option<StockSceneFrame> {
        self.scene_frame
    }

    pub fn set_item_rules(&mut self, rules: StockRoomItemRules) {
        self.item_rules = rules;
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
        self.scene_items.clear();
        let item_rules = self.item_rules;
        for seed in items
            .into_iter()
            .filter(valid_authored_scene_item)
            .filter(|seed| item_rules.allows(seed.category, seed.item_id))
        {
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
                    authored: true,
                },
            );
        }
        self.broadcast_scene_snapshot(now_ms)
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<Vec<StockGameEvent>, StockGameError> {
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
                    self.handle_application(
                        peer,
                        packet.opcode,
                        &packet.payload,
                        now_ms,
                        &mut events,
                    )?;
                }
                StockSessionEvent::Disconnected { peer, reason } => {
                    self.remove_disconnected_peer(peer, reason, now_ms, &mut events)?;
                }
                StockSessionEvent::Connected { .. } | StockSessionEvent::DiscoveryReply { .. } => {}
            }
        }
        if self
            .round_restart_at_ms
            .is_some_and(|deadline| now_ms >= deadline)
        {
            self.round_restart_at_ms = None;
            self.reset_roles_for_round(now_ms);
            self.round_ended = false;
            self.broadcast_scene_snapshot(now_ms)?;
            events.push(StockGameEvent::RoundRestart);
        }
        if !self.round_ended {
            self.assign_vips_if_ready(now_ms, &mut events)?;
            self.update_scene_items(now_ms, &mut events)?;
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
            field_c: 0,
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
        let disconnected_role = self.roles.remove(&peer);
        self.ready_peers.remove(&peer);
        self.remote_vitals_peers.remove(&peer);
        self.player_name_peers.remove(&peer);
        self.unicode_player_names.remove(&peer);
        self.requested_role_rates.remove(&peer);
        events.push(StockGameEvent::PeerDisconnected { peer, reason });
        if let Some(role) = disconnected_role {
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
                    | STOCK_REQUEST_ANGER_FULL
                    | STOCK_REQUEST_ANGER_COST
                    | STOCK_REQUEST_QI_BURST
                    | crate::stock_protocol::STOCK_REQUEST_POSE_CHAIN
                    | crate::stock_protocol::STOCK_EVENT_SCENE_OPS
            )
        {
            return Ok(());
        }
        match opcode {
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
                if self.round_ended {
                    return Ok(());
                }
                let spend = StockAngerSpend::decode(opcode, payload)?;
                let role = self
                    .roles
                    .get_mut(&peer)
                    .ok_or(StockGameError::RoleNotEntered)?;

                if role.health > 0.0
                    && (1..=100).contains(&spend.cost())
                    && role.anger >= spend.cost() as f32
                {
                    role.anger = spend.remaining(role.anger);
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
                let camp_valid = if self.rule == 1 {
                    request.camp == 0
                } else {
                    (1..=2).contains(&request.camp)
                };
                if !self.heroes.accepts(request) || !camp_valid {
                    log::warn!(target: "meteor_network::stock_session",
                        "[stock-lan] enter-role rejected: peer={peer}, reason={}, hero={}, camp={}, rule={}, checksum={:#010x}, expected_checksum={expected_checksum:?}",
                        if expected_checksum.is_none() { "unknown hero" }
                        else if !self.heroes.accepts(request) { "POS checksum mismatch" }
                        else { "invalid camp" },
                        request.hero, request.camp, self.rule, request.hero_checksum);
                    self.transport.disconnect_peer(peer, 0x80)?;
                    return Ok(());
                }
                let actor_id = self
                    .transport
                    .peer_transport_id(peer)
                    .ok_or(StockTransportError::UnknownPeer(peer))?;
                let existing: Vec<StockHostedRole> = self.roles.values().cloned().collect();
                let role = StockHostedRole {
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
                self.broadcast_player_names(now_ms)?;
                self.send_scene_bootstrap(peer, now_ms)?;

                let mut scene_init = 0u16.to_le_bytes().to_vec();
                scene_init.extend_from_slice(&(now_ms as u32).to_le_bytes());
                self.transport.send_application(
                    peer,
                    48,
                    scene_init,
                    STOCK_DISCRETE_RELIABLE_FLAGS,
                    now_ms,
                )?;
                log::debug!(target: "meteor_network::stock_session",
                    "[stock-lan] entry replies sent: peer={peer}, actor={actor_id}, scene_init=48");
                events.push(StockGameEvent::RoleEntered(role.entered_event()));
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
                if target_role.health <= 0.0
                    || target_role
                        .last_hit_ms
                        .is_some_and(|last| now_ms.saturating_sub(last) <= 200)
                {
                    return Ok(());
                }
                if now_ms.saturating_sub(target_role.spawned_at_ms) < 2_000 {
                    if let Some(target) = self.roles.get_mut(&target_peer) {
                        target.last_hit_ms = Some(now_ms);
                    }
                    return Ok(());
                }
                let attacker_weapon = attacker_role
                    .weapon_item_id
                    .and_then(stock_weapon_from_item_id)
                    .map(|(weapon, _)| weapon)
                    .unwrap_or(6);
                let defender_weapon = target_role
                    .weapon_item_id
                    .and_then(stock_weapon_from_item_id)
                    .map(|(weapon, _)| weapon)
                    .unwrap_or(6);
                let pose_id = (request.hit_word & 0x03ff) as u16;
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
                    .weapon_stats(attacker_weapon)
                    .map(|stats| stats.attack);
                let attacker_item_bonus = attacker_role.item_attack_bonus();
                let defender_weapon_def = self
                    .combat_resources
                    .weapon_stats(defender_weapon)
                    .map(|stats| stats.defense);
                let combat_resources = &self.combat_resources;
                let result = {
                    let target = self
                        .roles
                        .get_mut(&target_peer)
                        .ok_or(StockGameError::RoleNotEntered)?;
                    target.last_hit_ms = Some(now_ms);
                    resolve_hit(
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
                    )
                };
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
                if !result.guarded {
                    if let Some(definition) = self.combat_resources.pose_status_effect(pose_id) {
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
                    context: (request.hit_word >> 16) & 0x0f,
                    target_actor_id: request.target_actor_id,
                };
                let mut recipients = vec![peer];
                if target_peer != peer {
                    recipients.push(target_peer);
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
            }
            STOCK_REQUEST_EQUIP_WEAPON => {
                let request = StockEquipWeaponRequest::decode(payload)?;
                let actor_id = {
                    let role = self
                        .roles
                        .get_mut(&peer)
                        .ok_or(StockGameError::RoleNotEntered)?;
                    role.weapon_item_id = Some(request.weapon_item_id);
                    role.weapon_slots[0] = Some(request.weapon_item_id);
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
                    self.scene_items.insert(
                        record_id,
                        StockHostedSceneItem {
                            seed: StockAuthoredSceneItem {
                                item_id: record.item_id,
                                category: record.category,
                                position: record.role_position(),
                                half_extents: [0.0; 3],
                                respawn_ms: None,
                                scene_reference: None,
                                active: true,
                                interactive: true,
                            },
                            record_id,
                            active: true,
                            script_active: true,
                            interactive: true,
                            respawn_at_ms: None,
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
                if !self.roles.contains_key(&peer) {
                    return Err(StockGameError::RoleNotEntered);
                }
                let event = StockSceneOpsEvent::decode(payload)?;
                if !event.operations.is_empty() {
                    self.broadcast_discrete_except(peer, STOCK_EVENT_SCENE_OPS, payload, now_ms)?;
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
            .map(|item| item.record())
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

    fn broadcast_scene_snapshot(&mut self, now_ms: u64) -> Result<(), StockGameError> {
        for peer in self.transport.peer_addresses() {
            self.send_scene_bootstrap(peer, now_ms)?;
        }
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
                released.push(item.record());
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
        if self.round_ended
            || self.roles.is_empty()
            || !self
                .roles
                .keys()
                .all(|peer| self.ready_peers.contains(peer))
        {
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
            let Some(mut item) = self.scene_items.remove(&old_record_id) else {
                continue;
            };
            item.record_id = self.allocate_scene_record_id();
            item.active = true;
            item.interactive = true;
            item.respawn_at_ms = None;
            let record = item.record();
            self.scene_items.insert(item.record_id, item);
            self.broadcast_discrete(STOCK_EVENT_SCENE_SPAWN, &record.encode(), now_ms)?;
            events.push(StockGameEvent::SceneItemsGrounded {
                records: vec![record],
                spawned: true,
            });
        }

        let pickup_candidates = self
            .scene_items
            .iter()
            .filter(|(_, item)| item.active && item.interactive)
            .filter_map(|(record_id, item)| {
                self.roles
                    .iter()
                    .filter(|(_, role)| role.health > 0.0 && role_can_take_scene_item(role, item))
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
            let record = item.record();
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
            if position_inside_safe_zone(
                state.position,
                carrier.camp as i32,
                &self.escort_stage_data(),
            ) {
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
        if self.rule != 4
            || self.round_ended
            || self.vips.iter().any(Option::is_some)
            || self.roles.is_empty()
            || !self
                .roles
                .keys()
                .all(|peer| self.ready_peers.contains(peer))
        {
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
        if teams.iter().any(Vec::is_empty) {
            return Ok(());
        }
        teams.iter_mut().for_each(|actors| actors.sort_unstable());

        let actor_mix = teams
            .iter()
            .flatten()
            .fold(0u64, |seed, actor| seed.rotate_left(7) ^ u64::from(*actor));
        let seed =
            now_ms ^ actor_mix ^ u64::from(self.round_count).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let selected = [
            teams[0][seed as usize % teams[0].len()],
            teams[1][seed.rotate_left(29) as usize % teams[1].len()],
        ];

        for (index, actor_id) in selected.into_iter().enumerate() {
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
        if self.rule != 5 || self.round_ended {
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
        if team1.is_empty() || team2.is_empty() {
            return Ok(());
        }
        let team1_wiped = team1.iter().all(|role| role.health <= 0.0);
        let team2_wiped = team2.iter().all(|role| role.health <= 0.0);
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
        let stage_valid = self.stage_id.as_deref().is_some_and(is_siege_stage);
        if self.rule != 3
            || !stage_valid
            || !matches!(winner_team, 1 | 2)
            || self.round_ended
            || self.roles.is_empty()
            || self.ready_peers.len() < self.roles.len()
        {
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

    fn reset_roles_for_round(&mut self, now_ms: u64) {
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
        let packets = client.transport.tick(304).unwrap();
        let rejected = packets
            .iter()
            .find_map(|e| match e {
                StockSessionEvent::Application { packet, .. }
                    if packet.opcode == STOCK_EVENT_STAGE_CHECKSUM_REJECTED =>
                {
                    Some(packet)
                }
                _ => None,
            })
            .unwrap();
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
        for now in 42_200..42_300 {
            server.tick(now).unwrap();
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
        let init_index = entry[1].iter().position(|p| p.opcode == 48).unwrap();
        assert!(
            entry[1][..init_index].iter().any(|p| p.opcode == 9),
            "create roles before releasing the join barrier"
        );
        let init = &entry[1][init_index];
        assert!(init.reliable());
        assert_eq!(
            init.payload,
            [0, 0, 0xd8, 0xa4, 0, 0],
            "event 48 requires u16 index + u32 timestamp, even without records"
        );
        assert_eq!(entry[1].iter().filter(|p| p.opcode == 48).count(), 1);
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
        server.replace_environment_geometry(geometry.clone());
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
            let events = drive(&mut server, &mut [&mut a, &mut b], 2200, 100);
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
                assert!(events
                    .iter()
                    .flatten()
                    .any(|e| matches!(e, StockGameEvent::EscortGrounded { dropped: true, .. })));
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
        assert!(
            server.roles.values().all(|role| role.anger == 59.0),
            "insufficient or invalid debit cannot grant or remove Qi"
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
            assert!(events.iter().any(|event| matches!(event, StockGameEvent::PropertyEffect(effect) if effect.actor_id == target && effect.category == 2 && effect.resource_id == 12)));
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
        let mut late = connect();
        let joined = drive(
            &mut server,
            &mut [&mut host, &mut peer, &mut late],
            250,
            100,
        );
        assert!(joined[2].iter().any(|event| matches!(event,
            StockGameEvent::SceneItemsGrounded { records: observed, spawned: false } if observed == &records)));

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

        peer.send_scene_ops(
            &StockSceneOpsEvent {
                timestamp_ms: 380,
                operations: vec![
                    op(101, STOCK_SCENE_OP_ACTIVE, 1),
                    op(101, STOCK_SCENE_OP_INTERACTIVE, 1),
                ],
            },
            380,
        )
        .unwrap();
        drive(&mut server, &mut [&mut host, &mut peer, &mut late], 380, 20);
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
        peer.send_role_state(sample_state(20.0, 4), 400).unwrap();
        let healed = drive(&mut server, &mut [&mut host, &mut peer, &mut late], 400, 30);
        assert!(healed[1].iter().any(|event| matches!(event,
            StockGameEvent::SceneItemTaken { event, record: Some(_) }
                if event.category == 2 && event.item_id == 1 && Some(event.actor_id) == peer.actor_id())));
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
            server.active_scene_records().is_empty(),
            "new-round boxes must contain hidden items again"
        );
        assert!(server
            .scene_items
            .values()
            .all(|item| item.respawn_at_ms.is_none()));
    }

    #[test]
    fn stock_host_bootstraps_takes_and_respawns_authored_scene_item() {
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
        server
            .replace_authored_scene_items(
                vec![StockAuthoredSceneItem {
                    item_id: 1,
                    category: 2,
                    position: RoleVec3::ZERO,
                    half_extents: [0.25; 3],
                    respawn_ms: Some(50),
                    scene_reference: None,
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
        server.roles.values_mut().next().unwrap().health = 1_000.0;

        client.send_role_state(sample_state(0.0, 1), 120).unwrap();
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

        client.send_role_state(sample_state(20.0, 2), 151).unwrap();
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
        assert_eq!(stock_room_item_group(1, 6), Some(StockRoomItemGroup::Hide));
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
            "zero-length request 19 commits the alternate slot"
        );
        let mut switched_drop = g;
        switched_drop.item_id = 5;
        switched_drop.remaining_item_id = Some(6);
        client.send_weapon_drop(switched_drop, 5_290).unwrap();
        drive(&mut server, &mut [&mut client], 5_290, 30);
        let role = server.roles.values().next().unwrap();
        assert_eq!(role.weapon_slots, [None, Some(6)]);
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
                drive(&mut server, &mut [&mut a, &mut b], now, 2);
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

        assert_eq!(server.role_rate(), 17);
        assert_eq!(client.role_rate(), 17);
        assert!(seen
            .iter()
            .any(|event| matches!(event, StockGameEvent::RoleRate(17))));
    }

    #[test]
    fn rule2_pickup_and_safe_zone_delivery_end_the_round() {
        for custom in [false, true] {
            let definition =
                std::sync::Arc::new(crate::escort_definition::tests::authored_definition(custom));
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
            let delivered = drive(&mut server, &mut [&mut client1, &mut client2], 400, 100);
            for events in delivered {
                assert!(events.iter().any(|event| matches!(
                    event,
                    StockGameEvent::TeamScore(StockTeamScoreEvent {
                        team1_wins: 1,
                        team2_wins: 0,
                    })
                )));
                assert!(events
                    .iter()
                    .any(|event| matches!(event, StockGameEvent::MatchEnd)));
            }
            assert_eq!(server.team_wins(), [1, 0]);
        }
    }

    #[test]
    fn rule2_death_drops_then_timeout_and_disconnect_return_the_flag() {
        for custom in [false, true] {
            let definition =
                std::sync::Arc::new(crate::escort_definition::tests::authored_definition(custom));
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

        assert!(server.complete_siege_objective(2, 400).unwrap().is_empty());
        server.set_stage_id("sn04");
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
}
