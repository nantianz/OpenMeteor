use super::*;

fn set(host: &mut MissionHost, slot: i64, kind: &str, value: i64) -> f64 {
    host.builtin(
        "SetTarget",
        &[
            PstValue::Num(slot as f64),
            PstValue::Str(kind.into()),
            PstValue::Num(value as f64),
        ],
    )
    .as_f64()
}

fn distance(host: &mut MissionHost, a: i64, b: i64) -> f64 {
    host.builtin(
        "Distance",
        &[PstValue::Num(a as f64), PstValue::Num(b as f64)],
    )
    .as_f64()
}

#[test]
fn special_targets_use_first_scene_record_and_native_vertical_offsets() {
    let mut host = MissionHost::default();
    host.register_char("actor", 7);
    host.sync_actor_position(7, [0.0; 3]);
    host.sync_waypoint_position(0, [0.0; 3]);

    host.sync_scene_target("flag", 0, 90, [99.0; 3]);
    host.sync_scene_target("flag", 0, 12, [0.0; 3]);
    host.sync_scene_target("flag", 0, 40, [88.0; 3]);
    host.sync_scene_target("safe", 1, 10, [1.0, 2.0, 3.0]);
    host.sync_scene_target("safe", 2, 9, [4.0, 5.0, 6.0]);
    host.sync_scene_target("safe", 0, 0, [100.0; 3]);
    host.sync_scene_target("door", 0, 30, [7.0, 8.0, 9.0]);
    assert_eq!(set(&mut host, 0, "char", 7), 1.0);
    assert_eq!(set(&mut host, 1, "waypoint", 0), 1.0);
    assert_eq!(set(&mut host, 2, "flag", 1234), 1.0);
    assert_eq!(set(&mut host, 3, "safe", 1), 1.0);
    assert_eq!(set(&mut host, 4, "safe", 2), 1.0);
    assert_eq!(set(&mut host, 5, "door", -99), 1.0);
    assert_eq!(distance(&mut host, 0, 1), 30.0);
    assert_eq!(distance(&mut host, 2, 1), 20.0);
    assert_eq!(distance(&mut host, 0, 2), 10.0);
    let height = f64::from(20.0 * meteor_core::MODEL_SCALE);
    assert_eq!(host.target_position(3), Some([1.0, 2.0 + height, 3.0]));
    assert_eq!(host.target_position(4), Some([4.0, 5.0 + height, 6.0]));
    assert_eq!(host.target_position(5), Some([7.0, 8.0 + height, 9.0]));
}

#[test]
fn special_target_failures_preserve_slots_and_commands_capture_positions() {
    let mut host = MissionHost::default();
    host.sync_scene_target("safe", 1, 0, [3.0, 4.0, 5.0]);
    host.sync_scene_target("door", 0, 1, [6.0, 7.0, 8.0]);
    assert_eq!(set(&mut host, 0, "safe", 1), 1.0);
    let old = host.target_position(0);
    for (kind, value) in [("SAFE", 1), ("safe", 0), ("safe", 3), ("flag", 1)] {
        assert_eq!(set(&mut host, 0, kind, value), 0.0);
        assert_eq!(host.target_position(0), old);
    }
    assert_eq!(set(&mut host, 10, "door", 0), 0.0);

    assert_eq!(
        host.builtin(
            "SetTarget",
            &[
                PstValue::Num(0.0),
                PstValue::Str("safe".into()),
                PstValue::Num(1.0),
                PstValue::Num(7.0),
            ]
        )
        .as_f64(),
        0.0
    );
    host.builtin(
        "ChangeBehavior",
        &[
            PstValue::Num(7.0),
            PstValue::Str("attacktarget".into()),
            PstValue::Num(0.0),
        ],
    );
    host.begin_scene_target_sync();
    host.sync_scene_target("safe", 1, 0, [30.0, 40.0, 50.0]);
    assert_eq!(
        host.target_position(0),
        old,
        "scene refresh does not move an existing slot"
    );
    assert_eq!(set(&mut host, 0, "safe", 1), 1.0);
    let requests = host.take_behavior_requests_with_targets();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].3, old,
        "later SetTarget must not move an already-issued behavior"
    );
    assert_eq!(
        set(&mut host, 1, "door", 0),
        0.0,
        "removed scene records must not survive refresh"
    );
}

#[test]
fn distance_returns_source_unit_integer_and_zero_initialized_slots() {
    let mut host = MissionHost::default();
    assert_eq!(distance(&mut host, 0, 9), 0.0);
    host.sync_waypoint_position(1, [0.12, 0.12, 0.0]);
    assert_eq!(set(&mut host, 1, "waypoint", 1), 1.0);
    assert_eq!(
        distance(&mut host, 0, 1),
        1.0,
        "sqrt(2) truncates toward zero"
    );
    assert_eq!(distance(&mut host, 1, 0), 1.0);
    assert_eq!(
        distance(&mut host, -1, 0),
        999_999.0,
        "invalid indices stay bounded"
    );
}
