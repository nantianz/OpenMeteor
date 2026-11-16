use crate::runtime_sync::{ProjectileState, RuntimePacket};

fn runtime_clients() -> (
    StockGameServer,
    StockGameClient,
    StockGameClient,
    StockGameClient,
) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let transport =
        StockServerTransport::bind("127.0.0.1:0", "serverlist", "runtime sync").unwrap();
    let address = transport.local_addr().unwrap();
    let checksums = [1234; STOCK_HERO_COUNT];
    let resource = meteor_asset::res::parse_meteor_res(&fs::read(root.join("Meteor.res")).unwrap())
        .unwrap()
        .combat_table();
    let mut server = StockGameServer::with_rule_and_combat_resources(
        transport,
        StockHeroCatalog::from_checksums(checksums),
        3,
        resource,
    );
    server.set_stage_id("sn04");
    let client = |enabled| {
        let mut client = StockGameClient::new(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp: 1,
                hero_checksum: checksums[0],
            },
            stock_base_weapon_item_id(1).unwrap(),
        );
        client.set_runtime_sync_enabled(enabled);
        client
    };
    let host = client(true);
    let guest = client(true);
    let legacy = client(false);

    let owner = SocketAddr::from(([0, 0, 0, 0], host.transport().local_addr().unwrap().port()));
    server.enable_runtime_sync(
        owner,
        std::sync::Arc::new(
            crate::combat_authority::AttackCatalog::from_origin_res(&root).unwrap(),
        ),
    );
    (server, host, guest, legacy)
}
#[test]
fn runtime_udp_negotiates_routes_scene_hits_once_and_preserves_legacy_fallback() {
    let (mut server, mut host, mut guest, mut legacy) = runtime_clients();
    drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy],
        0,
        150,
    );
    let generation = guest.runtime_generation().unwrap();
    assert_eq!(host.runtime_generation(), Some(generation));
    assert_eq!(legacy.runtime_generation(), None);
    let hit = RuntimePacket::SceneHit {
        generation,
        serial: 1,
        actor_id: 999,
        reference: "D_BPdoor".into(),
        pose_id: 262,
        frame: 9770.0,
        damage: 999999,
    };
    guest.send_runtime(hit.clone(), 200).unwrap();
    guest.send_runtime(hit, 201).unwrap();
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy],
        200,
        100,
    );
    let hits = received[0]
        .iter()
        .filter_map(|event| match event {
            StockGameEvent::Runtime(RuntimePacket::SceneHit {
                actor_id, damage, ..
            }) => Some((*actor_id, *damage)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, guest.actor_id().unwrap());
    assert!(hits[0].1 > 0 && hits[0].1 != 999999);
    assert!(!received[1..]
        .iter()
        .flatten()
        .any(|event| matches!(event, StockGameEvent::Runtime(_))));
    guest
        .send_runtime(
            RuntimePacket::SceneHit {
                generation: generation.wrapping_sub(1),
                serial: 2,
                actor_id: 0,
                reference: "D_BPdoor".into(),
                pose_id: 262,
                frame: 9770.0,
                damage: 0,
            },
            300,
        )
        .unwrap();
    guest
        .send_runtime(
            RuntimePacket::SceneHit {
                generation,
                serial: 3,
                actor_id: 0,
                reference: "D_BPdoor".into(),
                pose_id: 262,
                frame: 1.0,
                damage: 0,
            },
            301,
        )
        .unwrap();
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy],
        300,
        100,
    );
    assert!(!received.iter().flatten().any(|event| matches!(
        event,
        StockGameEvent::Runtime(RuntimePacket::SceneHit { .. })
    )));
    let event = StockSceneOpsEvent {
        timestamp_ms: 400,
        operations: vec![StockSceneOperation {
            reference: 42,
            kind: crate::stock_protocol::STOCK_SCENE_OP_POSE,
            value: 1,
            extra: 0,
        }],
    };
    guest.send_scene_ops(&event, 400).unwrap();
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy],
        400,
        100,
    );
    assert!(
        !received
            .iter()
            .flatten()
            .any(|event| matches!(event, StockGameEvent::SceneOps(_))),
        "negotiated prediction cannot replace the owner's PST state"
    );
    legacy.send_scene_ops(&event, 500).unwrap();
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy],
        500,
        100,
    );
    assert!(received[0]
        .iter()
        .any(|event| matches!(event, StockGameEvent::SceneOps(_))));
}
#[test]
fn runtime_udp_projectiles_keep_actual_trajectory_late_join_and_terminal_state() {
    let (mut server, mut host, mut guest, mut legacy) = runtime_clients();
    drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy],
        0,
        150,
    );
    let generation = guest.runtime_generation().unwrap();
    let mut state = ProjectileState {
        player_id: "spoofed".into(),
        id: 1,
        sequence: 1,
        kind: 1,
        weapon: 1,
        variant: 0,
        pose_id: 200,
        pose_frame: 3519.0,
        position: [2.0, 4.0, 6.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        velocity: [4.0, 5.0, 6.0],
        age: 0.05,
        active: true,
    };
    guest
        .send_runtime(
            RuntimePacket::Projectile {
                generation,
                state: state.clone(),
            },
            200,
        )
        .unwrap();
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy],
        200,
        100,
    );
    let actual = received[0]
        .iter()
        .find_map(|event| match event {
            StockGameEvent::Runtime(RuntimePacket::Projectile { state, .. }) => Some(state),
            _ => None,
        })
        .unwrap();
    assert_eq!(actual.position, state.position);
    assert_eq!(actual.velocity, state.velocity);
    assert_eq!(
        actual.player_id,
        format!("stock-{}", guest.actor_id().unwrap())
    );
    assert!(!received[2]
        .iter()
        .any(|event| matches!(event, StockGameEvent::Runtime(_))));
    let transport = StockClientTransport::bind(
        "127.0.0.1:0",
        server.transport.local_addr().unwrap(),
        StockConnectRecord::for_open_room(),
    )
    .unwrap();
    let mut late = StockGameClient::new(
        transport,
        StockEnterRoleRequest {
            hero: 0,
            camp: 2,
            hero_checksum: 1234,
        },
        stock_base_weapon_item_id(6).unwrap(),
    );
    late.set_runtime_sync_enabled(true);
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy, &mut late],
        300,
        100,
    );
    assert!(received[3].iter().any(|event| matches!(event, StockGameEvent::Runtime(RuntimePacket::Projectile { state, .. }) if state.active && state.position == [2.0, 4.0, 6.0])));
    state.sequence = 2;
    state.active = false;
    guest
        .send_runtime(
            RuntimePacket::Projectile {
                generation,
                state: state.clone(),
            },
            400,
        )
        .unwrap();
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy, &mut late],
        400,
        100,
    );
    assert!(received[3].iter().any(|event| matches!(event, StockGameEvent::Runtime(RuntimePacket::Projectile { state, .. }) if !state.active)));
    state.sequence = 3;
    state.active = true;
    guest
        .send_runtime(RuntimePacket::Projectile { generation, state }, 500)
        .unwrap();
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy, &mut late],
        500,
        100,
    );
    assert!(!received.iter().flatten().any(|event| matches!(event, StockGameEvent::Runtime(RuntimePacket::Projectile { state, .. }) if state.active)));
    assert_eq!(server.runtime.projectiles.active().count(), 0);
    server.runtime_reset(600).unwrap();
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut legacy, &mut late],
        600,
        100,
    );
    assert_ne!(guest.runtime_generation(), Some(generation));
    assert!(received[2]
        .iter()
        .all(|event| !matches!(event, StockGameEvent::Runtime(_))));
}
#[test]
fn runtime_udp_guest_can_negotiate_before_the_embedded_scene_owner_connects() {
    let (mut server, mut host, mut guest, _) = runtime_clients();
    drive(&mut server, &mut [&mut guest], 0, 150);
    let generation = guest.runtime_generation().unwrap();
    guest
        .send_runtime(
            RuntimePacket::SceneHit {
                generation,
                serial: 1,
                actor_id: 0,
                reference: "D_BPdoor".into(),
                pose_id: 262,
                frame: 9770.0,
                damage: 0,
            },
            200,
        )
        .unwrap();
    drive(&mut server, &mut [&mut guest], 200, 100);
    assert_eq!(server.runtime.pending_hits.len(), 1);
    let received = drive(&mut server, &mut [&mut host, &mut guest], 300, 150);
    assert!(received[0].iter().any(|event| matches!(
        event,
        StockGameEvent::Runtime(RuntimePacket::SceneHit { .. })
    )));
}
