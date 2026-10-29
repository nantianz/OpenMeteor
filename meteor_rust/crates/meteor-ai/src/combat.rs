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
    let yaw = x.atan2(z) + (errors[0] * 0.1).to_radians();
    let pitch = y.atan2(x.hypot(z)) + (errors[1] * 0.125).to_radians();
    let raw = [yaw.sin(), pitch.tan(), yaw.cos()];
    let length = (raw[0] * raw[0] + raw[1] * raw[1] + raw[2] * raw[2]).sqrt();
    raw.map(|v| v / length)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn ranged_aim_uses_original_heights_and_authored_accuracy() {
        let direction = ranged_aim(3, [0.0; 3], [0.0, 1.2, 10.0], 100.0, [0; 2]);
        assert!(direction[0].abs() < 1e-6 && direction[1].abs() < 1e-6 && direction[2] > 0.9999);
        assert!(ranged_aim(3, [0.0; 3], [0.0, 5.0, 10.0], 100.0, [0; 2])[1] > 0.0);
        let scattered = ranged_aim(3, [0.0; 3], [0.0, 1.2, 10.0], 0.0, [0; 2]);
        assert!(scattered[0] > 0.08 && scattered[1] > 0.10);
    }
}
