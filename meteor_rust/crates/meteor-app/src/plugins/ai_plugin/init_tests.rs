fn init_parity_app(source: &str) -> (App, Entity) {
    let (mut app, npc) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 2.0, None);
    let mut runtime = meteor_scene::mission_runtime::MissionScriptRuntime::new(
        "OnStart(){ AddNPC(\"a\"); }",
        default(),
    );
    runtime.register_npc_script("a", source).unwrap();
    runtime.start().unwrap();
    runtime.host_mut().sync_fighter(0, 100.0, 100.0, 0.0);
    app.insert_resource(StageMissionRuntime::from_script(runtime));
    app.add_systems(Update, finish_npc_script_tick.after(drive_npc_ai));
    {
        let mut actor = app.world_mut().get_mut::<MissionNpc>(npc).unwrap();
        actor.id = 1;
        actor.think = 100.0;
        actor.forced_behavior = None;
        actor.behavior_args.clear();
    }
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .schedule_initial_think(100.0, 1);
    app.world_mut()
        .entity_mut(npc)
        .insert(NpcPerformance::default());
    (app, npc)
}

fn advance_init_parity(app: &mut App, dt: f32) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(dt));
    app.world_mut()
        .resource_mut::<StageMissionRuntime>()
        .script_mut()
        .unwrap()
        .update(f64::from(dt))
        .unwrap();
    app.update();
}

fn init_parity_output(app: &App) -> &[String] {
    app.world()
        .resource::<StageMissionRuntime>()
        .script()
        .unwrap()
        .host()
        .output()
}

#[test]
fn production_init_waits_for_think_then_first_reset_discards_early_behavior_and_perform() {
    use meteor_scene::pst_runtime::PstValue;
    let (mut app, npc) = init_parity_app(
        "int HP=900; OnInit(int id){ Print(id); } OnStart(){ Print(GetHP(1)); } OnUpdate(){ Print(3); }",
    );
    app.world_mut()
        .get_mut::<MissionNpc>(npc)
        .unwrap()
        .forced_behavior = Some(BehaviorMode::Run);
    app.world_mut()
        .resource_mut::<StageMissionRuntime>()
        .script_mut()
        .unwrap()
        .acknowledge_npc_behavior(1, "run");
    app.world_mut()
        .get_mut::<NpcPerformance>(npc)
        .unwrap()
        .queue
        .push_back(crate::plugins::stage_plugin::NpcPerformCommand {
            action: "pause".into(),
            args: vec![PstValue::Num(100.0)],
        });
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 37.0;
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.2;
    advance_init_parity(&mut app, 0.1);
    assert_eq!(init_parity_output(&app), &["1"]);
    assert!(
        app.world()
            .get::<NpcPatrol>(npc)
            .unwrap()
            .first_update_pending
    );
    advance_init_parity(&mut app, 0.11);
    assert_eq!(init_parity_output(&app), &["1", "900", "3"]);
    let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
    assert!(!patrol.first_update_pending);
    assert_eq!(
        patrol.think_remaining,
        meteor_ai::combat::decision_interval(100.0, patrol.init_delay)
    );
    assert_eq!(
        app.world().get::<MissionNpc>(npc).unwrap().forced_behavior,
        None
    );
    assert!(app
        .world()
        .get::<NpcPerformance>(npc)
        .unwrap()
        .queue
        .is_empty());
    assert_eq!(
        app.world().get::<Health>(npc).unwrap().current,
        900.0,
        "first Think restores the loaded HP property before OnStart"
    );
    assert_eq!(app.world().get::<Health>(npc).unwrap().max, 900.0);
    assert_eq!(app.world().get::<AiIntent>(npc).unwrap().enemy_id, None);
}

#[test]
fn production_missing_and_noop_onstart_stay_in_init_without_default_enemy_scan() {
    for (source, repeated) in [
        ("OnInit(int id){ Print(id); }", false),
        ("int n=0; OnStart(){ n=n+1; Print(n); }", true),
    ] {
        let (mut app, npc) = init_parity_app(source);
        for _ in 0..3 {
            app.world_mut()
                .get_mut::<NpcPatrol>(npc)
                .unwrap()
                .think_remaining = 0.0;
            advance_init_parity(&mut app, 0.01);
            let intent = app.world().get::<AiIntent>(npc).unwrap();
            assert!(intent.enemy_id.is_none() && !intent.want_attack);
            assert_eq!(intent.move_dir, Vec2::ZERO);
            assert_eq!(
                app.world().get::<MissionNpc>(npc).unwrap().forced_behavior,
                None
            );
        }
        assert!(app
            .world()
            .resource::<StageMissionRuntime>()
            .script()
            .unwrap()
            .npc_init_pending(1));
        assert_eq!(
            init_parity_output(&app),
            if repeated {
                &["1", "2", "3"][..]
            } else {
                &["1"][..]
            }
        );
    }
}

#[test]
fn production_init_onstart_runs_during_down_without_changing_character_state() {
    let (mut app, npc) = init_parity_app("OnStart(){ Print(1); }");
    app.world_mut().entity_mut(npc).insert(SmState {
        current: FighterState::Down,
        ..default()
    });
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.01);
    assert_eq!(init_parity_output(&app), &["1"]);
    assert_eq!(
        app.world().get::<SmState>(npc).unwrap().current,
        FighterState::Down
    );
}

#[test]
fn production_weapon_switch_does_not_suspend_init_or_the_script_think_tail() {
    let (mut app, npc) = init_parity_app("OnStart(){ Print(1); } OnUpdate(){ Print(2); }");
    app.world_mut()
        .entity_mut(npc)
        .insert(crate::components::fighter::WeaponSwitchActive::default());
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.01);
    assert_eq!(init_parity_output(&app), &["1", "2"]);
    assert!(app
        .world()
        .get::<crate::components::fighter::WeaponSwitchActive>(npc)
        .is_some());
}

#[test]
fn production_stock_room_npc_waits_for_init_then_enters_shipped_run_callback() {
    let (mut app, npc) = perception_test_app(BehaviorMode::Wait, Vec3::Z * 2.0, None);
    app.world_mut()
        .entity_mut(npc)
        .remove::<MissionNpc>()
        .insert((StockNpcBrain, stock_npc_profile()));
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
        patrol.schedule_initial_think(STOCK_NPC_THINK, 1);
        patrol.think_remaining = 0.02;
    }
    for dt in [0.01, 0.02] {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(dt));
        app.update();
        let intent = app.world().get::<AiIntent>(npc).unwrap();
        assert_eq!(intent.enemy_id, None);
        assert_eq!(intent.move_dir, Vec2::ZERO);
    }
    let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
    assert!(!patrol.first_update_pending);
    assert_eq!(
        patrol.think_remaining,
        meteor_ai::combat::decision_interval(STOCK_NPC_THINK, patrol.init_delay)
    );
    assert_eq!(patrol.callback_ticks_for_test(), (0, 0));
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(10));
    app.update();
    assert_eq!(app.world().get::<AiIntent>(npc).unwrap().enemy_id, Some(0));
}

#[test]
fn production_first_think_preserves_signed_hp_and_finishes_callbacks_before_death_gate() {
    let (mut app, npc) =
        init_parity_app("int HP=65535; OnStart(){ Print(GetHP(1)); } OnUpdate(){ Print(2); }");
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.01);
    let health = app.world().get::<Health>(npc).unwrap();
    assert_eq!((health.current, health.max), (-1.0, -1.0));
    assert_eq!(init_parity_output(&app), &["-1", "2"]);
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.5);
    assert_eq!(init_parity_output(&app), &["-1", "2"]);
}

#[test]
fn production_npc_onupdate_333ms_gate_requires_a_real_think_tail() {
    let (mut app, npc) = init_parity_app("int n=0; OnUpdate(){ n=n+1; Print(n); }");
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 5.0;
    advance_init_parity(&mut app, 0.34);
    assert!(init_parity_output(&app).is_empty());
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.01);
    assert_eq!(init_parity_output(&app), &["1"]);
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.1);
    assert_eq!(init_parity_output(&app), &["1"]);
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.24);
    assert_eq!(init_parity_output(&app), &["1", "2"]);
}

#[test]
fn production_temporary_kill_returns_to_init_before_retrying_onstart() {
    let (mut app, npc) = init_parity_app("int n=0; OnStart(){ n=n+1; Print(n); }");
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.01);
    let parent = app.world().get::<MissionNpc>(npc).unwrap().clone();
    let intent = app.world().get::<AiIntent>(npc).unwrap().clone();
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .prepare_behavior_change(Some(&parent), &intent, Some(BehaviorMode::Kill));
    {
        let mut actor = app.world_mut().get_mut::<MissionNpc>(npc).unwrap();
        actor.forced_behavior = Some(BehaviorMode::Kill);
        actor.behavior_args = vec![meteor_scene::pst_runtime::PstValue::Num(99.0)];
    }
    advance_init_parity(&mut app, 0.01);
    assert_eq!(init_parity_output(&app), &["1"]);
    assert_eq!(
        app.world().get::<MissionNpc>(npc).unwrap().forced_behavior,
        None
    );
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.01);
    assert_eq!(init_parity_output(&app), &["1", "2"]);
}

#[test]
fn production_perform_suspends_init_and_samples_onupdate_only_at_its_pulse() {
    let (mut app, npc) = init_parity_app("OnStart(){ Print(1); } OnUpdate(){ Print(2); }");
    {
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
        patrol.first_update_pending = false;
        patrol.think_remaining = 0.0;
    }
    app.world_mut()
        .get_mut::<NpcPerformance>(npc)
        .unwrap()
        .queue
        .push_back(crate::plugins::stage_plugin::NpcPerformCommand {
            action: "pause".into(),
            args: vec![meteor_scene::pst_runtime::PstValue::Num(100.0)],
        });
    advance_init_parity(&mut app, 0.1);
    assert!(init_parity_output(&app).is_empty());
    advance_init_parity(&mut app, 0.1);
    advance_init_parity(&mut app, 0.1);
    assert!(init_parity_output(&app).is_empty());
    advance_init_parity(&mut app, 0.04);
    assert_eq!(init_parity_output(&app), &["2"]);
    *app.world_mut().get_mut::<NpcPerformance>(npc).unwrap() = default();
    app.world_mut()
        .get_mut::<NpcPatrol>(npc)
        .unwrap()
        .think_remaining = 0.0;
    advance_init_parity(&mut app, 0.01);
    assert_eq!(init_parity_output(&app), &["2", "1"]);
}
