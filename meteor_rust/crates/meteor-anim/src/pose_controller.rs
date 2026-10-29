use meteor_asset::pos::{NextPoseBlock, Pose, SpeedBlock};

pub fn pose_playback_speed_at_frame(pose: &Pose, frame: f32) -> f32 {
    let authored_frame = frame.trunc();
    let mut speed: f32 = 1.0;
    for seg in &pose.blends {
        if in_segment(seg, authored_frame) {
            speed = seg.speed;
        }
    }
    for seg in &pose.actions {
        if in_segment(seg, authored_frame) {
            speed = seg.speed;
        }
    }

    let speed = if speed == 0.0 || speed.is_nan() {
        1.0
    } else {
        speed
    };
    speed.max(0.01)
}

pub fn pose_elapsed_seconds_at_frame(pose: &Pose, frame: f32) -> f32 {
    if !frame.is_finite() {
        return 0.0;
    }
    let frame = frame.clamp(pose.start as f32, pose.end.saturating_add(1) as f32);
    let end = frame as u32;
    let elapsed: f32 = (pose.start..end)
        .map(|f| 1.0 / (meteor_core::ANIM_FPS * pose_playback_speed_at_frame(pose, f as f32)))
        .sum();
    elapsed
        + (frame - end as f32) / (meteor_core::ANIM_FPS * pose_playback_speed_at_frame(pose, frame))
}

pub fn advance_pose_loop_cursor(
    cursor: f32,
    frame_budget: f32,
    start: u32,
    loop_start: u32,
    loop_end: u32,
) -> f32 {
    let advanced = cursor + frame_budget;
    let abs = start as f32 + advanced;
    if abs > loop_end as f32 {
        (loop_start as f32) - (start as f32)
    } else {
        advanced
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PoseLinkDecision {
    NoOp,

    CommandOverrodeLink,

    NoLink,

    InvalidLink,

    Link {
        link_pose_id: u16,
        entry_blend_seconds: Option<f32>,
    },
}

pub fn resolve_completed_pose(
    current_pose_id: u16,
    completed_pose: Option<&Pose>,
    pose_exists: impl Fn(u16) -> bool,
) -> PoseLinkDecision {
    let Some(completed) = completed_pose else {
        return PoseLinkDecision::NoOp;
    };

    if current_pose_id != completed.id {
        return PoseLinkDecision::CommandOverrodeLink;
    }

    let Some(raw_link) = completed.link else {
        return PoseLinkDecision::NoLink;
    };
    let link_pose_id = raw_link;
    if !pose_exists(link_pose_id) {
        return PoseLinkDecision::InvalidLink;
    }

    let entry_blend_seconds = link_blend_seconds(completed.next_pose.as_ref());

    PoseLinkDecision::Link {
        link_pose_id,
        entry_blend_seconds,
    }
}

fn link_blend_seconds(next_pose: Option<&NextPoseBlock>) -> Option<f32> {
    let np = next_pose?;
    let t = np.time;
    if t.is_finite() && t > 0.0 {
        Some(t)
    } else {
        None
    }
}

fn in_segment(seg: &SpeedBlock, authored_frame: f32) -> bool {
    let (Some(s), Some(e)) = (seg.start, seg.end) else {
        return false;
    };
    authored_frame >= s as f32 && authored_frame <= e as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_asset::pos::{NextPoseBlock, Pose, SpeedBlock};

    fn pose(
        id: u16,
        blends: Vec<SpeedBlock>,
        actions: Vec<SpeedBlock>,
        link: Option<u16>,
        next_pose: Option<NextPoseBlock>,
    ) -> Pose {
        Pose {
            id,
            source: 1,
            start: 0,
            end: 0,
            loop_start: 0,
            loop_end: 0,
            loops: false,
            link,
            note: String::new(),
            effect_type: None,
            effect_id: None,
            attacks: vec![],
            next_pose,
            blends,
            actions,
            action_entry_frame: 0,
            drags: vec![],
        }
    }

    fn seg(start: u32, end: u32, speed: f32) -> SpeedBlock {
        SpeedBlock {
            start: Some(start),
            end: Some(end),
            speed,
        }
    }

    #[test]
    fn effect_seek_uses_authored_speed_precedence_and_preserves_subframe_phase() {
        let mut p = pose(
            416,
            vec![seg(100, 129, 1.5)],
            vec![seg(120, 124, 0.5)],
            None,
            None,
        );
        p.start = 100;
        p.end = 150;

        let expected = 25.0 / 45.0 + 5.0 / 15.0;
        assert!((pose_elapsed_seconds_at_frame(&p, 130.0) - expected).abs() < 1e-5);
        assert!((pose_elapsed_seconds_at_frame(&p, 130.9) - expected - 0.9 / 30.0).abs() < 1e-5);
        assert_eq!(pose_elapsed_seconds_at_frame(&p, 100.0), 0.0);
        assert!((pose_elapsed_seconds_at_frame(&p, 100.9) - 0.9 / 45.0).abs() < 1e-5);
        assert!((pose_elapsed_seconds_at_frame(&p, 131.0) - expected - 1.0 / 30.0).abs() < 1e-5);
    }

    #[test]
    fn speed_default_when_no_segments() {
        let p = pose(1, vec![], vec![], None, None);
        assert!((pose_playback_speed_at_frame(&p, 10.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn speed_from_blend_inside_range() {
        let p = pose(1, vec![seg(10, 20, 0.5)], vec![], None, None);
        assert!((pose_playback_speed_at_frame(&p, 15.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn speed_inclusive_end_span() {
        let p = pose(1, vec![seg(10, 20, 0.5)], vec![], None, None);
        assert!((pose_playback_speed_at_frame(&p, 20.999) - 0.5).abs() < 1e-6);
        assert!((pose_playback_speed_at_frame(&p, 21.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn speed_action_wins_over_blend_on_overlap() {
        let p = pose(
            1,
            vec![seg(10, 20, 0.5)],
            vec![seg(10, 20, 2.0)],
            None,
            None,
        );
        assert!((pose_playback_speed_at_frame(&p, 15.0) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn speed_zero_treated_as_one() {
        let p = pose(1, vec![seg(10, 20, 0.0)], vec![], None, None);
        assert!((pose_playback_speed_at_frame(&p, 15.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn speed_floored_to_zero_01() {
        let p = pose(1, vec![seg(10, 20, 0.001)], vec![], None, None);
        assert!((pose_playback_speed_at_frame(&p, 15.0) - 0.01).abs() < 1e-6);
    }

    #[test]
    fn speed_outside_range_is_default() {
        let p = pose(1, vec![seg(10, 20, 0.5)], vec![], None, None);
        assert!((pose_playback_speed_at_frame(&p, 5.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn loop_advance_within_bounds() {
        let c = advance_pose_loop_cursor(5.0, 2.0, 0, 0, 60);
        assert!((c - 7.0).abs() < 1e-6);
    }

    #[test]
    fn loop_wrap_drops_fractional_overshoot() {
        let c = advance_pose_loop_cursor(59.0, 2.0, 0, 0, 60);
        assert!((c - 0.0).abs() < 1e-6);
    }

    #[test]
    fn loop_wrap_with_nonzero_start() {
        let c = advance_pose_loop_cursor(39.0, 2.0, 100, 120, 160);
        assert!((c - 41.0).abs() < 1e-6);

        let c = advance_pose_loop_cursor(59.0, 2.0, 100, 120, 160);
        assert!((c - 20.0).abs() < 1e-6);
    }

    #[test]
    fn loop_boundary_equal_not_wrap() {
        let c = advance_pose_loop_cursor(60.0, 0.0, 0, 0, 60);
        assert!((c - 60.0).abs() < 1e-6);
    }

    #[test]
    fn resolve_no_completed_pose_is_noop() {
        assert_eq!(
            resolve_completed_pose(5, None, |_| true),
            PoseLinkDecision::NoOp
        );
    }

    #[test]
    fn resolve_command_overrode_link() {
        let p = pose(5, vec![], vec![], Some(6), None);
        assert_eq!(
            resolve_completed_pose(7, Some(&p), |_| true),
            PoseLinkDecision::CommandOverrodeLink
        );
    }

    #[test]
    fn resolve_no_link() {
        let p = pose(5, vec![], vec![], None, None);
        assert_eq!(
            resolve_completed_pose(5, Some(&p), |_| true),
            PoseLinkDecision::NoLink
        );
    }

    #[test]
    fn resolve_invalid_link_target_absent() {
        let p = pose(5, vec![], vec![], Some(99), None);
        assert_eq!(
            resolve_completed_pose(5, Some(&p), |id| id != 99),
            PoseLinkDecision::InvalidLink
        );
    }

    #[test]
    fn resolve_link_without_next_pose_blend() {
        let p = pose(5, vec![], vec![], Some(6), None);
        assert_eq!(
            resolve_completed_pose(5, Some(&p), |_| true),
            PoseLinkDecision::Link {
                link_pose_id: 6,
                entry_blend_seconds: None,
            }
        );
    }

    #[test]
    fn resolve_link_with_positive_next_pose_time() {
        let np = NextPoseBlock {
            start: Some(40),
            end: Some(55),
            time: 0.15,
        };
        let p = pose(5, vec![], vec![], Some(6), Some(np));
        assert_eq!(
            resolve_completed_pose(5, Some(&p), |_| true),
            PoseLinkDecision::Link {
                link_pose_id: 6,
                entry_blend_seconds: Some(0.15),
            }
        );
    }

    #[test]
    fn resolve_link_with_zero_next_pose_time_is_none() {
        let np = NextPoseBlock {
            start: Some(40),
            end: Some(55),
            time: 0.0,
        };
        let p = pose(5, vec![], vec![], Some(6), Some(np));
        assert_eq!(
            resolve_completed_pose(5, Some(&p), |_| true),
            PoseLinkDecision::Link {
                link_pose_id: 6,
                entry_blend_seconds: None,
            }
        );
    }

    #[test]
    fn resolve_link_with_nan_next_pose_time_is_none() {
        let np = NextPoseBlock {
            start: Some(40),
            end: Some(55),
            time: f32::NAN,
        };
        let p = pose(5, vec![], vec![], Some(6), Some(np));
        assert_eq!(
            resolve_completed_pose(5, Some(&p), |_| true),
            PoseLinkDecision::Link {
                link_pose_id: 6,
                entry_blend_seconds: None,
            }
        );
    }
}
