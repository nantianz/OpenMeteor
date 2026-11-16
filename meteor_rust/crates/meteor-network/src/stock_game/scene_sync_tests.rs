#[test]
fn sn04_real_host_gate_transactions_relay_and_bootstrap_final_state() {
    use meteor_scene::scene_script_runtime::{
        PstValue, SceneItemInstance, SceneNetOp, SceneScriptRuntime,
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let desc = meteor_asset::des::parse_stage_description(
        &fs::read_to_string(root.join("sn04/sn04.des")).unwrap(),
    );
    let mut source = String::new();
    for path in [
        "PScriptLib/Def.h",
        "PScriptLib/box.pst",
        "PScriptLib/desk.pst",
        "PScriptLib/chair.pst",
        "PScriptLib/jug.pst",
        "sn04/sn04.pst",
    ] {
        source.push_str(
            &encoding_rs::GB18030
                .decode(&fs::read(root.join(path)).unwrap())
                .0,
        );
        source.push('\n');
    }
    let new_scene = || {
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, d)| SceneItemInstance {
                name: d.name.clone(),
                index,
                props: [
                    ("active".into(), PstValue::Num(1.0)),
                    ("interactive".into(), PstValue::Num(1.0)),
                    ("visible".into(), PstValue::Num(f64::from(d.visible))),
                    ("_has_fmc".into(), PstValue::Num(1.0)),
                    ("state".into(), PstValue::Num(3.0)),
                ]
                .into_iter()
                .collect(),
            })
            .collect();
        let mut scene = SceneScriptRuntime::load(&source, instances, 250_000).unwrap();
        scene.host_mut().set_game_mode(3);
        scene.host_mut().set_character_team(0, 1);
        scene.start().unwrap();
        scene.host_mut().take_net_batches();
        scene
    };
    let mut host_scene = new_scene();
    let mut guest_scene = new_scene();
    let transport =
        StockServerTransport::bind("127.0.0.1:0", "serverlist", "sn04 scene sync").unwrap();
    let address = transport.local_addr().unwrap();
    let checksums = [1234; STOCK_HERO_COUNT];
    let mut server = StockGameServer::with_rule_and_combat_resources(
        transport,
        StockHeroCatalog::from_checksums(checksums),
        3,
        CombatResourceTable::default(),
    );
    server.set_stage_id("sn04");
    let new_client = |camp| {
        StockGameClient::new(
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap(),
            StockEnterRoleRequest {
                hero: 0,
                camp,
                hero_checksum: checksums[0],
            },
            6,
        )
    };
    let mut host = new_client(1);
    drive(&mut server, &mut [&mut host], 0, 100);
    let mut guest = new_client(2);
    drive(&mut server, &mut [&mut host, &mut guest], 100, 100);
    let ref_index = host_scene.host().resolve_name("D_BPdoor").unwrap();
    assert!(ref_index > 0);
    let apply_event = |scene: &mut SceneScriptRuntime, event: &StockSceneOpsEvent| {
        use crate::stock_protocol::*;
        for op in &event.operations {
            let n = |n| PstValue::Num(n as f64);
            let (field, args) = match op.kind {
                STOCK_SCENE_OP_POSE => ("pose", vec![n(op.value), n(op.extra)]),
                STOCK_SCENE_OP_COLLISION => (
                    "attribute",
                    vec![PstValue::Str("collision".into()), n(op.value)],
                ),
                STOCK_SCENE_OP_INTERACTIVE => (
                    "attribute",
                    vec![PstValue::Str("interactive".into()), n(op.value)],
                ),
                STOCK_SCENE_OP_EFFECT => {
                    scene.host_mut().apply_net_op(&SceneNetOp::Effect {
                        ref_index: op.reference as usize,
                        effect: stock_rule3_effect_name(op.value).unwrap().into(),
                        extra: op.extra as f64,
                    });
                    continue;
                }
                _ => panic!("unexpected gate operation: {op:?}"),
            };
            scene.host_mut().apply_net_op(&SceneNetOp::Set {
                ref_index: op.reference as usize,
                field: field.into(),
                args,
            });
        }
    };
    for (index, (damage, expected)) in [(2600, 2), (2500, 4), (2500, 6), (2501, 8)]
        .into_iter()
        .enumerate()
    {
        host_scene
            .call_item(
                "D_BPdoor",
                "OnAttack",
                &[PstValue::Num(0.0), PstValue::Num(damage as f64)],
            )
            .unwrap();
        for batch in host_scene.host_mut().take_net_batches() {
            use crate::stock_protocol::*;
            let operations = batch
                .into_iter()
                .map(|op| match op {
                    SceneNetOp::Set {
                        ref_index,
                        field,
                        args,
                    } => {
                        let (kind, value, extra) = match field.as_str() {
                            "pose" => (
                                STOCK_SCENE_OP_POSE,
                                args[0].as_f64() as u32,
                                args[1].as_f64() as u32,
                            ),
                            "attribute" => {
                                let kind = match &args[0] {
                                    PstValue::Str(s) if s == "collision" => {
                                        STOCK_SCENE_OP_COLLISION
                                    }
                                    PstValue::Str(s) if s == "interactive" => {
                                        STOCK_SCENE_OP_INTERACTIVE
                                    }
                                    _ => panic!("unexpected attribute"),
                                };
                                (kind, args[1].as_f64() as u32, 0)
                            }
                            _ => panic!("unexpected field"),
                        };
                        StockSceneOperation {
                            reference: ref_index as u32,
                            kind,
                            value,
                            extra,
                        }
                    }
                    SceneNetOp::Effect {
                        ref_index,
                        effect,
                        extra,
                    } => StockSceneOperation {
                        reference: ref_index as u32,
                        kind: STOCK_SCENE_OP_EFFECT,
                        value: stock_rule3_effect_id(&effect).unwrap(),
                        extra: extra as u32,
                    },
                })
                .collect();
            host.send_scene_ops(
                &StockSceneOpsEvent {
                    timestamp_ms: 200 + index as u32 * 100,
                    operations,
                },
                200 + index as u64 * 100,
            )
            .unwrap();
        }
        let received = drive(
            &mut server,
            &mut [&mut host, &mut guest],
            200 + index as u64 * 100,
            100,
        );
        let mut count = 0;
        for event in &received[1] {
            if let StockGameEvent::SceneOps(event) = event {
                apply_event(&mut guest_scene, event);
                count += 1;
            }
        }
        assert!(count > 0, "each damage stage reaches the guest over UDP");
        assert_eq!(
            guest_scene
                .host()
                .instance(ref_index)
                .unwrap()
                .get("pose")
                .as_f64(),
            expected as f64
        );
        assert!(guest_scene.host_mut().take_net_batches().is_empty());
    }
    assert_eq!(
        guest_scene
            .host()
            .instance(ref_index)
            .unwrap()
            .get("interactive")
            .as_f64(),
        0.0
    );
    assert!(guest_scene
        .host_mut()
        .take_effects()
        .iter()
        .any(|op| matches!(op, SceneNetOp::Effect { effect, .. } if effect == "GiMaBRK")));
    let mut late = new_client(2);
    let received = drive(
        &mut server,
        &mut [&mut host, &mut guest, &mut late],
        600,
        100,
    );
    let retained = received[2]
        .iter()
        .filter_map(|event| match event {
            StockGameEvent::SceneOps(event) => Some(event),
            _ => None,
        })
        .flat_map(|event| &event.operations)
        .collect::<Vec<_>>();
    assert!(retained.iter().any(|op| op.reference == ref_index as u32
        && op.kind == crate::stock_protocol::STOCK_SCENE_OP_POSE
        && op.value == 8));
    assert!(!retained
        .iter()
        .any(|op| op.kind == crate::stock_protocol::STOCK_SCENE_OP_EFFECT));
}
