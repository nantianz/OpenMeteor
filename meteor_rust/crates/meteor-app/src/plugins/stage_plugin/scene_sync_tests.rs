#![cfg(feature = "original-assets")]
use super::*;
use crate::plugins::combat_plugin::forward_network_prop_hit;
use crate::plugins::item_plugin::StageProp;
use crate::plugins::network_plugin::{control_test_channel, NetState, NetStatus, NetTx};
use meteor_network::protocol::{Message, RoomMemberWire};

fn runtime() -> SceneScriptRuntime {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let mut sources = Vec::new();
    for include in ["Def.h", "box.pst", "desk.pst", "chair.pst", "jug.pst"] {
        sources.push(crate::text_codec::decode_text(
            &std::fs::read(root.join("PScriptLib").join(include)).unwrap(),
        ));
    }
    sources.push(crate::text_codec::decode_text(
        &std::fs::read(root.join("sn01/sn01.pst")).unwrap(),
    ));
    let desc = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join("sn01/sn01.des")).unwrap(),
    );
    let instances = desc
        .dummies
        .iter()
        .enumerate()
        .map(|(index, dummy)| SceneItemInstance {
            name: dummy.name.clone(),
            index,
            props: [
                ("active".into(), PstValue::Num(f64::from(dummy.visible))),
                ("interactive".into(), PstValue::Num(1.0)),
                ("state".into(), PstValue::Num(3.0)),
                ("_has_fmc".into(), PstValue::Num(1.0)),
            ]
            .into_iter()
            .collect(),
        })
        .collect();
    let mut runtime = SceneScriptRuntime::load(&sources.join("\n"), instances, 250_000).unwrap();
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
        .insert_resource(StageScriptRuntime(Some(runtime())))
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
