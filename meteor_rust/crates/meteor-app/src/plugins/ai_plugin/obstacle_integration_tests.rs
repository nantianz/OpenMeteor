fn obstacle_integration_wall(height: f32) -> StageMeshCollider {
    let mut mesh = StageMeshCollider::default();
    mesh.append_world_triangles(
        &[
            [-20.0, 0.0, 1.2],
            [20.0, 0.0, 1.2],
            [20.0, height, 1.2],
            [-20.0, height, 1.2],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    mesh
}

fn obstacle_integration_app(mesh: StageMeshCollider) -> (App, Entity) {
    let (mut app, npc) = perception_test_app(BehaviorMode::Follow, Vec3::Z * 20.0, None);
    app.init_resource::<CharacterContactFrame>()
        .insert_resource(crate::plugins::FrameClock {
            sim_dt: 0.1,
            ..default()
        })
        .insert_resource(StageCollider { mesh: Some(mesh) })
        .add_systems(Update, finish_npc_script_tick.after(drive_npc_ai));
    app.world_mut()
        .entity_mut(npc)
        .insert((NpcPerformance::default(), SmState::default()));
    app.world_mut().get_mut::<MissionNpc>(npc).unwrap().think = 100.0;
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
        assert!(patrol.navigation.request_path(
            Some(&line_graph()),
            None,
            Vec3::ZERO,
            Vec3::new(0.0, 30.0 * meteor_core::MODEL_SCALE, 20.0),
            || 5,
        ));
        patrol.path_heading = Some(0.0);
        patrol.path_input = Vec2::Y;
        patrol.think_remaining = 5.0;
        patrol.route = vec![0, 2];
        patrol.index = 1;
    }
    (app, npc)
}

fn obstacle_record_real_contact(app: &mut App, npc: Entity) {
    let mesh = app
        .world()
        .resource::<StageCollider>()
        .mesh
        .clone()
        .unwrap();
    let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    mover.set_mesh_collider(mesh);
    let mut frame = app.world_mut().resource_mut::<CharacterContactFrame>();
    let (_, _, _, _, blocked, actor_changed) =
        crate::plugins::character_contact_plugin::resolve_fighter_xz_with_contacts(
            Some(&mut frame),
            npc,
            &mover,
            Point3::ZERO,
            0.0,
            1.0,
            false,
        );
    assert!(blocked[1]);
    assert!(!actor_changed);
    assert!(frame.last_mover_scene_contact(npc));
}

fn obstacle_seed(first: impl Fn(u32) -> bool) -> u64 {
    (1..100_000)
        .find(|seed| {
            let mut rng = *seed;
            first((next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32)
        })
        .unwrap()
}

#[test]
fn obstacle_production_real_contact_requests_jump_after_prior_frame_clock() {
    let (mut app, npc) = obstacle_integration_app(obstacle_integration_wall(2.0));
    obstacle_record_real_contact(&mut app, npc);
    for _ in 0..4 {
        app.update();
        assert!(app
            .world()
            .get::<NpcPerformance>(npc)
            .unwrap()
            .ai_jump
            .is_none());
        assert!(app
            .world()
            .get::<NpcPatrol>(npc)
            .unwrap()
            .avoidance
            .is_none());
    }
    app.world_mut().get_mut::<NpcPatrol>(npc).unwrap().rng = obstacle_seed(|roll| roll % 3 != 0);
    app.update();
    assert_eq!(
        app.world().get::<NpcPerformance>(npc).unwrap().ai_jump,
        Some(true)
    );
    let intent = app.world().get::<AiIntent>(npc).unwrap();
    assert_eq!(intent.locomotion_input, Vec2::Y);
    assert!(app
        .world()
        .get::<NpcPatrol>(npc)
        .unwrap()
        .navigation
        .in_progress());
}

#[test]
fn obstacle_production_standing_without_scene_contact_never_infers_stuck() {
    let (mut app, npc) = obstacle_integration_app(obstacle_integration_wall(12.0));
    for _ in 0..20 {
        app.update();
    }
    let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
    assert!(patrol.avoidance.is_none());
    assert_eq!(patrol.collision_clock.blocked_seconds, 0.0);
    assert!(patrol.navigation.in_progress());
}

#[test]
fn obstacle_production_child_preserves_parent_and_does_not_accelerate_think() {
    let (mut app, npc) = obstacle_integration_app(obstacle_integration_wall(12.0));
    obstacle_record_real_contact(&mut app, npc);
    for _ in 0..5 {
        app.update();
    }
    let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
    let child = patrol.avoidance.as_ref().unwrap();
    assert_eq!(child.ticks, -1, "push does not force a Think callback");
    assert!(patrol.think_remaining > 4.0);
    assert_eq!(patrol.route, [0, 2]);
    assert_eq!(patrol.index, 1);
    assert!(!patrol.navigation.in_progress());
    assert_eq!(
        app.world().get::<MissionNpc>(npc).unwrap().forced_behavior,
        Some(BehaviorMode::Follow)
    );

    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
        patrol.avoidance.as_mut().unwrap().ticks = 3;
        patrol.think_remaining = 0.0;
        patrol.rng = obstacle_seed(|roll| roll % 270 == 135);
    }
    app.update();
    let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
    assert_eq!(patrol.avoidance.as_ref().unwrap().ticks, 4);
    assert!(!patrol.avoidance.as_ref().unwrap().forward);
    assert_eq!(patrol.route, [0, 2]);
    assert_eq!(
        app.world().get::<MotionState>(npc).unwrap().facing,
        0.0,
        "new desired heading is consumed next frame"
    );

    app.world_mut().resource_mut::<StageCollider>().mesh = Some(StageMeshCollider::default());
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
        patrol.avoidance.as_mut().unwrap().ticks = 6;
        patrol.think_remaining = 0.0;
    }
    app.update();
    let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
    assert!(patrol.avoidance.is_none());
    assert!(!patrol.navigation.in_progress());
    assert_eq!(patrol.behavior_ticks, 0);
    assert_eq!(patrol.route, [0, 2]);
    assert_eq!(patrol.index, 1);
    assert_eq!(
        patrol.collision_clock,
        obstacle_avoidance::CollisionClock::default()
    );
    assert_eq!(
        app.world().get::<MissionNpc>(npc).unwrap().forced_behavior,
        Some(BehaviorMode::Follow)
    );
}

#[test]
fn obstacle_production_clear_callback_holds_prior_input_until_next_frame() {
    let (mut app, npc) = obstacle_integration_app(StageMeshCollider::default());
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
        patrol.stop_navigation();
        patrol.avoidance = Some(ObstacleCallback {
            ticks: 0,
            desired_yaw: 0.0,
            forward: false,
        });
        patrol.think_remaining = 0.0;
    }
    app.update();
    assert!(
        app.world()
            .get::<NpcPatrol>(npc)
            .unwrap()
            .avoidance
            .as_ref()
            .unwrap()
            .forward
    );
    assert_eq!(
        app.world().get::<AiIntent>(npc).unwrap().locomotion_input,
        Vec2::ZERO
    );
    app.update();
    assert_eq!(
        app.world().get::<AiIntent>(npc).unwrap().locomotion_input,
        Vec2::Y
    );
}

#[test]
fn obstacle_production_mode11_does_not_accumulate_scene_contact() {
    for state in [FighterState::Down, FighterState::Fake] {
        let (mut app, npc) = obstacle_integration_app(obstacle_integration_wall(12.0));
        obstacle_record_real_contact(&mut app, npc);
        app.world_mut().get_mut::<SmState>(npc).unwrap().current = state;
        for _ in 0..5 {
            app.update();
        }
        let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
        assert_eq!(patrol.collision_clock.blocked_seconds, 0.0, "{state:?}");
        assert!(patrol.avoidance.is_none());
    }
}

#[test]
fn obstacle_production_ballistic_landing_restores_turn_and_walk_state() {
    for fps in [30, 60, 120] {
        for state in [FighterState::Jump, FighterState::Attack1] {
            let mut floor = StageMeshCollider::default();
            floor.append_world_triangles(
                &[
                    [-20.0, 0.0, -20.0],
                    [-20.0, 0.0, 20.0],
                    [20.0, 0.0, 20.0],
                    [20.0, 0.0, -20.0],
                ],
                &[0, 1, 2, 0, 2, 3],
            );
            let mut app = App::new();
            crate::plugins::configure_fighter_frame_sets(&mut app);
            app.init_resource::<Time>()
                .insert_resource(StageCollider { mesh: Some(floor) })
                .add_systems(
                    Update,
                    move_npc_fighters.in_set(crate::plugins::FighterFrameSet::Locomotion),
                )
                .add_systems(
                    Update,
                    npc_ballistic_air_root_motion
                        .in_set(crate::plugins::FighterFrameSet::RootMotion),
                );
            let mut air = FighterAir::default();
            air.air.start_fall(0.0, 0.0);
            let npc = app
                .world_mut()
                .spawn((
                    NpcFighter,
                    Fighter {
                        character_id: 1,
                        player_slot: 1,
                        weapon: 6,
                        weapon_slots: [6, 5],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    },
                    AiIntent::default(),
                    Transform::from_xyz(0.0, 3.0, 0.0),
                    MotionState {
                        movement_mode: meteor_core::MovementMode::Airborne,
                        velocity: Vec3::ZERO,
                        facing: 0.0,
                    },
                    air,
                    FighterDodge::default(),
                    PoseRuntime::idle(),
                    EntryBlend::default(),
                    SmState {
                        current: state,
                        ..default()
                    },
                    CombatState { state },
                ))
                .id();
            for _ in 0..fps * 5 {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
                app.update();
                if !app.world().get::<FighterAir>(npc).unwrap().air.airborne {
                    break;
                }
            }
            let air = app.world().get::<FighterAir>(npc).unwrap();
            assert!(
                !air.air.airborne,
                "{state:?} {fps} FPS must contact the real mesh"
            );
            let expected = if state == FighterState::Jump {
                FighterState::Name
            } else {
                state
            };
            assert_eq!(app.world().get::<SmState>(npc).unwrap().current, expected);
            assert_eq!(app.world().get::<CombatState>(npc).unwrap().state, expected);
            if state == FighterState::Jump {
                assert!(npc_can_turn(app.world().get::<SmState>(npc), false));
                app.world_mut()
                    .get_mut::<MotionState>(npc)
                    .unwrap()
                    .movement_mode = meteor_core::MovementMode::Run;
                app.world_mut().get_mut::<AiIntent>(npc).unwrap().move_dir = Vec2::Y;
                app.world_mut()
                    .get_mut::<AiIntent>(npc)
                    .unwrap()
                    .locomotion_input = Vec2::Y;
                let before = app.world().get::<Transform>(npc).unwrap().translation;
                app.update();
                assert!(
                    app.world().get::<Transform>(npc).unwrap().translation.z > before.z,
                    "landed Jump must resume the production ground mover at {fps} FPS"
                );
            }
        }
    }
}
