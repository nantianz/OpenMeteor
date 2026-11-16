use bevy::prelude::*;
use meteor_combat::hitbox::Obb;
use std::collections::HashMap;

const MAX_SAMPLES: usize = 256;

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
struct Key(Entity, u32, u32, usize);

struct Sample {
    key: Key,
    time: f64,
    center: Vec3,
    corners: [Vec3; 8],
}

struct History {
    center: Vec3,
    time: f64,
    emitted: f64,
    corners: [Vec3; 8],
}

pub(crate) struct WaterStrike {
    pub surface: Entity,
    pub point: Vec3,
    pub impact: super::spray::Impact,
}

#[derive(Resource, Default)]
pub(crate) struct WaterCombat {
    samples: Vec<Sample>,
    rays: Vec<(Vec3, Vec3, Vec3, f32)>,
    history: HashMap<Key, History>,
}

impl WaterCombat {
    pub(crate) fn record_boxes(
        &mut self,
        actor: Entity,
        serial: u32,
        window: u32,
        time: f64,
        _forward: Vec3,
        boxes: &[Obb],
    ) {
        for (index, b) in boxes.iter().take(16).enumerate() {
            if self.samples.len() >= MAX_SAMPLES {
                break;
            }
            let center = Vec3::new(b.center.x, b.center.y, b.center.z);
            let axes = b.axes.map(|a| Vec3::new(a.x, a.y, a.z));
            let half = Vec3::new(b.half_size.x, b.half_size.y, b.half_size.z);
            let corners = std::array::from_fn(|i| {
                center
                    + axes[0] * half.x * if i & 1 == 0 { -1.0 } else { 1.0 }
                    + axes[1] * half.y * if i & 2 == 0 { -1.0 } else { 1.0 }
                    + axes[2] * half.z * if i & 4 == 0 { -1.0 } else { 1.0 }
            });
            self.samples.push(Sample {
                key: Key(actor, serial, window, index),
                time,
                center,
                corners,
            });
        }
    }

    pub(crate) fn record_segment(&mut self, start: Vec3, end: Vec3, velocity: Vec3, radius: f32) {
        if self.rays.len() < 64 {
            self.rays.push((start, end, velocity, radius));
        }
    }

    pub(crate) fn drain(
        &mut self,
        now: f64,
        surfaces: &[(Entity, &super::interaction::LiquidSurface)],
        eye: Vec3,
    ) -> Vec<WaterStrike> {
        let mut strikes = Vec::new();
        self.history.retain(|_, h| now - h.time < 0.35);
        for sample in self.samples.drain(..) {
            if sample.center.distance_squared(eye) > 85.0 * 85.0 {
                continue;
            }
            let old = self.history.get(&sample.key);
            let elapsed = old.map_or(0.0, |h| (sample.time - h.time) as f32);
            let travel = old.map_or(Vec3::ZERO, |h| sample.center - h.center);
            let displacement = old.map_or(Vec3::ZERO, |h| {
                (0..8)
                    .map(|i| sample.corners[i] - h.corners[i])
                    .max_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
                    .unwrap()
            });
            let valid = elapsed > 0.0001 && elapsed < 0.2 && displacement.length() < 12.0;
            let velocity = if valid {
                displacement / elapsed
            } else {
                Vec3::ZERO
            };
            let mut emitted = old.map_or(f64::NEG_INFINITY, |h| h.emitted);
            let eligible = valid && sample.time - emitted >= 0.065 && displacement.length() > 0.015;
            if eligible {
                for &(id, surface) in surfaces {
                    if !surface.is_clear_water() {
                        continue;
                    }
                    let y = surface.top_height();
                    let lo = sample
                        .corners
                        .iter()
                        .map(|p| p.y)
                        .fold(f32::INFINITY, f32::min);
                    let hi = sample
                        .corners
                        .iter()
                        .map(|p| p.y)
                        .fold(f32::NEG_INFINITY, f32::max);
                    let mut candidates = Vec::new();
                    if lo <= y + 0.06 && hi >= y - 0.06 {
                        candidates.push(Vec3::new(sample.center.x, y, sample.center.z));

                        for i in 0..8 {
                            for bit in [1, 2, 4] {
                                if i & bit == 0 {
                                    if let Some(p) = plane_crossing(
                                        sample.corners[i],
                                        sample.corners[i | bit],
                                        y,
                                    ) {
                                        candidates.push(p);
                                    }
                                }
                            }
                        }
                    }
                    if valid {
                        if let Some(p) = plane_crossing(sample.center - travel, sample.center, y) {
                            candidates.push(p);
                        }
                        if let Some(old) = old {
                            for i in 0..8 {
                                if let Some(p) =
                                    plane_crossing(old.corners[i], sample.corners[i], y)
                                {
                                    candidates.push(p);
                                }
                            }
                        }
                    }
                    if let Some(point) = candidates
                        .into_iter()
                        .find(|&p| surface.contains_surface_point(p))
                    {
                        let extent = sample.corners[0].distance(sample.center);
                        strikes.push(WaterStrike {
                            surface: id,
                            point,
                            impact: super::spray::Impact {
                                velocity,
                                radius: (extent * 0.20).clamp(0.08, 0.7),
                                depth: (hi - lo).clamp(0.04, 0.35),
                            },
                        });
                        emitted = sample.time;
                        break;
                    }
                }
            }
            self.history.insert(
                sample.key,
                History {
                    center: sample.center,
                    time: sample.time,
                    emitted,
                    corners: sample.corners,
                },
            );
        }
        for (start, end, velocity, radius) in self.rays.drain(..) {
            for &(id, surface) in surfaces {
                if !surface.is_clear_water() {
                    continue;
                }
                let Some(point) = plane_crossing(start, end, surface.top_height()) else {
                    continue;
                };
                if point.distance_squared(eye) > 85.0 * 85.0
                    || !surface.contains_surface_point(point)
                {
                    continue;
                }
                strikes.push(WaterStrike {
                    surface: id,
                    point,
                    impact: super::spray::Impact {
                        velocity,
                        radius,
                        depth: radius * 2.0,
                    },
                });
                break;
            }
        }
        strikes.truncate(32);
        strikes
    }
}

fn plane_crossing(a: Vec3, b: Vec3, height: f32) -> Option<Vec3> {
    let dy = b.y - a.y;
    if dy.abs() < 1e-6 {
        return None;
    }
    let t = (height - a.y) / dy;
    (0.0..=1.0).contains(&t).then(|| a.lerp(b, t))
}

#[cfg(test)]
mod tests {
    use super::super::interaction::LiquidSurface;
    use super::*;
    fn shape(y: f32) -> Obb {
        use meteor_core::Vec3 as V;
        Obb {
            center: V::new(0.0, y, 0.0),
            axes: [
                V::new(1.0, 0.0, 0.0),
                V::new(0.0, 1.0, 0.0),
                V::new(0.0, 0.0, 1.0),
            ],
            half_size: V::new(1.0, 0.3, 0.2),
        }
    }
    #[test]
    fn attacks_require_surface_contact_and_stationary_frames_do_not_retrigger() {
        let (actor, water) = (Entity::from_bits(1), Entity::from_bits(2));
        let surface = LiquidSurface::test_pool(0.0);
        let mut combat = WaterCombat::default();
        combat.record_boxes(actor, 1, 0, 0.0, Vec3::X, &[shape(3.0)]);
        assert!(combat
            .drain(0.0, &[(water, &surface)], Vec3::ZERO)
            .is_empty());
        combat.record_boxes(actor, 1, 0, 0.1, Vec3::X, &[shape(-0.5)]);
        let impacts = combat.drain(0.1, &[(water, &surface)], Vec3::ZERO);
        assert_eq!(
            impacts.len(),
            1,
            "fast downward sweep must not tunnel through water"
        );
        assert_eq!(impacts[0].surface, water);
        combat.record_boxes(actor, 2, 0, 0.2, Vec3::X, &[shape(0.0)]);
        assert!(
            combat
                .drain(0.2, &[(water, &surface)], Vec3::ZERO)
                .is_empty(),
            "a new attack has no measured velocity yet; do not invent an 8-unit kick"
        );
        combat.record_boxes(actor, 2, 0, 0.3, Vec3::X, &[shape(0.0)]);
        assert!(combat
            .drain(0.3, &[(water, &surface)], Vec3::ZERO)
            .is_empty());
        let mut rotating = shape(0.0);
        rotating.axes.swap(0, 2);
        combat.record_boxes(actor, 2, 0, 0.4, Vec3::X, &[rotating]);
        let strikes = combat.drain(0.4, &[(water, &surface)], Vec3::ZERO);
        assert_eq!(
            strikes.len(),
            1,
            "a rotating weapon has speed even with a fixed box center"
        );
        assert!(strikes[0].impact.velocity.length() > 1.0);
    }
    #[test]
    fn projectile_segments_cannot_splash_beyond_scene_clipped_end_or_outside_water() {
        let surface = LiquidSurface::test_pool(0.0);
        let water = Entity::from_bits(2);
        let mut combat = WaterCombat::default();
        combat.record_segment(Vec3::Y * 4.0, Vec3::Y, Vec3::NEG_Y * 20.0, 0.1);
        combat.record_segment(
            Vec3::new(30.0, 2.0, 0.0),
            Vec3::new(30.0, -2.0, 0.0),
            Vec3::NEG_Y * 20.0,
            0.1,
        );
        assert!(combat
            .drain(0.0, &[(water, &surface)], Vec3::ZERO)
            .is_empty());
        combat.record_segment(
            Vec3::Y,
            Vec3::new(3.0, -1.0, 0.0),
            Vec3::new(30.0, -20.0, 0.0),
            0.1,
        );
        let hits = combat.drain(0.0, &[(water, &surface)], Vec3::ZERO);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].point, Vec3::new(1.5, 0.0, 0.0));
        assert!(combat
            .drain(0.0, &[(water, &surface)], Vec3::ZERO)
            .is_empty());
    }
}
