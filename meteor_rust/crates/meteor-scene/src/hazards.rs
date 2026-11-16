use std::collections::{HashMap, HashSet};

pub const HAZARD_BODY_HALF_XY: f32 = 0.96;

pub const HAZARD_BODY_HEIGHT: f32 = 3.84;

#[derive(Debug, Clone, Copy)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
pub struct MachinePartObb {
    pub center: [f32; 3],
    pub half: [f32; 3],

    pub axes: [[f32; 3]; 3],
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

    pub obbs: Vec<MachinePartObb>,

    pub collider_owners: Vec<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct StageHazards {
    pub volumes: Vec<HazardVolume>,
    pub machines: Vec<HazardMachine>,

    pub world_bounds: Option<Aabb>,

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
            if !object.ticket_admitted || (!object.is_hell && object.damage_name == 0) {
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
                box_: StageHazards::gmdl_object_world_aabb(&part.positions, center, 0.12)?,
                period: if object.is_hell { 0.0 } else { 1.0 },
            })
        })
        .collect()
}

pub fn build_world_bounds(mesh: &meteor_asset::gmdl::GmdlMesh, center: [f32; 3]) -> Option<Aabb> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut count = 0;
    for point in mesh
        .objects
        .iter()
        .flat_map(|object| object.positions.chunks_exact(3))
    {
        if point.iter().any(|value| !value.is_finite()) {
            return None;
        }
        let world = [
            (point[0] - center[0]) * 0.12,
            (point[2] - center[2]) * 0.12,
            -(point[1] - center[1]) * 0.12,
        ];
        for axis in 0..3 {
            min[axis] = min[axis].min(world[axis]);
            max[axis] = max[axis].max(world[axis]);
        }
        count += 1;
    }
    (count > 0).then_some(Aabb { min, max })
}

impl StageHazards {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build_from_stage(&mut self, volumes: Vec<HazardVolume>, machines: Vec<HazardMachine>) {
        self.volumes = volumes;
        self.machines = machines;
        self.cooldowns.clear();
    }

    pub fn parse_damage_name(custom: &str) -> i32 {
        let lower = custom.to_ascii_lowercase();
        let Some(rest) = lower.split("name").nth(1) else {
            return 0;
        };
        let rest = rest.trim_start().trim_start_matches('=').trim_start();
        let rest = rest.trim_start_matches('"');
        let digits: String = rest
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().unwrap_or(0)
    }

    pub fn is_hell(custom: &str, flag: bool) -> bool {
        flag || custom.to_ascii_lowercase().contains("hell")
    }

    fn tick_cooldowns(&mut self, dt: f32) {
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
        self.cooldowns.insert(key.to_owned(), period);
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

    pub fn outside_world_bounds(p: [f32; 3], bounds: &Aabb) -> bool {
        (0..3).any(|axis| p[axis] < bounds.min[axis] || p[axis] > bounds.max[axis])
    }

    pub fn actor_body_obb(feet: [f32; 3]) -> MachinePartObb {
        let half_y = HAZARD_BODY_HEIGHT * 0.5;
        MachinePartObb {
            center: [feet[0], feet[1] + half_y, feet[2]],
            half: [HAZARD_BODY_HALF_XY, half_y, HAZARD_BODY_HALF_XY],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    pub fn actor_touches_machine_aabb(feet: [f32; 3], box_: &Aabb) -> bool {
        let body = Self::actor_body_obb(feet);
        let body_min = [
            body.center[0] - body.half[0],
            body.center[1] - body.half[1],
            body.center[2] - body.half[2],
        ];
        let body_max = [
            body.center[0] + body.half[0],
            body.center[1] + body.half[1],
            body.center[2] + body.half[2],
        ];
        body_min[0] <= box_.max[0]
            && body_max[0] >= box_.min[0]
            && body_min[1] <= box_.max[1]
            && body_max[1] >= box_.min[1]
            && body_min[2] <= box_.max[2]
            && body_max[2] >= box_.min[2]
    }

    pub fn obbs_intersect(a: &MachinePartObb, b: &MachinePartObb) -> bool {
        let ae = a.half;
        let be = b.half;
        if ae
            .iter()
            .chain(be.iter())
            .any(|v| !v.is_finite() || *v < 0.0)
        {
            return false;
        }
        let mut rotation = [[0.0f32; 3]; 3];
        let mut abs_rotation = [[0.0f32; 3]; 3];
        let eps = 1e-6;
        for i in 0..3 {
            for j in 0..3 {
                rotation[i][j] = dot3(&a.axes[i], &b.axes[j]);
                abs_rotation[i][j] = rotation[i][j].abs() + eps;
            }
        }
        let t = [
            b.center[0] - a.center[0],
            b.center[1] - a.center[1],
            b.center[2] - a.center[2],
        ];
        let translation = [
            dot3(&t, &a.axes[0]),
            dot3(&t, &a.axes[1]),
            dot3(&t, &a.axes[2]),
        ];
        for i in 0..3 {
            let ra = ae[i];
            let rb = be[0] * abs_rotation[i][0]
                + be[1] * abs_rotation[i][1]
                + be[2] * abs_rotation[i][2];
            if translation[i].abs() > ra + rb {
                return false;
            }
        }
        for i in 0..3 {
            let ra = ae[0] * abs_rotation[0][i]
                + ae[1] * abs_rotation[1][i]
                + ae[2] * abs_rotation[2][i];
            let rb = be[i];
            let t_b = translation[0] * rotation[0][i]
                + translation[1] * rotation[1][i]
                + translation[2] * rotation[2][i];
            if t_b.abs() > ra + rb {
                return false;
            }
        }

        for i in 0..3 {
            for j in 0..3 {
                let ra = ae[(i + 1) % 3] * abs_rotation[(i + 2) % 3][j]
                    + ae[(i + 2) % 3] * abs_rotation[(i + 1) % 3][j];
                let rb = be[(j + 1) % 3] * abs_rotation[i][(j + 2) % 3]
                    + be[(j + 2) % 3] * abs_rotation[i][(j + 1) % 3];
                let t_c = translation[(i + 2) % 3] * rotation[(i + 1) % 3][j]
                    - translation[(i + 1) % 3] * rotation[(i + 2) % 3][j];
                if t_c.abs() > ra + rb {
                    return false;
                }
            }
        }
        true
    }

    pub fn actor_touches_machine(feet: [f32; 3], machine: &HazardMachine) -> bool {
        let body = Self::actor_body_obb(feet);
        if !machine.obbs.is_empty() {
            return machine
                .obbs
                .iter()
                .any(|obb| Self::obbs_intersect(&body, obb));
        }
        machine
            .boxes
            .iter()
            .any(|b| Self::actor_touches_machine_aabb(feet, b))
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
        self.update_with_machine_touch(dt, fighters, on_hurt, None)
    }

    pub fn update_with_machine_touch(
        &mut self,
        dt: f32,
        fighters: &[(String, [f32; 3], bool, bool)],
        on_hurt: &mut dyn FnMut(&str, &str, i32, bool),
        mut machine_touch: Option<&mut dyn FnMut([f32; 3], &HazardMachine) -> bool>,
    ) -> HazardTickStats {
        let mut stats = HazardTickStats::default();
        self.tick_cooldowns(dt);
        if fighters.is_empty() {
            return stats;
        }
        let mut outside = HashSet::new();
        if let Some(bounds) = self.world_bounds {
            for (fname, pos, dead, spawn_protected) in fighters {
                if !Self::outside_world_bounds(*pos, &bounds) {
                    continue;
                }
                outside.insert(fname.as_str());
                if *dead
                    || *spawn_protected
                    || self.cooldowns.contains_key(&format!("environment:{fname}"))
                {
                    continue;
                }
                on_hurt(fname, "scene_boundary", 100000, false);
                stats.volume_hits += 1;
            }
        }
        let volumes = self.volumes.clone();
        for vol in &volumes {
            for (fname, pos, dead, spawn_protected) in fighters {
                if outside.contains(fname.as_str()) {
                    continue;
                }
                if !Self::point_in_aabb(*pos, &vol.box_) {
                    continue;
                }
                let hell = vol.damage >= 100000;
                if *dead || *spawn_protected {
                    continue;
                }
                let key = format!("environment:{fname}");
                if hell && self.cooldowns.contains_key(&key) {
                    continue;
                }
                if !hell && !self.ready(&key, vol.period) {
                    continue;
                }
                on_hurt(fname, &vol.name, vol.damage, *dead);
                stats.volume_hits += 1;
            }
        }

        let machines = self.machines.clone();
        for mach in &machines {
            if !mach.damage || mach.damage_value <= 0 || !mach.active {
                continue;
            }
            for (fname, pos, dead, spawn_protected) in fighters {
                if outside.contains(fname.as_str()) {
                    continue;
                }
                let in_any = if let Some(touch) = machine_touch.as_mut() {
                    touch(*pos, mach)
                } else {
                    Self::actor_touches_machine(*pos, mach)
                };
                if !in_any || *dead || *spawn_protected {
                    continue;
                }
                let key = format!("environment:{fname}");
                if !self.ready(&key, 1.0) {
                    continue;
                }
                on_hurt(fname, &mach.name, mach.damage_value, *dead);
                stats.machine_hits += 1;
            }
        }
        stats
    }
}

fn dot3(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn part_world_obb(
    local_positions: &[[f32; 3]],
    translation: [f32; 3],
    rotation: [[f32; 3]; 3],
    scale: [f32; 3],
) -> Option<MachinePartObb> {
    if local_positions.is_empty() {
        return None;
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in local_positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let local_center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let half = [
        (max[0] - min[0]) * 0.5 * scale[0].abs(),
        (max[1] - min[1]) * 0.5 * scale[1].abs(),
        (max[2] - min[2]) * 0.5 * scale[2].abs(),
    ];
    let rotated = [
        rotation[0][0] * local_center[0]
            + rotation[0][1] * local_center[1]
            + rotation[0][2] * local_center[2],
        rotation[1][0] * local_center[0]
            + rotation[1][1] * local_center[1]
            + rotation[1][2] * local_center[2],
        rotation[2][0] * local_center[0]
            + rotation[2][1] * local_center[1]
            + rotation[2][2] * local_center[2],
    ];
    Some(MachinePartObb {
        center: [
            translation[0] + rotated[0],
            translation[1] + rotated[1],
            translation[2] + rotated[2],
        ],
        half,
        axes: rotation,
    })
}

pub fn obb_world_aabb(obb: &MachinePartObb) -> Aabb {
    let mut ext = [0.0f32; 3];
    for i in 0..3 {
        ext[i] = obb.axes[0][i].abs() * obb.half[0]
            + obb.axes[1][i].abs() * obb.half[1]
            + obb.axes[2][i].abs() * obb.half[2];
    }
    Aabb {
        min: [
            obb.center[0] - ext[0],
            obb.center[1] - ext[1],
            obb.center[2] - ext[2],
        ],
        max: [
            obb.center[0] + ext[0],
            obb.center[1] + ext[1],
            obb.center[2] + ext[2],
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "original-assets")]
    #[test]
    fn original_scene_bounds_apply_on_all_axes_without_stage_special_cases() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mesh =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn21/sn21.gmb")).unwrap())
                .unwrap();
        let desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn21/sn21.des")).unwrap(),
        );
        let waypoint = std::fs::read_to_string(root.join("Level/sn21.wp")).unwrap();
        let center = crate::stage_loader::get_stage_center("sn21", Some(&waypoint));
        assert!(build_volumes(&mesh, &desc, center).is_empty());
        let bounds = build_world_bounds(&mesh, center).unwrap();
        let lowest_source_z = mesh
            .objects
            .iter()
            .flat_map(|object| object.positions.chunks_exact(3).map(|point| point[2]))
            .reduce(f32::min)
            .unwrap();
        assert!((bounds.min[1] - (lowest_source_z - center[2]) * 0.12).abs() < 0.001);
        for spawn in desc
            .dummies
            .iter()
            .filter(|object| object.name.starts_with("D_start"))
        {
            let p = spawn.position;
            let world = [
                (p.x - center[0]) * 0.12,
                (p.z - center[2]) * 0.12,
                -(p.y - center[1]) * 0.12,
            ];
            assert!(
                StageHazards::point_in_aabb(world, &bounds),
                "{}",
                spawn.name
            );
        }
        let mut hazards = StageHazards::new();
        hazards.world_bounds = Some(bounds);
        let midpoint = std::array::from_fn(|axis| (bounds.min[axis] + bounds.max[axis]) * 0.5);
        for axis in 0..3 {
            for side in 0..2 {
                let mut p = midpoint;
                p[axis] = if side == 0 {
                    bounds.min[axis]
                } else {
                    bounds.max[axis]
                };
                let mut hits = Vec::new();
                hazards.update(
                    0.016,
                    &[("player".into(), p, false, false)],
                    &mut |_, _, amount, _| hits.push(amount),
                );
                assert!(
                    hits.is_empty(),
                    "axis {axis} side {side} boundary is inclusive"
                );
                p[axis] += if side == 0 { -0.01 } else { 0.01 };
                let mut sources = Vec::new();
                hazards.update(
                    0.016,
                    &[("player".into(), p, false, false)],
                    &mut |_, source, amount, _| {
                        sources.push(source.to_owned());
                        hits.push(amount);
                    },
                );
                assert_eq!(hits, [100000]);
                assert_eq!(sources, ["scene_boundary"]);
            }
        }
    }

    #[test]
    fn world_exit_shares_original_environment_cooldown_and_spawn_gate() {
        let mut hazards = StageHazards::new();
        hazards.world_bounds = Some(Aabb {
            min: [-1.0; 3],
            max: [1.0; 3],
        });
        hazards.volumes.push(HazardVolume {
            name: "damage30".into(),
            damage: 30,
            box_: Aabb {
                min: [-0.5; 3],
                max: [0.5; 3],
            },
            period: 1.0,
        });
        let mut hits = Vec::new();
        hazards.update(
            0.016,
            &[("player".into(), [2.0, 0.0, 0.0], false, true)],
            &mut |_, _, amount, _| hits.push(amount),
        );
        assert!(hits.is_empty());
        hazards.update(
            0.016,
            &[("player".into(), [0.0; 3], false, false)],
            &mut |_, _, amount, _| hits.push(amount),
        );
        assert_eq!(hits, [30]);
        hazards.update(
            0.1,
            &[("player".into(), [2.0, 0.0, 0.0], false, false)],
            &mut |_, _, amount, _| hits.push(amount),
        );
        assert_eq!(hits, [30]);
        hazards.update(
            1.0,
            &[("player".into(), [2.0, 0.0, 0.0], false, false)],
            &mut |_, _, amount, _| hits.push(amount),
        );
        assert_eq!(hits, [30, 100000]);
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn original_bounds_contain_authored_spawns_on_all_official_stages() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        for number in 1..=22 {
            let stage = format!("sn{number:02}");
            let mesh = meteor_asset::gmdl::decode_gmdl(
                &std::fs::read(root.join(format!("{stage}/{stage}.gmb"))).unwrap(),
            )
            .unwrap();
            let des = std::fs::read(root.join(format!("{stage}/{stage}.des"))).unwrap();
            let desc = meteor_asset::des::parse_stage_description(&String::from_utf8_lossy(&des));
            let wp = std::fs::read(root.join(format!("Level/{stage}.wp"))).ok();
            let wp_text = wp.as_deref().map(String::from_utf8_lossy);
            let center = crate::stage_loader::get_stage_center(&stage, wp_text.as_deref());
            let bounds = build_world_bounds(&mesh, center).unwrap();
            let mut count = 0;
            for spawn in desc.dummies.iter().filter(|object| {
                let name = object.name.to_ascii_lowercase();
                name.starts_with("d_start")
                    || name.starts_with("d_user")
                    || name.starts_with("d_team")
            }) {
                let p = spawn.position;
                let world = [
                    (p.x - center[0]) * 0.12,
                    (p.z - center[2]) * 0.12,
                    -(p.y - center[1]) * 0.12,
                ];
                assert!(
                    StageHazards::point_in_aabb(world, &bounds),
                    "{stage}/{}",
                    spawn.name
                );
                count += 1;
            }
            assert!(count > 0, "{stage} has no authored spawn pads");
        }
    }

    #[test]
    fn parse_damage_name() {
        assert_eq!(StageHazards::parse_damage_name(r#"name = "damage30""#), 30);
        assert_eq!(StageHazards::parse_damage_name("name = damage100"), 100);
        assert_eq!(StageHazards::parse_damage_name("blockplayer=no"), 0);
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
    }

    #[test]
    fn body_obb_touches_proxy_box_but_not_fat_aabb_air_corner() {
        let proxy = MachinePartObb {
            center: [0.0, 1.0, 0.0],
            half: [5.0, 1.0, 0.4],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        };
        let feet_side = [-0.3, 0.5, 0.0];
        assert!(StageHazards::obbs_intersect(
            &StageHazards::actor_body_obb(feet_side),
            &proxy
        ));

        let air_feet = [0.0, 0.5, 4.0];
        assert!(!StageHazards::obbs_intersect(
            &StageHazards::actor_body_obb(air_feet),
            &proxy
        ));
        let fat = Aabb {
            min: [-5.0, 0.0, -0.4],
            max: [5.0, 2.0, 4.5],
        };
        assert!(StageHazards::actor_touches_machine_aabb(air_feet, &fat));
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
            obbs: vec![],
            collider_owners: vec![],
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
    fn shared_environment_clock_and_spawn_gate() {
        let bounds = Aabb {
            min: [0.0; 3],
            max: [2.0; 3],
        };
        let mut hazards = StageHazards::new();
        hazards.machines = vec![HazardMachine {
            name: "stick".into(),
            damage: true,
            damage_value: 300,
            active: true,
            boxes: vec![bounds],
            obbs: vec![],
            collider_owners: vec![],
        }];
        let inside = vec![("player".into(), [1.0; 3], false, false)];
        let protected = vec![("player".into(), [1.0; 3], false, true)];
        let mut amounts = Vec::new();
        hazards.update(0.0, &protected, &mut |_, _, damage, _| amounts.push(damage));
        assert!(amounts.is_empty());
        hazards.update(0.0, &inside, &mut |_, _, damage, _| amounts.push(damage));
        assert_eq!(amounts, vec![300]);
        hazards.update(0.5, &[], &mut |_, _, damage, _| amounts.push(damage));
        hazards.update(0.5, &inside, &mut |_, _, damage, _| amounts.push(damage));
        assert_eq!(amounts, vec![300]);
        hazards.update(0.001, &inside, &mut |_, _, damage, _| amounts.push(damage));
        assert_eq!(amounts, vec![300, 300]);
        let other = vec![("other".into(), [1.0; 3], false, false)];
        hazards.update(0.0, &other, &mut |_, _, damage, _| amounts.push(damage));
        assert_eq!(amounts, vec![300, 300, 300]);
    }

    #[test]
    fn inactive_machine_does_not_damage() {
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
            obbs: vec![],
            collider_owners: vec![],
        }];
        let fighters = vec![("hero".into(), [1.5; 3], false, false)];
        let stats = hazards.update(0.016, &fighters, &mut |_, _, _, _| {});
        assert_eq!(stats.machine_hits, 0);
    }

    #[test]
    fn volume_damage_with_cooldown() {
        let mut hazards = StageHazards::new();
        hazards.volumes = vec![HazardVolume {
            name: "damage30".into(),
            damage: 30,
            box_: Aabb {
                min: [0.0; 3],
                max: [2.0; 3],
            },
            period: 1.0,
        }];
        let fighters = vec![("f1".to_string(), [1.0, 1.0, 1.0], false, false)];
        let mut hits = Vec::new();
        let stats = hazards.update(0.016, &fighters, &mut |who, src, dmg, _| {
            hits.push((who.to_string(), src.to_string(), dmg));
        });
        assert_eq!(stats.volume_hits, 1);
        assert_eq!(hits[0], ("f1".to_string(), "damage30".to_string(), 30));
        let stats = hazards.update(0.016, &fighters, &mut |_, _, _, _| {});
        assert_eq!(stats.volume_hits, 0);
        let stats = hazards.update(1.0, &fighters, &mut |_, _, _, _| {});
        assert_eq!(stats.volume_hits, 1);
    }

    #[test]
    fn gmdl_world_aabb_transform() {
        let box_ =
            StageHazards::gmdl_object_world_aabb(&[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 0.12).unwrap();
        assert!((box_.min[0] + 0.35).abs() < 1e-5);
        assert!((box_.max[0] - 0.35).abs() < 1e-5);
        let box_ = StageHazards::gmdl_object_world_aabb(&[10.0, 0.0, 20.0], [0.0, 0.0, 0.0], 0.12)
            .unwrap();
        assert!((box_.min[0] - (1.2 - 0.35)).abs() < 1e-4);
        assert!(StageHazards::gmdl_object_world_aabb(&[], [0.0, 0.0, 0.0], 0.12).is_none());
    }
}
