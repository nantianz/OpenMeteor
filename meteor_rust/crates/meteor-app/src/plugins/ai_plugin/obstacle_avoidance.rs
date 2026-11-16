use crate::plugins::stage_collider::StageMeshCollider;
use bevy::prelude::Vec3;
use meteor_core::MODEL_SCALE;
use meteor_physics::collision::{MeshCollider, Point3};

pub(super) const AVOID_CALLBACK_DELAY: f32 = 0.3;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CollisionClock {
    pub blocked_seconds: f32,
    pub clear_seconds: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ObstacleSpace<'a> {
    pub position: Vec3,
    pub facing: f32,
    pub mesh: Option<&'a StageMeshCollider>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FollowRecovery {
    Normal,

    KeepPathInputs,
    StrongJump,
    WeakJump,
    AvoidObstacle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct AvoidanceStep {
    pub desired_yaw: f32,
    pub forward: bool,
    pub return_to_parent: bool,
}

impl CollisionClock {
    pub fn record_previous_mover(&mut self, blocked: bool, dt: f32) {
        if blocked {
            self.blocked_seconds += dt;
            self.clear_seconds = 0.0;
        } else {
            self.clear_seconds += dt;
        }
    }

    pub fn follow_recovery(
        &mut self,
        space: ObstacleSpace<'_>,
        mut roll: impl FnMut() -> u32,
    ) -> FollowRecovery {
        if self.blocked_seconds > 0.3 {
            self.blocked_seconds = 0.0;
            let delta = forward(space.facing) * (20.0 * MODEL_SCALE);
            let upper = space.position + Vec3::Y * (80.0 * MODEL_SCALE);
            if !space.blocked(upper, upper + delta) && roll() % 3 != 0 {
                return FollowRecovery::StrongJump;
            }
            let lower = space.position + Vec3::Y * (40.0 * MODEL_SCALE);
            if !space.blocked(lower, lower + delta) && roll() % 3 != 0 {
                return FollowRecovery::WeakJump;
            }
            return FollowRecovery::AvoidObstacle;
        }

        if self.clear_seconds > 0.4 {
            self.blocked_seconds = 0.0;
        }
        if self.blocked_seconds == 0.0 {
            FollowRecovery::Normal
        } else {
            FollowRecovery::KeepPathInputs
        }
    }
}

impl ObstacleSpace<'_> {
    fn blocked(self, start: Vec3, end: Vec3) -> bool {
        self.mesh.is_some_and(|mesh| {
            mesh.segment_hit(
                Point3::new(start.x, start.y, start.z),
                Point3::new(end.x, end.y, end.z),
            )
            .is_some()
        })
    }

    fn avoidance_segment(self, candidate_yaw: f32) -> (Vec3, Vec3) {
        (
            self.position + (Vec3::Y * 5.0 - forward(self.facing) * 10.0) * MODEL_SCALE,
            self.position + (Vec3::Y * 25.0 + forward(candidate_yaw) * 30.0) * MODEL_SCALE,
        )
    }
}

fn forward(yaw: f32) -> Vec3 {
    let (sin, cos) = yaw.sin_cos();
    Vec3::new(sin, 0.0, cos)
}

pub(super) fn avoid_obstacle_tick(
    space: ObstacleSpace<'_>,
    desired_yaw: f32,
    tick: i32,
    mut roll: impl FnMut() -> u32,
) -> AvoidanceStep {
    let offset = 135 - (roll() % 270) as i32;
    let candidate_yaw = space.facing + (offset as f32).to_radians();
    let (start, end) = space.avoidance_segment(candidate_yaw);
    if space.blocked(start, end) {
        return AvoidanceStep {
            desired_yaw: desired_yaw + if offset > 0 { -60.0_f32 } else { 60.0_f32 }.to_radians(),
            forward: false,
            return_to_parent: false,
        };
    }
    AvoidanceStep {
        desired_yaw: if roll() % 10 == 0 {
            candidate_yaw
        } else {
            desired_yaw
        },
        forward: true,
        return_to_parent: tick != 0 && tick % 3 == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(mesh: Option<&StageMeshCollider>) -> ObstacleSpace<'_> {
        ObstacleSpace {
            position: Vec3::ZERO,
            facing: 0.0,
            mesh,
        }
    }

    fn recovering() -> CollisionClock {
        CollisionClock {
            blocked_seconds: 0.31,
            clear_seconds: 0.0,
        }
    }

    fn wall(z: f32, bottom: f32, top: f32) -> StageMeshCollider {
        let mut mesh = StageMeshCollider::default();
        mesh.append_world_triangles(
            &[
                [-100.0 * MODEL_SCALE, bottom * MODEL_SCALE, z * MODEL_SCALE],
                [100.0 * MODEL_SCALE, bottom * MODEL_SCALE, z * MODEL_SCALE],
                [100.0 * MODEL_SCALE, top * MODEL_SCALE, z * MODEL_SCALE],
                [-100.0 * MODEL_SCALE, top * MODEL_SCALE, z * MODEL_SCALE],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
        mesh
    }

    #[test]
    fn clocks_preserve_intermittent_contact_until_strict_clear_period() {
        let mut clock = CollisionClock::default();
        clock.record_previous_mover(true, 0.15);
        clock.record_previous_mover(false, 0.4);
        assert_eq!(
            clock.follow_recovery(space(None), || panic!("no probe yet")),
            FollowRecovery::KeepPathInputs
        );
        clock.record_previous_mover(true, 0.15);
        assert_eq!(clock.clear_seconds, 0.0);
        assert_eq!(
            clock.follow_recovery(space(None), || panic!("strict >0.3")),
            FollowRecovery::KeepPathInputs
        );
        clock.record_previous_mover(false, 0.401);
        assert_eq!(
            clock.follow_recovery(space(None), || panic!("clear reset")),
            FollowRecovery::Normal
        );
        assert_eq!(clock.blocked_seconds, 0.0);
    }

    #[test]
    fn recovery_uses_prior_clock_and_precedes_clear_reset() {
        let mut clock = CollisionClock {
            blocked_seconds: 0.3,
            clear_seconds: 0.0,
        };
        assert_eq!(
            clock.follow_recovery(space(None), || panic!("prior clock")),
            FollowRecovery::KeepPathInputs
        );
        clock.record_previous_mover(true, 0.01);
        clock.record_previous_mover(false, 0.5);
        assert_eq!(
            clock.follow_recovery(space(None), || 1),
            FollowRecovery::StrongJump
        );
        assert_eq!(clock.blocked_seconds, 0.0);
        assert_eq!(clock.clear_seconds, 0.5);
    }

    #[test]
    fn high_probe_obstruction_skips_its_draw_and_uses_lower_jump() {
        let mesh = wall(10.0, 70.0, 90.0);
        let mut calls = 0;
        let result = recovering().follow_recovery(space(Some(&mesh)), || {
            calls += 1;
            2
        });
        assert_eq!(result, FollowRecovery::WeakJump);
        assert_eq!(calls, 1);
    }

    #[test]
    fn failed_upper_random_choice_reaches_independent_lower_choice() {
        let mut rolls = [0, 2].into_iter();
        assert_eq!(
            recovering().follow_recovery(space(None), || rolls.next().unwrap()),
            FollowRecovery::WeakJump
        );
        assert!(rolls.next().is_none());
        assert_eq!(
            recovering().follow_recovery(space(None), || 0),
            FollowRecovery::AvoidObstacle
        );
        let mesh = wall(10.0, 0.0, 90.0);
        assert_eq!(
            recovering().follow_recovery(space(Some(&mesh)), || panic!("both rays blocked")),
            FollowRecovery::AvoidObstacle
        );
    }

    #[test]
    fn jump_probe_uses_current_facing_and_finite_twenty_unit_segment() {
        let behind_endpoint = wall(21.0, 0.0, 90.0);
        assert_eq!(
            recovering().follow_recovery(space(Some(&behind_endpoint)), || 1),
            FollowRecovery::StrongJump
        );
        let wall_ahead = wall(10.0, 0.0, 90.0);
        let rotated = ObstacleSpace {
            facing: std::f32::consts::PI,
            ..space(Some(&wall_ahead))
        };
        assert_eq!(
            recovering().follow_recovery(rotated, || 1),
            FollowRecovery::StrongJump
        );
    }

    #[test]
    fn avoidance_probe_starts_behind_and_slopes_up_to_candidate() {
        let at = ObstacleSpace {
            position: Vec3::new(5.0, 7.0, 9.0),
            ..space(None)
        };
        let (start, end) = at.avoidance_segment(std::f32::consts::FRAC_PI_2);
        assert!(start.abs_diff_eq(at.position + Vec3::new(0.0, 5.0, -10.0) * MODEL_SCALE, 1e-5));
        assert!(end.abs_diff_eq(at.position + Vec3::new(30.0, 25.0, 0.0) * MODEL_SCALE, 1e-5));

        let mesh = wall(10.0, 14.0, 16.0);
        let mut rolls = [135].into_iter();
        let result = avoid_obstacle_tick(space(Some(&mesh)), 0.25, 3, || rolls.next().unwrap());
        assert!(!result.forward);
        assert!(!result.return_to_parent);
        assert!((result.desired_yaw - (0.25 + 60.0_f32.to_radians())).abs() < 1e-6);
    }

    #[test]
    fn blocked_offsets_turn_previous_desired_heading_and_never_pop() {
        let mesh = wall(10.0, 0.0, 40.0);
        for (roll, degrees) in [(134, -60.0_f32), (135, 60.0), (136, 60.0)] {
            let mut calls = 0;
            let result = avoid_obstacle_tick(space(Some(&mesh)), 2.0, 6, || {
                calls += 1;
                roll
            });
            assert!(!result.forward);
            assert!(!result.return_to_parent);
            assert_eq!(calls, 1);
            assert!((result.desired_yaw - (2.0 + degrees.to_radians())).abs() < 1e-6);
        }
    }

    #[test]
    fn clear_callback_preserves_heading_nine_tenths_and_returns_only_on_nonzero_thirds() {
        for tick in [0, 1, 2, 3, 4, 6, -1, -3] {
            let mut rolls = [0, 1].into_iter();
            let result = avoid_obstacle_tick(space(None), -0.4, tick, || rolls.next().unwrap());
            assert!(result.forward);
            assert_eq!(result.desired_yaw, -0.4);
            assert_eq!(result.return_to_parent, matches!(tick, 3 | 6 | -3));
        }
    }

    #[test]
    fn selected_clear_heading_has_original_asymmetric_angle_range() {
        for (first_roll, expected) in [(0, 135.0_f32), (269, -134.0), (270, 135.0)] {
            let mut rolls = [first_roll, 10].into_iter();
            let result = avoid_obstacle_tick(
                ObstacleSpace {
                    facing: 0.5,
                    ..space(None)
                },
                2.0,
                0,
                || rolls.next().unwrap(),
            );
            assert!((result.desired_yaw - (0.5 + expected.to_radians())).abs() < 1e-6);
        }
    }
}
