#[test]
fn getitem_probability_matches_executed_exe_four_callback_gate_and_zero_weight() {
    let mut world = World::new();
    let entity = world.spawn_empty().id();
    let candidate = GetItemCandidate {
        record: 2,
        entity,
        position: Vec3::X * 6.0,
    };
    let mut patrol = NpcPatrol {
        rng: 0,
        ..default()
    };
    let profile = NpcCombatProfile {
        get_item: 0.0,
        ..default()
    };
    for tick in 0..5 {
        try_start_getitem_seek(&mut patrol, Some(&profile), 1000.0, Vec3::ZERO, [candidate]);
        assert_eq!(patrol.getitem_entity, (tick == 4).then_some(entity));
    }
    patrol.invalidate_navigation();
    assert_eq!(patrol.last_getitem_record, Some(2));
    for _ in 0..4 {
        try_start_getitem_seek(&mut patrol, Some(&profile), 1000.0, Vec3::ZERO, [candidate]);
    }
    assert!(
        patrol.getitem_entity.is_none(),
        "the last scene record stays excluded after return"
    );
}

#[test]
fn production_getitem_uses_authored_record_order_across_material_batches() {
    let (mut app, npc) = perception_test_app(BehaviorMode::Wait, Vec3::splat(1000.0), None);
    app.world_mut().entity_mut(npc).insert(NpcCombatProfile {
        get_item: 100.0,
        ..default()
    });
    let prop = |name: &str| StageProp {
        name: name.into(),
        model: "item01".into(),
        center_offset: Vec3::ZERO,
        half_extents: Vec3::ONE,
        hit_count: 0,
        respawn_timer: 0.0,
        interactive: true,
        collision_enabled: false,
        breakable: false,
        broken: false,
    };
    let near = app
        .world_mut()
        .spawn((
            prop("later"),
            Transform::from_translation(Vec3::X * 4.0),
            Visibility::Visible,
            crate::plugins::item_plugin::SceneRecordOrder(8),
        ))
        .id();
    let first = app
        .world_mut()
        .spawn((
            prop("first"),
            Transform::from_translation(Vec3::X * 10.0),
            Visibility::Visible,
            crate::plugins::item_plugin::SceneRecordOrder(3),
        ))
        .id();
    let batch = app
        .world_mut()
        .spawn((
            prop("first"),
            Transform::from_translation(Vec3::X * 10.0),
            Visibility::Visible,
            crate::plugins::item_plugin::SceneRecordOrder(3),
        ))
        .id();
    let player = app.world().get::<MissionNpc>(npc).unwrap().view;
    assert!(player >= 200.0);
    for tick in 0..5 {
        app.world_mut()
            .get_mut::<NpcPatrol>(npc)
            .unwrap()
            .think_remaining = 0.0;
        app.update();
        assert_eq!(
            app.world()
                .get::<NpcPatrol>(npc)
                .unwrap()
                .getitem_goal
                .is_some(),
            tick == 4
        );
    }
    let selected = app
        .world()
        .get::<NpcPatrol>(npc)
        .unwrap()
        .getitem_entity
        .unwrap();
    assert!(selected == first || selected == batch);
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .invalidate_navigation();
    for _ in 0..4 {
        app.world_mut()
            .get_mut::<NpcPatrol>(npc)
            .unwrap()
            .think_remaining = 0.0;
        app.update();
    }
    assert_eq!(
        app.world().get::<NpcPatrol>(npc).unwrap().getitem_entity,
        Some(near),
        "all batches of the previous scene record must be skipped"
    );
}

#[test]
fn getitem_arrival_matches_executed_exe_strict_boundaries() {
    for target in [Vec3::X * 20.0, Vec3::Z * 20.0, Vec3::Y * 60.0] {
        assert!(!getitem_arrived(
            Vec3::ZERO,
            target * meteor_core::MODEL_SCALE
        ));
        assert!(getitem_arrived(
            Vec3::ZERO,
            target * (0.99 * meteor_core::MODEL_SCALE)
        ));
    }
}

#[test]
fn getitem_direct_destination_does_not_require_a_nearby_waypoint() {
    let graph = parse_waypoint_graph("WayPoints 1\nPos -1000 0 30\nSize 1\nLink 0\n", [0.0; 3]);
    let mut mesh = StageMeshCollider::default();
    mesh.append_world_triangles(
        &[
            [-200.0, 0.0, -200.0],
            [200.0, 0.0, -200.0],
            [200.0, 0.0, 200.0],
            [-200.0, 0.0, 200.0],
        ],
        &[0, 2, 1, 0, 3, 2],
    );
    let target = Vec3::new(15.0, 20.0 * meteor_core::MODEL_SCALE, 0.0);
    assert!(getitem_route::direct_getitem_route(
        Some(&graph),
        Some(&mesh),
        Vec3::ZERO,
        target
    ));
    assert!(!getitem_route::direct_getitem_route(
        None,
        Some(&mesh),
        Vec3::ZERO,
        target
    ));
    let mut patrol = NpcPatrol {
        rng: 0,
        ..default()
    };
    let fighter = Fighter {
        character_id: 0,
        player_slot: 1,
        weapon: 8,
        weapon_slots: [8, 7],
        weapon_slot: 0,
        weapon_slot_variants: [3, 3],
        weapon_variant: 3,
    };
    assert!(
        weapon_replacement::admit_weapon_getitem(
            &mut patrol,
            &fighter,
            (5, 0),
            Vec3::ZERO,
            target,
            Some(&graph),
            Some(&mesh),
            1.0 / 60.0,
            true
        )
        .unwrap()
        .is_some(),
        "EXE direct branch retains the item as destination, so its 50-unit residual is zero"
    );
}

#[test]
fn production_getitem_seeks_network_ground_flag_and_returns_when_taken() {
    let (mut app, npc) = perception_test_app(BehaviorMode::Wait, Vec3::splat(1000.0), None);
    app.world_mut().entity_mut(npc).insert(NpcCombatProfile {
        get_item: 100.0,
        ..default()
    });
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .getitem_scan_ticks = 3;
    app.init_resource::<crate::plugins::network_plugin::NetState>();
    app.world_mut()
        .resource_mut::<crate::plugins::network_plugin::NetState>()
        .escort_flag = Some(meteor_network::protocol::EscortFlagSnapshot {
        position: [10.0, 0.0, 0.0],
        carrier_id: None,
        state: "home".into(),
        return_in: None,
    });
    let flag = app
        .world_mut()
        .spawn((
            crate::plugins::stage_plugin::NetworkEscortFlag,
            Transform::from_xyz(10.0, 0.0, 0.0),
            Visibility::Visible,
            crate::plugins::item_plugin::SceneRecordOrder(3),
        ))
        .id();
    app.update();
    assert_eq!(
        app.world().get::<NpcPatrol>(npc).unwrap().getitem_entity,
        Some(flag)
    );
    app.world_mut()
        .resource_mut::<crate::plugins::network_plugin::NetState>()
        .escort_flag
        .as_mut()
        .unwrap()
        .carrier_id = Some("stock-8".into());
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
fn getitem_scans_active_records_independently_of_render_and_touch_flags() {
    let mut prop = StageProp {
        name: "D_item".into(),
        model: "item01".into(),
        center_offset: Vec3::ZERO,
        half_extents: Vec3::ONE,
        hit_count: 0,
        respawn_timer: 0.0,
        interactive: false,
        collision_enabled: false,
        breakable: false,
        broken: false,
    };
    assert!(npc_seekable_prop_active(
        &prop,
        &Visibility::Hidden,
        Some(true)
    ));
    assert!(!npc_seekable_prop_active(
        &prop,
        &Visibility::Visible,
        Some(false)
    ));
    assert!(
        !npc_seekable_prop_active(&prop, &Visibility::Visible, None),
        "a flying drop has no active scene record"
    );
    prop.respawn_timer = 1.0;
    assert!(!npc_seekable_prop_active(
        &prop,
        &Visibility::Hidden,
        Some(true)
    ));
}
