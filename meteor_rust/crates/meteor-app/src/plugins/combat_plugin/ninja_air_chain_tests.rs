#[test]
fn production_ninja_air_chain_returns_to_free_fall() {
    use meteor_physics::jump::AirMotionMode;
    for fps in [30, 60, 120] {
        for direction in 0..4 {
            for stop_after_attack in [false, true] {
                let mut h = spawn_gameplay_harness_with_order(12, true)
                    .expect("original P1 resources required");
                let keys = crate::app_state::Keymap::default();
                let (direction_key, teleport) = match direction {
                    0 => (keys.move_up, 530),
                    1 => (keys.move_right, 531),
                    2 => (keys.move_left, 532),
                    _ => (keys.move_down, 533),
                };
                let dt = 1.0 / fps as f32;
                let cycles = if direction == 0 { 12 } else { 6 };
                h.press_binding(keys.jump);
                for _ in 0..(fps as f32 * 0.15).ceil() as usize {
                    h.step_dt(dt);
                }
                h.release_binding(keys.jump);
                h.step_dt(dt);
                assert!(h.airborne());
                for cycle in 0..cycles {
                    h.press_binding(keys.jump);
                    h.press_binding(direction_key);
                    h.step_dt(dt);
                    h.release_binding(direction_key);
                    h.step_dt(dt);
                    h.press_binding(direction_key);
                    h.press_binding(keys.attack);
                    h.step_dt(dt);
                    assert_eq!(h.pose_id(), teleport, "{fps}FPS cycle {cycle}");
                    h.release_binding(direction_key);
                    h.release_binding(keys.attack);
                    let finish_without_jump = stop_after_attack && cycle == cycles - 1;
                    if finish_without_jump {
                        h.release_binding(keys.jump);
                    }
                    for _ in 0..(fps as f32 * 0.8).round() as usize {
                        h.step_dt(dt);
                    }
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    assert_eq!(air.air.motion_mode, AirMotionMode::Ballistic,
                        "{fps}FPS pose {teleport} cycle {cycle}: completed teleport retained authored vertical motion");
                    if !finish_without_jump {
                        assert_eq!(
                            h.state(),
                            FighterState::Jump,
                            "{fps}FPS pose {teleport}: buffered jump must survive the fix"
                        );
                        assert!(air.air.vertical_speed > 0.0);
                    }
                    h.release_binding(keys.jump);
                    h.step_dt(dt);
                }
                let release_y = h.position().y;
                assert!(
                    release_y > 8.0,
                    "must exercise accumulated climb: {release_y}"
                );
                let mut fastest = 0.0_f32;
                let mut landed = false;
                for _ in 0..fps * 5 {
                    let before = h.position();
                    h.step_dt(dt);
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    if !h.airborne() {
                        landed = true;
                        break;
                    }
                    assert_eq!(air.air.motion_mode, AirMotionMode::Ballistic);
                    let speed = (before.y - h.position().y) / dt;
                    fastest = fastest.max(speed);
                    assert!(speed <= 72.01, "shared mover cap: {speed}");
                }
                assert!(
                    landed,
                    "{fps}FPS pose {teleport}: must land with keys released"
                );
                if cycles == 12 {
                    assert!(fastest > 65.0,
                        "{fps}FPS stop_after_attack={stop_after_attack}: descent stuck at {fastest}");
                }
                eprintln!("MOVE-062 fps={fps} pose={teleport} cycles={cycles} stop_after_attack={stop_after_attack} release_y={release_y:.3} max_fall_speed={fastest:.3}");
            }
        }
    }
}
#[test]
fn production_ninja_air_inputs_follow_the_last_fixed_recovery_step() {
    use std::time::Duration;
    for hz in [60, 120, 135] {
        let mut h = spawn_gameplay_harness_with_order(12, true).unwrap();
        h.app
            .world_mut()
            .resource_mut::<crate::app_state::Settings>()
            .physics_hz = hz;
        crate::plugins::install_frame_clock(&mut h.app);
        let keys = crate::app_state::Keymap::default();
        let tick = Duration::from_secs_f64(1.0 / f64::from(hz));
        h.press_binding(keys.jump);
        for _ in 0..(hz / 5) {
            h.step_duration(tick);
        }
        h.release_binding(keys.jump);
        h.step_duration(tick);
        assert!(h.airborne());
        h.press_binding(keys.move_up);
        h.step_duration(tick);
        h.release_binding(keys.move_up);
        h.step_duration(tick);
        h.press_binding(keys.move_up);
        h.press_binding(keys.attack);
        h.step_duration(tick);
        assert_eq!(h.pose_id(), 530, "{hz} Hz, first teleport");
        h.release_binding(keys.move_up);
        h.release_binding(keys.attack);
        h.step_duration(tick);
        {
            let mut pose = h.app.world_mut().get_mut::<PoseRuntime>(h.fighter).unwrap();
            pose.frame = pose.end as f32 - 0.01;
        }
        h.press_binding(keys.jump);
        h.press_binding(keys.move_up);
        h.step_duration(tick + tick / 2);
        assert_eq!(h.state(), FighterState::Name, "{hz} Hz");
        assert!(
            h.app
                .world()
                .get::<FighterAir>(h.fighter)
                .unwrap()
                .jump_charge_active,
            "{hz} Hz: key-down must see the completed recovery"
        );
        h.release_binding(keys.move_up);
        h.step_duration(tick);
        h.press_binding(keys.move_up);
        h.press_binding(keys.attack);
        h.step_duration(tick);
        assert_eq!(h.pose_id(), 530, "{hz} Hz, second teleport");
        h.release_binding(keys.move_up);
        h.release_binding(keys.attack);
        h.app
            .world_mut()
            .resource_mut::<crate::app_state::Settings>()
            .variable_physics = true;
        for _ in 0..hz {
            h.step_duration(tick);
            if h.state() == FighterState::Jump {
                break;
            }
        }
        assert_eq!(h.state(), FighterState::Jump, "{hz} Hz");
        assert!(
            h.app
                .world()
                .get::<FighterAir>(h.fighter)
                .unwrap()
                .air
                .vertical_speed
                > 0.0
        );
    }
}
#[test]
fn production_ninja_air_charge_respects_recovery_step_and_key_up_order() {
    use meteor_physics::jump::JUMP_VERTICAL_SPEED_STRONG;
    use std::time::Duration;
    #[derive(Debug, PartialEq, Eq)]
    enum JumpRelease {
        Held,
        DuringAttack,
        AfterAttack,
    }
    let step = Duration::from_nanos(8_333_334);
    let half_step = step / 2;
    for (release, insert_zero_step) in [
        (JumpRelease::Held, false),
        (JumpRelease::Held, true),
        (JumpRelease::DuringAttack, false),
        (JumpRelease::AfterAttack, true),
    ] {
        let mut h = spawn_gameplay_harness_with_order(12, true).unwrap();
        crate::plugins::install_frame_clock(&mut h.app);
        let keys = crate::app_state::Keymap::default();
        h.press_binding(keys.jump);
        for _ in 0..30 {
            h.step_duration(step);
        }
        h.release_binding(keys.jump);
        h.step_duration(step);
        assert!(h.airborne());
        h.press_binding(keys.jump);
        h.press_binding(keys.move_up);
        h.step_duration(step);
        h.release_binding(keys.move_up);
        h.step_duration(step);
        h.press_binding(keys.move_up);
        h.press_binding(keys.attack);
        h.step_duration(step);
        assert_eq!(h.pose_id(), 530);
        h.release_binding(keys.move_up);
        h.release_binding(keys.attack);
        if release == JumpRelease::DuringAttack {
            h.release_binding(keys.jump);
            h.step_duration(step);
            assert!(is_attacking_state(h.state()));
            assert!(
                !h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .jump_charge_active
            );
        }
        for _ in 0..120 {
            if !is_attacking_state(h.state()) {
                break;
            }
            h.step_duration(step);
        }
        assert_eq!(h.state(), FighterState::Name);
        assert!(h.airborne());
        let start = h.position();
        if release == JumpRelease::DuringAttack {
            h.step_duration(step);
            assert_eq!(h.state(), FighterState::Name);
            assert!(h.position().y <= start.y);
            continue;
        }
        if release == JumpRelease::AfterAttack {
            h.release_binding(keys.jump);
            h.step_duration(half_step);
            assert_eq!(
                h.app
                    .world()
                    .resource::<crate::plugins::FrameClock>()
                    .sim_dt,
                0.0
            );
        } else if insert_zero_step {
            h.step_duration(half_step);
            assert_eq!(
                h.app
                    .world()
                    .resource::<crate::plugins::FrameClock>()
                    .sim_dt,
                0.0
            );
            assert_eq!(h.state(), FighterState::Name);
            assert!(
                h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .jump_charge_active
            );
            assert_eq!(
                h.app
                    .world()
                    .get::<SmState>(h.fighter)
                    .unwrap()
                    .exe_ground_mode_ticks,
                1
            );
            h.step_duration(half_step);
        } else {
            h.step_duration(step);
        }
        assert_eq!(h.state(), FighterState::Jump, "{release:?}");
        let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
        assert_eq!(
            air.air.vertical_speed, JUMP_VERTICAL_SPEED_STRONG,
            "{release:?}"
        );
        assert!(!air.jump_charge_active);
        assert!((h.position() - start).length() < 1e-5, "{release:?}");
        if release == JumpRelease::Held {
            h.release_binding(keys.jump);
        }
        h.step_duration(if release == JumpRelease::AfterAttack {
            half_step
        } else {
            step
        });
        assert!(h.position().y > start.y);
        assert!(
            h.app
                .world()
                .get::<FighterAir>(h.fighter)
                .unwrap()
                .air
                .vertical_speed
                < JUMP_VERTICAL_SPEED_STRONG
        );
    }
}
