fn hit_follow_app(fps: f32) -> (App, Entity, Entity) {
    use crate::components::fighter::{LockOnController, MotionState, SmState};
    let mut time: Time = Time::default();
    time.advance_by(std::time::Duration::from_secs_f32(1.0 / fps));
    let mut app = App::new();
    app.insert_resource(time)
        .init_resource::<StageCollider>()
        .init_resource::<LockOnController>()
        .init_resource::<CameraBasis>()
        .init_resource::<Settings>()
        .init_resource::<crate::app_state::Keymap>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .configure_sets(
            Update,
            (
                crate::plugins::FighterFrameSet::Movement,
                crate::plugins::FighterFrameSet::CameraFinalize,
            )
                .chain(),
        );
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .press(MouseButton::Left);
    install_camera_input_for_test(&mut app);
    let fighter = app
        .world_mut()
        .spawn((
            LocalFighter,
            Transform::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            SmState::default(),
        ))
        .id();
    let camera = install_follow_camera_for_test(&mut app);
    (app, fighter, camera)
}

#[cfg(feature = "original-assets")]
#[test]
fn production_spawn_heading_initializes_before_input_and_survives_session_reset() {
    use crate::components::fighter::MotionState;
    use bevy::ecs::system::RunSystemOnce;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
    for stage in ["sn01", "sn03", "sn04", "sn05", "sn10", "sn14"] {
        let source = crate::text_codec::decode_text(
            &std::fs::read(root.join(format!("{stage}_.pst"))).unwrap(),
        );
        let definition = meteor_scene::mission_runtime::parse_mission_definition(&source);
        let facing = match definition.values.get("PlayerSpawnDir").unwrap() {
            meteor_scene::pst_runtime::PstValue::Num(value) => (*value as f32).to_radians(),
            _ => panic!("numeric PlayerSpawnDir expected"),
        };
        for fps in [30.0_f32, 60.0, 120.0] {
            for pointer in [false, true] {
                let (mut app, fighter, camera) = hit_follow_app(fps);
                for session in 0..2 {
                    app.world_mut()
                        .run_system_once(reset_follow_camera)
                        .unwrap();
                    app.world_mut()
                        .get_mut::<MotionState>(fighter)
                        .unwrap()
                        .facing = facing;
                    if pointer {
                        app.world_mut()
                            .resource_mut::<Messages<MouseMotion>>()
                            .write(MouseMotion {
                                delta: Vec2::new(28.0, 0.0),
                            });
                    }
                    app.update();
                    let expected = facing - if pointer { 20.0 * DEG } else { 0.0 };
                    let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
                    let basis = app.world().resource::<CameraBasis>().yaw;
                    assert!((basis - expected).abs() < 1e-5,
                        "{stage} fps{fps} session{session}: first movement basis {basis} != {expected}");
                    assert!((cam.yaw - expected).sin().abs() < 1e-5);
                    assert!((cam.smooth_yaw - expected).sin().abs() < 1e-5);
                    let offset =
                        app.world().get::<Transform>(camera).unwrap().translation - cam.target;
                    let back = Vec3::new(offset.x, 0.0, offset.z).normalize();
                    assert!(
                        back.abs_diff_eq(Vec3::new(-expected.sin(), 0.0, -expected.cos()), 1e-5)
                    );

                    app.world_mut()
                        .get_mut::<MotionState>(fighter)
                        .unwrap()
                        .facing += 0.5;
                    app.update();
                    assert!(
                        (app.world().get::<ThirdPersonState>(camera).unwrap().yaw - expected - 0.5)
                            .sin()
                            .abs()
                            < 1e-5
                    );
                }
            }
        }
    }
}

#[test]
fn production_hit_follow_survives_repeated_turns_down_and_final_recovery_frame() {
    use crate::components::fighter::{FighterHitReaction, MotionState, SmState};
    use meteor_combat::hit_reaction::{getup_reaction_for_pose, HitReactKind};
    use meteor_core::FighterState;
    for fps in [30.0_f32, 60.0, 120.0] {
        let (mut app, fighter, camera) = hit_follow_app(fps);

        for (state, kind, pose, facing) in [
            (FighterState::BeHit, HitReactKind::BeHit, 104, 0.7_f32),
            (FighterState::BeHit, HitReactKind::BeHit, 105, 2.9),
            (FighterState::Down, HitReactKind::Down, 113, 3.4),
            (FighterState::Dodge, HitReactKind::Roll, 176, 6.5),
            (FighterState::BeHit, HitReactKind::GetUp, 120, 7.1),
        ] {
            let mut runtime = FighterHitReaction::default();
            let mut reaction = getup_reaction_for_pose(pose, 0, 10);
            reaction.kind = kind;
            runtime.begin(reaction, false);
            app.world_mut().entity_mut(fighter).insert(runtime);
            app.world_mut().get_mut::<SmState>(fighter).unwrap().current = state;
            app.world_mut()
                .get_mut::<MotionState>(fighter)
                .unwrap()
                .facing = facing;
            app.world_mut()
                .get_mut::<Transform>(fighter)
                .unwrap()
                .translation += Vec3::new(0.2, 0.1, -0.3);
            app.world_mut()
                .resource_mut::<Messages<MouseMotion>>()
                .write(MouseMotion {
                    delta: Vec2::new(28.0, 2.0),
                });
            let pitch_before = app.world().get::<ThirdPersonState>(camera).unwrap().pitch;
            app.update();
            let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
            let back = Vec3::new(-facing.sin(), 0.0, -facing.cos());
            let offset = app.world().get::<Transform>(camera).unwrap().translation - cam.target;
            assert!(
                offset.normalize().dot(back) > 0.8,
                "fps={fps} state={state:?}: {offset:?}"
            );
            assert!((cam.yaw - facing).sin().abs() < 1e-5);
            assert!((cam.smooth_yaw - facing).sin().abs() < 1e-5);
            assert!(
                cam.pitch > pitch_before,
                "pitch remains independently usable"
            );
            assert!(!cam.free_look_pending);
        }

        app.world_mut()
            .get_mut::<FighterHitReaction>(fighter)
            .unwrap()
            .clear();
        app.world_mut().get_mut::<SmState>(fighter).unwrap().current = FighterState::Name;
        app.world_mut()
            .get_mut::<MotionState>(fighter)
            .unwrap()
            .facing = 7.3;
        app.update();
        let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
        assert!((cam.yaw - 7.3).sin().abs() < 1e-5);
        app.update();
        assert!(
            (app.world().resource::<CameraBasis>().yaw - 7.3)
                .sin()
                .abs()
                < 1e-5,
            "the next movement tick must use the recovered camera basis"
        );

        let before = app.world().get::<ThirdPersonState>(camera).unwrap().yaw;
        app.world_mut()
            .resource_mut::<Messages<MouseMotion>>()
            .write(MouseMotion {
                delta: Vec2::new(28.0, 0.0),
            });
        app.update();
        let after = app.world().get::<ThirdPersonState>(camera).unwrap().yaw;
        assert!((after - before + 20.0 * DEG).abs() < 1e-5);
    }
}

#[test]
fn production_free_follow_uses_model_heading_in_every_action_state() {
    use crate::components::fighter::{MotionState, SmState};
    use meteor_core::FighterState;
    for state in [
        FighterState::Name,
        FighterState::View,
        FighterState::Think,
        FighterState::Attack1,
        FighterState::Attack2,
        FighterState::Attack3,
        FighterState::Guard,
        FighterState::Dodge,
        FighterState::Jump,
        FighterState::Look,
        FighterState::Burst,
        FighterState::Cure,
        FighterState::GetItem,
        FighterState::Spawn,
        FighterState::BeHit,
        FighterState::Down,
        FighterState::Fake,
        FighterState::Dead,
    ] {
        let (mut app, fighter, camera) = hit_follow_app(60.0);
        app.world_mut().get_mut::<SmState>(fighter).unwrap().current = state;
        app.world_mut()
            .get_mut::<MotionState>(fighter)
            .unwrap()
            .facing = 1.5;
        {
            let mut cam = app.world_mut().get_mut::<ThirdPersonState>(camera).unwrap();
            cam.free_look_pending = true;
            cam.free_look_from_yaw = -0.8;
        }
        app.update();
        let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
        assert!((cam.yaw - 1.5).abs() < 1e-6, "{state:?}");
        assert_eq!(cam.smooth_yaw, cam.yaw);
        assert!(!cam.free_look_pending);

        app.world_mut().entity_mut(fighter).remove::<SmState>();
        app.world_mut()
            .get_mut::<MotionState>(fighter)
            .unwrap()
            .facing = 3.4;
        app.update();
        let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
        assert!(
            (cam.yaw - 3.4).sin().abs() < 1e-6,
            "{state:?} without a state component"
        );
    }
}

#[test]
fn production_hit_follow_preserves_target_lock_spectator_and_freecam_hold() {
    use crate::components::fighter::{LockOnController, MotionState, SmState};
    for mode in ["lock", "spectator", "hold"] {
        let (mut app, fighter, camera) = hit_follow_app(60.0);
        app.world_mut().get_mut::<SmState>(fighter).unwrap().current =
            meteor_core::FighterState::Down;
        app.world_mut()
            .get_mut::<MotionState>(fighter)
            .unwrap()
            .facing = 1.5;
        match mode {
            "lock" => {
                let target = app
                    .world_mut()
                    .spawn(Transform::from_xyz(0.0, 0.0, 4.0))
                    .id();
                app.world_mut()
                    .resource_mut::<LockOnController>()
                    .lock_entity = Some(target);
            }
            "spectator" => {
                app.init_resource::<spectator::SpectatorState>();
                let mut state = app.world_mut().resource_mut::<spectator::SpectatorState>();
                state.active = true;
                state.target = Some(fighter);
            }
            "hold" => app.world_mut().resource_mut::<CameraMode>().freecam_hold = true,
            _ => unreachable!(),
        }
        app.update();
        let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
        assert_eq!(
            cam.yaw, 0.0,
            "{mode}: do not bind the independent view to reaction facing"
        );
        if mode == "lock" {
            assert!(cam.lock_was_active);
            assert!(app.world().get::<Transform>(camera).unwrap().translation.z < 0.0);
        }
    }
}
