use crate::battle_rules::BattleRules;
use crate::hit_resolve::{CombatStats, HitOptions, HitOutcome, HitResult};

pub const ARMOR_START: u8 = 1;
pub const ARMOR_HIT: u8 = 2;
pub const INVALID_HIT: u8 = 3;

pub const STOCK_EFFECT_CONTEXT: u32 = 0x100;

#[derive(Debug, Clone, Default)]
pub struct DefensiveEffects {
    entry: Option<(u16, u32)>,
    armor: bool,
    first_hit: Option<(String, u16, u32, f64)>,
    immune_entry: Option<(String, u16, u32, f64)>,
}

impl DefensiveEffects {
    pub fn enter_pose(
        &mut self,
        rules: BattleRules,
        pose: u16,
        instance: u32,
        qi: &mut f32,
        qi_max: f32,
        alive: bool,
    ) -> bool {
        if !alive || !rules.enabled() {
            *self = Self::default();
            return false;
        }
        if self.entry == Some((pose, instance)) {
            return false;
        }
        self.entry = Some((pose, instance));
        self.armor = false;
        if let Some(effect) = rules.super_armor(pose) {
            let cost = qi_max * f32::from(effect.qi_cost_percent) / 100.0;
            if qi.is_finite() && qi_max.is_finite() && qi_max > 0.0 && *qi >= cost {
                *qi -= cost;
                self.armor = true;
            }
        }
        self.armor
    }

    pub fn armored(&self) -> bool {
        self.armor
    }
}

pub struct HitContext<'a> {
    pub attacker: &'a str,
    pub instance: u32,
    pub now_seconds: f64,
}

pub fn resolve_hit(
    stats: &mut impl CombatStats,
    opts: &HitOptions<'_>,
    rules: BattleRules,
    effects: &mut DefensiveEffects,
    context: HitContext<'_>,
) -> HitResult {
    if !rules.enabled() {
        *effects = DefensiveEffects::default();
        return crate::hit_resolve::resolve_hit_with_rules(stats, opts, rules);
    }
    let repeat_rule = rules.invalid_attack_key(opts.pose_id);
    let can_arm = stats.alive() && !stats.invincible();
    let flags = opts
        .dword1
        .map(crate::hit_resolve::unpack_hit_dword1)
        .map(|p| p.force_hit_flags)
        .or(opts.force_hit_flags)
        .unwrap_or_else(|| crate::hit_resolve::force_hit_flags_from_attack_type(opts.attack_type));
    let guarded = flags == 0 && opts.defender_in_guard;
    let matches = |hit: &(String, u16, u32, f64), key: u16, same_instance: bool, window_ms: u16| {
        hit.0 == context.attacker
            && hit.1 == key
            && ((hit.2 == context.instance) == same_instance)
            && context.now_seconds >= hit.3
            && context.now_seconds - hit.3 <= f64::from(window_ms) / 1000.0
    };
    let mut immune = false;
    if !guarded && stats.alive() && !stats.invincible() {
        if let Some((rule, key)) = repeat_rule {
            immune = effects
                .immune_entry
                .as_ref()
                .is_some_and(|h| matches(h, key, true, rule.window_ms));
            if !immune
                && effects
                    .first_hit
                    .as_ref()
                    .is_some_and(|h| matches(h, key, false, rule.window_ms))
            {
                let cost = stats.anger_max() * f32::from(rule.qi_cost_percent) / 100.0;
                if stats.try_spend_anger(cost) {
                    immune = true;
                    effects.first_hit = None;
                    effects.immune_entry = Some((
                        context.attacker.into(),
                        key,
                        context.instance,
                        context.now_seconds,
                    ));
                }
            }
        }
    }
    let mut proxy = DamageGate {
        inner: stats,
        immune,
    };
    let mut result = crate::hit_resolve::resolve_hit_with_rules(&mut proxy, opts, rules);
    if immune {
        result.rebalance_effect = INVALID_HIT;
        result.outcome = HitOutcome::None;
        result.target_move = 0.0;
        result.target_value = 0.0;
    } else if effects.armored() && !result.guarded && !result.killed {
        result.rebalance_effect = ARMOR_HIT;
        result.outcome = HitOutcome::None;
        result.target_move = 0.0;
        result.target_value = 0.0;
    }
    if result.killed {
        *effects = DefensiveEffects::default();
    } else if !immune {
        if can_arm
            && !result.guarded
            && repeat_rule.is_some_and(|(rule, _)| {
                result.damage > 0 || (rule.arm_on_hit && result.outcome == HitOutcome::BeHit)
            })
        {
            let key = repeat_rule.expect("checked above").1;

            let same = effects
                .first_hit
                .as_ref()
                .is_some_and(|h| h.0 == context.attacker && h.1 == key && h.2 == context.instance);
            if !same {
                effects.first_hit = Some((
                    context.attacker.into(),
                    key,
                    context.instance,
                    context.now_seconds,
                ));
            }
        } else {
            effects.first_hit = None;
        }
    }
    result
}

struct DamageGate<'a, S> {
    inner: &'a mut S,
    immune: bool,
}
impl<S: CombatStats> CombatStats for DamageGate<'_, S> {
    fn hp(&self) -> f32 {
        self.inner.hp()
    }
    fn alive(&self) -> bool {
        self.inner.alive()
    }
    fn invincible(&self) -> bool {
        self.inner.invincible()
    }
    fn anger(&self) -> f32 {
        self.inner.anger()
    }
    fn anger_max(&self) -> f32 {
        self.inner.anger_max()
    }
    fn def(&self) -> f32 {
        self.inner.def()
    }
    fn attack_bonus(&self) -> f32 {
        self.inner.attack_bonus()
    }
    fn take_damage(&mut self, amount: i32) -> i32 {
        if self.immune {
            0
        } else {
            self.inner.take_damage(amount)
        }
    }
    fn gain_anger_from_damage(&mut self, damage: i32) -> i32 {
        self.inner.gain_anger_from_damage(damage)
    }
    fn gain_anger_on_guard(&mut self, raw: i32) -> i32 {
        self.inner.gain_anger_on_guard(raw)
    }
    fn reset_spawn_clock(&mut self) {
        self.inner.reset_spawn_clock();
    }
}

#[cfg(test)]
mod tests;
