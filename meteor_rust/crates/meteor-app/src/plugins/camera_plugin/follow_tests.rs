use super::*;
use crate::components::fighter::{LockOnController, MotionState, SmState};

fn follow_app(fps: f32) -> (App, Entity, Entity) {
    let mut app = App::new();
    crate::plugins::configure_fighter_frame_sets(&mut app);
    let mut time: Time = Time::default();
    time.advance_by(std::time::Duration::from_secs_f32(1.0 / fps));
    app.insert_resource(time)
        .init_resource::<Settings>()
        .init_resource::<crate::app_state::Keymap>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<CameraBasis>()
        .init_resource::<LockOnController>()
        .init_resource::<StageCollider>();
    install_camera_input_for_test(&mut app);
    let camera = install_follow_camera_for_test(&mut app);
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    let fighter = app
        .world_mut()
        .spawn((
            LocalFighter,
            Transform::from_xyz(13.0, 8.0, -19.0),
            MotionState {
                facing: 1.2,
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
            },
            SmState::default(),
        ))
        .id();
    (app, fighter, camera)
}

fn assert_heading(app: &App, fighter: Entity, camera: Entity) {
    let facing = app.world().get::<MotionState>(fighter).unwrap().facing;
    let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
    let eye = app.world().get::<Transform>(camera).unwrap();
    let forward = eye.rotation * Vec3::NEG_Z;
    let actual = Vec2::new(forward.x, forward.z).normalize();
    let expected = Vec2::new(facing.sin(), facing.cos());
    assert!(
        actual.distance(expected) < 2e-5,
        "visible heading {actual:?} differs from actor {expected:?}; yaw={}, smooth={}",
        cam.yaw,
        cam.smooth_yaw
    );
    assert!((cam.yaw - facing).sin().abs() < 2e-5);
}

#[test]
fn production_free_follow_keeps_heading_after_root_turn_and_large_pointer_batches() {
    for fps in [30.0, 60.0, 120.0] {
        let (mut app, fighter, camera) = follow_app(fps);

        app.update();
        assert_heading(&app, fighter, camera);
        for frame in 0..48 {
            if frame == 12 || frame == 32 {
                let mut motion = app.world_mut().get_mut::<MotionState>(fighter).unwrap();
                motion.facing = if frame == 12 { -3.05 } else { 3.08 };
            }
            let before = app.world().get::<MotionState>(fighter).unwrap().facing;
            let deltas = if frame % 2 == 0 {
                [730.0, -190.0, 63.0]
            } else {
                [-810.0, 95.0, -126.0]
            };
            for dx in deltas {
                app.world_mut()
                    .resource_mut::<Messages<MouseMotion>>()
                    .write(MouseMotion {
                        delta: Vec2::new(dx, 0.0),
                    });
            }
            app.update();
            let facing = app.world().get::<MotionState>(fighter).unwrap().facing;
            let expected = before - deltas.iter().sum::<f32>() * (100.0 / 140.0) * DEG;
            assert!(
                (facing - expected).abs() < 2e-5,
                "mouse delta lost or applied twice"
            );
            assert!(
                (app.world().resource::<CameraBasis>().yaw - facing)
                    .sin()
                    .abs()
                    < 2e-5
            );
            assert_heading(&app, fighter, camera);
            app.update();
            assert_heading(&app, fighter, camera);
            assert_eq!(
                app.world().get::<MotionState>(fighter).unwrap().facing,
                facing
            );
        }
    }
}

#[test]
fn production_mouse_pitch_uses_native_half_yaw_sensitivity_and_input_gates() {
    use meteor_core::FighterState;

    for fps in [30.0, 60.0, 120.0] {
        for mouse_speed in [0.0, 49.0, 99.0] {
            for reverse in [false, true] {
                for state in [FighterState::Name, FighterState::Attack1] {
                    for locked in [false, true] {
                        let (mut app, fighter, camera) = follow_app(fps);
                        app.world_mut().get_mut::<SmState>(fighter).unwrap().current = state;
                        {
                            let mut settings = app.world_mut().resource_mut::<Settings>();
                            settings.mouse_speed = mouse_speed;
                            settings.reverse_mouse = reverse;
                        }
                        if locked {
                            let target = app
                                .world_mut()
                                .spawn(Transform::from_xyz(20.0, 8.0, -5.0))
                                .id();
                            app.world_mut()
                                .resource_mut::<LockOnController>()
                                .lock_entity = Some(target);
                        }
                        app.update();
                        let initial_pitch =
                            app.world().get::<ThirdPersonState>(camera).unwrap().pitch;
                        let initial_facing =
                            app.world().get::<MotionState>(fighter).unwrap().facing;

                        pointer_event(&mut app, Vec2::splat(28.0));
                        app.update();
                        let pitch = app.world().get::<ThirdPersonState>(camera).unwrap().pitch;
                        let expected_pitch = initial_pitch
                            + if locked {
                                0.0
                            } else {
                                (mouse_speed + 1.0) * 0.1 * DEG * if reverse { -1.0 } else { 1.0 }
                            };
                        assert!(
                            (pitch - expected_pitch).abs() < 2e-6,
                            "pitch fps={fps}, speed={mouse_speed}, reverse={reverse}, state={state:?}, lock={locked}"
                        );
                        let facing = app.world().get::<MotionState>(fighter).unwrap().facing;
                        let expected_facing = initial_facing
                            - if locked || state != FighterState::Name {
                                0.0
                            } else {
                                (mouse_speed + 1.0) * 0.2 * DEG
                            };
                        assert!((facing - expected_facing).abs() < 2e-6);
                        app.update();
                        assert_eq!(
                            app.world().get::<ThirdPersonState>(camera).unwrap().pitch,
                            pitch
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn production_transfer_rebases_height_and_preserves_pitch_and_zoom() {
    for fps in [30.0, 60.0, 120.0] {
        let (mut app, fighter, camera) = follow_app(fps);
        app.update();
        {
            let mut cam = app.world_mut().get_mut::<ThirdPersonState>(camera).unwrap();
            cam.pitch = 0.7;
            cam.distance = 11.5;
        }
        for position in [Vec3::new(-80.0, 45.0, 90.0), Vec3::new(60.0, -20.0, -120.0)] {
            app.world_mut()
                .get_mut::<Transform>(fighter)
                .unwrap()
                .translation = position;
            app.world_mut()
                .get_mut::<MotionState>(fighter)
                .unwrap()
                .facing = -2.1;
            app.world_mut()
                .entity_mut(fighter)
                .insert(CameraFollowReset);
            app.update();
            assert_heading(&app, fighter, camera);
            let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
            assert!(cam.target.distance(position + Vec3::Y * LOOK_HEIGHT) < 1e-5);
            assert_eq!((cam.pitch, cam.distance), (0.7, 11.5));
            assert!(!cam.free_look_pending);
            assert!(cam.position.distance(free_ideal(cam, -2.1, 0.7)) < 1e-5);
            assert!(app.world().get::<CameraFollowReset>(fighter).is_none());
            let eye = cam.position;
            app.update();
            assert!(
                app.world()
                    .get::<Transform>(camera)
                    .unwrap()
                    .translation
                    .distance(eye)
                    < 1e-5
            );
        }
    }
}

fn lifecycle_app(fps: f32) -> (App, Entity, Entity, Entity) {
    let mut app = App::new();
    app.add_plugins(bevy::state::app::StatesPlugin)
        .insert_state(AppState::Playing)
        .init_resource::<Settings>()
        .init_resource::<crate::app_state::Keymap>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<LockOnController>()
        .init_resource::<StageCollider>()
        .init_resource::<crate::plugins::mobile_controls_plugin::MobileLookDelta>()
        .add_message::<MouseMotion>()
        .add_message::<MouseWheel>()
        .add_plugins(CameraPlugin);
    crate::plugins::configure_fighter_frame_sets(&mut app);
    let mut time: Time = Time::default();
    time.advance_by(std::time::Duration::from_secs_f32(1.0 / fps));
    app.insert_resource(time);
    let window = app
        .world_mut()
        .spawn((Window::default(), CursorOptions::default(), PrimaryWindow))
        .id();
    let fighter = app
        .world_mut()
        .spawn((
            LocalFighter,
            Transform::from_xyz(13.0, 8.0, -19.0),
            MotionState {
                facing: 1.2,
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
            },
            SmState::default(),
        ))
        .id();
    app.update();
    app.update();
    let camera = app
        .world_mut()
        .query_filtered::<Entity, With<GameplayCamera>>()
        .single(app.world())
        .unwrap();
    (app, fighter, camera, window)
}

fn pointer_event(app: &mut App, delta: Vec2) {
    app.world_mut()
        .resource_mut::<Messages<MouseMotion>>()
        .write(MouseMotion { delta });
}

#[test]
fn production_camera_pause_resume_discards_menu_motion_and_preserves_view() {
    for fps in [30.0, 60.0, 120.0] {
        for locked in [false, true] {
            let (mut app, fighter, camera, window) = lifecycle_app(fps);
            if locked {
                let target = app
                    .world_mut()
                    .spawn(Transform::from_xyz(28.0, 8.0, -13.0))
                    .id();
                app.world_mut()
                    .resource_mut::<LockOnController>()
                    .lock_entity = Some(target);
            }
            {
                let mut cam = app.world_mut().get_mut::<ThirdPersonState>(camera).unwrap();
                cam.pitch = 0.6;
                cam.distance = 11.5;
            }
            app.update();
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Paused);
            app.update();
            let paused_eye = *app.world().get::<Transform>(camera).unwrap();
            for _ in 0..3 {
                pointer_event(&mut app, Vec2::new(252.0, 80.0));
                app.world_mut()
                    .resource_mut::<Messages<MouseWheel>>()
                    .write(MouseWheel {
                        unit: bevy::input::mouse::MouseScrollUnit::Line,
                        x: 0.0,
                        y: 3.0,
                        window,
                        phase: bevy::input::touch::TouchPhase::Moved,
                    });
                app.world_mut()
                    .resource_mut::<crate::plugins::mobile_controls_plugin::MobileLookDelta>()
                    .0 = Vec2::new(400.0, 200.0);
                app.update();
                assert_eq!(*app.world().get::<Transform>(camera).unwrap(), paused_eye);
            }
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(AppState::Playing);

            pointer_event(&mut app, Vec2::new(252.0, 80.0));
            app.update();
            assert_eq!(
                app.world().get::<MotionState>(fighter).unwrap().facing,
                1.2,
                "resume consumed menu motion"
            );
            let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
            assert_eq!((cam.pitch, cam.distance), (0.6, 11.5));
            if !locked {
                assert_heading(&app, fighter, camera);
            }
            app.world_mut()
                .resource_mut::<LockOnController>()
                .lock_entity = None;
            pointer_event(&mut app, Vec2::new(14.0, 0.0));
            app.update();
            assert!(
                (app.world().get::<MotionState>(fighter).unwrap().facing - (1.2 - 10.0 * DEG))
                    .abs()
                    < 1e-5
            );
            assert_heading(&app, fighter, camera);
        }
    }
}

#[test]
fn production_camera_guard_and_lock_discard_horizontal_shaking_until_release() {
    for fps in [30.0, 60.0, 120.0] {
        for locked in [false, true] {
            let (mut app, fighter, camera) = follow_app(fps);
            app.init_resource::<crate::plugins::mobile_controls_plugin::MobileLookDelta>();
            if locked {
                let target = app
                    .world_mut()
                    .spawn(Transform::from_xyz(28.0, 8.0, -13.0))
                    .id();
                app.world_mut()
                    .resource_mut::<LockOnController>()
                    .lock_entity = Some(target);
            } else {
                app.world_mut().get_mut::<SmState>(fighter).unwrap().current =
                    meteor_core::FighterState::Guard;
            }
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::ArrowRight);
            for frame in 0..24 {
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .clear_just_pressed(KeyCode::ArrowRight);
                pointer_event(
                    &mut app,
                    Vec2::new(if frame % 2 == 0 { 900.0 } else { -650.0 }, 0.1),
                );
                app.world_mut()
                    .resource_mut::<crate::plugins::mobile_controls_plugin::MobileLookDelta>()
                    .0 = Vec2::new(700.0, 0.0);
                app.update();
                assert_eq!(
                    app.world().get::<MotionState>(fighter).unwrap().facing,
                    1.2,
                    "locked={locked}"
                );
                assert!(
                    (app.world().resource::<CameraBasis>().yaw - 1.2).abs() < 1e-5,
                    "hidden horizontal input leaked into movement basis"
                );
                if !locked {
                    assert_heading(&app, fighter, camera);
                }
            }
            let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
            assert_eq!(cam.yaw_axis_hold, 0.0);
            if locked {
                assert_eq!(cam.pitch, DEFAULT_PITCH);
            } else {
                assert!(cam.pitch > DEFAULT_PITCH);
            }
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .release(KeyCode::ArrowRight);
            app.world_mut()
                .resource_mut::<LockOnController>()
                .lock_entity = None;
            app.world_mut().get_mut::<SmState>(fighter).unwrap().current =
                meteor_core::FighterState::Name;
            app.update();
            assert_heading(&app, fighter, camera);
            assert_eq!(app.world().get::<MotionState>(fighter).unwrap().facing, 1.2);
            pointer_event(&mut app, Vec2::new(14.0, 0.0));
            app.update();
            assert!(
                (app.world().get::<MotionState>(fighter).unwrap().facing - (1.2 - 10.0 * DEG))
                    .abs()
                    < 1e-5
            );
            assert_heading(&app, fighter, camera);
        }
    }
}

#[test]
fn production_camera_focus_and_console_reentry_discard_capture_boundary_motion() {
    use crate::plugins::debug_plugin::DebugConsoleState;
    for console in [false, true] {
        let (mut app, fighter, camera, window) = lifecycle_app(60.0);
        app.init_resource::<DebugConsoleState>();
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        if console {
            app.world_mut().resource_mut::<DebugConsoleState>().open = true;
        } else {
            app.world_mut().get_mut::<Window>(window).unwrap().focused = false;
        }
        pointer_event(&mut app, Vec2::new(252.0, 90.0));
        app.update();
        assert_eq!(app.world().get::<MotionState>(fighter).unwrap().facing, 1.2);
        if console {
            app.world_mut().resource_mut::<DebugConsoleState>().open = false;
        } else {
            app.world_mut().get_mut::<Window>(window).unwrap().focused = true;
        }
        pointer_event(&mut app, Vec2::new(252.0, 90.0));
        app.world_mut()
            .resource_mut::<Messages<MouseWheel>>()
            .write(MouseWheel {
                unit: bevy::input::mouse::MouseScrollUnit::Line,
                x: 0.0,
                y: 3.0,
                window,
                phase: bevy::input::touch::TouchPhase::Moved,
            });
        app.update();
        assert_heading(&app, fighter, camera);
        assert_eq!(app.world().get::<MotionState>(fighter).unwrap().facing, 1.2);
        let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
        assert_eq!((cam.pitch, cam.distance), (DEFAULT_PITCH, DEFAULT_DISTANCE));
        assert!(
            !app.world().resource::<Messages<MouseWheel>>().is_empty(),
            "UI keeps its wheel messages"
        );
        pointer_event(&mut app, Vec2::new(14.0, 0.0));
        app.update();
        assert!(
            (app.world().get::<MotionState>(fighter).unwrap().facing - (1.2 - 10.0 * DEG)).abs()
                < 1e-5
        );
        assert_heading(&app, fighter, camera);
    }
}

#[test]
fn production_camera_locked_view_matches_idle_input_and_unlocks_behind_actor() {
    for fps in [30.0, 60.0, 120.0] {
        let (mut shaking, fighter, camera) = follow_app(fps);
        let (mut idle, idle_fighter, idle_camera) = follow_app(fps);
        for app in [&mut shaking, &mut idle] {
            let target = app
                .world_mut()
                .spawn(Transform::from_xyz(28.0, 8.0, -13.0))
                .id();
            app.world_mut()
                .resource_mut::<LockOnController>()
                .lock_entity = Some(target);
        }
        for _ in 0..48 {
            pointer_event(&mut shaking, Vec2::new(720.0, -180.0));
            shaking.update();
            idle.update();
            assert_eq!(
                shaking.world().get::<Transform>(camera).unwrap(),
                idle.world().get::<Transform>(idle_camera).unwrap(),
                "mouse shaking changed the locked view",
            );
        }
        for app in [&mut shaking, &mut idle] {
            app.world_mut()
                .resource_mut::<LockOnController>()
                .lock_entity = None;
            app.update();
        }
        assert_heading(&shaking, fighter, camera);
        assert_heading(&idle, idle_fighter, idle_camera);
        assert_eq!(
            shaking.world().get::<Transform>(camera).unwrap(),
            idle.world().get::<Transform>(idle_camera).unwrap(),
            "hidden mouse motion changed the view on unlock",
        );
    }
}
