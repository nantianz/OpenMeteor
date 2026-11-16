#[derive(Resource)]
struct NpcPlanarCommand(Vec2, bool);

fn npc_planar_test_floor() -> StageCollider {
    let mut mesh = StageMeshCollider::default();
    mesh.append_world_triangles(
        &[
            [-1000.0, 0.0, -1000.0],
            [-1000.0, 0.0, 1000.0],
            [1000.0, 0.0, 1000.0],
            [1000.0, 0.0, -1000.0],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    StageCollider { mesh: Some(mesh) }
}

fn npc_planar_app() -> (App, Entity) {
    let mut app = App::new();
    crate::plugins::configure_fighter_frame_sets(&mut app);
    crate::plugins::fighter_frame::install(&mut app);
    app.init_resource::<Time>()
        .init_resource::<Time<bevy::time::Real>>()
        .init_resource::<crate::app_state::Settings>()
        .insert_resource(NpcPlanarCommand(Vec2::Y, false));
    crate::plugins::install_frame_clock(&mut app);
    app.insert_resource(npc_planar_test_floor())
        .add_plugins(AiPlugin);

    app.add_systems(
        crate::plugins::fighter_frame::FighterUpdate,
        (|command: Res<NpcPlanarCommand>,
          mut query: Query<(&mut AiIntent, &mut MotionState), With<NpcFighter>>| {
            for (mut intent, mut motion) in &mut query {
                intent.locomotion_input = command.0;
                intent.move_dir = decision_space::world_input(command.0, motion.facing);
                motion.movement_mode = if command.1 {
                    meteor_core::MovementMode::Crouch
                } else if command.0.y > 0.0 {
                    meteor_core::MovementMode::Run
                } else if command.0 != Vec2::ZERO {
                    meteor_core::MovementMode::Walk
                } else {
                    meteor_core::MovementMode::Idle
                };
            }
        })
        .after(drive_npc_ai)
        .before(move_npc_fighters),
    );
    app.add_systems(
        crate::plugins::fighter_frame::FighterUpdate,
        npc_ballistic_air_root_motion.in_set(crate::plugins::FighterFrameSet::RootMotion),
    );
    let npc = app
        .world_mut()
        .spawn((
            NpcFighter,
            Fighter {
                character_id: 0,
                player_slot: 1,
                weapon: 6,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Transform::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Run,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            FighterAir {
                ground_contact: true,
                ..default()
            },
            FighterDodge::default(),
            PoseRuntime::idle(),
            EntryBlend::default(),
            SmState::default(),
            CombatState::default(),
            AiIntent::default(),
            Health::default(),
            NpcPatrol::default(),
        ))
        .id();
    (app, npc)
}

fn npc_planar_step(app: &mut App, dt: f32) {
    let duration = std::time::Duration::from_secs_f32(dt);
    app.world_mut().resource_mut::<Time>().advance_by(duration);
    app.world_mut()
        .resource_mut::<Time<bevy::time::Real>>()
        .advance_by(duration);
    app.update();
}

#[test]
fn production_npc_planar_accelerates_and_brakes_with_full_controller_substeps() {
    for fps in [30, 60, 120, 144] {
        let (mut app, npc) = npc_planar_app();
        let dt = 1.0 / fps as f32;
        npc_planar_step(&mut app, dt);
        let first = app.world().get::<MotionState>(npc).unwrap().velocity.z;
        assert!(first > 0.0 && first < 5.0, "fps={fps}, first={first}");
        for _ in 1..fps * 2 {
            npc_planar_step(&mut app, dt);
        }
        let steady = app.world().get::<MotionState>(npc).unwrap().velocity.z;
        assert!(
            (18.76..18.83).contains(&steady),
            "fps={fps}, steady={steady}"
        );
        let before = app.world().get::<Transform>(npc).unwrap().translation.z;
        app.world_mut().resource_mut::<NpcPlanarCommand>().0 = Vec2::ZERO;
        npc_planar_step(&mut app, dt);
        assert!(
            app.world().get::<MotionState>(npc).unwrap().velocity.z > 0.0,
            "releasing input must not instantly replace persistent velocity"
        );
        for _ in 0..fps / 4 {
            npc_planar_step(&mut app, dt);
        }
        let after = app.world().get::<Transform>(npc).unwrap().translation.z;
        assert!(
            (0.58..0.63).contains(&(after - before)),
            "fps={fps}, brake={}",
            after - before
        );
        assert_eq!(app.world().get::<MotionState>(npc).unwrap().velocity.z, 0.0);
    }
}

#[test]
fn production_npc_planar_side_crouch_aim_and_wheel_use_the_shared_drive_gates() {
    for (input, crouch, aim, wheel) in [
        (Vec2::X, false, false, false),
        (Vec2::NEG_Y, false, false, false),
        (Vec2::Y, true, false, false),
        (Vec2::Y, false, true, false),
        (Vec2::Y, false, false, true),
    ] {
        let (mut app, npc) = npc_planar_app();
        *app.world_mut().resource_mut::<NpcPlanarCommand>() = NpcPlanarCommand(input, crouch);
        app.world_mut()
            .get_mut::<crate::components::fighter::GunAimState>(npc)
            .unwrap()
            .aiming = aim;
        app.world_mut()
            .get_mut::<RangedFireGate>(npc)
            .unwrap()
            .wheel_out = wheel;
        for _ in 0..240 {
            npc_planar_step(&mut app, 1.0 / 120.0);
        }
        let velocity = app.world().get::<MotionState>(npc).unwrap().velocity;
        let speed = velocity.xz().length();
        assert!(
            (speed - if wheel { 0.0 } else { 5.14277 }).abs() < 0.015,
            "input={input:?}, crouch={crouch}, aim={aim}, wheel={wheel}, speed={speed}"
        );
    }
}

#[test]
fn production_npc_planar_preserves_action_motion_ownership() {
    for state in [
        FighterState::Guard,
        FighterState::Attack1,
        FighterState::Dodge,
        FighterState::BeHit,
        FighterState::Fake,
        FighterState::Down,
        FighterState::Dead,
    ] {
        let (mut app, npc) = npc_planar_app();
        app.world_mut().get_mut::<SmState>(npc).unwrap().current = state;
        app.world_mut()
            .get_mut::<MotionState>(npc)
            .unwrap()
            .velocity = Vec3::X * 7.0;
        npc_planar_step(&mut app, 1.0 / 30.0);
        assert_eq!(
            app.world().get::<Transform>(npc).unwrap().translation,
            Vec3::ZERO,
            "{state:?}"
        );
        assert_eq!(
            app.world().get::<MotionState>(npc).unwrap().velocity,
            Vec3::X * 7.0,
            "{state:?}"
        );
    }
}

#[test]
fn production_npc_planar_air_zero_input_and_wheel_veto_still_spend_friction() {
    let mut velocities = Vec::new();
    for (input, wheel) in [(Vec2::ZERO, false), (Vec2::Y, true)] {
        let (mut app, npc) = npc_planar_app();
        app.world_mut().resource_mut::<NpcPlanarCommand>().0 = input;
        app.world_mut()
            .get_mut::<RangedFireGate>(npc)
            .unwrap()
            .wheel_out = wheel;
        app.world_mut()
            .get_mut::<Transform>(npc)
            .unwrap()
            .translation
            .y = 100.0;
        {
            let mut air = app.world_mut().get_mut::<FighterAir>(npc).unwrap();
            air.air.start_fall(10.0, 0.0);
            air.air.cling_armed = true;
            air.ledge_fall = false;
        }
        app.world_mut()
            .get_mut::<MotionState>(npc)
            .unwrap()
            .velocity = Vec3::X * 10.0;
        npc_planar_step(&mut app, 1.0 / 120.0);
        let air = app.world().get::<FighterAir>(npc).unwrap();
        assert!((air.air.air_time - 1.0 / 120.0).abs() < 1e-6);
        assert!(
            !air.ledge_fall,
            "ordinary air motion is not a new ledge fall"
        );
        assert!(air.air.cling_armed, "no contact must preserve jump arming");
        let v = app.world().get::<MotionState>(npc).unwrap().velocity;
        assert!(
            (9.9..9.99).contains(&v.x),
            "friction must run with zero acceleration: {v:?}"
        );
        assert!(v.y < 0.0);
        assert_eq!(v.z, 0.0);
        velocities.push(v);
    }
    assert!((velocities[0] - velocities[1]).length() < 1e-6);
}

#[test]
fn production_npc_planar_steep_vertical_contact_enters_mode1_in_the_same_tick() {
    for entry_vertical_speed in [-1.0, 2.0] {
        let (mut app, npc) = npc_planar_app();
        app.world_mut().resource_mut::<NpcPlanarCommand>().0 = Vec2::ZERO;
        let mut slope = StageMeshCollider::default();

        let height = 20.0 * (1.0_f32 - 0.3 * 0.3).sqrt() / 0.3;
        slope.append_world_triangles(
            &[
                [-20.0, height, -20.0],
                [-20.0, height, 20.0],
                [20.0, -height, 20.0],
                [20.0, -height, -20.0],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
        app.insert_resource(StageCollider { mesh: Some(slope) });
        app.world_mut()
            .get_mut::<Transform>(npc)
            .unwrap()
            .translation
            .y = 0.01;
        {
            let mut air = app.world_mut().get_mut::<FighterAir>(npc).unwrap();
            air.air.start_fall(0.0, 0.0);
            air.air.vertical_speed = entry_vertical_speed;
            air.air.cling_armed = true;
            air.ledge_fall = false;
            air.ground_contact = false;
            air.recovery_ground_mode = Some(false);
        }
        app.world_mut()
            .get_mut::<MotionState>(npc)
            .unwrap()
            .velocity = Vec3::Y * entry_vertical_speed;
        app.world_mut().get_mut::<SmState>(npc).unwrap().current = FighterState::Jump;
        app.world_mut().get_mut::<CombatState>(npc).unwrap().state = FighterState::Jump;
        npc_planar_step(&mut app, 1.0 / 120.0);
        let air = app.world().get::<FighterAir>(npc).unwrap();
        assert!(air.mover_vertical_contact);
        assert!(!air.air.cling_armed, "either vertical contact clears +e4");
        assert!(!air.ledge_fall, "mode0 contact is not a new ledge fall");
        assert!(air.carry_decay_steep);
        assert!(
            !air.ground_contact && air.air.airborne,
            "mode1 is distinct from physical walkable support"
        );
        let landed_mode = entry_vertical_speed < 0.0;
        assert_eq!(air.recovery_ground_mode, Some(landed_mode));
        let expected = if landed_mode {
            FighterState::Name
        } else {
            FighterState::Jump
        };
        assert_eq!(app.world().get::<SmState>(npc).unwrap().current, expected);
        assert_eq!(app.world().get::<CombatState>(npc).unwrap().state, expected);
    }
}
