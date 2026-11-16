#[test]
fn rebalance_udp_bundled_rules_are_transferred_in_full() {
    use meteor_combat::battle_rules::BattleRules;
    let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "final-rules").unwrap();
    let address = transport.local_addr().unwrap();
    let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
    let rules = BattleRules::bundled();
    server.set_battle_rules(rules.clone());
    let mut client = StockGameClient::for_bundled_revision(
        StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
            .unwrap(),
        0,
        0,
        7,
    )
    .unwrap();
    let joined = drive(&mut server, &mut [&mut client], 0, 400);
    assert!(client.actor_id().is_some());
    assert!(joined[0].iter().any(|event| matches!(event,
        StockGameEvent::BattleRules(received) if *received == rules
    )));
}

#[test]
fn rebalance_udp_stab_damage_guard_and_armor_authority() {
    use meteor_combat::rebalance_effects::{ARMOR_START, INVALID_HIT, STOCK_EFFECT_CONTEXT};
    let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "effects").unwrap();
    let address = transport.local_addr().unwrap();
    let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());

    server.set_battle_rules(meteor_combat::battle_rules::BattleRules::from_json(r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":10,"guardChipPoses":"all","poseOverrides":[{"poseId":282,"invalidAttack":{"qiCostPercent":0,"windowMs":1000}},{"poseId":302,"superArmor":{"qiCostPercent":20}}]}}"#).unwrap());
    let connect = || {
        StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            7,
        )
        .unwrap()
    };
    let mut a = connect();
    let mut b = connect();
    drive(&mut server, &mut [&mut a, &mut b], 0, 400);
    let source = a.transport.local_addr().unwrap();
    let target = b.transport.local_addr().unwrap();
    let actor_id = b.actor_id().unwrap();
    let stab = StockHitRequest {
        target_actor_id: actor_id,
        hit_word: 282,
        target_pose_word: 114,
        moves_word: 0,
        values_word: 0,
    };

    let mut guard = sample_state(0.0, 1);
    guard.state_words.guard_route = true;
    server.roles.get_mut(&target).unwrap().state = Some(guard);
    for (instance, now) in [(1, 2500), (2, 3000), (3, 3500)] {
        let mut state = sample_state(0.0, instance);
        state.state_words =
            crate::role_state::unpack_state_words(pack_state_word0(282, true, false, 0), instance);
        a.send_role_state(state, now).unwrap();
        drive(&mut server, &mut [&mut a, &mut b], now, 30);
        let before = server.roles[&target].health;
        a.send_hit(stab, now + 40).unwrap();
        let events = drive(&mut server, &mut [&mut a, &mut b], now + 40, 100);
        if instance < 3 {
            assert!(server.roles[&target].health < before);
            assert!(!events[1]
                .iter()
                .any(|e| matches!(e, StockGameEvent::HitReaction { event, .. }
                if event.context == STOCK_EFFECT_CONTEXT + u32::from(INVALID_HIT))));
        } else {
            assert_eq!(server.roles[&target].health, before);
            assert_eq!(server.roles[&target].anger, 0.0);
            for output in events {
                assert!(output
                    .iter()
                    .any(|e| matches!(e, StockGameEvent::HitReaction { event, .. }
                    if event.context == STOCK_EFFECT_CONTEXT + u32::from(INVALID_HIT))));
            }
        }
        server
            .roles
            .get_mut(&target)
            .unwrap()
            .state
            .as_mut()
            .unwrap()
            .state_words
            .guard_route = false;
        server.roles.get_mut(&target).unwrap().anger = 0.0;
    }

    server.roles.get_mut(&source).unwrap().anger = 20.0;
    let mut state = sample_state(0.0, 4);
    state.state_words =
        crate::role_state::unpack_state_words(pack_state_word0(302, true, false, 0), 4);
    a.send_role_state(state, 4000).unwrap();
    let events = drive(&mut server, &mut [&mut a, &mut b], 4000, 100);
    assert_eq!(server.roles[&source].anger, 0.0);
    assert!(server.roles[&source].defensive_effects.armored());
    for output in events {
        assert!(output
            .iter()
            .any(|e| matches!(e, StockGameEvent::HitReaction { event, .. }
            if event.context == STOCK_EFFECT_CONTEXT + u32::from(ARMOR_START))));
    }
    a.send_role_state(state, 4200).unwrap();
    let events = drive(&mut server, &mut [&mut a, &mut b], 4200, 100);
    assert!(!events[0]
        .iter()
        .any(|e| matches!(e, StockGameEvent::HitReaction { event, .. }
        if event.context == STOCK_EFFECT_CONTEXT + u32::from(ARMOR_START))));
    state.state_words =
        crate::role_state::unpack_state_words(pack_state_word0(302, true, false, 0), 5);
    a.send_role_state(state, 4400).unwrap();
    drive(&mut server, &mut [&mut a, &mut b], 4400, 100);
    assert!(!server.roles[&source].defensive_effects.armored());
}

#[test]
fn rebalance_udp_fist_group_protects_a_to_left_left_a() {
    use meteor_combat::battle_rules::BattleRules;
    use meteor_combat::rebalance_effects::{INVALID_HIT, STOCK_EFFECT_CONTEXT};
    let rules = BattleRules::from_json(r#"{"schemaVersion":2,"rebalance":{"guardChipPercent":0,"guardChipPoses":[],"poseOverrides":[{"poseId":407,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}},{"poseId":412,"invalidAttack":{"groupId":1,"armOnHit":true,"qiCostPercent":0,"windowMs":800}}]}}"#).unwrap();
    let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "fist-group").unwrap();
    let address = transport.local_addr().unwrap();
    let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
    server.set_battle_rules(rules.clone());
    let connect = || {
        StockGameClient::for_bundled_revision(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            0,
            0,
            11,
        )
        .unwrap()
    };
    let mut attacker = connect();
    let mut defender = connect();
    let joined = drive(&mut server, &mut [&mut attacker, &mut defender], 0, 400);
    assert!(joined[1]
        .iter()
        .any(|event| matches!(event, StockGameEvent::BattleRules(received) if *received == rules)));
    let target = defender.transport.local_addr().unwrap();
    let target_actor_id = defender.actor_id().unwrap();
    let request = |pose| StockHitRequest {
        target_actor_id,
        hit_word: u32::from(pose),
        target_pose_word: 114,
        moves_word: 0,
        values_word: 0,
    };
    for (pose, instance, now) in [(407, 1, 2500), (412, 2, 2900)] {
        let mut state = sample_state(0.0, instance);
        state.state_words =
            crate::role_state::unpack_state_words(pack_state_word0(pose, true, false, 0), instance);
        attacker.send_role_state(state, now).unwrap();
        drive(&mut server, &mut [&mut attacker, &mut defender], now, 30);
        let before = server.roles[&target].health;
        attacker.send_hit(request(pose), now + 40).unwrap();
        let events = drive(
            &mut server,
            &mut [&mut attacker, &mut defender],
            now + 40,
            100,
        );
        if pose == 407 {
            assert!(server.roles[&target].health < before);
        } else {
            assert_eq!(server.roles[&target].health, before);
            assert!(events
                .iter()
                .all(|side| side.iter().any(|event| matches!(event,
                    StockGameEvent::HitReaction { event, .. }
                        if event.context == STOCK_EFFECT_CONTEXT + u32::from(INVALID_HIT)
                ))));
        }
    }
}
