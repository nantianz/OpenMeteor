use std::collections::BTreeMap;
use std::fmt;

const HEADER_SIZE: usize = 0x30;
const DESCRIPTOR_COUNT_OFFSET: usize = 0x2e;
const DESCRIPTOR_SIZE: usize = 12;
const MAX_RECORD_LIST_LEN: usize = 4096;
const SEED_SIZE: usize = 12;
const SEGMENT_SIZE: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeteorResError {
    HeaderTooShort,
    DescriptorTableOutOfBounds { descriptor_count: u16 },
    InvalidRecord { offset: usize, field: &'static str },
}

impl fmt::Display for MeteorResError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderTooShort => write!(f, "Meteor.res header is shorter than 0x30 bytes"),
            Self::DescriptorTableOutOfBounds { descriptor_count } => write!(
                f,
                "Meteor.res descriptor table ({descriptor_count} entries) exceeds the file"
            ),
            Self::InvalidRecord { offset, field } => {
                write!(f, "invalid Meteor.res record {field} at 0x{offset:x}")
            }
        }
    }
}

impl std::error::Error for MeteorResError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeteorResSegment {
    pub words: [i32; 10],
}

impl MeteorResSegment {
    pub fn active_for_combat(self) -> bool {
        self.words[0] & 2 != 0 && self.words[1] == 0 && self.words[4] == -1
    }

    pub fn property(self) -> i32 {
        self.words[2]
    }

    pub fn operation(self) -> i32 {
        self.words[5]
    }

    pub fn value(self) -> i32 {
        self.words[6]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeteorResPropSeed {
    pub key: i32,
    pub value: i32,
    pub flag: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeteorResRecord {
    pub category: u32,
    pub id: u32,

    pub name: String,

    pub prop_seeds: Vec<MeteorResPropSeed>,
    pub segments: Vec<MeteorResSegment>,
}

impl MeteorResRecord {
    pub fn prop_seed_value(&self, key: i32) -> Option<i32> {
        self.prop_seeds
            .iter()
            .find(|seed| seed.key == key)
            .map(|seed| seed.value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeteorRes {
    pub records: Vec<MeteorResRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeaponCombatStats {
    pub attack: i32,
    pub defense: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitStatusEffect {
    pub id: u32,
    pub duration: f32,
    pub tick_every: f32,
    pub health: i32,
    pub anger: i32,
    pub speed_percent: Option<i32>,
    pub attack_bonus: i32,
    pub defense_bonus: i32,
    pub max_health_bonus: i32,
    pub concealed: bool,
    pub worn_effect: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CombatResourceTable {
    pose_attack_percent: BTreeMap<u16, f32>,
    weapon_stats_by_id: BTreeMap<u8, WeaponCombatStats>,
    weapon_stats_by_model: BTreeMap<String, WeaponCombatStats>,
    pose_status_ids: BTreeMap<u16, u32>,
    pose_self_status_ids: BTreeMap<u16, u32>,
    status_effects: BTreeMap<u32, HitStatusEffect>,
    status_ids_by_model: BTreeMap<String, u32>,
}

impl CombatResourceTable {
    pub fn pose_self_status_effect(&self, pose: u16) -> Option<HitStatusEffect> {
        self.status_effect(*self.pose_self_status_ids.get(&pose)?)
    }

    pub fn pose_status_effect(&self, pose: u16) -> Option<HitStatusEffect> {
        self.status_effect(*self.pose_status_ids.get(&pose)?)
    }

    pub fn status_effect(&self, id: u32) -> Option<HitStatusEffect> {
        self.status_effects.get(&id).copied()
    }

    pub fn status_effect_for_model(&self, model: &str) -> Option<HitStatusEffect> {
        self.status_effect(*self.status_ids_by_model.get(&model.to_ascii_lowercase())?)
    }
    pub fn pose_attack_percent(&self, pose_id: u16) -> Option<f32> {
        self.pose_attack_percent.get(&pose_id).copied()
    }

    pub fn weapon_stats(&self, weapon_id: u8) -> Option<WeaponCombatStats> {
        self.weapon_stats_by_id.get(&weapon_id).copied()
    }

    pub fn weapon_stats_for_model(&self, model: &str) -> Option<WeaponCombatStats> {
        let normalized = model.trim().to_ascii_lowercase();
        self.weapon_stats_by_model
            .get(&normalized)
            .copied()
            .or_else(|| {
                normalized
                    .strip_suffix(['r', 'l', 'a', 'b'])
                    .and_then(|base| self.weapon_stats_by_model.get(base).copied())
            })
    }

    pub fn pose_attack_percent_count(&self) -> usize {
        self.pose_attack_percent.len()
    }

    pub fn weapon_stats_count(&self) -> usize {
        self.weapon_stats_by_id.len()
    }
}

impl MeteorRes {
    pub fn combat_table(&self) -> CombatResourceTable {
        let mut table = CombatResourceTable::default();
        for record in self.records.iter().filter(|record| record.category == 3) {
            if let (Ok(pose), Some(id)) = (u16::try_from(record.id), record.prop_seed_value(18)) {
                if id > 0 {
                    table.pose_self_status_ids.insert(pose, id as u32);
                }
            }
            if let (Ok(pose), Some(id)) = (u16::try_from(record.id), record.prop_seed_value(16)) {
                if id > 0 {
                    table.pose_status_ids.insert(pose, id as u32);
                }
            }
        }

        for record in self.records.iter().filter(|record| record.category == 2) {
            if !record.name.is_empty() && record.name != "0" {
                table
                    .status_ids_by_model
                    .insert(record.name.to_ascii_lowercase(), record.id);
            }
            let mut effect = HitStatusEffect {
                id: record.id,
                duration: record.prop_seed_value(1).unwrap_or(0).max(0) as f32 / 10.0,
                tick_every: 0.0,
                health: 0,
                anger: 0,
                speed_percent: None,
                attack_bonus: 0,
                defense_bonus: 0,
                max_health_bonus: 0,
                concealed: false,
                worn_effect: record
                    .prop_seed_value(15)
                    .filter(|id| *id > 0)
                    .map(|id| id as u32),
            };
            for segment in &record.segments {
                if segment.words[0] & 2 == 0 || segment.words[1] != 0 {
                    continue;
                }
                match (segment.property(), segment.operation()) {
                    (2, 0) => {
                        effect.health += segment.value();

                        effect.tick_every = segment.words[3].max(0) as f32;
                    }
                    (6, 0) => {
                        effect.anger += segment.value();
                        effect.tick_every = segment.words[3].max(0) as f32;
                    }
                    (3, 0) => effect.attack_bonus += segment.value(),
                    (4, 0) => effect.defense_bonus += segment.value(),
                    (1, 0) => effect.max_health_bonus += segment.value(),
                    (10, 0) => effect.concealed |= segment.value() != 0,
                    (5, 1) => effect.speed_percent = Some(segment.value()),
                    _ => {}
                }
            }
            table.status_effects.insert(effect.id, effect);
        }
        for record in &self.records {
            match record.category {
                3 => {
                    let Some(pose_id) = u16::try_from(record.id).ok() else {
                        continue;
                    };
                    if let Some(percent) = record.segments.iter().find_map(|segment| {
                        (segment.active_for_combat()
                            && segment.property() == 3
                            && segment.operation() == 1)
                            .then_some(segment.value() as f32)
                    }) {
                        table.pose_attack_percent.insert(pose_id, percent);
                    }
                }
                1 => {
                    let Some(weapon_id) = u8::try_from(record.id).ok() else {
                        continue;
                    };
                    let attack = record
                        .segments
                        .iter()
                        .find_map(|segment| {
                            (segment.active_for_combat()
                                && segment.property() == 3
                                && segment.operation() == 0)
                                .then_some(segment.value())
                        })
                        .unwrap_or_default();
                    let defense = record
                        .segments
                        .iter()
                        .find_map(|segment| {
                            (segment.active_for_combat()
                                && segment.property() == 4
                                && segment.operation() == 0)
                                .then_some(segment.value())
                        })
                        .unwrap_or_default();
                    let stats = WeaponCombatStats { attack, defense };
                    table.weapon_stats_by_id.insert(weapon_id, stats);
                    if !record.name.is_empty() {
                        table
                            .weapon_stats_by_model
                            .insert(record.name.to_ascii_lowercase(), stats);
                    }
                }
                _ => {}
            }
        }
        table
    }
}

pub fn parse_meteor_res(data: &[u8]) -> Result<MeteorRes, MeteorResError> {
    if data.len() < HEADER_SIZE {
        return Err(MeteorResError::HeaderTooShort);
    }
    let descriptor_count = read_u16(data, DESCRIPTOR_COUNT_OFFSET).expect("header length checked");
    let mut offset = HEADER_SIZE
        .checked_add(usize::from(descriptor_count).saturating_mul(DESCRIPTOR_SIZE))
        .ok_or(MeteorResError::DescriptorTableOutOfBounds { descriptor_count })?;
    if offset > data.len() {
        return Err(MeteorResError::DescriptorTableOutOfBounds { descriptor_count });
    }

    let mut records = Vec::new();
    while offset + 8 <= data.len() {
        let record_offset = offset;
        let category = read_u32(data, offset).expect("record header length checked");
        let id = read_u32(data, offset + 4).expect("record header length checked");
        offset += 8;

        let mut name = String::new();
        for blob_index in 0..3 {
            let len = read_u32(data, offset).ok_or(MeteorResError::InvalidRecord {
                offset: record_offset,
                field: "string length",
            })?;
            offset += 4;
            let len = usize::try_from(len).map_err(|_| MeteorResError::InvalidRecord {
                offset: record_offset,
                field: "string length",
            })?;
            if len > MAX_RECORD_LIST_LEN || offset + len > data.len() {
                return Err(MeteorResError::InvalidRecord {
                    offset: record_offset,
                    field: "string blob",
                });
            }
            if blob_index == 1 {
                name = String::from_utf8_lossy(&data[offset..offset + len])
                    .trim_end_matches('\0')
                    .to_owned();
            }
            offset += len;
        }

        let seed_count = read_u32(data, offset).ok_or(MeteorResError::InvalidRecord {
            offset: record_offset,
            field: "seed count",
        })?;
        offset += 4;
        let seed_count =
            usize::try_from(seed_count).map_err(|_| MeteorResError::InvalidRecord {
                offset: record_offset,
                field: "seed count",
            })?;
        let seed_bytes =
            seed_count
                .checked_mul(SEED_SIZE)
                .ok_or(MeteorResError::InvalidRecord {
                    offset: record_offset,
                    field: "seed list",
                })?;
        if seed_count > MAX_RECORD_LIST_LEN || offset + seed_bytes > data.len() {
            return Err(MeteorResError::InvalidRecord {
                offset: record_offset,
                field: "seed list",
            });
        }
        let mut prop_seeds = Vec::with_capacity(seed_count);
        for _ in 0..seed_count {
            prop_seeds.push(MeteorResPropSeed {
                key: read_i32(data, offset).expect("seed bounds checked"),
                value: read_i32(data, offset + 4).expect("seed bounds checked"),
                flag: read_i32(data, offset + 8).expect("seed bounds checked"),
            });
            offset += SEED_SIZE;
        }

        let segment_count = read_u32(data, offset).ok_or(MeteorResError::InvalidRecord {
            offset: record_offset,
            field: "segment count",
        })?;
        offset += 4;
        let segment_count =
            usize::try_from(segment_count).map_err(|_| MeteorResError::InvalidRecord {
                offset: record_offset,
                field: "segment count",
            })?;
        let segment_bytes =
            segment_count
                .checked_mul(SEGMENT_SIZE)
                .ok_or(MeteorResError::InvalidRecord {
                    offset: record_offset,
                    field: "segment list",
                })?;
        if segment_count > MAX_RECORD_LIST_LEN || offset + segment_bytes > data.len() {
            return Err(MeteorResError::InvalidRecord {
                offset: record_offset,
                field: "segment list",
            });
        }
        let mut segments = Vec::with_capacity(segment_count);
        for _ in 0..segment_count {
            let mut words = [0; 10];
            for (word_index, word) in words.iter_mut().enumerate() {
                *word = read_i32(data, offset + word_index * 4).expect("segment bounds checked");
            }
            offset += SEGMENT_SIZE;
            segments.push(MeteorResSegment { words });
        }

        let name_printable =
            !name.is_empty() && name.bytes().all(|byte| (0x20..=0x7e).contains(&byte));
        if !name_printable && category == 0 && id == 0 && seed_count == 0 && segment_count == 0 {
            break;
        }
        records.push(MeteorResRecord {
            category,
            id,
            name,
            prop_seeds,
            segments,
        });
    }
    Ok(MeteorRes { records })
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset + 4)
        .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_i32(data: &[u8], offset: usize) -> Option<i32> {
    read_u32(data, offset).map(|value| value as i32)
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_headers() {
        assert!(matches!(
            parse_meteor_res(&[0; HEADER_SIZE - 1]),
            Err(MeteorResError::HeaderTooShort)
        ));
    }

    #[test]
    fn original_combat_records_match_web_anchors() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Meteor.res");
        if !path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let parsed = parse_meteor_res(&std::fs::read(path).unwrap()).unwrap();
        let weapon = parsed
            .records
            .iter()
            .find(|record| record.category == 1 && record.id == 6)
            .unwrap();
        assert_eq!(weapon.name, "W0_0");
        assert!(weapon.segments.iter().any(|segment| {
            segment.active_for_combat()
                && segment.property() == 3
                && segment.operation() == 0
                && segment.value() == 110
        }));
        assert!(weapon.segments.iter().any(|segment| {
            segment.active_for_combat()
                && segment.property() == 4
                && segment.operation() == 0
                && segment.value() == 15
        }));
        let pose = parsed
            .records
            .iter()
            .find(|record| record.category == 3 && record.id == 262)
            .unwrap();
        assert!(pose.segments.iter().any(|segment| {
            segment.active_for_combat()
                && segment.property() == 3
                && segment.operation() == 1
                && segment.value() == 35
        }));
        let table = parsed.combat_table();
        assert_eq!(table.pose_attack_percent_count(), 264);
        assert_eq!(table.weapon_stats_count(), 59);
        assert_eq!(table.pose_attack_percent(303), Some(80.0));
        assert_eq!(
            table.weapon_stats_for_model("W0_3"),
            Some(WeaponCombatStats {
                attack: 95,
                defense: 7,
            })
        );

        let item = parsed
            .records
            .iter()
            .find(|record| record.category == 2 && record.id == 1)
            .unwrap();
        assert_eq!(item.name, "Item01");
        assert_eq!(item.prop_seed_value(13), Some(120));
        assert_eq!(item.prop_seed_value(11), Some(0));
        assert_eq!(item.prop_seed_value(14), Some(672));

        for record in parsed
            .records
            .iter()
            .filter(|record| record.category == 2 && record.name != "0" && !record.name.is_empty())
        {
            assert_eq!(
                table.status_effect_for_model(&record.name.to_ascii_uppercase()),
                table.status_effect(record.id),
                "{}",
                record.name
            );
        }
        let hot = table.status_effect_for_model("hot").unwrap();
        assert_eq!(hot.anger, 1);
        assert_eq!(hot.tick_every, 1.0);
        assert!(table.status_effect_for_model("w0_0").is_none());
    }
}
