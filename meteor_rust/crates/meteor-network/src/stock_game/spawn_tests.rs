#[test]
fn spawn_pad_wire_rejects_partial_payload_and_preserves_empty() {
    use crate::stock_protocol::decode_spawn_pad;
    assert_eq!(decode_spawn_pad(&[]).unwrap(), None);
    for len in [1, 4, 7, 9] {
        assert!(decode_spawn_pad(&vec![0; len]).is_err());
    }
    assert_eq!(
        decode_spawn_pad(&[2, 0, 0, 0, 7, 0, 0, 0]).unwrap(),
        Some(crate::spawn::SpawnPad { group: 2, index: 7 })
    );
}

#[test]
fn spawn_pad_round_udp_allocates_unique_owner_indices_and_exhausts_groups() {
    for rule in 1..=5 {
        let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "spawn").unwrap();
        let addr = transport.local_addr().unwrap();
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::bundled_revision(),
            rule,
            Default::default(),
        );
        server.set_spawn_pad_counts([2, 2, 1]);
        let camps = if rule == 1 {
            [0, 0, 0, 0]
        } else {
            [1, 1, 1, 2]
        };
        let mut clients: Vec<_> = camps
            .into_iter()
            .map(|camp| {
                StockGameClient::for_bundled_revision(
                    StockClientTransport::bind(
                        "127.0.0.1:0",
                        addr,
                        StockConnectRecord::for_open_room(),
                    )
                    .unwrap(),
                    0,
                    camp,
                    6,
                )
                .unwrap()
            })
            .collect();
        drive(
            &mut server,
            &mut clients.iter_mut().collect::<Vec<_>>(),
            0,
            100,
        );
        assert_eq!(server.roles.len(), 4);
        for round in 0..8 {
            let now = 200 + round * 200;
            server.assign_round_spawn_pads(now).unwrap();
            let output = drive(
                &mut server,
                &mut clients.iter_mut().collect::<Vec<_>>(),
                now + 1,
                40,
            );
            let mut assigned = HashSet::new();
            let mut empty = 0;
            for (owner, events) in output.iter().enumerate() {
                let pads: Vec<_> = events
                    .iter()
                    .filter_map(|e| match e {
                        StockGameEvent::SpawnPad(pad) => Some(*pad),
                        _ => None,
                    })
                    .collect();
                assert_eq!(pads.len(), 1, "only the owning client gets its placement");
                if let Some(pad) = pads[0] {
                    assert_eq!(pad.group, camps[owner]);
                    assert!(pad.index < [2, 2, 1][pad.group as usize]);
                    assert!(assigned.insert((pad.group, pad.index)));
                } else {
                    empty += 1;
                }
            }
            assert_eq!(empty, if rule == 1 { 2 } else { 1 });
        }
    }
}

#[test]
fn spawn_pad_early_udp_assignment_waits_for_settlement_deadline() {
    let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "spawn").unwrap();
    let addr = transport.local_addr().unwrap();
    let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
    let mut client = StockGameClient::for_bundled_revision(
        StockClientTransport::bind("127.0.0.1:0", addr, StockConnectRecord::for_open_room())
            .unwrap(),
        0,
        0,
        6,
    )
    .unwrap();
    drive(&mut server, &mut [&mut client], 0, 100);
    client.round_restart_at_ms = Some(1000);
    let peer = *server.roles.keys().next().unwrap();
    server
        .transport
        .send_application(
            peer,
            crate::stock_protocol::STOCK_EVENT_SPAWN_PAD,
            vec![0, 0, 0, 0, 5, 0, 0, 0],
            STOCK_DISCRETE_RELIABLE_FLAGS,
            200,
        )
        .unwrap();
    let early = drive(&mut server, &mut [&mut client], 201, 30);
    assert!(!early[0]
        .iter()
        .any(|e| matches!(e, StockGameEvent::SpawnPad(_))));
    let released = client.tick(1000).unwrap();
    let restart = released
        .iter()
        .position(|e| matches!(e, StockGameEvent::RoundRestart))
        .unwrap();
    let pad = released
        .iter()
        .position(|e| {
            matches!(
                e,
                StockGameEvent::SpawnPad(Some(crate::spawn::SpawnPad { group: 0, index: 5 }))
            )
        })
        .unwrap();
    assert!(restart < pad);
    assert!(!client
        .tick(1001)
        .unwrap()
        .iter()
        .any(|e| matches!(e, StockGameEvent::SpawnPad(_))));
}
