#[test]
fn stock_npc_weapon_switch_reaches_observer_once_with_variant() {
    let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "NPC switch").unwrap();
    let address = transport.local_addr().unwrap();
    let mut server = StockGameServer::with_rule_and_combat_resources(
        transport,
        StockHeroCatalog::bundled_revision(),
        1,
        stock_combat_resources(),
    );
    let mut observer = StockGameClient::for_bundled_revision(
        StockClientTransport::bind(
            "127.0.0.1:0",
            address,
            meteor_network::stock_udp::StockConnectRecord::for_open_room(),
        )
        .unwrap(),
        0,
        0,
        6,
    )
    .unwrap();
    let npc = World::new().spawn_empty().id();
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut bridge = Bridge::default();
    let mut initial = role(npc);
    initial.weapon = 3;
    bridge.sync(&server, vec![initial], 0).unwrap();
    pump(&mut server, &mut [&mut observer], &mut bridge, &tx, 0, 200);
    let actor = bridge.clients[&npc].actor_id().unwrap();
    let mut switched = role(npc);
    switched.weapon_variant = 2;
    bridge.sync(&server, vec![switched], 201).unwrap();
    let events = pump(
        &mut server,
        &mut [&mut observer],
        &mut bridge,
        &tx,
        201,
        100,
    );
    let expected = stock_weapon_item_id(6, 2).unwrap();
    assert_eq!(
        events[0]
            .iter()
            .filter(|event| matches!(event,
        StockGameEvent::WeaponEquipped(e) if e.actor_id == actor && e.weapon_item_id == expected))
            .count(),
        1
    );
    let mut unchanged = role(npc);
    unchanged.weapon_variant = 2;
    bridge.sync(&server, vec![unchanged], 302).unwrap();
    let events = pump(
        &mut server,
        &mut [&mut observer],
        &mut bridge,
        &tx,
        302,
        100,
    );
    assert!(!events[0].iter().any(|event| matches!(event,
        StockGameEvent::WeaponEquipped(e) if e.actor_id == actor)));
}

#[test]
fn stock_npc_spare_drop_uses_own_actor_once_and_preserves_equipped_weapon() {
    let transport =
        StockServerTransport::bind("127.0.0.1:0", "serverlist", "NPC spare drop").unwrap();
    let address = transport.local_addr().unwrap();
    let mut server = StockGameServer::with_rule_and_combat_resources(
        transport,
        StockHeroCatalog::bundled_revision(),
        1,
        stock_combat_resources(),
    );
    let mut observer = StockGameClient::for_bundled_revision(
        StockClientTransport::bind(
            "127.0.0.1:0",
            address,
            meteor_network::stock_udp::StockConnectRecord::for_open_room(),
        )
        .unwrap(),
        0,
        0,
        6,
    )
    .unwrap();
    let npc = World::new().spawn_empty().id();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut bridge = Bridge::default();
    bridge.sync(&server, vec![role(npc)], 0).unwrap();
    pump(&mut server, &mut [&mut observer], &mut bridge, &tx, 0, 200);
    let actor = bridge.clients[&npc].actor_id().unwrap();
    let current = stock_weapon_item_id(6, 0).unwrap();
    let spare = stock_weapon_item_id(7, 3).unwrap();
    server
        .replace_authored_scene_items(
            vec![StockAuthoredSceneItem {
                repeat_on_xstar: false,
                item_id: spare,
                category: 1,
                position: role(npc).state.position,
                half_extents: [1.0; 3],
                respawn_ms: None,
                scene_reference: Some(0),
                active: true,
                interactive: true,
            }],
            201,
        )
        .unwrap();
    bridge.sync(&server, vec![role(npc)], 201).unwrap();
    let picked = pump(
        &mut server,
        &mut [&mut observer],
        &mut bridge,
        &tx,
        201,
        200,
    );
    assert!(picked[0].iter().any(|event| matches!(event,
        StockGameEvent::SceneItemTaken { event, .. } if event.actor_id == actor && event.item_id == spare)),
        "the spare must first exist in the server's actual NPC inventory");
    let position = RoleVec3 {
        x: 15.0,
        y: 0.0,
        z: -8.0,
    };
    let command = StockWeaponDropCommand {
        item_id: spare,
        position,
        forward: RoleVec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
        copy_only: false,
        remaining_item_id: Some(current),
    };
    let Some(StockGameEvent::WeaponThrown(thrown)) =
        bridge.send_weapon_drop(npc, command, 501).unwrap()
    else {
        panic!("drop must use the NPC's private role");
    };
    assert_eq!(thrown.actor_id, actor);
    assert_eq!(thrown.item_id, spare);
    assert_eq!(thrown.position, position);
    let events = pump(
        &mut server,
        &mut [&mut observer],
        &mut bridge,
        &tx,
        501,
        200,
    );
    assert_eq!(events[0].iter().filter(|event| matches!(event,
        StockGameEvent::RawApplication { opcode: meteor_network::stock_protocol::STOCK_EVENT_WEAPON_OBJECT, payload, .. }
            if payload.len() == 36 && u32::from_le_bytes(payload[..4].try_into().unwrap()) == actor)).count(), 1);
    let mut unchanged = role(npc);
    unchanged.state.position = position;
    bridge.sync(&server, vec![unchanged], 702).unwrap();
    let events = pump(
        &mut server,
        &mut [&mut observer],
        &mut bridge,
        &tx,
        702,
        100,
    );
    assert!(
        !events[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::WeaponEquipped(e) if e.actor_id == actor)),
        "dropping the spare must not switch the hand weapon"
    );
    assert!(bridge
        .confirm_weapon_rest(
            npc,
            thrown.throw_id,
            [position.x + 12.0, position.y, position.z],
            803
        )
        .unwrap());
    assert!(!bridge
        .confirm_weapon_rest(
            npc,
            thrown.throw_id,
            [position.x + 12.0, position.y, position.z],
            804
        )
        .unwrap());
    let events = pump(
        &mut server,
        &mut [&mut observer],
        &mut bridge,
        &tx,
        803,
        200,
    );
    assert_eq!(events[0].iter().filter(|event| matches!(event,
        StockGameEvent::SceneItemsGrounded { records, spawned: true } if records.iter().any(|record| record.item_id == spare))).count(), 1,
        "only the NPC owner confirms the actual landing and creates one pickup");
    assert!(rx.try_iter().any(|message| matches!(message,
        NetInbound::StockNpcWeaponEvent(owner, StockGameEvent::WeaponSettled { throw_id, .. })
            if owner == npc && throw_id == thrown.throw_id)));
    let replacement = stock_weapon_item_id(5, 0).unwrap();
    server
        .replace_authored_scene_items(
            vec![StockAuthoredSceneItem {
                repeat_on_xstar: false,
                item_id: replacement,
                category: 1,
                position,
                half_extents: [1.0; 3],
                respawn_ms: None,
                scene_reference: Some(1),
                active: true,
                interactive: true,
            }],
            1003,
        )
        .unwrap();
    let events = pump(
        &mut server,
        &mut [&mut observer],
        &mut bridge,
        &tx,
        1003,
        200,
    );
    assert_eq!(events[0].iter().filter(|event| matches!(event,
        StockGameEvent::SceneItemTaken { event, .. } if event.actor_id == actor && event.item_id == replacement)).count(), 1,
        "the NPC can pick a different weapon into the slot released by command20");
    let mut missing_world = World::new();
    missing_world.spawn_empty();
    let missing = missing_world.spawn_empty().id();
    assert!(bridge.send_weapon_drop(missing, command, 1003).is_err());
}

#[test]
fn npc_private_throw_namespaces_do_not_overwrite_each_other_or_player() {
    let mut world = World::new();
    let first = world.spawn_empty().id();
    let second = world.spawn_empty().id();
    let mut net = NetState::default();
    let throw = meteor_network::stock_game::StockWeaponThrowEvent {
        throw_id: 1,
        actor_id: 9,
        item_id: stock_weapon_item_id(7, 3).unwrap(),
        position: RoleVec3::ZERO,
        forward: RoleVec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
    };
    for entity in [first, second] {
        super::super::project_npc_weapon_event(
            &mut net,
            entity,
            StockGameEvent::WeaponThrown(throw),
        );
    }
    assert_eq!(net.dynamic_weapon_drops.len(), 2);
    assert!(net
        .dynamic_weapon_drops
        .keys()
        .all(|key| key.starts_with("D_wpStockNpcThrow")));
    super::super::project_npc_weapon_event(
        &mut net,
        first,
        StockGameEvent::WeaponSettled {
            throw_id: 1,
            record_id: 99,
        },
    );
    assert_eq!(net.dynamic_weapon_drops.len(), 1);
    assert_eq!(net.pending_weapon_drops.len(), 1);
    assert!(net
        .dynamic_weapon_drops
        .contains_key(&format!("D_wpStockNpcThrow{}_1", second.to_bits())));
}
