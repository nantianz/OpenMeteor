use bevy::prelude::*;

const STEP: f32 = 1.0 / 120.0;

pub(super) struct WaveField {
    origin: Vec2,
    spacing: f32,
    width: usize,
    height: usize,
    wet: Vec<bool>,
    triangles: Vec<[Vec2; 3]>,
    displacement: Vec<f32>,
    velocity: Vec<f32>,
    next: Vec<f32>,
    remainder: f32,
    awake: bool,
    foam: Vec<f32>,
    active_lo: UVec2,
    active_hi: UVec2,
    pub(super) dirty: bool,
    shallow: Option<super::shallow::ShallowFlow>,
}

fn contains(p: Vec2, [a, b, c]: [Vec2; 3]) -> bool {
    let u = b - a;
    let v = c - a;
    let det = u.perp_dot(v);
    if det.abs() < 1e-6 {
        return false;
    }
    let s = (p - a).perp_dot(v) / det;
    let t = u.perp_dot(p - a) / det;
    s >= -1e-5 && t >= -1e-5 && s + t <= 1.00001
}

impl WaveField {
    pub(super) fn new(triangles: &[[Vec2; 3]]) -> Self {
        Self::grid(triangles, None)
    }

    fn grid(triangles: &[[Vec2; 3]], shallow: Option<super::profiles::Shallow>) -> Self {
        let config = &super::config::get().simulation;
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        for &p in triangles.iter().flatten() {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let max_side = shallow.map_or(config.max_side, |s| s.max_grid_side.min(config.max_side));
        let min_spacing = shallow.map_or(config.min_spacing, |s| {
            config
                .min_spacing
                .max(STEP * ((s.gravity * (s.max_depth + 1.0)).sqrt() + 2.5) / 0.35)
        });
        let spacing = ((hi - lo).max_element() / (max_side - 3) as f32).max(min_spacing);
        let origin = lo - Vec2::splat(spacing);
        let size = ((hi - lo) / spacing).ceil().as_uvec2() + UVec2::splat(3);
        let (width, height) = (size.x as usize, size.y as usize);
        let mut wet = vec![false; width * height];
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let p = origin + Vec2::new(x as f32, y as f32) * spacing;
                wet[y * width + x] = triangles.iter().any(|&t| contains(p, t));
            }
        }
        Self {
            origin,
            spacing,
            width,
            height,
            wet,
            triangles: triangles.to_vec(),
            displacement: vec![0.0; width * height],
            velocity: vec![0.0; width * height],
            next: vec![0.0; width * height],
            remainder: 0.0,
            awake: false,
            foam: vec![0.0; width * height],
            active_lo: UVec2::new(width as u32, height as u32),
            active_hi: UVec2::ZERO,
            dirty: true,
            shallow: None,
        }
    }

    pub(super) fn use_shallow(&mut self, depth_at: impl Fn(Vec2) -> Option<f32>) {
        let settings = super::config::profiles().shallow;
        *self = Self::grid(&self.triangles, Some(settings));
        let rest = self
            .wet
            .iter_mut()
            .enumerate()
            .map(|(i, wet)| {
                if !*wet {
                    return 0.0;
                }
                let cell = Vec2::new((i % self.width) as f32, (i / self.width) as f32);
                let depth =
                    depth_at(self.origin + cell * self.spacing).unwrap_or(settings.fallback_depth);
                if depth <= 0.0 {
                    *wet = false;
                    return 0.0;
                }
                depth.clamp(settings.min_depth, settings.max_depth)
            })
            .collect();
        self.shallow = Some(super::shallow::ShallowFlow::new(rest, settings));
    }

    pub(super) fn clear(&mut self) {
        self.displacement.fill(0.0);
        self.velocity.fill(0.0);
        self.next.fill(0.0);
        self.remainder = 0.0;
        self.awake = false;
        self.foam.fill(0.0);
        self.active_lo = UVec2::new(self.width as u32, self.height as u32);
        self.active_hi = UVec2::ZERO;
        self.dirty = true;
        if let Some(flow) = &mut self.shallow {
            flow.clear();
        }
    }

    #[cfg(test)]
    pub(super) fn impulse(&mut self, point: Vec2, strength: f32, direction: Vec2) {
        self.impulse_at_radius(point, strength, direction, 1.2);
    }

    pub(super) fn impulse_at_radius(
        &mut self,
        point: Vec2,
        strength: f32,
        direction: Vec2,
        radius: f32,
    ) {
        if !self.triangles.iter().any(|&t| contains(point, t)) {
            return;
        }
        let grid = (point - self.origin) / self.spacing;

        let lower = grid.floor().as_uvec2();
        if ![UVec2::ZERO, UVec2::X, UVec2::Y, UVec2::ONE]
            .into_iter()
            .map(|offset| lower + offset)
            .any(|node| {
                node.x < self.width as u32
                    && node.y < self.height as u32
                    && self.wet[node.y as usize * self.width + node.x as usize]
            })
        {
            return;
        }
        let radius = radius.max(self.spacing * 1.2);
        let reach = (radius * 3.0 / self.spacing).ceil() as isize;
        let along = direction.normalize_or_zero();
        let mut footprint = Vec::new();
        for y in (grid.y as isize - reach).max(1)
            ..=(grid.y as isize + reach).min(self.height as isize - 2)
        {
            for x in (grid.x as isize - reach).max(1)
                ..=(grid.x as isize + reach).min(self.width as isize - 2)
            {
                let i = y as usize * self.width + x as usize;
                if !self.wet[i] {
                    continue;
                }
                let d = self.origin + Vec2::new(x as f32, y as f32) * self.spacing - point;
                let q = (d.length_squared() - 0.45 * d.dot(along).powi(2)) / (radius * radius);
                self.active_lo = self.active_lo.min(UVec2::new(x as u32, y as u32));
                self.active_hi = self.active_hi.max(UVec2::new(x as u32, y as u32));
                self.foam[i] = (self.foam[i]
                    + strength * (-q * 1.4).exp() * super::config::get().simulation.foam_gain)
                    .min(1.0);
                footprint.push((
                    i,
                    strength.clamp(0.0, 2.6)
                        * super::config::get().simulation.impulse_gain
                        * (q - 1.0)
                        * (-q).exp(),
                ));
            }
        }

        self.awake |= !footprint.is_empty();
        let mean = footprint.iter().map(|(_, v)| v).sum::<f32>() / footprint.len().max(1) as f32;
        if let Some(flow) = &self.shallow {
            for (_, v) in &mut footprint {
                *v *= STEP;
            }
            flow.displace(&mut self.displacement, &footprint, mean * STEP);
            return;
        }
        for (i, velocity) in footprint {
            self.velocity[i] = (self.velocity[i] + velocity - mean).clamp(-6.0, 6.0);
        }
    }

    pub(super) fn advance(&mut self, dt: f32) {
        if !self.awake {
            return;
        }

        self.remainder += dt.clamp(0.0, 0.1);
        self.dirty = true;
        let config = &super::config::get().simulation;
        if let Some(flow) = &mut self.shallow {
            while self.remainder + 1e-7 >= STEP {
                self.remainder = (self.remainder - STEP).max(0.0);
                let mut peak = flow.step(
                    self.width,
                    self.spacing,
                    STEP,
                    &self.displacement,
                    &mut self.next,
                );
                std::mem::swap(&mut self.displacement, &mut self.next);
                for (height, foam) in self.displacement.iter().zip(&mut self.foam) {
                    *foam *= 1.0 - STEP * config.foam_decay;
                    peak = peak.max(height.abs()).max(*foam);
                }
                if peak < 1e-4 {
                    self.clear();
                    break;
                }
            }
            return;
        }
        let k = config.speed * config.speed / (self.spacing * self.spacing);
        while self.remainder + 1e-7 >= STEP {
            self.remainder = (self.remainder - STEP).max(0.0);
            let mut peak = 0.0_f32;
            let lo = (self.active_lo.max(UVec2::ONE) - UVec2::ONE).max(UVec2::ONE);
            let hi = (self.active_hi + UVec2::ONE)
                .min(UVec2::new(self.width as u32 - 2, self.height as u32 - 2));
            self.active_lo = UVec2::new(self.width as u32, self.height as u32);
            self.active_hi = UVec2::ZERO;
            self.next.fill(0.0);
            for y in lo.y as usize..=hi.y as usize {
                for x in lo.x as usize..=hi.x as usize {
                    let i = y * self.width + x;
                    if !self.wet[i] {
                        continue;
                    }
                    let h = self.displacement[i];
                    let mut laplacian = 0.0;
                    for j in [i - 1, i + 1, i - self.width, i + self.width] {
                        laplacian += if self.wet[j] {
                            self.displacement[j] - h
                        } else {
                            0.0
                        };
                    }
                    self.velocity[i] =
                        (self.velocity[i] + STEP * k * laplacian) / (1.0 + config.damping * STEP);
                    let h_next = h + STEP * self.velocity[i];
                    peak = peak.max(h_next.abs()).max(self.velocity[i].abs());
                    self.next[i] = h_next.clamp(-0.8, 0.8);
                    self.foam[i] *= 1.0 - STEP * config.foam_decay;
                    let energy = self.next[i]
                        .abs()
                        .max(self.velocity[i].abs())
                        .max(self.foam[i]);
                    peak = peak.max(energy);
                    if energy > 1e-5 {
                        self.active_lo = self.active_lo.min(UVec2::new(x as u32, y as u32));
                        self.active_hi = self.active_hi.max(UVec2::new(x as u32, y as u32));
                    } else {
                        self.next[i] = 0.0;
                        self.velocity[i] = 0.0;
                        self.foam[i] = 0.0;
                    }
                    if h_next != self.next[i] {
                        self.velocity[i] = 0.0;
                    }
                }
            }
            std::mem::swap(&mut self.displacement, &mut self.next);
            if peak < 1e-4 {
                self.clear();
                break;
            }
        }
    }

    pub(super) fn layout(&self) -> (u32, u32, Vec4) {
        (
            self.width as u32,
            self.height as u32,
            Vec4::new(
                self.origin.x - self.spacing * 0.5,
                self.origin.y - self.spacing * 0.5,
                self.width as f32 * self.spacing,
                self.height as f32 * self.spacing,
            ),
        )
    }

    pub(super) fn pixels(&self) -> Vec<u8> {
        let mut out = [128, 128, 0, 255].repeat(self.width * self.height);
        if !self.awake {
            return out;
        }
        let last = UVec2::new(self.width as u32 - 1, self.height as u32 - 1);
        let (lo, hi) = if self.shallow.is_some() {
            (UVec2::ZERO, last)
        } else {
            (
                self.active_lo.max(UVec2::ONE) - UVec2::ONE,
                (self.active_hi + UVec2::ONE).min(last),
            )
        };
        for y in lo.y as usize..=hi.y as usize {
            for x in lo.x as usize..=hi.x as usize {
                let i = y * self.width + x;
                let h = |nx: usize, ny: usize| {
                    let j = ny * self.width + nx;
                    if self.wet[j] {
                        self.displacement[j]
                    } else {
                        self.displacement[i]
                    }
                };
                let dx = (h((x + 1).min(self.width - 1), y) - h(x.saturating_sub(1), y))
                    / (2.0 * self.spacing);
                let dz = (h(x, (y + 1).min(self.height - 1)) - h(x, y.saturating_sub(1)))
                    / (2.0 * self.spacing);
                out[i * 4] = (128.0 + dx * 64.0).round().clamp(0.0, 255.0) as u8;
                out[i * 4 + 1] = (128.0 + dz * 64.0).round().clamp(0.0, 255.0) as u8;
                out[i * 4 + 2] = (self.foam[i] * 255.0).clamp(0.0, 255.0) as u8;
            }
        }
        out
    }

    fn height_at(&self, point: Vec2) -> f32 {
        let p = (point - self.origin) / self.spacing;
        if p.x < 0.0
            || p.y < 0.0
            || p.x >= (self.width - 1) as f32
            || p.y >= (self.height - 1) as f32
        {
            return 0.0;
        }
        let (x, y) = (p.x as usize, p.y as usize);
        let f = p.fract();
        let i = y * self.width + x;
        let a = self.displacement[i] * (1.0 - f.x) + self.displacement[i + 1] * f.x;
        let b = self.displacement[i + self.width] * (1.0 - f.x)
            + self.displacement[i + self.width + 1] * f.x;
        a * (1.0 - f.y) + b * f.y
    }

    pub(super) fn sample(&self, point: Vec2) -> (f32, Vec2) {
        let dx = Vec2::X * self.spacing;
        let dy = Vec2::Y * self.spacing;
        (
            self.height_at(point),
            Vec2::new(
                self.height_at(point + dx) - self.height_at(point - dx),
                self.height_at(point + dy) - self.height_at(point - dy),
            ) / (2.0 * self.spacing),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pool() -> WaveField {
        WaveField::new(&[
            [Vec2::ZERO, Vec2::new(30.0, 0.0), Vec2::new(30.0, 30.0)],
            [Vec2::ZERO, Vec2::new(30.0, 30.0), Vec2::new(0.0, 30.0)],
        ])
    }
    #[test]
    fn impulse_propagates_decays_and_stays_inside_water() {
        let mut f = pool();
        f.impulse(Vec2::splat(15.0), 1.0, Vec2::X);
        assert_eq!(f.sample(Vec2::new(20.0, 15.0)).0, 0.0);
        for _ in 0..60 {
            f.advance(1.0 / 60.0);
        }
        assert!(f.sample(Vec2::new(20.0, 15.0)).0.abs() > 0.002);
        assert_eq!(f.sample(Vec2::new(-2.0, 15.0)).0, 0.0);
        let peak = f
            .displacement
            .iter()
            .copied()
            .map(f32::abs)
            .fold(0.0, f32::max);
        for _ in 0..1200 {
            f.advance(1.0 / 60.0);
        }
        let settled = f
            .displacement
            .iter()
            .copied()
            .map(f32::abs)
            .fold(0.0, f32::max);
        assert!(settled < peak * 0.1, "{peak} -> {settled}");
    }
    #[test]
    fn fixed_steps_match_frame_rates_and_stalls_are_bounded() {
        let (mut a, mut b) = (pool(), pool());
        for f in [&mut a, &mut b] {
            f.impulse(Vec2::splat(15.0), 0.7, Vec2::ZERO);
        }
        for _ in 0..30 {
            a.advance(1.0 / 30.0);
        }
        for _ in 0..120 {
            b.advance(1.0 / 120.0);
        }
        for (a, b) in a.displacement.iter().zip(&b.displacement) {
            assert!((a - b).abs() < 1e-5);
        }
        for _ in 0..100 {
            a.impulse(Vec2::splat(15.0), 1.2, Vec2::Y);
            a.advance(3.0);
        }
        assert!(a
            .displacement
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 0.8));
        a.clear();
        assert!(a.displacement.iter().chain(&a.velocity).all(|&v| v == 0.0));
    }
    #[test]
    fn bank_impulses_conserve_volume_and_waves_superpose() {
        let (mut a, mut b, mut both) = (pool(), pool(), pool());
        let bank = Vec2::new(0.6, 15.0);
        a.impulse(bank, 0.3, Vec2::Y);
        assert!(a.velocity.iter().sum::<f32>().abs() < 1e-4);
        b.impulse(Vec2::splat(15.0), 0.4, Vec2::X);
        both.impulse(bank, 0.3, Vec2::Y);
        both.impulse(Vec2::splat(15.0), 0.4, Vec2::X);
        for _ in 0..120 {
            a.advance(STEP);
            b.advance(STEP);
            both.advance(STEP);
        }
        for i in 0..both.displacement.len() {
            assert!((a.displacement[i] + b.displacement[i] - both.displacement[i]).abs() < 1e-4);
        }
    }

    #[test]
    fn wet_bank_contact_between_cells_produces_a_texture_disturbance() {
        let mut field = WaveField::new(&[[Vec2::ZERO, Vec2::X * 4.15, Vec2::Y * 4.15]]);
        let mut contact = None;
        for y in 1..field.height - 1 {
            for x in 1..field.width - 1 {
                if field.wet[y * field.width + x] {
                    continue;
                }
                let center = field.origin + Vec2::new(x as f32, y as f32) * field.spacing;
                let point = center - Vec2::splat(0.49 * field.spacing);
                if contains(point, field.triangles[0]) {
                    contact = Some(point);
                    break;
                }
            }
        }
        let neutral = field.pixels();
        field.impulse(
            contact.expect("wet point whose nearest grid cell is dry"),
            0.8,
            Vec2::X,
        );
        field.advance(STEP);
        assert_ne!(field.pixels(), neutral);
        field.clear();
        assert_eq!(field.pixels(), neutral);
    }
}
