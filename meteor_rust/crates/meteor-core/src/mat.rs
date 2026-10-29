use serde::{Deserialize, Serialize};

use crate::math::{Quat, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mat4 {
    pub m: [f32; 16],
}

impl Mat4 {
    pub const IDENTITY: Self = Self {
        m: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    };

    pub fn identity() -> Self {
        Self::IDENTITY
    }

    pub fn from_scale(scale: Vec3) -> Self {
        Self {
            m: [
                scale.x, 0.0, 0.0, 0.0, 0.0, scale.y, 0.0, 0.0, 0.0, 0.0, scale.z, 0.0, 0.0, 0.0,
                0.0, 1.0,
            ],
        }
    }

    pub fn compose(translation: Vec3, rotation: Quat, scale: Vec3) -> Self {
        let t = Self::from_translation(translation);
        let r = Self::from_quaternion(rotation);
        let s = Self::from_scale(scale);
        t.mul(&r.mul(&s))
    }

    pub fn translation(&self) -> Vec3 {
        Vec3::new(self.m[12], self.m[13], self.m[14])
    }

    pub fn set_translation(&mut self, t: Vec3) {
        self.m[12] = t.x;
        self.m[13] = t.y;
        self.m[14] = t.z;
    }

    pub fn from_translation(t: Vec3) -> Self {
        Self {
            m: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, t.x, t.y, t.z, 1.0,
            ],
        }
    }

    pub fn from_quaternion(q: Quat) -> Self {
        let Quat { x, y, z, w } = q;
        let x2 = x + x;
        let y2 = y + y;
        let z2 = z + z;
        let xx = x * x2;
        let xy = x * y2;
        let xz = x * z2;
        let yy = y * y2;
        let yz = y * z2;
        let zz = z * z2;
        let wx = w * x2;
        let wy = w * y2;
        let wz = w * z2;

        Self {
            m: [
                1.0 - (yy + zz),
                xy + wz,
                xz - wy,
                0.0,
                xy - wz,
                1.0 - (xx + zz),
                yz + wx,
                0.0,
                xz + wy,
                yz - wx,
                1.0 - (xx + yy),
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
            ],
        }
    }

    pub fn from_trs(translation: Vec3, rotation: Quat) -> Self {
        let r = Self::from_quaternion(rotation);
        let mut m = r.m;
        m[12] = translation.x;
        m[13] = translation.y;
        m[14] = translation.z;
        Self { m }
    }

    pub fn mul(&self, rhs: &Self) -> Self {
        let mut out = [0.0f32; 16];

        for col in 0..4 {
            for row in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += self.m[k * 4 + row] * rhs.m[col * 4 + k];
                }
                out[col * 4 + row] = s;
            }
        }
        Self { m: out }
    }

    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        let m = &self.m;
        let x = m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12];
        let y = m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13];
        let z = m[2] * p.x + m[6] * p.y + m[10] * p.z + m[14];
        let w = m[3] * p.x + m[7] * p.y + m[11] * p.z + m[15];
        if w.abs() > 1e-12 {
            Vec3::new(x / w, y / w, z / w)
        } else {
            Vec3::new(x, y, z)
        }
    }

    pub fn invert(&self) -> Self {
        let n = &self.m;
        let mut inv = [0.0f32; 16];

        inv[0] = n[5] * n[10] * n[15] - n[5] * n[11] * n[14] - n[9] * n[6] * n[15]
            + n[9] * n[7] * n[14]
            + n[13] * n[6] * n[11]
            - n[13] * n[7] * n[10];
        inv[4] = -n[4] * n[10] * n[15] + n[4] * n[11] * n[14] + n[8] * n[6] * n[15]
            - n[8] * n[7] * n[14]
            - n[12] * n[6] * n[11]
            + n[12] * n[7] * n[10];
        inv[8] = n[4] * n[9] * n[15] - n[4] * n[11] * n[13] - n[8] * n[5] * n[15]
            + n[8] * n[7] * n[13]
            + n[12] * n[5] * n[11]
            - n[12] * n[7] * n[9];
        inv[12] = -n[4] * n[9] * n[14] + n[4] * n[10] * n[13] + n[8] * n[5] * n[14]
            - n[8] * n[6] * n[13]
            - n[12] * n[5] * n[10]
            + n[12] * n[6] * n[9];

        inv[1] = -n[1] * n[10] * n[15] + n[1] * n[11] * n[14] + n[9] * n[2] * n[15]
            - n[9] * n[3] * n[14]
            - n[13] * n[2] * n[11]
            + n[13] * n[3] * n[10];
        inv[5] = n[0] * n[10] * n[15] - n[0] * n[11] * n[14] - n[8] * n[2] * n[15]
            + n[8] * n[3] * n[14]
            + n[12] * n[2] * n[11]
            - n[12] * n[3] * n[10];
        inv[9] = -n[0] * n[9] * n[15] + n[0] * n[11] * n[13] + n[8] * n[1] * n[15]
            - n[8] * n[3] * n[13]
            - n[12] * n[1] * n[11]
            + n[12] * n[3] * n[9];
        inv[13] = n[0] * n[9] * n[14] - n[0] * n[10] * n[13] - n[8] * n[1] * n[14]
            + n[8] * n[2] * n[13]
            + n[12] * n[1] * n[10]
            - n[12] * n[2] * n[9];

        inv[2] = n[1] * n[6] * n[15] - n[1] * n[7] * n[14] - n[5] * n[2] * n[15]
            + n[5] * n[3] * n[14]
            + n[13] * n[2] * n[7]
            - n[13] * n[3] * n[6];
        inv[6] = -n[0] * n[6] * n[15] + n[0] * n[7] * n[14] + n[4] * n[2] * n[15]
            - n[4] * n[3] * n[14]
            - n[12] * n[2] * n[7]
            + n[12] * n[3] * n[6];
        inv[10] = n[0] * n[5] * n[15] - n[0] * n[7] * n[13] - n[4] * n[1] * n[15]
            + n[4] * n[3] * n[13]
            + n[12] * n[1] * n[7]
            - n[12] * n[3] * n[5];
        inv[14] = -n[0] * n[5] * n[14] + n[0] * n[6] * n[13] + n[4] * n[1] * n[14]
            - n[4] * n[2] * n[13]
            - n[12] * n[1] * n[6]
            + n[12] * n[2] * n[5];

        inv[3] = -n[1] * n[6] * n[11] + n[1] * n[7] * n[10] + n[5] * n[2] * n[11]
            - n[5] * n[3] * n[10]
            - n[9] * n[2] * n[7]
            + n[9] * n[3] * n[6];
        inv[7] = n[0] * n[6] * n[11] - n[0] * n[7] * n[10] - n[4] * n[2] * n[11]
            + n[4] * n[3] * n[10]
            + n[8] * n[2] * n[7]
            - n[8] * n[3] * n[6];
        inv[11] = -n[0] * n[5] * n[11] + n[0] * n[7] * n[9] + n[4] * n[1] * n[11]
            - n[4] * n[3] * n[9]
            - n[8] * n[1] * n[7]
            + n[8] * n[3] * n[5];
        inv[15] = n[0] * n[5] * n[10] - n[0] * n[6] * n[9] - n[4] * n[1] * n[10]
            + n[4] * n[2] * n[9]
            + n[8] * n[1] * n[6]
            - n[8] * n[2] * n[5];

        let det = n[0] * inv[0] + n[1] * inv[4] + n[2] * inv[8] + n[3] * inv[12];
        if det.abs() < 1e-12 {
            return Self::IDENTITY;
        }
        let inv_det = 1.0 / det;
        for v in inv.iter_mut() {
            *v *= inv_det;
        }
        Self { m: inv }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: Mat4, b: Mat4, eps: f32) -> bool {
        a.m.iter().zip(b.m.iter()).all(|(x, y)| (x - y).abs() < eps)
    }

    #[test]
    fn identity_is_invertible_and_own_inverse() {
        assert!(approx_eq(Mat4::IDENTITY.invert(), Mat4::IDENTITY, 1e-6));
    }

    #[test]
    fn translation_round_trips() {
        let t = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let p = t.transform_point(Vec3::new(10.0, 20.0, 30.0));
        assert!((p.x - 11.0).abs() < 1e-5);
        assert!((p.y - 22.0).abs() < 1e-5);
        assert!((p.z - 33.0).abs() < 1e-5);

        let ti = t.invert();
        let p2 = ti.transform_point(p);
        assert!((p2.x - 10.0).abs() < 1e-4);
        assert!((p2.y - 20.0).abs() < 1e-4);
        assert!((p2.z - 30.0).abs() < 1e-4);
    }

    #[test]
    fn quat_identity_is_identity_matrix() {
        let m = Mat4::from_quaternion(Quat::IDENTITY);
        assert!(approx_eq(m, Mat4::IDENTITY, 1e-6));
    }

    #[test]
    fn quat_z_rotation_matches_manual() {
        let q = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let m = Mat4::from_quaternion(q);

        let p = m.transform_point(Vec3::new(1.0, 0.0, 0.0));
        assert!(p.x.abs() < 1e-5);
        assert!((p.y - 1.0).abs() < 1e-5);
        assert!(p.z.abs() < 1e-5);
    }

    #[test]
    fn trs_combines_translation_and_rotation() {
        let q = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let m = Mat4::from_trs(Vec3::new(5.0, 0.0, 0.0), q);

        let p = m.transform_point(Vec3::new(1.0, 0.0, 0.0));
        assert!((p.x - 5.0).abs() < 1e-5);
        assert!((p.y - 1.0).abs() < 1e-5);
        assert!(p.z.abs() < 1e-5);
    }

    #[test]
    fn mul_compose_order() {
        let t1 = Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0));
        let t2 = Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0));
        let composed = t1.mul(&t2);
        let p = composed.transform_point(Vec3::ZERO);
        assert!((p.x - 1.0).abs() < 1e-5);
        assert!((p.y - 2.0).abs() < 1e-5);
    }
}
