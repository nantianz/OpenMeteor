use super::*;

fn effect_rules() -> BattleRules {
    BattleRules::from_json(r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":10,"guardChipPoses":"all","poseOverrides":[{"poseId":282,"invalidAttack":{"qiCostPercent":0,"windowMs":1000}},{"poseId":302,"superArmor":{"qiCostPercent":20}}]}}"#).unwrap()
}

struct Stats {
    hp: f32,
    qi: f32,
    invincible: bool,
}
impl Default for Stats {
    fn default() -> Self {
        Self {
            hp: 1000.0,
            qi: 100.0,
            invincible: false,
        }
    }
}
impl CombatStats for Stats {
    fn hp(&self) -> f32 {
        self.hp
    }
    fn alive(&self) -> bool {
        self.hp > 0.0
    }
    fn invincible(&self) -> bool {
        self.invincible
    }
    fn anger(&self) -> f32 {
        self.qi
    }
    fn anger_max(&self) -> f32 {
        100.0
    }
    fn def(&self) -> f32 {
        0.0
    }
    fn attack_bonus(&self) -> f32 {
        0.0
    }
    fn take_damage(&mut self, amount: i32) -> i32 {
        if self.invincible {
            return 0;
        }
        let loss = amount.min(self.hp as i32).max(0);
        self.hp -= loss as f32;
        loss
    }
    fn gain_anger_from_damage(&mut self, damage: i32) -> i32 {
        let gain = damage * 14 / 100;
        self.qi = (self.qi + gain as f32).min(100.0);
        gain
    }
    fn gain_anger_on_guard(&mut self, _: i32) -> i32 {
        0
    }
    fn reset_spawn_clock(&mut self) {}
    fn try_spend_anger(&mut self, cost: f32) -> bool {
        if self.qi < cost {
            false
        } else {
            self.qi -= cost;
            true
        }
    }
}
fn hit(
    stats: &mut Stats,
    effects: &mut DefensiveEffects,
    rules: BattleRules,
    source: &str,
    instance: u32,
    guard: bool,
    power: i32,
    now: f64,
) -> HitResult {
    hit_pose(
        stats, effects, rules, 282, source, instance, guard, power, now,
    )
}

fn hit_pose(
    stats: &mut Stats,
    effects: &mut DefensiveEffects,
    rules: BattleRules,
    pose: u16,
    source: &str,
    instance: u32,
    guard: bool,
    power: i32,
    now: f64,
) -> HitResult {
    resolve_hit(
        stats,
        &HitOptions {
            attacker_team: 1,
            defender_team: 2,
            defender_in_guard: guard,
            defender_weapon_def: Some(0),
            weapon_id: 7,
            pose_id: pose,
            attack_power: Some(power),
            attack_bonus: 0,
            hit_key: None,
            force_hit_flags: None,
            attack_type: 0,
            dword1: None,
            attack: None,
            pose_attack_percent: &|_| Some(if power == 0 { 0.0 } else { 100.0 }),
        },
        rules,
        effects,
        HitContext {
            attacker: source,
            instance,
            now_seconds: now,
        },
    )
}

#[test]
fn spear_requires_actual_non_guard_damage_from_previous_action() {
    let rules = effect_rules();
    for (guard, power, invincible) in [(true, 100, false), (false, 0, false), (false, 100, true)] {
        let mut stats = Stats {
            invincible,
            ..Default::default()
        };
        let mut effects = DefensiveEffects::default();
        let first = hit(
            &mut stats,
            &mut effects,
            rules.clone(),
            "a",
            1,
            guard,
            power,
            0.0,
        );
        if guard {
            assert!(first.damage > 0, "guard chip must not arm protection");
        }
        stats.invincible = false;
        let second = hit(
            &mut stats,
            &mut effects,
            rules.clone(),
            "a",
            2,
            false,
            100,
            1.0,
        );
        assert_eq!(second.damage, 100);
        assert_eq!(second.rebalance_effect, 0);
    }
}

#[test]
fn second_stab_is_free_and_same_action_windows_do_not_count() {
    let mut stats = Stats {
        qi: 0.0,
        ..Default::default()
    };
    let mut effects = DefensiveEffects::default();
    let rules = effect_rules();
    assert_eq!(
        hit(
            &mut stats,
            &mut effects,
            rules.clone(),
            "a",
            1,
            false,
            100,
            0.0
        )
        .damage,
        100
    );
    assert_eq!(
        hit(
            &mut stats,
            &mut effects,
            rules.clone(),
            "a",
            1,
            false,
            100,
            0.3
        )
        .damage,
        100
    );
    stats.qi = 0.0;
    let second = hit(
        &mut stats,
        &mut effects,
        rules.clone(),
        "a",
        2,
        false,
        100,
        1.0,
    );
    assert_eq!(second.rebalance_effect, INVALID_HIT);
    assert_eq!(second.damage, 0);
    assert_eq!(second.outcome, HitOutcome::None);
    assert_eq!(stats.qi, 0.0);
    assert_eq!(
        hit(
            &mut stats,
            &mut effects,
            rules.clone(),
            "a",
            2,
            false,
            100,
            1.3
        )
        .damage,
        0
    );
    assert_eq!(
        hit(&mut stats, &mut effects, rules, "a", 3, false, 100, 2.0).damage,
        100
    );
}

#[test]
fn protection_is_target_source_and_time_scoped_and_original_is_unchanged() {
    for (source, second_time, rules) in [
        ("b", 1.0, effect_rules()),
        ("a", 1.001, effect_rules()),
        ("a", 1.0, BattleRules::ORIGINAL),
    ] {
        let mut stats = Stats::default();
        let mut effects = DefensiveEffects::default();
        hit(
            &mut stats,
            &mut effects,
            rules.clone(),
            "a",
            1,
            false,
            100,
            0.0,
        );
        assert_eq!(
            hit(
                &mut stats,
                &mut effects,
                rules,
                source,
                2,
                false,
                100,
                second_time
            )
            .damage,
            100
        );
    }
    let mut stats = Stats::default();
    let mut other_target = DefensiveEffects::default();
    assert_eq!(
        hit(
            &mut stats,
            &mut other_target,
            effect_rules(),
            "a",
            2,
            false,
            100,
            1.0
        )
        .damage,
        100
    );
}

#[test]
fn armor_spends_once_preserves_damage_and_never_prevents_death() {
    let rules = effect_rules();
    let mut stats = Stats::default();
    let mut effects = DefensiveEffects::default();
    assert!(effects.enter_pose(rules.clone(), 302, 1, &mut stats.qi, 100.0, true));
    assert_eq!(stats.qi, 80.0);
    assert!(!effects.enter_pose(rules.clone(), 302, 1, &mut stats.qi, 100.0, true));
    assert_eq!(stats.qi, 80.0);
    let result = hit(
        &mut stats,
        &mut effects,
        rules.clone(),
        "a",
        1,
        false,
        100,
        0.0,
    );
    assert_eq!(result.rebalance_effect, ARMOR_HIT);
    assert_eq!(stats.hp, 900.0);
    assert_eq!(stats.qi, 94.0, "normal damage anger is preserved");
    let result = hit(
        &mut stats,
        &mut effects,
        rules.clone(),
        "b",
        2,
        false,
        1000,
        1.0,
    );
    assert!(result.killed);
    assert_eq!(result.rebalance_effect, 0);
    assert!(!effects.armored());
    stats.qi = 19.0;
    assert!(!effects.enter_pose(rules.clone(), 302, 3, &mut stats.qi, 100.0, true));
    assert_eq!(stats.qi, 19.0);
    stats.qi = 20.0;
    assert!(
        !effects.enter_pose(rules.clone(), 302, 3, &mut stats.qi, 100.0, true),
        "no mid-pose activation"
    );
    assert!(effects.enter_pose(rules.clone(), 302, 4, &mut stats.qi, 100.0, true));
    effects.enter_pose(rules, 0, 5, &mut stats.qi, 100.0, true);
    assert!(!effects.armored());
}

#[test]
fn optional_effect_costs_default_to_twenty_and_are_validated() {
    let json = r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":10,"guardChipPoses":"all","poseOverrides":[{"poseId":282,"invalidAttack":{}},{"poseId":302,"superArmor":{}}]}}"#;
    let rules = BattleRules::from_json(json).unwrap();
    assert_eq!(rules.super_armor(302).unwrap().qi_cost_percent, 20);
    assert_eq!(rules.invalid_attack(282).unwrap().qi_cost_percent, 20);
    assert_eq!(rules.invalid_attack(282).unwrap().window_ms, 1000);
    assert_eq!(effect_rules().invalid_attack(282).unwrap().window_ms, 1000);
    assert_eq!(BattleRules::from_json(&rules.to_json()).unwrap(), rules);
    let mut stats = Stats::default();
    let mut effects = DefensiveEffects::default();
    hit(
        &mut stats,
        &mut effects,
        rules.clone(),
        "a",
        1,
        false,
        100,
        0.0,
    );
    stats.qi = 19.0;
    assert_eq!(
        hit(
            &mut stats,
            &mut effects,
            rules.clone(),
            "a",
            2,
            false,
            100,
            1.0
        )
        .damage,
        100
    );
    stats.qi = 20.0;
    assert_eq!(
        hit(&mut stats, &mut effects, rules, "a", 3, false, 100, 2.0).damage,
        0
    );
    assert_eq!(stats.qi, 0.0);
    for property in [
        r#""qiCostPercent":101"#,
        r#""qiCostPercent":-1"#,
        r#""windowMs":0"#,
        r#""unknown":1"#,
    ] {
        assert!(BattleRules::from_json(&json.replace(
            "\"invalidAttack\":{}",
            &format!("\"invalidAttack\":{{{property}}}")
        ))
        .is_err());
    }
}

#[test]
fn fist_group_protects_cross_pose_and_zero_damage_pushes() {
    let rules = BattleRules::from_json(r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":0,"guardChipPoses":[],"poseOverrides":[{"poseId":407,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}},{"poseId":411,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}},{"poseId":412,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}},{"poseId":413,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}}]}}"#).unwrap();
    for followup in [411, 412, 413] {
        let mut stats = Stats::default();
        let mut effects = DefensiveEffects::default();
        assert_eq!(
            hit_pose(
                &mut stats,
                &mut effects,
                rules.clone(),
                407,
                "a",
                1,
                false,
                100,
                0.0
            )
            .damage,
            100
        );
        let protected = hit_pose(
            &mut stats,
            &mut effects,
            rules.clone(),
            followup,
            "a",
            2,
            false,
            0,
            0.4,
        );
        assert_eq!(protected.rebalance_effect, INVALID_HIT);
        assert_eq!(protected.outcome, HitOutcome::None);
    }
    let mut stats = Stats::default();
    let mut effects = DefensiveEffects::default();
    let first = hit_pose(
        &mut stats,
        &mut effects,
        rules.clone(),
        412,
        "a",
        1,
        false,
        0,
        0.0,
    );
    assert_eq!(first.damage, 0);
    assert_eq!(first.outcome, HitOutcome::BeHit);
    assert_eq!(
        hit_pose(
            &mut stats,
            &mut effects,
            rules.clone(),
            413,
            "a",
            1,
            false,
            0,
            0.2
        )
        .rebalance_effect,
        0,
        "a linked hit in the same action instance is not a second attack"
    );
    assert_eq!(
        hit_pose(
            &mut stats,
            &mut effects,
            rules.clone(),
            413,
            "a",
            2,
            false,
            0,
            0.4
        )
        .rebalance_effect,
        INVALID_HIT
    );
    assert_eq!(
        hit_pose(
            &mut stats,
            &mut effects,
            rules.clone(),
            412,
            "a",
            3,
            false,
            0,
            0.6
        )
        .rebalance_effect,
        0,
        "a protected hit does not arm permanent immunity"
    );
    assert_eq!(
        hit_pose(&mut stats, &mut effects, rules, 413, "a", 4, false, 0, 0.8).rebalance_effect,
        INVALID_HIT
    );
}

#[test]
fn grouped_zero_damage_requires_a_real_unguarded_hit() {
    let json = r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":0,"guardChipPoses":[],"poseOverrides":[{"poseId":412,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}},{"poseId":413,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}}]}}"#;
    let rules = BattleRules::from_json(json).unwrap();
    for (guard, invincible, source, time) in [
        (true, false, "a", 0.4),
        (false, true, "a", 0.4),
        (false, false, "b", 0.4),
        (false, false, "a", 0.801),
    ] {
        let mut stats = Stats {
            invincible,
            ..Default::default()
        };
        let mut effects = DefensiveEffects::default();
        hit_pose(
            &mut stats,
            &mut effects,
            rules.clone(),
            412,
            "a",
            1,
            guard,
            0,
            0.0,
        );
        stats.invincible = false;
        assert_eq!(
            hit_pose(
                &mut stats,
                &mut effects,
                rules.clone(),
                413,
                source,
                2,
                false,
                0,
                time
            )
            .rebalance_effect,
            0
        );
    }
    let damage_only =
        BattleRules::from_json(&json.replace("\"armOnHit\":true", "\"armOnHit\":false")).unwrap();
    let mut stats = Stats::default();
    let mut effects = DefensiveEffects::default();
    hit_pose(
        &mut stats,
        &mut effects,
        damage_only.clone(),
        412,
        "a",
        1,
        false,
        0,
        0.0,
    );
    assert_eq!(
        hit_pose(
            &mut stats,
            &mut effects,
            damage_only,
            413,
            "a",
            2,
            false,
            0,
            0.4
        )
        .rebalance_effect,
        0
    );
}
