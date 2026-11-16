fn peace_mission(value: i32) -> StageMissionRuntime {
    let source = format!("int Peace={value}; OnStart(){{ Peace=0; Print(Peace); }}");
    let definition = meteor_scene::mission_runtime::parse_mission_definition(&source);
    let mut runtime =
        meteor_scene::mission_runtime::MissionScriptRuntime::new(&source, definition.values);
    runtime.start().unwrap();
    assert_eq!(
        runtime.host().output(),
        &["0"],
        "script assignment must actually execute"
    );
    StageMissionRuntime::from_script(runtime)
}

fn peace_ai_app(value: i32, mode: BehaviorMode) -> (App, Entity) {
    let (mut app, npc) = perception_test_app(mode, Vec3::Z * 2.0, None);
    app.insert_resource(peace_mission(value));
    app.world_mut().get_mut::<Fighter>(npc).unwrap().weapon = 6;
    app.world_mut().get_mut::<MissionNpc>(npc).unwrap().think = 100.0;
    app.world_mut().entity_mut(npc).insert((
        NpcPerformance::default(),
        NpcCombatProfile {
            attack1: 100.0,
            attack2: 0.0,
            attack3: 0.0,
            guard: 0.0,
            dodge: 0.0,
            jump: 0.0,
            look: 0.0,
            burst: 0.0,
            aim: 100.0,
            get_item: 0.0,
        },
    ));
    (app, npc)
}

#[test]
fn peace_loaded_snapshot_gates_acquisition_and_attack_separately() {
    assert_eq!(StageMissionRuntime::default().peace(), 0);
    for peace in [-1, 0, 1, 2] {
        let (mut app, npc) = peace_ai_app(peace, BehaviorMode::Wait);
        assert_eq!(app.world().resource::<StageMissionRuntime>().peace(), peace);
        app.update();
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert_eq!(intent.enemy_id, (peace <= 1).then_some(0), "Peace={peace}");
        assert_eq!(intent.want_attack, peace <= 0, "Peace={peace}");
    }
}

#[test]
fn peace_keeps_guard_existing_combat_and_downed_direction_choices() {
    let (mut app, npc) = peace_ai_app(1, BehaviorMode::Wait);
    app.world_mut()
        .get_mut::<NpcCombatProfile>(npc)
        .unwrap()
        .guard = 100.0;
    app.update();
    let intent = app.world().get::<AiIntent>(npc).unwrap();
    assert!(intent.want_guard);
    assert!(!intent.want_attack);

    let (mut app, npc) = peace_ai_app(2, BehaviorMode::Wait);
    app.world_mut().get_mut::<AiIntent>(npc).unwrap().enemy_id = Some(0);
    app.world_mut().entity_mut(npc).insert(SmState {
        current: FighterState::Attack1,
        ..default()
    });
    app.update();
    assert_eq!(app.world().get::<AiIntent>(npc).unwrap().enemy_id, Some(0));
    assert!(!app.world().get::<AiIntent>(npc).unwrap().want_attack);
    assert_eq!(
        app.world().get::<SmState>(npc).unwrap().current,
        FighterState::Attack1
    );

    for roll in 0..8 {
        let mut intent = AiIntent::default();
        queue_npc_down_recovery(&mut intent, roll, false, 0.0, Vec3::ZERO, None);
        assert!(!intent.want_attack);
        assert_eq!(intent.dodge_burst, (roll < 4).then_some(false));
    }
}

#[test]
fn peace_attacktarget_spends_attempt_without_attack_and_still_travels() {
    let (mut app, npc) = peace_ai_app(2, BehaviorMode::AttackTarget);
    app.world_mut()
        .get_mut::<MissionNpc>(npc)
        .unwrap()
        .attack_target_remaining = 2;
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .attack_target_point = Some(Vec3::Z * 2.0);
    app.update();
    assert_eq!(
        app.world()
            .get::<MissionNpc>(npc)
            .unwrap()
            .attack_target_remaining,
        1
    );
    assert!(!app.world().get::<AiIntent>(npc).unwrap().want_attack);
    assert_eq!(
        app.world().get::<MissionNpc>(npc).unwrap().forced_behavior,
        Some(BehaviorMode::AttackTarget)
    );

    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .attack_target_point = Some(Vec3::Z * 20.0);
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    app.update();
    assert!(
        app.world()
            .get::<AiIntent>(npc)
            .unwrap()
            .move_dir
            .length_squared()
            > 0.0
    );
    assert_eq!(
        app.world()
            .get::<MissionNpc>(npc)
            .unwrap()
            .attack_target_remaining,
        1
    );
}

#[test]
fn peace_consumes_perform_attack_but_preserves_other_perform_commands() {
    for (action, args) in [
        ("attack", vec![]),
        ("jump", vec![]),
        ("aggress", vec![]),
        ("use", vec![meteor_scene::pst_runtime::PstValue::Num(15.0)]),
    ] {
        let (mut app, npc) = peace_ai_app(2, BehaviorMode::Wait);
        app.world_mut()
            .get_mut::<NpcPerformance>(npc)
            .unwrap()
            .queue
            .push_back(crate::plugins::stage_plugin::NpcPerformCommand {
                action: action.into(),
                args,
            });
        for _ in 0..4 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(100));
            app.update();
        }
        let performance = app.world().get::<NpcPerformance>(npc).unwrap();
        assert!(
            performance.queue.is_empty(),
            "{action} should still be consumed"
        );
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert!(!intent.want_attack, "{action}");
        assert_eq!(performance.jump_requested, action == "jump");
        assert_eq!(intent.want_taunt, action == "aggress");
        assert_eq!(
            performance.use_items.as_slice(),
            if action == "use" { &[15][..] } else { &[] }
        );
    }
}

#[test]
fn npc_turn_uses_original_small_angle_dead_zone_and_strict_snap_gate() {
    let dt = 0.001;
    for sign in [-1.0, 1.0] {
        for degrees in [0.0_f32, 0.5, 0.999] {
            assert_eq!(npc_combat_turn(0.0, sign * degrees.to_radians(), dt), 0.0);
        }
        for degrees in [1.0_f32, 2.0, 2.999] {
            let target = sign * degrees.to_radians();
            assert_eq!(npc_combat_turn(0.0, target, dt), target);
        }
        for degrees in [3.0_f32, 3.001, 30.0] {
            let result = npc_combat_turn(0.0, sign * degrees.to_radians(), dt);
            assert!((result - sign * 0.5_f32.to_radians()).abs() < 1e-7);
        }
        for (degrees, elapsed, expected) in [(3.9_f32, 0.01, 3.0_f32), (8.5, 1.0 / 60.0, 8.0)] {
            let result = npc_combat_turn(0.0, sign * degrees.to_radians(), elapsed);
            assert!((result - sign * expected.to_radians()).abs() < 1e-7);
            assert_eq!(
                npc_combat_turn(result, sign * degrees.to_radians(), elapsed),
                result,
                "the remaining fractional degree lies in the original dead zone"
            );
        }
    }
}
