use super::{Contact, Imprint, CONTACT_RANGE};
use bevy::prelude::*;
use meteor_combat::hitbox::Obb;
use std::collections::HashMap;

const MAX_BOX_HISTORY: usize = 256;
const MAX_PENDING: usize = 48;

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
struct BoxKey(Entity, u32, u32, usize);

struct Sample {
    bottom: Vec3,
    time: f64,
    seen: f64,
}

#[derive(Clone, Copy)]
struct SnowStroke {
    start: Vec3,
    end: Vec3,
    width: f32,
    strength: f32,
    extra_height: f32,
    downward: bool,
    effect: bool,
}

#[derive(Resource, Default)]
pub(crate) struct SnowCombat {
    eye: Option<Vec2>,
    history: HashMap<BoxKey, Sample>,
    pending: Vec<SnowStroke>,
    latest_sample: f64,
}

impl SnowCombat {
    pub(crate) fn set_eye(&mut self, eye: Option<Vec2>) {
        if self.eye.is_some_and(|old| {
            eye.is_none_or(|new| old.distance_squared(new) > (2.0 * CONTACT_RANGE).powi(2))
        }) {
            self.pending.clear();
            self.history.clear();
        }
        self.eye = eye;
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
            if !bottom.is_finite()
                || (bottom.xz() - eye).length_squared() > (CONTACT_RANGE + 8.0).powi(2)
            {
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
            let downward = -travel.y > elapsed * 4.5 && -travel.y > travel.xz().length();
            let width = extent(axes[0].x, axes[1].x, axes[2].x)
                .min(extent(axes[0].z, axes[1].z, axes[2].z));
            let speed = if elapsed > 0.0 {
                travel.length() / elapsed
            } else {
                0.0
            };
            let strength = (0.55
                + 0.30 * (speed / 24.0).clamp(0.0, 1.0)
                + 0.10 * (width / 1.4).clamp(0.0, 1.0)
                + if effect { 0.12 } else { 0.0 })
            .clamp(0.0, 1.0);
            self.push(SnowStroke {
                start,
                end: bottom,
                width,
                strength,
                extra_height: if effect { 0.85 } else { 0.0 },
                downward: downward || (effect && width >= 1.0 && travel.length_squared() < 0.08),
                effect,
            });
        }
    }

    pub(crate) fn record_segment(&mut self, start: Vec3, end: Vec3, width: f32) {
        self.push(SnowStroke {
            start,
            end,
            width,
            strength: 0.72,
            extra_height: 0.35,
            downward: false,
            effect: false,
        });
    }

    fn push(&mut self, stroke: SnowStroke) {
        let Some(eye) = self.eye else { return };
        if self.pending.len() >= MAX_PENDING || !stroke.start.is_finite() || !stroke.end.is_finite()
        {
            return;
        }
        let max_sq = (CONTACT_RANGE + 6.0).powi(2);
        if (stroke.start.xz() - eye).length_squared() > max_sq
            && (stroke.end.xz() - eye).length_squared() > max_sq
        {
            return;
        }
        self.pending.push(stroke);
    }

    pub(super) fn drain_contacts(&mut self, floor: f32, thickness: f32, out: &mut Vec<Contact>) {
        self.history
            .retain(|_, sample| self.latest_sample - sample.seen <= 0.25);
        if self.pending.is_empty() {
            return;
        }
        let min_y = floor - 0.45;
        for stroke in self.pending.drain(..) {
            let max_y = floor + thickness + 0.55 + stroke.extra_height;
            let lo_y = stroke.start.y.min(stroke.end.y);
            let hi_y = stroke.start.y.max(stroke.end.y);
            if lo_y > max_y || hi_y < min_y {
                continue;
            }
            let (seg_start, seg_end) = clip_height(stroke.start, stroke.end, min_y, max_y);
            let from = Vec3::new(seg_start.x, floor, seg_start.z);
            let position = Vec3::new(seg_end.x, floor, seg_end.z);
            let planar = position.xz() - from.xz();
            let planar_len = planar.length();
            let direction = if planar_len > 0.001 {
                planar / planar_len
            } else {
                Vec2::Y
            };
            if stroke.downward || planar_len < 0.08 {
                let powder = (6.0 + 6.0 * stroke.strength + if stroke.effect { 2.0 } else { 0.0 })
                    .round()
                    .clamp(4.0, 14.0) as u8;
                out.push(Contact {
                    from: position,
                    position,
                    direction,
                    imprint: Imprint::Landing,
                    left: false,
                    powder,
                });
            } else {
                let capped_end = if planar_len > 6.0 {
                    from + Vec3::new(direction.x * 6.0, 0.0, direction.y * 6.0)
                } else {
                    position
                };
                let powder = (3.0 + 4.0 * stroke.strength + stroke.width.min(0.6) * 3.0)
                    .round()
                    .clamp(3.0, 9.0) as u8;
                out.push(Contact {
                    from,
                    position: capped_end,
                    direction,
                    imprint: Imprint::Slide,
                    left: false,
                    powder,
                });
            }
        }
    }
}

fn clip_height(start: Vec3, end: Vec3, min_y: f32, max_y: f32) -> (Vec3, Vec3) {
    let dy = end.y - start.y;
    if dy.abs() < 1e-5 {
        return (start, end);
    }
    let t0 = ((min_y - start.y) / dy).clamp(0.0, 1.0);
    let t1 = ((max_y - start.y) / dy).clamp(0.0, 1.0);
    let lo = t0.min(t1);
    let hi = t0.max(t1);
    (start.lerp(end, lo), start.lerp(end, hi))
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_core::math::Vec3 as CoreVec3;

    fn obb(center: Vec3, half: Vec3) -> Obb {
        Obb {
            center: CoreVec3::new(center.x, center.y, center.z),
            axes: [
                CoreVec3::new(1.0, 0.0, 0.0),
                CoreVec3::new(0.0, 1.0, 0.0),
                CoreVec3::new(0.0, 0.0, 1.0),
            ],
            half_size: CoreVec3::new(half.x, half.y, half.z),
        }
    }

    #[test]
    fn low_sweeps_carve_slide_furrows_and_high_swings_do_not_touch_snow() {
        let mut combat = SnowCombat::default();
        combat.set_eye(Some(Vec2::ZERO));
        let actor = Entity::from_bits(1);

        combat.record_boxes(
            actor,
            1,
            0,
            1.0,
            false,
            &[obb(Vec3::new(0.0, 0.4, 0.0), Vec3::splat(0.3))],
        );
        combat.record_boxes(
            actor,
            1,
            0,
            1.05,
            false,
            &[obb(Vec3::new(1.4, 0.4, 0.2), Vec3::splat(0.3))],
        );

        combat.record_boxes(
            actor,
            2,
            0,
            1.0,
            false,
            &[obb(Vec3::new(0.0, 3.0, 0.0), Vec3::splat(0.3))],
        );
        combat.record_boxes(
            actor,
            2,
            0,
            1.05,
            false,
            &[obb(Vec3::new(1.4, 3.0, 0.0), Vec3::splat(0.3))],
        );

        let mut contacts = Vec::new();
        combat.drain_contacts(0.0, 0.38, &mut contacts);
        assert!(
            contacts
                .iter()
                .any(|c| c.imprint == Imprint::Slide && c.powder > 0),
            "low sweep must carve a snow furrow with powder"
        );
        assert!(
            contacts.iter().all(|c| (c.position.y - 0.0).abs() < 1e-4),
            "snow contacts must project onto the snow floor"
        );
        assert_eq!(contacts.len(), 2, "high overhead swings must be ignored");
    }

    #[test]
    fn downward_slam_and_low_projectile_create_landing_crater_and_slide() {
        let mut combat = SnowCombat::default();
        combat.set_eye(Some(Vec2::ZERO));
        let actor = Entity::from_bits(2);

        combat.record_boxes(
            actor,
            3,
            0,
            2.0,
            false,
            &[obb(Vec3::new(2.0, 1.6, 1.0), Vec3::splat(0.35))],
        );
        combat.record_boxes(
            actor,
            3,
            0,
            2.08,
            false,
            &[obb(Vec3::new(2.0, 0.35, 1.0), Vec3::splat(0.35))],
        );

        combat.record_segment(Vec3::new(-1.0, 0.3, 0.0), Vec3::new(2.0, 0.1, 0.0), 0.1);

        let mut contacts = Vec::new();
        combat.drain_contacts(0.0, 0.38, &mut contacts);
        assert!(contacts.iter().any(|c| c.imprint == Imprint::Landing));
        assert!(contacts.iter().any(|c| c.imprint == Imprint::Slide));
    }
}
