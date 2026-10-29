use std::collections::HashMap;

pub const RESPAWN_DELAY_SECONDS: f32 = 3.0;

pub const NO_RESPAWN_RULES: [i32; 3] = [4, 5, 6];

pub fn can_respawn(rule: Option<i32>) -> bool {
    match rule {
        None => true,
        Some(value) => !NO_RESPAWN_RULES.contains(&value),
    }
}

#[derive(Debug, Clone)]
pub struct DeathSystem {
    respawn_seconds: f32,

    rule: Option<i32>,

    allow_respawn: bool,

    timers: HashMap<u64, f32>,
}

impl DeathSystem {
    pub fn new(rule: Option<i32>, respawn_seconds: Option<f32>) -> Self {
        let respawn_seconds = match respawn_seconds {
            Some(v) if v > 0.0 => v,
            _ => RESPAWN_DELAY_SECONDS,
        };
        let allow_respawn = can_respawn(rule);
        Self {
            respawn_seconds,
            rule,
            allow_respawn,
            timers: HashMap::new(),
        }
    }

    pub fn set_rule(&mut self, rule: Option<i32>) -> bool {
        self.rule = rule;
        self.allow_respawn = can_respawn(rule);
        self.allow_respawn
    }

    pub fn time_since_death(&self, fighter_id: u64) -> f32 {
        self.timers.get(&fighter_id).copied().unwrap_or(0.0)
    }

    pub fn forget(&mut self, fighter_id: u64) {
        self.timers.remove(&fighter_id);
    }

    pub fn reset(&mut self) {
        self.timers.clear();
    }

    pub fn update(&mut self, dt: f32, fighters: impl IntoIterator<Item = (u64, bool)>) -> Vec<u64> {
        let mut due = Vec::new();
        if dt.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return due;
        }
        for (fighter_id, alive) in fighters {
            if alive {
                self.timers.remove(&fighter_id);
                continue;
            }
            let elapsed = self.timers.get(&fighter_id).copied().unwrap_or(0.0) + dt;
            self.timers.insert(fighter_id, elapsed);

            if self.allow_respawn && elapsed > self.respawn_seconds {
                self.timers.remove(&fighter_id);
                due.push(fighter_id);
            }
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn respawn_rules() {
        assert!(can_respawn(None));
        assert!(can_respawn(Some(1)));
        assert!(can_respawn(Some(2)));
        assert!(can_respawn(Some(3)));
        assert!(can_respawn(Some(10)));

        assert!(!can_respawn(Some(4)));
        assert!(!can_respawn(Some(5)));
        assert!(!can_respawn(Some(6)));
    }

    #[test]
    fn default_respawn_seconds() {
        let sys = DeathSystem::new(None, None);
        assert_eq!(sys.respawn_seconds, RESPAWN_DELAY_SECONDS);

        let sys = DeathSystem::new(None, Some(-1.0));
        assert_eq!(sys.respawn_seconds, RESPAWN_DELAY_SECONDS);

        let sys = DeathSystem::new(None, Some(5.0));
        assert_eq!(sys.respawn_seconds, 5.0);
    }

    #[test]
    fn time_since_death_zero_before() {
        let sys = DeathSystem::new(None, None);
        assert_eq!(sys.time_since_death(1), 0.0);
    }

    #[test]
    fn alive_clears_timer() {
        let mut sys = DeathSystem::new(None, None);

        let due = sys.update(1.0, [(1, false)]);
        assert!(due.is_empty());
        assert_eq!(sys.time_since_death(1), 1.0);

        let due = sys.update(1.0, [(1, true)]);
        assert!(due.is_empty());
        assert_eq!(sys.time_since_death(1), 0.0);
    }

    #[test]
    fn due_after_threshold() {
        let mut sys = DeathSystem::new(None, Some(3.0));

        let due = sys.update(2.9, [(7, false)]);
        assert!(due.is_empty());

        let due = sys.update(0.2, [(7, false)]);
        assert_eq!(due, vec![7]);

        assert_eq!(sys.time_since_death(7), 0.0);
    }

    #[test]
    fn no_respawn_rule_never_due() {
        let mut sys = DeathSystem::new(Some(5), Some(1.0));
        for _ in 0..10 {
            let due = sys.update(0.5, [(1, false)]);
            assert!(due.is_empty());
        }
    }

    #[test]
    fn forget_and_reset() {
        let mut sys = DeathSystem::new(None, None);
        sys.update(1.0, [(1, false)]);
        sys.forget(1);
        assert_eq!(sys.time_since_death(1), 0.0);
        sys.update(1.0, [(2, false)]);
        sys.reset();
        assert_eq!(sys.time_since_death(2), 0.0);
    }
}
