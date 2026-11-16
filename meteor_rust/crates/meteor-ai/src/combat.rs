use meteor_combat::combo_engine::Dir;
use meteor_combat::weapon_strategy::weapon_to_strategy;

#[derive(Debug)]
pub struct WeaponTactics {
    pub minimum_range: f32,
    pub attack_range: f32,
    pub aim_height: f32,
    pub angle_degrees: f32,
    pub ranged: bool,
    pub commands: [&'static [&'static str]; 3],
}

pub const TACTICS: [WeaponTactics; 12] = [
    WeaponTactics {
        minimum_range: 0.0,
        attack_range: 60.0,
        aim_height: 40.0,
        angle_degrees: 30.0,
        ranged: false,
        commands: [
            &[""],
            &["f", "b", "l", "r"],
            &["bb", "bf", "fb", "ff", "bff", "lr", "rl", "lrf"],
        ],
    },
    WeaponTactics {
        minimum_range: 0.0,
        attack_range: 60.0,
        aim_height: 40.0,
        angle_degrees: 30.0,
        ranged: false,
        commands: [&["", "f"], &["ff", "l", "r"], &["b", "bf", "bb", "bff"]],
    },
    WeaponTactics {
        minimum_range: 100.0,
        attack_range: 2000.0,
        aim_height: 30.0,
        angle_degrees: 20.0,
        ranged: true,
        commands: [&[""], &["f", "b"], &["bf", "bff"]],
    },
    WeaponTactics {
        minimum_range: 80.0,
        attack_range: 1000.0,
        aim_height: 50.0,
        angle_degrees: 20.0,
        ranged: true,
        commands: [&[""], &["f", "b"], &["bf", "bff"]],
    },
    WeaponTactics {
        minimum_range: 0.0,
        attack_range: 60.0,
        aim_height: 40.0,
        angle_degrees: 60.0,
        ranged: false,
        commands: [
            &["", "f", "b"],
            &["bf", "ff", "bb"],
            &["lr", "rl", "lrb", "lrf", "bff", "bbf"],
        ],
    },
    WeaponTactics {
        minimum_range: 0.0,
        attack_range: 60.0,
        aim_height: 40.0,
        angle_degrees: 60.0,
        ranged: false,
        commands: [
            &[""],
            &["f", "b", "l", "r"],
            &["ff", "bf", "bb", "lr", "lrf", "bbf", "lrb"],
        ],
    },
    WeaponTactics {
        minimum_range: 80.0,
        attack_range: 300.0,
        aim_height: 40.0,
        angle_degrees: 20.0,
        ranged: true,
        commands: [&[""], &["f", "b"], &["bf", "bff"]],
    },
    WeaponTactics {
        minimum_range: 0.0,
        attack_range: 60.0,
        aim_height: 40.0,
        angle_degrees: 50.0,
        ranged: false,
        commands: [
            &[""],
            &["b", "f", "rl", "bb"],
            &["bf", "ff", "lr", "bbf", "lrf", "fff", "lrb", "bff"],
        ],
    },
    WeaponTactics {
        minimum_range: 0.0,
        attack_range: 60.0,
        aim_height: 40.0,
        angle_degrees: 50.0,
        ranged: false,
        commands: [
            &["", "f"],
            &["r", "l", "lr"],
            &["bf", "b", "ff", "bbf", "lrf", "blr"],
        ],
    },
    WeaponTactics {
        minimum_range: 0.0,
        attack_range: 65.0,
        aim_height: 40.0,
        angle_degrees: 50.0,
        ranged: false,
        commands: [
            &["", "f", "b", "l", "r", "bb", "ff"],
            &["ff", "bb", "lrf", "lrb"],
            &["bf", "fb", "bbf"],
        ],
    },
    WeaponTactics {
        minimum_range: 0.0,
        attack_range: 55.0,
        aim_height: 40.0,
        angle_degrees: 50.0,
        ranged: false,
        commands: [
            &[""],
            &["b", "lr", "ll", "rr", "bb"],
            &["lrf", "bf", "ff", "fb", "bbf", "lrb", "bff"],
        ],
    },
    WeaponTactics {
        minimum_range: 0.0,
        attack_range: 60.0,
        aim_height: 40.0,
        angle_degrees: 50.0,
        ranged: false,
        commands: [
            &[""],
            &["f", "b", "fb", "ff", "ll", "rr", "bb", "lr"],
            &["bbf", "bf", "lrf", "lrfb", "lrb", "bff"],
        ],
    },
];

pub fn weapon_tactics(weapon: u8) -> &'static WeaponTactics {
    &TACTICS[weapon_to_strategy(weapon) as usize]
}

pub fn decision_interval(think: f32, callback_delay: f32) -> f32 {
    callback_delay / (think * 0.01).max(0.1)
}

pub fn wants_weapon_switch(
    current: u8,
    spare: u8,
    distance: f32,
    height: f32,
    elapsed_since_switch: f64,
    mut roll: impl FnMut() -> u32,
) -> bool {
    if current == 0 || spare == 0 || !distance.is_finite() || !height.is_finite() {
        return false;
    }
    let current = weapon_tactics(current);
    let alternate = weapon_tactics(spare);
    let in_range =
        |t: &WeaponTactics| distance < t.attack_range && (t.ranged || height.abs() < 80.0);
    if !in_range(current) {
        return elapsed_since_switch > 20.0 && in_range(alternate);
    }
    if distance < current.minimum_range {
        return distance >= alternate.minimum_range && in_range(alternate);
    }

    current.ranged && weapon_to_strategy(spare) > 0 && roll() % 20 == 0
}

#[cfg(test)]
mod weapon_switch_tests {
    use super::*;
    #[test]
    fn stock_distance_switch_boundaries_and_random_sword_exception() {
        let wants = |current, spare, distance, height, elapsed, roll| {
            wants_weapon_switch(current, spare, distance, height, elapsed, || roll)
        };
        assert!(wants(3, 6, 59.9, 0.0, 0.0, 1));
        assert!(!wants(3, 6, 60.0, 0.0, 100.0, 0));
        assert!(!wants(3, 0, 10.0, 0.0, 100.0, 0));
        assert!(!wants(6, 3, 120.0, 0.0, 20.0, 1));
        assert!(wants(6, 3, 120.0, 0.0, 20.001, 1));
        assert!(!wants(3, 6, 150.0, 0.0, 100.0, 0));
        assert!(wants(3, 5, 150.0, 0.0, 0.0, 0));
        assert!(!wants(3, 5, 150.0, 0.0, 0.0, 1));
        assert!(!wants(3, 2, 50.0, 0.0, 100.0, 0));
        assert!(!wants(3, 6, f32::NAN, 0.0, 100.0, 0));
    }
}

pub fn attack_or_guard(guard: f32, attacks: [f32; 3], roll: u32) -> Option<bool> {
    let roll = (roll % 100) as f32;
    if roll < guard {
        Some(false)
    } else if roll < guard + attacks.iter().sum::<f32>() {
        Some(true)
    } else {
        None
    }
}

pub fn attack_category(attacks: [f32; 3], roll: u32) -> usize {
    let roll = (roll % 100) as f32;
    if roll < attacks[0] {
        0
    } else if roll < attacks[0] + attacks[1] {
        1
    } else {
        2
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommandMemory {
    category: usize,
    index: usize,
}

impl CommandMemory {
    pub fn choose(
        &mut self,
        tactics: &WeaponTactics,
        category: usize,
        reuse: bool,
        roll: u32,
    ) -> &'static str {
        let category = if reuse {
            self.category
        } else {
            category.min(2)
        };
        let group = tactics.commands[category];
        let index = if reuse {
            self.index.min(group.len() - 1)
        } else {
            roll as usize % group.len()
        };
        self.category = category;
        self.index = index;
        group[index]
    }
}

pub const AIR_COMMANDS: [&str; 17] = [
    "", "", "", "", "", "", "", "b", "fb", "b", "fb", "b", "bf", "bb", "ff", "ll", "rr",
];

pub fn directions(command: &str) -> Vec<Dir> {
    command
        .bytes()
        .map(|key| match key {
            b'f' => Dir::Up,
            b'b' => Dir::Down,
            b'l' => Dir::Left,
            b'r' => Dir::Right,
            _ => unreachable!("validated original NPC command table"),
        })
        .collect()
}

pub fn reacts_with_qi_burst(damaged: bool, qi: f32, distance: f32, roll: u32) -> bool {
    damaged && qi > 60.0 && distance < 80.0 * meteor_core::MODEL_SCALE && roll.is_multiple_of(5)
}

pub fn ranged_aim(
    weapon: u8,
    from: [f32; 3],
    target: [f32; 3],
    facing: f32,
    aim: f32,
    rolls: [u32; 2],
) -> [f32; 3] {
    let x = target[0] - from[0];
    let y =
        target[1] - from[1] + (20.0 - weapon_tactics(weapon).aim_height) * meteor_core::MODEL_SCALE;
    let z = target[2] - from[2];
    let spread = (100 - aim as i32).max(0) as u32;
    let errors = if spread == 0 {
        [0.0; 2]
    } else {
        [
            (spread / 2) as f32 - (rolls[0] % spread) as f32,
            (spread / 2) as f32 - (rolls[1] % spread) as f32,
        ]
    };
    let yaw = facing + (errors[0] * 0.1).to_radians();
    let pitch = y.atan2(x.hypot(z)) + (errors[1] * 0.125).to_radians();
    let raw = [yaw.sin(), pitch.tan(), yaw.cos()];
    let length = (raw[0] * raw[0] + raw[1] * raw[1] + raw[2] * raw[2]).sqrt();
    raw.map(|v| v / length)
}

pub fn in_attack_range(weapon: u8, delta: [f32; 3], ground_controllable: bool) -> bool {
    let tactics = weapon_tactics(weapon);
    let range = tactics.attack_range * meteor_core::MODEL_SCALE;
    let [x, y, z] = delta;
    if tactics.ranged || ground_controllable {
        (tactics.ranged || y.abs() <= 80.0 * meteor_core::MODEL_SCALE)
            && x * x + y * y + z * z < range * range
    } else {
        x * x + z * z < 3.0 * range * range
    }
}

pub fn point_aim(weapon: u8, from: [f32; 3], mut target: [f32; 3], facing: f32) -> [f32; 3] {
    target[1] -= 20.0 * meteor_core::MODEL_SCALE;
    ranged_aim(weapon, from, target, facing, 100.0, [0, 0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attack_range_obeys_pose_height_and_strict_boundaries() {
        let s = meteor_core::MODEL_SCALE;
        assert!(!in_attack_range(6, [60.0 * s, 0.0, 0.0], true));
        assert!(in_attack_range(6, [59.0 * s, 0.0, 0.0], true));
        assert!(!in_attack_range(6, [0.0, 61.0 * s, 0.0], true));
        assert!(in_attack_range(6, [100.0 * s, 500.0 * s, 0.0], false));
        assert!(!in_attack_range(6, [104.0 * s, 0.0, 0.0], false));
        assert!(!in_attack_range(3, [0.0, 2001.0 * s, 0.0], false));
        assert!(in_attack_range(3, [0.0, 1999.0 * s, 0.0], false));
    }

    #[test]
    fn original_think_and_qi_thresholds() {
        assert_eq!(decision_interval(100.0, 0.2), 0.2);
        assert_eq!(decision_interval(50.0, 0.2), 0.4);
        assert_eq!(decision_interval(0.0, 0.2), 2.0);
        assert!(!reacts_with_qi_burst(false, 100.0, 3.0, 0));
        assert!(!reacts_with_qi_burst(true, 60.0, 3.0, 0));
        assert!(reacts_with_qi_burst(true, 61.0, 3.0, 0));
        assert!(!reacts_with_qi_burst(
            true,
            100.0,
            80.0 * meteor_core::MODEL_SCALE,
            0
        ));
        assert!(!reacts_with_qi_burst(true, 100.0, 3.0, 1));
    }

    #[test]
    fn original_categories_include_directional_and_anger_commands_for_every_weapon() {
        for weapon in 1..=12 {
            let tactics = weapon_tactics(weapon);
            assert!(tactics.commands.iter().all(|group| !group.is_empty()));
            assert!(tactics.commands[2]
                .iter()
                .all(|command| !command.is_empty()));
            let mut paid = false;
            for command in tactics.commands.iter().flat_map(|group| group.iter()) {
                let sequence = directions(command);
                if let Some(resolved) =
                    meteor_combat::combo_engine::resolve_affordable_weapon_command(
                        weapon,
                        &sequence,
                        false,
                        0,
                        |_| true,
                    )
                {
                    paid |= resolved.anger_cost > 0;
                }
            }
            assert!(
                paid,
                "original weapon {weapon} table must reach an anger move"
            );
        }
    }

    #[test]
    fn command_memory_is_per_actor_and_survives_rejected_requests() {
        let tactics = weapon_tactics(6);
        let mut a = CommandMemory::default();
        let mut b = CommandMemory::default();
        assert_eq!(a.choose(tactics, 2, false, 1), "bf");
        assert_eq!(a.choose(tactics, 0, true, 0), "bf");
        assert_eq!(b.choose(tactics, 2, true, 0), "");
    }

    #[test]
    fn ranged_aim_uses_actor_yaw_even_at_perfect_accuracy() {
        for weapon in [1, 2, 3] {
            for facing in [0.0, 0.7, -2.4] {
                let direction =
                    ranged_aim(weapon, [0.0; 3], [10.0, 5.0, -10.0], facing, 100.0, [0; 2]);
                let yaw = direction[0].atan2(direction[2]);
                assert!((yaw - facing).abs() < 1e-6);
                assert!(direction[1] > 0.0, "target still determines pitch");
            }
        }
    }

    #[test]
    fn ranged_aim_uses_original_heights_and_authored_accuracy() {
        let direction = ranged_aim(3, [0.0; 3], [0.0, 1.2, 10.0], 0.0, 100.0, [0; 2]);
        assert!(direction[0].abs() < 1e-6 && direction[1].abs() < 1e-6 && direction[2] > 0.9999);
        assert!(ranged_aim(3, [0.0; 3], [0.0, 5.0, 10.0], 0.0, 100.0, [0; 2])[1] > 0.0);
        let scattered = ranged_aim(3, [0.0; 3], [0.0, 1.2, 10.0], 0.0, 0.0, [0; 2]);
        assert!(scattered[0] > 0.08 && scattered[1] > 0.10);
    }
}
