use super::*;
use bevy::ecs::system::RunSystemOnce;
use meteor_scene::pst_game::{ActorAction, ScriptActor};
use meteor_scene::pst_runtime::PstValue;
use meteor_scene::pst_ui::PstUi;

#[test]
fn scripted_actor_and_temporary_item_requests_reach_the_live_stage() {
    let mut world = World::new();
    world.insert_resource(State::new(AppState::Playing));
    world.init_resource::<crate::plugins::network_plugin::NetState>();
    world.init_resource::<LaunchRequest>();
    world.init_resource::<StageMissionRuntime>();
    world.init_resource::<LoadedStageInfo>();
    let game = PstGame::default();
    game.begin_stage(true);
    world.insert_resource(PstSession(game.clone()));
    let player = world
        .spawn((
            LocalFighter,
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 1,
                weapon_variant: 0,
                weapon_slot: 0,
                weapon_slots: [1, 4],
                weapon_slot_variants: [0, 0],
            },
            Transform::IDENTITY,
            MotionState {
                movement_mode: MovementMode::Idle,
                velocity: Vec3::X,
                facing: 0.0,
            },
            Health {
                max: 100.0,
                current: 100.0,
            },
        ))
        .id();
    let mut scene = SceneScriptRuntime::load(
        r#"
        Setup() {
            CharSetPosition(0, Vector3(30, 20, 10));
            CharSetRotationYaw(0, 90);
            CharEquipWeapon(0, 6);
            SetTeam(0, 2);
            CharSetBaseAttrib(0, 0, 400);
            CharCreateTempItem(0, 2, 2, Vector3(30, 20, 10));
        }
    "#,
        vec![],
        2000,
    )
    .unwrap();
    scene.host_mut().enable_dynamic_items();
    scene.set_game(game);
    world.insert_resource(StageScriptRuntime::from_script(scene));
    world.insert_resource(ScriptItemModels(Some(BTreeMap::from([(
        [2, 2],
        "Item02".into(),
    )]))));

    update_script_actors(&mut world);
    world
        .resource_mut::<StageScriptRuntime>()
        .script_mut()
        .unwrap()
        .call("Setup", &[])
        .unwrap();
    apply_game_requests(&mut world);

    assert_eq!(world.get::<Fighter>(player).unwrap().weapon_slots, [6, 4]);
    assert_eq!(world.get::<Health>(player).unwrap().max, 400.0);
    assert_eq!(
        world.get::<MotionState>(player).unwrap().velocity,
        Vec3::ZERO
    );
    assert_eq!(
        world.get::<MotionState>(player).unwrap().facing,
        std::f32::consts::FRAC_PI_2
    );
    assert_eq!(
        world.get::<Transform>(player).unwrap().translation,
        source_to_world([30.0, 20.0, 10.0], [0.0; 3])
    );
    let net = world.resource::<crate::plugins::network_plugin::NetState>();
    assert_eq!(combat_plugin::fighter_combat_team(net, true, None, None), 2);
    let spawns = world
        .resource_mut::<StageScriptRuntime>()
        .script_mut()
        .unwrap()
        .host_mut()
        .take_item_spawns();
    assert_eq!(spawns.len(), 1);
    assert_eq!(spawns[0].model, "Item02");
    assert_eq!(spawns[0].position, Some([30.0, 20.0, 10.0]));
    assert_eq!(spawns[0].temporary, Some(([2, 2], None)));
}

#[test]
fn native_stage_handoff_preserves_the_script_panel_and_music_channel() {
    let mut world = World::new();
    world.init_resource::<NextState<AppState>>();
    let music = PstMusicRequests::new(|path| (path == "music/s8.mp3").then(|| vec![1]));
    let ui = PstUi::default();
    let mut scene = SceneScriptRuntime::load(
        r#"Scene_OnInit() {
            PlayMusic("music/s8.mp3");
            UiCreatePanel("arrival", "Arrival");
            UiAddButton("arrival", "ok", "OK");
            UiShowPanel("arrival");
        }"#,
        vec![],
        2000,
    )
    .unwrap();
    scene.host_mut().set_ui(ui.clone());
    scene.host_mut().set_music_requests(music.clone());
    let stage = PreparedStage {
        characters: None,
        common: None,
        info: LoadedStageInfo {
            ready: true,
            ..default()
        },
        assets: default(),
        script: StageScriptRuntime::from_script(scene),
        mission: default(),
        script_music: Some(StageScriptMusic(music)),
        script_ui: pst_ui::StagePstUi(Some(ui)),
        waypoints: default(),
        escort: default(),
        graphics: None,
    };
    let pool = bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
    world.insert_resource(PendingStageLoad(pool.spawn(async move { stage })));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while world.contains_resource::<PendingStageLoad>() {
        assert!(std::time::Instant::now() < deadline);
        world.run_system_once(poll_stage_load).unwrap();
        std::thread::yield_now();
    }

    world
        .resource_mut::<StageScriptRuntime>()
        .script_mut()
        .unwrap()
        .start()
        .unwrap();
    assert_eq!(
        world.resource::<StageScriptMusic>().0.take().unwrap().path,
        "music/s8.mp3"
    );
    assert_eq!(
        world
            .resource::<pst_ui::StagePstUi>()
            .0
            .as_ref()
            .unwrap()
            .visible()
            .unwrap()
            .id,
        "arrival"
    );
}

#[test]
fn entertainment_menu_drives_shortcuts_paging_save_restore_and_mission_calls() {
    use std::sync::Mutex;
    static SAVED: Mutex<Option<String>> = Mutex::new(None);
    let game = PstGame::new(ScriptStorage {
        read_asset: |_| None,
        read: |key| {
            assert_eq!(key, "demo_scene");
            SAVED.lock().unwrap().clone()
        },
        write: |key, text| {
            assert_eq!(key, "demo_scene");
            *SAVED.lock().unwrap() = Some(text.to_string());
            true
        },
    });
    game.begin_stage(true);
    game.set_actors(BTreeMap::from([(
        0,
        ScriptActor {
            position: [5.0, 6.0, 7.0],
            alive: true,
            ..default()
        },
    )]));
    let ui = PstUi::default();
    let mut startup = StartupScripts::default();
    startup
        .add(
            "menu.pst",
            include_str!("../../../../../../../examples/pst/entertainment-menu/menu.pst"),
            game.clone(),
            ui.clone(),
            PstMusicRequests::new(|_| None),
        )
        .unwrap();
    let mut scene = SceneScriptRuntime::load("", vec![], 2000).unwrap();
    let source = include_str!("../../../../../../../examples/pst/entertainment-menu/mission.pst");
    let mut mission = MissionScriptRuntime::new(source, default());
    let guard = include_str!("../../../../../../../examples/pst/key-door-guard/guard.pst");
    mission
        .register_npc_script("pst_demo_guard", guard)
        .unwrap();
    mission.set_game(game.clone());
    let mut world = World::new();
    world.insert_resource(PstSession(game.clone()));
    world.init_resource::<Time>();
    let mut keys = ButtonInput::<KeyCode>::default();
    keys.press(KeyCode::KeyJ);
    world.insert_resource(keys);
    world.run_system_once(sample_input).unwrap();
    let press = |startup: &mut StartupScripts,
                 scene: &mut SceneScriptRuntime,
                 mission: &mut MissionScriptRuntime,
                 button: &str| {
        startup
            .dispatch(
                Some(0),
                &[ScriptEvent {
                    name: "OnUiButton".into(),
                    args: vec![PstValue::from("entertainment"), PstValue::from(button)],
                }],
                Some(scene),
                Some(mission),
            )
            .unwrap();
    };
    let update = [ScriptEvent {
        name: "OnUpdate".into(),
        args: vec![],
    }];
    startup
        .dispatch(None, &update, Some(&mut scene), Some(&mut mission))
        .unwrap();
    let first = ui.visible().unwrap();
    assert!(first.buttons.iter().any(|button| button.id == "save"));
    assert_eq!(first.images[0].path, "Image/PstUi/menu.png");
    press(&mut startup, &mut scene, &mut mission, "save");
    assert_eq!(SAVED.lock().unwrap().as_deref(), Some("[5.0,6.0,7.0]"));
    press(&mut startup, &mut scene, &mut mission, "load");
    assert!(game.take_requests().contains(&GameRequest::Actor(
        0,
        ActorAction::Position([5.0, 6.0, 7.0])
    )));
    press(&mut startup, &mut scene, &mut mission, "actors");
    assert!(ui
        .visible()
        .unwrap()
        .buttons
        .iter()
        .any(|button| button.id == "npc"));
    press(&mut startup, &mut scene, &mut mission, "weapon");
    assert_eq!(
        game.take_requests(),
        [GameRequest::Actor(0, ActorAction::Equip(6))]
    );
    press(&mut startup, &mut scene, &mut mission, "npc");
    let requests = mission.host_mut().take_npc_requests();
    let [NpcRequest::Add { id, .. }] = requests.as_slice() else {
        panic!("menu must enqueue the mission NPC");
    };
    let guard = mission
        .initialized_npc_definition(*id, "pst_demo_guard")
        .unwrap();
    assert_eq!(guard.name, "练习守卫");
    assert_eq!(guard.hp, 600);
    press(&mut startup, &mut scene, &mut mission, "items");
    assert!(ui
        .visible()
        .unwrap()
        .buttons
        .iter()
        .any(|button| button.id == "item"));
    press(&mut startup, &mut scene, &mut mission, "item");
    assert!(matches!(
        game.take_requests().as_slice(),
        [GameRequest::TempItem {
            actor: 0,
            resource: [2, 2],
            thrown: true,
            ..
        }]
    ));
}
