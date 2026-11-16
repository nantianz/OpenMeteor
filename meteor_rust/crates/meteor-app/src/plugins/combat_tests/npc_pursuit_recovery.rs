#[test]
fn production_npc_failed_pursuit_plays_pose32_then_reengages_on_shore() {
    let (graph, mesh) = crate::plugins::npc_navigation::original_stage_for_test("sn13");
    let (from, river) = {
        let mut mover = meteor_physics::collision::CollisionSystem::new(
            crate::plugins::movement_plugin::COLLIDER_RADIUS,
            crate::plugins::movement_plugin::COLLIDER_HEIGHT,
        );
        mover.set_mesh_collider(&mesh);
        let wp = Vec3::from_array(graph.world_position(91).unwrap());
        let hit = mover.sample_ground_hit(wp.x, wp.z, wp.y).unwrap();
        let from = Vec3::new(wp.x, hit.y, wp.z);

        let angle = 31.0 * std::f32::consts::TAU / 32.0;
        let at = from + Vec3::new(angle.sin(), 0.0, angle.cos()) * 6.0;
        let hit = mover.sample_ground_hit(at.x, at.z, from.y + 1.0).unwrap();
        (from, Vec3::new(at.x, hit.y, at.z))
    };
    let mut nav = crate::plugins::npc_navigation::Navigation::default();
    assert!(
        !nav.request_path(
            Some(&graph),
            Some(&mesh),
            from,
            river + Vec3::Y * (30.0 * meteor_core::MODEL_SCALE),
            || 5
        ),
        "fixed river support sample must fail: from={from:?} river={river:?} nav={nav:?}"
    );
    for fps in [30, 60, 120] {
        let (mut h, player) = spawn_npc_combat_harness_with_order(6, [100.0; 3], true);
        crate::plugins::character_plugin::spawn_fighter_model_for_test(
            h.app.world_mut(),
            h.fighter,
        );
        let schedule = crate::plugins::fighter_frame::schedule(&h.app);
        h.app.add_systems(
            schedule,
            crate::plugins::character_plugin::drive_fighter_animation_for_test
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        );
        h.app.insert_resource(graph.clone());
        h.app.insert_resource(StageCollider {
            mesh: Some(mesh.clone()),
        });
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = from;
        h.app
            .world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = river;
        h.app
            .world_mut()
            .get_mut::<MotionState>(h.fighter)
            .unwrap()
            .facing = (river.x - from.x).atan2(river.z - from.z);
        h.app
            .world_mut()
            .get_mut::<FighterAir>(h.fighter)
            .unwrap()
            .ground_y = from.y;
        h.app
            .world_mut()
            .get_mut::<AiIntent>(h.fighter)
            .unwrap()
            .enemy_id = Some(0);
        {
            let mut fighter = h.app.world_mut().get_mut::<Fighter>(h.fighter).unwrap();
            fighter.weapon_slots = [6, 0];
        }
        {
            let mut profile = h
                .app
                .world_mut()
                .get_mut::<NpcCombatProfile>(h.fighter)
                .unwrap();
            profile.look = 0.0;
            profile.get_item = 0.0;
            profile.guard = 0.0;
            profile.burst = 0.0;
        }
        let mut taunted = false;
        let mut completed = false;
        for _ in 0..fps * 10 {
            h.step_dt(1.0 / fps as f32);
            taunted |= h.pose_id() == 32;
            completed |= taunted && h.state() == FighterState::Name;
            assert!(
                !h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .air
                    .airborne,
                "unreachable pursuit requests a taunt, not a jump"
            );
            let intent = h.app.world().get::<AiIntent>(h.fighter).unwrap();
            assert_eq!(
                intent.move_dir,
                Vec2::ZERO,
                "same-waypoint failure must not request walking across the ledge"
            );
            assert!(intent.navigation_jump_target.is_none());
        }
        h.trace("failed-pursuit-end");
        eprintln!(
            "fps={fps} taunted={taunted} completed={completed} intent={:?}",
            h.app.world().get::<AiIntent>(h.fighter)
        );
        assert!(
            taunted && completed,
            "Pose32 must enter and finish through production combat at {fps} FPS"
        );

        let at = from + Vec3::NEG_X * 3.0;
        let bank = {
            let mut mover = meteor_physics::collision::CollisionSystem::new(
                crate::plugins::movement_plugin::COLLIDER_RADIUS,
                crate::plugins::movement_plugin::COLLIDER_HEIGHT,
            );
            mover.set_mesh_collider(&mesh);
            let hit = mover.sample_ground_hit(at.x, at.z, at.y).unwrap();
            Vec3::new(at.x, hit.y, at.z)
        };
        h.app
            .world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = bank;
        let mut attacked = false;
        for _ in 0..fps * 8 {
            h.step_dt(1.0 / fps as f32);
            attacked |= matches!(
                h.state(),
                FighterState::Attack1 | FighterState::Attack2 | FighterState::Attack3
            ) && h.pose_id() != 32;
        }
        assert!(
            attacked,
            "opponent returning to the bank must resume real attacks at {fps} FPS"
        );
    }
}
