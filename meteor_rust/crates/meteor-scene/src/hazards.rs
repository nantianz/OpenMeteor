use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct HazardVolume {
    pub name: String,

    pub damage: i32,
    pub box_: Aabb,

    pub period: f32,
}

#[derive(Debug, Clone)]
pub struct HazardMachine {
    pub name: String,
    pub damage: bool,
    pub damage_value: i32,
    pub active: bool,
    pub boxes: Vec<Aabb>,
}

#[derive(Debug, Clone, Default)]
pub struct StageHazards {
    pub volumes: Vec<HazardVolume>,
    pub machines: Vec<HazardMachine>,

    cooldowns: HashMap<String, f32>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HazardTickStats {
    pub volume_hits: u32,
    pub machine_hits: u32,
}

pub fn build_volumes(
    mesh: &meteor_asset::gmdl::GmdlMesh,
    desc: &meteor_asset::des::StageDescription,
    center: [f32; 3],
) -> Vec<HazardVolume> {
    desc.scene_objects
        .iter()
        .filter_map(|object| {
            if !object.is_hell && object.damage_name == 0 {
                return None;
            }
            let part = mesh.objects.iter().find(|part| part.name == object.name)?;
            Some(HazardVolume {
                name: object.name.clone(),
                damage: if object.is_hell {
                    100000
                } else {
                    object.damage_name as i32
                },
                period: if object.is_hell { 0.1 } else { 1.0 },
                box_: StageHazards::gmdl_object_world_aabb(&part.positions, center, 0.12)?,
            })
        })
        .collect()
}

impl StageHazards {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse_damage_name(custom: &str) -> i32 {
        for line in custom.lines() {
            let l = line.trim();

            let lower = l.to_lowercase();
            if let Some(idx) = lower.find("name") {
                let after = &l[idx + 4..];
                if let Some(eq) = after.find('=') {
                    let value = after[eq + 1..].trim().trim_matches('"').trim();
                    if let Some(rest) = value.strip_prefix("damage") {
                        if let Ok(n) = rest.parse::<i32>() {
                            return n;
                        }
                    }
                }
            }
        }
        0
    }

    pub fn is_hell(custom: &str, is_hell_flag: bool) -> bool {
        if is_hell_flag {
            return true;
        }
        let lower = custom.to_lowercase();
        lower.contains("hell")
    }

    pub fn build_from_stage(&mut self, volumes: Vec<HazardVolume>, machines: Vec<HazardMachine>) {
        self.volumes = volumes;
        self.machines = machines;
        self.cooldowns.clear();
    }

    pub fn tick_cooldowns(&mut self, dt: f32) {
        if dt.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return;
        }
        let mut expired = Vec::new();
        for (key, left) in &mut self.cooldowns {
            *left -= dt;
            if *left < 0.0 {
                expired.push(key.clone());
            }
        }
        for key in expired {
            self.cooldowns.remove(&key);
        }
    }

    fn ready(&mut self, key: &str, period: f32) -> bool {
        if self.cooldowns.contains_key(key) {
            return false;
        }
        self.cooldowns.insert(key.to_string(), period);
        true
    }

    pub fn point_in_aabb(p: [f32; 3], box_: &Aabb) -> bool {
        p[0] >= box_.min[0]
            && p[0] <= box_.max[0]
            && p[1] >= box_.min[1]
            && p[1] <= box_.max[1]
            && p[2] >= box_.min[2]
            && p[2] <= box_.max[2]
    }

    pub fn gmdl_object_world_aabb(positions: &[f32], center: [f32; 3], scale: f32) -> Option<Aabb> {
        if positions.is_empty() {
            return None;
        }
        let (cx, cy, cz) = (center[0], center[1], center[2]);
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut min_z = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut max_z = f32::NEG_INFINITY;
        for chunk in positions.chunks_exact(3) {
            let x = (chunk[0] - cx) * scale;
            let y = (chunk[2] - cz) * scale;
            let z = -(chunk[1] - cy) * scale;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            min_z = min_z.min(z);
            max_z = max_z.max(z);
        }
        let pad = 0.35;
        Some(Aabb {
            min: [min_x - pad, min_y - pad, min_z - pad],
            max: [max_x + pad, max_y + pad, max_z + pad],
        })
    }

    pub fn update(
        &mut self,
        dt: f32,
        fighters: &[(String, [f32; 3], bool, bool)],
        on_hurt: &mut dyn FnMut(&str, &str, i32, bool),
    ) -> HazardTickStats {
        let mut stats = HazardTickStats::default();
        self.tick_cooldowns(dt);
        if fighters.is_empty() {
            return stats;
        }

        let volumes = self.volumes.clone();
        for vol in &volumes {
            for (fname, pos, dead, spawn_protected) in fighters {
                let in_box = Self::point_in_aabb(*pos, &vol.box_);
                if !in_box {
                    continue;
                }
                let hell = vol.damage >= 100000;
                if !hell && (*dead || *spawn_protected) {
                    continue;
                }
                let key = if hell {
                    format!("vol:{}:{}", vol.name, fname)
                } else {
                    format!("environment:{fname}")
                };
                let ready_ok = self.ready(&key, vol.period);
                if !ready_ok {
                    continue;
                }
                on_hurt(fname, &vol.name, vol.damage, *dead);
                stats.volume_hits += 1;
                let _ = spawn_protected;
            }
        }

        let machines = self.machines.clone();
        for mach in &machines {
            if !mach.damage || mach.damage_value <= 0 || !mach.active {
                continue;
            }
            if mach.boxes.is_empty() {
                continue;
            }
            for (fname, pos, dead, spawn_protected) in fighters {
                let in_any = mach.boxes.iter().any(|b| Self::point_in_aabb(*pos, b));
                if !in_any {
                    continue;
                }
                if *dead || *spawn_protected {
                    continue;
                }
                let key = format!("environment:{fname}");
                if !self.ready(&key, 1.0) {
                    continue;
                }
                on_hurt(fname, &mach.name, mach.damage_value, *dead);
                stats.machine_hits += 1;
                let _ = spawn_protected;
            }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_damage_name() {
        assert_eq!(StageHazards::parse_damage_name(r#"name = "damage30""#), 30);
        assert_eq!(StageHazards::parse_damage_name("name = damage100"), 100);
        assert_eq!(StageHazards::parse_damage_name("blockplayer=no"), 0);
        assert_eq!(StageHazards::parse_damage_name(""), 0);
    }

    #[test]
    fn hell_detection() {
        assert!(StageHazards::is_hell(r#"name = "hell""#, false));
        assert!(StageHazards::is_hell("", true));
        assert!(!StageHazards::is_hell("blockplayer=no", false));
    }

    #[test]
    fn point_in_aabb_does_not_reach_feet_above_the_hazard() {
        let box_ = Aabb {
            min: [0.0, 0.0, 0.0],
            max: [2.0, 2.0, 2.0],
        };

        assert!(StageHazards::point_in_aabb([1.0, 1.0, 1.0], &box_));

        assert!(StageHazards::point_in_aabb([1.0, 2.0, 1.0], &box_));
        assert!(!StageHazards::point_in_aabb([1.0, 2.01, 1.0], &box_));
        assert!(!StageHazards::point_in_aabb([1.0, 3.5, 1.0], &box_));

        assert!(!StageHazards::point_in_aabb([1.0, 4.0, 1.0], &box_));

        assert!(!StageHazards::point_in_aabb([5.0, 1.0, 1.0], &box_));
    }

    #[test]
    fn gmdl_world_aabb_transform() {
        let box_ =
            StageHazards::gmdl_object_world_aabb(&[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 0.12).unwrap();
        assert!((box_.min[0] + 0.35).abs() < 1e-5);
        assert!((box_.max[0] - 0.35).abs() < 1e-5);

        let box_ = StageHazards::gmdl_object_world_aabb(&[10.0, 0.0, 20.0], [0.0, 0.0, 0.0], 0.12)
            .unwrap();
        assert!((box_.min[0] - (1.2 - 0.35)).abs() < 1e-5);
        assert!((box_.min[1] - (2.4 - 0.35)).abs() < 1e-5);

        assert!(StageHazards::gmdl_object_world_aabb(&[], [0.0, 0.0, 0.0], 0.12).is_none());
    }

    #[test]
    fn volume_damage_with_cooldown() {
        let mut hazards = StageHazards::new();
        hazards.volumes = vec![HazardVolume {
            name: "damage30".into(),
            damage: 30,
            box_: Aabb {
                min: [0.0, 0.0, 0.0],
                max: [5.0, 5.0, 5.0],
            },
            period: 1.0,
        }];
        let fighters = vec![("f1".to_string(), [1.0, 1.0, 1.0], false, false)];
        let mut hits = Vec::new();
        let stats = hazards.update(0.016, &fighters, &mut |who, src, dmg, _| {
            hits.push((who.to_string(), src.to_string(), dmg));
        });
        assert_eq!(stats.volume_hits, 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0], ("f1".to_string(), "damage30".to_string(), 30));

        let stats = hazards.update(0.016, &fighters, &mut |_, _, _, _| {});
        assert_eq!(stats.volume_hits, 0);

        let stats = hazards.update(1.0, &fighters, &mut |_, _, _, _| {});
        assert_eq!(stats.volume_hits, 1);
    }

    #[test]
    fn machine_damage_one_second_shared_actor_window() {
        let mut hazards = StageHazards::new();
        hazards.machines = vec![HazardMachine {
            name: "stick01".into(),
            damage: true,
            damage_value: 15,
            active: true,
            boxes: vec![Aabb {
                min: [0.0, 0.0, 0.0],
                max: [3.0, 3.0, 3.0],
            }],
        }];
        let fighters = vec![("f1".to_string(), [1.0, 1.0, 1.0], false, false)];
        let mut hits = Vec::new();
        let stats = hazards.update(0.016, &fighters, &mut |who, src, dmg, _| {
            hits.push((who.to_string(), src.to_string(), dmg));
        });
        assert_eq!(stats.machine_hits, 1);

        let stats = hazards.update(0.5, &fighters, &mut |_, _, _, _| {});
        assert_eq!(stats.machine_hits, 0);
        let stats = hazards.update(0.5, &fighters, &mut |_, _, _, _| {});
        assert_eq!(stats.machine_hits, 0);
        let stats = hazards.update(0.001, &fighters, &mut |_, _, _, _| {});
        assert_eq!(stats.machine_hits, 1);
    }

    #[test]
    fn damage_volumes_share_actor_clock_across_sources_and_leave_reenter() {
        let mut hazards = StageHazards::new();
        let bounds = Aabb {
            min: [0.0; 3],
            max: [3.0; 3],
        };
        hazards.volumes = vec![HazardVolume {
            name: "DaBox".into(),
            damage: 30,
            period: 1.0,
            box_: bounds,
        }];
        hazards.machines = vec![HazardMachine {
            name: "stick".into(),
            damage: true,
            damage_value: 300,
            active: true,
            boxes: vec![bounds],
        }];
        let inside = vec![("player".into(), [1.0; 3], false, false)];
        let protected = vec![("player".into(), [1.0; 3], false, true)];
        let mut amounts = Vec::new();
        hazards.update(0.0, &protected, &mut |_, _, damage, _| amounts.push(damage));
        assert!(amounts.is_empty());
        hazards.update(0.0, &inside, &mut |_, _, damage, _| amounts.push(damage));
        assert_eq!(amounts, [30], "one admission across overlapping sources");
        hazards.volumes.clear();
        hazards.update(0.5, &[], &mut |_, _, damage, _| amounts.push(damage));
        hazards.update(0.5, &inside, &mut |_, _, damage, _| amounts.push(damage));
        assert_eq!(
            amounts,
            [30],
            "leaving does not reset; exactly one second is blocked"
        );
        hazards.update(0.001, &inside, &mut |_, _, damage, _| amounts.push(damage));
        assert_eq!(amounts, [30, 300]);
        let other = vec![("npc".into(), [1.0; 3], false, false)];
        hazards.update(0.0, &other, &mut |_, _, damage, _| amounts.push(damage));
        assert_eq!(amounts, [30, 300, 300], "actors have independent clocks");
    }

    #[test]
    fn inactive_machine_no_damage() {
        let mut hazards = StageHazards::new();
        hazards.machines = vec![HazardMachine {
            name: "dead_pillar".into(),
            damage: true,
            damage_value: 20,
            active: false,
            boxes: vec![Aabb {
                min: [0.0, 0.0, 0.0],
                max: [3.0, 3.0, 3.0],
            }],
        }];
        let fighters = vec![("f1".to_string(), [1.0, 1.0, 1.0], false, false)];
        let stats = hazards.update(0.016, &fighters, &mut |_, _, _, _| {});
        assert_eq!(stats.machine_hits, 0);
    }
}
