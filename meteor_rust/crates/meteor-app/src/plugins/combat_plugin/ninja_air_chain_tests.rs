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
