use encoding_rs::GB18030;
use meteor_asset::pos::{parse_poses, AttackBlock, Pose};
use meteor_asset::res::{
    parse_meteor_res, CombatResourceTable, MeteorRes, MeteorResError, WeaponCombatStats,
};
use meteor_combat::hitbox::Obb;
use meteor_core::Vec3;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::Path;

pub const CHARACTER_COUNT: u8 = 20;
pub const PACKED_OBB_SIZE: usize = 15;
pub const MAX_COMBAT_COORDINATE: f32 = 512.0;
pub const MAX_ATTACK_OBB_HALF_SIZE: f32 = 64.0;
pub const MAX_HURT_OBB_HALF_SIZE: f32 = 24.0;

pub const ATTACK_SOURCE_PRECISE: u8 = 1;
pub const ATTACK_SOURCE_EFFECT: u8 = 2;
pub const ATTACK_SOURCE_PLAIN_RANGED: u8 = 4;

#[derive(Debug)]
pub enum AttackCatalogError {
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    ParseResource {
        path: std::path::PathBuf,
        source: MeteorResError,
    },
}

impl fmt::Display for AttackCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    f,
                    "unable to read authored POS file {}: {source}",
                    path.display()
                )
            }
            Self::ParseResource { path, source } => {
                write!(
                    f,
                    "unable to parse original resource file {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for AttackCatalogError {}

pub type WeaponStats = WeaponCombatStats;

#[derive(Debug, Clone)]
pub struct AttackRecord {
    pub pose_start: u32,
    pub start: u32,
    pub end: u32,
    pub source_flags: u8,
    pub attack: AttackBlock,
}

#[derive(Debug, Clone, Default)]
pub struct AttackCatalog {
    poses: HashMap<u8, BTreeMap<u16, Pose>>,
    combat_resources: CombatResourceTable,

    resource_registry: Option<MeteorRes>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneItemMetadata {
    pub category: u32,
    pub id: u32,
    pub room_group: Option<i32>,
    pub respawn_seconds: Option<i32>,
}

impl AttackCatalog {
    pub fn from_origin_res(root: &Path) -> Result<Self, AttackCatalogError> {
        Self::from_reader(root, |path| std::fs::read(path))
    }

    pub fn from_mpk() -> Result<Self, AttackCatalogError> {
        Self::from_reader(Path::new(""), |path| {
            meteor_asset::runtime_assets::read_file(path)
        })
    }

    fn from_reader(
        root: &Path,
        read: impl Fn(&Path) -> std::io::Result<Vec<u8>>,
    ) -> Result<Self, AttackCatalogError> {
        let mut sources = Vec::with_capacity(CHARACTER_COUNT as usize);
        for character_id in 0..CHARACTER_COUNT {
            let path = root.join("pmodel").join(format!("P{character_id}.POS"));
            let bytes = read(&path).map_err(|source| AttackCatalogError::Read {
                path: path.clone(),
                source,
            })?;
            sources.push((character_id, decode_legacy_text(&bytes)));
        }
        let mut catalog = Self::from_pos_sources(sources);
        let resource_path = root.join("Meteor.res");
        let resource_bytes = read(&resource_path).map_err(|source| AttackCatalogError::Read {
            path: resource_path.clone(),
            source,
        })?;
        let resource = parse_meteor_res(&resource_bytes).map_err(|source| {
            AttackCatalogError::ParseResource {
                path: resource_path,
                source,
            }
        })?;
        catalog.load_combat_stats(&resource);
        catalog.resource_registry = Some(resource);
        Ok(catalog)
    }

    pub fn from_pos_sources<I>(sources: I) -> Self
    where
        I: IntoIterator<Item = (u8, String)>,
    {
        let poses = sources
            .into_iter()
            .map(|(character_id, text)| {
                let entries = parse_poses(&text)
                    .into_iter()
                    .filter(|pose| !pose.attacks.is_empty())
                    .map(|pose| (pose.id, pose))
                    .collect();
                (character_id, entries)
            })
            .collect();
        Self {
            poses,
            combat_resources: CombatResourceTable::default(),
            resource_registry: None,
        }
    }

    pub fn attack_record(
        &self,
        character_id: u8,
        pose_id: u16,
        attack_index: u32,
    ) -> Option<AttackRecord> {
        let pose = self.poses.get(&character_id)?.get(&pose_id)?;
        let mut attack = pose
            .attacks
            .get(usize::try_from(attack_index).ok()?)?
            .clone();

        attack.start = attack.start.max(pose.start);
        attack.end = attack.end.min(pose.end);
        (attack.start <= attack.end).then(|| AttackRecord {
            pose_start: pose.start,
            start: attack.start,
            end: attack.end,
            source_flags: attack_source_flags(&attack),
            attack,
        })
    }

    pub fn active_attack_index(&self, character_id: u8, pose_id: u16, frame: f32) -> Option<u32> {
        if !frame.is_finite() || frame < 0.0 {
            return None;
        }
        let pose = self.poses.get(&character_id)?.get(&pose_id)?;
        let integer_frame = frame.floor() as u32;
        pose.attacks
            .iter()
            .enumerate()
            .filter_map(|(index, attack)| {
                let start = attack.start.max(pose.start);
                let end = attack.end.min(pose.end);
                (start <= end && (start..=end).contains(&integer_frame)).then_some(index as u32)
            })
            .next_back()
    }

    pub fn character_count(&self) -> usize {
        self.poses.len()
    }

    pub fn pose_attack_percent(&self, pose_id: u16) -> Option<f32> {
        self.combat_resources.pose_attack_percent(pose_id)
    }

    pub fn pose_self_status_effect(
        &self,
        pose_id: u16,
    ) -> Option<meteor_asset::res::HitStatusEffect> {
        self.combat_resources.pose_self_status_effect(pose_id)
    }

    pub fn pose_status_effect(&self, pose_id: u16) -> Option<meteor_asset::res::HitStatusEffect> {
        self.combat_resources.pose_status_effect(pose_id)
    }

    pub fn weapon_stats(&self, weapon_id: u8) -> Option<WeaponStats> {
        self.combat_resources.weapon_stats(weapon_id)
    }

    pub fn weapon_stats_for_model(&self, model: &str) -> Option<WeaponStats> {
        self.combat_resources.weapon_stats_for_model(model)
    }

    pub fn pose_attack_percent_count(&self) -> usize {
        self.combat_resources.pose_attack_percent_count()
    }

    pub fn weapon_stats_count(&self) -> usize {
        self.combat_resources.weapon_stats_count()
    }

    pub fn scene_item_metadata(&self, model: &str) -> Option<SceneItemMetadata> {
        let normalized = model.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return None;
        }
        let record = self
            .resource_registry
            .as_ref()?
            .records
            .iter()
            .find(|record| {
                matches!(record.category, 1 | 2)
                    && record.name.trim().eq_ignore_ascii_case(&normalized)
            })?;
        Some(SceneItemMetadata {
            category: record.category,
            id: record.id,
            room_group: record.prop_seed_value(11),
            respawn_seconds: record.prop_seed_value(13),
        })
    }

    fn load_combat_stats(&mut self, resource: &MeteorRes) {
        self.combat_resources = resource.combat_table();
    }
}

fn decode_legacy_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => GB18030.decode(bytes).0.into_owned(),
    }
}

fn attack_source_flags(attack: &AttackBlock) -> u8 {
    let bones = attack
        .bones
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut flags = 0;
    if bones.iter().any(|name| name != "effect") {
        flags |= ATTACK_SOURCE_PRECISE;
    }
    if bones.iter().any(|name| name == "effect") {
        flags |= ATTACK_SOURCE_EFFECT;
    }
    if bones.is_empty() {
        flags |= ATTACK_SOURCE_PLAIN_RANGED;
    }
    flags
}

pub fn decode_packed_obb(values: &[f32], max_half_size: f32) -> Option<Obb> {
    if values.len() != PACKED_OBB_SIZE
        || !max_half_size.is_finite()
        || max_half_size <= 0.0
        || values[..12]
            .iter()
            .any(|value| !value.is_finite() || value.abs() > MAX_COMBAT_COORDINATE)
        || values[12..]
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0 || *value > max_half_size)
        || values[12..].iter().all(|value| *value <= 1e-5)
    {
        return None;
    }

    let mut axes = [
        Vec3::new(values[3], values[4], values[5]),
        Vec3::new(values[6], values[7], values[8]),
        Vec3::new(values[9], values[10], values[11]),
    ];
    for axis in &mut axes {
        let length = axis.dot(axis).sqrt();
        if !(0.9..=1.1).contains(&length) {
            return None;
        }
        *axis = *axis * (1.0 / length);
    }
    if axes[0].dot(&axes[1]).abs() > 0.08
        || axes[0].dot(&axes[2]).abs() > 0.08
        || axes[1].dot(&axes[2]).abs() > 0.08
    {
        return None;
    }
    Some(Obb {
        center: Vec3::new(values[0], values[1], values[2]),
        axes,
        half_size: Vec3::new(values[12], values[13], values[14]),
    })
}

pub fn obb_center_near(obb: &Obb, point: Vec3, max_xz: f32, max_y: f32) -> bool {
    let dx = obb.center.x - point.x;
    let dz = obb.center.z - point.z;
    (dx * dx + dz * dz).sqrt() <= max_xz && (obb.center.y - point.y).abs() <= max_y
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_pos_catalog_matches_web_pose_200_record() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let catalog = AttackCatalog::from_origin_res(&root).unwrap();
        assert_eq!(catalog.character_count(), CHARACTER_COUNT as usize);
        let record = catalog.attack_record(0, 200, 0).unwrap();

        assert_eq!(
            (record.pose_start, record.start, record.end),
            (3511, 3517, 3519)
        );
        assert_eq!(record.source_flags, ATTACK_SOURCE_PLAIN_RANGED);
        assert_eq!(catalog.active_attack_index(0, 200, 3518.99), Some(0));
        assert_eq!(catalog.active_attack_index(0, 200, 3520.0), None);
        assert_eq!(
            catalog
                .attack_record(0, 262, 0)
                .map(|record| record.source_flags),
            Some(ATTACK_SOURCE_PRECISE),
            "the shipped multiline bone = syntax remains a precise melee source"
        );

        assert_eq!(catalog.pose_attack_percent_count(), 264);
        assert_eq!(catalog.weapon_stats_count(), 59);

        assert_eq!(
            catalog.scene_item_metadata("item01"),
            Some(SceneItemMetadata {
                category: 2,
                id: 1,
                room_group: Some(0),
                respawn_seconds: Some(120),
            })
        );
        assert_eq!(
            catalog.scene_item_metadata("W0_0"),
            Some(SceneItemMetadata {
                category: 1,
                id: 6,
                room_group: Some(0),
                respawn_seconds: Some(30),
            })
        );
        assert_eq!(catalog.pose_attack_percent(262), Some(35.0));
        assert_eq!(catalog.pose_attack_percent(303), Some(80.0));
        assert_eq!(
            catalog.weapon_stats(6),
            Some(WeaponStats {
                attack: 110,
                defense: 15,
            })
        );
    }

    #[test]
    fn packed_obb_rejects_skew_and_normalizes_rounding_drift() {
        let values = [
            1.0, 2.0, 3.0, 1.0001, 0.0, 0.0, 0.0, 0.9999, 0.0, 0.0, 0.0, 1.0, 0.5, 1.0, 0.5,
        ];
        let obb = decode_packed_obb(&values, MAX_HURT_OBB_HALF_SIZE).unwrap();
        assert!(obb_center_near(&obb, Vec3::new(1.0, 2.0, 3.0), 0.01, 0.01));
        let skew = [
            0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.99, 0.1, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0,
        ];
        assert!(decode_packed_obb(&skew, MAX_HURT_OBB_HALF_SIZE).is_none());
    }
}
