#[test]
fn production_original_npc_unreachable_pursuit_recovers() {
    for (stage, waypoint, offset) in [("sn13", 91, Vec3::X * 6.0)] {
        let (graph, mesh) = super::super::npc_navigation::original_stage_for_test(stage);
        let (from, original_target) = {
            let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
            mover.set_mesh_collider(&mesh);
            let wp = Vec3::from_array(graph.world_position(waypoint).unwrap());
            let hit = mover.sample_ground_hit(wp.x, wp.z, wp.y).unwrap();
            let from = Vec3::new(wp.x, hit.y, wp.z);
            let at = from + offset;
            let hit = mover.sample_ground_hit(at.x, at.z, at.y).unwrap();
            (from, Vec3::new(at.x, hit.y, at.z))
        };
        let original_aim = original_target + Vec3::Y * (30.0 * meteor_core::MODEL_SCALE);
        let mut original_nav = super::super::npc_navigation::Navigation::default();
        let original_built =
            original_nav.request_path(Some(&graph), Some(&mesh), from, original_aim, || 5);
        println!("river authored stage={stage} start={:?} end={:?} built={original_built} nav={original_nav:?}",
            super::super::npc_navigation::nearest_original(&graph, Some(&mesh), from + Vec3::Y * (30.0 * meteor_core::MODEL_SCALE)),
            super::super::npc_navigation::nearest_original(&graph, Some(&mesh), original_aim));
        assert!(
            original_built,
            "the historical +X sample has an authored detour"
        );
        assert_eq!(
            super::super::npc_navigation::nearest_original(&graph, Some(&mesh), original_aim),
            Some(66)
        );

        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(&mesh);
        let angle = 31.0 * std::f32::consts::TAU / 32.0;
        let at = from + Vec3::new(angle.sin(), 0.0, angle.cos()) * 6.0;
        let target = Vec3::new(
            at.x,
            mover.sample_ground_hit(at.x, at.z, from.y + 1.0).unwrap().y,
            at.z,
        );
        let mut failed_nav = super::super::npc_navigation::Navigation::default();
        assert!(!failed_nav.request_path(
            Some(&graph),
            Some(&mesh),
            from,
            target + Vec3::Y * (30.0 * meteor_core::MODEL_SCALE),
            || 5
        ));
        println!("river unreachable stage={stage} from={from:?} target={target:?}");
        for fps in [30, 60, 120] {
            let (mut app, npc) = perception_test_app(BehaviorMode::Wait, target, None);
            app.insert_resource(graph.clone());
            app.insert_resource(StageCollider {
                mesh: Some(mesh.clone()),
            });
            app.world_mut()
                .get_mut::<Transform>(npc)
                .unwrap()
                .translation = from;
            {
                let mut fighter = app.world_mut().get_mut::<Fighter>(npc).unwrap();
                fighter.weapon = 6;
                fighter.weapon_slots = [6, 0];
            }
            app.world_mut().get_mut::<MissionNpc>(npc).unwrap().think = 100.0;

            app.world_mut().get_mut::<AiIntent>(npc).unwrap().enemy_id = Some(0);
            let mut held = 0;
            let mut moves = 0;
            let mut attacks = 0;
            let mut taunts = 0;
            let mut parents = 0;
            app.world_mut().get_mut::<MotionState>(npc).unwrap().facing =
                yaw_towards(target - from).unwrap();
            for _ in 0..fps * 10 {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
                app.world_mut().get_mut::<AiIntent>(npc).unwrap().want_taunt = false;
                app.update();
                parents += usize::from(
                    app.world()
                        .get::<NpcPatrol>(npc)
                        .unwrap()
                        .perception
                        .parent_returned,
                );
                let intent = app.world().get::<AiIntent>(npc).unwrap();
                held += usize::from(intent.enemy_id == Some(0));
                moves += usize::from(intent.move_dir.length_squared() > 0.001);
                attacks += usize::from(intent.want_attack);
                taunts += usize::from(intent.want_taunt);
            }
            println!("stage={stage} wp={waypoint} fps={fps} seconds=10 from={from:?} target={target:?} held={held} move_frames={moves} attack_frames={attacks} taunts={taunts} parent_frames={parents} spacing={:?}",
                app.world().get::<NpcPatrol>(npc).unwrap().combat_spacing);
            assert_eq!(held, fps * 10);
            assert_eq!(
                (moves, attacks),
                (0, 0),
                "no walk through the river wall or melee out of range"
            );
            assert!(
                taunts > 0 && parents > 0,
                "both original recovery branches must execute at {fps} FPS"
            );
            let player = app
                .world_mut()
                .query_filtered::<Entity, With<LocalFighter>>()
                .single(app.world())
                .unwrap();
            app.world_mut()
                .get_mut::<Transform>(player)
                .unwrap()
                .translation = from + Vec3::Z * 2.0;
            app.world_mut()
                .get_mut::<NpcPatrol>(npc)
                .unwrap()
                .think_remaining = 0.0;
            app.update();
            let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
            assert!(!patrol.perception.parent_returned);
            assert_eq!(
                patrol.combat_spacing,
                Some(BehaviorMode::Attack),
                "return to shore must reengage"
            );
        }
    }
}

#[test]
fn production_sn05_authored_river_exit_moves_through_body_collision() {
    let (graph, mesh) = super::super::npc_navigation::original_stage_for_test("sn05");
    let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    mover.set_mesh_collider(&mesh);
    let wp = Vec3::from_array(graph.world_position(42).unwrap());
    let from = Vec3::new(
        wp.x,
        mover.sample_ground_hit(wp.x, wp.z, wp.y).unwrap().y,
        wp.z,
    );
    let at = from + Vec3::NEG_Z * 6.0;
    let target = Vec3::new(
        at.x,
        mover.sample_ground_hit(at.x, at.z, at.y).unwrap().y,
        at.z,
    );
    let mut nav = super::super::npc_navigation::Navigation::default();
    assert!(nav.request_path(
        Some(&graph),
        Some(&mesh),
        from,
        target + Vec3::Y * (30.0 * meteor_core::MODEL_SCALE),
        || 5
    ));
    assert_eq!(graph.link_types.get(&(90, 91)), Some(&1));
    assert_eq!(
        nav.steering_target(),
        graph.world_position(90).map(Vec3::from_array)
    );
    for fps in [30, 60, 120] {
        let (mut app, npc) = perception_test_app(BehaviorMode::Wait, target, None);
        app.insert_resource(graph.clone())
            .insert_resource(StageCollider {
                mesh: Some(mesh.clone()),
            })
            .add_systems(Update, move_npc_fighters.after(drive_npc_ai));
        app.world_mut().entity_mut(npc).insert((
            FighterDodge::default(),
            SmState::default(),
            CombatState::default(),
            PoseRuntime::default(),
            EntryBlend::default(),
        ));
        app.world_mut()
            .get_mut::<Transform>(npc)
            .unwrap()
            .translation = from;
        app.world_mut().get_mut::<MotionState>(npc).unwrap().facing = std::f32::consts::FRAC_PI_2;
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(npc).unwrap();
            fighter.weapon = 6;
            fighter.weapon_slots = [6, 0];
        }
        app.world_mut().get_mut::<MissionNpc>(npc).unwrap().think = 100.0;
        app.world_mut().get_mut::<AiIntent>(npc).unwrap().enemy_id = Some(0);
        for _ in 0..fps {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
            app.update();
        }
        let pos = app.world().get::<Transform>(npc).unwrap().translation;
        assert!(
            pos.x > from.x + 1.0,
            "authored exit must make accepted eastward progress at {fps}: {from:?}->{pos:?}"
        );
        assert!(
            pos.y > target.y + 5.0,
            "first walk leg must stay on the bank: {pos:?}"
        );
    }
}

fn recovery_seed(branch: u32, accepts_look: bool) -> u64 {
    (1..100_000)
        .find(|seed| {
            let mut rng = *seed;
            let roll = (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32;
            let look = (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32;
            roll % 4 == branch && (look % 100 < 50) == accepts_look
        })
        .unwrap()
}

#[test]
fn failed_pursuit_preserves_parent_stack_and_uses_original_clock_and_action() {
    for branch in 0..4 {
        let (app, entity) = perception_test_app(BehaviorMode::Patrol, Vec3::Z * 20.0, None);
        let npc = app.world().get::<MissionNpc>(entity).unwrap().clone();
        let mut intent = AiIntent {
            enemy_id: Some(0),
            ..default()
        };
        let mut patrol = NpcPatrol {
            route: vec![2, 3],
            index: 1,
            ..default()
        };

        patrol.prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::Kill));
        patrol.rng = recovery_seed(branch, true);
        let yaw = pursuit_recovery::recover(
            &mut patrol,
            &mut intent,
            0.0,
            Vec3::Z * 20.0,
            Vec3::ZERO,
            50.0,
            50.0,
        );
        assert_eq!(intent.enemy_id, Some(0));
        assert_eq!(patrol.behavior_stack.len(), 1);
        assert_eq!(
            (
                patrol.behavior_stack[0].route.as_slice(),
                patrol.behavior_stack[0].index
            ),
            (&[2, 3][..], 1)
        );
        assert!(
            patrol.route.is_empty(),
            "combat return must leave the current Kill callback's route separate"
        );
        assert_eq!(intent.want_taunt, branch == 0);
        assert_eq!(patrol.perception.parent_returned, branch != 0);
        let delay = meteor_ai::combat::decision_interval(50.0, if branch == 0 { 0.8 } else { 0.4 });
        assert!((patrol.think_remaining - delay).abs() < 1e-6);
        if branch != 0 {
            assert_eq!(
                patrol.idle_facing, yaw,
                "Look heading persists through parent delay"
            );
        }
    }
    for (angle, expected) in [(4.99_f32, true), (5.01, false), (90.0, false)] {
        let mut patrol = NpcPatrol {
            rng: recovery_seed(0, false),
            ..default()
        };
        let mut intent = AiIntent::default();
        pursuit_recovery::recover(
            &mut patrol,
            &mut intent,
            angle.to_radians(),
            Vec3::Z,
            Vec3::ZERO,
            0.0,
            100.0,
        );
        assert_eq!(intent.want_taunt, expected);
    }
}
