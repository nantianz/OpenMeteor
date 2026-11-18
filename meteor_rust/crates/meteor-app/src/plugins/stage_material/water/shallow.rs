use super::profiles::Shallow;
use bevy::prelude::*;

pub(super) struct ShallowFlow {
    pub settings: Shallow,
    rest: Vec<f32>,
    flow: Vec<Vec2>,
}

impl ShallowFlow {
    pub(super) fn new(rest: Vec<f32>, settings: Shallow) -> Self {
        let flow = vec![Vec2::ZERO; rest.len()];
        Self {
            settings,
            rest,
            flow,
        }
    }

    pub(super) fn clear(&mut self) {
        self.flow.fill(Vec2::ZERO);
    }

    pub(super) fn step(
        &mut self,
        width: usize,
        spacing: f32,
        dt: f32,
        height: &[f32],
        next: &mut [f32],
    ) -> f32 {
        next.copy_from_slice(height);
        let scale = dt / spacing;
        let drag = 1.0 / (1.0 + self.settings.drag * dt);
        let velocity_limit = (0.4 / scale
            - (self.settings.gravity * (self.settings.max_depth + 1.0)).sqrt())
        .max(0.25);
        let mut peak = 0.0_f32;
        for i in width..height.len() - width {
            if self.rest[i] == 0.0 {
                continue;
            }
            for (axis, j) in [i + 1, i + width].into_iter().enumerate() {
                if self.rest[j] == 0.0 {
                    continue;
                }
                let common_depth = self.rest[i].min(self.rest[j]);
                let velocity = ((self.flow[i][axis]
                    - self.settings.gravity * scale * (height[j] - height[i]))
                    * drag)
                    .clamp(-velocity_limit, velocity_limit);
                self.flow[i][axis] = velocity;
                let donor = if velocity >= 0.0 { i } else { j };
                let column = (common_depth + height[donor]).max(0.0);
                let available = (self.rest[donor] + height[donor]).max(0.0);
                let volume = scale * velocity.abs() * column;
                let flux = volume.min(available * 0.24).copysign(velocity);
                next[i] -= flux;
                next[j] += flux;
                peak = peak.max(velocity.abs());
            }
        }
        peak
    }

    pub(super) fn displace(&self, height: &mut [f32], footprint: &[(usize, f32)], mean: f32) {
        let mut factor = 1.0_f32;
        for &(i, v) in footprint {
            let delta = v - mean;
            if delta < 0.0 {
                factor = factor.min((self.rest[i] + height[i]).max(0.0) * 0.75 / -delta);
            }
        }
        for &(i, v) in footprint {
            height[i] += factor * (v - mean);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flux_preserves_lake_at_rest_and_conserves_a_pulse_across_a_depth_step() {
        let width = 12;
        let mut rest = vec![0.; width * 8];
        for y in 1..7 {
            for x in 1..width - 1 {
                rest[y * width + x] = if x < width / 2 { 0.12 } else { 1.8 };
            }
        }
        let settings = super::super::config::profiles().shallow;
        let mut flow = ShallowFlow::new(rest.clone(), settings);
        let mut h = vec![0.; rest.len()];
        let mut next = h.clone();
        assert_eq!(flow.step(width, 0.4, 1. / 120., &h, &mut next), 0.);
        assert_eq!(h, next);
        h[3 * width + 3] = 0.06;
        let before = h.iter().sum::<f32>();
        let mut crossed = false;
        for _ in 0..600 {
            flow.step(width, 0.4, 1. / 120., &h, &mut next);
            std::mem::swap(&mut h, &mut next);
            crossed |= h[3 * width + 8].abs() > 0.00001;
            assert!(h
                .iter()
                .zip(&rest)
                .all(|(a, b)| a.is_finite() && a + b >= 0. && (*b > 0. || *a == 0.)));
        }
        assert!(crossed);
        assert!((h.iter().sum::<f32>() - before).abs() < 1e-5);
    }
}
