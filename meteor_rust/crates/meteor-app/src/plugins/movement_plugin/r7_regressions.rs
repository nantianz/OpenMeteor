use super::*;
use crate::plugins::movement_plugin::FighterAttackMotion;

fn r7_harness(target_fps: u32) -> GameplayHarness {
    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).expect("original p1 resources");
    h.app.world_mut().resource_mut::<Settings>().target_fps = target_fps;
    crate::plugins::install_frame_clock(&mut h.app);
    install_guard_test_animation(&mut h);
    h
}

fn r7_wall_harness(target_fps: u32) -> GameplayHarness {
    let mut h = r7_harness(target_fps);
    h.press_binding(crate::app_state::Keymap::default().move_up);

    h.step_dt(1.0 / 120.0);
    let mut mesh = harness_ground();
    mesh.append_world_triangles(
        &[
            [-20.0, 0.0, 2.0],
            [20.0, 0.0, 2.0],
            [20.0, 100.0, 2.0],
            [-20.0, 100.0, 2.0],
        ],
        &[0, 2, 1, 0, 3, 2],
    );
    h.replace_stage_mesh(mesh);
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation = Vec3::new(0.0, 50.0, -0.16);
    h.app
        .world_mut()
        .get_mut::<SmState>(h.fighter)
        .unwrap()
        .current = FighterState::Jump;
    {
        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
        air.recovery_ground_mode = None;
        air.air.start_fall(0.0, 0.0);
        air.air.motion_mode = meteor_physics::jump::AirMotionMode::Wall;
    }
    let now = crate::plugins::simulation_play_now(
        Some(h.app.world().resource::<crate::plugins::FrameClock>()),
        0.0,
    );
    {
        let mut wall = h.app.world_mut().get_mut::<FighterWall>(h.fighter).unwrap();
        wall.cling_active = true;
        wall.cling_pose = 161;
        wall.wall_normal = Vec3::new(0.0, 0.0, -1.0);
        wall.external_accel = Vec3::new(0.0, 0.0, 60.0);
        wall.remaining = 1.8;
        wall.deadline = Some(now + 1.8);
    }
    h
}

#[test]
fn production_r7_guard_uses_old_carry_without_banking_running_velocity() {
    for old_carry in [0.0, 3.0] {
        let mut h = r7_harness(0);
        let keys = crate::app_state::Keymap::default();
        h.press_binding(keys.move_up);
        for _ in 0..90 {
            h.step_dt(1.0 / 120.0);
        }
        let run = h
            .app
            .world()
            .get::<MotionState>(h.fighter)
            .unwrap()
            .velocity
            .z;
        assert!(run > 10.0, "fixture reaches running speed: {run}");
        h.app
            .world_mut()
            .get_mut::<FighterAttackMotion>(h.fighter)
            .unwrap()
            .carry_z = old_carry;
        let before = h.position();
        h.press_binding(keys.guard);
        h.step_dt(1.0 / 120.0);
        assert_eq!(h.state(), FighterState::Guard);
        let velocity = h
            .app
            .world()
            .get::<MotionState>(h.fighter)
            .unwrap()
            .velocity;
        let expected = old_carry * (1.0 - 9.0 / 120.0);
        assert!(
            (velocity.z - expected).abs() < 0.002,
            "run={run} carry={old_carry} velocity={velocity:?}"
        );
        assert!((h.position().z - before.z - expected / 120.0).abs() < 0.002);
        assert!(!h.airborne());
    }
}

#[test]
fn production_r7_guard_losing_support_hands_off_after_one_mover() {
    let mut h = r7_harness(0);
    h.press_binding(crate::app_state::Keymap::default().guard);
    for _ in 0..30 {
        h.step_dt(1.0 / 120.0);
    }
    assert_eq!(h.state(), FighterState::Guard);
    h.replace_stage_mesh(StageMeshCollider::default());
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation
        .y = 5.0;
    {
        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
        air.air.vertical_speed = 0.0;
        air.air.airborne = false;
        air.ground_contact = true;
    }
    h.step_dt(1.0 / 120.0);
    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
    let expected_speed = meteor_physics::jump::GRAVITY / 120.0;
    assert_eq!(h.state(), FighterState::Name);
    assert!(h.airborne());
    assert_eq!(air.recovery_ground_mode, Some(false));
    assert!((air.air.vertical_speed - expected_speed).abs() < 1e-5);
    assert!((h.position().y - (5.0 + expected_speed / 120.0)).abs() < 1e-5);
}

#[test]
fn production_r7_action_start_cannot_take_legacy_step_down() {
    let mut h = r7_harness(0);
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation
        .y = 0.8;
    h.app
        .world_mut()
        .get_mut::<FighterAir>(h.fighter)
        .unwrap()
        .recovery_ground_mode = None;
    h.press_binding(crate::app_state::Keymap::default().attack);
    h.step_dt(1.0 / 120.0);
    assert!(is_attacking_state(h.state()));
    assert!(
        h.position().y > 0.5,
        "an action's mover cannot be preceded by the old 1.4-unit snap: {:?}",
        h.position()
    );
    assert!(h.airborne());
}

#[test]
fn production_r7_low_speed_authored_ascent_keeps_its_scene_owner() {
    let mut h = spawn_gameplay_harness_with_order(12, true).expect("original ninja resources");
    crate::plugins::install_frame_clock(&mut h.app);
    install_guard_test_animation(&mut h);
    let mut phase = 0;
    let mut sampled_low_ascent = false;
    let mut saw_launch = false;

    for _ in 0..360 {
        h.release_key(KeyCode::KeyW);
        h.release_key(KeyCode::KeyS);
        h.release_mouse(MouseButton::Left);
        let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
        match phase {
            0 => {
                h.press_key(KeyCode::KeyS);
                h.press_mouse(MouseButton::Left);
                phase = 1;
            }
            1 if pose.pose_id == 464 && pose.frame >= 14876.0 => {
                h.press_key(KeyCode::KeyS);
                phase = 2;
            }
            2 => {
                h.press_key(KeyCode::KeyW);
                h.press_mouse(MouseButton::Left);
                phase = 3;
            }
            _ => {}
        }
        let before_y = h.position().y;
        h.step_dt(1.0 / 120.0);
        if h.pose_id() == 471 {
            saw_launch = true;
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            if !sampled_low_ascent
                && air.air.airborne
                && air.air.vertical_speed > 1.0
                && air.air.vertical_speed / 120.0 < 2.0 * meteor_core::MODEL_SCALE
            {
                assert!(
                    h.position().y > before_y,
                    "a low-speed authored step must lift the root: {before_y} -> {}",
                    h.position().y
                );
                sampled_low_ascent = true;
            }
        }
    }
    assert!(saw_launch, "real command must reach the authored launcher");
    assert!(
        sampled_low_ascent,
        "exercise authored ascent below the ordinary foot depth per step"
    );
}

#[test]
fn production_r7_wall_deadline_counts_input_only_frames_once_and_keeps_stalls() {
    for fps in [0, 30, 60, 120, 137] {
        let mut h = r7_wall_harness(fps);
        for _ in 0..120 {
            h.step_dt(1.0 / 120.0);
        }
        let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
        assert!(
            wall.cling_active,
            "target {fps}: expires before one real second"
        );
        assert!(
            (wall.remaining - 0.8).abs() < 0.00001,
            "target {fps}: remaining={}",
            wall.remaining
        );
        h.step_dt(0.5);
        let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
        assert!(wall.cling_active);
        assert!(
            (wall.remaining - 0.3).abs() < 0.00001,
            "target {fps}: stall remaining={}",
            wall.remaining
        );
        h.step_dt(0.35);
        assert!(
            !h.app
                .world()
                .get::<FighterWall>(h.fighter)
                .unwrap()
                .cling_active,
            "target {fps}"
        );
    }
}

#[test]
fn production_r7_wall_deadline_freezes_offline_and_recording_pause() {
    use bevy::state::app::AppExtStates;
    for replay in [false, true] {
        let mut h = r7_wall_harness(30);
        h.app
            .add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(AppState::Playing);
        h.step_dt(1.0 / 30.0);
        let before = h
            .app
            .world()
            .get::<FighterWall>(h.fighter)
            .unwrap()
            .remaining;
        if replay {
            h.app
                .insert_resource(crate::plugins::replay_plugin::MrcPlayback {
                    active: true,
                    paused: true,
                    ..Default::default()
                });
        } else {
            h.app
                .world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Paused);
        }
        for _ in 0..4 {
            h.step_dt(0.5);
        }
        assert!(
            (h.app
                .world()
                .get::<FighterWall>(h.fighter)
                .unwrap()
                .remaining
                - before)
                .abs()
                < 1e-6
        );
        if replay {
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::replay_plugin::MrcPlayback>()
                .paused = false;
        } else {
            h.app
                .world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Playing);
        }
        h.step_dt(1.0 / 30.0);
        let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
        assert!(wall.cling_active);
        assert!(
            (wall.remaining - (before - 1.0 / 30.0)).abs() < 1e-6,
            "replay={replay}: {}",
            wall.remaining
        );
    }
}

#[test]
fn production_r7_airborne_age_and_nearby_floor_cannot_manufacture_contact() {
    let mut h = r7_harness(0);
    let mut mesh = harness_ground();
    mesh.append_world_triangles(
        &[
            [-10.0, 5.0, -10.0],
            [-10.0, 5.0, 10.0],
            [10.0, 5.0, 10.0],
            [10.0, 5.0, -10.0],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    h.replace_stage_mesh(mesh);
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation
        .y = 0.8;
    h.app
        .world_mut()
        .get_mut::<SmState>(h.fighter)
        .unwrap()
        .current = FighterState::Jump;
    {
        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
        air.recovery_ground_mode = None;
        air.air.start_fall(0.0, 0.0);
        air.air.vertical_speed = 5.0;
        air.air.air_time = 3.0;
        air.air.cling_armed = true;

        air.ground_y = 4.0;
    }
    h.step_dt(1.0 / 120.0);
    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
    assert!(h.airborne());
    assert!(air.air.cling_armed);
    assert!(!air.mover_vertical_contact);
    assert!(
        h.position().y > 0.8 && h.position().y < 1.0,
        "{:?}",
        h.position()
    );
}

#[test]
fn production_r7_rising_contact_after_two_point_five_seconds_does_not_land() {
    let mut h = r7_harness(0);
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation
        .y = 0.001;
    h.app
        .world_mut()
        .get_mut::<SmState>(h.fighter)
        .unwrap()
        .current = FighterState::Jump;
    {
        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
        air.recovery_ground_mode = None;
        air.air.start_fall(0.0, 0.0);
        air.air.vertical_speed = 5.0;
        air.air.air_time = 3.0;
        air.air.cling_armed = true;
    }
    h.step_dt(1.0 / 120.0);
    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
    assert!(
        air.mover_vertical_contact,
        "ordinary short foot probe contacts the floor"
    );
    assert!(
        h.airborne(),
        "only the saved downward velocity admits mode1"
    );
    assert_eq!(h.state(), FighterState::Jump);
    assert!(
        !air.air.cling_armed,
        "real vertical contact clears the wall-entry arm"
    );
    assert!(air.air.vertical_speed > 0.0);
}
