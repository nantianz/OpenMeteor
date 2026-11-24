use super::*;

#[test]
fn mission_npc_uses_script_model_above_stock_range_and_reports_missing_assets() {
    use crate::plugins::character_plugin::CharacterAssetsSet;
    let Some(root) = std::env::var_os("METEOR_TEST_ORIGINAL_DIR") else {
        return;
    };
    let root = Path::new(&root).join("pmodel");
    let mut blob = Vec::new();
    let entries: Vec<_> = [
        ("p0.skc", "p20.skc"), ("p0_300.skc", "p20_300.skc"),
        ("P0.bnc", "p20.bnc"), ("P0.POS", "p20.pos"), ("p0.amb", "p20.amb"),
    ].into_iter().map(|(source, target)| {
        let bytes = std::fs::read(root.join(source)).unwrap();
        let entry = serde_json::json!({"name": format!("pmodel/{target}"), "off": blob.len(), "len": bytes.len()});
        blob.extend(bytes);
        entry
    }).collect();
    let manifest = serde_json::to_vec(&serde_json::json!({
        "formatVersion": 1, "kind": "test", "packId": "test/mission-p20", "entries": entries,
    }))
    .unwrap();
    let mut pack = b"MPK1".to_vec();
    pack.extend((manifest.len() as u32).to_le_bytes());
    pack.extend(manifest);
    pack.extend(blob);
    meteor_asset::runtime_assets::register_mpk1(&pack).unwrap();

    for model in [20, 254] {
        let mut runtime = MissionScriptRuntime::new(r#"OnStart(){ AddNPC("custom"); }"#, default());
        runtime
            .register_npc_script(
                "custom",
                &format!("int Model = 0; OnInit(){{ Model = {model}; }}"),
            )
            .unwrap();
        runtime.start().unwrap();
        let mut app = App::new();
        app.insert_resource(StageMissionRuntime::from_script(runtime))
            .insert_resource(LaunchRequest {
                stage_id: "sn24".into(),
                ..default()
            })
            .init_resource::<PlayerPerformance>()
            .init_resource::<PendingMissionNpcCommands>()
            .init_resource::<CharacterAssetsSet>()
            .init_resource::<LoadedStageInfo>()
            .init_resource::<NextState<AppState>>()
            .add_systems(Update, consume_mission_npc_requests);
        app.update();
        let world = app.world_mut();
        let mut query = world.query_filtered::<&Fighter, With<MissionNpc>>();
        if model == 20 {
            assert_eq!(query.single(world).unwrap().character_id, 20);
            assert!(world.resource::<CharacterAssetsSet>().0.contains_key("p20"));
        } else {
            assert_eq!(query.iter(world).count(), 0);
            let error = world
                .resource::<LoadedStageInfo>()
                .error
                .as_deref()
                .unwrap();
            assert!(
                error.contains("sn24")
                    && error.contains("custom")
                    && error.contains("pmodel/p254.skc"),
                "{error}"
            );
            assert!(matches!(
                world.resource::<NextState<AppState>>(),
                NextState::Pending(AppState::Error)
            ));
        }
    }
}

#[test]
fn hard_behavior_clears_old_perform_and_preserves_new_commands_while_spawn_is_pending() {
    let source = r#"OnStart(){ AddNPC("a"); Perform(1,"say","old");
        ChangeBehavior(1,"wait"); Perform(1,"say","new"); }"#;
    let mut runtime = MissionScriptRuntime::new(source, default());
    runtime
        .register_npc_script("a", "string Name=\"A\";")
        .unwrap();
    runtime.start().unwrap();
    runtime.host_mut().take_npc_requests();
    let mut app = App::new();
    app.insert_resource(StageMissionRuntime::from_script(runtime))
        .insert_resource(LaunchRequest::default())
        .init_resource::<PlayerPerformance>()
        .insert_resource(PendingMissionNpcCommands {
            performs: vec![(1, "say".into(), vec![PstValue::Str("older pending".into())])],
            ..default()
        })
        .add_systems(Update, consume_mission_npc_requests);
    app.update();
    let pending = app.world().resource::<PendingMissionNpcCommands>();
    assert_eq!(pending.behaviors.len(), 1);
    assert_eq!(
        pending.performs,
        vec![(1, "say".into(), vec![PstValue::Str("new".into())])]
    );

    let actor = app
        .world_mut()
        .spawn((
            MissionNpc {
                id: 1,
                name: "A".into(),
                team: 2,
                in_scene_roster: true,
                view: 400.0,
                think: 100.0,
                forced_behavior: None,
                is_vip: false,
                attack_target_slot: None,
                behavior_args: Vec::new(),
                attack_target_remaining: 0,
            },
            crate::components::fighter::AiIntent::default(),
            NpcPerformance {
                pause_remaining: 10.0,
                ..default()
            },
            crate::plugins::ai_plugin::NpcPatrol::default(),
        ))
        .id();
    app.update();
    assert_eq!(
        app.world()
            .get::<MissionNpc>(actor)
            .unwrap()
            .forced_behavior,
        Some(meteor_ai::behavior::BehaviorMode::Wait)
    );
    let performance = app.world().get::<NpcPerformance>(actor).unwrap();
    assert_eq!(performance.pause_remaining, 0.0);
    assert_eq!(performance.queue.len(), 1);
    assert_eq!(performance.queue[0].args, vec![PstValue::Str("new".into())]);
    assert!(!app
        .world()
        .resource::<StageMissionRuntime>()
        .script()
        .unwrap()
        .npc_init_pending(1));
}

#[test]
fn delayed_onstart_install_preserves_callback_tail_but_onupdate_does_not_increment() {
    let source = "OnStart(){ AddNPC(\"a\"); }";
    let mut runtime = MissionScriptRuntime::new(source, default());
    runtime
        .register_npc_script(
            "a",
            r#"
        OnStart(){ ChangeBehavior(1,"wait"); }
        OnUpdate(){ ChangeBehavior(1,"idle"); }
    "#,
        )
        .unwrap();
    runtime.start().unwrap();
    runtime.host_mut().take_npc_requests();
    runtime.run_npc_init(1).unwrap();
    let mut app = App::new();
    app.insert_resource(StageMissionRuntime::from_script(runtime))
        .insert_resource(LaunchRequest::default())
        .init_resource::<PlayerPerformance>()
        .init_resource::<PendingMissionNpcCommands>()
        .add_systems(Update, consume_mission_npc_requests);
    for _ in 0..2 {
        app.update();
        let pending = app.world().resource::<PendingMissionNpcCommands>();
        assert_eq!(pending.behaviors.len(), 1);
        assert!(pending.behaviors[0].4);
    }
    let actor = app
        .world_mut()
        .spawn((
            MissionNpc {
                id: 1,
                name: "A".into(),
                team: 2,
                in_scene_roster: true,
                view: 400.0,
                think: 100.0,
                forced_behavior: None,
                is_vip: false,
                attack_target_slot: None,
                behavior_args: Vec::new(),
                attack_target_remaining: 0,
            },
            crate::components::fighter::AiIntent::default(),
            NpcPerformance::default(),
            crate::plugins::ai_plugin::NpcPatrol::default(),
        ))
        .id();
    app.update();
    let patrol = app
        .world()
        .get::<crate::plugins::ai_plugin::NpcPatrol>(actor)
        .unwrap();
    assert_eq!(patrol.callback_ticks_for_test(), (0, 0));
    assert_eq!(
        app.world()
            .get::<MissionNpc>(actor)
            .unwrap()
            .forced_behavior,
        Some(meteor_ai::behavior::BehaviorMode::Wait)
    );
    app.world_mut()
        .resource_mut::<StageMissionRuntime>()
        .script_mut()
        .unwrap()
        .update_npc_script(1)
        .unwrap();
    app.update();
    let patrol = app
        .world()
        .get::<crate::plugins::ai_plugin::NpcPatrol>(actor)
        .unwrap();
    assert_eq!(patrol.callback_ticks_for_test(), (-1, -1));
    assert_eq!(
        app.world()
            .get::<MissionNpc>(actor)
            .unwrap()
            .forced_behavior,
        Some(meteor_ai::behavior::BehaviorMode::Idle)
    );
}
