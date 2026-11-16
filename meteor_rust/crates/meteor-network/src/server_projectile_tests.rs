#[test]
fn projectile_ws_authenticates_source_subscribes_and_retains_late_join_flights() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let mut server = RoomServer::from_origin_res(&root).unwrap();
    for conn in [1, 2, 3] {
        server.connect(conn);
        let mut hello = hello();
        if let Message::Hello { weapon, .. } = &mut hello {
            *weapon = 1;
        }
        server.handle(conn, hello);
    }
    let mut create = create_room("deathmatch", "sn01");
    if let Message::RoomCreate { weapon, .. } = &mut create {
        *weapon = 1;
    }
    server.handle(1, create);
    for conn in [2, 3] {
        let mut join = join_room("room1");
        if let Message::RoomJoin {
            weapon,
            character_id,
            ..
        } = &mut join
        {
            *weapon = 1;
            *character_id = 0;
        }
        server.handle(conn, join);
    }
    server.handle(1, Message::RoomStart);
    let room = &server.rooms["room1"];
    let epochs = (room.match_epoch, room.round_epoch);
    let player = server.sessions[&2].player_id.clone().unwrap();
    server.handle(
        1,
        Message::ProjectileSubscribe {
            room_id: "room1".into(),
        },
    );
    let state = crate::runtime_sync::ProjectileState {
        player_id: "spoof".into(),
        id: 1,
        sequence: 1,
        kind: 1,
        weapon: 1,
        variant: 0,
        pose_id: 200,
        pose_frame: 3519.0,
        position: [1.0, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        velocity: [3.0, 4.0, 5.0],
        age: 0.1,
        active: true,
    };
    let message = |state| Message::ProjectileState {
        room_id: "room1".into(),
        match_epoch: epochs.0,
        round_epoch: epochs.1,
        state,
    };
    let decoded = Message::from_v4_binary(&message(state.clone()).to_v4_binary().unwrap()).unwrap();
    let submitted = server.handle(2, decoded);
    assert!(submitted.to_others.is_empty());
    assert!(
        matches!(submitted.to_conn.as_slice(), [(1, Message::ProjectileState { state, .. })]
        if state.player_id == player && state.position == [1.0, 2.0, 3.0])
    );
    let duplicate = server.handle(2, message(state.clone()));
    assert!(duplicate.to_conn.is_empty());
    let subscribed = server.handle(
        3,
        Message::ProjectileSubscribe {
            room_id: "room1".into(),
        },
    );
    assert!(subscribed
        .to_sender
        .iter()
        .any(|m| matches!(m, Message::ProjectileState { state, .. } if state.active)));
    let health = server.rooms["room1"].combatants[&player].health;
    let mut stop = state.clone();
    stop.sequence = 2;
    stop.active = false;
    let stopped = server.handle(2, message(stop));
    assert_eq!(stopped.to_conn.len(), 2);
    assert_eq!(
        server.rooms["room1"].combatants[&player].health, health,
        "visuals cannot deal damage"
    );
    assert_eq!(server.rooms["room1"].projectiles.active().count(), 0);
    let mut stale = state.clone();
    stale.sequence = 3;
    assert!(server.handle(2, message(stale)).to_conn.is_empty());
    let mut invalid = state.clone();
    invalid.id = 2;
    invalid.pose_frame = 1.0;
    assert!(server.handle(2, message(invalid)).to_conn.is_empty());
    let mut old = message(state.clone());
    if let Message::ProjectileState { round_epoch, .. } = &mut old {
        *round_epoch = round_epoch.saturating_sub(1);
    }
    assert!(server.handle(2, old).to_conn.is_empty());

    server.disconnect(2);
    assert!(!server.rooms["room1"].projectiles.known(&player, 1));
}
