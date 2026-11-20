fn low_air_a_harness_120hz() -> (GameplayHarness, crate::app_state::Keymap) {
    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).expect("original P1 assets");
    crate::plugins::install_frame_clock(&mut h.app);
    h.app
        .world_mut()
        .entity_mut(h.fighter)
        .insert(crate::components::fighter::VisualRootPin::default());
    crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
    let schedule = super::super::fighter_frame::schedule(&h.app);
    h.app.add_systems(
        schedule,
        crate::plugins::character_plugin::drive_fighter_animation_for_test
            .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
    );
    let keys = h.app.world().resource::<crate::app_state::Keymap>().clone();
    h.press_binding(keys.move_up);
    for _ in 0..30 {
        h.step_duration(LOW_AIR_A_TICK);
    }
    h.press_binding(keys.jump);
    for _ in 0..6 {
        h.step_duration(LOW_AIR_A_TICK);
    }
    h.release_binding(keys.jump);
    (h, keys)
}

const LOW_AIR_A_TICK: std::time::Duration = std::time::Duration::from_nanos(1_000_000_000 / 120);

#[test]
fn production_air_a_120hz_uses_the_authored_speed_at_low_and_high_entries() {
    use meteor_core::MODEL_SCALE;
    use meteor_physics::jump::AirMotionMode;

    for (delay, expected_entry, expected_peak) in [
        (0, 0.0, 0.0),
        (1, 2.328, 4.245),
        (2, 4.551, 6.468),
        (3, 6.711, 8.628),
        (24, 37.528, 39.444),
    ] {
        let (mut h, keys) = low_air_a_harness_120hz();
        let floor = h.position().y;
        for _ in 0..delay {
            h.step_duration(LOW_AIR_A_TICK);
        }
        h.press_binding(keys.attack);
        h.step_duration(LOW_AIR_A_TICK);
        h.release_binding(keys.attack);

        assert_eq!(h.pose_id(), 245, "A pressed at tick {delay}");
        let world = h.app.world();
        let air = world.get::<FighterAir>(h.fighter).unwrap();
        let published = world.get::<MotionState>(h.fighter).unwrap().velocity.y;
        let entry = (h.position().y - floor) / MODEL_SCALE;
        assert_eq!(air.air.motion_mode, AirMotionMode::Curve, "tick {delay}");
        assert!(
            (air.air.vertical_speed / MODEL_SCALE - 5.0).abs() < 0.01,
            "Pose 245's first step must use its authored vertical speed at tick {delay}",
        );
        assert_eq!(air.mover_vertical_contact, delay == 0, "tick {delay}");
        if delay == 0 {
            assert!(air.ground_contact);
            assert!(
                published.abs() < 0.001,
                "floor probe held the feet in place"
            );
        }
        assert!(
            (entry - expected_entry).abs() < 0.02,
            "tick {delay}: {entry}"
        );

        let mut peak = entry;
        for _ in 0..90 {
            h.step_duration(LOW_AIR_A_TICK);
            if h.pose_id() != 245 {
                break;
            }
            peak = peak.max((h.position().y - floor) / MODEL_SCALE);
        }
        assert_ne!(h.pose_id(), 245, "Pose 245 must finish at tick {delay}");
        eprintln!("120 Hz A at tick {delay}: entry={entry:.3} peak={peak:.3} source units");
        assert!((peak - expected_peak).abs() < 0.02, "tick {delay}: {peak}");
    }
}

#[test]
fn production_air_a_120hz_control_plain_jump_keeps_its_ballistic_arc() {
    use meteor_core::MODEL_SCALE;
    use meteor_physics::jump::AirMotionMode;

    let (mut h, _) = low_air_a_harness_120hz();
    let floor = h.position().y;
    h.step_duration(LOW_AIR_A_TICK);
    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
    assert_eq!(h.state(), FighterState::Jump);
    assert_eq!(air.air.motion_mode, AirMotionMode::Ballistic);
    assert!((air.air.vertical_speed / MODEL_SCALE - 274.38).abs() < 0.02);
    assert!(((h.position().y - floor) / MODEL_SCALE - 2.287).abs() < 0.02);

    let mut peak = h.position().y;
    for _ in 0..120 {
        h.step_duration(LOW_AIR_A_TICK);
        peak = peak.max(h.position().y);
        if !h.airborne() {
            break;
        }
    }
    let peak = (peak - floor) / MODEL_SCALE;
    eprintln!("120 Hz plain jump: peak={peak:.3} source units");
    assert!((40.0..46.0).contains(&peak), "ordinary jump apex: {peak}");
    assert!(!h.airborne());
    assert!((h.position().y - floor).abs() < 1e-4);
}
