use meteor_asset::pos::AttackBlock;

pub const GUARD_ANGER_DIVISOR: i64 = 500;

pub fn guard_anger_gain(raw_atk: i32) -> i32 {
    ((14_i64 * i64::from(raw_atk.max(0))) / GUARD_ANGER_DIVISOR) as i32
}

#[derive(Debug, Clone)]
pub struct HitResult {
    pub rebalance_effect: u8,
    pub guarded: bool,
    pub damage: i32,
    pub raw: i32,
    pub scale: f32,
    pub atk: i32,
    pub def: i32,
    pub anger_gain: i32,
    pub killed: bool,
    pub outcome: HitOutcome,
    pub hit_key: u32,
    pub force_hit_flags: u32,
    pub dword1: u32,

    pub defense_value: f32,
    pub defense_move: f32,

    pub target_value: f32,
    pub target_move: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitOutcome {
    Guard,
    BeHit,
    None,
}

pub trait CombatStats {
    fn try_spend_anger(&mut self, cost: f32) -> bool {
        cost == 0.0
    }
    fn hp(&self) -> f32;
    fn alive(&self) -> bool;
    fn invincible(&self) -> bool;
    fn anger(&self) -> f32;
    fn anger_max(&self) -> f32;
    fn def(&self) -> f32;
    fn attack_bonus(&self) -> f32;

    fn take_damage(&mut self, amount: i32) -> i32;

    fn gain_anger_from_damage(&mut self, damage: i32) -> i32;

    fn gain_anger_on_guard(&mut self, raw_atk: i32) -> i32;

    fn reset_spawn_clock(&mut self);
}

pub fn force_hit_flags_from_attack_type(attack_type: i32) -> u32 {
    (attack_type & 0x3f) as u32
}

pub fn hit_key_from_pose(pose_id: u16) -> u32 {
    (pose_id as u32) & 0x3ff
}

pub fn pack_hit_dword1(hit_key: u32, force_hit_flags: u32, extra_nibble: u32) -> u32 {
    (hit_key & 0x3ff) | ((force_hit_flags & 0x3f) << 10) | ((extra_nibble & 0xf) << 16)
}

#[derive(Debug, Clone, Copy)]
pub struct HitDword1 {
    pub hit_key: u32,
    pub force_hit_flags: u32,
    pub extra_nibble: u32,
}

pub fn unpack_hit_dword1(dword1: u32) -> HitDword1 {
    HitDword1 {
        hit_key: dword1 & 0x3ff,
        force_hit_flags: (dword1 >> 10) & 0x3f,
        extra_nibble: (dword1 >> 16) & 0xf,
    }
}

pub fn is_force_hit(force_hit_flags: u32) -> bool {
    force_hit_flags != 0
}

pub fn team_mask(team: i32) -> u32 {
    if team <= 0 {
        return 0;
    }

    1u32.checked_shl((team - 1) as u32).unwrap_or(0)
}

pub fn team_damage_scale(attacker_team: i32, defender_team: i32) -> f32 {
    if (team_mask(attacker_team) & team_mask(defender_team)) != 0 {
        0.2
    } else {
        1.0
    }
}

pub fn weapon_base_atk(weapon_id: u8) -> i32 {
    match weapon_id {
        1 => 75,
        2 => 70,
        3 => 80,
        4 => 85,
        5 => 90,
        6 => 110,
        7 => 115,
        8 => 130,
        9 => 140,
        10 => 112,
        11 => 87,
        12 => 100,
        _ => 110,
    }
}

pub fn sum_attack_power(
    weapon_id: u8,
    pose_id: u16,
    hit_key: u32,
    attack_power: Option<i32>,
    attack_bonus: i32,
    pose_attack_percent: &dyn Fn(u16) -> Option<f32>,
) -> i32 {
    let equipped = attack_power.unwrap_or(0);
    let weapon_atk = if equipped > 0 {
        equipped
    } else {
        weapon_base_atk(weapon_id)
    };
    let base = weapon_atk + attack_bonus;
    let percent = pose_attack_percent(pose_id).or_else(|| pose_attack_percent(hit_key as u16));
    match percent {
        None => base,
        Some(p) => base + ((p * 0.01 - 1.0) * base as f32).trunc() as i32,
    }
}

pub struct HitOptions<'a> {
    pub attacker_team: i32,

    pub defender_team: i32,

    pub defender_in_guard: bool,

    pub defender_weapon_def: Option<i32>,

    pub weapon_id: u8,

    pub pose_id: u16,

    pub attack_power: Option<i32>,

    pub attack_bonus: i32,

    pub hit_key: Option<u32>,

    pub force_hit_flags: Option<u32>,

    pub attack_type: i32,

    pub dword1: Option<u32>,

    pub attack: Option<&'a AttackBlock>,

    pub pose_attack_percent: &'a dyn Fn(u16) -> Option<f32>,
}

pub fn resolve_hit<S: CombatStats>(defender_stats: &mut S, opts: &HitOptions) -> HitResult {
    resolve_hit_with_rules(
        defender_stats,
        opts,
        crate::battle_rules::BattleRules::ORIGINAL,
    )
}

pub fn resolve_hit_with_rules<S: CombatStats>(
    defender_stats: &mut S,
    opts: &HitOptions,
    rules: crate::battle_rules::BattleRules,
) -> HitResult {
    let packed = opts.dword1.map(unpack_hit_dword1);

    let force_hit_flags = packed
        .map(|p| p.force_hit_flags)
        .or(opts.force_hit_flags)
        .unwrap_or_else(|| force_hit_flags_from_attack_type(opts.attack_type));

    let hit_key = packed
        .map(|p| p.hit_key)
        .or(opts.hit_key)
        .unwrap_or_else(|| hit_key_from_pose(opts.pose_id));

    let can_guard = !is_force_hit(force_hit_flags) && opts.defender_in_guard;

    let atk = sum_attack_power(
        opts.weapon_id,
        opts.pose_id,
        hit_key,
        opts.attack_power,
        opts.attack_bonus,
        opts.pose_attack_percent,
    );

    let equipped_def = opts.defender_weapon_def.unwrap_or(0);
    let def = (defender_stats.def() as i32) + equipped_def;
    let raw = (atk - def).max(0);
    let scale = team_damage_scale(opts.attacker_team, opts.defender_team);
    let damage = rules.damage(opts.pose_id, ((raw as f32) * scale).trunc().max(0.0) as i32);

    if can_guard {
        let same_team = (team_mask(opts.attacker_team) & team_mask(opts.defender_team)) != 0;
        let anger_gain = if same_team {
            0
        } else {
            defender_stats.gain_anger_on_guard(raw)
        };
        let dword1 = pack_hit_dword1(hit_key, force_hit_flags, 0);
        let (defense_value, defense_move) = opts
            .attack
            .map(|a| (a.defense_value, a.defense_move))
            .unwrap_or((0.0, 0.0));
        let defense_value = rules.guard_stun_seconds(opts.pose_id, defense_value);
        let chip = rules.guard_damage(opts.pose_id, damage);
        let applied = if chip > 0 {
            defender_stats.take_damage(chip)
        } else {
            0
        };
        return HitResult {
            rebalance_effect: 0,
            guarded: true,
            damage: applied,
            raw,
            scale: 1.0,
            atk,
            def,
            anger_gain,
            killed: chip > 0 && !defender_stats.alive(),
            outcome: HitOutcome::Guard,
            hit_key,
            force_hit_flags,
            dword1,
            defense_value,
            defense_move,
            target_value: 0.0,
            target_move: 0.0,
        };
    }

    let applied = defender_stats.take_damage(damage);
    let anger_gain = defender_stats.gain_anger_from_damage(applied);
    let killed = !defender_stats.alive();
    let dword1 = pack_hit_dword1(hit_key, force_hit_flags, 0);
    let (target_value, target_move) = opts
        .attack
        .map(|a| (a.target_value, a.target_move))
        .unwrap_or((0.0, 0.0));
    let target_value = rules.hit_stun_seconds(opts.pose_id, target_value);
    HitResult {
        rebalance_effect: 0,
        guarded: false,
        damage: applied,
        raw,
        scale,
        atk,
        def,
        anger_gain,
        killed,
        outcome: HitOutcome::BeHit,
        hit_key,
        force_hit_flags,
        dword1,
        defense_value: 0.0,
        defense_move: 0.0,
        target_value,
        target_move,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubStats {
        hp: f32,
        alive: bool,
        invincible: bool,
        anger: f32,
        anger_max: f32,
        def: f32,
        attack_bonus: f32,
    }

    impl StubStats {
        fn new() -> Self {
            Self {
                hp: 1000.0,
                alive: true,
                invincible: false,
                anger: 0.0,
                anger_max: 100.0,
                def: 0.0,
                attack_bonus: 0.0,
            }
        }
    }

    impl CombatStats for StubStats {
        fn hp(&self) -> f32 {
            self.hp
        }
        fn alive(&self) -> bool {
            self.alive
        }
        fn invincible(&self) -> bool {
            self.invincible
        }
        fn anger(&self) -> f32 {
            self.anger
        }
        fn anger_max(&self) -> f32 {
            self.anger_max
        }
        fn def(&self) -> f32 {
            self.def
        }
        fn attack_bonus(&self) -> f32 {
            self.attack_bonus
        }
        fn take_damage(&mut self, amount: i32) -> i32 {
            let dmg = amount.max(0);
            if dmg <= 0 || !self.alive || self.invincible {
                return 0;
            }
            let before = self.hp;
            self.hp = (self.hp - dmg as f32).max(0.0);
            if self.hp <= 0.0 {
                self.alive = false;
            }
            (before - self.hp) as i32
        }
        fn gain_anger_from_damage(&mut self, damage: i32) -> i32 {
            let dmg = damage.max(0);
            if dmg <= 0 {
                return 0;
            }
            let gain = (14 * dmg) / 100;
            let before = self.anger;
            self.anger = (self.anger + gain as f32).min(self.anger_max);
            (self.anger - before) as i32
        }
        fn gain_anger_on_guard(&mut self, raw_atk: i32) -> i32 {
            let gain = guard_anger_gain(raw_atk);
            if gain <= 0 {
                return 0;
            }
            let before = self.anger;
            self.anger = (self.anger + gain as f32).min(self.anger_max);
            (self.anger - before) as i32
        }
        fn reset_spawn_clock(&mut self) {}
    }

    fn sword_percent(pose_id: u16) -> Option<f32> {
        match pose_id {
            262 => Some(35.0),
            _ => None,
        }
    }

    fn base_opts<'a>() -> HitOptions<'a> {
        HitOptions {
            attacker_team: 1,
            defender_team: 2,
            defender_in_guard: false,
            defender_weapon_def: None,
            weapon_id: 6,
            pose_id: 262,
            attack_power: None,
            attack_bonus: 0,
            hit_key: None,
            force_hit_flags: None,
            attack_type: 0,
            dword1: None,
            attack: None,
            pose_attack_percent: &sword_percent,
        }
    }

    #[test]
    fn team_mask_basic() {
        assert_eq!(team_mask(0), 0);
        assert_eq!(team_mask(1), 1);
        assert_eq!(team_mask(2), 2);
        assert_eq!(team_mask(3), 4);
        assert_eq!(team_mask(32), 1 << 31);
        assert_eq!(team_mask(33), 0);
        assert_eq!(team_mask(i32::MAX), 0);
        assert_eq!(team_mask(i32::MIN), 0);

        assert_eq!(team_damage_scale(1, 1), 0.2);
        assert_eq!(team_damage_scale(1, 2), 1.0);

        assert_eq!(team_damage_scale(0, 0), 1.0);
    }

    #[test]
    fn hit_pack_roundtrip() {
        let packed = pack_hit_dword1(262, 1, 0);
        let unpacked = unpack_hit_dword1(packed);
        assert_eq!(unpacked.hit_key, 262);
        assert_eq!(unpacked.force_hit_flags, 1);

        assert_eq!(
            pack_hit_dword1(
                unpacked.hit_key,
                unpacked.force_hit_flags,
                unpacked.extra_nibble
            ),
            packed
        );
    }

    #[test]
    fn force_hit_break_guard() {
        let mut stats = StubStats::new();
        let mut opts = base_opts();
        opts.defender_in_guard = true;
        opts.force_hit_flags = Some(1);
        let result = resolve_hit(&mut stats, &opts);
        assert!(!result.guarded);
        assert_eq!(result.outcome, HitOutcome::BeHit);
    }

    #[test]
    fn pose_chip_allowlist_never_changes_guard_break_admission() {
        use crate::battle_rules::BattleRules;
        let mut opts = base_opts();
        opts.attack_power = Some(100);
        opts.pose_attack_percent = &|_| Some(100.0);
        opts.defender_in_guard = true;
        for (list, chip) in [(r#""all""#, 10), ("[262]", 10), ("[263]", 0), ("[]", 0)] {
            let json = format!(
                r#"{{"schemaVersion":2,"rebalance":{{"guardChipPercent":10,"guardChipPoses":{list},"poseOverrides":[]}}}}"#
            );
            let rules = BattleRules::from_json(&json).unwrap();
            opts.dword1 = None;
            opts.force_hit_flags = None;
            opts.attack_type = 0;
            let guard = resolve_hit_with_rules(&mut StubStats::new(), &opts, rules.clone());
            assert!(guard.guarded);
            assert_eq!(guard.damage, chip);
            for packed in [false, true] {
                opts.force_hit_flags = Some(1);
                opts.dword1 = packed.then(|| pack_hit_dword1(262, 1, 0));
                let hit = resolve_hit_with_rules(&mut StubStats::new(), &opts, rules.clone());
                assert!(!hit.guarded);
                assert_eq!(hit.damage, 100, "guard break must use full ordinary damage");
            }
            opts.dword1 = None;
            opts.force_hit_flags = None;
            opts.attack_type = 1;
            let original_break = resolve_hit_with_rules(&mut StubStats::new(), &opts, rules);
            assert!(!original_break.guarded);
            assert_eq!(original_break.damage, 100);
        }
    }

    #[test]
    fn pose_stun_overrides_apply_at_shared_resolution() {
        let rules = crate::battle_rules::BattleRules::from_json(r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":10,"guardChipPoses":"all","poseOverrides":[{"poseId":262,"hitStunMs":400,"guardStunMs":250}]}}"#).unwrap();
        let mut opts = base_opts();
        let hit = resolve_hit_with_rules(&mut StubStats::new(), &opts, rules.clone());
        assert_eq!(hit.target_value, 0.4);
        opts.defender_in_guard = true;
        let guard = resolve_hit_with_rules(&mut StubStats::new(), &opts, rules);
        assert_eq!(guard.defense_value, 0.25);
    }

    #[test]
    fn balance_chip_keeps_guard_anger_and_can_kill() {
        const BALANCED: crate::battle_rules::BattleRules =
            crate::battle_rules::BattleRules::BALANCED;
        let mut opts = base_opts();
        opts.pose_attack_percent = &|_| Some(100.0);
        opts.attack_power = Some(100);
        opts.defender_in_guard = true;
        let mut stats = StubStats::new();
        stats.hp = 7.0;
        let result = resolve_hit_with_rules(&mut stats, &opts, BALANCED);
        assert_eq!(result.damage, 7);
        assert!(result.guarded && result.killed);
        assert_eq!(result.outcome, HitOutcome::Guard);
        assert_eq!(result.anger_gain, 2);
        assert_eq!(stats.hp, 0.0);
    }

    #[test]
    fn balance_dagger_applies_after_defense_to_both_poses_only() {
        const BALANCED: crate::battle_rules::BattleRules =
            crate::battle_rules::BattleRules::BALANCED;
        let mut opts = base_opts();
        opts.pose_attack_percent = &|_| Some(100.0);
        opts.attack_power = Some(100);
        opts.defender_weapon_def = Some(15);
        opts.weapon_id = 5;
        for (pose, damage) in [(257, 51), (258, 51), (256, 85)] {
            opts.pose_id = pose;
            let result = resolve_hit_with_rules(&mut StubStats::new(), &opts, BALANCED);
            assert_eq!(result.damage, damage);
        }
        opts.pose_id = 258;
        opts.defender_in_guard = true;
        assert_eq!(
            resolve_hit_with_rules(&mut StubStats::new(), &opts, BALANCED).damage,
            6
        );
        opts.force_hit_flags = Some(1);
        let result = resolve_hit_with_rules(&mut StubStats::new(), &opts, BALANCED);
        assert!(!result.guarded);
        assert_eq!(result.damage, 51);
    }

    #[test]
    fn balance_chip_rounds_up_minimum_one_and_respects_invincibility() {
        const BALANCED: crate::battle_rules::BattleRules =
            crate::battle_rules::BattleRules::BALANCED;
        let mut opts = base_opts();
        opts.pose_attack_percent = &|_| Some(100.0);
        opts.attack_power = Some(9);
        opts.defender_in_guard = true;
        let result = resolve_hit_with_rules(&mut StubStats::new(), &opts, BALANCED);
        assert_eq!(result.damage, 1);
        assert!(!result.killed);
        opts.attack_power = Some(100);
        let mut stats = StubStats::new();
        stats.invincible = true;
        assert_eq!(
            resolve_hit_with_rules(&mut stats, &opts, BALANCED).damage,
            0
        );
    }

    #[test]
    fn guard_hit_zero_damage() {
        let mut stats = StubStats::new();
        let mut opts = base_opts();
        opts.defender_in_guard = true;
        let result = resolve_hit(&mut stats, &opts);
        assert!(result.guarded);
        assert_eq!(result.outcome, HitOutcome::Guard);
        assert_eq!(result.damage, 0);
        assert_eq!(stats.hp(), 1000.0);

        assert_eq!(result.anger_gain, 1);
        assert_eq!(stats.anger(), 1.0);
    }

    #[test]
    fn guard_anger_threshold_matches_signed_division_by_500() {
        assert_eq!(guard_anger_gain(-1), 0);
        assert_eq!(guard_anger_gain(35), 0);
        assert_eq!(guard_anger_gain(36), 1);
        assert_eq!(guard_anger_gain(1_000), 28);
    }

    #[test]
    fn behit_damage_and_anger() {
        let mut stats = StubStats::new();
        let result = resolve_hit(&mut stats, &base_opts());
        assert_eq!(result.outcome, HitOutcome::BeHit);

        assert_eq!(result.atk, 39);
        assert_eq!(result.raw, 39);
        assert_eq!(result.damage, 39);
        assert_eq!(stats.hp(), 961.0);

        assert_eq!(result.anger_gain, 5);
    }

    #[test]
    fn def_reduces_raw() {
        let mut stats = StubStats::new();
        stats.def = 15.0;
        let result = resolve_hit(&mut stats, &base_opts());
        assert_eq!(result.def, 15);
        assert_eq!(result.raw, 39 - 15);
    }

    #[test]
    fn teammate_guard_gives_no_anger() {
        let mut stats = StubStats::new();
        let mut opts = base_opts();
        opts.defender_in_guard = true;
        opts.attacker_team = 2;
        opts.defender_team = 2;
        let result = resolve_hit(&mut stats, &opts);
        assert!(result.guarded);
        assert_eq!(result.anger_gain, 0);
    }

    #[test]
    fn lethal_hit_kills() {
        let mut stats = StubStats::new();
        stats.hp = 10.0;
        let result = resolve_hit(&mut stats, &base_opts());
        assert!(result.killed);
        assert!(!stats.alive());
        assert_eq!(stats.hp(), 0.0);
    }

    #[test]
    fn none_when_no_payload_fields() {
        let mut stats = StubStats::new();
        let opts = HitOptions {
            attacker_team: 1,
            defender_team: 2,
            defender_in_guard: false,
            defender_weapon_def: None,
            weapon_id: 6,
            pose_id: 262,
            attack_power: None,
            attack_bonus: 0,
            hit_key: None,
            force_hit_flags: None,
            attack_type: 0,
            dword1: None,
            attack: None,
            pose_attack_percent: &sword_percent,
        };
        let result = resolve_hit(&mut stats, &opts);
        assert_eq!(result.outcome, HitOutcome::BeHit);
        assert_eq!(result.hit_key, 262);
    }
}
