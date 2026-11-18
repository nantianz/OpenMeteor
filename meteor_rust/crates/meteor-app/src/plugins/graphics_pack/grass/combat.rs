use super::interaction::{BendMap, RANGE, SPACING};
use bevy::prelude::*;
use meteor_combat::hitbox::Obb;
use std::collections::HashMap;

const MAX_BOX_HISTORY: usize = 256;
const MAX_PENDING: usize = 48;
const MAX_PATCHES: usize = 12;
const MAX_GROUND_SAMPLES: usize = 128;
const GROUND_CACHE_TICKS: u64 = 8;

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
struct BoxKey(Entity, u32, u32, usize);

struct Sample {
    bottom: Vec3,
    time: f64,
    seen: f64,
}

#[derive(Clone, Copy)]
struct Stroke {
    start: Vec3,
    end: Vec3,
    radius: f32,
    strength: f32,
    extra_height: f32,
    headroom: f32,
    downward: bool,
}

struct GroundSample {
    height: f32,
    tick: u64,
}

#[derive(Resource, Default)]
pub(crate) struct GrassCombat {
    eye: Option<Vec2>,
    history: HashMap<BoxKey, Sample>,
    pending: Vec<Stroke>,
    latest_sample: f64,
    ground: HashMap<(IVec3, i32), GroundSample>,
    tick: u64,
}

impl GrassCombat {
    pub(crate) fn set_eye(&mut self, eye: Option<Vec2>) {
        if self.eye.is_some_and(|old| {
            eye.is_none_or(|new| old.distance_squared(new) > (2.0 * RANGE).powi(2))
        }) {
            self.pending.clear();
            self.history.clear();
            self.invalidate_ground();
        }
        self.eye = eye;
    }

    pub(super) fn invalidate_ground(&mut self) {
        self.ground.clear();
    }

    pub(crate) fn record_boxes(
        &mut self,
        actor: Entity,
        pose_instance: u32,
        window: u32,
        time: f64,
        effect: bool,
        boxes: &[Obb],
    ) {
        let Some(eye) = self.eye else { return };
        self.latest_sample = time;
        for (index, bounds) in boxes.iter().take(16).enumerate() {
            let key = BoxKey(actor, pose_instance, window, index);
            let center = Vec3::new(bounds.center.x, bounds.center.y, bounds.center.z);
            let axes = bounds.axes;
            let half = bounds.half_size;
            let extent =
                |x: f32, y: f32, z: f32| x.abs() * half.x + y.abs() * half.y + z.abs() * half.z;
            let height = extent(axes[0].y, axes[1].y, axes[2].y);
            let bottom = center - Vec3::Y * height;
            if !bottom.is_finite() || (bottom.xz() - eye).abs().max_element() > RANGE + 8.0 {
                continue;
            }
            if self.history.len() == MAX_BOX_HISTORY && !self.history.contains_key(&key) {
                continue;
            }
            let mut start = bottom;
            let mut elapsed = 0.0;
            match self.history.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let previous = entry.get_mut();
                    let age = time - previous.time;
                    let distance = previous.bottom.distance_squared(bottom);
                    if age <= 0.0 {
                        continue;
                    }
                    let recent = time - previous.seen <= 0.2;
                    previous.seen = time;
                    if recent && distance < 0.04 * 0.04 {
                        continue;
                    }
                    if recent && age <= 0.2 && distance <= 12.0 * 12.0 {
                        start = previous.bottom;
                        elapsed = age as f32;
                    }
                    *previous = Sample {
                        bottom,
                        time,
                        seen: time,
                    };
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(Sample {
                        bottom,
                        time,
                        seen: time,
                    });
                }
            }
            let travel = bottom - start;
            let downward = -travel.y > elapsed * 5.0 && -travel.y > travel.xz().length();
            let width = extent(axes[0].x, axes[1].x, axes[2].x)
                .min(extent(axes[0].z, axes[1].z, axes[2].z));
            self.push(Stroke {
                start,
                end: bottom,
                radius: (0.52 + width * 0.65).clamp(0.6, 2.1),
                strength: if effect { 0.86 } else { 0.76 },
                extra_height: if effect { 0.9 } else { 0.0 },
                headroom: (height * 2.0).clamp(0.55, 4.0),
                downward: downward || (effect && width >= 1.2 && travel.length_squared() < 0.04),
            });
        }
    }

    pub(crate) fn record_segment(&mut self, start: Vec3, end: Vec3, width: f32) {
        self.push(Stroke {
            start,
            end,
            radius: (0.52 + width).clamp(0.55, 1.45),
            strength: 0.72,
            extra_height: 0.65,
            headroom: 0.55,
            downward: false,
        });
    }

    fn push(&mut self, stroke: Stroke) {
        let Some(eye) = self.eye else { return };
        if self.pending.len() == MAX_PENDING || !stroke.start.is_finite() || !stroke.end.is_finite()
        {
            return;
        }
        if let Some((start, end)) = clip(stroke.start, stroke.end, eye, RANGE + stroke.radius) {
            self.pending.push(Stroke {
                start,
                end,
                ..stroke
            });
        }
    }

    pub(super) fn paint(
        &mut self,
        map: &mut BendMap,
        eye: Vec2,
        blade_height: f32,
        mut ground: impl FnMut(Vec3, f32, f32) -> Option<Vec3>,
    ) -> bool {
        self.history
            .retain(|_, sample| self.latest_sample - sample.seen <= 0.25);
        if self.pending.is_empty() {
            return false;
        }
        self.tick = self.tick.wrapping_add(1);
        if self.ground.len() >= MAX_GROUND_SAMPLES {
            self.ground
                .retain(|_, sample| self.tick.wrapping_sub(sample.tick) <= GROUND_CACHE_TICKS);
        }
        let mut project = |point: Vec3, reach: f32, headroom: f32| {
            let cell = (point / Vec3::new(SPACING, 0.25, SPACING))
                .floor()
                .as_ivec3();
            let key = (cell, (headroom * 4.0).ceil() as i32);
            if let Some(sample) = self.ground.get(&key).filter(|sample| {
                self.tick.wrapping_sub(sample.tick) <= GROUND_CACHE_TICKS
                    && (-headroom..=reach).contains(&(point.y - sample.height))
            }) {
                return Some(point.with_y(sample.height));
            }
            let hit = ground(point, reach, headroom)?;
            if self.ground.len() < MAX_GROUND_SAMPLES {
                self.ground.insert(
                    key,
                    GroundSample {
                        height: hit.y,
                        tick: self.tick,
                    },
                );
            }
            Some(hit)
        };
        self.pending.sort_unstable_by(|a, b| {
            let distance = |stroke: &Stroke| {
                let a = stroke.start.xz();
                let b = stroke.end.xz();
                (eye - eye.clamp(a.min(b), a.max(b))).length_squared()
            };
            b.downward
                .cmp(&a.downward)
                .then_with(|| distance(a).total_cmp(&distance(b)))
        });
        let mut dirty = false;
        let mut budget = MAX_PATCHES;
        for stroke in self.pending.drain(..) {
            let Some((start, end)) = clip(stroke.start, stroke.end, eye, RANGE + stroke.radius)
            else {
                continue;
            };
            let reach = blade_height + stroke.extra_height;
            let pieces = (start.xz().distance(end.xz()) / 4.0).ceil().clamp(1.0, 8.0) as usize;
            for part in 0..pieces {
                if budget == 0 {
                    return dirty;
                }
                budget -= 1;
                let a = start.lerp(end, part as f32 / pieces as f32);
                let b = start.lerp(end, (part + 1) as f32 / pieces as f32);
                let ground_b = project(b, reach, stroke.headroom);
                if let Some(hit) = ground_b.filter(|hit| stroke.downward && b.y - hit.y <= 0.45) {
                    dirty |= map.stamp_scaled(hit, hit, eye, (stroke.radius * 1.65).min(3.0), 1.0);
                    continue;
                }
                let ground_a = if a == b {
                    ground_b
                } else {
                    project(a, reach, stroke.headroom)
                };
                if let Some(a) = ground_a.or(ground_b) {
                    let b = ground_b.filter(|b| (b.y - a.y).abs() < 1.0).unwrap_or(a);
                    dirty |= map.stamp_scaled(a, b, eye, stroke.radius, stroke.strength);
                }
            }
        }
        dirty
    }

    #[cfg(test)]
    pub(crate) fn drain_effect_reach(&mut self, ground_y: f32, blade_height: f32) -> f32 {
        self.pending
            .drain(..)
            .filter(|stroke| {
                stroke.extra_height > 0.8
                    && (-stroke.headroom..=blade_height + stroke.extra_height)
                        .contains(&(stroke.end.y - ground_y))
            })
            .map(|stroke| stroke.end.xz().length())
            .fold(0.0, f32::max)
    }
}

fn clip(a: Vec3, b: Vec3, eye: Vec2, radius: f32) -> Option<(Vec3, Vec3)> {
    let local = a.xz() - eye;
    let path = b.xz() - a.xz();
    let (mut enter, mut exit) = (0.0_f32, 1.0_f32);
    for axis in 0..2 {
        let origin = local[axis];
        let delta = path[axis];
        if delta.abs() <= 1e-5 {
            if origin.abs() > radius {
                return None;
            }
            continue;
        }
        let u = (-radius - origin) / delta;
        let v = (radius - origin) / delta;
        enter = enter.max(u.min(v));
        exit = exit.min(u.max(v));
        if enter > exit {
            return None;
        }
    }
    Some((a.lerp(b, enter), a.lerp(b, exit)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f32, bottom: f32) -> Obb {
        use meteor_core::Vec3 as V;
        Obb {
            center: V::new(x, bottom + 0.5, 0.0),
            axes: [V::new(1., 0., 0.), V::new(0., 1., 0.), V::new(0., 0., 1.)],
            half_size: V::new(0.7, 0.5, 0.7),
        }
    }

    fn floor(p: Vec3, max_height: f32, headroom: f32) -> Option<Vec3> {
        (-headroom..=max_height)
            .contains(&p.y)
            .then_some(p.with_y(0.0))
    }

    #[test]
    fn grass_combat_uses_height_sweep_and_radial_impact_without_stationary_repeats() {
        let actor = Entity::from_bits(11);
        let mut combat = GrassCombat::default();
        let mut map = BendMap::default();
        combat.record_boxes(actor, 1, 0, 0.0, false, &[bounds(0., 0.)]);
        assert!(!combat.paint(&mut map, Vec2::ZERO, 1.1, floor));
        combat.set_eye(Some(Vec2::ZERO));
        combat.record_boxes(actor, 1, 0, 0.1, false, &[bounds(0., 3.)]);
        assert!(!combat.paint(&mut map, Vec2::ZERO, 1.1, floor));
        combat.record_boxes(actor, 1, 0, 0.16, false, &[bounds(0., 0.12)]);
        assert!(combat.paint(&mut map, Vec2::ZERO, 1.1, floor));
        let pressure = |column: usize| map.pixels(-1., 2.)[column * 4 + 2];
        assert!(
            pressure(1) > 80,
            "slam must reach beyond the ordinary sweep"
        );
        combat.record_boxes(actor, 1, 0, 0.18, false, &[bounds(0., 0.12)]);
        assert!(!combat.paint(&mut map, Vec2::ZERO, 1.1, floor));
        combat.record_boxes(actor, 2, 0, 0.2, true, &[bounds(4., -0.8)]);
        assert!(combat.paint(&mut map, Vec2::ZERO, 1.1, floor));
    }

    #[test]
    fn grass_combat_clips_projectile_range_and_bounds_ground_probes() {
        let mut combat = GrassCombat::default();
        combat.set_eye(Some(Vec2::ZERO));
        for _ in 0..1000 {
            combat.record_segment(Vec3::new(-2000., 0.7, 0.0), Vec3::new(5., 0.7, 0.0), 0.1);
        }
        assert_eq!(combat.pending.len(), MAX_PENDING);
        let mut probes = 0;
        let mut map = BendMap::default();
        assert!(
            combat.paint(&mut map, Vec2::ZERO, 1.1, |p, reach, headroom| {
                probes += 1;
                assert!(p.x.abs() <= RANGE + 1.0);
                floor(p, reach, headroom)
            })
        );
        let pixels = map.pixels(-1., 2.);
        assert!(probes <= MAX_PATCHES * 2);
        assert!(pixels[..4][2] > 50);
        assert_eq!(
            pixels[14 * 4 + 2],
            0,
            "grass behind the clipped endpoint stays untouched"
        );
        assert!(combat.pending.is_empty());
        combat.record_segment(Vec3::new(-2000., 0.7, 0.0), Vec3::new(5., 0.7, 0.0), 0.1);
        assert!(combat.paint(&mut map, Vec2::ZERO, 1.1, |_, _, _| {
            panic!("the same trace should reuse the ground samples")
        }));
    }
}
