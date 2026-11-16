#[test]
fn stock_spawn_pad_udp_bridge_routes_only_the_assigned_npc() {
    let transport = StockServerTransport::bind("127.0.0.1:0", "serverlist", "pads").unwrap();
    let mut server = StockGameServer::new(transport, StockHeroCatalog::bundled_revision());
    let mut world = World::new();
    let first = world.spawn_empty().id();
    let second = world.spawn_empty().id();
    let (tx, rx) = std::sync::mpsc::channel();
    let mut bridge = Bridge::default();
    bridge
        .sync(&server, vec![role(first), role(second)], 0)
        .unwrap();
    pump(&mut server, &mut [], &mut bridge, &tx, 0, 100);
    let mut net = NetState::default();
    for message in rx.try_iter() {
        if let NetInbound::StockNpcEvent(entity, event) = message {
            apply_event(&mut net, entity, event);
        }
    }
    net.pending_respawns.clear();
    net.pending_spawn_relocations.clear();
    let peer = bridge.clients[&first].transport().local_addr().unwrap();
    server
        .transport_mut()
        .send_application(
            peer,
            meteor_network::stock_protocol::STOCK_EVENT_SPAWN_PAD,
            vec![2, 0, 0, 0, 4, 0, 0, 0],
            meteor_network::stock_protocol::STOCK_DISCRETE_RELIABLE_FLAGS,
            200,
        )
        .unwrap();
    pump(&mut server, &mut [], &mut bridge, &tx, 201, 40);
    let mut owners = Vec::new();
    for message in rx.try_iter() {
        if let NetInbound::StockNpcEvent(entity, event) = message {
            if matches!(event, StockGameEvent::SpawnPad(_)) {
                owners.push(entity);
            }
            apply_event(&mut net, entity, event);
        }
    }
    assert_eq!(owners, vec![first]);
    let id = format!("stock-{}", bridge.clients[&first].actor_id().unwrap());
    assert_eq!(net.pending_respawns, vec![id.clone()]);
    assert_eq!(
        net.pending_spawn_pads.get(&id),
        Some(&Some(meteor_network::spawn::SpawnPad {
            group: 2,
            index: 4
        }))
    );
    assert_eq!(net.pending_spawn_pads.len(), 1);
}
