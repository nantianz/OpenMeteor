#[test]
fn rebalance_ws_stab_uses_real_damage_and_actor_generation() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let catalog = AttackCatalog::from_origin_res(&root).unwrap();
    let mut server = RoomServer::new();
    for id in 1..=2 {
        server.connect(id);
        server.handle(id, hello());
    }
    let mut create = create_room("leader", "sn01");
    if let Message::RoomCreate { rules, .. } = &mut create {
        rules.battle_rules = Some(meteor_combat::battle_rules::BattleRules::bundled());
    }
    server.handle(1, create);
    server.handle(2, join_room("room1"));
    server.handle(1, Message::RoomStart);
    let room = server.rooms.get_mut("room1").unwrap();
    let member = room.members["p1"].clone();
    let attack = catalog.attack_record(0, 282, 0).unwrap();
    for (instance, guarded, immune) in [(1, true, false), (2, false, false), (3, false, true)] {
        room.elapsed = 3.0 + instance as f32;
        room.hit_elapsed = 3.0 + f64::from(instance);
        room.combatants.get_mut("p1").unwrap().balance_pose_sequence = instance;
        let target = room.combatants.get_mut("p2").unwrap();
        target.target_hit_clock.advance_spawn(3.0);
        target.guard = guarded;
        target.anger = 0.0;
        let before = target.health;
        let events = resolve_confirmed_hit(room, &catalog, "p1", &member, "p2", 282, &attack);
        let hit = events
            .iter()
            .find_map(|event| match event {
                Message::CombatHit {
                    damage,
                    reaction,
                    anger,
                    ..
                } => Some((*damage, reaction.as_ref().unwrap(), *anger)),
                _ => None,
            })
            .unwrap();
        if immune {
            assert_eq!(hit.0, 0.0);
            assert_eq!(
                hit.1.rebalance_effect,
                meteor_combat::rebalance_effects::INVALID_HIT
            );
            assert_eq!(hit.2, Some(0.0));
            assert_eq!(room.combatants["p2"].health, before);
        } else {
            assert!(hit.0 > 0.0);
            assert_eq!(hit.1.rebalance_effect, 0);
            assert!(room.combatants["p2"].health < before);
        }
    }
}

#[test]
fn rebalance_ws_fist_group_protects_cross_pose_hit() {
    use meteor_combat::battle_rules::BattleRules;
    use meteor_combat::rebalance_effects::INVALID_HIT;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let catalog = AttackCatalog::from_origin_res(&root).unwrap();
    let rules = BattleRules::from_json(r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":0,"guardChipPoses":[],"poseOverrides":[{"poseId":407,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}},{"poseId":412,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}}]}}"#).unwrap();
    let mut server = RoomServer::new();
    for id in 1..=2 {
        server.connect(id);
        server.handle(id, hello());
    }
    let mut create = create_room("leader", "sn01");
    if let Message::RoomCreate { rules: wire, .. } = &mut create {
        wire.battle_rules = Some(rules);
    }
    server.handle(1, create);
    server.handle(2, join_room("room1"));
    server.handle(1, Message::RoomStart);
    let room = server.rooms.get_mut("room1").unwrap();
    let member = room.members["p1"].clone();
    for (pose, instance, time) in [(407, 1, 3.0), (412, 2, 3.4)] {
        room.elapsed = time;
        room.hit_elapsed = f64::from(time);
        room.combatants.get_mut("p1").unwrap().balance_pose_sequence = instance;
        let target = room.combatants.get_mut("p2").unwrap();
        target.target_hit_clock.advance_spawn(3.0);
        target.guard = false;
        let before = target.health;
        let attack = catalog.attack_record(0, pose, 0).unwrap();
        let events = resolve_confirmed_hit(room, &catalog, "p1", &member, "p2", pose, &attack);
        let effect = events
            .iter()
            .find_map(|event| match event {
                Message::CombatHit {
                    reaction: Some(reaction),
                    ..
                } => Some(reaction.rebalance_effect),
                _ => None,
            })
            .unwrap();
        if pose == 407 {
            assert!(room.combatants["p2"].health < before);
            assert_eq!(effect, 0);
        } else {
            assert_eq!(room.combatants["p2"].health, before);
            assert_eq!(effect, INVALID_HIT);
        }
    }
}
