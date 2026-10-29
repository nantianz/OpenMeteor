use crate::locomotion::pose;
use meteor_core::MODEL_SCALE;

pub const WALL_CLING_PROBE: f32 = 16.0 * MODEL_SCALE;

pub const WALL_KICK_PROBE: f32 = 20.0 * MODEL_SCALE;

pub const WALL_CLING_FACING_WEIGHT: f32 = 2.0;

pub const WALL_PROBE_HEIGHT: f32 = 0.0;

pub const WALL_KICK_SPEED: f32 = 180.0 * MODEL_SCALE;

pub const WALL_KICK_SPEED_STRONG: f32 = 260.0 * MODEL_SCALE;

pub const WALL_CLING_FRONT_ANGLE: f32 = 2.7;

pub const WALL_CLING_MAX_TIME: f32 = 1.8;

pub const WALL_VELOCITY_DAMPING: f32 = 1.2;

pub const WALL_GRAVITY_SCALE: f32 = 0.1;

pub const WALL_EXT_ACCEL: f32 = 800.0;

pub const WALL_EXT_NORMAL_BIAS: f32 = 100.0;

pub const WALL_CONTACT_PROBE: f32 = 18.0 * MODEL_SCALE;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn dot(&self, o: &Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn normalize(&self) -> Self {
        let len = self.length();
        if len > 1e-8 {
            Self {
                x: self.x / len,
                y: self.y / len,
                z: self.z / len,
            }
        } else {
            *self
        }
    }
}

impl std::ops::Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
        }
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
        }
    }
}

impl std::ops::Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
        }
    }
}

pub fn wall_cling_probe_direction(facing: Vec3, planar_velocity: Vec3) -> Vec3 {
    (facing * WALL_CLING_FACING_WEIGHT + planar_velocity.normalize()).normalize()
}

pub fn wall_normal_climbable(normal: Vec3) -> bool {
    normal.x.is_finite()
        && normal.y.is_finite()
        && normal.z.is_finite()
        && normal.dot(&normal) > 1e-12
}

pub fn cling_pose_for_normal(wall_outward_normal: Vec3, facing: Vec3) -> u16 {
    let n = Vec3::new(wall_outward_normal.x, 0.0, wall_outward_normal.z).normalize();
    let f = facing.normalize();

    let cos_angle = (n.x * f.x + n.z * f.z).clamp(-1.0, 1.0);
    let angle = cos_angle.acos();
    if angle >= WALL_CLING_FRONT_ANGLE {
        return pose::WALL_CLING_F;
    }

    let cross_y = n.x * f.z - n.z * f.x;
    if cross_y > 0.0 {
        pose::WALL_CLING_R
    } else {
        pose::WALL_CLING_L
    }
}

pub fn wall_kick_velocity(normal: Vec3, speed: f32) -> Vec3 {
    let up = Vec3::new(0.0, 1.0, 0.0);
    (normal + up).normalize() * speed
}

pub fn wall_cling_external_accel(velocity: Vec3, wall_normal: Vec3) -> Vec3 {
    let n = wall_normal;
    let v_src = velocity * (1.0 / MODEL_SCALE);
    let dx = v_src.x - WALL_EXT_NORMAL_BIAS * n.x;
    let dz = v_src.z - WALL_EXT_NORMAL_BIAS * n.z;
    let dy = v_src.y - WALL_EXT_NORMAL_BIAS * n.y;
    let len = (dx * dx + dy * dy + dz * dz).sqrt();
    if len < 1e-6 {
        Vec3::ZERO
    } else {
        Vec3::new(dx, 0.0, dz) * (WALL_EXT_ACCEL * MODEL_SCALE / len)
    }
}

pub fn wall_cling_accel(velocity: Vec3, external_accel: Vec3) -> Vec3 {
    velocity * -WALL_VELOCITY_DAMPING + external_accel
}

pub fn wall_cling_probe_offset(wall_normal: Vec3) -> Vec3 {
    wall_normal * -WALL_CONTACT_PROBE
}

#[derive(Debug, Clone)]
pub struct WallClingState {
    pub active: bool,

    pub remaining: f32,

    pub wall_normal: Vec3,
}

impl WallClingState {
    pub fn new() -> Self {
        Self {
            active: false,
            remaining: 0.0,
            wall_normal: Vec3::ZERO,
        }
    }

    pub fn start(&mut self, wall_normal: Vec3) {
        self.active = true;
        self.remaining = WALL_CLING_MAX_TIME;
        self.wall_normal = wall_normal.normalize();
    }

    pub fn tick(&mut self, dt: f32) -> bool {
        if !self.active {
            return false;
        }
        self.remaining -= dt;
        if self.remaining <= 0.0 {
            self.active = false;
            return true;
        }
        false
    }

    pub fn stop(&mut self) {
        self.active = false;
    }
}

impl Default for WallClingState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_direction_weights_facing() {
        let facing = Vec3::new(0.0, 0.0, 1.0);
        let planar = Vec3::ZERO;
        let dir = wall_cling_probe_direction(facing, planar);
        assert!((dir.z - 1.0).abs() < 1e-5);

        let planar = Vec3::new(1.0, 0.0, 1.0);
        let dir = wall_cling_probe_direction(facing, planar);
        assert!(dir.x > 0.0);
        assert!(dir.z > 0.0);
    }

    #[test]
    fn normal_climbable_gate() {
        assert!(wall_normal_climbable(Vec3::new(0.0, 1.0, 0.0)));

        assert!(wall_normal_climbable(Vec3::new(0.0, 0.6, 0.0)));

        assert!(wall_normal_climbable(Vec3::new(1.0, 0.0, 0.0)));
        assert!(wall_normal_climbable(Vec3::new(0.6, 0.8, 0.0)));
        assert!(wall_normal_climbable(Vec3::new(0.6, -0.8, 0.0)));
    }

    #[test]
    fn cling_pose_head_on() {
        let wall = Vec3::new(0.0, 0.0, -1.0);
        let facing = Vec3::new(0.0, 0.0, 1.0);
        assert_eq!(cling_pose_for_normal(wall, facing), pose::WALL_CLING_F);
    }

    #[test]
    fn cling_pose_side() {
        let wall = Vec3::new(1.0, 0.0, 0.0);
        let facing = Vec3::new(0.0, 0.0, 1.0);
        assert_eq!(cling_pose_for_normal(wall, facing), pose::WALL_CLING_R);

        let wall = Vec3::new(-1.0, 0.0, 0.0);
        assert_eq!(cling_pose_for_normal(wall, facing), pose::WALL_CLING_L);
    }

    #[test]
    fn wall_kick_adds_up_and_away() {
        let v = wall_kick_velocity(Vec3::new(1.0, 0.0, 0.0), WALL_KICK_SPEED);
        assert!(v.x > 0.0);
        assert!(v.y > 0.0);

        let v_strong = wall_kick_velocity(Vec3::new(1.0, 0.0, 0.0), WALL_KICK_SPEED_STRONG);
        assert!(v_strong.length() > v.length());
    }

    #[test]
    fn cling_accel_damps_and_adheres() {
        let velocity = Vec3::new(10.0, 0.0, 0.0);
        let normal = Vec3::new(1.0, 0.0, 0.0);
        let external = wall_cling_external_accel(velocity, normal);
        let accel = wall_cling_accel(velocity, external);

        assert!(accel.x < 0.0);

        assert!(accel.x < -1.0);

        assert_eq!(accel.y, 0.0);
        let probe = wall_cling_probe_offset(normal);
        assert!((probe.x + WALL_CONTACT_PROBE).abs() < 1e-5);
    }

    #[test]
    fn cling_external_accel_is_frozen_at_entry() {
        let normal = Vec3::new(1.0, 0.0, 0.0);
        let entry = Vec3::new(10.0, 20.0, 0.0);
        let external = wall_cling_external_accel(entry, normal);
        let later = Vec3::new(-4.0, -8.0, 3.0);
        let accel = wall_cling_accel(later, external);
        let recomputed = wall_cling_accel(later, wall_cling_external_accel(later, normal));
        assert_ne!(accel, recomputed);
    }

    #[test]
    fn cling_times_out() {
        let mut c = WallClingState::new();
        c.start(Vec3::new(1.0, 0.0, 0.0));
        assert!(c.active);
        assert_eq!(c.remaining, WALL_CLING_MAX_TIME);

        let timed_out = c.tick(2.0);
        assert!(timed_out);
        assert!(!c.active);
    }

    #[test]
    fn constants_match_exe() {
        assert_eq!(WALL_CLING_PROBE, 16.0 * MODEL_SCALE);
        assert_eq!(WALL_KICK_PROBE, 20.0 * MODEL_SCALE);
        assert_eq!(WALL_KICK_SPEED, 180.0 * MODEL_SCALE);
        assert_eq!(WALL_KICK_SPEED_STRONG, 260.0 * MODEL_SCALE);
        assert_eq!(WALL_CONTACT_PROBE, 18.0 * MODEL_SCALE);
        assert_eq!(WALL_CLING_MAX_TIME, 1.8);
    }
}
