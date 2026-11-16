#[test]
fn rebalance_authority_effects_preserve_armor_and_release_stun() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    if !root.exists() {
        eprintln!("origin_res missing, skipping");
        return;
    }
    let character = crate::plugins::character_plugin::load_character_asset_for_test(
        std::slice::from_ref(&root),
        "p0",
    )
    .expect("p0 assets");
    let mut app = App::new();
    app.insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
        std::collections::HashMap::from([("p0".to_string(), character)]),
    ))
    .insert_resource(NetState {
        round_active: true,
        status: NetStatus::InRoom,
        battle_rules: meteor_combat::battle_rules::BattleRules::bundled(),
        pending_hits: vec![AuthorityHit {
            stock_guard_bucket: None,
            damage: 100.0,
            health: Some(900.0),
            anger: Some(14.0),
            guarded: false,
            attacker_id: None,
            effect_strategy: None,
            motion_scale: Some(1.5),
            reaction: Some(CombatReactionWire {
                rebalance_effect: 0,
                target_move: 1.5,
                target_value: 0.5,
                target_pose: Some(93),
                target_pose_front: Some(93),
                target_pose_back: Some(94),
                target_pose_left: Some(92),
                target_pose_right: Some(91),
                ..CombatReactionWire::default()
            }),
            tick: Some(12),
            impact: true,
            terminal: false,
        }],
        ..NetState::default()
    })
    .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
    .add_systems(Update, net_handle_hit);
    let entity = app
        .world_mut()
        .spawn((
            LocalFighter,
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Transform::default(),
            Health::default(),
            Qi::default(),
            DeathRespawn::default(),
            (
                PoseRuntime::idle(),
                EntryBlend {
                    duration: 0.4,
                    ..EntryBlend::default()
                },
                CombatState {
                    state: meteor_core::FighterState::Attack1,
                },
                SmState {
                    current: meteor_core::FighterState::Attack1,
                    ..SmState::default()
                },
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::new(6.0, 0.0, -2.0),
                    facing: 0.0,
                },
                FighterAir::default(),
                FighterAttackMotion {
                    active: true,
                    ..FighterAttackMotion::default()
                },
            ),
        ))
        .id();

    app.update();
    assert!(app.world().get::<SmState>(entity).unwrap().locked);
    assert!(app
        .world()
        .get::<FighterHitReaction>(entity)
        .unwrap()
        .current
        .is_some());
    let hit = |effect, hp, pose_id| AuthorityHit {
        stock_guard_bucket: None,
        damage: 0.0,
        health: Some(hp),
        anger: Some(0.0),
        guarded: false,
        attacker_id: None,
        effect_strategy: None,
        motion_scale: Some(0.0),
        reaction: Some(CombatReactionWire {
            rebalance_effect: effect,
            target_pose: Some(pose_id),
            ..Default::default()
        }),
        tick: Some(13),
        impact: true,
        terminal: false,
    };
    app.world_mut()
        .resource_mut::<NetState>()
        .pending_hits
        .push(hit(
            meteor_combat::rebalance_effects::INVALID_HIT,
            900.0,
            114,
        ));
    app.update();
    assert_eq!(app.world().get::<Health>(entity).unwrap().current, 900.0);
    assert_eq!(app.world().get::<Qi>(entity).unwrap().current, 0.0);
    assert_eq!(
        app.world().get::<CombatState>(entity).unwrap().state,
        meteor_core::FighterState::Name
    );
    assert!(!app.world().get::<SmState>(entity).unwrap().locked);
    assert!(app
        .world()
        .get::<FighterHitReaction>(entity)
        .unwrap()
        .current
        .is_none());
    assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 0);
    assert!(app
        .world()
        .get::<super::super::rebalance_plugin::BalanceGlow>(entity)
        .unwrap()
        .armor_pose
        .is_none());

    app.world_mut()
        .get_mut::<PoseRuntime>(entity)
        .unwrap()
        .pose_id = 302;
    app.world_mut()
        .get_mut::<CombatState>(entity)
        .unwrap()
        .state = meteor_core::FighterState::Attack1;
    app.world_mut().get_mut::<SmState>(entity).unwrap().current =
        meteor_core::FighterState::Attack1;
    app.world_mut()
        .get_mut::<FighterAttackMotion>(entity)
        .unwrap()
        .active = true;
    app.world_mut()
        .resource_mut::<NetState>()
        .pending_hits
        .push(hit(
            meteor_combat::rebalance_effects::ARMOR_START,
            900.0,
            302,
        ));
    app.update();
    assert_eq!(
        app.world()
            .get::<super::super::rebalance_plugin::BalanceGlow>(entity)
            .unwrap()
            .armor_pose,
        Some(302)
    );
    app.world_mut()
        .resource_mut::<NetState>()
        .pending_hits
        .push(hit(meteor_combat::rebalance_effects::ARMOR_HIT, 800.0, 93));
    app.update();
    assert_eq!(app.world().get::<Health>(entity).unwrap().current, 800.0);
    assert_eq!(app.world().get::<PoseRuntime>(entity).unwrap().pose_id, 302);
    assert_eq!(
        app.world().get::<CombatState>(entity).unwrap().state,
        meteor_core::FighterState::Attack1
    );
    assert!(
        app.world()
            .get::<FighterAttackMotion>(entity)
            .unwrap()
            .active
    );
}
