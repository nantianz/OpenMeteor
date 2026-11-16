use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const RULES_CAPABILITY: u32 = 5;
pub const MAX_RULES_JSON_BYTES: usize = 10 * 1024 * 1024;
const MAX_POSE_OVERRIDES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectCost {
    #[serde(default = "default_effect_cost")]
    pub qi_cost_percent: u8,
}
const fn default_effect_cost() -> u8 {
    20
}
impl Default for EffectCost {
    fn default() -> Self {
        Self {
            qi_cost_percent: 20,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvalidAttackRule {
    #[serde(default = "default_effect_cost")]
    pub qi_cost_percent: u8,
    #[serde(default = "default_repeat_window")]
    pub window_ms: u16,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<u8>,

    #[serde(default, skip_serializing_if = "is_false")]
    pub arm_on_hit: bool,
}
fn is_false(value: &bool) -> bool {
    !*value
}
const fn default_repeat_window() -> u16 {
    1000
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PoseOverride {
    pose_id: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    damage_percent: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    speed_percent: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hit_stun_ms: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    guard_stun_ms: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    super_armor: Option<EffectCost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invalid_attack: Option<InvalidAttackRule>,
}
impl PoseOverride {
    const EMPTY: Self = Self {
        pose_id: 0,
        damage_percent: None,
        speed_percent: None,
        hit_stun_ms: None,
        guard_stun_ms: None,
        super_armor: None,
        invalid_attack: None,
    };
    const fn damage(pose_id: u16, percent: u16) -> Self {
        Self {
            pose_id,
            damage_percent: Some(percent),
            ..Self::EMPTY
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum GuardChipPoses {
    Mode(String),
    Poses(Vec<u16>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RebalanceJson {
    guard_chip_percent: u8,
    guard_chip_poses: GuardChipPoses,
    pose_overrides: Vec<PoseOverride>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RulesJson {
    schema_version: u32,
    #[serde(deserialize_with = "required_rebalance")]
    rebalance: Option<RebalanceJson>,
}
fn required_rebalance<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<RebalanceJson>, D::Error> {
    Option::<RebalanceJson>::deserialize(d)
}

#[derive(Debug, Clone)]
enum PoseRules {
    Static(&'static [PoseOverride]),
    Shared(Arc<[PoseOverride]>),
}
impl PoseRules {
    fn as_slice(&self) -> &[PoseOverride] {
        match self {
            Self::Static(poses) => poses,
            Self::Shared(poses) => poses,
        }
    }
}
impl PartialEq for PoseRules {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}
impl Eq for PoseRules {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RulesJson", into = "RulesJson")]
pub struct BattleRules {
    enabled: bool,
    guard_chip_percent: u8,
    chip_all: bool,
    chip_poses: [u64; 16],
    poses: PoseRules,
}
impl Default for BattleRules {
    fn default() -> Self {
        Self::ORIGINAL
    }
}
impl From<BattleRules> for RulesJson {
    fn from(value: BattleRules) -> Self {
        Self {
            schema_version: 2,
            rebalance: value.enabled.then(|| RebalanceJson {
                guard_chip_percent: value.guard_chip_percent,
                guard_chip_poses: if value.chip_all {
                    GuardChipPoses::Mode("all".into())
                } else {
                    GuardChipPoses::Poses(
                        (0..1024u16)
                            .filter(|pose| value.allows_guard_chip(*pose))
                            .collect(),
                    )
                },
                pose_overrides: value.poses.as_slice().to_vec(),
            }),
        }
    }
}
impl TryFrom<RulesJson> for BattleRules {
    type Error = String;
    fn try_from(value: RulesJson) -> Result<Self, String> {
        if value.schema_version != 2 {
            return Err("unsupported battle rules schemaVersion (expected 2)".into());
        }
        let Some(mut config) = value.rebalance else {
            return Ok(Self::ORIGINAL);
        };
        if config.guard_chip_percent > 100 {
            return Err("guardChipPercent must be in 0..=100".into());
        }
        if config.pose_overrides.len() > MAX_POSE_OVERRIDES {
            return Err(format!(
                "at most {MAX_POSE_OVERRIDES} poseOverrides are supported"
            ));
        }
        let mut chip_poses = [0u64; 16];
        let chip_all = match &config.guard_chip_poses {
            GuardChipPoses::Mode(mode) if mode == "all" => true,
            GuardChipPoses::Mode(_) => {
                return Err("guardChipPoses must be 'all' or an array of Pose IDs".into())
            }
            GuardChipPoses::Poses(poses) => {
                for &pose in poses {
                    if pose > 1023 {
                        return Err("guardChipPoses IDs must be in 0..=1023".into());
                    }
                    let word = &mut chip_poses[usize::from(pose / 64)];
                    let bit = 1u64 << (pose % 64);
                    if *word & bit != 0 {
                        return Err("duplicate Pose in guardChipPoses".into());
                    }
                    *word |= bit;
                }
                chip_poses.iter().all(|word| *word == u64::MAX)
            }
        };
        config.pose_overrides.sort_by_key(|rule| rule.pose_id);
        let mut previous = None;
        let mut repeat_groups = [None; 256];
        for rule in &config.pose_overrides {
            if rule.pose_id > 1023 || previous == Some(rule.pose_id) {
                return Err("poseId must be unique and in 0..=1023".into());
            }
            previous = Some(rule.pose_id);
            if rule.super_armor.is_some_and(|r| r.qi_cost_percent > 100)
                || rule.invalid_attack.is_some_and(|r| {
                    r.qi_cost_percent > 100
                        || !(200..=10000).contains(&r.window_ms)
                        || r.group_id == Some(0)
                })
            {
                return Err(
                    "effect qiCostPercent must be in 0..=100; windowMs in 200..=10000; groupId in 1..=255".into(),
                );
            }
            if let Some(effect) = rule.invalid_attack {
                if let Some(group_id) = effect.group_id {
                    let group = &mut repeat_groups[usize::from(group_id)];
                    if group.is_some_and(|existing| existing != effect) {
                        return Err("all invalidAttack members of a groupId must use the same qiCostPercent, windowMs and armOnHit".into());
                    }
                    *group = Some(effect);
                }
            }
            if rule.damage_percent.is_some_and(|p| p > 1000)
                || rule.speed_percent.is_some_and(|p| !(10..=400).contains(&p))
            {
                return Err("damagePercent must be in 0..=1000; speedPercent in 10..=400".into());
            }
            for ms in [rule.hit_stun_ms, rule.guard_stun_ms].into_iter().flatten() {
                if ms > 10_000 || ms % 10 != 0 {
                    return Err("stun durations must be in 0..=10000 ms, in 10 ms steps".into());
                }
            }
            if *rule
                == (PoseOverride {
                    pose_id: rule.pose_id,
                    ..PoseOverride::EMPTY
                })
            {
                return Err("a poseOverride must change at least one property".into());
            }
        }
        let rules = Self {
            enabled: true,
            guard_chip_percent: config.guard_chip_percent,
            chip_all,
            chip_poses: if chip_all { [0; 16] } else { chip_poses },
            poses: PoseRules::Shared(config.pose_overrides.into()),
        };
        if rules.to_json().len() > MAX_RULES_JSON_BYTES {
            return Err("canonical rules JSON exceeds 10 MiB".into());
        }
        Ok(rules)
    }
}
impl BattleRules {
    pub const ORIGINAL: Self = Self {
        enabled: false,
        guard_chip_percent: 0,
        chip_all: false,
        chip_poses: [0; 16],
        poses: PoseRules::Static(&[]),
    };

    pub const BALANCED: Self = Self {
        enabled: true,
        guard_chip_percent: 10,
        chip_all: true,
        chip_poses: [0; 16],
        poses: PoseRules::Static(&[PoseOverride::damage(257, 60), PoseOverride::damage(258, 60)]),
    };
    pub fn from_json(json: &str) -> Result<Self, String> {
        if json.len() > MAX_RULES_JSON_BYTES {
            return Err("battle rules JSON exceeds 10 MiB".into());
        }
        serde_json::from_str(json).map_err(|error| format!("invalid battle rules JSON: {error}"))
    }
    pub fn bundled() -> Self {
        Self::from_json(include_str!("../../../config/balance-rules.json"))
            .expect("valid bundled rules")
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self).expect("integer rules serialize")
    }
    fn pose(&self, id: u16) -> Option<PoseOverride> {
        self.poses
            .as_slice()
            .iter()
            .find(|rule| rule.pose_id == id)
            .copied()
    }
    pub fn super_armor(&self, pose: u16) -> Option<EffectCost> {
        self.pose(pose).and_then(|p| p.super_armor)
    }
    pub fn invalid_attack(&self, pose: u16) -> Option<InvalidAttackRule> {
        self.pose(pose).and_then(|p| p.invalid_attack)
    }
    pub fn invalid_attack_key(&self, pose: u16) -> Option<(InvalidAttackRule, u16)> {
        self.invalid_attack(pose).map(|rule| {
            let key = rule.group_id.map_or(pose, |group| 1024 + u16::from(group));
            (rule, key)
        })
    }
    pub fn damage(&self, pose: u16, normal_damage: i32) -> i32 {
        let percent = self
            .pose(pose)
            .and_then(|r| r.damage_percent)
            .unwrap_or(100);
        (i64::from(normal_damage.max(0)) * i64::from(percent) / 100).min(i64::from(i32::MAX)) as i32
    }
    pub fn speed_scale(&self, pose: u16) -> f32 {
        f32::from(self.pose(pose).and_then(|r| r.speed_percent).unwrap_or(100)) / 100.0
    }
    pub fn hit_stun_seconds(&self, pose: u16, original: f32) -> f32 {
        self.pose(pose)
            .and_then(|r| r.hit_stun_ms)
            .map_or(original, |ms| f32::from(ms) / 1000.0)
    }
    pub fn guard_stun_seconds(&self, pose: u16, original: f32) -> f32 {
        self.pose(pose)
            .and_then(|r| r.guard_stun_ms)
            .map_or(original, |ms| f32::from(ms) / 1000.0)
    }
    pub fn allows_guard_chip(&self, pose: u16) -> bool {
        pose <= 1023
            && (self.chip_all
                || self.chip_poses[usize::from(pose / 64)] & (1u64 << (pose % 64)) != 0)
    }
    pub fn guard_damage(&self, pose: u16, normal_damage: i32) -> i32 {
        if !self.enabled || self.guard_chip_percent == 0 || !self.allows_guard_chip(pose) {
            return 0;
        }
        let numerator = i64::from(normal_damage.max(0)) * i64::from(self.guard_chip_percent);
        ((numerator + 99) / 100).max(1) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pose_overrides_are_generic_and_order_independent() {
        let rules = BattleRules::from_json(r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":10,"guardChipPoses":"all","poseOverrides":[{"poseId":262,"damagePercent":150,"speedPercent":125,"hitStunMs":400,"guardStunMs":250},{"poseId":258,"damagePercent":45}]}}"#).unwrap();
        assert_eq!(rules.damage(262, 100), 150);
        assert_eq!(rules.damage(258, 100), 45);
        assert_eq!(rules.damage(263, 100), 100);
        assert_eq!(rules.speed_scale(262), 1.25);
        assert_eq!(rules.speed_scale(263), 1.0);
        assert_eq!(rules.hit_stun_seconds(262, 0.0), 0.4);
        assert_eq!(rules.guard_stun_seconds(262, 0.0), 0.25);
        assert_eq!(rules.hit_stun_seconds(263, 0.7), 0.7);
        assert_eq!(BattleRules::from_json(&rules.to_json()).unwrap(), rules);
        assert_eq!(rules.guard_damage(262, 57), 6);
        assert_eq!(rules.guard_damage(262, 0), 1);
    }
    #[test]
    fn bundled_final_rules_load_and_round_trip() {
        let rules = BattleRules::bundled();
        let json: serde_json::Value = serde_json::from_str(&rules.to_json()).unwrap();
        assert_eq!(
            json["rebalance"]["poseOverrides"].as_array().unwrap().len(),
            76
        );
        assert_eq!(BattleRules::from_json(&rules.to_json()).unwrap(), rules);
        assert_eq!(rules.guard_damage(257, 57), 3);
        assert_eq!(rules.damage(257, 100), 60);
        assert_eq!(rules.damage(570, 100), 300);
        assert_eq!(rules.speed_scale(282), 0.9);
        assert_eq!(rules.speed_scale(323), 1.1);
        assert_eq!(rules.super_armor(301).unwrap().qi_cost_percent, 7);
        assert_eq!(rules.invalid_attack(282).unwrap().qi_cost_percent, 0);
        assert_eq!(rules.invalid_attack(282).unwrap().window_ms, 1500);
        assert_eq!(rules.invalid_attack(412).unwrap().window_ms, 600);
        assert_eq!(rules.invalid_attack(413).unwrap().window_ms, 600);
    }

    #[test]
    fn pose_override_capacity_is_bounded_and_round_trips_without_loss() {
        assert!(
            std::mem::size_of::<BattleRules>() <= 256,
            "rules must remain small on the stack"
        );
        let mut config = serde_json::json!({
            "schemaVersion": 2,
            "rebalance": {"guardChipPercent": 5, "guardChipPoses": "all", "poseOverrides": []}
        });
        let overrides = config["rebalance"]["poseOverrides"].as_array_mut().unwrap();
        for pose in (0..1024).rev() {
            overrides.push(serde_json::json!({"poseId": pose, "damagePercent": 75}));
        }
        let rules = BattleRules::from_json(&config.to_string()).unwrap();
        assert_eq!(rules.damage(1023, 100), 75);
        assert_eq!(rules.damage(1024, 100), 100);
        assert_eq!(BattleRules::from_json(&rules.to_json()).unwrap(), rules);
        config["rebalance"]["poseOverrides"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"poseId": 0, "damagePercent": 75}));
        assert!(BattleRules::from_json(&config.to_string())
            .unwrap_err()
            .contains("at most 1024 poseOverrides"));
    }

    #[test]
    fn guard_chip_pose_list_is_explicit_and_canonical() {
        let rules = BattleRules::from_json(r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":10,"guardChipPoses":[262,258],"poseOverrides":[]}}"#).unwrap();
        assert_eq!(rules.guard_damage(258, 57), 6);
        assert_eq!(rules.guard_damage(262, 0), 1);
        assert_eq!(rules.guard_damage(257, 57), 0);
        assert_eq!(BattleRules::from_json(&rules.to_json()).unwrap(), rules);
        for list in [r#""none""#, "[258,258]", "[1024]", "[-1]"] {
            let json = format!(
                r#"{{"schemaVersion":2,"rebalance":{{"guardChipPercent":10,"guardChipPoses":{list},"poseOverrides":[]}}}}"#
            );
            assert!(BattleRules::from_json(&json).is_err());
        }
    }

    #[test]
    fn rejects_ambiguous_unsupported_and_invalid_overrides() {
        for overrides in [
            r#"[{"poseId":257},{"poseId":257}]"#,
            r#"[{"poseId":257,"damagePercent":50},{"poseId":257,"speedPercent":120}]"#,
            r#"[{"poseId":1024,"damagePercent":50}]"#,
            r#"[{"poseId":257,"speedPercent":0}]"#,
            r#"[{"poseId":257,"damagePercent":-1}]"#,
            r#"[{"poseId":257,"hitStunMs":123}]"#,
            r#"[{"poseId":257,"guardStunMs":10010}]"#,
            r#"[{"poseId":257,"unknown":1}]"#,
        ] {
            let json = format!(
                r#"{{"schemaVersion":2,"rebalance":{{"guardChipPercent":10,"guardChipPoses":"all","poseOverrides":{overrides}}}}}"#
            );
            assert!(BattleRules::from_json(&json).is_err(), "{json}");
        }
        assert!(BattleRules::from_json(r#"{"schemaVersion":1,"rebalance":null}"#).is_err());
        assert!(BattleRules::from_json(r#"{"schemaVersion":2}"#).is_err());
    }

    #[test]
    fn accepts_ten_mib_json_and_rejects_the_next_byte() {
        let canonical = BattleRules::BALANCED.to_json();
        let mut json = canonical.clone();
        json.push_str(&" ".repeat(MAX_RULES_JSON_BYTES - canonical.len()));
        assert_eq!(json.len(), MAX_RULES_JSON_BYTES);
        assert_eq!(
            BattleRules::from_json(&json).unwrap(),
            BattleRules::BALANCED
        );
        json.push(' ');
        assert_eq!(
            BattleRules::from_json(&json).unwrap_err(),
            "battle rules JSON exceeds 10 MiB"
        );
    }

    #[test]
    fn repeat_group_is_canonical_and_rejects_conflicting_members() {
        let json = r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":0,"guardChipPoses":[],"poseOverrides":[{"poseId":413,"invalidAttack":{"groupId":1,"armOnHit":true,"windowMs":800,"qiCostPercent":0}},{"poseId":407,"invalidAttack":{"groupId":1,"armOnHit":true,"windowMs":800,"qiCostPercent":0}}]}}"#;
        let rules = BattleRules::from_json(json).unwrap();
        assert_eq!(
            rules.invalid_attack_key(407).unwrap().1,
            rules.invalid_attack_key(413).unwrap().1
        );
        assert_eq!(BattleRules::from_json(&rules.to_json()).unwrap(), rules);
        assert!(
            rules.to_json().find("\"poseId\":407").unwrap()
                < rules.to_json().find("\"poseId\":413").unwrap()
        );
        assert!(
            BattleRules::from_json(&json.replacen("\"windowMs\":800", "\"windowMs\":900", 1))
                .is_err()
        );
        assert!(BattleRules::from_json(&json.replace("\"groupId\":1", "\"groupId\":0")).is_err());
        assert!(!BattleRules::bundled().to_json().contains("groupId"));
    }
}
