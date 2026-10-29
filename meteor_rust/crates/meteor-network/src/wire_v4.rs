use prost::Message;

pub const PROTOCOL_VERSION: u32 = 6;

#[derive(Clone, PartialEq, Message)]
pub struct Envelope {
    #[prost(uint32, tag = "1")]
    pub protocol: u32,
    #[prost(oneof = "envelope::Payload", tags = "10, 11, 12, 13, 14, 15, 16, 17")]
    pub payload: Option<envelope::Payload>,
}

pub mod envelope {
    use super::{
        Control, Input, RelayFromHost, RelayFromPeer, RelayHost, RelayPeer, RoleSnapshot, Snapshot,
    };
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Payload {
        #[prost(message, tag = "10")]
        Input(Input),
        #[prost(message, tag = "11")]
        Snapshot(Snapshot),
        #[prost(message, tag = "12")]
        RelayHost(RelayHost),
        #[prost(message, tag = "13")]
        RelayPeer(RelayPeer),
        #[prost(message, tag = "14")]
        RelayFromHost(RelayFromHost),
        #[prost(message, tag = "15")]
        RelayFromPeer(RelayFromPeer),
        #[prost(message, tag = "16")]
        Control(Control),
        #[prost(message, tag = "17")]
        RoleSnapshot(RoleSnapshot),
    }
}

#[derive(Clone, PartialEq, Message)]
pub struct Control {
    #[prost(enumeration = "MessageKind", tag = "1")]
    pub kind: i32,
    #[prost(message, optional, tag = "2")]
    pub fields: Option<DynamicObject>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DynamicObject {
    #[prost(message, repeated, tag = "1")]
    pub fields: Vec<DynamicField>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DynamicField {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(message, optional, tag = "2")]
    pub value: Option<DynamicValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DynamicList {
    #[prost(message, repeated, tag = "1")]
    pub values: Vec<DynamicValue>,
}

#[derive(Clone, PartialEq, Message)]
pub struct DynamicValue {
    #[prost(oneof = "dynamic_value::Kind", tags = "1, 2, 3, 4, 5, 6, 7")]
    pub kind: Option<dynamic_value::Kind>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Vec2 {
    #[prost(float, tag = "1")]
    pub x: f32,
    #[prost(float, tag = "2")]
    pub z: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct Vec3 {
    #[prost(float, tag = "1")]
    pub x: f32,
    #[prost(float, tag = "2")]
    pub y: f32,
    #[prost(float, tag = "3")]
    pub z: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct PackedObb {
    #[prost(float, repeated, packed = "true", tag = "1")]
    pub values: Vec<f32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct CombatJudge {
    #[prost(string, tag = "1")]
    pub kind: String,
    #[prost(uint32, tag = "2")]
    pub pose_id: u32,
    #[prost(uint32, tag = "3")]
    pub attack_index: u32,
    #[prost(uint32, tag = "4")]
    pub pose_instance_id: u32,
    #[prost(float, tag = "5")]
    pub frame: f32,
    #[prost(float, tag = "6")]
    pub cursor: f32,
    #[prost(message, repeated, tag = "7")]
    pub boxes: Vec<PackedObb>,
    #[prost(string, tag = "8")]
    pub ranged_kind: String,
    #[prost(message, optional, tag = "9")]
    pub start: Option<Vec3>,
    #[prost(message, optional, tag = "10")]
    pub end: Option<Vec3>,
    #[prost(string, repeated, tag = "11")]
    pub target_ids: Vec<String>,
}

#[derive(Clone, PartialEq, Message)]
pub struct InputVisual {
    #[prost(uint32, tag = "1")]
    pub pose_id: u32,
    #[prost(float, tag = "2")]
    pub cursor: f32,
    #[prost(float, tag = "3")]
    pub lift: f32,
    #[prost(string, tag = "4")]
    pub character: String,
    #[prost(string, tag = "5")]
    pub weapon: String,
    #[prost(message, optional, tag = "6")]
    pub world_position: Option<Vec3>,
    #[prost(bool, tag = "7")]
    pub carries_hurt_obbs: bool,
    #[prost(message, repeated, tag = "8")]
    pub hurt_obbs: Vec<PackedObb>,
    #[prost(bool, tag = "9")]
    pub carries_hurt_whole_obb: bool,
    #[prost(message, optional, tag = "10")]
    pub hurt_whole_obb: Option<PackedObb>,
}

#[derive(Clone, PartialEq, Message)]
pub struct Input {
    #[prost(uint32, tag = "1")]
    pub seq: u32,
    #[prost(bytes, tag = "2")]
    pub role_state: Vec<u8>,
    #[prost(uint32, optional, tag = "3")]
    pub role_rate_request: Option<u32>,
    #[prost(message, optional, tag = "4")]
    pub r#move: Option<Vec2>,
    #[prost(float, tag = "5")]
    pub yaw: f32,
    #[prost(bool, tag = "6")]
    pub run: bool,
    #[prost(bool, tag = "7")]
    pub guard: bool,
    #[prost(string, repeated, tag = "8")]
    pub actions: Vec<String>,
    #[prost(message, repeated, tag = "9")]
    pub combat: Vec<CombatJudge>,
    #[prost(message, optional, tag = "10")]
    pub visual: Option<InputVisual>,
    #[prost(uint32, optional, tag = "11")]
    pub match_epoch: Option<u32>,
    #[prost(uint32, optional, tag = "12")]
    pub round_epoch: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct WeaponSlot {
    #[prost(uint32, tag = "1")]
    pub index: u32,
    #[prost(bool, tag = "2")]
    pub present: bool,
    #[prost(string, tag = "3")]
    pub weapon: String,
    #[prost(uint32, optional, tag = "4")]
    pub weapon_id: Option<u32>,
    #[prost(string, optional, tag = "5")]
    pub model: Option<String>,
    #[prost(uint32, tag = "6")]
    pub resource_class: u32,
    #[prost(uint32, tag = "7")]
    pub variant: u32,
    #[prost(float, tag = "8")]
    pub attack_power: f32,
    #[prost(float, tag = "9")]
    pub defense_power: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct ItemEffect {
    #[prost(string, tag = "1")]
    pub model: String,
    #[prost(float, tag = "2")]
    pub remaining: f32,
}

#[derive(Clone, PartialEq, Message)]
pub struct SnapshotPlayer {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub name: String,
    #[prost(string, tag = "3")]
    pub character: String,
    #[prost(string, tag = "4")]
    pub weapon: String,
    #[prost(string, optional, tag = "5")]
    pub weapon_model: Option<String>,
    #[prost(uint32, tag = "6")]
    pub weapon_slot: u32,
    #[prost(uint32, optional, tag = "7")]
    pub team: Option<u32>,
    #[prost(bool, tag = "8")]
    pub is_leader: bool,
    #[prost(bool, tag = "9")]
    pub connected: bool,
    #[prost(bool, tag = "10")]
    pub ready: bool,
    #[prost(float, tag = "11")]
    pub health: f32,
    #[prost(float, optional, tag = "12")]
    pub max_health: Option<f32>,
    #[prost(float, tag = "13")]
    pub anger: f32,
    #[prost(uint32, tag = "14")]
    pub kills: u32,
    #[prost(uint32, tag = "15")]
    pub round_kills: u32,
    #[prost(uint32, tag = "16")]
    pub round_wins: u32,
    #[prost(uint32, tag = "17")]
    pub deaths: u32,
    #[prost(bool, tag = "18")]
    pub guard: bool,
    #[prost(bool, tag = "19")]
    pub alive: bool,
    #[prost(uint32, tag = "20")]
    pub pose_id: u32,
    #[prost(float, tag = "21")]
    pub pose_cursor: f32,
    #[prost(float, tag = "22")]
    pub lift: f32,
    #[prost(bytes, tag = "23")]
    pub role_state: Vec<u8>,
    #[prost(message, optional, tag = "24")]
    pub position: Option<Vec3>,
    #[prost(float, optional, tag = "25")]
    pub yaw: Option<f32>,
    #[prost(message, repeated, tag = "26")]
    pub weapon_slots: Vec<WeaponSlot>,
    #[prost(message, repeated, tag = "27")]
    pub item_effects: Vec<ItemEffect>,
    #[prost(message, repeated, tag = "28")]
    pub worn_effects: Vec<ItemEffect>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SnapshotNpc {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(string, tag = "2")]
    pub kind: String,
    #[prost(string, tag = "3")]
    pub name: String,
    #[prost(string, tag = "4")]
    pub character: String,
    #[prost(string, tag = "5")]
    pub weapon: String,
    #[prost(uint32, optional, tag = "6")]
    pub team: Option<u32>,
    #[prost(bool, tag = "7")]
    pub vip: bool,
    #[prost(message, optional, tag = "8")]
    pub position: Option<Vec3>,
    #[prost(float, tag = "9")]
    pub yaw: f32,
    #[prost(float, tag = "10")]
    pub health: f32,
    #[prost(float, tag = "11")]
    pub max_health: f32,
    #[prost(uint32, tag = "12")]
    pub kills: u32,
    #[prost(uint32, tag = "13")]
    pub deaths: u32,
    #[prost(bool, tag = "14")]
    pub alive: bool,
    #[prost(uint32, tag = "15")]
    pub pose_id: u32,
    #[prost(float, tag = "16")]
    pub pose_cursor: f32,
    #[prost(bytes, tag = "17")]
    pub role_state: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct SnapshotDrop {
    #[prost(string, tag = "1")]
    pub r#ref: String,
    #[prost(string, tag = "2")]
    pub model: String,
    #[prost(message, optional, tag = "3")]
    pub position: Option<Vec3>,
    #[prost(bool, tag = "4")]
    pub settled: bool,
}

#[derive(Clone, PartialEq, Message)]
pub struct Snapshot {
    #[prost(string, tag = "1")]
    pub room_id: String,
    #[prost(uint32, tag = "2")]
    pub tick: u32,
    #[prost(double, tag = "3")]
    pub server_time: f64,
    #[prost(message, repeated, tag = "4")]
    pub players: Vec<SnapshotPlayer>,
    #[prost(message, repeated, tag = "5")]
    pub npcs: Vec<SnapshotNpc>,
    #[prost(message, repeated, tag = "6")]
    pub drops: Vec<SnapshotDrop>,
    #[prost(message, optional, tag = "7")]
    pub flag: Option<DynamicObject>,
    #[prost(message, optional, tag = "8")]
    pub r#match: Option<DynamicObject>,
    #[prost(uint32, optional, tag = "9")]
    pub match_epoch: Option<u32>,
    #[prost(uint32, optional, tag = "10")]
    pub round_epoch: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct RoleStateRecord {
    #[prost(string, tag = "1")]
    pub id: String,
    #[prost(bytes, tag = "2")]
    pub role_state: Vec<u8>,
}

#[derive(Clone, PartialEq, Message)]
pub struct RoleSnapshot {
    #[prost(string, tag = "1")]
    pub room_id: String,
    #[prost(uint32, tag = "2")]
    pub tick: u32,
    #[prost(double, tag = "3")]
    pub server_time: f64,
    #[prost(message, repeated, tag = "4")]
    pub roles: Vec<RoleStateRecord>,
    #[prost(uint32, optional, tag = "5")]
    pub match_epoch: Option<u32>,
    #[prost(uint32, optional, tag = "6")]
    pub round_epoch: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct RelayHost {
    #[prost(bytes, tag = "1")]
    pub payload: Vec<u8>,
    #[prost(uint32, optional, tag = "2")]
    pub match_epoch: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub round_epoch: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct RelayPeer {
    #[prost(string, repeated, tag = "1")]
    pub player_ids: Vec<String>,
    #[prost(bytes, tag = "2")]
    pub payload: Vec<u8>,
    #[prost(uint32, optional, tag = "3")]
    pub match_epoch: Option<u32>,
    #[prost(uint32, optional, tag = "4")]
    pub round_epoch: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct RelayFromHost {
    #[prost(string, tag = "1")]
    pub room_id: String,
    #[prost(bytes, tag = "2")]
    pub payload: Vec<u8>,
    #[prost(uint32, optional, tag = "3")]
    pub match_epoch: Option<u32>,
    #[prost(uint32, optional, tag = "4")]
    pub round_epoch: Option<u32>,
}

#[derive(Clone, PartialEq, Message)]
pub struct RelayFromPeer {
    #[prost(string, tag = "1")]
    pub room_id: String,
    #[prost(string, tag = "2")]
    pub player_id: String,
    #[prost(bytes, tag = "3")]
    pub payload: Vec<u8>,
    #[prost(uint32, optional, tag = "4")]
    pub match_epoch: Option<u32>,
    #[prost(uint32, optional, tag = "5")]
    pub round_epoch: Option<u32>,
}

pub mod dynamic_value {
    use super::{DynamicList, DynamicObject};
    use prost::Oneof;

    #[derive(Clone, PartialEq, Oneof)]
    pub enum Kind {
        #[prost(double, tag = "1")]
        Number(f64),
        #[prost(string, tag = "2")]
        String(String),
        #[prost(bool, tag = "3")]
        Bool(bool),
        #[prost(bytes, tag = "4")]
        Bytes(Vec<u8>),
        #[prost(message, tag = "5")]
        Object(DynamicObject),
        #[prost(message, tag = "6")]
        List(DynamicList),
        #[prost(enumeration = "super::NullValue", tag = "7")]
        Null(i32),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum NullValue {
    Unspecified = 0,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
#[repr(i32)]
pub enum MessageKind {
    Unspecified = 0,
    Hello = 1,
    Welcome = 2,
    Error = 3,
    Ping = 4,
    Pong = 5,
    RoomCreate = 10,
    RoomJoin = 11,
    RoomListRequest = 12,
    RoomList = 13,
    RoomLeave = 14,
    RoomLeft = 15,
    RoomReady = 16,
    RoomProfile = 17,
    RoomSettings = 18,
    RoomStart = 19,
    RoomLatency = 20,
    RoomJoined = 21,
    RoomState = 22,
    RoomPlayerLeft = 23,
    ChatSend = 30,
    ChatMessage = 31,
    TeamSelect = 32,
    MatchPrepare = 40,
    MatchStart = 41,
    MatchEnd = 42,
    MatchReset = 43,
    RoundStart = 44,
    RoundEnd = 45,
    MatchAssetPrepare = 46,
    MatchAssetReady = 47,
    RtcSignal = 50,
    RtcReport = 51,
    HostState = 52,
    SceneOps = 60,
    ItemClaim = 61,
    ItemClaimed = 62,
    ItemRespawned = 63,
    ItemEffect = 64,
    WeaponSwitched = 65,
    WeaponDropped = 66,
    ObjectiveEnd = 67,
    WeaponDropRest = 68,
    CombatHit = 70,
    CombatDeath = 71,
    CombatRespawn = 72,
    CombatCure = 73,
    EscortTaken = 80,
    EscortDropped = 81,
    EscortDelivered = 82,
    EscortReturned = 83,
    RoleRate = 90,
    RolePoseChain = 91,
    RoleSelfEffect = 92,
    SceneTransfer = 93,
    SceneTransferAck = 94,
    ScenePropHit = 95,
    RtcPing = 100,
    RtcPong = 101,
}
