use meteor_asset::res::HitStatusEffect;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusDelta {
    pub health: i32,
    pub anger: i32,
}

#[derive(Debug, Clone)]
pub struct ActiveHitEffect {
    pub definition: HitStatusEffect,
    pub expires_at: f64,
    next_tick: f64,
}

#[derive(Debug, Clone, Default)]
pub struct HitStatusEffects {
    pub active: Vec<ActiveHitEffect>,
}

impl HitStatusEffects {
    pub fn defense_bonus(&self) -> i32 {
        self.active
            .iter()
            .map(|effect| effect.definition.defense_bonus)
            .sum()
    }
    pub fn max_health_bonus(&self) -> i32 {
        self.active
            .iter()
            .map(|effect| effect.definition.max_health_bonus)
            .sum()
    }
    pub fn concealment_remaining(&self, now: f64) -> f32 {
        self.active
            .iter()
            .filter(|effect| effect.definition.concealed)
            .map(|effect| (effect.expires_at - now).max(0.0) as f32)
            .fold(0.0, f32::max)
    }

    pub fn attack_bonus(&self) -> i32 {
        self.active
            .iter()
            .map(|effect| effect.definition.attack_bonus)
            .sum()
    }

    pub fn apply(&mut self, definition: HitStatusEffect, now: f64) -> StatusDelta {
        if let Some(effect) = self
            .active
            .iter_mut()
            .find(|effect| effect.definition.id == definition.id && effect.expires_at > now)
        {
            effect.expires_at = now + f64::from(definition.duration);
            return StatusDelta::default();
        }
        self.active
            .retain(|effect| effect.definition.id != definition.id);
        if definition.duration > 0.0 {
            self.active.push(ActiveHitEffect {
                definition,
                expires_at: now + f64::from(definition.duration),
                next_tick: now + f64::from(definition.tick_every),
            });
        }
        StatusDelta {
            health: definition.health,
            anger: definition.anger,
        }
    }

    pub fn tick(&mut self, now: f64) -> StatusDelta {
        self.tick_with_damage(now).0
    }

    pub fn tick_with_damage(&mut self, now: f64) -> (StatusDelta, bool) {
        let mut damaging = false;
        let mut delta = StatusDelta::default();
        for effect in &mut self.active {
            if effect.definition.tick_every <= 0.0 {
                continue;
            }
            while effect.next_tick <= now && effect.next_tick < effect.expires_at {
                damaging |= effect.definition.health < 0;
                delta.health += effect.definition.health;
                delta.anger += effect.definition.anger;
                effect.next_tick += f64::from(effect.definition.tick_every);
            }
        }
        self.active.retain(|effect| effect.expires_at > now);
        (delta, damaging)
    }

    pub fn speed_factor(&self, item_factor: f32) -> f32 {
        (item_factor
            + self
                .active
                .iter()
                .filter_map(|effect| effect.definition.speed_percent)
                .map(|percent| percent as f32 / 100.0 - 1.0)
                .sum::<f32>())
        .max(0.0)
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_damage_flag_survives_simultaneous_healing_and_is_consumed_once() {
        let poison = table().status_effect(14).unwrap();
        let mut healing = poison;
        healing.id = 1000;
        healing.health = 20;
        healing.duration = 10.0;
        let mut effects = HitStatusEffects::default();
        effects.apply(poison, 0.0);
        effects.apply(healing, 0.0);
        let (delta, damage) = effects.tick_with_damage(5.0);
        assert_eq!(delta.health, 10);
        assert!(
            damage,
            "EXE tests individual negative segments before aggregating"
        );
        assert_eq!(
            effects.tick_with_damage(5.0),
            (StatusDelta::default(), false)
        );
        assert_eq!(effects.tick_with_damage(10.0).0.health, -10);
    }

    #[test]
    fn periodic_healing_and_expired_poison_do_not_request_hit() {
        let mut definition = table().status_effect(14).unwrap();
        definition.duration = 5.0;
        let mut effects = HitStatusEffects::default();
        effects.apply(definition, 0.0);
        assert_eq!(
            effects.tick_with_damage(5.0),
            (StatusDelta::default(), false)
        );
        definition.health = 10;
        definition.duration = 10.0;
        effects.apply(definition, 5.0);
        assert_eq!(
            effects.tick_with_damage(10.0),
            (
                StatusDelta {
                    health: 10,
                    anger: 0
                },
                false
            )
        );
    }

    #[test]
    fn fist_self_buff_uses_original_ninety_second_record_and_refreshes_without_stacking() {
        let definition = table().pose_self_status_effect(489).unwrap();
        assert_eq!(definition.id, 34);
        assert_eq!(definition.duration, 90.0);
        assert_eq!(definition.attack_bonus, 40);
        assert_eq!(definition.worn_effect, Some(739));
        let mut status = HitStatusEffects::default();
        status.apply(definition, 0.0);
        status.tick(89.99);
        assert_eq!(status.attack_bonus(), 40);
        status.apply(definition, 89.99);
        assert_eq!(status.active.len(), 1);
        assert_eq!(status.attack_bonus(), 40);
        status.tick(179.98);
        assert_eq!(status.attack_bonus(), 40);
        status.tick(179.99);
        assert_eq!(status.attack_bonus(), 0);
    }
    fn table() -> meteor_asset::res::CombatResourceTable {
        meteor_asset::res::parse_meteor_res(include_bytes!("../../../../origin_res/Meteor.res"))
            .unwrap()
            .combat_table()
    }

    #[test]
    fn original_pose_statuses_and_periodic_expiry() {
        let table = table();
        for (pose, id) in [
            (209, 10),
            (210, 11),
            (203, 12),
            (204, 12),
            (224, 13),
            (201, 22),
        ] {
            assert_eq!(table.pose_status_effect(pose).unwrap().id, id);
        }
        for pose in [200, 202, 218, 220, 222] {
            assert!(table.pose_status_effect(pose).is_none());
        }
        let mut statuses = HitStatusEffects::default();
        let poison = table.pose_status_effect(210).unwrap();
        assert_eq!(poison.duration, 15.0);
        assert_eq!(poison.tick_every, 3.0);
        assert_eq!(statuses.apply(poison, 0.0).health, -10);
        assert_eq!(statuses.tick(2.99).health, 0);
        assert_eq!(statuses.tick(3.0).health, -10);
        assert_eq!(statuses.tick(15.0).health, -30);
        assert!(statuses.active.is_empty());
        assert_eq!(statuses.apply(poison, 100.0).health, -10);
        assert_eq!(statuses.apply(poison, 101.0).health, 0);
        assert_eq!(
            statuses.tick(103.0).health,
            -10,
            "refresh must preserve the first tick deadline"
        );
        statuses = HitStatusEffects::default();
        let slow = table.pose_status_effect(224).unwrap();
        statuses.apply(slow, 0.0);
        assert!((statuses.speed_factor(1.0) - 0.3).abs() < 1e-6);
        statuses.apply(slow, 20.0);
        assert_eq!(statuses.active.len(), 1);
        statuses.tick(60.0);
        assert!((statuses.speed_factor(1.5) - 0.8).abs() < 1e-6);
        statuses.tick(80.0);
        assert_eq!(statuses.speed_factor(1.0), 1.0);
        assert_eq!(
            statuses
                .apply(table.pose_status_effect(201).unwrap(), 90.0)
                .anger,
            -20
        );
        assert!(statuses.active.is_empty());
    }
}
