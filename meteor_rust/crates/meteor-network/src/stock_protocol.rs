use crate::role_state::{
    decode_event2_state, decode_request1_state, encode_event2_state, encode_request1_state,
    RoleState, RoleStateError, RoleVec3, EVENT2_STATE_BYTES, REQUEST1_STATE_BYTES,
    ROLE_WORLD_SCALE,
};
use meteor_asset::pos::AttackBlock;
use meteor_combat::hit_reaction::{
    hit_direction_bits, pick_reaction_pose_id, FacingBucket, LIGHT_HIT_FALLBACK_POSES,
};
use meteor_combat::hit_resolve::{
    force_hit_flags_from_attack_type, hit_key_from_pose, pack_hit_dword1,
};
use std::fs;
use std::io;
use std::path::Path;

pub const STOCK_REQUEST_ROLE_STATE: u16 = 1;
pub const STOCK_EVENT_PROPERTY_EFFECT: u16 = 61;

pub use crate::role_state::PropertyEffect as StockPropertyEffectEvent;

impl StockPropertyEffectEvent {
    pub fn encode(self) -> Vec<u8> {
        [self.actor_id, self.category, self.resource_id]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 12)?;
        let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        Ok(Self {
            actor_id: word(0),
            category: word(4),
            resource_id: word(8),
        })
    }
}
pub const STOCK_EVENT_ROLE_STATE: u16 = 2;

pub const STOCK_EVENT_LOCAL_RESPAWN: u16 = 67;
pub const STOCK_REQUEST_ENTER_ROLE: u16 = 4;
pub const STOCK_REQUEST_HIT: u16 = 5;
pub const STOCK_REQUEST_ANGER_FULL: u16 = 18;
pub const STOCK_REQUEST_QI_BURST: u16 = 36;
pub const STOCK_REQUEST_ANGER_COST: u16 = 41;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockAngerSpend {
    Full,
    Burst,
    Cost(u32),
}

impl StockAngerSpend {
    pub fn opcode(self) -> u16 {
        match self {
            Self::Full => STOCK_REQUEST_ANGER_FULL,
            Self::Burst => STOCK_REQUEST_QI_BURST,
            Self::Cost(_) => STOCK_REQUEST_ANGER_COST,
        }
    }
    pub fn encode(self) -> Vec<u8> {
        match self {
            Self::Cost(cost) => cost.to_le_bytes().to_vec(),
            _ => Vec::new(),
        }
    }
    pub fn decode(opcode: u16, bytes: &[u8]) -> Result<Self, StockProtocolError> {
        match opcode {
            STOCK_REQUEST_ANGER_FULL => {
                require_length(bytes, 0)?;
                Ok(Self::Full)
            }
            STOCK_REQUEST_QI_BURST => {
                require_length(bytes, 0)?;
                Ok(Self::Burst)
            }
            STOCK_REQUEST_ANGER_COST => {
                require_length(bytes, 4)?;
                Ok(Self::Cost(u32::from_le_bytes(bytes.try_into().unwrap())))
            }
            _ => Err(StockProtocolError::InvalidAngerOpcode(opcode)),
        }
    }
    pub fn cost(self) -> u32 {
        match self {
            Self::Full => 100,
            Self::Burst => 60,
            Self::Cost(cost) => cost,
        }
    }
    pub fn remaining(self, anger: f32) -> f32 {
        match self {
            Self::Full => 0.0,
            _ => (anger - self.cost() as f32).max(0.0),
        }
    }
}

pub const STOCK_EVENT_ROLE_LEFT: u16 = 6;
pub const STOCK_EVENT_ROLE_ENTERED: u16 = 9;
pub const STOCK_EVENT_ROSTER: u16 = 4;

pub const STOCK_EVENT_POSE_CHAIN: u16 = 51;

pub const STOCK_REQUEST_POSE_CHAIN: u16 = 26;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StockPoseChainRequest {
    pub pose_id: u16,
    pub sequence: u16,
    pub blend_seconds: f32,
}

impl StockPoseChainRequest {
    pub fn encode(self) -> [u8; 8] {
        let mut bytes = [0; 8];
        bytes[..2].copy_from_slice(&self.pose_id.to_le_bytes());

        bytes[4..6].copy_from_slice(&self.sequence.to_le_bytes());

        let centiseconds = (f64::from(self.blend_seconds) * 100.0) as i16;
        bytes[6..].copy_from_slice(&centiseconds.to_le_bytes());
        bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StockPoseChainEvent {
    pub actor_id: u16,
    pub pose_id: u16,
    pub minimum_seconds: f32,
    pub sequence: u16,
    pub blend_seconds: f32,
}

impl From<StockPoseChainEvent> for crate::role_state::PoseChain {
    fn from(event: StockPoseChainEvent) -> Self {
        Self {
            pose_id: event.pose_id,
            minimum_seconds: event.minimum_seconds,
            sequence: event.sequence,
            blend_seconds: event.blend_seconds,
        }
    }
}

impl StockPoseChainEvent {
    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 10)?;
        let word = |offset| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        Ok(Self {
            actor_id: word(0),
            pose_id: word(2),
            minimum_seconds: (word(4) as i16) as f32 * 0.01,
            sequence: word(6),
            blend_seconds: (word(8) as i16) as f32 * 0.01,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockRosterEntry {
    pub actor_id: u32,
    pub hero: u32,
    pub entered: bool,
    pub camp: u32,
    pub weapon_item_id: u32,
    pub name: String,
}

pub fn decode_stock_roster(bytes: &[u8]) -> Result<Vec<StockRosterEntry>, StockProtocolError> {
    let malformed = || StockProtocolError::InvalidRoster;
    let count = bytes
        .first()
        .copied()
        .and_then(|b| b.checked_sub(1))
        .ok_or_else(malformed)?;
    let mut offset = 1;
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let fixed = bytes.get(offset..offset + 14).ok_or_else(malformed)?;
        offset += 14;
        let mut strings = Vec::with_capacity(3);
        for _ in 0..3 {
            let length = bytes
                .get(offset)
                .copied()
                .and_then(|b| b.checked_sub(1))
                .ok_or_else(malformed)? as usize;
            offset += 1;
            let raw = bytes.get(offset..offset + length).ok_or_else(malformed)?;
            offset += length;
            strings.push(raw);
        }
        let field = |index: usize| {
            fixed[index]
                .checked_sub(1)
                .map(u32::from)
                .ok_or_else(malformed)
        };
        let (name, _, invalid) = encoding_rs::GBK.decode(strings[0]);
        if invalid {
            return Err(malformed());
        }
        entries.push(StockRosterEntry {
            actor_id: field(0)?,
            hero: field(1)?,
            entered: field(2)? == 1,

            camp: field(6)?,
            weapon_item_id: field(5)?,
            name: name.into_owned(),
        });
    }
    if offset != bytes.len() {
        return Err(malformed());
    }
    Ok(entries)
}

fn encode_member_byte(value: u32) -> Result<u8, StockProtocolError> {
    u8::try_from(value)
        .ok()
        .and_then(|v| v.checked_add(1))
        .ok_or(StockProtocolError::InvalidRoster)
}

fn append_member_name(bytes: &mut Vec<u8>, name: &str) -> Result<(), StockProtocolError> {
    let (encoded, _, invalid) = encoding_rs::GBK.encode(name);
    if invalid || encoded.contains(&0) {
        return Err(StockProtocolError::InvalidRoster);
    }
    bytes.push(encode_member_byte(encoded.len() as u32)?);
    bytes.extend_from_slice(&encoded);
    bytes.extend_from_slice(&[1, 1]);
    Ok(())
}

pub fn encode_stock_member_joined(
    actor_id: u32,
    name: &str,
) -> Result<Vec<u8>, StockProtocolError> {
    let mut bytes = vec![encode_member_byte(actor_id)?, 1, 1];
    append_member_name(&mut bytes, name)?;
    Ok(bytes)
}

pub fn encode_stock_roster(entries: &[StockRosterEntry]) -> Result<Vec<u8>, StockProtocolError> {
    let mut bytes = vec![encode_member_byte(entries.len() as u32)?];
    for entry in entries {
        let mut fixed = [1; 14];
        for (index, value) in [
            (0, entry.actor_id),
            (1, entry.hero),
            (2, u32::from(entry.entered)),
            (5, entry.weapon_item_id),
            (6, entry.camp),
        ] {
            fixed[index] = encode_member_byte(value)?;
        }
        bytes.extend_from_slice(&fixed);
        append_member_name(&mut bytes, &entry.name)?;
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy)]
pub struct StockSceneFrame(pub [f32; 3]);

impl StockSceneFrame {
    pub fn to_scene(self, mut state: RoleState) -> RoleState {
        state.position.x -= self.0[0] * ROLE_WORLD_SCALE;
        state.position.y -= self.0[2] * ROLE_WORLD_SCALE;
        state.position.z = self.0[1] * ROLE_WORLD_SCALE - state.position.z;
        state.forward.z = -state.forward.z;
        state.velocity.z = -state.velocity.z;
        state
    }

    pub fn to_stock(self, mut state: RoleState) -> RoleState {
        state.position.x += self.0[0] * ROLE_WORLD_SCALE;
        state.position.y += self.0[2] * ROLE_WORLD_SCALE;
        state.position.z = self.0[1] * ROLE_WORLD_SCALE - state.position.z;
        state.forward.z = -state.forward.z;
        state.velocity.z = -state.velocity.z;
        state
    }
}

pub const STOCK_EVENT_STAGE_CONFIG: u16 = 10;
pub const STOCK_REQUEST_EQUIP_WEAPON: u16 = 6;
pub const STOCK_REQUEST_WEAPON_OBJECT: u16 = 19;
pub const STOCK_REQUEST_SPAWN_SCENE_ITEM: u16 = 20;
pub const STOCK_EVENT_WEAPON_OBJECT: u16 = 30;
pub const STOCK_REQUEST_DROP_ITEM: u16 = 28;
pub const STOCK_REQUEST_CURE: u16 = 31;
pub const STOCK_REQUEST_ROLE_RATE: u16 = 33;
pub const STOCK_REQUEST_READY: u16 = 43;
pub const STOCK_EVENT_BE_HIT: u16 = 11;
pub const STOCK_EVENT_WEAPON_EQUIPPED: u16 = 12;
pub const STOCK_EVENT_GUARD_HIT: u16 = 14;
pub const STOCK_EVENT_KILL: u16 = 23;
pub const STOCK_EVENT_DEATH: u16 = 24;

pub const STOCK_EVENT_ENVIRONMENT_DEATH: u16 = 78;
pub const STOCK_EVENT_VITALS: u16 = 25;
pub const STOCK_EVENT_ROUND_RESTART: u16 = 27;
pub const STOCK_EVENT_SCENE_SNAPSHOT: u16 = 29;
pub const STOCK_EVENT_SCENE_SPAWN: u16 = 31;
pub const STOCK_EVENT_SCENE_TAKEN: u16 = 32;
pub const STOCK_EVENT_TEAM_SCORE: u16 = 34;
pub const STOCK_EVENT_VIP: u16 = 35;
pub const STOCK_EVENT_MATCH_END: u16 = 40;
pub const STOCK_EVENT_RESPAWN: u16 = 46;
pub const STOCK_EVENT_FLAG_RETURNED: u16 = 53;
pub const STOCK_EVENT_FLAG_REMOVED: u16 = 54;
pub const STOCK_EVENT_DROP_ITEM: u16 = 55;
pub const STOCK_EVENT_ROLE_RATE: u16 = 64;

pub const STOCK_EVENT_SCENE_OPS: u16 = 56;

pub const STOCK_REQUEST_ROLE_STATE_FLAGS: u8 = 0;
pub const STOCK_EVENT_ROLE_STATE_FLAGS: u8 = 0;
pub const STOCK_REQUEST_HIT_FLAGS: u8 = 0;
pub const STOCK_EVENT_HIT_FLAGS: u8 = 0;
pub const STOCK_DISCRETE_RELIABLE_FLAGS: u8 = 1;
pub const STOCK_EVENT_DROP_ITEM_FLAGS: u8 = 0;
pub const STOCK_EVENT_WEAPON_OBJECT_FLAGS: u8 = 0;

pub const STOCK_SCENE_OP_POSE: u32 = 1;
pub const STOCK_SCENE_OP_FRAME: u32 = 2;
pub const STOCK_SCENE_OP_COLLISION: u32 = 3;
pub const STOCK_SCENE_OP_VISIBLE: u32 = 4;
pub const STOCK_SCENE_OP_DAMAGE: u32 = 5;
pub const STOCK_SCENE_OP_ACTIVE: u32 = 6;
pub const STOCK_SCENE_OP_INTERACTIVE: u32 = 7;
pub const STOCK_SCENE_OP_EFFECT: u32 = 8;

pub const STOCK_SCENE_OPS_PER_PACKET: usize = 63;

pub const STOCK_EFFECT_FIRE_BREAK: u32 = 695;
pub const STOCK_EFFECT_FIRE_HIT: u32 = 696;
pub const STOCK_EFFECT_GATE_BREAK: u32 = 701;
pub const STOCK_EFFECT_GATE_HIT: u32 = 702;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockSceneOperation {
    pub reference: u32,
    pub kind: u32,
    pub value: u32,
    pub extra: u32,
}

impl StockSceneOperation {
    pub const BYTES: usize = 16;

    pub fn encode(self) -> [u8; Self::BYTES] {
        encode_four_u32([self.reference, self.kind, self.value, self.extra])
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        let words = decode_four_u32(payload)?;
        Ok(Self {
            reference: words[0],
            kind: words[1],
            value: words[2],
            extra: words[3],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockSceneOpsEvent {
    pub timestamp_ms: u32,
    pub operations: Vec<StockSceneOperation>,
}

impl StockSceneOpsEvent {
    pub const HEADER_BYTES: usize = 4;

    pub fn encode(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(
            Self::HEADER_BYTES + self.operations.len() * StockSceneOperation::BYTES,
        );
        payload.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        for operation in &self.operations {
            payload.extend_from_slice(&operation.encode());
        }
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        if payload.len() < Self::HEADER_BYTES
            || !(payload.len() - Self::HEADER_BYTES).is_multiple_of(StockSceneOperation::BYTES)
        {
            return Err(StockProtocolError::InvalidLength {
                expected: Self::HEADER_BYTES,
                actual: payload.len(),
            });
        }
        let operations = payload[Self::HEADER_BYTES..]
            .chunks_exact(StockSceneOperation::BYTES)
            .map(StockSceneOperation::decode)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            timestamp_ms: read_u32(payload, 0),
            operations,
        })
    }
}

pub fn stock_rule3_effect_id(name: &str) -> Option<u32> {
    let normalized = name.to_ascii_lowercase();
    match normalized.strip_suffix(".ef").unwrap_or(&normalized) {
        "boxbrk" => Some(691),
        "boxhit" => Some(692),
        "firebrk" => Some(STOCK_EFFECT_FIRE_BREAK),
        "firehit" => Some(STOCK_EFFECT_FIRE_HIT),
        "gimabrk" => Some(STOCK_EFFECT_GATE_BREAK),
        "gimahit" => Some(STOCK_EFFECT_GATE_HIT),
        _ => None,
    }
}

pub fn stock_rule3_effect_name(effect_id: u32) -> Option<&'static str> {
    match effect_id {
        691 => Some("BoxBRK"),
        692 => Some("BoxHIT"),
        STOCK_EFFECT_FIRE_BREAK => Some("FireBRK"),
        STOCK_EFFECT_FIRE_HIT => Some("FireHIT"),
        STOCK_EFFECT_GATE_BREAK => Some("GiMaBRK"),
        STOCK_EFFECT_GATE_HIT => Some("GiMaHIT"),
        _ => None,
    }
}

pub const STOCK_REVISION_HERO_CHECKSUMS: [u32; 20] = [
    0x6c73_e7ed,
    0x7992_907d,
    0x2711_8aa9,
    0x2b16_5893,
    0xcd7c_303c,
    0x270f_3742,
    0x6b6b_1d1b,
    0x270e_1f0b,
    0x2b18_64c2,
    0x2b17_7021,
    0x3140_7f26,
    0x2b16_4764,
    0x2b17_036c,
    0x7991_f553,
    0x2b16_57ff,
    0x2710_1924,
    0x270e_20c9,
    0x2b9e_b969,
    0x2711_a0a0,
    0x2b17_485b,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockEnterRoleRequest {
    pub hero: u32,
    pub camp: u32,
    pub hero_checksum: u32,
}

impl StockEnterRoleRequest {
    pub fn for_bundled_revision(hero: u32, camp: u32) -> Result<Self, StockProtocolError> {
        let checksums = STOCK_REVISION_HERO_CHECKSUMS;
        let hero_checksum = checksums
            .get(hero as usize)
            .copied()
            .ok_or(StockProtocolError::InvalidHero(hero))?;
        Ok(Self {
            hero,
            camp,
            hero_checksum,
        })
    }

    pub fn for_packed_revision(hero: u32, camp: u32) -> Result<Self, StockProtocolError> {
        let checksums = meteor_asset::runtime_assets::stock_checksums();
        let checksums =
            checksums.map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let hero_checksum = checksums
            .get(hero as usize)
            .copied()
            .ok_or(StockProtocolError::InvalidHero(hero))?;
        Ok(Self {
            hero,
            camp,
            hero_checksum,
        })
    }

    pub fn from_origin_res(
        origin_res: impl AsRef<Path>,
        hero: u32,
        camp: u32,
    ) -> Result<Self, StockProtocolError> {
        if hero > 19 {
            return Err(StockProtocolError::InvalidHero(hero));
        }
        let path = origin_res
            .as_ref()
            .join("pmodel")
            .join(format!("P{hero}.POS"));
        let bytes = fs::read(path)?;
        Ok(Self {
            hero,
            camp,
            hero_checksum: stock_resource_checksum(&bytes),
        })
    }

    pub fn encode(self) -> [u8; 12] {
        let mut bytes = [0u8; 12];
        bytes[0..4].copy_from_slice(&self.hero.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.camp.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.hero_checksum.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 12)?;
        Ok(Self {
            hero: read_u32(bytes, 0),
            camp: read_u32(bytes, 4),
            hero_checksum: read_u32(bytes, 8),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockRoleEnteredEvent {
    pub actor_id: u32,
    pub hero: u32,

    pub hero_checksum: u32,
    pub camp: u32,
}

impl StockRoleEnteredEvent {
    pub fn encode(self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&self.actor_id.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.hero.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.hero_checksum.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.camp.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 16)?;
        Ok(Self {
            actor_id: read_u32(bytes, 0),
            hero: read_u32(bytes, 4),
            hero_checksum: read_u32(bytes, 8),
            camp: read_u32(bytes, 12),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockStageConfigEvent {
    pub stage_id: String,
    pub field_a: u32,
    pub field_b: u32,
    pub field_c: u32,
}

impl StockStageConfigEvent {
    pub fn encode(&self) -> Result<[u8; 16], StockProtocolError> {
        let stage = self.stage_id.as_bytes();
        if stage.len() != 4 || !stage.is_ascii() {
            return Err(StockProtocolError::InvalidStageId(self.stage_id.clone()));
        }
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(stage);
        bytes[4..8].copy_from_slice(&self.field_a.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.field_b.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.field_c.to_le_bytes());
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 16)?;
        let stage_id = bytes[0..4]
            .iter()
            .map(|byte| *byte as char)
            .collect::<String>();
        if !stage_id.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(StockProtocolError::InvalidStageId(stage_id));
        }
        Ok(Self {
            stage_id,
            field_a: read_u32(bytes, 4),
            field_b: read_u32(bytes, 8),
            field_c: read_u32(bytes, 12),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockRoleLeftEvent {
    pub actor_id: u32,
}

impl StockRoleLeftEvent {
    pub fn encode(self) -> [u8; 4] {
        self.actor_id.to_le_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 4)?;
        Ok(Self {
            actor_id: read_u32(bytes, 0),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StockRoleRate {
    pub hz: f32,
}

impl StockRoleRate {
    pub fn encode(self) -> [u8; 4] {
        self.hz.to_le_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 4)?;
        Ok(Self {
            hz: f32::from_le_bytes(bytes.try_into().expect("four-byte payload")),
        })
    }

    pub fn requested_hz(self) -> Option<u32> {
        exact_integer_in_range(self.hz, 7, 21)
    }

    pub fn session_hz(self) -> Option<u32> {
        exact_integer_in_range(self.hz, 8, 20)
    }
}

fn exact_integer_in_range(value: f32, minimum: u32, maximum: u32) -> Option<u32> {
    if !value.is_finite() {
        return None;
    }
    let rounded = value.round();
    if (value - rounded).abs() > 1e-6 {
        return None;
    }
    let value = rounded as i64;
    (i64::from(minimum)..=i64::from(maximum))
        .contains(&value)
        .then_some(value as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockEquipWeaponRequest {
    pub weapon_item_id: u32,
}

impl StockEquipWeaponRequest {
    pub fn encode(self) -> [u8; 4] {
        self.weapon_item_id.to_le_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 4)?;
        Ok(Self {
            weapon_item_id: read_u32(bytes, 0),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockHitRequest {
    pub target_actor_id: u32,
    pub hit_word: u32,
    pub target_pose_word: u32,
    pub moves_word: u32,
    pub values_word: u32,
}

impl StockHitRequest {
    pub const BYTES: usize = 20;

    pub fn from_attack(
        target_actor_id: u32,
        attacker_pose_id: u16,
        attacker_weapon: u8,
        attack: &AttackBlock,
        bucket: FacingBucket,
        defender_poses: &[u16],
    ) -> Self {
        let reaction_pose = pick_reaction_pose_id(attack, bucket, defender_poses)
            .or(attack.target_pose)
            .or_else(|| LIGHT_HIT_FALLBACK_POSES.first().copied())
            .unwrap_or(90);
        Self {
            target_actor_id,
            hit_word: pack_hit_dword1(
                hit_key_from_pose(attacker_pose_id),
                force_hit_flags_from_attack_type(attack.attack_type as i32),
                u32::from(meteor_combat::weapon_strategy::weapon_to_strategy(
                    attacker_weapon,
                )),
            ),
            target_pose_word: u32::from(reaction_pose) | hit_direction_bits(bucket),
            moves_word: pack_hundred_pair(attack.target_move, attack.defense_move),
            values_word: pack_hundred_pair(attack.target_value, attack.defense_value),
        }
    }

    pub fn encode(self) -> [u8; Self::BYTES] {
        encode_five_u32([
            self.target_actor_id,
            self.hit_word,
            self.target_pose_word,
            self.moves_word,
            self.values_word,
        ])
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        let words = decode_five_u32(payload)?;
        Ok(Self {
            target_actor_id: words[0],
            hit_word: words[1],
            target_pose_word: words[2],
            moves_word: words[3],
            values_word: words[4],
        })
    }

    pub fn force_hit_flags(self) -> u32 {
        (self.hit_word >> 10) & 0x3f
    }

    pub fn direction_selector(self) -> u32 {
        (self.target_pose_word >> 12) & 0x0f
    }

    pub fn target_pose(self) -> u16 {
        (self.target_pose_word & 0x0fff) as u16
    }

    pub fn target_move(self) -> f32 {
        unpack_hundred(self.moves_word as u16)
    }

    pub fn defense_move(self) -> f32 {
        unpack_hundred((self.moves_word >> 16) as u16)
    }

    pub fn target_value(self) -> f32 {
        unpack_hundred(self.values_word as u16)
    }

    pub fn defense_value(self) -> f32 {
        unpack_hundred((self.values_word >> 16) as u16)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockHitReactionEvent {
    pub pose_id: u32,
    pub move_hundred: u32,
    pub value_hundred: u32,

    pub context: u32,
    pub target_actor_id: u32,
}

impl StockHitReactionEvent {
    pub const BYTES: usize = 20;

    pub fn encode(self) -> [u8; Self::BYTES] {
        encode_five_u32([
            self.pose_id,
            self.move_hundred,
            self.value_hundred,
            self.context,
            self.target_actor_id,
        ])
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        let words = decode_five_u32(payload)?;
        Ok(Self {
            pose_id: words[0],
            move_hundred: words[1],
            value_hundred: words[2],
            context: words[3],
            target_actor_id: words[4],
        })
    }

    pub fn motion_scale(self) -> f32 {
        unpack_hundred(self.move_hundred as u16)
    }

    pub fn minimum_seconds(self) -> f32 {
        unpack_hundred(self.value_hundred as u16)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockCureRequest {
    pub target_actor_ids: Vec<u8>,
}

impl StockCureRequest {
    pub fn encode(&self) -> Vec<u8> {
        self.target_actor_ids.clone()
    }

    pub fn decode(payload: &[u8]) -> Self {
        Self {
            target_actor_ids: payload.to_vec(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockSceneItemRecord {
    pub record_id: u32,
    pub item_id: u32,
    pub category: u32,
    pub position_tenths: [i32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockSceneItemSpawnRequest {
    pub item_id: u32,
    pub category: u32,
    pub position_tenths: [i32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockWeaponObjectRequest {
    pub record_key: u32,
    pub selector: u32,
    pub position_tenths: [i32; 3],
    pub forward_tenths: [i32; 3],
}

impl StockWeaponObjectRequest {
    pub const BYTES: usize = 32;

    pub fn from_role_vectors(
        record_key: u32,
        selector: u32,
        position: RoleVec3,
        forward: RoleVec3,
    ) -> Self {
        let quantize = |value: RoleVec3| {
            [
                quantize_scene_coordinate(value.x),
                quantize_scene_coordinate(value.z),
                quantize_scene_coordinate(value.y),
            ]
        };
        Self {
            record_key,
            selector,
            position_tenths: quantize(position),
            forward_tenths: quantize(forward),
        }
    }

    pub fn encode(self) -> [u8; Self::BYTES] {
        let mut payload = [0u8; Self::BYTES];
        payload[0..4].copy_from_slice(&self.record_key.to_le_bytes());
        payload[4..8].copy_from_slice(&self.selector.to_le_bytes());
        for (index, value) in self
            .position_tenths
            .into_iter()
            .chain(self.forward_tenths)
            .enumerate()
        {
            let offset = 8 + index * 4;
            payload[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, Self::BYTES)?;
        Ok(Self {
            record_key: read_u32(payload, 0),
            selector: read_u32(payload, 4),
            position_tenths: [
                read_i32(payload, 8),
                read_i32(payload, 12),
                read_i32(payload, 16),
            ],
            forward_tenths: [
                read_i32(payload, 20),
                read_i32(payload, 24),
                read_i32(payload, 28),
            ],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockDropItemEvent {
    pub actor_id: u32,
}

impl StockDropItemEvent {
    pub const BYTES: usize = 4;

    pub fn encode(self) -> [u8; Self::BYTES] {
        self.actor_id.to_le_bytes()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, Self::BYTES)?;
        Ok(Self {
            actor_id: read_u32(payload, 0),
        })
    }
}

impl StockSceneItemSpawnRequest {
    pub const BYTES: usize = 20;

    pub fn from_role_position(item_id: u32, category: u32, position: RoleVec3) -> Self {
        Self {
            item_id,
            category,
            position_tenths: [
                quantize_scene_coordinate(position.x),
                quantize_scene_coordinate(position.z),
                quantize_scene_coordinate(position.y),
            ],
        }
    }

    pub fn role_position(self) -> RoleVec3 {
        RoleVec3 {
            x: decode_scene_coordinate(self.position_tenths[0]),
            y: decode_scene_coordinate(self.position_tenths[2]),
            z: decode_scene_coordinate(self.position_tenths[1]),
        }
    }

    pub fn encode(self) -> [u8; Self::BYTES] {
        let mut payload = [0u8; Self::BYTES];
        payload[0..4].copy_from_slice(&self.item_id.to_le_bytes());
        payload[4..8].copy_from_slice(&self.category.to_le_bytes());
        for (index, coordinate) in self.position_tenths.into_iter().enumerate() {
            let offset = 8 + index * 4;
            payload[offset..offset + 4].copy_from_slice(&coordinate.to_le_bytes());
        }
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, Self::BYTES)?;
        Ok(Self {
            item_id: read_u32(payload, 0),
            category: read_u32(payload, 4),
            position_tenths: [
                read_i32(payload, 8),
                read_i32(payload, 12),
                read_i32(payload, 16),
            ],
        })
    }

    pub fn into_record(self, record_id: u32) -> StockSceneItemRecord {
        StockSceneItemRecord {
            record_id,
            item_id: self.item_id,
            category: self.category,
            position_tenths: self.position_tenths,
        }
    }
}

impl StockSceneItemRecord {
    pub const BYTES: usize = 24;

    pub fn from_role_position(
        record_id: u32,
        item_id: u32,
        category: u32,
        position: RoleVec3,
    ) -> Self {
        Self {
            record_id,
            item_id,
            category,

            position_tenths: [
                quantize_scene_coordinate(position.x),
                quantize_scene_coordinate(position.z),
                quantize_scene_coordinate(position.y),
            ],
        }
    }

    pub fn role_position(self) -> RoleVec3 {
        RoleVec3 {
            x: decode_scene_coordinate(self.position_tenths[0]),
            y: decode_scene_coordinate(self.position_tenths[2]),
            z: decode_scene_coordinate(self.position_tenths[1]),
        }
    }

    pub fn encode(self) -> [u8; Self::BYTES] {
        let mut payload = [0u8; Self::BYTES];
        payload[0..4].copy_from_slice(&self.record_id.to_le_bytes());
        payload[4..8].copy_from_slice(&self.item_id.to_le_bytes());
        payload[8..12].copy_from_slice(&self.category.to_le_bytes());
        for (index, coordinate) in self.position_tenths.into_iter().enumerate() {
            let offset = 12 + index * 4;
            payload[offset..offset + 4].copy_from_slice(&coordinate.to_le_bytes());
        }
        payload
    }

    pub fn decode_many(payload: &[u8]) -> Result<Vec<Self>, StockProtocolError> {
        if !payload.len().is_multiple_of(Self::BYTES) {
            return Err(StockProtocolError::InvalidLength {
                expected: payload.len().div_ceil(Self::BYTES) * Self::BYTES,
                actual: payload.len(),
            });
        }
        payload
            .chunks_exact(Self::BYTES)
            .map(Self::decode)
            .collect()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, Self::BYTES)?;
        Ok(Self {
            record_id: read_u32(payload, 0),
            item_id: read_u32(payload, 4),
            category: read_u32(payload, 8),
            position_tenths: [
                read_i32(payload, 12),
                read_i32(payload, 16),
                read_i32(payload, 20),
            ],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockSceneTakenEvent {
    pub actor_id: u32,
    pub record_id: u32,
    pub category: u32,
    pub item_id: u32,
}

impl StockSceneTakenEvent {
    pub const BYTES: usize = 16;

    pub fn encode(self) -> [u8; Self::BYTES] {
        encode_four_u32([self.actor_id, self.record_id, self.category, self.item_id])
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        let words = decode_four_u32(payload)?;
        Ok(Self {
            actor_id: words[0],
            record_id: words[1],
            category: words[2],
            item_id: words[3],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockFlagReturnedEvent {
    pub record_id: u32,
    pub category: u32,
    pub item_id: u32,
    pub scene_object_id: i32,
}

impl StockFlagReturnedEvent {
    pub const BYTES: usize = 16;

    pub fn encode(self) -> [u8; Self::BYTES] {
        encode_four_u32([
            self.record_id,
            self.category,
            self.item_id,
            self.scene_object_id as u32,
        ])
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        let words = decode_four_u32(payload)?;
        Ok(Self {
            record_id: words[0],
            category: words[1],
            item_id: words[2],
            scene_object_id: words[3] as i32,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockFlagRemovedEvent {
    pub scene_object_id: i32,
}

impl StockFlagRemovedEvent {
    pub fn encode(self) -> [u8; 4] {
        self.scene_object_id.to_le_bytes()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, 4)?;
        Ok(Self {
            scene_object_id: read_i32(payload, 0),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockVitalsEvent {
    pub health: u16,
    pub max_health: u16,
    pub anger: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockKillEvent {
    pub attacker_actor_id: u32,
    pub target_actor_id: u32,
}

impl StockKillEvent {
    pub const BYTES: usize = 8;

    pub fn encode(self) -> [u8; Self::BYTES] {
        let mut payload = [0u8; Self::BYTES];
        payload[..4].copy_from_slice(&self.attacker_actor_id.to_le_bytes());
        payload[4..].copy_from_slice(&self.target_actor_id.to_le_bytes());
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, Self::BYTES)?;
        Ok(Self {
            attacker_actor_id: read_u32(payload, 0),
            target_actor_id: read_u32(payload, 4),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockTeamScoreEvent {
    pub team1_wins: u32,
    pub team2_wins: u32,
}

impl StockTeamScoreEvent {
    pub const BYTES: usize = 8;

    pub fn encode(self) -> [u8; Self::BYTES] {
        let mut payload = [0u8; Self::BYTES];
        payload[..4].copy_from_slice(&self.team1_wins.to_le_bytes());
        payload[4..].copy_from_slice(&self.team2_wins.to_le_bytes());
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, Self::BYTES)?;
        Ok(Self {
            team1_wins: read_u32(payload, 0),
            team2_wins: read_u32(payload, 4),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockVipEvent {
    pub camp: u32,
    pub actor_id: u32,
}

impl StockVipEvent {
    pub const BYTES: usize = 8;

    pub fn encode(self) -> [u8; Self::BYTES] {
        let mut payload = [0u8; Self::BYTES];
        payload[..4].copy_from_slice(&self.camp.to_le_bytes());
        payload[4..].copy_from_slice(&self.actor_id.to_le_bytes());
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, Self::BYTES)?;
        Ok(Self {
            camp: read_u32(payload, 0),
            actor_id: read_u32(payload, 4),
        })
    }
}

impl StockVitalsEvent {
    pub const BYTES: usize = 6;

    pub fn encode(self) -> [u8; Self::BYTES] {
        let mut payload = [0u8; Self::BYTES];
        payload[0..2].copy_from_slice(&self.health.to_le_bytes());
        payload[2..4].copy_from_slice(&self.max_health.to_le_bytes());
        payload[4..6].copy_from_slice(&self.anger.to_le_bytes());
        payload
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, Self::BYTES)?;
        Ok(Self {
            health: u16::from_le_bytes([payload[0], payload[1]]),
            max_health: u16::from_le_bytes([payload[2], payload[3]]),
            anger: u16::from_le_bytes([payload[4], payload[5]]),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockRespawnEvent {
    pub actor_id: u32,
}

impl StockRespawnEvent {
    pub fn encode(self) -> [u8; 4] {
        self.actor_id.to_le_bytes()
    }

    pub fn decode(payload: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(payload, 4)?;
        Ok(Self {
            actor_id: read_u32(payload, 0),
        })
    }
}

fn pack_hundred_pair(low: f32, high: f32) -> u32 {
    u32::from(pack_hundred(low)) | (u32::from(pack_hundred(high)) << 16)
}

fn pack_hundred(value: f32) -> u16 {
    if !value.is_finite() {
        return 0;
    }
    ((value * 100.0).round() as i32).clamp(0, i32::from(u16::MAX)) as u16
}

fn unpack_hundred(value: u16) -> f32 {
    f32::from(value) * 0.01
}

fn encode_five_u32(words: [u32; 5]) -> [u8; 20] {
    let mut payload = [0u8; 20];
    for (index, word) in words.into_iter().enumerate() {
        payload[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    payload
}

fn decode_five_u32(payload: &[u8]) -> Result<[u32; 5], StockProtocolError> {
    require_length(payload, 20)?;
    let mut words = [0u32; 5];
    for (index, word) in words.iter_mut().enumerate() {
        *word = read_u32(payload, index * 4);
    }
    Ok(words)
}

fn encode_four_u32(words: [u32; 4]) -> [u8; 16] {
    let mut payload = [0u8; 16];
    for (index, word) in words.into_iter().enumerate() {
        payload[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    payload
}

fn decode_four_u32(payload: &[u8]) -> Result<[u32; 4], StockProtocolError> {
    require_length(payload, 16)?;
    Ok([
        read_u32(payload, 0),
        read_u32(payload, 4),
        read_u32(payload, 8),
        read_u32(payload, 12),
    ])
}

fn quantize_scene_coordinate(value: f32) -> i32 {
    let meteor = if value.is_finite() {
        value / ROLE_WORLD_SCALE
    } else {
        0.0
    };
    (meteor * 10.0 + 0.5).trunc() as i32
}

fn decode_scene_coordinate(value: i32) -> f32 {
    value as f32 * 0.1 * ROLE_WORLD_SCALE
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockWeaponEquippedEvent {
    pub weapon_item_id: u32,
    pub actor_id: u32,
}

impl StockWeaponEquippedEvent {
    pub fn encode(self) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&self.weapon_item_id.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.actor_id.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 8)?;
        Ok(Self {
            weapon_item_id: read_u32(bytes, 0),
            actor_id: read_u32(bytes, 4),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StockRoleStateRecord {
    pub actor_id: u16,
    pub state: RoleState,
}

pub fn encode_role_state_request(state: RoleState) -> [u8; REQUEST1_STATE_BYTES] {
    encode_request1_state(state)
}

pub fn decode_role_state_request(bytes: &[u8]) -> Result<RoleState, StockProtocolError> {
    Ok(decode_request1_state(bytes)?)
}

pub fn encode_role_state_event(records: &[StockRoleStateRecord]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(records.len() * (2 + EVENT2_STATE_BYTES));
    for record in records {
        bytes.extend_from_slice(&record.actor_id.to_le_bytes());
        bytes.extend_from_slice(&encode_event2_state(record.state));
    }
    bytes
}

pub fn decode_role_state_event(
    bytes: &[u8],
) -> Result<Vec<StockRoleStateRecord>, StockProtocolError> {
    const RECORD_BYTES: usize = 2 + EVENT2_STATE_BYTES;
    if !bytes.len().is_multiple_of(RECORD_BYTES) {
        return Err(StockProtocolError::InvalidLength {
            expected: bytes.len().div_ceil(RECORD_BYTES) * RECORD_BYTES,
            actual: bytes.len(),
        });
    }
    bytes
        .chunks_exact(RECORD_BYTES)
        .map(|record| {
            Ok(StockRoleStateRecord {
                actor_id: u16::from_le_bytes([record[0], record[1]]),
                state: decode_event2_state(&record[2..])?,
            })
        })
        .collect()
}

pub fn stock_resource_checksum(bytes: &[u8]) -> u32 {
    bytes.iter().enumerate().fold(0i32, |sum, (index, byte)| {
        let signed = i32::from(*byte as i8);
        sum.wrapping_add(signed.wrapping_mul((index as i32).wrapping_add(1)))
    }) as u32
}

pub fn stock_base_weapon_item_id(weapon: u8) -> Option<u32> {
    match weapon {
        1..=9 => Some(u32::from(weapon)),
        10 => Some(47),
        11 => Some(51),
        12 => Some(55),
        _ => None,
    }
}

pub fn stock_weapon_from_item_id(item_id: u32) -> Option<(u8, u8)> {
    match item_id {
        1..=45 => Some((((item_id - 1) % 9 + 1) as u8, ((item_id - 1) / 9) as u8)),
        46 => Some((1, 5)),
        47..=50 => Some((10, (item_id - 47) as u8)),
        51..=54 => Some((11, (item_id - 51) as u8)),
        55..=58 => Some((12, (item_id - 55) as u8)),
        59 => Some((2, 5)),
        _ => None,
    }
}

pub fn stock_weapon_item_id(weapon: u8, variant: u8) -> Option<u32> {
    Some(match (weapon, variant) {
        (1..=9, 0..=4) => u32::from(weapon) + u32::from(variant) * 9,
        (1, 5) => 46,
        (10, 0..=3) => 47 + u32::from(variant),
        (11, 0..=3) => 51 + u32::from(variant),
        (12, 0..=3) => 55 + u32::from(variant),
        (2, 5) => 59,
        _ => return None,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum StockProtocolError {
    #[error("stock member roster is malformed")]
    InvalidRoster,
    #[error("not a stock anger request: {0}")]
    InvalidAngerOpcode(u16),
    #[error("stock protocol payload requires {expected} bytes, received {actual}")]
    InvalidLength { expected: usize, actual: usize },
    #[error("stock protocol hero id is outside 0..19: {0}")]
    InvalidHero(u32),
    #[error("stock stage config id is not 4 printable ASCII bytes: {0:?}")]
    InvalidStageId(String),
    #[error("unable to read stock resource: {0}")]
    Io(#[from] io::Error),
    #[error("invalid stock role state: {0}")]
    RoleState(#[from] RoleStateError),
}

fn require_length(bytes: &[u8], expected: usize) -> Result<(), StockProtocolError> {
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(StockProtocolError::InvalidLength {
            expected,
            actual: bytes.len(),
        })
    }
}

#[cfg(test)]
mod pose_chain_tests {
    use super::*;

    #[test]
    fn captured_event51_decodes_explicit_entry_and_rejects_truncation() {
        let bytes = [1, 0, 0xfd, 0, 0, 0, 0xc7, 2, 0x1e, 0];
        let event = StockPoseChainEvent::decode(&bytes).unwrap();
        assert_eq!(
            (event.actor_id, event.pose_id, event.sequence),
            (1, 253, 711)
        );
        assert!((event.blend_seconds - 0.3).abs() < 1e-6);
        assert_eq!(event.minimum_seconds, 0.0);
        for length in 0..10 {
            assert!(StockPoseChainEvent::decode(&bytes[..length]).is_err());
        }
        assert!(StockPoseChainEvent::decode(&[0; 11]).is_err());
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("checked payload"),
    )
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("checked payload"),
    )
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_initialization_encodes_original_wire_records() {
        assert_eq!(
            encode_stock_member_joined(2, "").unwrap(),
            [3, 1, 1, 1, 1, 1]
        );
        assert_eq!(
            encode_stock_member_joined(2, "孟").unwrap(),
            [3, 1, 1, 3, 0xc3, 0xcf, 1, 1]
        );
        let member = StockRosterEntry {
            actor_id: 1,
            hero: 4,
            entered: true,
            camp: 2,
            weapon_item_id: 5,
            name: "unname".into(),
        };
        let bytes = encode_stock_roster(&[member.clone()]).unwrap();
        assert_eq!(
            bytes,
            [
                2, 2, 5, 2, 1, 1, 6, 3, 1, 1, 1, 1, 1, 1, 1, 7, b'u', b'n', b'n', b'a', b'm', b'e',
                1, 1
            ]
        );
        assert_eq!(decode_stock_roster(&bytes).unwrap(), vec![member]);
        assert!(encode_stock_member_joined(255, "").is_err());
        assert!(encode_stock_member_joined(1, &"a".repeat(255)).is_err());
        assert!(encode_stock_member_joined(1, "a\0b").is_err());
    }

    #[test]
    fn stock_roster_decodes_live_monk_and_variable_length_members() {
        let bytes = [
            2, 2, 5, 2, 1, 1, 5, 1, 1, 1, 1, 1, 1, 1, 1, 7, b'u', b'n', b'n', b'a', b'm', b'e', 1,
            1,
        ];
        let members = decode_stock_roster(&bytes).unwrap();
        assert_eq!(
            members,
            vec![StockRosterEntry {
                actor_id: 1,
                hero: 4,
                entered: true,
                camp: 0,
                weapon_item_id: 4,
                name: "unname".into(),
            }]
        );
        for end in 0..bytes.len() {
            assert!(
                decode_stock_roster(&bytes[..end]).is_err(),
                "truncated at {end}"
            );
        }
        let mut two = vec![3];
        two.extend_from_slice(&bytes[1..]);
        let mut second = bytes[1..15].to_vec();
        second[0] = 8;
        second[1] = 20;
        second[2] = 1;
        second[5] = 1;
        two.extend(second);
        let (name, _, _) = encoding_rs::GBK.encode("和尚");
        two.push(name.len() as u8 + 1);
        two.extend_from_slice(&name);
        two.extend_from_slice(&[3, b'a', b'b', 2, b'c']);
        let members = decode_stock_roster(&two).unwrap();
        assert_eq!(members[1].actor_id, 7);
        assert_eq!(members[1].hero, 19);
        assert_eq!(members[1].name, "和尚");
        assert!(!members[1].entered);
        assert_eq!(members[1].weapon_item_id, 0);
        two.push(0);
        assert!(decode_stock_roster(&two).is_err());
    }

    #[test]
    fn stock_roster_weapon_byte_is_plus_one_unlike_event12() {
        let mut bytes = [2, 2, 5, 2, 1, 1, 6, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1];
        for item in 0..=59 {
            bytes[6] = item + 1;
            let member = &decode_stock_roster(&bytes).unwrap()[0];
            assert_eq!(member.weapon_item_id, u32::from(item));
            let event = StockWeaponEquippedEvent {
                actor_id: 1,
                weapon_item_id: u32::from(item),
            };
            assert_eq!(
                StockWeaponEquippedEvent::decode(&event.encode())
                    .unwrap()
                    .weapon_item_id,
                member.weapon_item_id
            );
        }
        bytes[6] = 0;
        assert!(decode_stock_roster(&bytes).is_err());
    }

    #[test]
    fn stock_scene_frame_matches_source_coordinates_before_wire_quantization() {
        let frame = StockSceneFrame([100.0, -200.0, 30.0]);
        let mut source = sample_state();

        source.position = RoleVec3 {
            x: 18.0,
            y: 4.8,
            z: -30.0,
        };
        source.forward = RoleVec3 {
            x: 0.0,
            y: 0.0,
            z: 1.0,
        };
        source.velocity = RoleVec3 {
            x: 1.2,
            y: 3.6,
            z: 2.4,
        };
        let scene = frame.to_scene(source);
        assert!((scene.position.x - 6.0).abs() < 0.0001);
        assert!((scene.position.y - 1.2).abs() < 0.0001);
        assert!((scene.position.z - 6.0).abs() < 0.0001);
        assert_eq!(scene.forward.z, -1.0);
        assert_eq!(scene.velocity.z, -2.4);
        let wire = encode_role_state_request(frame.to_stock(scene));

        assert_eq!(
            &wire[..10],
            &[0xdc, 0x05, 0x3d, 0xf6, 0x90, 0x01, 0, 0, 0xe8, 0x03]
        );
        let decoded = frame.to_scene(decode_role_state_request(&wire).unwrap());
        assert!((decoded.position.z - scene.position.z).abs() < 0.02);
        let event = encode_role_state_event(&[StockRoleStateRecord {
            actor_id: 1,
            state: source,
        }]);
        let decoded = frame.to_scene(decode_role_state_event(&event).unwrap()[0].state);
        assert!((decoded.position.x - 6.0).abs() < 0.07);
        assert!((decoded.position.y - 1.2).abs() < 0.07);
        assert!((decoded.position.z - 6.0).abs() < 0.07);
        assert!(decoded.forward.z < -0.99);
    }

    #[test]
    fn stock_anger_requests_match_exe_command_17_33_47_payloads() {
        for (spend, opcode, bytes) in [
            (StockAngerSpend::Full, 18, vec![]),
            (StockAngerSpend::Burst, 36, vec![]),
            (StockAngerSpend::Cost(10), 41, vec![10, 0, 0, 0]),
        ] {
            assert_eq!(spend.opcode(), opcode);
            assert_eq!(spend.encode(), bytes);
            assert_eq!(StockAngerSpend::decode(opcode, &bytes).unwrap(), spend);
        }
        assert!(StockAngerSpend::decode(41, &[10]).is_err());
        assert!(StockAngerSpend::decode(18, &[0]).is_err());
        assert!(StockAngerSpend::decode(7, &[10, 0, 0, 0]).is_err());
    }
    use crate::role_state::{pack_state_word0, StateWords};

    fn sample_state() -> RoleState {
        RoleState {
            position: crate::role_state::RoleVec3 {
                x: 1.2,
                y: 2.4,
                z: -3.6,
            },
            forward: crate::role_state::RoleVec3 {
                x: 0.0,
                y: 0.0,
                z: 1.0,
            },
            velocity: crate::role_state::RoleVec3::ZERO,
            state_words: StateWords {
                state_word0: pack_state_word0(120, true, false, 0),
                state_word1: 9,
                pose_id: 120,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: 9,
            },
        }
    }

    #[test]
    fn enter_role_and_weapon_payloads_match_proven_lengths_and_order() {
        let enter = StockEnterRoleRequest {
            hero: 3,
            camp: 2,
            hero_checksum: 0x1122_3344,
        };
        assert_eq!(
            enter.encode(),
            [3, 0, 0, 0, 2, 0, 0, 0, 0x44, 0x33, 0x22, 0x11]
        );
        assert_eq!(
            StockEnterRoleRequest::decode(&enter.encode()).unwrap(),
            enter
        );

        let entered = StockRoleEnteredEvent {
            actor_id: 7,
            hero: 3,
            hero_checksum: 0x1122_3344,
            camp: 2,
        };
        assert_eq!(
            StockRoleEnteredEvent::decode(&entered.encode()).unwrap(),
            entered
        );

        let left = StockRoleLeftEvent {
            actor_id: 0x1122_3344,
        };
        assert_eq!(left.encode(), [0x44, 0x33, 0x22, 0x11]);
        assert_eq!(StockRoleLeftEvent::decode(&left.encode()).unwrap(), left);

        let rate = StockRoleRate { hz: 17.0 };
        assert_eq!(rate.encode(), [0x00, 0x00, 0x88, 0x41]);
        assert_eq!(StockRoleRate::decode(&rate.encode()).unwrap(), rate);
        assert_eq!(rate.requested_hz(), Some(17));
        assert_eq!(rate.session_hz(), Some(17));
        assert_eq!(StockRoleRate { hz: 7.0 }.session_hz(), None);
        assert_eq!(StockRoleRate { hz: 21.0 }.requested_hz(), Some(21));
        assert_eq!(StockRoleRate { hz: 17.5 }.requested_hz(), None);

        let equipped = StockWeaponEquippedEvent {
            weapon_item_id: 47,
            actor_id: 7,
        };
        assert_eq!(
            StockWeaponEquippedEvent::decode(&equipped.encode()).unwrap(),
            equipped
        );
        assert_eq!(stock_base_weapon_item_id(10), Some(47));
        assert_eq!(stock_weapon_from_item_id(47), Some((10, 0)));
        assert_eq!(stock_weapon_from_item_id(46), Some((1, 5)));
        assert_eq!(stock_weapon_item_id(10, 3), Some(50));
        assert_eq!(stock_weapon_item_id(2, 5), Some(59));
        assert_eq!(stock_weapon_item_id(12, 4), None);
    }

    #[test]
    fn escort_scene_events_match_stock_fixed_vectors() {
        let object = StockWeaponObjectRequest {
            record_key: 6,
            selector: 1,
            position_tenths: [1, -2, 3],
            forward_tenths: [-4, 5, -6],
        };
        assert_eq!(
            object.encode(),
            [
                6, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff, 3, 0, 0, 0, 0xfc, 0xff,
                0xff, 0xff, 5, 0, 0, 0, 0xfa, 0xff, 0xff, 0xff,
            ]
        );
        assert_eq!(
            StockWeaponObjectRequest::decode(&object.encode()).unwrap(),
            object
        );
        let drop_item = StockDropItemEvent { actor_id: 9 };
        assert_eq!(drop_item.encode(), [9, 0, 0, 0]);
        assert_eq!(
            StockDropItemEvent::decode(&drop_item.encode()).unwrap(),
            drop_item
        );

        let spawn = StockSceneItemSpawnRequest {
            item_id: 6,
            category: 1,
            position_tenths: [0x102, -0x203, 0x304],
        };
        assert_eq!(
            spawn.encode(),
            [6, 0, 0, 0, 1, 0, 0, 0, 2, 1, 0, 0, 0xfd, 0xfd, 0xff, 0xff, 4, 3, 0, 0,]
        );
        assert_eq!(
            StockSceneItemSpawnRequest::decode(&spawn.encode()).unwrap(),
            spawn
        );
        assert_eq!(spawn.into_record(9).record_id, 9);

        let record = StockSceneItemRecord {
            record_id: 0x1122_3344,
            item_id: 2,
            category: 4,
            position_tenths: [0x102, -0x203, 0x304],
        };
        assert_eq!(
            record.encode(),
            [
                0x44, 0x33, 0x22, 0x11, 2, 0, 0, 0, 4, 0, 0, 0, 2, 1, 0, 0, 0xfd, 0xfd, 0xff, 0xff,
                4, 3, 0, 0,
            ]
        );
        assert_eq!(
            StockSceneItemRecord::decode(&record.encode()).unwrap(),
            record
        );
        assert_eq!(
            StockSceneItemRecord::decode_many(&[record.encode(), record.encode()].concat())
                .unwrap(),
            vec![record, record]
        );

        let taken = StockSceneTakenEvent {
            actor_id: 7,
            record_id: 0x1122_3344,
            category: 4,
            item_id: 2,
        };
        assert_eq!(
            taken.encode(),
            [7, 0, 0, 0, 0x44, 0x33, 0x22, 0x11, 4, 0, 0, 0, 2, 0, 0, 0]
        );
        assert_eq!(
            StockSceneTakenEvent::decode(&taken.encode()).unwrap(),
            taken
        );

        let returned = StockFlagReturnedEvent {
            record_id: 9,
            category: 4,
            item_id: 2,
            scene_object_id: 215,
        };
        assert_eq!(
            returned.encode(),
            [9, 0, 0, 0, 4, 0, 0, 0, 2, 0, 0, 0, 215, 0, 0, 0]
        );
        assert_eq!(
            StockFlagReturnedEvent::decode(&returned.encode()).unwrap(),
            returned
        );
        let removed = StockFlagRemovedEvent {
            scene_object_id: 215,
        };
        assert_eq!(removed.encode(), [215, 0, 0, 0]);
        assert_eq!(
            StockFlagRemovedEvent::decode(&removed.encode()).unwrap(),
            removed
        );

        let home = StockSceneItemRecord::from_role_position(1, 2, 4, crate::escort::SN03.flag_home);
        let decoded = home.role_position();
        assert!((decoded.x - crate::escort::SN03.flag_home.x).abs() <= 0.012);
        assert!((decoded.y - crate::escort::SN03.flag_home.y).abs() <= 0.012);
        assert!((decoded.z - crate::escort::SN03.flag_home.z).abs() <= 0.012);
    }

    #[test]
    fn net_event56_scene_transaction_matches_stock_fixed_vector() {
        let event = StockSceneOpsEvent {
            timestamp_ms: 0x1234_5678,
            operations: vec![
                StockSceneOperation {
                    reference: 42,
                    kind: STOCK_SCENE_OP_POSE,
                    value: 8,
                    extra: 0,
                },
                StockSceneOperation {
                    reference: 42,
                    kind: STOCK_SCENE_OP_COLLISION,
                    value: 0,
                    extra: 0,
                },
            ],
        };
        assert_eq!(
            event.encode(),
            [
                0x78, 0x56, 0x34, 0x12, 42, 0, 0, 0, 1, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 42, 0, 0,
                0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ]
        );
        assert_eq!(StockSceneOpsEvent::decode(&event.encode()).unwrap(), event);
        assert!(StockSceneOpsEvent::decode(&event.encode()[..35]).is_err());
        assert_eq!(stock_rule3_effect_id("GiMaHIT.ef"), Some(702));
        assert_eq!(stock_rule3_effect_name(695), Some("FireBRK"));
    }

    #[test]
    fn request5_and_hit_events_match_proven_five_dword_layout() {
        let attack = AttackBlock {
            start: 10,
            end: 12,
            attack_type: 3,
            check_friend: 0,
            defense_value: 1.25,
            defense_move: 2.0,
            target_value: 0.5,
            target_move: 4.25,
            target_pose: Some(110),
            target_pose_front: None,
            target_pose_back: None,
            target_pose_left: None,
            target_pose_right: None,
            bones: vec!["weapon".into()],
        };
        let request = StockHitRequest::from_attack(9, 250, 11, &attack, FacingBucket::Left, &[110]);
        assert_eq!(request.target_actor_id, 9);
        assert_eq!(request.hit_word, 250 | (3 << 10) | (10 << 16));
        assert_eq!(request.target_pose_word, 110 | 0x2000);
        assert_eq!(request.moves_word, 425 | (200 << 16));
        assert_eq!(request.values_word, 50 | (125 << 16));
        assert_eq!(StockHitRequest::decode(&request.encode()).unwrap(), request);

        let event = StockHitReactionEvent {
            pose_id: 110,
            move_hundred: 425,
            value_hundred: 50,
            context: (request.hit_word >> 16) & 0xf,
            target_actor_id: 9,
        };
        assert_eq!(
            StockHitReactionEvent::decode(&event.encode()).unwrap(),
            event
        );
        assert_eq!(event.motion_scale(), 4.25);
        assert_eq!(event.minimum_seconds(), 0.5);

        let vitals = StockVitalsEvent {
            health: 1875,
            max_health: 2000,
            anger: 17,
        };
        assert_eq!(vitals.encode(), [0x53, 0x07, 0xd0, 0x07, 17, 0]);
        assert_eq!(StockVitalsEvent::decode(&vitals.encode()).unwrap(), vitals);
        let kill = StockKillEvent {
            attacker_actor_id: 7,
            target_actor_id: 9,
        };
        assert_eq!(kill.encode(), [7, 0, 0, 0, 9, 0, 0, 0]);
        assert_eq!(StockKillEvent::decode(&kill.encode()).unwrap(), kill);
        let score = StockTeamScoreEvent {
            team1_wins: 3,
            team2_wins: 2,
        };
        assert_eq!(score.encode(), [3, 0, 0, 0, 2, 0, 0, 0]);
        assert_eq!(StockTeamScoreEvent::decode(&score.encode()).unwrap(), score);
        let vip = StockVipEvent {
            camp: 2,
            actor_id: 9,
        };
        assert_eq!(vip.encode(), [2, 0, 0, 0, 9, 0, 0, 0]);
        assert_eq!(StockVipEvent::decode(&vip.encode()).unwrap(), vip);
        let cure = StockCureRequest {
            target_actor_ids: vec![3, 9, 12],
        };
        assert_eq!(cure.encode(), [3, 9, 12]);
        assert_eq!(StockCureRequest::decode(&cure.encode()), cure);
        let respawn = StockRespawnEvent { actor_id: 9 };
        assert_eq!(
            StockRespawnEvent::decode(&respawn.encode()).unwrap(),
            respawn
        );
    }

    #[test]
    fn box_scene_effect_indices_match_original_list() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let bytes = std::fs::read(root.join("SFX/effect.lst")).unwrap();
        let list = String::from_utf8_lossy(&bytes);
        for (name, expected) in [("BoxBRK", 691), ("BoxHIT", 692)] {
            assert_eq!(stock_rule3_effect_id(name), Some(expected));
            assert_eq!(stock_rule3_effect_name(expected), Some(name));
            assert_eq!(
                list.lines().nth(expected as usize).unwrap().trim(),
                format!("{name}.ef")
            );
        }
    }

    #[test]
    fn resource_checksum_uses_signed_bytes_and_one_based_index() {
        assert_eq!(
            stock_resource_checksum(&[1, 2, 0xff, 0x80]),
            (-510i32) as u32
        );
    }

    #[test]
    fn source_checksums_match_all_packed_pos_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
        let packs = meteor_asset::pack_repository::PackRepository::open(&root).unwrap();
        let metadata: serde_json::Value =
            serde_json::from_slice(&packs.read("compatibility/stock.json").unwrap()).unwrap();
        assert_eq!(metadata["profile"], "source-bytes");
        for hero in 0..20 {
            let bytes = packs.read(&format!("pmodel/p{hero}.pos")).unwrap();
            let source = fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("../../../origin_res/pmodel/P{hero}.POS")),
            )
            .unwrap();
            assert_eq!(bytes, source);
            assert_eq!(
                metadata["heroes"][hero]["checksum"].as_u64(),
                Some(u64::from(stock_resource_checksum(&bytes)))
            );
        }
    }

    #[test]
    fn stage_config_matches_live_stock_host_event10() {
        let bytes = [
            b's', b'n', b'0', b'6', 0x01, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x3e, 0x00,
            0x00, 0x00,
        ];
        let config = StockStageConfigEvent::decode(&bytes).unwrap();
        assert_eq!(config.stage_id, "sn06");
        assert_eq!(
            (config.field_a, config.field_b, config.field_c),
            (1, 6, 0x3e)
        );
        assert_eq!(config.encode().unwrap(), bytes);
        assert!(matches!(
            StockStageConfigEvent::decode(&[
                b's', b'n', 0x00, b'6', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
            ]),
            Err(StockProtocolError::InvalidStageId(_))
        ));
    }

    #[test]
    fn role_state_request_and_event_batches_reuse_original_codecs() {
        let state = sample_state();
        let request = encode_role_state_request(state);
        assert_eq!(request.len(), REQUEST1_STATE_BYTES);
        assert_eq!(
            decode_role_state_request(&request).unwrap(),
            decode_request1_state(&request).unwrap()
        );

        let records = [
            StockRoleStateRecord { actor_id: 2, state },
            StockRoleStateRecord { actor_id: 3, state },
        ];
        let event = encode_role_state_event(&records);
        assert_eq!(event.len(), 32);
        let decoded = decode_role_state_event(&event).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].actor_id, 2);
        assert_eq!(decoded[1].actor_id, 3);
        assert_eq!(encode_role_state_event(&decoded), event);
    }
}

pub const STOCK_REQUEST_STAGE_CHECKSUM: u16 = 34;

pub const STOCK_EVENT_STAGE_CHECKSUM_REJECTED: u16 = 69;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockStageChecksumRequest {
    pub checksum: u32,
}
impl StockStageChecksumRequest {
    pub fn encode(self) -> [u8; 4] {
        self.checksum.to_le_bytes()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 4)?;
        Ok(Self {
            checksum: u32::from_le_bytes(bytes.try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockStageChecksumRejected {
    pub actor_id: u32,
}
impl StockStageChecksumRejected {
    pub fn encode(self) -> [u8; 4] {
        self.actor_id.to_le_bytes()
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, StockProtocolError> {
        require_length(bytes, 4)?;
        Ok(Self {
            actor_id: u32::from_le_bytes(bytes.try_into().unwrap()),
        })
    }
}

#[cfg(test)]
mod stage_checksum_tests {
    use super::*;
    #[test]
    fn stage_checksum_wire_is_exactly_one_little_endian_dword() {
        let bytes = [0xef, 0xcd, 0xab, 0x89];
        assert_eq!(
            StockStageChecksumRequest {
                checksum: 0x89abcdef
            }
            .encode(),
            bytes
        );
        assert_eq!(
            StockStageChecksumRejected::decode(&bytes).unwrap().actor_id,
            0x89abcdef
        );
        for len in [0, 1, 3, 5, 8] {
            assert!(StockStageChecksumRequest::decode(&vec![0; len]).is_err());
            assert!(StockStageChecksumRejected::decode(&vec![0; len]).is_err());
        }
        assert_eq!(stock_resource_checksum(&[1, 2, 255, 128]), (-510i32) as u32);
        assert_ne!(
            stock_resource_checksum(b"a\nb"),
            stock_resource_checksum(b"a\r\nb")
        );
    }
}
