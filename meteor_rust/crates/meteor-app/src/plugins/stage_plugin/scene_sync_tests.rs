#![cfg(feature = "original-assets")]
use super::*;
use crate::plugins::combat_plugin::forward_network_prop_hit;
use crate::plugins::item_plugin::StageProp;
use crate::plugins::network_plugin::{control_test_channel, NetState, NetStatus, NetTx};
use meteor_network::protocol::{Message, RoomMemberWire};

fn runtime() -> SceneScriptRuntime {
    mode_runtime("sn01", None)
}

fn mode_runtime(stage: &str, rule: Option<u32>) -> SceneScriptRuntime {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let mut sources = Vec::new();
    let boxes = if stage == "sn21" {
        "sn21/sn21box.pst"
    } else {
        "PScriptLib/box.pst"
    };
    for include in [
        "PScriptLib/Def.h",
        boxes,
        "PScriptLib/desk.pst",
        "PScriptLib/chair.pst",
        "PScriptLib/jug.pst",
    ] {
        sources.push(crate::text_codec::decode_text(
            &std::fs::read(root.join(include)).unwrap(),
        ));
    }
    sources.push(crate::text_codec::decode_text(
        &std::fs::read(root.join(format!("{stage}/{stage}.pst"))).unwrap(),
    ));
    let desc = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join(format!("{stage}/{stage}.des"))).unwrap(),
    );
    let instances = desc
        .dummies
        .iter()
        .enumerate()
        .map(|(index, dummy)| SceneItemInstance {
            name: dummy.name.clone(),
            index,
            props: [
                ("active".into(), PstValue::Num(1.0)),
                ("visible".into(), PstValue::Num(f64::from(dummy.visible))),
                ("interactive".into(), PstValue::Num(1.0)),
                ("state".into(), PstValue::Num(3.0)),
                ("_has_fmc".into(), PstValue::Num(1.0)),
            ]
            .into_iter()
            .collect(),
        })
        .collect();
    let mut runtime = SceneScriptRuntime::load(&sources.join("\n"), instances, 250_000).unwrap();
    if let Some(rule) = rule {
        runtime.host_mut().set_game_mode(i64::from(rule));
        runtime.host_mut().exclude_instances(
            desc.dummies
                .iter()
                .enumerate()
                .filter(|(_, d)| !d.admits_ticket(1 << rule))
                .map(|(i, _)| i),
        );
    }
    runtime.start().unwrap();
    runtime.host_mut().take_net_batches();
    runtime
}

fn net(player: &str) -> NetState {
    NetState {
        status: NetStatus::InRoom,
        address: "ws://localhost".into(),
        room_id: "room1".into(),
        player_id: player.into(),
        round_active: true,
        match_epoch: Some(1),
        round_epoch: Some(1),
        members: ["p1", "p2"]
            .into_iter()
            .map(|id| RoomMemberWire {
                player_id: id.into(),
                name: id.into(),
                character_id: 0,
                weapon: 6,
                team: Some(1),
                ready: true,
                connected: true,
            })
            .collect(),
        ..Default::default()
    }
}

fn app(net: NetState) -> (App, impl Fn() -> Vec<Message>) {
    let (tx, drain) = control_test_channel();
    let mut app = App::new();
    app.init_resource::<Time>()
        .insert_resource(net)
        .insert_resource(tx)
        .insert_resource(StageScriptRuntime(Some(mode_runtime("sn01", None))))
        .init_resource::<crate::plugins::stage_collider::StageCollider>()
        .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
        .add_systems(Update, tick_stage_script);
    for name in ["D_BBox01", "D_BBox02", "D_wpBBox01", "D_wpBBox02"] {
        app.world_mut().spawn((
            StageProp {
                name: name.into(),
                model: "bbox02".into(),
                center_offset: Vec3::ZERO,
                half_extents: Vec3::ONE,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: true,
                collision_enabled: true,
                breakable: name.starts_with("D_BBox"),
                broken: false,
            },
            Visibility::Visible,
            GlobalTransform::IDENTITY,
        ));
    }
    app.update();
    (app, drain)
}

#[test]
fn solo_sn21_second_star_reopens_consumed_candy() {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<NetState>()
        .insert_resource(StageScriptRuntime(Some(mode_runtime("sn21", None))))
        .init_resource::<crate::plugins::stage_collider::StageCollider>()
        .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
        .add_systems(Update, tick_stage_script);
    let prop = app
        .world_mut()
        .spawn((
            StageProp {
                name: "D_Item05".into(),
                model: "item11".into(),
                center_offset: Vec3::ZERO,
                half_extents: Vec3::ONE,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: false,
                collision_enabled: false,
                breakable: false,
                broken: false,
            },
            Visibility::Hidden,
            GlobalTransform::IDENTITY,
        ))
        .id();
    for strike in 0..2 {
        if strike == 1 {
            app.world_mut()
                .get_mut::<StageProp>(prop)
                .unwrap()
                .respawn_timer = 99_999.0;
            *app.world_mut().get_mut::<Visibility>(prop).unwrap() = Visibility::Hidden;
        }
        let mut script = app.world_mut().resource_mut::<StageScriptRuntime>();
        let runtime = script.script_mut().unwrap();
        if strike == 1 {
            runtime.set_item_property("D_Item05", "active", PstValue::Num(0.0));
            runtime.set_item_property("D_Item05", "interactive", PstValue::Num(0.0));
        }
        runtime.set_item_property("D_sn21st01", "state", PstValue::Num(3.0));
        runtime
            .call_item(
                "D_sn21st01",
                "OnAttack",
                &[PstValue::Num(0.0), PstValue::Num(100.0)],
            )
            .unwrap();
        app.update();
        assert!(app.world().get::<StageProp>(prop).unwrap().interactive);
        assert_eq!(
            app.world().get::<StageProp>(prop).unwrap().respawn_timer,
            0.0
        );
        assert_eq!(
            *app.world().get::<Visibility>(prop).unwrap(),
            Visibility::Visible
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn stock_sn21_star_keeps_all_200_operations_including_phones() {
    use bevy::ecs::system::RunSystemOnce;
    use meteor_network::stock_protocol::{
        STOCK_EFFECT_XSTAR, STOCK_SCENE_OPS_PER_PACKET, STOCK_SCENE_OP_INTERACTIVE,
    };

    let (tx, drain) = control_test_channel();
    let mut app = App::new();
    app.init_resource::<Time>()
        .insert_resource(NetState {
            status: NetStatus::InRoom,
            address: "udp-host://127.0.0.1:5230".into(),
            stage: "sn21".into(),
            ..Default::default()
        })
        .insert_resource(tx)
        .insert_resource(StageScriptRuntime(Some(mode_runtime("sn21", None))))
        .init_resource::<crate::plugins::stage_collider::StageCollider>()
        .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
        .add_systems(Update, tick_stage_script);
    app.world_mut().run_system_once(start_stage_script).unwrap();
    let mut script = app.world_mut().resource_mut::<StageScriptRuntime>();
    let runtime = script.script_mut().unwrap();
    let phone = runtime.host().resolve_name("D_Item103").unwrap() as u32;
    runtime.set_item_property("D_sn21st01", "state", PstValue::Num(3.0));
    runtime
        .call_item(
            "D_sn21st01",
            "OnAttack",
            &[PstValue::Num(0.0), PstValue::Num(100.0)],
        )
        .unwrap();
    app.update();
    let messages = drain();
    let operations: Vec<_> = messages
        .iter()
        .flat_map(|message| {
            let Message::SceneOps { ops, .. } = message else {
                panic!("expected scene transaction chunks")
            };
            assert!(ops.len() <= 63);
            crate::plugins::network_plugin::stock_scene_ops_from_message(ops, 1).operations
        })
        .collect();
    let wire = meteor_network::stock_protocol::StockSceneOpsEvent {
        timestamp_ms: 1,
        operations,
    };
    assert_eq!(wire.operations.len(), 200);
    assert_eq!(wire.operations[0].value, STOCK_EFFECT_XSTAR);
    assert_eq!(
        wire.operations.chunks(STOCK_SCENE_OPS_PER_PACKET).count(),
        4
    );
    assert!(wire.operations.iter().any(|op| {
        op.reference == phone && op.kind == STOCK_SCENE_OP_INTERACTIVE && op.value == 1
    }));
}

fn value(app: &App, name: &str, field: &str) -> f64 {
    let runtime = app
        .world()
        .resource::<StageScriptRuntime>()
        .script()
        .unwrap();
    runtime
        .host()
        .instance(runtime.host().resolve_name(name).unwrap())
        .unwrap()
        .get(field)
        .as_f64()
}

fn relay(guest: &mut App, messages: Vec<Message>) {
    for message in messages {
        if let Message::SceneOps { ops, .. } = message {
            guest
                .world_mut()
                .resource_mut::<NetState>()
                .pending_scene_ops
                .extend(ops);
        } else {
            panic!("unexpected scene output: {message:?}");
        }
    }
    guest.update();
}

fn check_sn03_network_button(udp: bool, guest_attacks: bool) {
    for (button, door) in [
        ("D_Abutton01", "D_ADoor01"),
        ("D_Abutton02", "D_ADoor01"),
        ("D_Bbutton01", "D_BDoor01"),
        ("D_Bbutton02", "D_BDoor01"),
    ] {
        check_sn03_network_button_side(udp, guest_attacks, button, door);
    }
}

fn check_sn03_network_button_side(udp: bool, guest_attacks: bool, button: &str, door: &str) {
    use crate::plugins::network_plugin::{scene_ops_from_stock, stock_scene_ops_from_message};
    use meteor_network::stock_protocol::StockSceneOpsEvent;
    let mut host_net = net("p1");
    let mut guest_net = net("p2");
    for n in [&mut host_net, &mut guest_net] {
        n.mode = "escort".into();
        n.stage = "sn03".into();
    }
    if udp {
        host_net.address = "udp-host://127.0.0.1:5230".into();
        guest_net.address = "udp://127.0.0.1:5230".into();
    }
    let (mut host, host_output) = app(host_net);
    let (mut guest, guest_output) = app(guest_net);
    host.insert_resource(StageScriptRuntime(Some(mode_runtime("sn03", Some(2)))));
    guest.insert_resource(StageScriptRuntime(Some(mode_runtime("sn03", Some(2)))));
    host.world_mut()
        .resource_mut::<StageScriptRuntime>()
        .script_mut()
        .unwrap()
        .update(30.0)
        .unwrap();
    let deliver = |target: &mut App, messages: Vec<Message>| {
        for message in messages {
            let Message::SceneOps { ops, .. } = message else {
                panic!("expected scene ops")
            };
            let ops = if udp {
                let wire = stock_scene_ops_from_message(&ops, 30000);
                scene_ops_from_stock(&StockSceneOpsEvent::decode(&wire.encode()).unwrap())
            } else {
                ops
            };
            target
                .world_mut()
                .resource_mut::<NetState>()
                .pending_scene_ops
                .extend(ops);
        }
        target.update();
    };
    for cycle in 0..3 {
        let guest_attacks = if cycle == 1 {
            !guest_attacks
        } else {
            guest_attacks
        };
        if cycle == 2 {
            for a in [&mut host, &mut guest] {
                a.insert_resource(StageScriptRuntime(Some(mode_runtime("sn03", Some(2)))));
            }
            host.world_mut()
                .resource_mut::<StageScriptRuntime>()
                .script_mut()
                .unwrap()
                .update(30.0)
                .unwrap();
        }
        if guest_attacks && !udp {
            host.world_mut()
                .resource_mut::<NetState>()
                .pending_scene_prop_hits
                .push(Message::ScenePropHit {
                    room_id: "room1".into(),
                    match_epoch: 1,
                    round_epoch: 1,
                    player_id: "p2".into(),
                    reference: button.into(),
                    pose_id: 262,
                    frame: 9770.0,
                    damage: 1,
                });
            host.update();
        } else {
            let attacker = if guest_attacks { &mut guest } else { &mut host };
            attacker
                .world_mut()
                .resource_mut::<StageScriptRuntime>()
                .script_mut()
                .unwrap()
                .call_item(
                    button,
                    "OnAttack",
                    &[PstValue::Num(0.0), PstValue::Num(1.0)],
                )
                .unwrap();
            attacker.update();
        }
        if udp && guest_attacks {
            deliver(&mut host, guest_output());
        } else {
            deliver(&mut guest, host_output());
        }
        for a in [&host, &guest] {
            assert_eq!(value(a, door, "pose"), 1.0, "opening must reach both peers");
        }
        assert!(
            host_output().is_empty(),
            "received activation must not echo"
        );

        host.world_mut()
            .resource_mut::<StageScriptRuntime>()
            .script_mut()
            .unwrap()
            .set_item_property(door, "state", PstValue::Num(3.0));
        host.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.2));
        host.update();
        assert_eq!(
            value(&host, door, "pose"),
            1.0,
            "door must not close immediately after a late guest activation"
        );
        for _ in 0..160 {
            host.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(0.05));
            host.update();
        }
        deliver(&mut guest, host_output());
        assert_eq!(value(&host, door, "pose"), 2.0);
        assert_eq!(value(&guest, door, "pose"), 2.0);
        host.world_mut()
            .resource_mut::<StageScriptRuntime>()
            .script_mut()
            .unwrap()
            .set_item_property(door, "state", PstValue::Num(3.0));
        for _ in 0..3 {
            host.update();
        }
        deliver(&mut guest, host_output());
        assert_eq!(value(&host, door, "pose"), 0.0);
        assert_eq!(value(&guest, door, "pose"), 0.0);
    }
}

#[test]
fn sn03_network_button_host_and_ws_guest_open_and_auto_close() {
    for (udp, guest) in [(false, false), (false, true), (true, false)] {
        check_sn03_network_button(udp, guest);
    }
}

#[test]
fn sn03_network_button_udp_guest_keeps_full_open_interval() {
    check_sn03_network_button(true, true);
}

#[test]
fn sn03_network_button_duplicate_does_not_restart_timer_or_echo() {
    let mut guest = mode_runtime("sn03", Some(2));
    guest
        .call_item(
            "D_Abutton01",
            "OnAttack",
            &[PstValue::Num(0.0), PstValue::Num(1.0)],
        )
        .unwrap();
    let ops = guest
        .host_mut()
        .take_net_batches()
        .into_iter()
        .flatten()
        .map(|op| scene_op_from_runtime(&op))
        .collect::<Vec<_>>();
    let mut host = mode_runtime("sn03", Some(2));
    host.update(30.0).unwrap();

    reconcile_sn03_button_transaction(&mut host, &ops[..1]);
    let door = host.host().resolve_name("D_ADoor01").unwrap();
    assert_eq!(
        host.host().instance(door).unwrap().get("pose"),
        PstValue::Num(0.0)
    );
    reconcile_sn03_button_transaction(&mut host, &ops);
    for op in &ops {
        apply_network_scene_op(&mut host, op);
    }
    assert!(host.host_mut().take_net_batches().is_empty());
    host.update(3.0).unwrap();
    reconcile_sn03_button_transaction(&mut host, &ops);
    for op in &ops {
        apply_network_scene_op(&mut host, op);
    }
    assert!(host.host_mut().take_net_batches().is_empty());
    host.set_item_property("D_ADoor01", "state", PstValue::Num(3.0));
    host.update(4.1).unwrap();
    assert_eq!(
        host.host().instance(door).unwrap().get("pose"),
        PstValue::Num(2.0)
    );
}

#[test]
fn ws_real_boxes_converge_without_any_remote_pose_replay() {
    let (mut host, host_output) = app(net("p1"));
    let (mut guest, guest_output) = app(net("p2"));
    for (index, name) in ["D_BBox01", "D_BBox02"].into_iter().enumerate() {
        if index == 0 {
            assert!(forward_network_prop_hit(
                guest.world().resource::<NetState>(),
                Some(guest.world().resource::<NetTx>()),
                name,
                262,
                9770.0,
            ));
            assert_eq!(value(&guest, name, "pose"), 0.0, "no speculative open");
            let mut reports = guest_output();
            assert_eq!(reports.len(), 1);

            if let Message::ScenePropHit {
                player_id, damage, ..
            } = &mut reports[0]
            {
                *player_id = "p2".into();
                *damage = 10000;
            } else {
                panic!("missing reliable report");
            }
            host.world_mut()
                .resource_mut::<NetState>()
                .pending_scene_prop_hits
                .extend(reports);
        } else {
            assert!(!forward_network_prop_hit(
                host.world().resource::<NetState>(),
                Some(host.world().resource::<NetTx>()),
                name,
                262,
                9770.0,
            ));
            host.world_mut()
                .resource_mut::<StageScriptRuntime>()
                .script_mut()
                .unwrap()
                .call_item(
                    name,
                    "OnAttack",
                    &[PstValue::Num(0.0), PstValue::Num(10000.0)],
                )
                .unwrap();
        }
        host.update();
        let output = host_output();
        assert!(!output.is_empty());
        relay(&mut guest, output);
        for app in [&host, &guest] {
            assert_eq!(value(app, name, "pose"), 1.0);
            assert_eq!(value(app, name, "interactive"), 0.0);
            assert_eq!(value(app, name, "collision"), 0.0);
            assert_eq!(
                value(app, &format!("D_wpBBox0{}", index + 1), "active"),
                1.0
            );
        }

        host.world_mut()
            .resource_mut::<NetState>()
            .pending_scene_prop_hits
            .push(Message::ScenePropHit {
                room_id: "room1".into(),
                match_epoch: 1,
                round_epoch: 1,
                player_id: "p2".into(),
                reference: name.into(),
                pose_id: 262,
                frame: 9770.0,
                damage: 10000,
            });
        host.update();
        assert!(host_output().is_empty());

        {
            let mut script = host.world_mut().resource_mut::<StageScriptRuntime>();
            let runtime = script.script_mut().unwrap();
            let reference = runtime.host().resolve_name(name).unwrap();
            runtime.host_mut().instances_mut()[reference].set("state", PstValue::Num(3.0));
        }
        for _ in 0..4 {
            host.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
            host.update();
        }
        relay(&mut guest, host_output());
        for app in [&mut host, &mut guest] {
            assert_eq!(value(app, name, "active"), 0.0);
            let mut query = app.world_mut().query::<(&StageProp, &Visibility)>();
            let (_, visibility) = query
                .iter(app.world())
                .find(|(prop, _)| prop.name == name)
                .unwrap();
            assert_eq!(*visibility, Visibility::Hidden);
        }
        assert!(
            guest_output().is_empty(),
            "applying scene ops never echoes or predicts"
        );
    }
}

#[test]
fn scene_bootstrap_waits_for_runtime_and_stale_prop_hits_do_not_open_boxes() {
    let (mut host, _) = app(net("p1"));
    let script = host
        .world_mut()
        .remove_resource::<StageScriptRuntime>()
        .unwrap();
    host.insert_resource(StageScriptRuntime(None));
    let op = meteor_network::protocol::SceneOp {
        kind: "set".into(),
        reference: JsonValue::from("D_BBox01"),
        field: "pose".into(),
        args: vec![JsonValue::from(1), JsonValue::from(0)],
    };
    host.world_mut()
        .resource_mut::<NetState>()
        .pending_scene_ops
        .push(op);
    host.update();
    assert_eq!(
        host.world().resource::<NetState>().pending_scene_ops.len(),
        1
    );
    host.insert_resource(script);
    host.update();
    assert_eq!(value(&host, "D_BBox01", "pose"), 1.0);
    for (room, epoch, player) in [
        ("other", 1, "p2"),
        ("room1", 0, "p2"),
        ("room1", 1, "outsider"),
    ] {
        host.world_mut()
            .resource_mut::<NetState>()
            .pending_scene_prop_hits
            .push(Message::ScenePropHit {
                room_id: room.into(),
                match_epoch: epoch,
                round_epoch: 1,
                player_id: player.into(),
                reference: "D_BBox02".into(),
                pose_id: 262,
                frame: 9770.0,
                damage: 10000,
            });
        host.update();
        assert_eq!(value(&host, "D_BBox02", "pose"), 0.0);
    }
}

#[test]
fn udp_real_box_transactions_preserve_break_effect_and_host_cleanup() {
    use crate::plugins::network_plugin::{scene_ops_from_stock, stock_scene_ops_from_message};
    use meteor_network::stock_protocol::StockSceneOpsEvent;
    let mut host_net = net("p1");
    host_net.address = "udp-host://127.0.0.1:5230".into();
    host_net.mode = "deathmatch".into();
    let mut guest_net = net("p2");
    guest_net.address = "udp://127.0.0.1:5230".into();
    let (mut host, host_output) = app(host_net);
    let (mut guest, guest_output) = app(guest_net);
    assert!(!forward_network_prop_hit(
        guest.world().resource::<NetState>(),
        None,
        "D_BBox01",
        262,
        9770.0
    ));
    guest
        .world_mut()
        .resource_mut::<StageScriptRuntime>()
        .script_mut()
        .unwrap()
        .call_item(
            "D_BBox01",
            "OnAttack",
            &[PstValue::Num(1.0), PstValue::Num(10000.0)],
        )
        .unwrap();
    guest.update();
    let mut effects = 0;
    for message in guest_output() {
        let Message::SceneOps { ops, .. } = message else {
            panic!("scene transaction expected");
        };
        let wire = stock_scene_ops_from_message(&ops, 100);
        assert_eq!(
            wire.operations.len(),
            ops.len(),
            "BoxBRK must not disappear during encoding"
        );
        let received = StockSceneOpsEvent::decode(&wire.encode()).unwrap();
        let decoded = scene_ops_from_stock(&received);
        effects += decoded
            .iter()
            .filter(|op| op.kind == "effect" && op.field == "BoxBRK")
            .count();
        host.world_mut()
            .resource_mut::<NetState>()
            .pending_scene_ops
            .extend(decoded);
    }
    assert_eq!(effects, 1);
    host.update();
    for app in [&host, &guest] {
        assert_eq!(value(app, "D_BBox01", "pose"), 1.0);
        assert_eq!(value(app, "D_BBox01", "collision"), 0.0);
        assert_eq!(value(app, "D_wpBBox01", "active"), 1.0);
    }
    assert!(host_output().is_empty(), "received break does not echo");
    host.world_mut()
        .resource_mut::<StageScriptRuntime>()
        .script_mut()
        .unwrap()
        .set_item_property("D_BBox01", "state", PstValue::Num(3.0));
    for _ in 0..4 {
        host.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
        host.update();
    }
    for message in host_output() {
        let Message::SceneOps { ops, .. } = message else {
            panic!("scene transaction expected");
        };
        let wire = stock_scene_ops_from_message(&ops, 300);
        guest
            .world_mut()
            .resource_mut::<NetState>()
            .pending_scene_ops
            .extend(scene_ops_from_stock(&wire));
    }
    guest.update();
    assert_eq!(value(&host, "D_BBox01", "active"), 0.0);
    assert_eq!(value(&guest, "D_BBox01", "active"), 0.0);
    assert!(guest_output().is_empty());
}

#[test]
fn escort_qin_pickup_raises_authored_steps_once_and_syncs_without_guest_echo() {
    use crate::plugins::network_plugin::{scene_ops_from_stock, stock_scene_ops_from_message};
    use meteor_network::stock_protocol::StockSceneOpsEvent;
    for udp in [false, true] {
        let definition = crate::plugins::escort_assets::load_escort("sn02").unwrap();
        let (mut host, host_output) = app(net("p1"));
        let (mut guest, guest_output) = app(net("p2"));
        for a in [&mut host, &mut guest] {
            a.insert_resource(StageScriptRuntime(Some(mode_runtime("sn02", Some(2)))))
                .insert_resource(crate::plugins::escort_assets::LoadedEscort(Some(
                    definition.clone(),
                )))
                .add_systems(
                    Update,
                    apply_network_escort_pickups.before(tick_stage_script),
                );
            let mut n = a.world_mut().resource_mut::<NetState>();
            n.stage = "sn02".into();
            n.mode = "escort".into();
            if udp {
                n.address = if n.player_id == "p1" {
                    "udp-host://127.0.0.1:5230"
                } else {
                    "udp://127.0.0.1:5230"
                }
                .into();
            }
            n.pending_escort_pickups.push("p2".into());
        }
        for name in ["D_sn02st01", "D_sn02st02", "D_sn02st03"] {
            assert_eq!(value(&host, name, "active"), 0.0);
        }
        host.update();
        guest.update();
        let output = host_output();
        assert_eq!(
            output.len(),
            1,
            "one pickup produces one original NetEvent transaction"
        );
        assert!(
            guest_output().is_empty(),
            "a pickup event must not produce guest transactions"
        );
        for message in output {
            let Message::SceneOps { ops, .. } = message else {
                panic!("expected scene ops")
            };
            let ops = if udp {
                let wire = stock_scene_ops_from_message(&ops, 200);
                scene_ops_from_stock(&StockSceneOpsEvent::decode(&wire.encode()).unwrap())
            } else {
                ops
            };
            guest
                .world_mut()
                .resource_mut::<NetState>()
                .pending_scene_ops
                .extend(ops);
        }
        guest.update();
        for (name, frame) in [
            ("D_sn02st01", 0.0),
            ("D_sn02st02", 60.0),
            ("D_sn02st03", 120.0),
        ] {
            for a in [&host, &guest] {
                assert_eq!(value(a, name, "active"), 1.0);
                assert_eq!(value(a, name, "pose"), 1.0);
                assert_eq!(value(a, name, "pose_loop"), 1.0);
                assert_eq!(value(a, name, "frame"), frame);
            }
        }
        host.update();
        guest.update();
        assert!(host_output().is_empty());
        assert!(guest_output().is_empty());

        use bevy::ecs::system::RunSystemOnce;
        for a in [&mut host, &mut guest] {
            a.world_mut().run_system_once(start_stage_script).unwrap();
            for name in ["D_sn02st01", "D_sn02st02", "D_sn02st03"] {
                assert_eq!(
                    value(a, name, "active"),
                    0.0,
                    "new round resets raised steps"
                );
            }
            assert_eq!(value(a, "D_IPItem01", "active"), 1.0);
        }
    }
}

#[test]
fn escort_qin_offline_pickup_calls_original_scene_script_and_both_teams_deliver() {
    for team in [1, 2] {
        let (mut app, player) = tests::escort_test_app(false);
        let definition = crate::plugins::escort_assets::load_escort("sn02").unwrap();
        app.world_mut().resource_mut::<LaunchRequest>().stage_id = "sn02".into();
        app.world_mut().resource_mut::<LaunchRequest>().mode = "escort".into();
        app.insert_resource(crate::plugins::escort_assets::LoadedEscort(Some(
            definition.clone(),
        )))
        .insert_resource(StageScriptRuntime(Some(mode_runtime("sn02", Some(2)))))
        .add_systems(Update, sync_network_escort_safe_zones);
        let home = definition.stage.flag_home;
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = Vec3::new(home.x, home.y, home.z);
        if team == 2 {
            app.world_mut().entity_mut(player).remove::<LocalFighter>();
            app.world_mut().entity_mut(player).insert(MissionNpc {
                id: 7,
                name: "escort tester".into(),
                team: 2,
                in_scene_roster: true,
                view: 1000.0,
                think: 0.0,
                forced_behavior: None,
                is_vip: false,
                attack_target_slot: None,
                behavior_args: Vec::new(),
                attack_target_remaining: 0,
            });
        }
        app.update();
        assert!(app.world().get::<EscortCarrying>(player).is_some());
        let mut zones = app.world_mut().query::<&NetworkEscortSafeZone>();
        assert_eq!(
            zones.iter(app.world()).count(),
            1,
            "one shared exit is drawn once"
        );
        for name in ["D_sn02st01", "D_sn02st02", "D_sn02st03"] {
            assert_eq!(value(&app, name, "active"), 1.0);
        }
        assert!(
            !app.world()
                .resource::<crate::components::fighter::MatchEnd>()
                .settled
        );
        let safe = definition.stage.safe_zones[0];
        app.world_mut()
            .get_mut::<Transform>(player)
            .unwrap()
            .translation = Vec3::new(safe.center_x, safe.center_y, safe.center_z);
        app.update();
        let end = app
            .world()
            .resource::<crate::components::fighter::MatchEnd>();
        assert!(end.settled);
        assert_eq!(
            end.result,
            if team == 1 {
                meteor_network::modes::ROUND_RESULT_TEAM2_WIPED
            } else {
                meteor_network::modes::ROUND_RESULT_TEAM1_WIPED
            }
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn sn04_host_gate_damage_reaches_guest_rigid_parts_over_both_codecs() {
    use crate::plugins::network_plugin::{scene_ops_from_stock, stock_scene_ops_from_message};
    use crate::plugins::render_plugin::{sync_fmc_with_script, update_fmc_clips, FmcClip};
    use meteor_network::stock_protocol::StockSceneOpsEvent;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let fmc = meteor_asset::fmc::parse_fmc(
        &std::fs::read_to_string(root.join("cmodel/Pdoor02.fmc")).unwrap(),
    );
    let poses = meteor_asset::fmc::parse_prop_poses(
        &std::fs::read_to_string(root.join("cmodel/Pdoor02.pos")).unwrap(),
    );
    for udp in [false, true] {
        for (gate, team) in [("D_APdoor", 2), ("D_BPdoor", 1)] {
            let mut peers = Vec::new();
            let mut outputs = Vec::new();
            for player in ["p1", "p2"] {
                let mut n = net(player);
                n.mode = "siege".into();
                n.stage = "sn04".into();
                n.members[0].team = Some(team);
                n.members[1].team = Some(3 - team);
                if udp {
                    n.address = if player == "p1" {
                        "udp-host://127.0.0.1:5230"
                    } else {
                        "udp://127.0.0.1:5230"
                    }
                    .into();
                }
                let (tx, output) = control_test_channel();
                let mut a = App::new();
                a.init_resource::<Time>()
                    .insert_resource(n)
                    .insert_resource(tx)
                    .insert_resource(StageScriptRuntime(Some(mode_runtime("sn04", Some(3)))))
                    .init_resource::<crate::plugins::stage_collider::StageCollider>()
                    .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
                    .add_systems(
                        Update,
                        (update_fmc_clips, sync_fmc_with_script, tick_stage_script).chain(),
                    );
                for part_index in 0..fmc.scene_object_count as usize {
                    for _ in 0..2 {
                        a.world_mut().spawn((
                            Transform::default(),
                            GlobalTransform::IDENTITY,
                            Visibility::Visible,
                            StageProp {
                                name: gate.into(),
                                model: "Pdoor02".into(),
                                center_offset: Vec3::ZERO,
                                half_extents: Vec3::ONE,
                                hit_count: 0,
                                respawn_timer: 0.0,
                                interactive: true,
                                collision_enabled: false,
                                breakable: false,
                                broken: false,
                            },
                            FmcClip {
                                name: gate.into(),
                                clip: fmc.clone(),
                                part_index,
                                poses: poses.clone(),
                                pose_index: 0,
                                pose_start: 0,
                                pose_end: 0,
                                pose_loop: 0,
                                pose_revision: 0,
                                anim_time: 0.0,
                                state: 3,
                                frame_set: None,
                                base_translation: Vec3::ZERO,
                                base_rotation: Quat::IDENTITY,
                                collider_owner: None,
                                collider_vertex_offset: 0,
                                local_verts: Vec::new(),
                                last_reported_state: 3,
                            },
                        ));
                    }
                }
                a.update();
                peers.push(a);
                outputs.push(output);
            }
            for (damage, expected_pose) in [(2600, 2), (2500, 4), (2500, 6), (2501, 8)] {
                peers[0]
                    .world_mut()
                    .resource_mut::<StageScriptRuntime>()
                    .script_mut()
                    .unwrap()
                    .call_item(
                        gate,
                        "OnAttack",
                        &[PstValue::Num(0.0), PstValue::Num(damage as f64)],
                    )
                    .unwrap();
                peers[0].update();
                let messages = outputs[0]();
                assert!(
                    !messages.is_empty(),
                    "gate damage must commit a transaction"
                );
                for message in messages {
                    let Message::SceneOps { ops, .. } = message else {
                        panic!("scene ops expected")
                    };
                    let ops = if udp {
                        let wire = stock_scene_ops_from_message(&ops, 100);
                        assert_eq!(wire.operations.len(), ops.len());
                        scene_ops_from_stock(&StockSceneOpsEvent::decode(&wire.encode()).unwrap())
                    } else {
                        let message = Message::SceneOps {
                            room_id: "room1".into(),
                            authority_generated: false,
                            ops,
                        };
                        let Message::SceneOps { ops, .. } =
                            Message::from_v4_binary(&message.to_v4_binary().unwrap()).unwrap()
                        else {
                            panic!("scene ops expected")
                        };
                        ops
                    };
                    peers[1]
                        .world_mut()
                        .resource_mut::<NetState>()
                        .pending_scene_ops
                        .extend(ops);
                }
                for _ in 0..120 {
                    for a in &mut peers {
                        a.world_mut()
                            .resource_mut::<Time>()
                            .advance_by(std::time::Duration::from_secs_f32(1.0 / 30.0));
                        a.update();
                    }
                }
                for a in &mut peers {
                    assert_eq!(value(a, gate, "pose"), expected_pose as f64);
                    let mut q = a.world_mut().query::<(&FmcClip, &Transform)>();
                    for (clip, _) in q.iter(a.world()) {
                        assert_eq!(clip.pose_index, expected_pose);
                        assert_eq!(clip.state, 3);
                    }
                }
                let mut host_query = peers[0].world_mut().query::<&Transform>();
                let host_transforms = host_query
                    .iter(peers[0].world())
                    .copied()
                    .collect::<Vec<_>>();
                let mut guest_query = peers[1].world_mut().query::<&Transform>();
                let guest_transforms = guest_query
                    .iter(peers[1].world())
                    .copied()
                    .collect::<Vec<_>>();
                assert_eq!(
                    host_transforms, guest_transforms,
                    "every guest rigid part must match the host"
                );
                assert!(
                    outputs[1]().is_empty(),
                    "guest must not echo host transactions"
                );
            }
            assert_eq!(value(&peers[1], gate, "interactive"), 0.0);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn udp_siege_host_applies_common_props_but_preserves_objective_hp_replay() {
    use crate::plugins::network_plugin::{scene_ops_from_stock, stock_scene_ops_from_message};
    for (stage, goal) in [
        ("sn04", "D_BPdoor"),
        ("sn11", "D_BPdoor01"),
        ("sn12", "D_BStove01"),
    ] {
        let mut n = net("p1");
        n.stage = stage.into();
        n.mode = "siege".into();
        n.address = "udp-host://127.0.0.1:5230".into();
        let (mut host, output) = app(n);
        host.insert_resource(StageScriptRuntime(Some(mode_runtime(stage, Some(3)))));
        let mut guest = mode_runtime(stage, Some(3));
        guest
            .call_item(
                "D_BBox01",
                "OnAttack",
                &[PstValue::Num(0.0), PstValue::Num(10000.0)],
            )
            .unwrap();
        let mut ops = guest
            .host_mut()
            .take_net_batches()
            .iter()
            .flatten()
            .map(scene_op_from_runtime)
            .collect::<Vec<_>>();
        let reference = guest.host().resolve_name(goal).unwrap();
        ops.push(meteor_network::protocol::SceneOp {
            kind: "set".into(),
            reference: JsonValue::from(reference as u64),
            field: "pose".into(),
            args: vec![JsonValue::from(1), JsonValue::from(0)],
        });
        let wire = stock_scene_ops_from_message(&ops, 100);
        host.world_mut()
            .resource_mut::<NetState>()
            .pending_scene_ops
            .extend(scene_ops_from_stock(&wire));
        host.update();
        assert_eq!(value(&host, "D_BBox01", "pose"), 1.0);
        assert_eq!(value(&host, "D_BBox01", "interactive"), 0.0);
        assert_eq!(
            value(&host, goal, "pose"),
            0.0,
            "objective remains eligible for host replay"
        );
        assert!(output().is_empty());
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn sn04_udp_mixed_attackers_must_not_restore_an_earlier_gate_damage_stage() {
    let network = |player: &str, host: bool| {
        let mut n = net(player);
        n.address = if host {
            "udp-host://127.0.0.1:5230"
        } else {
            "udp://127.0.0.1:5230"
        }
        .into();
        n.stage = "sn04".into();
        n.mode = "siege".into();
        n.match_epoch = None;
        n.round_epoch = None;
        n.stock_runtime_generation = Some(7);
        for (member, id) in n.members.iter_mut().zip(["stock-1", "stock-2"]) {
            member.player_id = id.into();
            member.team = Some(1);
        }
        n
    };
    let (mut host, host_output) = app(network("stock-1", true));
    let (mut guest, guest_output) = app(network("stock-2", false));
    for a in [&mut host, &mut guest] {
        a.insert_resource(StageScriptRuntime(Some(mode_runtime("sn04", Some(3)))));
        a.update();
    }
    host.world_mut()
        .resource_mut::<NetState>()
        .stock_scene_hit_peers
        .insert("stock-2".into());
    host.world_mut()
        .resource_mut::<StageScriptRuntime>()
        .script_mut()
        .unwrap()
        .call_item(
            "D_BPdoor",
            "OnAttack",
            &[PstValue::Num(0.0), PstValue::Num(2600.0)],
        )
        .unwrap();
    host.update();
    relay(&mut guest, host_output());
    assert_eq!(value(&guest, "D_BPdoor", "pose"), 2.0);
    for _ in 0..3 {
        for a in [&mut host, &mut guest] {
            a.world_mut()
                .resource_mut::<StageScriptRuntime>()
                .script_mut()
                .unwrap()
                .set_item_property("D_BPdoor", "state", PstValue::Num(3.0));
        }
        assert!(forward_network_prop_hit(
            guest.world().resource::<NetState>(),
            Some(guest.world().resource::<NetTx>()),
            "D_BPdoor",
            262,
            9770.0
        ));
        let mut messages = guest_output();
        assert_eq!(messages.len(), 1);
        let Message::ScenePropHit {
            reference,
            pose_id,
            frame,
            ..
        } = messages.pop().unwrap()
        else {
            panic!("reliable hit expected")
        };

        host.world_mut()
            .resource_mut::<NetState>()
            .pending_scene_prop_hits
            .push(Message::ScenePropHit {
                room_id: "room1".into(),
                match_epoch: 0,
                round_epoch: 0,
                player_id: "stock-2".into(),
                reference,
                pose_id,
                frame,
                damage: 1,
            });
        host.update();
        relay(&mut guest, host_output());
        assert_eq!(value(&host, "D_BPdoor", "pose"), 3.0);
        assert_eq!(value(&guest, "D_BPdoor", "pose"), 3.0);
        assert!(
            guest_output().is_empty(),
            "guest applies the owner's result without writing its obsolete HP"
        );
    }
    guest
        .world_mut()
        .resource_mut::<NetState>()
        .stock_runtime_generation = None;
    assert!(
        !forward_network_prop_hit(
            guest.world().resource::<NetState>(),
            None,
            "D_BPdoor",
            262,
            9770.0
        ),
        "a legacy/original host keeps its existing event-56 path"
    );
}
