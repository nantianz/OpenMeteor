use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
    pub const UP: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 1.0,
    };

    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn dot(&self, other: &Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(&self, other: &Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    pub fn length(&self) -> f32 {
        self.dot(self).sqrt()
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

    pub fn to_bevy(&self) -> Self {
        Self {
            x: self.x,
            y: self.z,
            z: self.y,
        }
    }

    pub fn from_bevy(&self) -> Self {
        Self {
            x: self.x,
            y: self.z,
            z: self.y,
        }
    }

    pub fn des_to_bevy(&self) -> Self {
        Self {
            x: self.x,
            y: self.z,
            z: self.y,
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub fn from_rotation_z(angle: f32) -> Self {
        let half = angle * 0.5;
        Self {
            x: 0.0,
            y: 0.0,
            z: half.sin(),
            w: half.cos(),
        }
    }

    pub fn from_rotation_x(angle: f32) -> Self {
        let half = angle * 0.5;
        Self {
            x: half.sin(),
            y: 0.0,
            z: 0.0,
            w: half.cos(),
        }
    }

    pub fn normalize(&self) -> Self {
        let len = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if len <= f32::EPSILON {
            return Self::IDENTITY;
        }
        Self {
            x: self.x / len,
            y: self.y / len,
            z: self.z / len,
            w: self.w / len,
        }
    }

    pub fn dot(&self, other: &Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    pub fn slerp(&self, to: &Self, factor: f32) -> Self {
        let t = factor.clamp(0.0, 1.0);
        let mut x1 = to.x;
        let mut y1 = to.y;
        let mut z1 = to.z;
        let mut w1 = to.w;
        let mut dot = self.dot(to);
        if dot < 0.0 {
            dot = -dot;
            x1 = -x1;
            y1 = -y1;
            z1 = -z1;
            w1 = -w1;
        }
        dot = dot.clamp(-1.0, 1.0);

        let mut from_weight = 1.0 - t;
        let mut to_weight = t;
        if 1.0 - dot > 0.0001 {
            let angle = dot.acos();
            let reciprocal_sin = 1.0 / angle.sin();
            from_weight = ((1.0 - t) * angle).sin() * reciprocal_sin;
            to_weight = (t * angle).sin() * reciprocal_sin;
        }

        Self {
            x: self.x * from_weight + x1 * to_weight,
            y: self.y * from_weight + y1 * to_weight,
            z: self.z * from_weight + z1 * to_weight,
            w: self.w * from_weight + w1 * to_weight,
        }
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

pub fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let mut diff = b - a;
    while diff > std::f32::consts::PI {
        diff -= std::f32::consts::TAU;
    }
    while diff < -std::f32::consts::PI {
        diff += std::f32::consts::TAU;
    }
    a + diff * t.clamp(0.0, 1.0)
}

pub fn deg_to_rad(deg: f32) -> f32 {
    deg * std::f32::consts::PI / 180.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Quat, b: Quat, eps: f32) -> bool {
        (a.x - b.x).abs() < eps
            && (a.y - b.y).abs() < eps
            && (a.z - b.z).abs() < eps
            && (a.w - b.w).abs() < eps
    }

    #[test]
    fn slerp_endpoints_are_inputs() {
        let a = Quat::IDENTITY;
        let b = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        assert!(approx(a.slerp(&b, 0.0), a, 1e-5));
        assert!(approx(a.slerp(&b, 1.0), b, 1e-5));
    }

    #[test]
    fn slerp_midpoint_is_half_angle() {
        let a = Quat::IDENTITY;
        let b = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let mid = a.slerp(&b, 0.5);

        let expected = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
        assert!(approx(mid, expected, 1e-4));
    }

    #[test]
    fn slerp_takes_shortest_path_when_dot_negative() {
        let a = Quat::IDENTITY;

        let b = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let neg_b = Quat {
            x: -b.x,
            y: -b.y,
            z: -b.z,
            w: -b.w,
        };
        let mid = a.slerp(&neg_b, 0.5);
        let expected = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);

        assert!((mid.x.abs() - expected.x.abs()) < 1e-4);
        assert!((mid.y.abs() - expected.y.abs()) < 1e-4);
        assert!((mid.z.abs() - expected.z.abs()) < 1e-4);
        assert!((mid.w.abs() - expected.w.abs()) < 1e-4);
    }

    #[test]
    fn slerp_near_identical_falls_back_to_linear() {
        let a = Quat::IDENTITY;
        let b = Quat {
            x: 0.0,
            y: 0.0,
            z: 1e-6,
            w: 1.0,
        };

        let mid = a.slerp(&b, 0.5);
        assert!((mid.z - 5e-7).abs() < 1e-7);
        assert!((mid.w - 1.0).abs() < 1e-5);
    }

    #[test]
    fn lerp_clamps_factor() {
        assert!((lerp(0.0, 10.0, -1.0) - 0.0).abs() < 1e-6);
        assert!((lerp(0.0, 10.0, 2.0) - 10.0).abs() < 1e-6);
        assert!((lerp(0.0, 10.0, 0.5) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_angle_keeps_webs_positive_half_turn_tie_break() {
        let half = lerp_angle(0.0, std::f32::consts::PI, 0.5);
        assert!((half - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }
}
