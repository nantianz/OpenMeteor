#[test]
fn authority_target_hit_clock_keeps_precision_after_long_room_uptime() {
    for fps in [60, 120] {
        for start in [100.0, 3600.0, 86400.0] {
            let mut server = RoomServer::new();
            for conn in [1, 2] {
                server.connect(conn);
                server.handle(conn, hello());
            }
            server.handle(1, create_room("deathmatch", "sn01"));
            server.handle(2, join_room("room1"));
            server.handle(2, Message::RoomReady { ready: true });
            server.handle(1, Message::RoomStart);
            let room = server.rooms.get_mut("room1").unwrap();
            room.elapsed = start as f32;
            room.hit_elapsed = start;
            room.round.as_mut().unwrap().ends_at = start as f32 + 1000.0;
            let target = room.combatants.get_mut("p2").unwrap();
            target.target_hit_clock.advance_spawn(3.0);
            target.target_hit_clock.reset_target_hit_clock(start);
            for _ in 0..fps / 5 {
                server.advance_authority(1.0 / f64::from(fps), 0.0);
            }
            let room = &server.rooms["room1"];
            assert!(
                !can_attack_combatant(&room.combatants["p2"], room.hit_elapsed),
                "fps={fps}, start={start}, elapsed={}",
                room.hit_elapsed - start
            );
            server.advance_authority(1.0 / f64::from(fps), 0.0);
            let room = &server.rooms["room1"];
            assert!(can_attack_combatant(
                &room.combatants["p2"],
                room.hit_elapsed
            ));
        }
    }
}
