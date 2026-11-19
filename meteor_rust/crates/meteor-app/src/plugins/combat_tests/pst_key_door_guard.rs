#[cfg(not(target_arch = "wasm32"))]
#[test]
fn pst_key_door_guard_demo_runs_through_production_systems() {
    run_pst_key_door_guard_demo();
}

#[cfg(not(target_arch = "wasm32"))]
pub(in crate::plugins) struct PstKeyDoorGuardUi {
    pub dialogue: crate::plugins::ui_plugin::DialogueState,
    pub player: PstHudActor,
    pub panel: meteor_scene::pst_ui::UiPanel,
    pub panel_player: PstHudActor,
    pub door_dialogue: crate::plugins::ui_plugin::DialogueState,
    pub door_player: PstHudActor,
    pub request: crate::app_state::LaunchRequest,
    pub result: MatchEnd,
    pub next: NextState<AppState>,
    pub kills: crate::components::fighter::KillScore,
    pub scores: crate::plugins::battle_result::OfflineScores,
}

#[cfg(not(target_arch = "wasm32"))]
type PstHudActor = (Fighter, Health, Qi, MotionState);

#[cfg(not(target_arch = "wasm32"))]
fn capture_pst_hud_actor(world: &World, player: Entity) -> PstHudActor {
    (
        world.get::<Fighter>(player).unwrap().clone(),
        world.get::<Health>(player).unwrap().clone(),
        world.get::<Qi>(player).unwrap().clone(),
        world.get::<MotionState>(player).unwrap().clone(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
pub(in crate::plugins) fn run_pst_key_door_guard_demo() -> PstKeyDoorGuardUi {
    use crate::plugins::audio_plugin::{play_script_music, BgmEntity};
    use crate::plugins::item_plugin::{item_pickup, StageProp};
    use crate::plugins::pst_ui::{
        dispatch_button, sync_modal_input, sync_panel, PstPanelButton, StagePstUi,
    };
    use crate::plugins::render_plugin::{
        spawn_pst_scene_items, PropColliderOwner, ScriptSpawnedPickup,
    };
    use crate::plugins::stage_plugin::{
        consume_mission_npc_requests, consume_mission_say_requests, consume_pst_actor_requests,
        scene_item_props_from_dummy, tick_stage_script, MissionNpc, PendingMissionNpcCommands,
        StageMissionRuntime, StageScriptMusic, StageScriptRuntime,
    };
    use crate::plugins::ui_plugin::DialogueState;
    use bevy::ecs::system::RunSystemOnce;
    use meteor_physics::collision::MeshCollider;
    use meteor_scene::mission_runtime::{parse_mission_definition, MissionScriptRuntime};
    use meteor_scene::pst_music::PstMusicRequests;
    use meteor_scene::scene_script_runtime::{
        SceneItemCatalog, SceneItemInstance, SceneScriptRuntime,
    };

    const SCENE: &str = include_str!("../../../../../../examples/pst/key-door-guard/scene.pst");
    const MISSION: &str = include_str!("../../../../../../examples/pst/key-door-guard/mission.pst");
    const GUARD: &str = include_str!("../../../../../../examples/pst/key-door-guard/guard.pst");
    const PROPS: &str = include_str!("../../../../../../examples/pst/key-door-guard/props.des");

    let (mut harness, previous_target, _) = spawn_local_stock_hit_harness(5);
    let player = harness.fighter;
    harness.app.world_mut().despawn(previous_target);
    let descriptor = meteor_asset::des::parse_stage_description(PROPS);
    assert_eq!(descriptor.dummies.len(), 2);
    let instances = descriptor
        .dummies
        .iter()
        .enumerate()
        .map(|(index, dummy)| SceneItemInstance {
            name: dummy.name.clone(),
            index,
            props: scene_item_props_from_dummy(dummy),
        })
        .collect();
    let music = PstMusicRequests::new(|path| {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../origin_res")
                .join(path),
        )
        .ok()
    });
    let mut scene = SceneScriptRuntime::load(SCENE, instances, 250_000).unwrap();
    let ui = meteor_scene::pst_ui::PstUi::default();
    scene.set_script_name("sn01/sn01.pst");
    scene.host_mut().set_ui(ui.clone());
    scene.host_mut().set_music_requests(music.clone());
    scene.host_mut().enable_dynamic_items();
    let definition = parse_mission_definition(MISSION);
    assert_eq!(definition.npc_scripts, ["pst_demo_guard"]);
    let catalog = SceneItemCatalog::new(
        descriptor
            .dummies
            .iter()
            .map(|dummy| Some(dummy.name.clone())),
    );
    let mut mission = MissionScriptRuntime::with_scene_items(MISSION, definition.values, catalog);
    mission.set_script_name("Level/sn01_.pst");
    mission.host_mut().set_ui(ui.clone());
    mission.host_mut().set_music_requests(music.clone());
    mission
        .register_npc_script(&definition.npc_scripts[0], GUARD)
        .unwrap();
    scene.start_linked(Some(&mut mission)).unwrap();
    mission.start_linked(Some(&mut scene)).unwrap();

    let mut request = crate::app_state::LaunchRequest::default();
    request.mode = "single".into();
    request.stage_id = "sn01".into();
    request.stage_name = crate::plugins::campaign_ui::mission("sn01")
        .unwrap()
        .name
        .into();
    harness
        .app
        .insert_resource(NetState::default())
        .insert_resource(request)
        .insert_resource(StageScriptRuntime::from_script(scene))
        .insert_resource(StageMissionRuntime::from_script(mission))
        .insert_resource(StageScriptMusic(music))
        .insert_resource(StagePstUi(Some(ui.clone())))
        .insert_resource(crate::plugins::launcher_ui::UiFont(Handle::default()))
        .insert_resource(crate::app_state::LoadedStageInfo {
            stage_id: "sn01".into(),
            ..default()
        })
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<bevy::image::Image>>()
        .init_resource::<Assets<crate::plugins::stage_material::GmdlMaterial>>()
        .init_resource::<BgmEntity>()
        .init_resource::<MatchEnd>()
        .init_resource::<NextState<AppState>>()
        .insert_resource(State::new(AppState::Playing))
        .init_resource::<DialogueState>()
        .init_resource::<crate::plugins::battle_result::OfflineScores>()
        .init_resource::<PendingMissionNpcCommands>()
        .init_resource::<crate::plugins::stage_plugin::PlayerPerformance>()
        .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
        .add_systems(Update, evaluate_match_end);

    let world = harness.app.world_mut();
    let owner = world
        .resource_mut::<StageCollider>()
        .mesh
        .as_mut()
        .unwrap()
        .append_world_triangles(
            &[
                [-4.0, 0.0, 14.0],
                [4.0, 0.0, 14.0],
                [4.0, 6.0, 14.0],
                [-4.0, 6.0, 14.0],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
    let prop = |index: usize, collision_enabled| StageProp {
        name: descriptor.dummies[index].name.clone(),
        model: descriptor.dummies[index].model.clone().unwrap(),
        center_offset: Vec3::ZERO,
        half_extents: Vec3::ONE,
        hit_count: 0,
        respawn_timer: 0.0,
        interactive: true,
        collision_enabled,
        breakable: false,
        broken: false,
    };
    let key = world
        .spawn((
            prop(0, false),
            Transform::from_xyz(0.0, 0.0, 8.0),
            Visibility::Visible,
        ))
        .id();
    let door = world
        .spawn((
            prop(1, true),
            Transform::from_xyz(0.0, 0.0, 14.0),
            Visibility::Visible,
            PropColliderOwner(owner),
        ))
        .id();
    assert!(world
        .resource::<StageCollider>()
        .mesh
        .as_ref()
        .unwrap()
        .box_blocked(0.0, 0.0, 14.0));
    world.run_system_once(item_pickup).unwrap();
    world.run_system_once(consume_mission_npc_requests).unwrap();
    assert_eq!(
        world
            .query::<&crate::plugins::stage_plugin::MissionNpc>()
            .iter(world)
            .count(),
        0
    );
    world.get_mut::<Transform>(player).unwrap().translation = Vec3::new(0.0, 0.0, 8.0);
    world.run_system_once(item_pickup).unwrap();
    world.run_system_once(item_pickup).unwrap();
    world.run_system_once(tick_stage_script).unwrap();
    world.run_system_once(consume_mission_npc_requests).unwrap();
    assert_eq!(world.get::<Visibility>(door), Some(&Visibility::Visible));
    assert!(world
        .resource::<StageCollider>()
        .mesh
        .as_ref()
        .unwrap()
        .box_blocked(0.0, 0.0, 14.0));
    assert_eq!(world.query::<&MissionNpc>().iter(world).count(), 0);
    let panel = ui.visible().expect("the key pickup opens its panel");
    assert_eq!(panel.id, "key_door");
    let panel_player = capture_pst_hud_actor(world, player);
    world.run_system_once(sync_modal_input).unwrap();
    assert!(world
        .resource::<crate::plugins::stage_plugin::PlayerPerformance>()
        .blocks_input());
    world.run_system_once(sync_panel).unwrap();
    let button = world
        .query_filtered::<Entity, (With<Button>, With<PstPanelButton>)>()
        .single(world)
        .unwrap();
    world.run_system_once(dispatch_button).unwrap();
    assert!(
        ui.visible().is_some(),
        "rendering a button must not activate it"
    );
    *world.get_mut::<Interaction>(button).unwrap() =
        crate::plugins::ui_plugin::press_pst_panel_button(panel.clone());
    world.run_system_once(dispatch_button).unwrap();
    assert!(ui.visible().is_none());
    world.run_system_once(sync_panel).unwrap();
    world.run_system_once(sync_modal_input).unwrap();
    assert!(!world
        .resource::<crate::plugins::stage_plugin::PlayerPerformance>()
        .blocks_input());
    assert!(world.get_entity(button).is_err());
    world.run_system_once(tick_stage_script).unwrap();
    world.run_system_once(consume_mission_npc_requests).unwrap();
    world.run_system_once(spawn_pst_scene_items).unwrap();
    world.run_system_once(play_script_music).unwrap();
    world.run_system_once(consume_mission_say_requests).unwrap();
    assert!(world
        .resource::<DialogueState>()
        .text
        .as_deref()
        .is_some_and(|text| {
            text.ends_with("门开了。捡起脚边的剑，迎战守卫！")
        }));
    let door_dialogue = std::mem::take(&mut *world.resource_mut::<DialogueState>());
    let door_player = capture_pst_hud_actor(world, player);
    assert_eq!(world.get::<StageProp>(key).unwrap().hit_count, 1);
    assert_eq!(world.get::<StageProp>(key).unwrap().respawn_timer, 0.0);
    assert_eq!(world.get::<Visibility>(door), Some(&Visibility::Hidden));
    assert!(!world
        .resource::<StageCollider>()
        .mesh
        .as_ref()
        .unwrap()
        .box_blocked(0.0, 0.0, 14.0));
    let bgm = world.query::<&AudioPlayer>().single(world).unwrap();
    let bytes = &world
        .resource::<Assets<AudioSource>>()
        .get(&bgm.0)
        .unwrap()
        .bytes;
    let original = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/music/s8.mp3"),
    )
    .unwrap();
    assert_eq!(&**bytes, original.as_slice());
    let sword = world
        .query_filtered::<Entity, With<ScriptSpawnedPickup>>()
        .single(world)
        .unwrap();
    let sword_prop = world.get::<StageProp>(sword).unwrap();
    assert_eq!(sword_prop.name, "D_DemoSword");
    assert_eq!(sword_prop.model, "w0_0");
    assert!(!sword_prop.collision_enabled);
    assert!(world.get::<Children>(sword).is_some());
    assert_eq!(
        world
            .resource::<StageScriptRuntime>()
            .script()
            .unwrap()
            .host()
            .resolve_name("D_DemoSword"),
        Some(2),
    );
    world.run_system_once(item_pickup).unwrap();
    world.run_system_once(item_pickup).unwrap();
    world.run_system_once(consume_pst_actor_requests).unwrap();
    assert_eq!(world.get::<Visibility>(sword), Some(&Visibility::Hidden));
    let sword_prop = world.get::<StageProp>(sword).unwrap();
    assert_eq!(sword_prop.hit_count, 1);
    assert_eq!(sword_prop.respawn_timer, 0.0);
    let fighter = world.get::<Fighter>(player).unwrap();
    assert_eq!(fighter.weapon_slots, [5, 6]);
    assert_eq!(fighter.weapon, 5);
    assert!(world.get::<ScriptWeaponSwitch>(player).is_some());

    let guard = world
        .query_filtered::<Entity, With<MissionNpc>>()
        .single(world)
        .unwrap();
    assert_eq!(world.get::<Health>(guard).unwrap().current, 250.0);
    let guard_spawn = world.get::<DeathRespawn>(guard).unwrap().spawn_pos;
    crate::plugins::character_plugin::spawn_fighter_model_for_test(world, guard);
    world.run_system_once(evaluate_match_end).unwrap();
    assert!(
        !world.resource::<MatchEnd>().settled,
        "Rule 1 must wait for the task"
    );
    harness.step();
    assert!(harness
        .app
        .world()
        .get::<WeaponSwitchActive>(player)
        .is_some());
    harness.steps(150);
    assert_eq!(
        harness.app.world().get::<Fighter>(player).unwrap().weapon,
        6
    );
    assert!(harness
        .app
        .world()
        .get::<WeaponSwitchActive>(player)
        .is_none());

    kill_pst_demo_guard(&mut harness, guard);
    let world = harness.app.world_mut();
    world.run_system_once(consume_pst_actor_requests).unwrap();
    assert!(world.get::<ScriptReborn>(guard).is_some());
    world.run_system_once(scripted_respawn).unwrap();
    assert!(!world.get::<DeathRespawn>(guard).unwrap().active);
    assert_eq!(world.get::<Health>(guard).unwrap().current, 250.0);
    assert_eq!(
        world.get::<Transform>(guard).unwrap().translation,
        guard_spawn
    );
    assert_eq!(
        world.get::<SmState>(guard).unwrap().current,
        FighterState::Name
    );
    assert_eq!(
        world
            .get::<crate::components::fighter::TargetHitClock>(guard)
            .unwrap()
            .spawn_elapsed,
        0.0
    );
    assert!(world.get::<ScriptReborn>(guard).is_none());
    world.run_system_once(consume_mission_say_requests).unwrap();
    let dialogue = std::mem::take(&mut *world.resource_mut::<DialogueState>());
    assert!(dialogue
        .text
        .as_deref()
        .is_some_and(|text| { text.ends_with("守卫复活了，再击败他一次！") }));
    let actor = capture_pst_hud_actor(world, player);
    world.run_system_once(evaluate_match_end).unwrap();
    assert!(!world.resource::<MatchEnd>().settled);
    assert_eq!(
        world
            .resource::<StageMissionRuntime>()
            .script()
            .unwrap()
            .host()
            .game_over(),
        None
    );

    harness.steps(150);
    kill_pst_demo_guard(&mut harness, guard);
    let world = harness.app.world_mut();
    assert_eq!(
        world
            .resource::<StageMissionRuntime>()
            .script()
            .unwrap()
            .host()
            .game_over(),
        Some(1),
    );
    world.run_system_once(evaluate_match_end).unwrap();
    assert_eq!(
        world.resource::<MatchEnd>().result,
        meteor_network::modes::ROUND_RESULT_TEAM2_WIPED
    );
    assert!(world.resource::<MatchEnd>().settled);
    world
        .run_system_once(crate::plugins::scoreboard::capture_offline_roster)
        .unwrap();
    world
        .resource_mut::<Time<bevy::time::Real>>()
        .advance_by(std::time::Duration::from_secs_f32(MISSION_SETTLE_SECONDS));
    world.run_system_once(advance_match_settlement).unwrap();
    let next = world.remove_resource::<NextState<AppState>>().unwrap();
    assert!(matches!(next, NextState::Pending(AppState::Result)));
    PstKeyDoorGuardUi {
        dialogue,
        player: actor,
        panel,
        panel_player,
        door_dialogue,
        door_player,
        request: world.remove_resource().unwrap(),
        result: world.remove_resource().unwrap(),
        next,
        kills: world.remove_resource().unwrap(),
        scores: world.remove_resource().unwrap(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn kill_pst_demo_guard(harness: &mut GameplayHarness, guard: Entity) {
    use bevy::ecs::system::RunSystemOnce;
    let world = harness.app.world_mut();

    world.get_mut::<Health>(guard).unwrap().current = 1.0;
    world.get_mut::<Transform>(guard).unwrap().translation = Vec3::Z * 3.0;
    world
        .get_mut::<NpcPerformance>(guard)
        .unwrap()
        .pause_remaining = 100.0;
    world
        .get_mut::<crate::components::fighter::TargetHitClock>(guard)
        .unwrap()
        .spawn_elapsed = 3.0;
    world
        .get_mut::<Transform>(harness.fighter)
        .unwrap()
        .translation = Vec3::ZERO;
    harness.press_mouse(MouseButton::Left);
    for tick in 0..150 {
        harness.step();
        if tick == 0 {
            harness.release_mouse(MouseButton::Left);
        }
        let world = harness.app.world_mut();
        crate::plugins::character_plugin::drive_fighter_animation_for_test(world);
        world.run_system_once(attack_windows).unwrap();
        if world.get::<DeathRespawn>(guard).unwrap().active {
            break;
        }
    }
    let world = harness.app.world_mut();
    assert!(
        world.get::<DeathRespawn>(guard).unwrap().active,
        "the sword hit must be lethal"
    );
}
