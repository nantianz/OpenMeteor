use crate::plugins::stage_collider::{StageCollider, StageMeshCollider};
use meteor_scene::stage_loader::parse_waypoint_graph;

fn sn05_story_app() -> (App, Entity) {
    fn advance_wall_clock(time: Res<Time>, mut wall: ResMut<Time<bevy::time::Real>>) {
        wall.advance_by(time.delta());
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let center = meteor_scene::stage_loader::get_stage_center("sn05", None);
    let mut graph = parse_waypoint_graph(
        &std::fs::read_to_string(root.join("Level/sn05.wp")).unwrap(),
        center,
    );
    let gmdl = meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn05/sn05.gmb")).unwrap())
        .unwrap();
    graph.navigation_bounds = crate::plugins::npc_navigation::source_bounds_for_test(&gmdl);
    let mut des = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join("sn05/sn05.des")).unwrap(),
    );
    let cob =
        meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn05/sn05.cob")).unwrap()).unwrap();
    let mut mesh = StageMeshCollider::from_gmdl(&gmdl, &des, Some(&cob.face_ids), center);

    use meteor_scene::scene_script_runtime::{SceneItemInstance, SceneScriptRuntime};
    let sources = ["PScriptLib/Def.h", "sn05/sn05.pst"]
        .map(|path| crate::text_codec::decode_text(&std::fs::read(root.join(path)).unwrap()))
        .join("\n");
    let instances = des
        .dummies
        .iter()
        .enumerate()
        .map(|(index, dummy)| SceneItemInstance {
            name: dummy.name.clone(),
            index,
            props: default(),
        })
        .collect();
    let mut scene = SceneScriptRuntime::load(&sources, instances, 250_000).unwrap();
    scene.call("Scene_OnLoad", &[]).unwrap();
    for (index, dummy) in des.dummies.iter_mut().enumerate() {
        if scene.host().instance(index).unwrap().collision_enabled() {
            dummy.collision_attr = Some(1);
        }
    }
    super::super::render_plugin::append_solid_props_for_test(&root, &des, center, &mut mesh);
    let source =
        crate::text_codec::decode_text(&std::fs::read(root.join("Level/sn05_.pst")).unwrap());
    let def = meteor_scene::mission_runtime::parse_mission_definition(&source);
    let mut runtime = MissionScriptRuntime::new(&source, def.values);
    for name in def.npc_scripts {
        let text = crate::text_codec::decode_text(
            &std::fs::read(root.join(format!("Level/{name}.pst"))).unwrap(),
        );
        runtime.register_npc_script(&name, &text).unwrap();
    }
    runtime.start().unwrap();
    let characters = ["p15", "p16"].map(|key| {
        (
            key.to_string(),
            crate::plugins::character_plugin::load_character_asset_for_test(
                std::slice::from_ref(&root),
                key,
            )
            .unwrap(),
        )
    });
    let common =
        meteor_asset::amb::parse_amb(&std::fs::read(root.join("Common/character.amb")).unwrap())
            .unwrap();
    let mut app = App::new();
    crate::plugins::configure_fighter_frame_sets(&mut app);
    crate::plugins::fighter_frame::install(&mut app);
    app.init_resource::<Time>()
        .init_resource::<Time<bevy::time::Real>>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<crate::app_state::Keymap>()
        .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
        .init_resource::<crate::components::fighter::LockOnController>()
        .init_resource::<crate::plugins::input_plugin::LocalCrouchInput>()
        .init_resource::<crate::plugins::input_plugin::LocalGuardInput>()
        .init_resource::<crate::plugins::item_plugin::ItemBuffs>()
        .insert_resource(StageMissionRuntime(Some(runtime)))
        .insert_resource(LaunchRequest {
            stage_id: "sn05".into(),
            mode: "single".into(),
            ..default()
        })
        .init_resource::<PendingMissionNpcCommands>()
        .init_resource::<PlayerPerformance>()
        .init_resource::<crate::plugins::character_contact_plugin::CharacterContactFrame>()
        .init_resource::<crate::plugins::ui_plugin::DialogueState>()
        .insert_resource(graph)
        .insert_resource(StageCollider { mesh: Some(mesh) })
        .insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
            characters.into(),
        ))
        .insert_resource(crate::plugins::character_plugin::CommonAmb(common))
        .add_systems(
            Update,
            advance_wall_clock.before(crate::plugins::FighterFrameSet::Movement),
        )
        .add_systems(
            Update,
            crate::plugins::character_contact_plugin::begin_character_contact_frame
                .before(crate::plugins::FighterFrameSet::Movement),
        )
        .add_systems(
            Update,
            (
                sync_mission_spatial,
                sync_mission_fighters,
                tick_mission_script,
                consume_mission_npc_requests,
            )
                .chain()
                .after(crate::plugins::FighterFrameSet::RootMotion)
                .before(crate::plugins::FighterFrameSet::PoseResolve),
        )
        .add_systems(
            Update,
            drive_player_performance.in_set(crate::plugins::FighterFrameSet::Movement),
        )
        .add_systems(
            crate::plugins::fighter_frame::FighterUpdate,
            (
                crate::plugins::ai_plugin::drive_npc_ai,
                crate::plugins::ai_plugin::finish_npc_script_tick,
            )
                .chain()
                .in_set(crate::plugins::FighterFrameSet::Movement),
        )
        .add_systems(
            crate::plugins::fighter_frame::FighterUpdate,
            crate::plugins::ai_plugin::move_npc_fighters
                .in_set(crate::plugins::FighterFrameSet::Locomotion),
        )
        .add_systems(
            crate::plugins::fighter_frame::FighterUpdate,
            crate::plugins::movement_plugin::move_local_fighter
                .run_if(crate::plugins::fighter_frame::current_is_local)
                .in_set(crate::plugins::FighterFrameSet::Locomotion),
        )
        .add_systems(
            crate::plugins::fighter_frame::FighterUpdate,
            crate::plugins::combat_plugin::advance_poses
                .in_set(crate::plugins::FighterFrameSet::PoseAdvance),
        )
        .add_systems(
            crate::plugins::fighter_frame::FighterUpdate,
            (
                crate::plugins::movement_plugin::dodge_root_motion,
                crate::plugins::ai_plugin::npc_ballistic_air_root_motion,
            )
                .chain()
                .in_set(crate::plugins::FighterFrameSet::RootMotion),
        );
    let player = app
        .world_mut()
        .spawn((
            LocalFighter,
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 5,
                weapon_slots: [5, 6],
                weapon_slot: 0,
                weapon_slot_variants: [0; 2],
                weapon_variant: 0,
            },
            Health {
                current: 2500.0,
                max: 2500.0,
            },
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            crate::plugins::movement_plugin::FighterAir::default(),
            crate::plugins::movement_plugin::FighterDodge::default(),
            crate::plugins::movement_plugin::FighterWall::default(),
            crate::components::fighter::PoseRuntime::idle(),
            crate::components::fighter::EntryBlend::default(),
            crate::components::fighter::SmState::default(),
            crate::components::fighter::CombatState::default(),
            Transform::from_xyz(1000.0, 0.0, 1000.0),
        ))
        .id();
    for _ in 0..2 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 120.0));
        app.update();
    }

    let mut query = app.world_mut().query::<(Entity, &Fighter)>();
    let story_actors: Vec<_> = query
        .iter(app.world())
        .filter_map(|(entity, fighter)| matches!(fighter.character_id, 15 | 16).then_some(entity))
        .collect();
    for actor in story_actors {
        crate::plugins::character_plugin::spawn_fighter_model_for_test(app.world_mut(), actor);
    }
    crate::plugins::character_plugin::install_crowd_animation_for_test(&mut app);
    (app, player)
}

#[test]
fn production_sn05_both_story_branches_meet_and_release_fanxuan() {
    #[derive(Resource, Default)]
    struct SpokenReports(Vec<String>);
    fn observe_speech(
        dialogue: Res<crate::plugins::ui_plugin::DialogueState>,
        mut reports: ResMut<SpokenReports>,
    ) {
        if let Some(text) = &dialogue.text {
            if text.contains("报告主子") || text.contains("主子！请您先行离开") {
                reports.0.push(text.clone());
            }
        }
    }
    for fps in [60, 120] {
        for first in ["屠城", "范璇"] {
            let (mut app, player) = sn05_story_app();

            app.init_resource::<SpokenReports>().add_systems(
                crate::plugins::fighter_frame::FighterUpdate,
                observe_speech
                    .after(crate::plugins::ai_plugin::drive_npc_ai)
                    .in_set(crate::plugins::FighterFrameSet::Movement),
            );
            let mut query = app.world_mut().query::<(&MissionNpc, &Transform)>();
            let p = query
                .iter(app.world())
                .find(|(npc, _)| npc.name == first)
                .unwrap()
                .1
                .translation;
            app.world_mut()
                .get_mut::<Transform>(player)
                .unwrap()
                .translation = p + Vec3::new(-3.0, 0.0, -3.0);

            *app.world_mut()
                .get_mut::<crate::plugins::movement_plugin::FighterAir>(player)
                .unwrap() = default();
            *app.world_mut().get_mut::<MotionState>(player).unwrap() = MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            };
            let mut meeting = false;
            let mut spoken_report = false;
            let mut removed = false;
            for tick in 0..fps * 180 {
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
                app.update();
                let mut q = app.world_mut().query::<(&MissionNpc, &NpcPerformance)>();
                let mut in_roster = false;
                for (npc, performance) in q.iter(app.world()) {
                    if npc.name == "范璇" {
                        in_roster |= npc.in_scene_roster;
                    }
                    let marker = if first == "屠城" {
                        "报告主子"
                    } else {
                        "主子！请您先行离开"
                    };
                    meeting |= performance.queue.iter().any(|command| {
                        command.action == "say"
                            && command.args.iter().any(
                                |arg| matches!(arg, PstValue::Str(text) if text.contains(marker)),
                            )
                    });
                }
                let marker = if first == "屠城" {
                    "报告主子"
                } else {
                    "主子！请您先行离开"
                };
                spoken_report |= app
                    .world()
                    .resource::<crate::plugins::ui_plugin::DialogueState>()
                    .text
                    .as_deref()
                    .is_some_and(|text| text.contains(marker));
                spoken_report |= app
                    .world()
                    .resource::<SpokenReports>()
                    .0
                    .iter()
                    .any(|text| text.contains(marker));
                let just_removed = !removed && !in_roster;
                removed = !in_roster;
                if meeting && spoken_report && removed {
                    break;
                }
                if just_removed {
                    let mut q = app.world_mut().query::<(
                        &MissionNpc,
                        &NpcPerformance,
                        &crate::components::fighter::SmState,
                    )>();
                    for (npc, performance, state) in q.iter(app.world()) {
                        if npc.name == "屠城" {
                            eprintln!("sn05-report branch={first} fps={fps} t={} state={state:?} performance={performance:?}", tick as f32 / fps as f32);
                        }
                    }
                }
            }
            assert!(meeting && spoken_report && removed, "{first} branch must meet, enqueue report and release Fanxuan: meeting={meeting}, spoken_report={spoken_report}, removed={removed}");
        }
    }
}

#[test]
fn performance_stack_preempts_and_resumes_counted_pause() {
    let mut state = PlayerPerformance::default();
    let command = |action: &str, arg: PstValue| NpcPerformCommand {
        action: action.into(),
        args: vec![arg],
    };
    state.clock.install(
        &mut state.queue,
        command("say", PstValue::Str("old".into())),
    );
    state
        .clock
        .install(&mut state.queue, command("pause", PstValue::Num(2.0)));
    assert!(player_performance_pulse(&mut state).is_none());
    assert!(player_performance_pulse(&mut state).is_none());
    state.clock.install(
        &mut state.queue,
        command("say", PstValue::Str("new1".into())),
    );
    state.clock.install(
        &mut state.queue,
        command("say", PstValue::Str("new2".into())),
    );
    assert_eq!(
        player_performance_pulse(&mut state).as_deref(),
        Some("new2")
    );
    assert_eq!(
        player_performance_pulse(&mut state).as_deref(),
        Some("new1")
    );
    for _ in 0..3 {
        assert!(player_performance_pulse(&mut state).is_none());
    }
    assert_eq!(player_performance_pulse(&mut state).as_deref(), Some("old"));
    assert!(state.queue.is_empty());
}

#[test]
fn production_sn10_story_executes_maximum_before_healing_at_each_frame_rate() {
    use crate::components::fighter::{Fighter, FighterStatusEffects, Qi};
    for fps in [60, 120] {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let read = |name: &str| {
            crate::text_codec::decode_text(
                &std::fs::read(root.join(format!("Level/{name}"))).unwrap(),
            )
        };
        let source = read("sn10_.pst");
        let def = meteor_scene::mission_runtime::parse_mission_definition(&source);
        let mut runtime = MissionScriptRuntime::new(&source, def.values);
        for index in 1..=5 {
            let name = format!("npc10_{index:02}");
            runtime
                .register_npc_script(&name, &read(&format!("{name}.pst")))
                .unwrap();
        }
        runtime.start().unwrap();
        let boss = runtime.host().char_id("屠城").unwrap();
        runtime.host_mut().sync_fighter(0, 400.0, 3000.0, 1.0);
        runtime.host_mut().sync_fighter(boss, 3000.0, 20000.0, 2.0);
        runtime.host_mut().sync_enemy(boss, Some(0));
        runtime.host_mut().sync_actor_position(0, [0.0; 3]);
        runtime
            .host_mut()
            .sync_actor_position(boss, [0.0, 0.0, 3.0]);
        runtime.update(0.1).unwrap();
        runtime.host_mut().take_player_perform_requests();
        runtime.update(7.0).unwrap();
        let table =
            meteor_asset::res::parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap())
                .unwrap()
                .combat_table();
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(StageMissionRuntime(Some(runtime)))
            .insert_resource(LaunchRequest {
                stage_id: "sn10".into(),
                mode: "single".into(),
                ..default()
            })
            .init_resource::<PendingMissionNpcCommands>()
            .init_resource::<PlayerPerformance>()
            .insert_resource(crate::plugins::combat_plugin::CombatResourceData(table))
            .add_systems(
                Update,
                (
                    consume_mission_npc_requests,
                    drive_player_performance,
                    crate::plugins::status_plugin::tick_status_effects,
                )
                    .chain(),
            );
        let player = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 6],
                    weapon_slot: 0,
                    weapon_slot_variants: [0; 2],
                    weapon_variant: 0,
                },
                Health {
                    current: 400.0,
                    max: 3000.0,
                },
                Qi::default(),
                Transform::default(),
            ))
            .id();
        let mut healing_states = Vec::new();
        let mut previous = (400.0, 3000.0);
        for _ in 0..fps * 400 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
            app.update();
            let health = app.world().get::<Health>(player).unwrap();
            let pair = (health.current, health.max);
            if pair != previous {
                healing_states.push(pair);
                previous = pair;
            }
            if app.world().resource::<PlayerPerformance>().queue.is_empty() {
                break;
            }
        }
        assert_eq!(healing_states, vec![(400.0, 9999.0), (5400.0, 9999.0), (9999.0, 9999.0)],
            "actual sn10 late-story use20/use15/use15 must preserve original LIFO and each HP clamp at {fps} FPS");
        assert_eq!(
            app.world()
                .get::<FighterStatusEffects>(player)
                .unwrap()
                .effects
                .attack_bonus(),
            150
        );
        assert!(!app.world().resource::<PlayerPerformance>().input_blocked);
    }
}

#[test]
fn production_sn05_intro_crouches_then_releases_the_original_control_latches() {
    let (mut app, _) = sn05_story_app();
    app.init_resource::<crate::plugins::input_plugin::LocalCrouchInput>();
    let mut saw_block = false;
    let mut saw_crouch = false;
    for _ in 0..120 * 60 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / 120.0));
        app.update();
        saw_block |= app.world().resource::<PlayerPerformance>().input_blocked;
        saw_crouch |= app
            .world()
            .resource::<crate::plugins::input_plugin::LocalCrouchInput>()
            .held;
        if saw_block && app.world().resource::<PlayerPerformance>().queue.is_empty() {
            break;
        }
    }
    assert!(saw_block && saw_crouch);
    assert!(!app.world().resource::<PlayerPerformance>().input_blocked);
    assert!(
        !app.world()
            .resource::<crate::plugins::input_plugin::LocalCrouchInput>()
            .held
    );
    assert!(app.world().resource::<PlayerPerformance>().queue.is_empty());
}

#[test]
fn performance_stack_resumes_guard_count_after_another_guard_preempts_it() {
    let mut clock = PerformanceClock::default();
    let mut queue = std::collections::VecDeque::new();
    let command = |action: &str, count: f64| NpcPerformCommand {
        action: action.into(),
        args: vec![PstValue::Num(count)],
    };
    clock.install(&mut queue, command("say", 0.0));
    clock.install(&mut queue, command("guard", 3.0));
    let pulse = |clock: &mut PerformanceClock,
                 queue: &mut std::collections::VecDeque<NpcPerformCommand>| {
        for _ in 0..3 {
            assert!(clock.next_eligible(queue, 0.1, true, true).is_none());
        }
        clock.next_eligible(queue, 0.1, true, true)
    };
    for _ in 0..3 {
        assert!(pulse(&mut clock, &mut queue).is_none());
    }
    clock.install(&mut queue, command("guard", 1.0));
    for _ in 0..2 {
        assert!(pulse(&mut clock, &mut queue).is_none());
    }
    assert_eq!(pulse(&mut clock, &mut queue).unwrap().args[0].as_f64(), 1.0);
    assert!(clock.guard_held());
    for _ in 0..3 {
        assert!(pulse(&mut clock, &mut queue).is_none());
    }
    assert_eq!(pulse(&mut clock, &mut queue).unwrap().args[0].as_f64(), 3.0);
    assert!(!clock.guard_held());
    assert_eq!(pulse(&mut clock, &mut queue).unwrap().action, "say");
}
