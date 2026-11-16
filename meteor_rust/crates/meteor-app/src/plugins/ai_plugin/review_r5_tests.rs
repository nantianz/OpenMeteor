fn review_floor() -> StageMeshCollider {
    let mut mesh = StageMeshCollider::default();
    mesh.append_world_triangles(
        &[
            [-60., 0., -60.],
            [-60., 0., 60.],
            [60., 0., 60.],
            [60., 0., -60.],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    mesh
}

#[test]
fn r5_idle_exposes_enemy_to_scripts_without_attacking_or_locking() {
    let (mut app, npc) = peace_ai_app(0, BehaviorMode::Idle);
    app.update();
    let intent = app.world().get::<AiIntent>(npc).unwrap();
    assert_eq!(intent.enemy_id, Some(0));
    assert!(!intent.want_attack);
    assert!(intent.lock_target.is_none());
    assert_eq!(intent.move_dir, Vec2::ZERO);
    assert_eq!(
        app.world().get::<NpcPatrol>(npc).unwrap().think_remaining,
        0.4
    );
}

#[test]
fn r5_perform_caches_enemy_on_its_own_pulse_without_interrupting_script() {
    for peace in [0, 2] {
        let (mut app, npc) = peace_ai_app(peace, BehaviorMode::Wait);
        app.world_mut().get_mut::<MissionNpc>(npc).unwrap().think = 10000.0;
        app.world_mut()
            .get_mut::<NpcPerformance>(npc)
            .unwrap()
            .queue
            .push_back(crate::plugins::stage_plugin::NpcPerformCommand {
                action: "pause".into(),
                args: vec![meteor_scene::pst_runtime::PstValue::Num(10.0)],
            });
        for tick in 1..=4 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(100));
            app.update();
            let intent = app.world().get::<AiIntent>(npc).unwrap();
            assert_eq!(intent.enemy_id, (peace == 0 && tick == 4).then_some(0));
            assert!(!intent.want_attack);
            assert!(intent.lock_target.is_none());
            assert_eq!(intent.move_dir, Vec2::ZERO);
            assert!(!app
                .world()
                .get::<NpcPatrol>(npc)
                .unwrap()
                .perception
                .in_combat());
            assert_eq!(
                app.world().get::<NpcPerformance>(npc).unwrap().queue.len(),
                1
            );
        }
    }
}

#[test]
fn r5_melee_unlocks_when_target_leaves_range_without_losing_enemy() {
    let (mut app, npc) = peace_ai_app(0, BehaviorMode::Wait);
    app.update();
    assert!(app
        .world()
        .get::<AiIntent>(npc)
        .unwrap()
        .lock_target
        .is_some());
    let target = app
        .world()
        .get::<AiIntent>(npc)
        .unwrap()
        .enemy_entity
        .unwrap();
    app.world_mut()
        .get_mut::<Transform>(target)
        .unwrap()
        .translation = Vec3::Z * 20.0;

    app.update();
    let intent = app.world().get::<AiIntent>(npc).unwrap();
    assert_eq!(intent.enemy_id, Some(0));
    assert!(intent.lock_target.is_none());
}

#[test]
fn r5_script_enemy_cache_does_not_override_attackgoal_between_think_ticks() {
    let (mut app, npc) = peace_ai_app(0, BehaviorMode::AttackTarget);
    app.world_mut().get_mut::<AiIntent>(npc).unwrap().enemy_id = Some(0);
    let graph = app.world().resource::<WaypointGraph>().clone();
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
        patrol.think_remaining = 10.0;
        patrol.attack_target_point = Some(Vec3::Z * 20.0);
        assert!(
            !patrol
                .travel_step(Some(&graph), None, Vec3::ZERO, Vec3::Z * 20.0, true)
                .failed
        );
    }
    app.update();
    let intent = app.world().get::<AiIntent>(npc).unwrap();
    assert_eq!(
        intent.enemy_id,
        Some(0),
        "cached script target remains readable"
    );
    assert!(!intent.want_attack);
    assert!(intent.lock_target.is_none());
    assert!(
        intent.move_dir.y > 0.9,
        "the authored goal still owns movement"
    );
}

#[test]
fn r5_burst_probes_landing_in_the_actors_updated_facing() {
    let mut strip = StageMeshCollider::default();
    strip.append_world_triangles(
        &[
            [-1., 0., -60.],
            [-1., 0., 60.],
            [1., 0., 60.],
            [1., 0., -60.],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    for seed in 1..33 {
        let (mut app, npc) = peace_ai_app(0, BehaviorMode::Wait);
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<LocalFighter>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = Vec3::X * 2.0;
        app.world_mut()
            .get_mut::<NpcCombatProfile>(npc)
            .unwrap()
            .burst = 100.0;
        app.world_mut().get_mut::<NpcPatrol>(npc).unwrap().rng = seed;
        app.insert_resource(StageCollider {
            mesh: Some(strip.clone()),
        });
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(100));
        app.update();
        let facing = app.world().get::<MotionState>(npc).unwrap().facing;
        assert!(facing > 40.0_f32.to_radians());
        assert!(
            app.world()
                .get::<AiIntent>(npc)
                .unwrap()
                .dodge_burst
                .is_none(),
            "all 70-unit landings after turning are outside the narrow strip; seed={seed}"
        );
    }
}

#[test]
fn r5_attackgoal_uses_command_memory_and_exact_point_aim_without_spread() {
    let mut reused = 0;
    for seed in 1..33 {
        let (mut app, npc) = peace_ai_app(0, BehaviorMode::AttackTarget);
        app.world_mut().get_mut::<Fighter>(npc).unwrap().weapon = 3;
        app.world_mut()
            .get_mut::<NpcCombatProfile>(npc)
            .unwrap()
            .aim = 0.0;
        app.world_mut()
            .get_mut::<MissionNpc>(npc)
            .unwrap()
            .attack_target_remaining = 1;
        let target = Vec3::new(0.0, 1.0, 5.0);
        {
            let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
            patrol.attack_target_point = Some(target);
            patrol.rng = seed;
            patrol
                .command_memory
                .choose(meteor_ai::combat::weapon_tactics(3), 2, false, 1);
        }
        app.update();
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert!(intent.want_attack);
        assert_eq!(
            app.world()
                .get::<MissionNpc>(npc)
                .unwrap()
                .attack_target_remaining,
            0
        );
        let expected = Vec3::new(0.0, 1.0 - 30.0 * meteor_core::MODEL_SCALE, 5.0).normalize();
        assert!(intent.ranged_aim.unwrap().distance(expected) < 1e-5);
        reused += usize::from(intent.attack_directions == meteor_ai::combat::directions("bff"));
    }
    assert!(
        reused >= 8,
        "shared entry must actually reuse the saved command: {reused}"
    );
}

#[test]
fn r5_direction_probe_requires_landing_support_but_not_a_walkable_corridor() {
    let mut floor = review_floor();
    floor.append_world_triangles(
        &[[-10., 0., 2.], [10., 0., 2.], [10., 3., 2.], [-10., 3., 2.]],
        &[0, 1, 2, 0, 2, 3],
    );
    assert!(super::super::npc_navigation::direct_combat_direction(
        &floor,
        Vec3::ZERO,
        Vec3::Z * (70.0 * meteor_core::MODEL_SCALE)
    )
    .is_none());
    let space = DecisionSpace {
        position: Vec3::ZERO,
        facing: 0.0,
        ground_controllable: true,
        mesh: Some(&floor),
    };
    assert!(space.supported(Vec2::Y, 70.0));
    let empty = StageMeshCollider::default();
    assert!(!DecisionSpace {
        mesh: Some(&empty),
        ..space
    }
    .supported(Vec2::Y, 70.0));
    assert!(!DecisionSpace {
        ground_controllable: false,
        ..space
    }
    .supported(Vec2::Y, 70.0));
}

#[test]
fn r5_production_jump_attribute_requests_both_impulses_and_rejects_ledge() {
    let mut strengths = std::collections::HashSet::new();
    for seed in 1..33 {
        let (mut app, npc) = peace_ai_app(0, BehaviorMode::Wait);
        app.insert_resource(StageCollider {
            mesh: Some(review_floor()),
        });
        app.world_mut()
            .get_mut::<NpcCombatProfile>(npc)
            .unwrap()
            .jump = 400.0;
        app.world_mut().get_mut::<NpcPatrol>(npc).unwrap().rng = seed;
        app.update();
        strengths.insert(
            app.world()
                .get::<NpcPerformance>(npc)
                .unwrap()
                .ai_jump
                .unwrap(),
        );
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert_eq!(intent.locomotion_input, Vec2::Y);
        assert!(!intent.want_attack);
        assert!((0.1..=0.55).contains(&intent.attack_cooldown));
    }
    assert_eq!(strengths.len(), 2);
    let (mut app, npc) = peace_ai_app(0, BehaviorMode::Wait);
    app.insert_resource(StageCollider {
        mesh: Some(StageMeshCollider::default()),
    });
    app.world_mut()
        .get_mut::<NpcCombatProfile>(npc)
        .unwrap()
        .jump = 400.0;
    app.update();
    assert!(app
        .world()
        .get::<NpcPerformance>(npc)
        .unwrap()
        .ai_jump
        .is_none());
}

#[test]
fn r5_guard_can_crouch_and_burst_without_support_falls_through_to_attack() {
    let mut crouches = 0;
    for seed in 1..101 {
        let (mut app, npc) = peace_ai_app(0, BehaviorMode::Wait);
        app.world_mut().get_mut::<NpcPatrol>(npc).unwrap().rng = seed;
        app.world_mut()
            .get_mut::<NpcCombatProfile>(npc)
            .unwrap()
            .guard = 100.0;
        app.update();
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        let performance = app.world().get::<NpcPerformance>(npc).unwrap();
        assert_ne!(intent.want_guard, performance.crouching);
        assert_eq!(intent.attack_cooldown, 0.4);
        crouches += usize::from(performance.crouching);
    }
    assert!((8..=32).contains(&crouches), "crouch count {crouches}");
    let (mut app, npc) = peace_ai_app(0, BehaviorMode::Wait);
    app.insert_resource(StageCollider {
        mesh: Some(StageMeshCollider::default()),
    });
    app.world_mut()
        .get_mut::<NpcCombatProfile>(npc)
        .unwrap()
        .burst = 100.0;
    app.update();
    let intent = app.world().get::<AiIntent>(npc).unwrap();
    assert!(intent.dodge_burst.is_none());
    assert!(intent.want_attack);
}

#[test]
fn r5_follow_kill_and_combat_use_their_own_item_scan_radius() {
    for (mode, enemy_z, item_x, expected) in [
        (BehaviorMode::Follow, 200.0, 35.0, false),
        (BehaviorMode::Follow, 200.0, 15.0, true),
        (BehaviorMode::Kill, 200.0, 35.0, true),
        (BehaviorMode::Wait, 2.0, 35.0, false),
        (BehaviorMode::Wait, 2.0, 15.0, true),
    ] {
        let (mut app, npc) = peace_ai_app(0, mode);
        let player = app
            .world_mut()
            .query_filtered::<Entity, With<LocalFighter>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = Vec3::Z * enemy_z;
        app.world_mut().get_mut::<MissionNpc>(npc).unwrap().view = 1000.0;
        app.world_mut()
            .get_mut::<NpcCombatProfile>(npc)
            .unwrap()
            .get_item = 100.0;
        app.world_mut()
            .get_mut::<NpcPatrol>(npc)
            .unwrap()
            .getitem_scan_ticks = 3;
        let cargo = app
            .world_mut()
            .spawn((
                LocalEscortState::default(),
                Transform::from_xyz(item_x, 0.0, 0.0),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<NpcPatrol>(npc).unwrap().getitem_entity,
            expected.then_some(cargo),
            "{mode:?} enemy={enemy_z} item={item_x}"
        );
        if expected {
            let intent = app.world().get::<AiIntent>(npc).unwrap();
            assert!(!intent.want_attack);
            assert!(intent.lock_target.is_none());
            app.world_mut().despawn(cargo);
            app.world_mut()
                .get_mut::<NpcPatrol>(npc)
                .unwrap()
                .think_remaining = 0.0;
            app.update();
            assert!(app
                .world()
                .get::<NpcPatrol>(npc)
                .unwrap()
                .getitem_entity
                .is_none());
            assert_eq!(
                app.world().get::<MissionNpc>(npc).unwrap().forced_behavior,
                Some(mode)
            );
        }
    }
}

#[test]
fn r5_guard_is_available_before_attack_facing_gate() {
    let (mut app, npc) = peace_ai_app(0, BehaviorMode::Wait);
    let player = app
        .world_mut()
        .query_filtered::<Entity, With<LocalFighter>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .get_mut::<Transform>(player)
        .unwrap()
        .translation = -Vec3::Z * 2.0;
    app.world_mut()
        .get_mut::<NpcCombatProfile>(npc)
        .unwrap()
        .guard = 100.0;
    app.update();
    let intent = app.world().get::<AiIntent>(npc).unwrap();
    assert!(intent.want_guard || app.world().get::<NpcPerformance>(npc).unwrap().crouching);
    assert!(!intent.want_attack);
}

#[test]
fn r5_damage_reaction_runs_in_parent_callback_and_not_combat() {
    let (mut app, npc) = peace_ai_app(0, BehaviorMode::Wait);
    app.world_mut()
        .entity_mut(npc)
        .insert(crate::components::fighter::Qi {
            current: 100.0,
            max: 100.0,
        });
    app.update();
    assert!(app
        .world()
        .get::<NpcPatrol>(npc)
        .unwrap()
        .perception
        .in_combat());
    app.world_mut().get_mut::<Health>(npc).unwrap().current -= 1.0;
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .set_decision_for_test(0.0, 11);
    app.update();
    assert!(!app.world().get::<AiIntent>(npc).unwrap().want_qi_burst);
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
        patrol.return_combat_to_parent_for_test();
        patrol.set_decision_for_test(0.0, 11);
    }
    app.update();
    assert!(
        app.world().get::<AiIntent>(npc).unwrap().want_qi_burst,
        "damage snapshot must survive combat until its parent can react"
    );
}
