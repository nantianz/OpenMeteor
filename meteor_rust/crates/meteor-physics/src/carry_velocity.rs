use meteor_core::MODEL_SCALE;

pub const CARRY_DECAY_GROUND: f32 = 9.0;

pub const CARRY_MAX_SPEED: f32 = 600.0 * MODEL_SCALE;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CarryVelocity {
    pub x: f32,
    pub z: f32,
}

impl CarryVelocity {
    pub fn new(x: f32, z: f32) -> Self {
        Self { x, z }
    }

    pub fn is_nonzero(&self) -> bool {
        self.x.abs() > 1e-5 || self.z.abs() > 1e-5
    }

    pub fn clear(&mut self) {
        self.x = 0.0;
        self.z = 0.0;
    }

    pub fn scale(&mut self, factor: f32) {
        if factor.is_finite() {
            self.x *= factor;
            self.z *= factor;
        }
    }
}

pub fn carry_decay_factor(dt: f32) -> f32 {
    let step = if dt.is_finite() { dt } else { 0.0 };
    (1.0 - CARRY_DECAY_GROUND * step).max(0.0)
}

pub fn decay_carry(vel: CarryVelocity, dt: f32) -> CarryVelocity {
    let k = carry_decay_factor(dt);
    CarryVelocity {
        x: vel.x * k,
        z: vel.z * k,
    }
}

pub fn clamp_composed_velocity_3d(
    vx: f32,
    vy: f32,
    vz: f32,
    max_speed: f32,
) -> (f32, f32, f32, f32) {
    let vx = if vx.is_finite() { vx } else { 0.0 };
    let vy = if vy.is_finite() { vy } else { 0.0 };
    let vz = if vz.is_finite() { vz } else { 0.0 };
    let speed = (vx * vx + vy * vy + vz * vz).sqrt();
    if speed.partial_cmp(&max_speed) != Some(std::cmp::Ordering::Greater)
        || max_speed.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
    {
        return (vx, vy, vz, 1.0);
    }
    let scale = max_speed / speed;
    (vx * scale, vy * scale, vz * scale, scale)
}

pub fn clamp_composed_delta_3d(
    dx: f32,
    dy: f32,
    dz: f32,
    dt: f32,
    max_speed: f32,
) -> (f32, f32, f32, f32) {
    let step = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
    if step <= 0.0 {
        return (dx, dy, dz, 1.0);
    }
    let (vx, vy, vz, scale) =
        clamp_composed_velocity_3d(dx / step, dy / step, dz / step, max_speed);
    (vx * step, vy * step, vz * step, scale)
}

pub fn bank_carry_velocity(
    carry: &mut CarryVelocity,
    composed: CarryVelocity,
    locally_driven: bool,
) -> Option<CarryVelocity> {
    if !locally_driven {
        carry.clear();
        return None;
    }
    *carry = composed;
    Some(*carry)
}

pub fn advance_carry(carry: &mut CarryVelocity, dt: f32) -> (f32, f32) {
    *carry = decay_carry(*carry, dt);
    (carry.x * dt, carry.z * dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_exe() {
        assert_eq!(CARRY_DECAY_GROUND, 9.0);
        assert_eq!(CARRY_MAX_SPEED, 600.0 * MODEL_SCALE);
        assert!((CARRY_MAX_SPEED - 72.0).abs() < 1e-5);
    }

    #[test]
    fn decay_factor_linear() {
        assert!((carry_decay_factor(0.0) - 1.0).abs() < 1e-6);
        assert!((carry_decay_factor(1.0 / 30.0) - (1.0 - 9.0 / 30.0)).abs() < 1e-6);

        assert_eq!(carry_decay_factor(1.0), 0.0);

        assert!((carry_decay_factor(f32::NAN) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn decay_reduces_velocity() {
        let vel = CarryVelocity::new(12.0, 6.0);
        let decayed = decay_carry(vel, 1.0 / 30.0);
        assert!(decayed.x < 12.0);
        assert!(decayed.x > 0.0);

        assert!((decayed.z / decayed.x - 0.5).abs() < 1e-5);
    }

    #[test]
    fn clamp_velocity_by_magnitude() {
        let (x, _, _, scale) = clamp_composed_velocity_3d(100.0, 0.0, 0.0, 50.0);
        assert!((x - 50.0).abs() < 1e-5);
        assert!((scale - 0.5).abs() < 1e-5);

        let (x, _, _, scale) = clamp_composed_velocity_3d(10.0, 0.0, 0.0, 50.0);
        assert!((x - 10.0).abs() < 1e-5);
        assert_eq!(scale, 1.0);

        let (x, y, _, scale) = clamp_composed_velocity_3d(30.0, 40.0, 0.0, 40.0);
        assert!((x - 24.0).abs() < 1e-5);
        assert!((y - 32.0).abs() < 1e-5);
        assert!((scale - 0.8).abs() < 1e-5);
    }

    #[test]
    fn clamp_delta_roundtrip() {
        let (dx, _, _, scale) = clamp_composed_delta_3d(2.0, 0.0, 0.0, 1.0 / 30.0, CARRY_MAX_SPEED);
        assert!((dx - 2.0).abs() < 1e-5);
        assert_eq!(scale, 1.0);

        let (dx, _, _, scale) = clamp_composed_delta_3d(4.0, 0.0, 0.0, 1.0 / 30.0, CARRY_MAX_SPEED);
        assert!((dx - 2.4).abs() < 1e-5);
        assert!((scale - 0.6).abs() < 1e-5);
    }

    #[test]
    fn bank_copies_composed() {
        let mut carry = CarryVelocity::default();
        let composed = CarryVelocity::new(12.0, 0.0);
        let banked = bank_carry_velocity(&mut carry, composed, true).unwrap();
        assert!((banked.x - 12.0).abs() < 1e-5);

        let mut carry = CarryVelocity::new(5.0, 5.0);
        assert!(bank_carry_velocity(&mut carry, composed, false).is_none());
        assert_eq!(carry, CarryVelocity::default());
    }

    #[test]
    fn advance_moves_by_residual() {
        let mut carry = CarryVelocity::new(12.0, 0.0);
        let (dx, dz) = advance_carry(&mut carry, 1.0 / 30.0);
        assert!(dx > 0.0);
        assert!(dx < 12.0 / 30.0);
        assert_eq!(dz, 0.0);

        assert!(carry.x < 12.0);
    }

    #[test]
    fn nonlocal_clears_each_tick() {
        let mut carry = CarryVelocity::new(8.0, 0.0);
        bank_carry_velocity(&mut carry, CarryVelocity::new(8.0, 0.0), false);
        assert!(!carry.is_nonzero());
    }
}
