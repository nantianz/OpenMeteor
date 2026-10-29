use meteor_core::MODEL_SCALE;
use std::collections::HashMap;

pub const DART_SPEED: f32 = 600.0 * MODEL_SCALE;

pub const DART_VERTICAL_BIAS: f32 = 0.13;
pub const DART_GRAVITY: f32 = 50.0 * MODEL_SCALE;
pub const DART_SPAWN_HEIGHT: f32 = 40.0 * MODEL_SCALE;
pub const DART_SPIN_SPEED: f32 = 8.0;
pub const GUN_SPAWN_HEIGHT: f32 = 30.0 * MODEL_SCALE;
pub const GUN_RANGE: f32 = 1000.0 * MODEL_SCALE;
pub const WHEEL_SPAWN_HEIGHT: f32 = 20.0 * MODEL_SCALE;
pub const WHEEL_GUIDE_RANGE: f32 = 600.0 * MODEL_SCALE;
pub const WHEEL_TARGET_LEAD_TIME: f32 = 0.2;
pub const WHEEL_RETURN_ACCEL: f32 = 8000.0 * MODEL_SCALE;
pub const WHEEL_RETURN_SPEED_SCALE: f32 = 1.2;
pub const WHEEL_PATH_EPSILON: f32 = 26.0 * MODEL_SCALE;

pub const RANGED_TARGET_RADIUS: f32 = 7.0 * MODEL_SCALE;
pub const RANGED_TARGET_HEIGHT: f32 = 36.0 * MODEL_SCALE;

pub const PROJECTILE_MAX_AGE: f32 = 5.0;

pub const DART_HIT_SCALE: f32 = 1.0;
pub const GUN_HIT_SCALE: f32 = 1.0;
pub const WHEEL_HIT_SCALE: f32 = 2.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangedMode {
    Plain,

    Effect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RangedKind {
    Dart,
    Gun,
    Wheel,
}

#[derive(Debug, Clone, Copy)]
pub struct RangedProfile {
    pub weapon_id: u8,
    pub strategy_id: u8,
    pub kind: RangedKind,
    pub mode: RangedMode,

    pub entity: bool,

    pub hit_scale: f32,

    pub speed: Option<f32>,

    pub ray: bool,
}

pub fn ranged_profiles() -> &'static HashMap<u16, RangedProfile> {
    use RangedKind::*;
    use RangedMode::*;
    static PROFILES: std::sync::OnceLock<HashMap<u16, RangedProfile>> = std::sync::OnceLock::new();
    PROFILES.get_or_init(|| {
        let mut m = HashMap::new();

        for pose in [200u16, 202, 209, 210] {
            m.insert(
                pose,
                RangedProfile {
                    weapon_id: 1,
                    strategy_id: 3,
                    kind: Dart,
                    mode: Plain,
                    entity: true,
                    hit_scale: DART_HIT_SCALE,
                    speed: None,
                    ray: false,
                },
            );
        }
        for pose in [201u16, 204, 551] {
            m.insert(
                pose,
                RangedProfile {
                    weapon_id: 1,
                    strategy_id: 3,
                    kind: Dart,
                    mode: Effect,
                    entity: false,
                    hit_scale: DART_HIT_SCALE,
                    speed: None,
                    ray: false,
                },
            );
        }
        m.insert(
            203,
            RangedProfile {
                weapon_id: 1,
                strategy_id: 3,
                kind: Dart,
                mode: Effect,
                entity: false,
                hit_scale: DART_HIT_SCALE,
                speed: None,
                ray: false,
            },
        );

        for pose in [206u16, 207, 208] {
            m.insert(
                pose,
                RangedProfile {
                    weapon_id: 3,
                    strategy_id: 2,
                    kind: Gun,
                    mode: Plain,
                    entity: false,
                    hit_scale: GUN_HIT_SCALE,
                    speed: None,
                    ray: true,
                },
            );
        }
        m.insert(
            215,
            RangedProfile {
                weapon_id: 3,
                strategy_id: 2,
                kind: Gun,
                mode: Effect,
                entity: false,
                hit_scale: GUN_HIT_SCALE,
                speed: None,
                ray: false,
            },
        );
        m.insert(
            217,
            RangedProfile {
                weapon_id: 3,
                strategy_id: 2,
                kind: Gun,
                mode: Effect,
                entity: false,
                hit_scale: GUN_HIT_SCALE,
                speed: None,
                ray: true,
            },
        );

        m.insert(
            218,
            RangedProfile {
                weapon_id: 2,
                strategy_id: 6,
                kind: Wheel,
                mode: Plain,
                entity: true,
                hit_scale: WHEEL_HIT_SCALE,
                speed: Some(42.0),
                ray: false,
            },
        );
        m.insert(
            220,
            RangedProfile {
                weapon_id: 2,
                strategy_id: 6,
                kind: Wheel,
                mode: Plain,
                entity: true,
                hit_scale: WHEEL_HIT_SCALE,
                speed: Some(48.0),
                ray: false,
            },
        );
        m.insert(
            222,
            RangedProfile {
                weapon_id: 2,
                strategy_id: 6,
                kind: Wheel,
                mode: Plain,
                entity: true,
                hit_scale: WHEEL_HIT_SCALE,
                speed: Some(54.0),
                ray: false,
            },
        );
        m.insert(
            224,
            RangedProfile {
                weapon_id: 2,
                strategy_id: 6,
                kind: Wheel,
                mode: Effect,
                entity: true,
                hit_scale: WHEEL_HIT_SCALE,
                speed: Some(54.0),
                ray: false,
            },
        );
        for pose in [552u16, 554] {
            m.insert(
                pose,
                RangedProfile {
                    weapon_id: 2,
                    strategy_id: 6,
                    kind: Wheel,
                    mode: Effect,
                    entity: false,
                    hit_scale: WHEEL_HIT_SCALE,
                    speed: None,
                    ray: false,
                },
            );
        }
        m
    })
}

pub fn ranged_attack_profile(
    pose_id: u16,
    weapon_id: u8,
    strategy_id: Option<u8>,
) -> Option<RangedProfile> {
    let profile = *ranged_profiles().get(&pose_id)?;
    if weapon_id != profile.weapon_id {
        return None;
    }
    if let Some(s) = strategy_id {
        if s != profile.strategy_id {
            return None;
        }
    }
    Some(profile)
}

pub fn is_plain_ranged_attack(profile: Option<RangedProfile>, attack_has_bones: bool) -> bool {
    match profile {
        Some(p) if p.mode == RangedMode::Plain => !attack_has_bones,
        _ => false,
    }
}

pub fn bypasses_melee_hitboxes(profile: Option<RangedProfile>) -> bool {
    profile.is_some()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct TargetBox {
    pub min: Point3,
    pub max: Point3,
}

pub fn ranged_target_box(foot: Point3, hit_scale: f32) -> TargetBox {
    let r = RANGED_TARGET_RADIUS * hit_scale.max(1.0);
    TargetBox {
        min: Point3 {
            x: foot.x - r,
            y: foot.y,
            z: foot.z - r,
        },
        max: Point3 {
            x: foot.x + r,
            y: foot.y + RANGED_TARGET_HEIGHT,
            z: foot.z + r,
        },
    }
}

pub fn segment_hits_box_distance(p0: Point3, p1: Point3, box_: TargetBox) -> Option<f32> {
    let d = Point3 {
        x: p1.x - p0.x,
        y: p1.y - p0.y,
        z: p1.z - p0.z,
    };
    let length = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
    let mut enter = 0.0f32;
    let mut exit = 1.0f32;

    for (delta, origin, min, max) in [
        (d.x, p0.x, box_.min.x, box_.max.x),
        (d.y, p0.y, box_.min.y, box_.max.y),
        (d.z, p0.z, box_.min.z, box_.max.z),
    ] {
        if delta.abs() < 1e-9 {
            if origin < min || origin > max {
                return None;
            }
            continue;
        }
        let mut t0 = (min - origin) / delta;
        let mut t1 = (max - origin) / delta;
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        if t0 > enter {
            enter = t0;
        }
        if t1 < exit {
            exit = t1;
        }
        if enter > exit {
            return None;
        }
    }
    Some(enter * length)
}

pub fn forward_components_from_yaw(yaw: f32) -> Point3 {
    Point3 {
        x: yaw.sin(),
        y: 0.0,
        z: yaw.cos(),
    }
}

pub fn wheel_guide_score(origin: Point3, yaw: f32, target: Point3) -> Option<f32> {
    let dx = target.x - origin.x;
    let dy = target.y - origin.y;
    let dz = target.z - origin.z;
    let range = WHEEL_GUIDE_RANGE;
    if dx * dx + dy * dy + dz * dz > range * range {
        return None;
    }
    let horizontal = (dx * dx + dz * dz).sqrt();
    if horizontal <= 1e-8 {
        return None;
    }
    let f = forward_components_from_yaw(yaw);
    let score = (dx * f.x + dz * f.z) / horizontal;
    if score > 0.0 {
        Some(score)
    } else {
        None
    }
}

pub fn dart_gravity_drop(dt: f32, vertical_speed: f32) -> f32 {
    DART_GRAVITY * dt + vertical_speed * dt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_cover_all_poses() {
        let m = ranged_profiles();
        assert_eq!(m.len(), 19);

        assert_eq!(m.get(&218).unwrap().speed, Some(42.0));
        assert_eq!(m.get(&222).unwrap().speed, Some(54.0));
        assert_eq!(m.get(&224).unwrap().speed, Some(54.0));
        assert!(!m.get(&203).unwrap().entity);
        assert_eq!(m.get(&551).unwrap().mode, RangedMode::Effect);
        assert!(!m.get(&552).unwrap().entity);
        assert!(!m.get(&554).unwrap().entity);

        assert!(m.get(&206).unwrap().ray);
        assert!(!m.get(&215).unwrap().ray);
    }

    #[test]
    fn profile_matches_weapon_and_strategy() {
        let p = ranged_attack_profile(200, 1, Some(3));
        assert!(p.is_some());
        assert_eq!(p.unwrap().kind, RangedKind::Dart);

        assert!(ranged_attack_profile(200, 2, Some(3)).is_none());

        assert!(ranged_attack_profile(200, 1, Some(6)).is_none());

        assert!(ranged_attack_profile(999, 1, Some(3)).is_none());
    }

    #[test]
    fn plain_vs_effect() {
        let p = ranged_attack_profile(200, 1, Some(3));
        assert!(is_plain_ranged_attack(p, false));

        let p = ranged_attack_profile(201, 1, Some(3));
        assert!(!is_plain_ranged_attack(p, false));

        let p = ranged_attack_profile(200, 1, Some(3));
        assert!(!is_plain_ranged_attack(p, true));
    }

    #[test]
    fn ranged_bypasses_melee() {
        let p = ranged_attack_profile(206, 3, Some(2));
        assert!(bypasses_melee_hitboxes(p));
        assert!(!bypasses_melee_hitboxes(None));
    }

    #[test]
    fn target_box_sizes() {
        let foot = Point3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let box_ = ranged_target_box(foot, DART_HIT_SCALE);
        let r = RANGED_TARGET_RADIUS;
        assert!((box_.min.x + r).abs() < 1e-5);
        assert!((box_.max.y - RANGED_TARGET_HEIGHT).abs() < 1e-5);

        let wheel_box = ranged_target_box(foot, WHEEL_HIT_SCALE);
        assert!((wheel_box.max.x - r * 2.6).abs() < 1e-5);
    }

    #[test]
    fn segment_hit_enters_box() {
        let box_ = TargetBox {
            min: Point3 {
                x: -1.0,
                y: -1.0,
                z: -1.0,
            },
            max: Point3 {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        };

        let d = segment_hits_box_distance(
            Point3 {
                x: -5.0,
                y: 0.0,
                z: 0.0,
            },
            Point3 {
                x: 5.0,
                y: 0.0,
                z: 0.0,
            },
            box_,
        )
        .unwrap();

        assert!((d - 4.0).abs() < 1e-5);
    }

    #[test]
    fn segment_miss_box() {
        let box_ = TargetBox {
            min: Point3 {
                x: -1.0,
                y: -1.0,
                z: -1.0,
            },
            max: Point3 {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        };

        assert!(segment_hits_box_distance(
            Point3 {
                x: -5.0,
                y: 5.0,
                z: 0.0
            },
            Point3 {
                x: 5.0,
                y: 5.0,
                z: 0.0
            },
            box_,
        )
        .is_none());
    }

    #[test]
    fn forward_components() {
        let f = forward_components_from_yaw(0.0);
        assert!((f.x - 0.0).abs() < 1e-5);
        assert!((f.z - 1.0).abs() < 1e-5);
        let f = forward_components_from_yaw(std::f32::consts::FRAC_PI_2);
        assert!((f.x - 1.0).abs() < 1e-5);
    }

    #[test]
    fn wheel_guide_filters() {
        let origin = Point3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };

        let s = wheel_guide_score(
            origin,
            0.0,
            Point3 {
                x: 0.0,
                y: 0.0,
                z: 5.0,
            },
        );
        assert!(s.is_some());
        assert!((s.unwrap() - 1.0).abs() < 1e-5);

        assert!(wheel_guide_score(
            origin,
            0.0,
            Point3 {
                x: 0.0,
                y: 0.0,
                z: -5.0
            }
        )
        .is_none());

        assert!(wheel_guide_score(
            origin,
            0.0,
            Point3 {
                x: 0.0,
                y: 0.0,
                z: 1000.0
            }
        )
        .is_none());
    }
}
