use serde::{Deserialize, Serialize};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::wire_v4::{
    dynamic_value, envelope, Control as WireControl, DynamicField, DynamicList, DynamicObject,
    DynamicValue, Envelope as WireEnvelope, Input as WireInput, MessageKind, NullValue,
    RelayFromHost as WireRelayFromHost, RelayFromPeer as WireRelayFromPeer,
    RelayHost as WireRelayHost, RelayPeer as WireRelayPeer, RoleSnapshot as WireRoleSnapshot,
    Snapshot as WireSnapshot, PROTOCOL_VERSION as V4_VERSION,
};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneOp {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: JsonValue,
    pub field: String,
    #[serde(default)]
    pub args: Vec<JsonValue>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_true(value: &bool) -> bool {
    *value
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ItemWeaponSlotWire {
    pub index: u8,
    pub present: bool,
    #[serde(default)]
    pub weapon: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon_id: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub resource_class: u8,
    #[serde(default)]
    pub variant: u8,
    #[serde(default)]
    pub attack_power: f32,
    #[serde(default)]
    pub defense_power: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ItemEffectWire {
    pub model: String,
    pub remaining: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum V4CodecError {
    #[error("protobuf frame cannot be decoded: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("protocol version mismatch: expected {V4_VERSION}, got {0}")]
    Version(u32),
    #[error("protobuf envelope does not contain a payload usable by this API")]
    UnsupportedPayload,
    #[error("message type {0} has no protocol-v4 control representation")]
    UnsupportedType(&'static str),
    #[error("protobuf control kind {0} has no Rust message representation")]
    UnsupportedKind(i32),
    #[error("protobuf control fields are not a JSON object")]
    InvalidFields,
    #[error("protobuf control fields could not decode: {0}")]
    Json(#[from] serde_json::Error),
}

pub enum V4Payload {
    Control(Message),
    Input(WireInput),
    Snapshot(WireSnapshot),
    RoleSnapshot(WireRoleSnapshot),
    RelayHost(WireRelayHost),
    RelayPeer(WireRelayPeer),
    RelayFromHost(WireRelayFromHost),
    RelayFromPeer(WireRelayFromPeer),
}

impl V4Payload {
    pub fn to_v4_binary(&self) -> Result<Vec<u8>, V4CodecError> {
        if let Self::Control(message) = self {
            return message.to_v4_binary();
        }
        let payload = match self {
            Self::Control(_) => unreachable!("control returned above"),
            Self::Input(input) => envelope::Payload::Input(input.clone()),
            Self::Snapshot(snapshot) => envelope::Payload::Snapshot(snapshot.clone()),
            Self::RoleSnapshot(snapshot) => envelope::Payload::RoleSnapshot(snapshot.clone()),
            Self::RelayHost(relay) => envelope::Payload::RelayHost(relay.clone()),
            Self::RelayPeer(relay) => envelope::Payload::RelayPeer(relay.clone()),
            Self::RelayFromHost(relay) => envelope::Payload::RelayFromHost(relay.clone()),
            Self::RelayFromPeer(relay) => envelope::Payload::RelayFromPeer(relay.clone()),
        };
        Ok(prost::Message::encode_to_vec(&WireEnvelope {
            protocol: V4_VERSION,
            payload: Some(payload),
        }))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoomLatencyWire {
    #[serde(default)]
    pub server_rtt: Option<f64>,
    #[serde(default)]
    pub host_rtt: Option<f64>,
    #[serde(default)]
    pub p2p_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoomMemberWire {
    #[serde(alias = "id")]
    pub player_id: String,
    #[serde(default)]
    pub name: String,

    #[serde(
        default,
        alias = "character",
        deserialize_with = "deserialize_character_id"
    )]
    pub character_id: u8,

    #[serde(default, deserialize_with = "deserialize_weapon_id")]
    pub weapon: u8,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u32>,
    #[serde(default)]
    pub ready: bool,
    #[serde(default = "default_connected")]
    pub connected: bool,
}

fn default_connected() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoomLobbyWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_player_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_players: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_loading: Option<bool>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum U8OrString {
    Number(u8),
    String(String),
}

fn deserialize_character_id<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match U8OrString::deserialize(deserializer)? {
        U8OrString::Number(value) => value,
        U8OrString::String(value) => character_id_from_web_name(&value),
    })
}

fn deserialize_weapon_id<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match U8OrString::deserialize(deserializer)? {
        U8OrString::Number(value) => value,
        U8OrString::String(value) => weapon_id_from_web_name(&value),
    })
}

fn deserialize_nullable_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

pub fn character_id_from_web_name(character: &str) -> u8 {
    character
        .strip_prefix('p')
        .or_else(|| character.strip_prefix('P'))
        .unwrap_or(character)
        .parse::<u8>()
        .unwrap_or(0)
}

pub fn weapon_id_from_web_name(weapon: &str) -> u8 {
    match weapon {
        "dart" => 1,
        "wheel" => 2,
        "gun" => 3,
        "stinger" => 4,
        "dagger" => 5,
        "sword" => 6,
        "spear" => 7,
        "blade" => 8,
        "hammer" => 9,
        "staffBlade" => 10,
        "fist" => 11,
        "ninjaBlade" => 12,
        _ => weapon.parse::<u8>().unwrap_or(6),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AngerSpendAction {
    pub pose_instance_id: u32,
    pub action_id: u16,
    pub cost: u16,
}

pub fn encode_anger_spend_action(
    pose_instance_id: u32,
    action_id: u16,
    cost: u16,
) -> Option<String> {
    (pose_instance_id <= 0x7fff_ffff && action_id <= 2048 && (1..=100).contains(&cost))
        .then(|| format!("anger_spend:{pose_instance_id}:{action_id}:{cost}"))
}

pub fn parse_anger_spend_action(value: &str) -> Option<AngerSpendAction> {
    let mut fields = value.split(':');
    if fields.next()? != "anger_spend" {
        return None;
    }
    let pose_instance_id = fields.next()?.parse::<u32>().ok()?;
    let action_id = fields.next()?.parse::<u16>().ok()?;
    let cost = fields.next()?.parse::<u16>().ok()?;
    if fields.next().is_some() {
        return None;
    }
    let action = AngerSpendAction {
        pose_instance_id,
        action_id,
        cost,
    };
    (encode_anger_spend_action(pose_instance_id, action_id, cost).as_deref() == Some(value))
        .then_some(action)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoomSummary {
    pub room_id: String,
    pub name: String,
    pub mode: Option<String>,
    pub stage_id: Option<String>,
    pub configured: bool,
    pub state: String,
    pub has_password: bool,
    pub player_count: usize,
    pub max_players: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_counts: Option<[usize; 2]>,
    pub owner_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoomRulesWire {
    #[serde(default)]
    pub max_players: Option<usize>,
    #[serde(default)]
    pub round_time: Option<u32>,
    #[serde(default)]
    pub game_num: Option<u32>,
    #[serde(default)]
    pub friendly_fire: Option<bool>,
    #[serde(default)]
    pub life_adjust: Option<u32>,
    #[serde(default)]
    pub hide_item: Option<bool>,
    #[serde(default)]
    pub health_item: Option<bool>,
    #[serde(default)]
    pub update_item: Option<bool>,
    #[serde(default)]
    pub special_item: Option<bool>,
    #[serde(default)]
    pub transport: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct P2pMatchConfig {
    #[serde(default)]
    pub match_epoch: u32,
    #[serde(default)]
    pub round_epoch: u32,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub mode: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub stage_id: String,
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub room_name: String,
    #[serde(default)]
    pub rules: RoomRulesWire,
    #[serde(default)]
    pub players: Vec<RoomMemberWire>,
}

impl RoomRulesWire {
    pub fn web_defaults() -> Self {
        Self {
            max_players: Some(16),
            round_time: Some(999),
            game_num: Some(999),
            friendly_fire: Some(false),
            life_adjust: Some(200),
            hide_item: Some(true),
            health_item: Some(true),
            update_item: Some(true),
            special_item: Some(true),
            transport: Some("server".into()),
        }
    }

    pub fn merge_from(&mut self, update: Self) {
        macro_rules! merge {
            ($field:ident) => {
                if update.$field.is_some() {
                    self.$field = update.$field;
                }
            };
        }
        merge!(max_players);
        merge!(round_time);
        merge!(game_num);
        merge!(friendly_fire);
        merge!(life_adjust);
        merge!(hide_item);
        merge!(health_item);
        merge!(update_item);
        merge!(special_item);
        merge!(transport);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FighterSnapshot {
    pub x: f32,
    pub y: f32,
    pub z: f32,

    pub yaw: f32,

    pub pose_id: u16,
    pub pose_frame: u32,
    pub pose_start: u32,
    pub pose_end: u32,
    pub pose_loop_start: u32,
    pub pose_loop_end: u32,
    pub pose_loops: bool,

    pub hp: f32,
    pub max_hp: f32,
    pub weapon: u8,
    pub movement_mode: u8,

    #[serde(default)]
    pub kills: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EscortFlagSnapshot {
    pub position: [f32; 3],
    pub carrier_id: Option<String>,
    pub state: String,
    pub return_in: Option<f32>,
}

pub fn escort_flag_from_v4(value: Option<&DynamicObject>) -> Option<EscortFlagSnapshot> {
    let object = dynamic_object_to_json(value);
    let object = object.as_object()?;
    let position = object.get("position")?.as_array()?;
    if position.len() != 3 {
        return None;
    }
    let mut coordinates = [0.0; 3];
    for (index, value) in position.iter().enumerate() {
        let value = value.as_f64()? as f32;
        if !value.is_finite() {
            return None;
        }
        coordinates[index] = value;
    }
    let carrier_id = object
        .get("carrierId")
        .and_then(JsonValue::as_str)
        .map(str::to_string);
    let state = object.get("state")?.as_str()?.to_string();
    let return_in = object
        .get("returnIn")
        .and_then(JsonValue::as_f64)
        .map(|value| value as f32)
        .filter(|value| value.is_finite() && *value >= 0.0);
    Some(EscortFlagSnapshot {
        position: coordinates,
        carrier_id,
        state,
        return_in,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CombatReactionWire {
    pub defense_move: f32,
    pub target_move: f32,
    pub defense_value: f32,
    pub target_value: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_pose: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_pose_front: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_pose_back: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_pose_left: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_pose_right: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum Message {
    #[serde(rename = "hello")]
    Hello {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume_token: Option<String>,

        #[serde(default)]
        character_id: u8,
        #[serde(default)]
        weapon: u8,
    },

    #[serde(rename = "welcome")]
    Welcome {
        #[serde(default)]
        player_id: String,
        #[serde(default)]
        room_id: String,
        #[serde(default)]
        members: Vec<RoomMemberWire>,
        #[serde(default)]
        client_id: String,
        #[serde(default)]
        session_token: String,
        #[serde(default)]
        resumed: bool,
    },

    #[serde(rename = "error")]
    Error {
        code: String,
        #[serde(rename = "message", alias = "text")]
        text: String,
    },

    #[serde(rename = "ping")]
    Ping {
        #[serde(default)]
        client_time: Option<f64>,
    },
    #[serde(rename = "pong")]
    Pong {
        #[serde(default)]
        client_time: Option<f64>,
        #[serde(default)]
        server_time: Option<f64>,
    },

    #[serde(rename = "quit")]
    Quit,

    #[serde(rename = "room.create")]
    RoomCreate {
        mode: String,
        stage: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        character_id: u8,
        #[serde(default)]
        weapon: u8,
        #[serde(default)]
        room_name: String,
        #[serde(default)]
        password: String,
        #[serde(default)]
        rules: RoomRulesWire,
    },

    #[serde(rename = "room.join")]
    RoomJoin {
        room_id: String,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        stage: Option<String>,
        #[serde(default)]
        name: String,
        #[serde(default)]
        character_id: u8,
        #[serde(default)]
        weapon: u8,
        #[serde(default)]
        password: String,
    },

    #[serde(rename = "room.list")]
    RoomList {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rooms: Option<Vec<RoomSummary>>,
    },

    #[serde(rename = "room.leave")]
    RoomLeave,

    #[serde(rename = "room.left")]
    RoomLeft,

    #[serde(rename = "room.ready")]
    RoomReady { ready: bool },

    #[serde(rename = "room.latency")]
    RoomLatency {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        room_id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        player_id: String,
        #[serde(flatten)]
        latency: RoomLatencyWire,
    },

    #[serde(rename = "team.select")]
    TeamSelect { team: u32 },
    #[serde(rename = "room.profile")]
    RoomProfile { character: String, weapon: String },
    #[serde(rename = "room.settings")]
    RoomSettings {
        mode: String,
        stage_id: String,
        #[serde(default)]
        rules: RoomRulesWire,
    },
    #[serde(rename = "room.start")]
    RoomStart,
    #[serde(rename = "room.joined")]
    RoomJoined {
        room_id: String,
        player_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        match_epoch: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        round_epoch: Option<u32>,
        #[serde(default)]
        mode: Option<String>,
        #[serde(default)]
        resumed: bool,
        #[serde(default)]
        last_processed_input_seq: u32,
        #[serde(default)]
        room: JsonValue,

        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scene_state: Vec<SceneOp>,
    },

    #[serde(rename = "room.state")]
    RoomState {
        room_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        match_epoch: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        round_epoch: Option<u32>,
        #[serde(default, deserialize_with = "deserialize_nullable_string")]
        mode: String,
        #[serde(
            default,
            alias = "stageId",
            deserialize_with = "deserialize_nullable_string"
        )]
        stage: String,
        #[serde(default, alias = "players")]
        members: Vec<RoomMemberWire>,
        #[serde(default)]
        started: bool,

        #[serde(default)]
        rules: RoomRulesWire,
        #[serde(flatten)]
        lobby: RoomLobbyWire,
    },

    #[serde(rename = "room.playerLeft")]
    RoomPlayerLeft { player_id: String },

    #[serde(rename = "match.assetPrepare")]
    MatchAssetPrepare {
        room_id: String,
        match_epoch: u32,
        round_epoch: u32,
        attempt_id: String,
        purpose: String,
        #[serde(default)]
        joiner_id: Option<String>,
        players: Vec<RoomMemberWire>,
    },
    #[serde(rename = "match.assetReady")]
    MatchAssetReady { attempt_id: String, ok: bool },

    #[serde(rename = "match.prepare")]
    MatchPrepare {
        #[serde(default)]
        room_id: String,
        match_epoch: u32,
        round_epoch: u32,
        attempt_id: String,
        host_id: String,
        #[serde(default)]
        timeout_ms: u32,
        config: P2pMatchConfig,
    },

    #[serde(rename = "match.start")]
    MatchStart {
        #[serde(default)]
        room_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        match_epoch: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        round_epoch: Option<u32>,
        #[serde(default, deserialize_with = "deserialize_nullable_string")]
        mode: String,
        #[serde(
            default,
            alias = "stageId",
            deserialize_with = "deserialize_nullable_string"
        )]
        stage: String,
        #[serde(default)]
        players: Vec<RoomMemberWire>,

        #[serde(default)]
        slots: Vec<String>,
        #[serde(default)]
        resumed: bool,
        #[serde(default)]
        late_joined: bool,
        #[serde(default)]
        authority: String,
        #[serde(default, deserialize_with = "deserialize_nullable_string")]
        host_id: String,
    },

    #[serde(rename = "rtc.signal")]
    RtcSignal {
        #[serde(default)]
        room_id: String,
        #[serde(default)]
        target_id: String,
        #[serde(default)]
        from_id: String,
        #[serde(default)]
        payload: JsonValue,
    },
    #[serde(rename = "rtc.report")]
    RtcReport { attempt_id: String, ok: bool },

    #[serde(rename = "host.state")]
    HostState {
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        match_epoch: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        round_epoch: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        winner_team: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        winner_player_id: Option<String>,
    },

    #[serde(rename = "__rtc_ping")]
    RtcPing { id: String, sent_at: f64 },
    #[serde(rename = "__rtc_pong")]
    RtcPong { id: String, sent_at: f64 },

    #[serde(rename = "round.start")]
    RoundStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        match_epoch: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        round_epoch: Option<u32>,
        round: u32,
        round_time: i32,
        game_num: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team_wins: Option<std::collections::BTreeMap<String, u32>>,
    },

    #[serde(rename = "round.end")]
    RoundEnd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        match_epoch: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        round_epoch: Option<u32>,
        round: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        winner_team: Option<i32>,
        #[serde(default)]
        winner_teams: Vec<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        winner_player_id: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        team_wins: Option<std::collections::BTreeMap<String, u32>>,
    },

    #[serde(rename = "match.reset")]
    MatchReset,

    #[serde(rename = "match.end")]
    MatchEnd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        winner_team: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        winner_player_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lobby_return_seconds: Option<f32>,
    },

    #[serde(rename = "objective.end")]
    ObjectiveEnd { winner_team: i32 },

    #[serde(rename = "scene.propHit")]
    ScenePropHit {
        room_id: String,
        match_epoch: u32,
        round_epoch: u32,
        #[serde(default)]
        player_id: String,
        reference: String,
        pose_id: u16,
        frame: f32,
        #[serde(default)]
        damage: i32,
    },

    #[serde(rename = "scene.ops")]
    SceneOps {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        room_id: String,
        #[serde(default, skip_serializing_if = "is_false")]
        authority_generated: bool,
        ops: Vec<SceneOp>,
    },

    #[serde(rename = "scene.transfer")]
    SceneTransfer {
        room_id: String,
        match_epoch: u32,
        round_epoch: u32,
        serial: u32,
        tick: u32,
        player_id: String,
        position: [f32; 3],
        yaw: f32,
    },

    #[serde(rename = "scene.transfer.ack")]
    SceneTransferAck {
        room_id: String,
        match_epoch: u32,
        round_epoch: u32,
        serial: u32,
    },

    #[serde(rename = "item.claim")]
    ItemClaim {
        #[serde(rename = "ref")]
        reference: String,
        player_id: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<[f32; 3]>,
        #[serde(default = "default_true", skip_serializing_if = "is_true")]
        renewable: bool,
    },

    #[serde(rename = "item.claimed")]
    ItemClaimed {
        #[serde(rename = "ref")]
        reference: String,
        player_id: String,
        model: String,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        picked_weapon_model: Option<String>,
        #[serde(default)]
        weapon: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weapon_model: Option<String>,
        #[serde(default)]
        weapon_slot: u8,
        #[serde(default)]
        weapon_slots: Vec<ItemWeaponSlotWire>,
        health: f32,
        max_health: f32,
        anger: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        respawn_seconds: Option<f32>,
        #[serde(default)]
        item_effects: Vec<ItemEffectWire>,
    },

    #[serde(rename = "item.respawned")]
    ItemRespawned {
        #[serde(rename = "ref")]
        reference: String,
        model: String,
    },

    #[serde(rename = "item.effect")]
    ItemEffect {
        player_id: String,
        health: f32,
        max_health: f32,
        anger: f32,
        #[serde(default)]
        item_effects: Vec<ItemEffectWire>,
    },

    #[serde(rename = "weapon.switched")]
    WeaponSwitched {
        player_id: String,
        weapon: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weapon_model: Option<String>,
        weapon_slot: u8,
        #[serde(default)]
        weapon_slots: Vec<ItemWeaponSlotWire>,
    },

    #[serde(rename = "weapon.dropped")]
    WeaponDropped {
        player_id: String,
        #[serde(rename = "ref")]
        reference: String,
        model: String,
        position: [f32; 3],
        velocity: [f32; 3],
        weapon: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        weapon_model: Option<String>,
        weapon_slot: u8,
        #[serde(default)]
        weapon_slots: Vec<ItemWeaponSlotWire>,
    },

    #[serde(rename = "weapon.dropRest")]
    WeaponDropRest {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        match_epoch: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        round_epoch: Option<u32>,
        #[serde(rename = "ref")]
        reference: String,
        position: [f32; 3],
    },

    #[serde(rename = "role.rate")]
    RoleRate { rate: u32 },

    #[serde(rename = "role.selfEffect")]
    RoleSelfEffect {
        seq: u32,
        #[serde(rename = "roomId")]
        room_id: String,
        #[serde(rename = "matchEpoch")]
        match_epoch: u32,
        #[serde(rename = "roundEpoch")]
        round_epoch: u32,
        #[serde(rename = "poseId")]
        pose_id: u16,
        #[serde(rename = "poseInstance")]
        pose_instance: u32,
    },

    #[serde(rename = "role.poseChain")]
    RolePoseChain {
        seq: u32,
        #[serde(rename = "roomId")]
        room_id: String,
        #[serde(rename = "playerId", default)]
        player_id: String,
        #[serde(rename = "matchEpoch")]
        match_epoch: u32,
        #[serde(rename = "roundEpoch")]
        round_epoch: u32,
        #[serde(default)]
        tick: u32,
        #[serde(flatten)]
        chain: crate::role_state::PoseChain,
    },

    #[serde(rename = "snapshot")]
    Snapshot {
        #[serde(skip_serializing_if = "Option::is_none")]
        player_id: Option<String>,
        #[serde(flatten)]
        fighter: FighterSnapshot,
    },

    #[serde(rename = "combat.hit")]
    CombatHit {
        #[serde(rename = "targetId", alias = "target")]
        target: String,
        damage: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attacker_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        guarded: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        health: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        anger: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        motion_scale: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reaction: Option<CombatReactionWire>,

        #[serde(default, skip_serializing_if = "Option::is_none")]
        tick: Option<u32>,
    },

    #[serde(rename = "combat.death")]
    CombatDeath {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        attacker_id: Option<String>,
        target_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        respawn_seconds: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tick: Option<u32>,
    },

    #[serde(rename = "combat.respawn")]
    CombatRespawn {
        player_id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        healer_id: Option<String>,
    },

    #[serde(rename = "combat.cure")]
    CombatCure {
        healer_id: String,
        target_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<[f32; 3]>,
    },
    #[serde(rename = "escort.taken")]
    EscortTaken {
        player_id: String,
        team: i32,
        position: [f32; 3],
    },
    #[serde(rename = "escort.dropped")]
    EscortDropped {
        player_id: String,
        position: [f32; 3],
        return_seconds: f32,
    },
    #[serde(rename = "escort.delivered")]
    EscortDelivered {
        player_id: String,
        team: i32,
        safe_zone: String,
    },
    #[serde(rename = "escort.returned")]
    EscortReturned { reason: String, position: [f32; 3] },

    #[serde(rename = "chat.send")]
    ChatSend { text: String },
    #[serde(rename = "chat.message")]
    ChatMessage {
        #[serde(skip_serializing_if = "Option::is_none")]
        player_id: Option<String>,

        #[serde(default)]
        name: String,
        text: String,
    },
}

impl Message {
    pub fn type_name(&self) -> &'static str {
        match self {
            Message::Hello { .. } => "hello",
            Message::Welcome { .. } => "welcome",
            Message::Error { .. } => "error",
            Message::Ping { .. } => "ping",
            Message::Pong { .. } => "pong",
            Message::RoomCreate { .. } => "room.create",
            Message::RoomJoin { .. } => "room.join",
            Message::RoomList { .. } => "room.list",
            Message::RoomLeave => "room.leave",
            Message::RoomLeft => "room.left",
            Message::RoomReady { .. } => "room.ready",
            Message::RoomLatency { .. } => "room.latency",
            Message::TeamSelect { .. } => "team.select",
            Message::RoomProfile { .. } => "room.profile",
            Message::RoomSettings { .. } => "room.settings",
            Message::RoomStart => "room.start",
            Message::RoomJoined { .. } => "room.joined",
            Message::RoomState { .. } => "room.state",
            Message::RoomPlayerLeft { .. } => "room.playerLeft",
            Message::MatchPrepare { .. } => "match.prepare",
            Message::MatchAssetPrepare { .. } => "match.assetPrepare",
            Message::MatchAssetReady { .. } => "match.assetReady",
            Message::MatchStart { .. } => "match.start",
            Message::RtcSignal { .. } => "rtc.signal",
            Message::RtcReport { .. } => "rtc.report",
            Message::HostState { .. } => "host.state",
            Message::RtcPing { .. } => "__rtc_ping",
            Message::RtcPong { .. } => "__rtc_pong",
            Message::RoundStart { .. } => "round.start",
            Message::RoundEnd { .. } => "round.end",
            Message::MatchReset => "match.reset",
            Message::MatchEnd { .. } => "match.end",
            Message::ObjectiveEnd { .. } => "objective.end",
            Message::SceneOps { .. } => "scene.ops",
            Message::ScenePropHit { .. } => "scene.propHit",
            Message::SceneTransfer { .. } => "scene.transfer",
            Message::SceneTransferAck { .. } => "scene.transfer.ack",
            Message::ItemClaim { .. } => "item.claim",
            Message::ItemClaimed { .. } => "item.claimed",
            Message::ItemRespawned { .. } => "item.respawned",
            Message::ItemEffect { .. } => "item.effect",
            Message::WeaponSwitched { .. } => "weapon.switched",
            Message::WeaponDropped { .. } => "weapon.dropped",
            Message::WeaponDropRest { .. } => "weapon.dropRest",
            Message::RoleRate { .. } => "role.rate",
            Message::RoleSelfEffect { .. } => "role.selfEffect",
            Message::RolePoseChain { .. } => "role.poseChain",
            Message::Snapshot { .. } => "snapshot",
            Message::CombatHit { .. } => "combat.hit",
            Message::CombatDeath { .. } => "combat.death",
            Message::CombatRespawn { .. } => "combat.respawn",
            Message::CombatCure { .. } => "combat.cure",
            Message::EscortTaken { .. } => "escort.taken",
            Message::EscortDropped { .. } => "escort.dropped",
            Message::EscortDelivered { .. } => "escort.delivered",
            Message::EscortReturned { .. } => "escort.returned",
            Message::Quit => "quit",
            Message::ChatSend { .. } => "chat.send",
            Message::ChatMessage { .. } => "chat.message",
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            "{\"type\":\"error\",\"code\":\"encode\",\"text\":\"serialize failed\"}".to_string()
        })
    }

    pub fn from_json(text: &str) -> Option<Message> {
        let mut value = serde_json::from_str(text).ok()?;
        normalize_web_lobby_fields(&mut value);
        serde_json::from_value(value).ok()
    }

    pub fn to_v4_binary(&self) -> Result<Vec<u8>, V4CodecError> {
        let kind = kind_for_message(self).ok_or(V4CodecError::UnsupportedType(self.type_name()))?;
        let mut object = match serde_json::to_value(self)? {
            JsonValue::Object(object) => object,
            _ => return Err(V4CodecError::InvalidFields),
        };
        object.remove("type");
        if self.type_name() == "hello" {
            object.insert("protocol".to_string(), JsonValue::from(V4_VERSION));
        }
        let envelope = WireEnvelope {
            protocol: V4_VERSION,
            payload: Some(envelope::Payload::Control(WireControl {
                kind: kind as i32,
                fields: Some(dynamic_object_from_json(&object)),
            })),
        };
        Ok(prost::Message::encode_to_vec(&envelope))
    }

    pub fn from_v4_payload(bytes: &[u8]) -> Result<V4Payload, V4CodecError> {
        let envelope = <WireEnvelope as prost::Message>::decode(bytes)?;
        if envelope.protocol != V4_VERSION {
            return Err(V4CodecError::Version(envelope.protocol));
        }
        match envelope.payload {
            Some(envelope::Payload::Control(control)) => {
                Ok(V4Payload::Control(Self::from_v4_control(control)?))
            }
            Some(envelope::Payload::Input(input)) => Ok(V4Payload::Input(input)),
            Some(envelope::Payload::Snapshot(snapshot)) => Ok(V4Payload::Snapshot(snapshot)),
            Some(envelope::Payload::RoleSnapshot(snapshot)) => {
                Ok(V4Payload::RoleSnapshot(snapshot))
            }
            Some(envelope::Payload::RelayHost(relay)) => Ok(V4Payload::RelayHost(relay)),
            Some(envelope::Payload::RelayPeer(relay)) => Ok(V4Payload::RelayPeer(relay)),
            Some(envelope::Payload::RelayFromHost(relay)) => Ok(V4Payload::RelayFromHost(relay)),
            Some(envelope::Payload::RelayFromPeer(relay)) => Ok(V4Payload::RelayFromPeer(relay)),
            None => Err(V4CodecError::UnsupportedPayload),
        }
    }

    pub fn from_v4_binary(bytes: &[u8]) -> Result<Message, V4CodecError> {
        match Self::from_v4_payload(bytes)? {
            V4Payload::Control(message) => Ok(message),
            _ => Err(V4CodecError::UnsupportedPayload),
        }
    }

    fn from_v4_control(control: WireControl) -> Result<Message, V4CodecError> {
        let type_name =
            type_for_kind(control.kind).ok_or(V4CodecError::UnsupportedKind(control.kind))?;
        let mut fields = dynamic_object_to_json(control.fields.as_ref());
        let JsonValue::Object(ref mut object) = fields else {
            return Err(V4CodecError::InvalidFields);
        };
        object.insert("type".to_string(), JsonValue::String(type_name.to_string()));
        normalize_web_lobby_fields(&mut fields);
        Ok(serde_json::from_value(fields)?)
    }
}

fn normalize_web_lobby_fields(value: &mut JsonValue) {
    let JsonValue::Object(fields) = value else {
        return;
    };
    let Some(message_type) = fields
        .get("type")
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    match message_type.as_str() {
        "hello" => {
            normalize_loadout_field(fields, "character", "characterId", character_id_from_web);
            normalize_loadout_field(fields, "weapon", "weapon", weapon_id_from_web);
        }
        "room.create" => {
            if !fields.get("mode").is_some_and(JsonValue::is_string) {
                fields.insert("mode".into(), JsonValue::String(String::new()));
            }
            move_field(fields, "stageId", "stage");
            if !fields.get("stage").is_some_and(JsonValue::is_string) {
                fields.insert("stage".into(), JsonValue::String(String::new()));
            }
            normalize_loadout_field(fields, "character", "characterId", character_id_from_web);
            normalize_loadout_field(fields, "weapon", "weapon", weapon_id_from_web);
        }
        "room.join" => {
            move_field(fields, "stageId", "stage");
            normalize_loadout_field(fields, "character", "characterId", character_id_from_web);
            normalize_loadout_field(fields, "weapon", "weapon", weapon_id_from_web);
        }
        _ => {}
    }
}

fn move_field(fields: &mut JsonMap<String, JsonValue>, from: &str, to: &str) {
    if !fields.contains_key(to) {
        if let Some(value) = fields.remove(from) {
            fields.insert(to.to_string(), value);
        }
    }
}

fn normalize_loadout_field(
    fields: &mut JsonMap<String, JsonValue>,
    from: &str,
    to: &str,
    resolver: fn(&str) -> Option<u8>,
) {
    if let Some(label) = fields
        .get(from)
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
    {
        if let Some(id) = resolver(&label) {
            fields.insert(to.to_string(), JsonValue::from(id));
        }
    }
}

fn character_id_from_web(label: &str) -> Option<u8> {
    label
        .strip_prefix('p')
        .or_else(|| label.strip_prefix('P'))
        .and_then(|value| value.parse().ok())
}

fn weapon_id_from_web(label: &str) -> Option<u8> {
    Some(match label {
        "dart" => 1,
        "wheel" => 2,
        "gun" => 3,
        "stinger" => 4,
        "dagger" => 5,
        "sword" => 6,
        "spear" => 7,
        "blade" => 8,
        "hammer" => 9,
        "staffBlade" => 10,
        "fist" => 11,
        "ninjaBlade" => 12,
        _ => return None,
    })
}

fn kind_for_message(message: &Message) -> Option<MessageKind> {
    if let Message::RoomList { rooms } = message {
        return Some(if rooms.is_some() {
            MessageKind::RoomList
        } else {
            MessageKind::RoomListRequest
        });
    }
    kind_for_type(message.type_name())
}

fn kind_for_type(type_name: &str) -> Option<MessageKind> {
    Some(match type_name {
        "hello" => MessageKind::Hello,
        "welcome" => MessageKind::Welcome,
        "error" => MessageKind::Error,
        "ping" => MessageKind::Ping,
        "pong" => MessageKind::Pong,
        "room.create" => MessageKind::RoomCreate,
        "room.join" => MessageKind::RoomJoin,
        "room.left" => MessageKind::RoomLeft,
        "room.leave" => MessageKind::RoomLeave,
        "room.ready" => MessageKind::RoomReady,
        "room.latency" => MessageKind::RoomLatency,
        "team.select" => MessageKind::TeamSelect,
        "room.profile" => MessageKind::RoomProfile,
        "room.settings" => MessageKind::RoomSettings,
        "room.start" => MessageKind::RoomStart,
        "room.joined" => MessageKind::RoomJoined,
        "room.state" => MessageKind::RoomState,
        "room.playerLeft" => MessageKind::RoomPlayerLeft,
        "match.prepare" => MessageKind::MatchPrepare,
        "match.assetPrepare" => MessageKind::MatchAssetPrepare,
        "match.assetReady" => MessageKind::MatchAssetReady,
        "match.start" => MessageKind::MatchStart,
        "rtc.signal" => MessageKind::RtcSignal,
        "rtc.report" => MessageKind::RtcReport,
        "host.state" => MessageKind::HostState,
        "__rtc_ping" => MessageKind::RtcPing,
        "__rtc_pong" => MessageKind::RtcPong,
        "round.start" => MessageKind::RoundStart,
        "round.end" => MessageKind::RoundEnd,
        "match.reset" => MessageKind::MatchReset,
        "match.end" => MessageKind::MatchEnd,
        "objective.end" => MessageKind::ObjectiveEnd,
        "scene.ops" => MessageKind::SceneOps,
        "scene.propHit" => MessageKind::ScenePropHit,
        "scene.transfer" => MessageKind::SceneTransfer,
        "scene.transfer.ack" => MessageKind::SceneTransferAck,
        "item.claim" => MessageKind::ItemClaim,
        "item.claimed" => MessageKind::ItemClaimed,
        "item.respawned" => MessageKind::ItemRespawned,
        "item.effect" => MessageKind::ItemEffect,
        "weapon.switched" => MessageKind::WeaponSwitched,
        "weapon.dropped" => MessageKind::WeaponDropped,
        "weapon.dropRest" => MessageKind::WeaponDropRest,
        "role.rate" => MessageKind::RoleRate,
        "role.selfEffect" => MessageKind::RoleSelfEffect,
        "role.poseChain" => MessageKind::RolePoseChain,
        "combat.hit" => MessageKind::CombatHit,
        "combat.death" => MessageKind::CombatDeath,
        "combat.respawn" => MessageKind::CombatRespawn,
        "combat.cure" => MessageKind::CombatCure,
        "escort.taken" => MessageKind::EscortTaken,
        "escort.dropped" => MessageKind::EscortDropped,
        "escort.delivered" => MessageKind::EscortDelivered,
        "escort.returned" => MessageKind::EscortReturned,
        "chat.send" => MessageKind::ChatSend,
        "chat.message" => MessageKind::ChatMessage,
        _ => return None,
    })
}

fn type_for_kind(kind: i32) -> Option<&'static str> {
    Some(match MessageKind::try_from(kind).ok()? {
        MessageKind::Hello => "hello",
        MessageKind::Welcome => "welcome",
        MessageKind::Error => "error",
        MessageKind::Ping => "ping",
        MessageKind::Pong => "pong",
        MessageKind::RoomCreate => "room.create",
        MessageKind::RoomJoin => "room.join",
        MessageKind::RoomListRequest | MessageKind::RoomList => "room.list",
        MessageKind::RoomLeave => "room.leave",
        MessageKind::RoomLeft => "room.left",
        MessageKind::RoomReady => "room.ready",
        MessageKind::RoomLatency => "room.latency",
        MessageKind::TeamSelect => "team.select",
        MessageKind::RoomProfile => "room.profile",
        MessageKind::RoomSettings => "room.settings",
        MessageKind::RoomStart => "room.start",
        MessageKind::RoomJoined => "room.joined",
        MessageKind::RoomState => "room.state",
        MessageKind::RoomPlayerLeft => "room.playerLeft",
        MessageKind::MatchPrepare => "match.prepare",
        MessageKind::MatchAssetPrepare => "match.assetPrepare",
        MessageKind::MatchAssetReady => "match.assetReady",
        MessageKind::MatchStart => "match.start",
        MessageKind::RtcSignal => "rtc.signal",
        MessageKind::RtcReport => "rtc.report",
        MessageKind::HostState => "host.state",
        MessageKind::RtcPing => "__rtc_ping",
        MessageKind::RtcPong => "__rtc_pong",
        MessageKind::RoundStart => "round.start",
        MessageKind::RoundEnd => "round.end",
        MessageKind::MatchReset => "match.reset",
        MessageKind::MatchEnd => "match.end",
        MessageKind::ObjectiveEnd => "objective.end",
        MessageKind::SceneOps => "scene.ops",
        MessageKind::ScenePropHit => "scene.propHit",
        MessageKind::SceneTransfer => "scene.transfer",
        MessageKind::SceneTransferAck => "scene.transfer.ack",
        MessageKind::ItemClaim => "item.claim",
        MessageKind::ItemClaimed => "item.claimed",
        MessageKind::ItemRespawned => "item.respawned",
        MessageKind::ItemEffect => "item.effect",
        MessageKind::WeaponSwitched => "weapon.switched",
        MessageKind::WeaponDropped => "weapon.dropped",
        MessageKind::WeaponDropRest => "weapon.dropRest",
        MessageKind::RoleRate => "role.rate",
        MessageKind::RoleSelfEffect => "role.selfEffect",
        MessageKind::RolePoseChain => "role.poseChain",
        MessageKind::CombatHit => "combat.hit",
        MessageKind::CombatDeath => "combat.death",
        MessageKind::CombatRespawn => "combat.respawn",
        MessageKind::CombatCure => "combat.cure",
        MessageKind::EscortTaken => "escort.taken",
        MessageKind::EscortDropped => "escort.dropped",
        MessageKind::EscortDelivered => "escort.delivered",
        MessageKind::EscortReturned => "escort.returned",
        MessageKind::ChatSend => "chat.send",
        MessageKind::ChatMessage => "chat.message",
        _ => return None,
    })
}

pub(crate) fn dynamic_object_from_json(object: &JsonMap<String, JsonValue>) -> DynamicObject {
    DynamicObject {
        fields: object
            .iter()
            .map(|(key, value)| DynamicField {
                key: key.clone(),
                value: Some(dynamic_value_from_json(value)),
            })
            .collect(),
    }
}

fn dynamic_value_from_json(value: &JsonValue) -> DynamicValue {
    let kind = match value {
        JsonValue::Null => dynamic_value::Kind::Null(NullValue::Unspecified as i32),
        JsonValue::Bool(value) => dynamic_value::Kind::Bool(*value),
        JsonValue::Number(value) => dynamic_value::Kind::Number(value.as_f64().unwrap_or(0.0)),
        JsonValue::String(value) => dynamic_value::Kind::String(value.clone()),
        JsonValue::Array(values) => dynamic_value::Kind::List(DynamicList {
            values: values.iter().map(dynamic_value_from_json).collect(),
        }),
        JsonValue::Object(object) => dynamic_value::Kind::Object(dynamic_object_from_json(object)),
    };
    DynamicValue { kind: Some(kind) }
}

fn dynamic_object_to_json(object: Option<&DynamicObject>) -> JsonValue {
    JsonValue::Object(
        object
            .map(|object| {
                object
                    .fields
                    .iter()
                    .map(|field| {
                        (
                            field.key.clone(),
                            dynamic_value_to_json(field.value.as_ref()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn dynamic_value_to_json(value: Option<&DynamicValue>) -> JsonValue {
    let Some(kind) = value.and_then(|value| value.kind.as_ref()) else {
        return JsonValue::Null;
    };
    match kind {
        dynamic_value::Kind::Number(value) => {
            json_number_from_wire(*value).map_or(JsonValue::Null, JsonValue::Number)
        }
        dynamic_value::Kind::String(value) => JsonValue::String(value.clone()),
        dynamic_value::Kind::Bool(value) => JsonValue::Bool(*value),

        dynamic_value::Kind::Bytes(value) => JsonValue::Array(
            value
                .iter()
                .copied()
                .map(|byte| JsonValue::Number(byte.into()))
                .collect(),
        ),
        dynamic_value::Kind::Object(value) => dynamic_object_to_json(Some(value)),
        dynamic_value::Kind::List(value) => JsonValue::Array(
            value
                .values
                .iter()
                .map(|value| dynamic_value_to_json(Some(value)))
                .collect(),
        ),
        dynamic_value::Kind::Null(_) => JsonValue::Null,
    }
}

fn json_number_from_wire(value: f64) -> Option<serde_json::Number> {
    if !value.is_finite() {
        return None;
    }
    if value.fract() == 0.0 {
        if value >= 0.0 && value <= u64::MAX as f64 {
            return Some(serde_json::Number::from(value as u64));
        }
        if value >= i64::MIN as f64 && value <= i64::MAX as f64 {
            return Some(serde_json::Number::from(value as i64));
        }
    }
    serde_json::Number::from_f64(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_json_tags_match_type_names() {
        let samples: Vec<Message> = vec![
            Message::Hello {
                name: "n".into(),
                resume_token: Some("resume".into()),
                character_id: 1,
                weapon: 2,
            },
            Message::Welcome {
                player_id: "p1".into(),
                room_id: "room1".into(),
                members: vec![],
                client_id: "c1".into(),
                session_token: "token".into(),
                resumed: false,
            },
            Message::RoomCreate {
                mode: "deathmatch".into(),
                stage: "sn01".into(),
                name: "n".into(),
                character_id: 1,
                weapon: 2,
                room_name: "room".into(),
                password: "secret".into(),
                rules: Default::default(),
            },
            Message::RoomJoin {
                room_id: String::new(),
                mode: None,
                stage: None,
                name: "n".into(),
                character_id: 1,
                weapon: 2,
                password: String::new(),
            },
            Message::RoomList { rooms: None },
            Message::RoomLeft,
            Message::RoomProfile {
                character: "p0".into(),
                weapon: "sword".into(),
            },
            Message::RoomSettings {
                mode: "leader".into(),
                stage_id: "sn01".into(),
                rules: RoomRulesWire {
                    max_players: Some(8),
                    ..Default::default()
                },
            },
            Message::RoomStart,
            Message::RoomJoined {
                room_id: "room1".into(),
                player_id: "p1".into(),
                match_epoch: None,
                round_epoch: None,
                mode: Some("leader".into()),
                resumed: false,
                last_processed_input_seq: 0,
                room: JsonValue::Null,
                scene_state: Vec::new(),
            },
            Message::RoomState {
                lobby: Default::default(),
                room_id: "room1".into(),
                match_epoch: None,
                round_epoch: None,
                mode: "deathmatch".into(),
                stage: "sn01".into(),
                members: vec![],
                started: false,
                rules: RoomRulesWire::web_defaults(),
            },
            Message::RoomPlayerLeft {
                player_id: "p2".into(),
            },
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: None,
                round_epoch: None,
                mode: "deathmatch".into(),
                stage: "sn01".into(),
                players: vec![],
                slots: vec![],
                resumed: false,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            },
            Message::RoundStart {
                match_epoch: None,
                round_epoch: None,
                round: 1,
                round_time: 15,
                game_num: 3,
                team_wins: Some(std::collections::BTreeMap::from([
                    ("1".into(), 0),
                    ("2".into(), 0),
                ])),
            },
            Message::RoundEnd {
                match_epoch: None,
                round_epoch: None,
                round: 1,
                winner_team: None,
                winner_teams: vec![],
                winner_player_id: None,
                reason: "timeout".into(),
                team_wins: None,
            },
            Message::MatchReset,
            Message::MatchEnd {
                winner_team: Some(2),
                winner_player_id: None,
                result: Some("defeat".into()),
                reason: "defeat".into(),
                lobby_return_seconds: Some(10.0),
            },
            Message::ObjectiveEnd { winner_team: 1 },
            Message::SceneOps {
                room_id: "room1".into(),
                authority_generated: false,
                ops: vec![SceneOp {
                    kind: "set".into(),
                    reference: JsonValue::from(4),
                    field: "attribute".into(),
                    args: vec![JsonValue::from("active"), JsonValue::from(0)],
                }],
            },
            Message::CombatHit {
                target: "p2".into(),
                damage: 12.5,
                attacker_id: None,
                guarded: None,
                health: None,
                anger: None,
                motion_scale: None,
                reaction: None,
                tick: None,
            },
            Message::ChatSend { text: "hi".into() },
        ];
        for message in &samples {
            let json = message.to_json();
            let expected = format!("\"{}\"", message.type_name());
            assert!(json.contains(&expected), "{json} lacks tag {expected}");
            let parsed = Message::from_json(&json);
            assert!(parsed.is_some(), "{json} failed to parse");
            assert_eq!(
                parsed.as_ref().map(Message::type_name),
                Some(message.type_name())
            );
        }

        let join = r#"{"type":"room.join","roomId":"","mode":"deathmatch","stage":"sn01","name":"甲","characterId":0,"weapon":6}"#;
        assert!(matches!(
            Message::from_json(join),
            Some(Message::RoomJoin { weapon: 6, .. })
        ));
        let web_join = r#"{"type":"room.join","roomId":"ABC123","name":"Web","character":"p3","weapon":"blade"}"#;
        assert!(matches!(
            Message::from_json(web_join),
            Some(Message::RoomJoin {
                room_id,
                character_id: 3,
                weapon: 8,
                ..
            }) if room_id == "ABC123"
        ));
        let web_create =
            r#"{"type":"room.create","mode":null,"stageId":null,"roomName":"Web room"}"#;
        assert!(matches!(
            Message::from_json(web_create),
            Some(Message::RoomCreate {
                mode,
                stage,
                room_name,
                ..
            }) if mode.is_empty() && stage.is_empty() && room_name == "Web room"
        ));
        assert!(Message::Error {
            code: "bad".into(),
            text: "Web field name".into(),
        }
        .to_json()
        .contains("\"message\""));
        let hit = r#"{"type":"combat.hit","target":"p2","damage":12.5}"#;
        assert!(matches!(
            Message::from_json(hit),
            Some(Message::CombatHit { damage: 12.5, .. })
        ));
        let item_claim = r#"{"type":"item.claim","ref":"D_It01","playerId":"p2","model":"item01"}"#;
        assert!(matches!(
            Message::from_json(item_claim),
            Some(Message::ItemClaim {
                renewable: true,
                ..
            })
        ));
    }

    #[test]
    fn message_round_trips_json() {
        let snapshot = Message::Snapshot {
            player_id: Some("p1".into()),
            fighter: FighterSnapshot {
                x: 1.0,
                y: 2.0,
                z: 3.0,
                yaw: 0.5,
                pose_id: 24,
                pose_frame: 10,
                pose_start: 0,
                pose_end: 30,
                pose_loop_start: 0,
                pose_loop_end: 30,
                pose_loops: false,
                hp: 800.0,
                max_hp: 1000.0,
                kills: 3,
                weapon: 6,
                movement_mode: 1,
            },
        };
        let json = snapshot.to_json();
        assert!(json.contains("\"type\":\"snapshot\""));
        assert!(json.contains("\"playerId\":\"p1\""));
        let decoded = Message::from_json(&json).expect("decodes");
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn message_types_match_web_names() {
        assert_eq!(
            Message::Hello {
                name: "a".into(),
                resume_token: None,
                character_id: 0,
                weapon: 6
            }
            .type_name(),
            "hello"
        );
        assert_eq!(
            Message::RoomState {
                lobby: Default::default(),
                room_id: "r".into(),
                match_epoch: None,
                round_epoch: None,
                mode: "leader".into(),
                stage: "sn01".into(),
                members: vec![],
                started: false,
                rules: RoomRulesWire::web_defaults(),
            }
            .type_name(),
            "room.state"
        );
        assert_eq!(
            Message::MatchStart {
                room_id: String::new(),
                match_epoch: None,
                round_epoch: None,
                mode: "leader".into(),
                stage: "sn01".into(),
                players: vec![],
                slots: vec![],
                resumed: false,
                late_joined: false,
                authority: String::new(),
                host_id: String::new(),
            }
            .type_name(),
            "match.start"
        );
        assert_eq!(
            Message::MatchEnd {
                winner_team: Some(1),
                winner_player_id: None,
                result: None,
                reason: "wipe".into(),
                lobby_return_seconds: None,
            }
            .type_name(),
            "match.end"
        );
        assert_eq!(
            Message::RoundStart {
                match_epoch: None,
                round_epoch: None,
                round: 1,
                round_time: 15,
                game_num: 3,
                team_wins: None,
            }
            .type_name(),
            "round.start"
        );
        assert_eq!(Message::MatchReset.type_name(), "match.reset");
        assert_eq!(
            Message::ObjectiveEnd { winner_team: 1 }.type_name(),
            "objective.end"
        );
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = r#"{"type":"snapshot","playerId":"p2","x":1.5,"y":0.9,"z":4.0,"yaw":0.0,"poseId":0,"poseFrame":0,"poseStart":0,"poseEnd":1,"poseLoopStart":0,"poseLoopEnd":1,"poseLoops":true,"hp":100,"maxHp":100,"weapon":5,"movementMode":0,"extraField":123}"#;
        let message = Message::from_json(json).expect("decodes with unknown field");
        assert!(matches!(message, Message::Snapshot { .. }));
    }

    #[test]
    fn malformed_json_returns_none() {
        assert!(Message::from_json("not json").is_none());
        assert!(Message::from_json(r#"{"type":"unknown"}"#).is_none());
    }

    #[test]
    fn v4_control_decodes_web_hello_bytes() {
        let bytes = [
            0x08, 0x06, 0x82, 0x01, 0x2b, 0x08, 0x01, 0x12, 0x27, 0x0a, 0x15, 0x0a, 0x08, 0x70,
            0x72, 0x6f, 0x74, 0x6f, 0x63, 0x6f, 0x6c, 0x12, 0x09, 0x09, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x18, 0x40, 0x0a, 0x0e, 0x0a, 0x04, 0x6e, 0x61, 0x6d, 0x65, 0x12, 0x06,
            0x12, 0x04, 0x52, 0x75, 0x73, 0x74,
        ];
        assert_eq!(
            Message::from_v4_binary(&bytes).expect("Web hello decodes"),
            Message::Hello {
                name: "Rust".into(),
                resume_token: None,
                character_id: 0,
                weapon: 0,
            }
        );
    }

    #[test]
    fn v4_control_decodes_web_hello_with_browser_loadout_fields() {
        let bytes = hex_bytes(
            "08068201500801124c0a150a0870726f746f636f6c12090900000000000018400a0d0a046e616d65120512037261770a110a096368617261637465721204120270300a110a06776561706f6e1207120573776f7264",
        );
        assert_eq!(
            Message::from_v4_binary(&bytes).expect("Web hello with loadout decodes"),
            Message::Hello {
                name: "raw".into(),
                resume_token: None,
                character_id: 0,
                weapon: 6,
            }
        );
    }

    #[test]
    fn v6_resume_fields_match_current_web_codec_bytes() {
        let hello = Message::from_v4_binary(&hex_bytes(
            "0806820148080112440a150a0870726f746f636f6c12090900000000000018400a0e0a046e616d6512061204527573740a1b0a0b726573756d65546f6b656e120c120a726573756d652d313233",
        ))
        .expect("Web resume hello decodes");
        assert_eq!(
            hello,
            Message::Hello {
                name: "Rust".into(),
                resume_token: Some("resume-123".into()),
                character_id: 0,
                weapon: 0,
            }
        );

        let start = Message::from_v4_binary(&hex_bytes(
            "0806820194010829128f010a110a06726f6f6d496412071205726f6f6d310a170a0a6d6174636845706f63681209090000000000001c400a170a0a726f756e6445706f636812090900000000000008400a140a046d6f6465120c120a64656174686d617463680a110a077374616765496412061204736e30330a0d0a07726573756d6564120218010a100a0a6c6174654a6f696e656412021800",
        ))
        .expect("Web resumed match.start decodes");
        assert!(matches!(
            start,
            Message::MatchStart {
                match_epoch: Some(7),
                round_epoch: Some(3),
                resumed: true,
                late_joined: false,
                ..
            }
        ));
    }

    #[test]
    fn v6_p2p_controls_decode_current_web_codec_bytes() {
        let prepare = Message::from_v4_binary(&hex_bytes(
            "080682018203082812fd020a0d0a06726f6f6d496412031201520a170a0a6d6174636845706f6368120909000000000000f03f0a170a0a726f756e6445706f636812090900000000000000000a100a09617474656d7074496412031201610a0d0a06686f7374496412031201680a160a0974696d656f75744d73120909000000000000f03f0a80020a06636f6e66696712f5012af2010a170a0a6d6174636845706f6368120909000000000000f03f0a170a0a726f756e6445706f636812090900000000000000000a100a046d6f6465120812066c65616465720a110a077374616765496412061204736e30310a100a0a636f6e66696775726564120218010a0f0a08726f6f6d4e616d6512031201780a0b0a0572756c657312022a000a690a07706c6179657273125e325c0a5a2a580a090a02696412031201680a0d0a046e616d65120512035765620a110a047465616d120909000000000000f03f0a110a096368617261637465721204120270330a160a06776561706f6e120c120a7374616666426c616465",
        ))
        .expect("Web match.prepare decodes");
        assert!(matches!(
            prepare,
            Message::MatchPrepare {
                room_id,
                match_epoch: 1,
                round_epoch: 0,
                host_id,
                config,
                ..
            } if room_id == "R"
                && host_id == "h"
                && config.mode == "leader"
                && config.stage_id == "sn01"
                && config.players[0].player_id == "h"
                && config.players[0].character_id == 3
                && config.players[0].weapon == 10
        ));

        let signal = Message::from_v4_binary(&hex_bytes(
            "0806820180010832127c0a120a06726f6f6d496412081206524f4f4d30310a120a0666726f6d496412081206705f686f73740a520a077061796c6f616412472a450a150a0a67656e65726174696f6e1207120567656e2d310a2c0a0373647012252a230a0f0a0474797065120712056f666665720a100a0373647012091207763d305c725c6e",
        ))
        .expect("Web rtc.signal decodes");
        assert!(matches!(
            signal,
            Message::RtcSignal {
                room_id,
                from_id,
                payload,
                ..
            } if room_id == "ROOM01"
                && from_id == "p_host"
                && payload["generation"] == "gen-1"
                && payload["sdp"]["type"] == "offer"
        ));

        let start = Message::from_v4_binary(&hex_bytes(
            "08068201bd01082912b8010a120a06726f6f6d496412081206524f4f4d30310a170a0a6d6174636845706f63681209090000000000001c400a170a0a726f756e6445706f6368120909000000000000f03f0a140a046d6f6465120c120a64656174686d617463680a110a077374616765496412061204736e30330a120a09617574686f72697479120512037032700a120a06686f7374496412081206705f686f73740a0d0a07726573756d6564120218000a100a0a6c6174654a6f696e656412021800",
        ))
        .expect("Web p2p match.start decodes");
        assert!(matches!(
            start,
            Message::MatchStart {
                room_id,
                match_epoch: Some(7),
                round_epoch: Some(1),
                authority,
                host_id,
                ..
            } if room_id == "ROOM01" && authority == "p2p" && host_id == "p_host"
        ));

        let ping = Message::from_v4_binary(&hex_bytes(
            "0806820128086412240a0d0a026964120712053132335f780a130a0673656e7441741209090000000000e05e40",
        ))
        .expect("Web rtc ping decodes");
        assert_eq!(
            ping,
            Message::RtcPing {
                id: "123_x".into(),
                sent_at: 123.5,
            }
        );
    }

    #[test]
    fn v6_match_prepare_round_trip_preserves_web_authority_config() {
        let message = Message::MatchPrepare {
            room_id: "ROOM01".into(),
            match_epoch: 7,
            round_epoch: 0,
            attempt_id: "rtc_ab12".into(),
            host_id: "p_host".into(),
            timeout_ms: 12_000,
            config: P2pMatchConfig {
                match_epoch: 7,
                round_epoch: 0,
                mode: "deathmatch".into(),
                stage_id: "sn03".into(),
                configured: true,
                room_name: "Cross".into(),
                rules: RoomRulesWire::web_defaults(),
                players: vec![RoomMemberWire {
                    connected: true,
                    player_id: "p_rust".into(),
                    name: "Rust".into(),
                    character_id: 3,
                    weapon: 10,
                    team: Some(2),
                    ready: true,
                }],
            },
        };
        let bytes = message.to_v4_binary().expect("prepare encodes");
        assert_eq!(
            Message::from_v4_binary(&bytes).expect("prepare decodes"),
            message
        );
    }

    #[test]
    fn v4_control_round_trips_nested_room_state() {
        let message = Message::RoomState {
            lobby: Default::default(),
            room_id: "room1".into(),
            match_epoch: Some(7),
            round_epoch: Some(3),
            mode: "leader".into(),
            stage: "sn01".into(),
            members: vec![RoomMemberWire {
                connected: true,
                player_id: "p1".into(),
                name: "Rust".into(),
                character_id: 3,
                weapon: 7,
                team: Some(2),
                ready: true,
            }],
            started: true,
            rules: RoomRulesWire {
                friendly_fire: Some(true),
                ..RoomRulesWire::web_defaults()
            },
        };
        let frame = message.to_v4_binary().expect("control encodes");
        assert_eq!(Message::from_v4_binary(&frame).unwrap(), message);
    }

    #[test]
    fn web_lobby_metadata_and_offline_members_survive_actual_web_codec() {
        let message = Message::from_v4_binary(&hex_bytes("08068201de03081612d9030a150a06726f6f6d4964120b12097765622d6c6f6262790a140a046d6f6465120c120a64656174686d617463680a110a077374616765496412061204736e30310a100a0a636f6e66696775726564120218010a100a057374617465120712056c6f6262790a120a0c61737365744c6f6164696e67120218000a180a0d6f776e6572506c617965724964120712056f776e65720a170a0a6d6178506c617965727312090900000000000030400a85020a07706c617965727312f90132f6010a792a770a0d0a0269641207120567756573740a0f0a046e616d651207120547756573740a110a096368617261637465721204120270330a110a06776561706f6e12071205776865656c0a110a047465616d12090900000000000000400a0b0a057265616479120218010a0f0a09636f6e6e6563746564120218000a792a770a0d0a026964120712056f776e65720a0f0a046e616d65120712054f776e65720a110a096368617261637465721204120270310a110a06776561706f6e1207120573776f72640a110a047465616d120909000000000000f03f0a0b0a057265616479120218000a0f0a09636f6e6e6563746564120218010a240a0572756c6573121b2a190a170a0a6d6178506c61796572731209090000000000003040" )).unwrap();
        let Message::RoomState {
            ref lobby,
            ref members,
            ..
        } = message
        else {
            panic!("room.state");
        };
        assert_eq!(lobby.owner_player_id.as_deref(), Some("owner"));
        assert_eq!(lobby.max_players, Some(16));
        assert_eq!(lobby.configured, Some(true));
        assert_eq!(lobby.state.as_deref(), Some("lobby"));
        assert_eq!(lobby.asset_loading, Some(false));
        assert!(!members[0].connected);
        assert_eq!(members[0].weapon, 2);
        assert!(members[0].ready);
        assert!(members[1].connected);
        let roundtrip = Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap();
        let Message::RoomState {
            lobby: actual,
            members: actual_members,
            ..
        } = roundtrip
        else {
            panic!("room.state");
        };
        assert_eq!(&actual, lobby);
        assert_eq!(&actual_members, members);
    }

    #[test]
    fn web_room_state_shape_decodes_into_rust_roster() {
        let json = r#"{
            "type":"room.state",
            "roomId":"room-web",
            "mode":"deathmatch",
            "stageId":"sn03",
            "players":[{
                "id":"web-p1",
                "name":"Web Player",
                "character":"p3",
                "weapon":"staffBlade",
                "team":2,
                "ready":true,
                "connected":true
            }],
            "state":"lobby",
            "rules":{"friendlyFire":true}
        }"#;

        let message: Message = serde_json::from_str(json).expect("Web room.state decodes");
        let Message::RoomState {
            room_id,
            mode,
            stage,
            members,
            rules,
            ..
        } = message
        else {
            panic!("expected room.state");
        };
        assert_eq!(room_id, "room-web");
        assert_eq!(mode, "deathmatch");
        assert_eq!(stage, "sn03");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].player_id, "web-p1");
        assert_eq!(members[0].character_id, 3);
        assert_eq!(members[0].weapon, 10);
        assert_eq!(members[0].team, Some(2));
        assert_eq!(rules.friendly_fire, Some(true));
    }

    #[test]
    fn web_server_authority_start_accepts_null_host() {
        let message = Message::from_v4_binary(&hex_bytes("08068201e701082912e2010a0d0a06726f6f6d496412031201720a170a0a6d6174636845706f6368120909000000000000f03f0a170a0a726f756e6445706f6368120909000000000000f03f0a140a046d6f6465120c120a64656174686d617463680a110a0472756c6512090900000000000014400a110a077374616765496412061204736e30310a180a0b706c61796572436f756e7412090900000000000000400a150a086e7063436f756e7412090900000000000000000a0d0a07726573756d6564120218000a150a09617574686f72697479120812067365727665720a0c0a06686f7374496412023800" )).unwrap();
        assert!(
            matches!(message, Message::MatchStart { host_id, authority, .. } if host_id.is_empty() && authority == "server")
        );
    }

    #[test]
    fn web_asset_ready_success_and_failure_round_trip() {
        for ok in [true, false] {
            let message = Message::MatchAssetReady {
                attempt_id: "asset_test".into(),
                ok,
            };
            assert_eq!(
                Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap(),
                message
            );
        }
    }

    #[test]
    fn legacy_control_kinds_have_rust_mappings() {
        let source = include_str!("../tests/fixtures/legacy_v6_codec.mjs");
        let table = source
            .split_once("var TYPE_TO_KIND =")
            .unwrap()
            .1
            .split_once("]);")
            .unwrap()
            .0;
        let mut count = 0;
        for line in table.lines() {
            let Some(entry) = line.trim().strip_prefix("[\"") else {
                continue;
            };
            let (name, number) = entry.split_once("\", ").unwrap();
            let number: i32 = number.split_whitespace().next().unwrap().parse().unwrap();
            assert_eq!(
                kind_for_type(name).map(|kind| kind as i32),
                Some(number),
                "encode {name}"
            );
            assert_eq!(type_for_kind(number), Some(name), "decode {number}: {name}");
            count += 1;
        }
        assert!(
            count >= 51,
            "Web table parsing must not silently skip entries"
        );
        assert_eq!(type_for_kind(12), Some("room.list"));
        assert_eq!(type_for_kind(13), Some("room.list"));
        for value in 1..=255 {
            if MessageKind::try_from(value).is_ok() {
                assert!(
                    type_for_kind(value).is_some(),
                    "unmapped Rust wire kind {value}"
                );
            }
        }
    }

    #[test]
    fn self_effect_request_round_trips_json_and_binary() {
        let request = Message::RoleSelfEffect {
            seq: 17,
            room_id: "room1".into(),
            match_epoch: 2,
            round_epoch: 3,
            pose_id: 489,
            pose_instance: 65536,
        };
        assert_eq!(
            Message::from_json(&serde_json::to_string(&request).unwrap()).unwrap(),
            request
        );
        assert_eq!(
            Message::from_v4_binary(&request.to_v4_binary().unwrap()).unwrap(),
            request
        );
    }

    #[test]
    fn pose_chain_decodes_real_web_frame_and_round_trips_every_field() {
        let frame = hex_bytes("08068201e501085b12e0010a100a0373657112090900000000000045400a110a06726f6f6d496412071205726f6f6d310a100a08706c6179657249641204120270320a170a0a6d6174636845706f6368120909000000000000f03f0a170a0a726f756e6445706f6368120909000000000000f03f0a110a047469636b12090900000000000037400a130a06706f736549641209090000000000a06f400a1b0a0e6d696e696d756d5365636f6e647312090900000000000000000a150a0873657175656e636512090900000000003886400a190a0c626c656e645365636f6e6473120909333333333333d33f");
        let expected = Message::RolePoseChain {
            seq: 42,
            room_id: "room1".into(),
            player_id: "p2".into(),
            match_epoch: 1,
            round_epoch: 1,
            tick: 23,
            chain: crate::role_state::PoseChain {
                pose_id: 253,
                minimum_seconds: 0.0,
                sequence: 711,
                blend_seconds: 0.3,
            },
        };
        assert_eq!(Message::from_v4_binary(&frame).unwrap(), expected);
        assert_eq!(
            Message::from_v4_binary(&expected.to_v4_binary().unwrap()).unwrap(),
            expected
        );
    }

    #[test]
    fn room_latency_reports_accept_missing_ids_and_null_rtt() {
        let message = Message::from_json(
            r#"{"type":"room.latency","serverRtt":null,"hostRtt":null,"p2pState":"waiting"}"#,
        )
        .unwrap();
        assert_eq!(
            Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap(),
            message
        );
        assert!(
            matches!(message, Message::RoomLatency { room_id, player_id, latency }
            if room_id.is_empty() && player_id.is_empty() && latency.server_rtt.is_none() && latency.host_rtt.is_none())
        );
    }

    #[test]
    fn web_room_latency_broadcast_decodes() {
        let frame = hex_bytes("0806820175081412710a140a06726f6f6d4964120a1208726f6f6d2d7765620a180a08706c617965724964120c120a7765622d706c617965720a160a0973657276657252747412090900000000000043400a0d0a07686f7374527474120238000a180a087032705374617465120c120a636f6e6e656374696e67");
        let message = Message::from_v4_binary(&frame).expect("kind 20 must decode as room.latency");
        let Message::RoomLatency {
            ref room_id,
            ref player_id,
            ref latency,
        } = message
        else {
            panic!("room.latency");
        };
        assert_eq!(room_id, "room-web");
        assert_eq!(player_id, "web-player");
        assert_eq!(latency.server_rtt, Some(38.0));
        assert_eq!(latency.host_rtt, None);
        assert_eq!(latency.p2p_state, "connecting");
        assert_eq!(
            Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap(),
            message
        );
    }

    #[test]
    fn web_asset_prepare_frame_decodes() {
        let frame = hex_bytes("080682018802082e1283020a0d0a06726f6f6d496412031201720a170a0a6d6174636845706f6368120909000000000000f03f0a170a0a726f756e6445706f636812090900000000000000000a190a09617474656d70744964120c120a61737365745f746573740a180a07707572706f7365120d120b6d617463682d73746172740a0e0a086a6f696e65724964120238000a7b0a07706c61796572731270326e0a342a320a0a0a0269641204120270310a110a096368617261637465721204120270300a110a06776561706f6e1207120573776f72640a362a340a0a0a0269641204120270320a110a096368617261637465721204120270310a130a06776561706f6e120912077374696e676572");
        let message = Message::from_v4_binary(&frame)
            .expect("Web asset preparation must decode before match.start");
        let Message::MatchAssetPrepare {
            ref players,
            ref attempt_id,
            joiner_id: None,
            ..
        } = message
        else {
            panic!("assetPrepare");
        };
        assert_eq!(attempt_id, "asset_test");
        assert_eq!((players[1].character_id, players[1].weapon), (1, 4));
        assert_eq!(
            Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap(),
            message
        );
    }

    #[test]
    fn web_match_start_without_private_roster_fields_decodes() {
        let json = r#"{
            "type":"match.start",
            "roomId":"room-web",
            "mode":"deathmatch",
            "stageId":"sn03",
            "playerCount":2,
            "npcCount":0,
            "authority":"server"
        }"#;

        assert!(matches!(
            serde_json::from_str::<Message>(json).expect("Web match.start decodes"),
            Message::MatchStart { mode, stage, players, slots, .. }
                if mode == "deathmatch" && stage == "sn03"
                    && players.is_empty() && slots.is_empty()
        ));
    }

    #[test]
    fn web_protocol_codec_room_and_match_frames_decode() {
        let room = Message::from_v4_binary(&hex_bytes(
            "0806820191020816128c020a140a06726f6f6d4964120a1208726f6f6d2d7765620a140a046d6f6465120c120a64656174686d617463680a110a077374616765496412061204736e30330a97010a07706c6179657273128b013288010a85012a82010a0e0a026964120812067765622d70310a140a046e616d65120c120a57656220506c617965720a110a096368617261637465721204120270330a160a06776561706f6e120c120a7374616666426c6164650a110a047465616d12090900000000000000400a0b0a057265616479120218010a0f0a09636f6e6e6563746564120218010a100a057374617465120712056c6f6262790a1f0a0572756c657312162a140a120a0c667269656e646c794669726512021801",
        ))
        .expect("Web protocolCodec room.state frame decodes");
        assert!(matches!(
            room,
            Message::RoomState { stage, members, rules, .. }
                if stage == "sn03" && members[0].character_id == 3
                    && members[0].weapon == 10 && rules.friendly_fire == Some(true)
        ));

        let start = Message::from_v4_binary(&hex_bytes(
            "080682018c0108291287010a140a06726f6f6d4964120a1208726f6f6d2d7765620a140a046d6f6465120c120a64656174686d617463680a110a077374616765496412061204736e30330a180a0b706c61796572436f756e7412090900000000000000400a150a086e7063436f756e7412090900000000000000000a150a09617574686f7269747912081206736572766572",
        ))
        .expect("Web protocolCodec match.start frame decodes");
        assert!(matches!(
            start,
            Message::MatchStart { stage, players, slots, .. }
                if stage == "sn03" && players.is_empty() && slots.is_empty()
        ));
    }

    #[test]
    fn web_protocol_codec_role_snapshot_frame_decodes() {
        let frame = hex_bytes(
            "08068a012c0a05726f6f6d3110091900000000004a934022140a027032120e0102030405060708090a0b0c0d0e28073003",
        );
        assert!(matches!(
            Message::from_v4_payload(&frame).expect("Web role.snapshot decodes"),
            V4Payload::RoleSnapshot(snapshot)
                if snapshot.room_id == "room1" && snapshot.tick == 9
                    && snapshot.match_epoch == Some(7)
                    && snapshot.round_epoch == Some(3)
                    && snapshot.roles[0].id == "p2"
                    && snapshot.roles[0].role_state == (1_u8..=14).collect::<Vec<_>>()
        ));
    }

    #[test]
    fn web_protocol_codec_weapon_drop_rest_frame_decodes() {
        let frame = hex_bytes(
            "080682017a084412760a170a0a6d6174636845706f63681209090000000000001c400a170a0a726f756e6445706f636812090900000000000008400a110a03726566120a1208445f77704e6574370a2f0a08706f736974696f6e122332210a0909000000000000f03f0a090900000000000000400a09090000000000000840",
        );
        assert_eq!(
            Message::from_v4_binary(&frame).expect("Web weapon.dropRest decodes"),
            Message::WeaponDropRest {
                match_epoch: Some(7),
                round_epoch: Some(3),
                reference: "D_wpNet7".into(),
                position: [1.0, 2.0, 3.0],
            }
        );
    }

    #[test]
    fn anger_spend_action_matches_web_canonical_encoding() {
        let encoded = encode_anger_spend_action(17, 150, 60).unwrap();
        assert_eq!(encoded, "anger_spend:17:150:60");
        assert_eq!(
            parse_anger_spend_action(&encoded),
            Some(AngerSpendAction {
                pose_instance_id: 17,
                action_id: 150,
                cost: 60,
            })
        );
        assert!(parse_anger_spend_action("anger_spend:017:150:60").is_none());
        assert!(parse_anger_spend_action("anger_spend:17:150:0").is_none());
        assert!(encode_anger_spend_action(17, 2049, 60).is_none());
    }

    #[test]
    fn v4_control_round_trips_role_rate_event64_equivalent() {
        let message = Message::RoleRate { rate: 17 };
        let frame = message.to_v4_binary().expect("role.rate encodes");
        assert_eq!(Message::from_v4_binary(&frame).unwrap(), message);
    }

    #[test]
    fn v4_control_round_trips_scene_objective_end() {
        let message = Message::ObjectiveEnd { winner_team: 2 };
        let frame = message.to_v4_binary().expect("objective.end encodes");
        assert_eq!(Message::from_v4_binary(&frame).unwrap(), message);
    }

    #[test]
    fn v4_control_round_trips_scene_net_event_transaction() {
        let message = Message::SceneOps {
            room_id: "room1".into(),
            authority_generated: true,
            ops: vec![SceneOp {
                kind: "set".into(),
                reference: JsonValue::String("D_BBox01".into()),
                field: "attribute".into(),
                args: vec![JsonValue::String("interactive".into()), JsonValue::from(0)],
            }],
        };
        let frame = message.to_v4_binary().expect("scene.ops encodes");
        assert_eq!(Message::from_v4_binary(&frame).unwrap(), message);
    }

    #[test]
    fn scene_prop_hit_round_trips_reliable_control() {
        let message = Message::ScenePropHit {
            room_id: "room1".into(),
            match_epoch: 3,
            round_epoch: 2,
            player_id: "p2".into(),
            reference: "D_BBox01".into(),
            pose_id: 200,
            frame: 12.0,
            damage: 126,
        };
        let frame = message.to_v4_binary().unwrap();
        assert_eq!(Message::from_v4_binary(&frame).unwrap(), message);
        assert_eq!(
            serde_json::from_str::<Message>(&serde_json::to_string(&message).unwrap()).unwrap(),
            message
        );
    }

    #[test]
    fn v4_control_decodes_web_objective_end_bytes() {
        let frame =
            hex_bytes("080682011d084312190a170a0a77696e6e65725465616d1209090000000000000040");
        assert_eq!(
            Message::from_v4_binary(&frame).unwrap(),
            Message::ObjectiveEnd { winner_team: 2 }
        );
    }

    #[test]
    fn v4_control_decodes_web_team_select_bytes() {
        let frame = hex_bytes("0806820117082012130a110a047465616d1209090000000000000040");
        assert_eq!(
            Message::from_v4_binary(&frame).unwrap(),
            Message::TeamSelect { team: 2 }
        );
    }

    #[test]
    fn v4_control_decodes_web_round_lifecycle_bytes() {
        let team_wins = Some(std::collections::BTreeMap::from([
            ("1".to_string(), 0),
            ("2".to_string(), 0),
        ]));
        assert_eq!(
            Message::from_v4_binary(&hex_bytes("08068201a901082c12a4010a170a0a6d6174636845706f63681209090000000000001c400a170a0a726f756e6445706f636812090900000000000008400a120a05726f756e64120909000000000000f03f0a160a09726f756e6454696d651209090000000000002e400a140a0767616d654e756d12090900000000000008400a2e0a087465616d57696e7312222a200a0e0a013112090900000000000000000a0e0a01321209090000000000000000")).unwrap(),
            Message::RoundStart {
                match_epoch: Some(7),
                round_epoch: Some(3),
                round: 1,
                round_time: 15,
                game_num: 3,
                team_wins: team_wins.clone(),
            }
        );
        assert_eq!(
            Message::from_v4_binary(&hex_bytes("08068201cb01082d12c6010a170a0a6d6174636845706f63681209090000000000001c400a170a0a726f756e6445706f636812090900000000000008400a120a05726f756e64120909000000000000f03f0a100a0a77696e6e65725465616d120238000a110a0b77696e6e65725465616d73120232000a140a0e77696e6e6572506c617965724964120238000a130a06726561736f6e1209120774696d656f75740a2e0a087465616d57696e7312222a200a0e0a013112090900000000000000000a0e0a01321209090000000000000000")).unwrap(),
            Message::RoundEnd {
                match_epoch: Some(7),
                round_epoch: Some(3),
                round: 1,
                winner_team: None,
                winner_teams: Vec::new(),
                winner_player_id: None,
                reason: "timeout".into(),
                team_wins,
            }
        );
        assert_eq!(
            Message::from_v4_binary(&hex_bytes("0806820104082b1200")).unwrap(),
            Message::MatchReset
        );
        assert_eq!(
            Message::from_v4_binary(&hex_bytes("0806820174082a12700a100a0a77696e6e65725465616d120238000a140a0e77696e6e6572506c617965724964120238000a100a06726573756c7412061204647261770a130a06726561736f6e1209120774696d656f75740a1f0a126c6f62627952657475726e5365636f6e64731209090000000000002440")).unwrap(),
            Message::MatchEnd {
                winner_team: None,
                winner_player_id: None,
                result: Some("draw".into()),
                reason: "timeout".into(),
                lobby_return_seconds: Some(10.0),
            }
        );
    }

    #[test]
    fn v4_control_round_trips_authoritative_combat_hit_fields() {
        let messages = [
            Message::CombatHit {
                target: "p2".into(),
                damage: 95.0,
                attacker_id: Some("p1".into()),
                guarded: Some(false),
                health: Some(1905.0),
                anger: Some(13.3),
                motion_scale: Some(0.25),
                reaction: Some(CombatReactionWire {
                    defense_move: 2.0,
                    target_move: 0.25,
                    defense_value: 0.3,
                    target_value: 0.5,
                    target_pose: Some(93),
                    target_pose_front: Some(93),
                    target_pose_back: Some(94),
                    target_pose_left: Some(92),
                    target_pose_right: Some(91),
                }),
                tick: Some(77),
            },
            Message::CombatDeath {
                attacker_id: Some("p1".into()),
                target_id: "p2".into(),
                respawn_seconds: Some(3.0),
                tick: Some(77),
            },
            Message::CombatRespawn {
                player_id: "p2".into(),
                reason: "cure".into(),
                healer_id: Some("p1".into()),
            },
            Message::CombatCure {
                healer_id: "p1".into(),
                target_id: "p2".into(),
                position: Some([1.0, 2.0, 3.0]),
            },
            Message::ItemClaim {
                reference: "D_It01".into(),
                player_id: "p2".into(),
                model: "item01".into(),
                position: Some([1.0, 2.0, 3.0]),
                renewable: true,
            },
            Message::ItemClaimed {
                reference: "D_Wp01".into(),
                player_id: "p2".into(),
                model: "W0_0".into(),
                kind: "weapon".into(),
                picked_weapon_model: Some("w0_0".into()),
                weapon: "sword".into(),
                weapon_model: Some("w0_0".into()),
                weapon_slot: 0,
                weapon_slots: vec![ItemWeaponSlotWire {
                    index: 0,
                    present: true,
                    weapon: "sword".into(),
                    weapon_id: Some(6),
                    model: Some("w0_0".into()),
                    resource_class: 0,
                    variant: 0,
                    attack_power: 110.0,
                    defense_power: 15.0,
                }],
                health: 1000.0,
                max_health: 1000.0,
                anger: 0.0,
                respawn_seconds: Some(30.0),
                item_effects: vec![ItemEffectWire {
                    model: "item02".into(),
                    remaining: 599.5,
                }],
            },
            Message::ItemRespawned {
                reference: "D_It01".into(),
                model: "item01".into(),
            },
            Message::ItemEffect {
                player_id: "p2".into(),
                health: 980.0,
                max_health: 1000.0,
                anger: 11.0,
                item_effects: vec![ItemEffectWire {
                    model: "hot".into(),
                    remaining: 30.0,
                }],
            },
            Message::WeaponSwitched {
                player_id: "p2".into(),
                weapon: "blade".into(),
                weapon_model: Some("w5_0".into()),
                weapon_slot: 1,
                weapon_slots: vec![],
            },
            Message::WeaponDropped {
                player_id: "p2".into(),
                reference: "D_wpNet1".into(),
                model: "w5_0".into(),
                position: [1.0, 6.0, 3.0],
                velocity: [0.0, 0.0, 12.0],
                weapon: "sword".into(),
                weapon_model: Some("w0_0".into()),
                weapon_slot: 0,
                weapon_slots: vec![],
            },
            Message::EscortTaken {
                player_id: "p1".into(),
                team: 1,
                position: [1.0, 2.0, 3.0],
            },
            Message::EscortDropped {
                player_id: "p1".into(),
                position: [1.0, 2.0, 3.0],
                return_seconds: 5.0,
            },
            Message::EscortDelivered {
                player_id: "p1".into(),
                team: 1,
                safe_zone: "safeA".into(),
            },
            Message::EscortReturned {
                reason: "timeout".into(),
                position: [1.0, 2.0, 3.0],
            },
        ];
        for message in messages {
            let frame = message.to_v4_binary().expect("combat event encodes");
            assert_eq!(Message::from_v4_binary(&frame).unwrap(), message);
        }
    }

    #[test]
    fn v4_room_list_uses_request_and_response_kinds() {
        let request = Message::RoomList { rooms: None };
        let request_frame = request.to_v4_binary().expect("request encodes");
        let request_wire = <WireEnvelope as prost::Message>::decode(request_frame.as_slice())
            .expect("request envelope decodes");
        assert!(matches!(
            request_wire.payload,
            Some(envelope::Payload::Control(WireControl {
                kind,
                ..
            })) if kind == MessageKind::RoomListRequest as i32
        ));
        assert_eq!(Message::from_v4_binary(&request_frame).unwrap(), request);

        let response = Message::RoomList {
            rooms: Some(vec![RoomSummary {
                room_id: "room1".into(),
                name: "Rust room".into(),
                mode: Some("leader".into()),
                stage_id: Some("sn01".into()),
                configured: true,
                state: "lobby".into(),
                has_password: false,
                player_count: 1,
                max_players: 16,
                team_counts: None,
                owner_name: Some("Rust".into()),
            }]),
        };
        let response_frame = response.to_v4_binary().expect("response encodes");
        let response_wire = <WireEnvelope as prost::Message>::decode(response_frame.as_slice())
            .expect("response envelope decodes");
        assert!(matches!(
            response_wire.payload,
            Some(envelope::Payload::Control(WireControl {
                kind,
                ..
            })) if kind == MessageKind::RoomList as i32
        ));
        assert_eq!(Message::from_v4_binary(&response_frame).unwrap(), response);
    }

    #[test]
    fn v4_payload_round_trips_dedicated_input_and_snapshot_schemas() {
        use crate::wire_v4::{CombatJudge, InputVisual, PackedObb, SnapshotPlayer, Vec2, Vec3};

        let input = WireInput {
            seq: 42,
            role_state: vec![1, 2, 3],
            role_rate_request: Some(20),
            r#move: Some(Vec2 { x: -0.25, z: 1.0 }),
            yaw: 1.5,
            run: true,
            guard: false,
            actions: vec!["attack".into()],
            combat: vec![CombatJudge {
                kind: "precise".into(),
                pose_id: 24,
                attack_index: 1,
                pose_instance_id: 7,
                frame: 12.5,
                cursor: 13.0,
                boxes: vec![PackedObb {
                    values: vec![0.0; 15],
                }],
                ranged_kind: String::new(),
                start: Some(Vec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                }),
                end: None,
                target_ids: vec!["p2".into()],
            }],
            visual: Some(InputVisual {
                pose_id: 24,
                cursor: 13.0,
                lift: 0.2,
                character: "p3".into(),
                weapon: "blade".into(),
                world_position: Some(Vec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                }),
                carries_hurt_obbs: true,
                hurt_obbs: vec![PackedObb {
                    values: vec![1.0; 15],
                }],
                carries_hurt_whole_obb: false,
                hurt_whole_obb: None,
            }),
            match_epoch: Some(7),
            round_epoch: Some(3),
        };
        let input_frame = V4Payload::Input(input).to_v4_binary().unwrap();
        assert!(matches!(
            Message::from_v4_payload(&input_frame).unwrap(),
            V4Payload::Input(decoded)
                if decoded.seq == 42
                    && decoded.role_state == [1, 2, 3]
                    && decoded.r#move.as_ref().is_some_and(|move_state| move_state.x == -0.25 && move_state.z == 1.0)
                    && decoded.combat[0].target_ids == ["p2"]
                    && decoded.visual.as_ref().is_some_and(|visual| visual.hurt_obbs[0].values.len() == 15)
        ));
        assert!(matches!(
            Message::from_v4_binary(&input_frame),
            Err(V4CodecError::UnsupportedPayload)
        ));

        let snapshot = WireSnapshot {
            room_id: "room1".into(),
            tick: 99,
            server_time: 1234.5,
            players: vec![SnapshotPlayer {
                id: "p1".into(),
                name: "Web".into(),
                character: "p3".into(),
                weapon: "blade".into(),
                weapon_model: Some("W0_0".into()),
                weapon_slot: 1,
                team: Some(2),
                is_leader: true,
                connected: true,
                ready: true,
                health: 900.0,
                max_health: Some(1000.0),
                anger: 60.0,
                kills: 2,
                round_kills: 1,
                round_wins: 3,
                deaths: 4,
                guard: true,
                alive: true,
                pose_id: 24,
                pose_cursor: 12.5,
                lift: 0.0,
                role_state: vec![4, 5],
                position: Some(Vec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                }),
                yaw: Some(1.5),
                weapon_slots: vec![],
                item_effects: vec![],
                worn_effects: vec![],
            }],
            npcs: vec![],
            drops: vec![],
            flag: None,
            r#match: None,
            match_epoch: Some(7),
            round_epoch: Some(3),
        };
        let snapshot_frame = V4Payload::Snapshot(snapshot).to_v4_binary().unwrap();
        assert!(matches!(
            Message::from_v4_payload(&snapshot_frame).unwrap(),
            V4Payload::Snapshot(decoded)
                if decoded.room_id == "room1"
                    && decoded.tick == 99
                    && decoded.players[0].role_state == [4, 5]
                    && decoded.players[0].position.as_ref().is_some_and(|position| position.z == 3.0)
        ));
    }

    #[test]
    fn v4_payload_decodes_web_protocol_codec_input_and_snapshot_bytes() {
        let input = hex_bytes(
            "0806528602082a12030102031814220a0d000080be150000803f2d0000c03f3001420661747461636b4a6e0a07707265636973651018180120072d0000484135000050413a3e0a3c0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000004a0f0d0000803f15000000401d000040405a027032526c081815000050411dcdcc4c3e220270332a05626c616465320f0d0000803f15000000401d000040403801423e0a3c0000803f0000803f0000803f0000803f0000803f0000803f0000803f0000803f0000803f0000803f0000803f0000803f0000803f0000803f0000803f480158076003",
        );
        assert!(matches!(
            Message::from_v4_payload(&input).unwrap(),
            V4Payload::Input(decoded)
                if decoded.seq == 42
                    && decoded.role_rate_request == Some(20)
                    && decoded.match_epoch == Some(7)
                    && decoded.round_epoch == Some(3)
                    && decoded.role_state == [1, 2, 3]
                    && decoded.r#move.as_ref().is_some_and(|move_state| move_state.x == -0.25 && move_state.z == 1.0)
                    && decoded.combat[0].kind == "precise"
                    && decoded.combat[0].target_ids == ["p2"]
                    && decoded.visual.as_ref().is_some_and(|visual| visual.character == "p3" && visual.hurt_obbs.len() == 1)
        ));

        let snapshot = hex_bytes(
            "08065a81010a05726f6f6d3110631900000000004a934022690a02703112035765621a0270332205626c6164652a0457305f30300138024001480150015d000061446500007a446d0000704270027801800103880104900101980101a00118ad0100004841ba01020405c2010f0d0000803f15000000401d00004040cd010000c03f48075003",
        );
        assert!(matches!(
            Message::from_v4_payload(&snapshot).unwrap(),
            V4Payload::Snapshot(decoded)
                if decoded.room_id == "room1"
                    && decoded.tick == 99
                    && decoded.players[0].weapon_model.as_deref() == Some("W0_0")
                    && decoded.players[0].team == Some(2)
                    && decoded.players[0].role_state == [4, 5]
                    && decoded.match_epoch == Some(7)
                    && decoded.round_epoch == Some(3)
                    && decoded.players[0].position.as_ref().is_some_and(|position| position.x == 1.0 && position.z == 3.0)
        ));
    }

    #[test]
    fn v4_rejects_legacy_only_snapshot() {
        let message = Message::Snapshot {
            player_id: None,
            fighter: FighterSnapshot {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                yaw: 0.0,
                pose_id: 0,
                pose_frame: 0,
                pose_start: 0,
                pose_end: 0,
                pose_loop_start: 0,
                pose_loop_end: 0,
                pose_loops: false,
                hp: 100.0,
                max_hp: 100.0,
                weapon: 6,
                movement_mode: 0,
                kills: 0,
            },
        };
        assert!(matches!(
            message.to_v4_binary(),
            Err(V4CodecError::UnsupportedType("snapshot"))
        ));
    }

    #[test]
    fn v4_escort_flag_projection_preserves_authority_fields() {
        let value = serde_json::json!({
            "position": [26.937681, 0.59184, -155.95084],
            "carrierId": null,
            "state": "dropped",
            "returnIn": 4.25,
        });
        let object = dynamic_object_from_json(value.as_object().unwrap());
        assert_eq!(
            escort_flag_from_v4(Some(&object)),
            Some(EscortFlagSnapshot {
                position: [26.937_681, 0.591_84, -155.950_84],
                carrier_id: None,
                state: "dropped".into(),
                return_in: Some(4.25),
            })
        );
    }

    fn hex_bytes(hex: &str) -> Vec<u8> {
        assert_eq!(hex.len() % 2, 0, "hex string must have full octets");
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect()
    }
}
