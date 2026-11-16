use bevy::prelude::*;

pub(super) const GRAVITY: f32 = -meteor_physics::jump::GRAVITY;
const MAX_PARTICLES: usize = 256;

#[derive(Clone, Copy, Debug)]
pub(super) struct Impact {
    pub velocity: Vec3,
    pub radius: f32,
    pub depth: f32,
}
impl Impact {
    fn speed_squared(self) -> f32 {
        self.velocity.y.powi(2) + self.velocity.xz().length_squared() * 0.35
    }
    fn volume(self) -> f32 {
        std::f32::consts::PI * self.radius.powi(2) * self.depth
    }
    pub fn wave_strength(self) -> f32 {
        (self.volume() * self.speed_squared().sqrt() * 0.65).min(2.6)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Particle {
    pub origin: Vec3,
    pub velocity: Vec3,
    pub radius: f32,
    pub birth: f32,
    pub landing: f32,
    drag: f32,
}
impl Particle {
    pub fn volume(&self) -> f32 {
        self.radius.powi(3) * (4.0 * std::f32::consts::PI / 3.0)
    }
    pub fn at(&self, age: f32) -> (Vec3, Vec3) {
        let t = (age - self.birth).max(0.0);
        let decay = (-self.drag * t).exp();
        let terminal = Vec3::NEG_Y * (GRAVITY / self.drag);
        (
            self.origin + (self.velocity - terminal) * ((1.0 - decay) / self.drag) + terminal * t,
            (self.velocity - terminal) * decay + terminal,
        )
    }
    pub fn visible(&self, age: f32) -> bool {
        self.birth <= age && age < self.landing
    }
}

#[derive(Debug)]
pub(super) struct Spray {
    pub center: Vec3,
    pub particles: Vec<Particle>,
}
fn random(mut x: u32) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^= x >> 16;
    (x >> 8) as f32 / 16_777_216.0
}
impl Spray {
    pub fn new(center: Vec3, impact: Impact, gain: f32, seed: u32) -> Self {
        let mut result = Self {
            center,
            particles: Vec::new(),
        };
        if !impact.velocity.is_finite()
            || !impact.radius.is_finite()
            || !impact.depth.is_finite()
            || impact.radius <= 0.0
            || impact.depth <= 0.0
            || gain <= 0.0
        {
            return result;
        }
        let speed2 = impact.speed_squared();

        let ejected = impact.volume() * 0.025 * (1.0 - (-speed2 / 180.0).exp()) * gain;
        if speed2 < 4.0 || ejected < 0.000002 {
            return result;
        }
        let count = ((ejected / 0.000008).ceil() as usize).clamp(1, MAX_PARTICLES);
        let weights: Vec<_> = (0..count)
            .map(|i| 0.15 + random(seed.wrapping_add(i as u32 * 47)).powi(3) * 2.8)
            .collect();
        let total: f32 = weights.iter().sum();
        let tangent = impact.velocity.with_y(0.0);
        let down = (-impact.velocity.y).max(0.0);
        let lifted = impact.velocity.y.max(0.0);
        let horizontal = tangent.length();
        for (i, weight) in weights.into_iter().enumerate() {
            let r = |offset| random(seed.wrapping_add(i as u32 * 137 + offset));
            let angle = r(19) * std::f32::consts::TAU;
            let radial = Vec3::new(angle.cos(), 0.0, angle.sin());
            let radius = (ejected * weight / total * 3.0 / (4.0 * std::f32::consts::PI)).cbrt();
            let birth = r(23) * 0.035;
            let velocity = tangent * (0.18 + r(31) * 0.22)
                + radial * (down * (0.12 + r(37) * 0.22) + horizontal * 0.08)
                + Vec3::Y
                    * (down * (0.28 + r(41) * 0.30)
                        + horizontal * (0.16 + r(43) * 0.22)
                        + lifted * (0.45 + r(59) * 0.15));
            result.particles.push(Particle {
                origin: center
                    + radial * impact.radius * r(53).sqrt()
                    + tangent * birth * 0.35
                    + Vec3::Y * radius,
                velocity,
                radius,
                birth,
                landing: 0.0,
                drag: 0.4 + 0.012 / radius.max(0.002),
            });
        }

        let energy: f32 = result
            .particles
            .iter()
            .map(|p| 0.5 * p.volume() * p.velocity.length_squared())
            .sum();
        let budget = 0.5 * ejected * speed2 * 0.45;
        let scale = (budget / energy.max(1e-10)).min(1.0).sqrt();
        for p in &mut result.particles {
            p.velocity *= scale;
            let (mut lo, mut hi) = (p.birth, p.birth + 3.0);
            for _ in 0..22 {
                let mid = (lo + hi) * 0.5;
                if p.at(mid).0.y > center.y {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            p.landing = (lo + hi) * 0.5;
        }
        result
    }
    pub fn returns(&self, before: f32, after: f32) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.particles
            .iter()
            .filter(move |p| before < p.landing && p.landing <= after)
            .map(|p| {
                let (mut position, velocity) = p.at(p.landing);
                position.y = self.center.y;
                (position, p.volume() * (-velocity.y).max(0.0) * 2.0)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn impact(speed: f32) -> Impact {
        Impact {
            velocity: Vec3::new(speed, -speed, 0.0),
            radius: 0.4,
            depth: 0.3,
        }
    }
    #[test]
    fn measured_speed_controls_volume_height_and_range_without_energy_gain() {
        let mut previous = (0.0, 0.0, 0.0);
        assert!(Spray::new(Vec3::ZERO, impact(0.0), 1.0, 71)
            .particles
            .is_empty());
        for speed in [4.0, 10.0, 20.0, 40.0] {
            let i = impact(speed);
            let s = Spray::new(Vec3::ZERO, i, 1.0, 71);
            let volume: f32 = s.particles.iter().map(Particle::volume).sum();
            let energy: f32 = s
                .particles
                .iter()
                .map(|p| p.volume() * p.velocity.length_squared() * 0.5)
                .sum();
            assert!(energy <= 0.5 * volume * i.speed_squared() * 0.4501);
            let height = s
                .particles
                .iter()
                .map(|p| {
                    (0..100)
                        .map(|n| p.at(p.birth + (p.landing - p.birth) * n as f32 / 100.0).0.y)
                        .fold(0.0, f32::max)
                })
                .fold(0.0, f32::max);
            let range = s
                .particles
                .iter()
                .map(|p| p.at(p.landing).0.xz().distance(p.origin.xz()))
                .fold(0.0, f32::max);
            eprintln!(
                "WATER_SPEED speed={speed:.1} volume={volume:.6} apex={height:.4} range={range:.4} particles={}",
                s.particles.len()
            );
            assert!(
                volume > previous.0 && height > previous.1 && range > previous.2,
                "speed={speed}, volume={volume}, height={height}, range={range}, previous={previous:?}"
            );
            previous = (volume, height, range);
            assert!(s.particles.iter().all(|p| p.radius < 0.055));
        }
    }
    #[test]
    fn upward_weapon_lifts_water_instead_of_being_treated_as_zero_speed() {
        let s = Spray::new(
            Vec3::ZERO,
            Impact {
                velocity: Vec3::Y * 25.0,
                radius: 0.15,
                depth: 0.2,
            },
            1.0,
            71,
        );
        assert!(!s.particles.is_empty());
        assert!(s.particles.iter().all(|p| p.velocity.y > 8.0));
    }
    #[test]
    fn drag_landing_and_return_momentum_are_frame_rate_independent() {
        let s = Spray::new(Vec3::ZERO, impact(30.0), 1.0, 71);
        for fps in [30, 60, 120] {
            let returned: usize = (0..fps * 3)
                .map(|n| {
                    s.returns(n as f32 / fps as f32, (n + 1) as f32 / fps as f32)
                        .count()
                })
                .sum();
            assert_eq!(returned, s.particles.len());
        }
        for p in &s.particles {
            assert!(p.at(p.landing).0.y.abs() < 0.0001);
            assert!(p.at(p.birth + 0.1).1.x.abs() < p.velocity.x.abs());
        }
    }
}
