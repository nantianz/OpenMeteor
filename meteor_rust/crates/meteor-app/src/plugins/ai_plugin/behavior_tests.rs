#[test]
fn stock_bot_decisions_match_all_original_weapon_templates() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
    let profile = stock_npc_profile();
    for name in [
        "PUNCH", "Sword", "Lance", "Hammer", "Dagger", "Blade", "Stinger", "KATANA", "SAMURAI",
    ] {
        let bytes = std::fs::read(root.join(format!("{name}.pst"))).unwrap();
        let definition = meteor_scene::mission_runtime::parse_npc_definition(
            &crate::text_codec::decode_text(&bytes),
            name,
        );
        assert_eq!(
            (definition.view as f32, definition.think as f32),
            (STOCK_NPC_VIEW, STOCK_NPC_THINK),
            "{name}"
        );
        assert_eq!(
            (
                definition.attack1 as f32,
                definition.attack2 as f32,
                definition.attack3 as f32,
                definition.guard as f32,
                definition.dodge as f32,
                definition.jump as f32,
                definition.look as f32,
                definition.burst as f32,
                definition.aim as f32,
                definition.get_item as f32
            ),
            (
                profile.attack1,
                profile.attack2,
                profile.attack3,
                profile.guard,
                profile.dodge,
                profile.jump,
                profile.look,
                profile.burst,
                profile.aim,
                profile.get_item
            ),
            "{name}",
        );
    }
}

#[test]
fn original_dodge_chooses_first_concealed_waypoint_not_farthest_point() {
    let graph = parse_waypoint_graph(
        "WayPoints 3\nPos 0 0 0\nSize 1\nLink 1\n1 0 83\nPos 0 -83 0\nSize 1\nLink 2\n0 0 83\n2 0 83\nPos 0 -166 0\nSize 1\nLink 1\n1 0 83\n",
        [0.0; 3],
    );
    let wall = perception_wall(10.0, 5.0);
    assert_eq!(
        concealed_escape_waypoint(&graph, &wall, Vec3::ZERO, Vec3::ZERO, || 1),
        Some(1)
    );
    assert_eq!(
        concealed_escape_waypoint(&graph, &wall, Vec3::ZERO, Vec3::ZERO, || 0),
        None,
        "the original rand%4 gate can skip every occlusion check"
    );
    assert_eq!(
        concealed_escape_waypoint(
            &graph,
            &StageMeshCollider::default(),
            Vec3::ZERO,
            Vec3::ZERO,
            || 1
        ),
        None
    );
}

#[test]
fn production_room_stock_brain_uses_original_run_and_long_view() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 100.0, None);
    app.world_mut()
        .entity_mut(entity)
        .remove::<MissionNpc>()
        .insert((StockNpcBrain, stock_npc_profile()));
    app.update();
    assert_eq!(
        app.world().get::<AiIntent>(entity).unwrap().enemy_id,
        Some(0)
    );
    assert_eq!(
        app.world().get::<NpcPatrol>(entity).unwrap().combat_spacing,
        Some(BehaviorMode::Attack)
    );
    assert!(
        app.world()
            .get::<NpcPatrol>(entity)
            .unwrap()
            .think_remaining
            < 0.01
    );
}

#[test]
fn production_stock_npc_keeps_remote_guest_as_enemy_after_guest_moves_behind() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Wait, -Vec3::Z * 100.0, None);
    let mut net = crate::plugins::network_plugin::NetState::default();
    net.status = crate::plugins::network_plugin::NetStatus::InRoom;
    net.mode = "leader".into();
    app.insert_resource(net);
    app.world_mut()
        .entity_mut(entity)
        .remove::<MissionNpc>()
        .insert((StockNpcBrain, stock_npc_profile()));
    let guest = app
        .world_mut()
        .spawn((
            RemoteFighter {
                player_id: "guest".into(),
            },
            Health::default(),
            Transform::from_translation(Vec3::Z * 20.0),
        ))
        .id();
    app.update();
    let guest_id = -1 - i64::from(guest.index_u32());
    assert_eq!(
        app.world().get::<AiIntent>(entity).unwrap().enemy_id,
        Some(guest_id)
    );

    app.world_mut()
        .get_mut::<Transform>(guest)
        .unwrap()
        .translation = -Vec3::Z * 20.0;
    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .think_remaining = 0.0;
    app.update();
    assert_eq!(
        app.world().get::<AiIntent>(entity).unwrap().enemy_id,
        Some(guest_id)
    );
}

#[test]
fn npc_temporary_behavior_restores_nested_parent_and_authored_route() {
    let (app, entity) = perception_test_app(BehaviorMode::Patrol, Vec3::Z * 5.0, None);
    let mut npc = app.world().get::<MissionNpc>(entity).unwrap().clone();
    let mut intent = AiIntent {
        enemy_id: Some(0),
        ..default()
    };
    let mut patrol = NpcPatrol {
        route: vec![3, 5, 9],
        index: 2,
        ..default()
    };
    let original_args = npc.behavior_args.clone();
    patrol.prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::Kill));
    npc.forced_behavior = Some(BehaviorMode::Kill);
    npc.behavior_args.clear();
    patrol.prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::Dodge));
    npc.forced_behavior = Some(BehaviorMode::Dodge);
    patrol.travel_route = vec![99];
    assert!(patrol.return_to_parent(Some(&mut npc), &mut intent));
    assert_eq!(npc.forced_behavior, Some(BehaviorMode::Kill));
    assert!(patrol.travel_route.is_empty());
    assert!(patrol.return_to_parent(Some(&mut npc), &mut intent));
    assert_eq!(npc.forced_behavior, Some(BehaviorMode::Patrol));
    assert_eq!(
        format!("{:?}", npc.behavior_args),
        format!("{original_args:?}")
    );
    assert_eq!((patrol.route.as_slice(), patrol.index), (&[3, 5, 9][..], 2));
    assert_eq!(intent.enemy_id, Some(0));
    assert!(!patrol.return_to_parent(Some(&mut npc), &mut intent));
    patrol.prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::Dodge));
    patrol.prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::Wait));
    assert!(
        patrol.behavior_stack.is_empty(),
        "replacement commands cancel old children"
    );
}

#[test]
fn npc_dodge_returns_on_far_lost_invisible_or_unroutable_target() {
    for (position, wall, dead) in [
        (Vec3::Z * 40.0, None, false),
        (Vec3::Z * 5.0, None, true),
        (Vec3::Z * 20.0, Some(8.0), false),
        (Vec3::Z * 5.0, None, false),
    ] {
        let (mut app, entity) = perception_test_app(BehaviorMode::Wait, position, wall);
        let npc = app.world().get::<MissionNpc>(entity).unwrap().clone();
        let intent = app.world().get::<AiIntent>(entity).unwrap().clone();
        app.world_mut()
            .get_mut::<NpcPatrol>(entity)
            .unwrap()
            .prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::Dodge));
        app.world_mut()
            .get_mut::<MissionNpc>(entity)
            .unwrap()
            .forced_behavior = Some(BehaviorMode::Dodge);
        if dead {
            let target = app
                .world_mut()
                .query_filtered::<Entity, With<LocalFighter>>()
                .single(app.world())
                .unwrap();
            app.world_mut().get_mut::<Health>(target).unwrap().current = 0.0;
        }
        app.update();
        assert_eq!(
            app.world()
                .get::<MissionNpc>(entity)
                .unwrap()
                .forced_behavior,
            Some(BehaviorMode::Wait)
        );
        assert_eq!(
            app.world().get::<AiIntent>(entity).unwrap().move_dir,
            Vec2::ZERO
        );
    }
}

#[test]
fn npc_dodge_return_keeps_original_target_heading_through_search() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 40.0, None);
    let npc = app.world().get::<MissionNpc>(entity).unwrap().clone();
    let intent = app.world().get::<AiIntent>(entity).unwrap().clone();
    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::Dodge));
    app.world_mut()
        .get_mut::<MissionNpc>(entity)
        .unwrap()
        .forced_behavior = Some(BehaviorMode::Dodge);
    app.world_mut()
        .get_mut::<MotionState>(entity)
        .unwrap()
        .facing = std::f32::consts::PI;
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(16));
    app.update();
    assert_eq!(
        app.world()
            .get::<MissionNpc>(entity)
            .unwrap()
            .forced_behavior,
        Some(BehaviorMode::Wait)
    );
    assert_eq!(
        app.world().get::<NpcPatrol>(entity).unwrap().idle_facing,
        Some(0.0)
    );

    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .perception
        .start_search();
    let before = app.world().get::<MotionState>(entity).unwrap().facing;
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(16));
    app.update();
    let after = app.world().get::<MotionState>(entity).unwrap().facing;
    assert!(npc_facing_error(after, 0.0).abs() < npc_facing_error(before, 0.0).abs());
}

#[test]
fn production_npc_reengages_old_enemy_after_search_when_player_catches_up_behind() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 5.0, None);
    let player = app
        .world_mut()
        .query_filtered::<Entity, With<LocalFighter>>()
        .single(app.world())
        .unwrap();
    app.update();
    assert_eq!(
        app.world().get::<AiIntent>(entity).unwrap().enemy_id,
        Some(0)
    );

    app.world_mut()
        .get_mut::<Transform>(player)
        .unwrap()
        .translation = -Vec3::Z * 80.0;
    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .think_remaining = 0.0;
    app.update();
    assert!(app
        .world()
        .get::<NpcPatrol>(entity)
        .unwrap()
        .perception
        .searching());
    assert_eq!(app.world().get::<AiIntent>(entity).unwrap().enemy_id, None);

    app.world_mut()
        .get_mut::<Transform>(player)
        .unwrap()
        .translation = -Vec3::Z * 20.0;
    for _ in 0..=9 {
        app.world_mut()
            .get_mut::<NpcPatrol>(entity)
            .unwrap()
            .think_remaining = 0.0;
        app.update();
        assert_eq!(app.world().get::<AiIntent>(entity).unwrap().enemy_id, None);
    }
    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .think_remaining = 0.0;
    app.update();
    assert_eq!(
        app.world().get::<AiIntent>(entity).unwrap().enemy_id,
        Some(0)
    );
    assert_eq!(
        app.world().get::<NpcPatrol>(entity).unwrap().combat_spacing,
        Some(BehaviorMode::Attack)
    );
}

#[test]
fn npc_damage_dodge_uses_think_tick_and_returns_for_scripted_and_training_npcs() {
    for scripted in [false, true] {
        let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 5.0, None);
        if !scripted {
            app.world_mut().entity_mut(entity).remove::<MissionNpc>();
        }
        app.world_mut().entity_mut(entity).insert(NpcCombatProfile {
            dodge: 100.0,
            ..default()
        });
        app.world_mut()
            .get_mut::<AiIntent>(entity)
            .unwrap()
            .enemy_id = Some(0);
        {
            let mut patrol = app.world_mut().get_mut::<NpcPatrol>(entity).unwrap();
            patrol.sampled_hp = Some(200.0);
            patrol.think_remaining = 0.5;
        }
        app.update();
        assert!(app
            .world()
            .get::<NpcPatrol>(entity)
            .unwrap()
            .behavior_stack
            .is_empty());
        app.world_mut()
            .get_mut::<NpcPatrol>(entity)
            .unwrap()
            .think_remaining = 0.0;
        app.update();
        assert_eq!(
            app.world()
                .get::<NpcPatrol>(entity)
                .unwrap()
                .behavior_stack
                .len(),
            1
        );
        assert!(!app.world().get::<AiIntent>(entity).unwrap().want_qi_burst);
        app.update();
        let patrol = app.world().get::<NpcPatrol>(entity).unwrap();
        assert!(patrol.behavior_stack.is_empty());
        assert!(patrol.temporary_target.is_none());
    }
}

#[test]
fn npc_authored_callback_cadence_uses_mode_and_think() {
    for (mode, delay) in [
        (BehaviorMode::Wait, 0.4),
        (BehaviorMode::Idle, 0.4),
        (BehaviorMode::Patrol, 0.3),
        (BehaviorMode::Run, 0.3),
        (BehaviorMode::Follow, 0.3),
        (BehaviorMode::Kill, 0.3),
        (BehaviorMode::AttackTarget, 0.3),
        (BehaviorMode::Dodge, 0.4),
    ] {
        for think in [50.0, 100.0] {
            let (mut app, entity) = perception_test_app(mode, Vec3::Z * 100.0, None);
            app.world_mut().get_mut::<MissionNpc>(entity).unwrap().think = think;
            app.update();
            let remaining = app
                .world()
                .get::<NpcPatrol>(entity)
                .unwrap()
                .think_remaining;
            assert!(
                (remaining - delay / (think * 0.01)).abs() < 1e-5,
                "{mode:?} Think={think}: {remaining}"
            );
        }
    }
}

#[test]
fn npc_damage_switch_requires_new_damage_and_a_spare_even_without_enemy() {
    let seed = (1..10000)
        .find(|seed| {
            let mut rng = *seed;
            ((next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32) % 5 == 0
        })
        .unwrap();
    for (damaged, spare) in [(true, 5), (true, 0), (false, 5)] {
        let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 100.0, None);
        app.world_mut().get_mut::<MissionNpc>(entity).unwrap().think = 100.0;
        app.world_mut()
            .get_mut::<Fighter>(entity)
            .unwrap()
            .weapon_slots[1] = spare;
        {
            let mut patrol = app.world_mut().get_mut::<NpcPatrol>(entity).unwrap();
            patrol.sampled_hp = Some(if damaged { 101.0 } else { 100.0 });
            patrol.set_decision_for_test(0.0, seed);
        }
        app.update();
        let expected = damaged && spare != 0;
        assert_eq!(
            app.world()
                .get::<AiIntent>(entity)
                .unwrap()
                .want_weapon_switch,
            expected
        );
        if expected {
            assert_eq!(
                app.world()
                    .get::<NpcPatrol>(entity)
                    .unwrap()
                    .think_remaining,
                0.6
            );
        }
    }
}

#[test]
fn npc_dodge_counter_zero_skips_distance_exit_then_later_returns() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 40.0, None);
    app.insert_resource(parse_waypoint_graph(
        "WayPoints 4\nPos 0 0 30\nSize 1\nLink 1\n1 0 100\nPos 100 0 30\nSize 1\nLink 2\n0 0 100\n2 0 83\nPos 100 83 30\nSize 1\nLink 2\n1 0 83\n3 0 100\nPos 0 83 30\nSize 1\nLink 1\n2 0 100\n",
        [0.0; 3]));
    let mut mesh = perception_wall(10.0, -5.0);
    mesh.append_world_triangles(
        &[
            [-100.0, 0.0, -100.0],
            [100.0, 0.0, -100.0],
            [100.0, 0.0, 100.0],
            [-100.0, 0.0, 100.0],
        ],
        &[0, 2, 1, 0, 3, 2],
    );
    app.insert_resource(StageCollider { mesh: Some(mesh) });
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation = Vec3::NEG_X * 6.0;
    let npc = app.world().get::<MissionNpc>(entity).unwrap().clone();
    let intent = app.world().get::<AiIntent>(entity).unwrap().clone();
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(entity).unwrap();
        patrol.prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::Dodge));
        patrol.behavior_ticks = 0;
        patrol.rng = (1..1000)
            .find(|seed| {
                let mut rng = *seed;
                for _ in 0..3 {
                    next_npc_rand(&mut rng);
                }
                ((next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32) % 4 != 0
            })
            .unwrap();
    }
    app.world_mut()
        .get_mut::<MissionNpc>(entity)
        .unwrap()
        .forced_behavior = Some(BehaviorMode::Dodge);
    app.update();
    assert_eq!(
        app.world()
            .get::<MissionNpc>(entity)
            .unwrap()
            .forced_behavior,
        Some(BehaviorMode::Dodge)
    );
    assert_eq!(
        app.world().get::<NpcPatrol>(entity).unwrap().flee_goal,
        Some(3)
    );
    assert_eq!(
        app.world().get::<AiIntent>(entity).unwrap().move_dir,
        Vec2::ZERO,
        "a new route behind the actor first turns in place"
    );
    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .think_remaining = 10.0;
    for _ in 0..4 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(100));
        app.update();
    }
    assert!(
        app.world()
            .get::<AiIntent>(entity)
            .unwrap()
            .move_dir
            .length_squared()
            > 0.0
    );
    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .think_remaining = 0.0;
    app.update();
    assert_eq!(
        app.world()
            .get::<MissionNpc>(entity)
            .unwrap()
            .forced_behavior,
        Some(BehaviorMode::Wait)
    );
}

#[test]
fn npc_dodge_returns_when_real_navigation_cannot_find_supported_route() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 5.0, None);
    app.insert_resource(parse_waypoint_graph(
        "WayPoints 2\nPos 0 0 0\nSize 1\nLink 1\n1 0 600.0\nPos 600 0 0\nSize 1\nLink 1\n0 0 600.0\n", [0.0; 3]));

    app.insert_resource(StageCollider {
        mesh: Some(perception_wall(8.0, 500.0)),
    });
    let npc = app.world().get::<MissionNpc>(entity).unwrap().clone();
    let intent = app.world().get::<AiIntent>(entity).unwrap().clone();
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(entity).unwrap();
        patrol.prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::Dodge));
        patrol.behavior_ticks = 0;
    }
    app.world_mut()
        .get_mut::<MissionNpc>(entity)
        .unwrap()
        .forced_behavior = Some(BehaviorMode::Dodge);
    app.update();
    assert_eq!(
        app.world()
            .get::<MissionNpc>(entity)
            .unwrap()
            .forced_behavior,
        Some(BehaviorMode::Wait)
    );
    assert!(app
        .world()
        .get::<NpcPatrol>(entity)
        .unwrap()
        .behavior_stack
        .is_empty());
}

#[test]
fn npc_attacktarget_budget_returns_parent_and_uses_callback_clock() {
    for count in [0, 3] {
        let (mut app, entity) = perception_test_app(BehaviorMode::Follow, Vec3::Z * 3.0, None);
        let parent = app.world().get::<MissionNpc>(entity).unwrap().clone();
        let intent = app.world().get::<AiIntent>(entity).unwrap().clone();
        {
            let mut patrol = app.world_mut().get_mut::<NpcPatrol>(entity).unwrap();
            patrol.prepare_behavior_change(
                Some(&parent),
                &intent,
                Some(BehaviorMode::AttackTarget),
            );
            patrol.attack_target_point = Some(Vec3::Z * 3.0);
        }
        {
            let mut npc = app.world_mut().get_mut::<MissionNpc>(entity).unwrap();
            npc.forced_behavior = Some(BehaviorMode::AttackTarget);
            npc.attack_target_remaining = count;
            npc.think = 100.0;
        }
        let mut requests = Vec::new();
        for frame in 0..120 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
            app.update();
            let returned = app
                .world()
                .get::<MissionNpc>(entity)
                .unwrap()
                .forced_behavior
                == Some(BehaviorMode::Follow);
            if app.world().get::<AiIntent>(entity).unwrap().want_attack {
                requests.push(frame);
            }
            app.world_mut()
                .get_mut::<AiIntent>(entity)
                .unwrap()
                .want_attack = false;
            if returned {
                break;
            }
        }
        assert_eq!(
            app.world()
                .get::<MissionNpc>(entity)
                .unwrap()
                .forced_behavior,
            Some(BehaviorMode::Follow)
        );
        assert_eq!(requests.len(), count as usize);
        for pair in requests.windows(2) {
            assert!((18..=19).contains(&(pair[1] - pair[0])), "{requests:?}");
        }
    }
}

#[test]
fn npc_getitem_failed_route_returns_to_enemy_acquisition() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 100.0, None);
    app.world_mut().entity_mut(entity).insert(NpcCombatProfile {
        get_item: 100.0,
        ..default()
    });
    app.world_mut().get_mut::<MissionNpc>(entity).unwrap().think = 100.0;
    app.world_mut().spawn((
        LocalEscortState::default(),
        Transform::from_xyz(15.0, 0.0, 0.0),
    ));
    app.insert_resource(StageCollider {
        mesh: Some(perception_wall(8.0, 500.0)),
    });
    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .getitem_scan_ticks = 3;
    app.update();
    assert!(
        app.world()
            .get::<NpcPatrol>(entity)
            .unwrap()
            .getitem_goal
            .is_none(),
        "failed route returns immediately"
    );
    let player = app
        .world_mut()
        .query_filtered::<Entity, With<LocalFighter>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .get_mut::<Transform>(player)
        .unwrap()
        .translation = Vec3::Z * 5.0;
    for _ in 0..30 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        app.update();
    }
    assert_eq!(
        app.world().get::<AiIntent>(entity).unwrap().enemy_id,
        Some(0)
    );
}

#[test]
fn npc_getitem_keeps_selected_object_and_expires_bounded_callback() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 100.0, None);
    app.world_mut().entity_mut(entity).insert(NpcCombatProfile {
        get_item: 100.0,
        ..default()
    });
    app.world_mut().get_mut::<MissionNpc>(entity).unwrap().think = 100.0;
    let cargo = app
        .world_mut()
        .spawn((
            LocalEscortState::default(),
            Transform::from_xyz(15.0, 0.0, 0.0),
        ))
        .id();
    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .getitem_scan_ticks = 3;
    app.update();
    assert_eq!(
        app.world().get::<NpcPatrol>(entity).unwrap().getitem_entity,
        Some(cargo)
    );
    app.world_mut().spawn((
        LocalEscortState::default(),
        Transform::from_xyz(5.0, 0.0, 0.0),
    ));

    app.world_mut()
        .get_mut::<NpcCombatProfile>(entity)
        .unwrap()
        .get_item = -1.0;
    for _ in 0..5 {
        app.world_mut()
            .get_mut::<NpcPatrol>(entity)
            .unwrap()
            .think_remaining = 0.0;
        app.update();
        assert_eq!(
            app.world().get::<NpcPatrol>(entity).unwrap().getitem_entity,
            Some(cargo)
        );
    }
    for _ in 0..10 {
        app.world_mut()
            .get_mut::<NpcPatrol>(entity)
            .unwrap()
            .think_remaining = 0.0;
        app.update();
    }
    assert!(app
        .world()
        .get::<NpcPatrol>(entity)
        .unwrap()
        .getitem_goal
        .is_none());
}

#[test]
fn npc_getitem_selects_scene_prop_and_releases_hidden_target() {
    let (mut app, npc) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 100.0, None);
    app.world_mut().entity_mut(npc).insert(NpcCombatProfile {
        get_item: 100.0,
        ..default()
    });
    app.world_mut().get_mut::<MissionNpc>(npc).unwrap().think = 100.0;
    let prop = app
        .world_mut()
        .spawn((
            StageProp {
                name: "D_it01".into(),
                model: "item01".into(),
                center_offset: Vec3::Y,
                half_extents: Vec3::ONE,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: true,
                collision_enabled: true,
                breakable: false,
                broken: false,
            },
            Transform::from_xyz(15.0, 0.0, 0.0),
            Visibility::Visible,
        ))
        .id();
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .getitem_scan_ticks = 3;
    app.update();
    let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
    assert_eq!(patrol.getitem_entity, Some(prop));
    assert_eq!(
        patrol.getitem_goal,
        Some(Vec3::new(15.0, 1.0 + 20.0 * meteor_core::MODEL_SCALE, 0.0))
    );

    *app.world_mut().get_mut::<Visibility>(prop).unwrap() = Visibility::Hidden;
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    app.update();
    assert!(app
        .world()
        .get::<NpcPatrol>(npc)
        .unwrap()
        .getitem_goal
        .is_none());
}

#[test]
fn npc_patrol_selects_on_callback_and_keeps_authored_pause() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Patrol, Vec3::splat(1000.0), None);
    app.insert_resource(parse_waypoint_graph(
        "WayPoints 2\nPos 0 0 30\nSize 10\nLink 1\n1 0 600.0\nPos 600 0 30\nSize 10\nLink 1\n0 0 600.0\n", [0.0; 3]));
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation = Vec3::NEG_X * 6.0;
    let seed = (1..10000)
        .find(|seed| {
            let mut rng = *seed;
            ((next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32) % 10 == 0
        })
        .unwrap();
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(entity).unwrap();
        patrol.route = vec![1, 0];
        patrol.set_decision_for_test(0.0, seed);
    }
    app.update();
    assert_eq!(
        app.world()
            .get::<NpcPatrol>(entity)
            .unwrap()
            .patrol_destination,
        Some(1)
    );
    assert_eq!(
        app.world().get::<AiIntent>(entity).unwrap().move_dir,
        Vec2::ZERO
    );
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(100));
    app.update();
    assert!(app.world().get::<AiIntent>(entity).unwrap().move_dir.x > 0.0);

    app.world_mut()
        .get_mut::<NpcPatrol>(entity)
        .unwrap()
        .think_remaining = 10.0;
    app.world_mut()
        .get_mut::<MotionState>(entity)
        .unwrap()
        .velocity = Vec3::X * 12.0;
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation = Vec3::ZERO;
    app.update();
    app.update();
    app.world_mut()
        .get_mut::<Transform>(entity)
        .unwrap()
        .translation = Vec3::X * 72.0;
    app.update();
    app.update();
    let patrol = app.world().get::<NpcPatrol>(entity).unwrap();
    assert!(!patrol.patrol_navigation.in_progress());
    let index = patrol.index;
    app.update();
    assert_eq!(app.world().get::<NpcPatrol>(entity).unwrap().index, index);
    assert_eq!(
        app.world().get::<AiIntent>(entity).unwrap().move_dir,
        Vec2::ZERO,
        "arrival must not immediately select another authored target on a non-Think frame"
    );
}

#[test]
fn npc_normal_turn_obeys_recorded_original_speed_bound() {
    for dt in [1.0 / 30.0, 0.0160853751, 1.0 / 120.0] {
        let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::splat(1000.0), None);
        {
            let mut patrol = app.world_mut().get_mut::<NpcPatrol>(entity).unwrap();
            patrol.idle_facing = Some(2.9);
            patrol.think_remaining = 100.0;
        }
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(dt));
        app.update();
        let facing = app.world().get::<MotionState>(entity).unwrap().facing;
        assert!((facing - 500.0_f32.to_radians() * dt).abs() < 1e-5);
    }
}

#[test]
fn npc_attacktarget_unaligned_ranged_callback_tries_spare_without_spending_budget() {
    for (weapon, spare, expected) in [(3, 6, true), (3, 0, false), (6, 3, false)] {
        let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 100.0, None);
        let npc = app.world().get::<MissionNpc>(entity).unwrap().clone();
        let intent = app.world().get::<AiIntent>(entity).unwrap().clone();
        {
            let mut patrol = app.world_mut().get_mut::<NpcPatrol>(entity).unwrap();
            patrol.prepare_behavior_change(Some(&npc), &intent, Some(BehaviorMode::AttackTarget));
            patrol.attack_target_point = Some(Vec3::X * 3.0);
        }
        {
            let mut npc = app.world_mut().get_mut::<MissionNpc>(entity).unwrap();
            npc.forced_behavior = Some(BehaviorMode::AttackTarget);
            npc.attack_target_remaining = 1;
            npc.think = 100.0;
        }
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(entity).unwrap();
            fighter.weapon = weapon;
            fighter.weapon_slots = [weapon, spare];
        }
        app.update();
        assert_eq!(
            app.world()
                .get::<AiIntent>(entity)
                .unwrap()
                .want_weapon_switch,
            expected
        );
        assert_eq!(
            app.world()
                .get::<MissionNpc>(entity)
                .unwrap()
                .attack_target_remaining,
            1
        );
        if expected {
            assert_eq!(
                app.world()
                    .get::<NpcPatrol>(entity)
                    .unwrap()
                    .think_remaining,
                0.6
            );
        }
    }
}

#[test]
fn npc_damage_guard_and_no_enemy_search_use_original_priority() {
    for has_enemy in [true, false] {
        let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 100.0, None);
        app.world_mut().entity_mut(entity).insert(NpcCombatProfile {
            guard: if has_enemy { 100.0 } else { 0.0 },
            dodge: if has_enemy { 0.0 } else { 100.0 },
            ..default()
        });
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(entity).unwrap();
            fighter.weapon = 6;
            fighter.weapon_slots = [6, 0];
        }
        app.world_mut()
            .get_mut::<AiIntent>(entity)
            .unwrap()
            .enemy_id = has_enemy.then_some(0);
        app.world_mut()
            .get_mut::<NpcPatrol>(entity)
            .unwrap()
            .sampled_hp = Some(200.0);
        app.update();
        assert_eq!(
            app.world().get::<AiIntent>(entity).unwrap().want_guard,
            has_enemy
        );
        assert_eq!(
            app.world()
                .get::<NpcPatrol>(entity)
                .unwrap()
                .perception
                .searching(),
            !has_enemy
        );
        if has_enemy {
            assert!(
                app.world()
                    .get::<NpcPerformance>(entity)
                    .unwrap()
                    .guard_remaining
                    > 0.0
            );
        }
    }
}

#[test]
fn npc_airborne_route_end_keeps_held_heading_and_input_until_landing() {
    let (mut app, entity) = perception_test_app(BehaviorMode::Wait, Vec3::splat(1000.0), None);
    app.world_mut()
        .get_mut::<FighterAir>(entity)
        .unwrap()
        .air
        .airborne = true;
    app.world_mut()
        .get_mut::<AiIntent>(entity)
        .unwrap()
        .navigation_jump_target = Some(Vec3::NEG_Z * 2.0);
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(entity).unwrap();
        patrol.airborne_heading = Some(0.5);
        patrol.airborne_input = Vec2::Y;
        patrol.think_remaining = 10.0;
    }
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(16));
    app.update();
    let intent = app.world().get::<AiIntent>(entity).unwrap();
    assert_eq!(intent.locomotion_input, Vec2::Y);
    assert!(intent.navigation_jump_target.is_some());
    let facing = app.world().get::<MotionState>(entity).unwrap().facing;
    assert!(
        (facing - 500.0_f32.to_radians() * 0.016).abs() < 1e-5,
        "must retain the command heading, not turn back towards the passed waypoint"
    );
    app.world_mut()
        .get_mut::<FighterAir>(entity)
        .unwrap()
        .air
        .airborne = false;
    app.update();
    assert!(app
        .world()
        .get::<AiIntent>(entity)
        .unwrap()
        .navigation_jump_target
        .is_none());
    assert!(app
        .world()
        .get::<NpcPatrol>(entity)
        .unwrap()
        .airborne_heading
        .is_none());
}
