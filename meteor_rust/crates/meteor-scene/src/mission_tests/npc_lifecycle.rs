use super::*;

fn runtime(npc: &str) -> MissionScriptRuntime {
    let source = "OnStart(){ AddNPC(\"a\"); }";
    let mut runtime = MissionScriptRuntime::new(source, parse_globals(source));
    runtime.register_npc_script("a", npc).unwrap();
    runtime.start().unwrap();
    runtime
}

#[test]
fn loading_runs_oninit_and_reads_properties_before_the_first_init_think() {
    let mut runtime = runtime(
        r#"string Name="Before"; int HP=100; int Team=1; int Model=1;
        int Weapon=1; int Spawn=0; int Think=100; int self;
        OnInit(int id) { self=GetSelf(id); Name="After"; HP=900; Team=2;
            Model=4; Weapon=5; Spawn=8; Think=250; Print(self); }
        OnStart() { Print(GetHP(self)); ChangeBehavior(self,"wait"); }"#,
    );
    assert_eq!(runtime.host().output(), &["1"]);
    assert_eq!(runtime.host().char_id("Before"), None);
    assert_eq!(runtime.host().char_id("After"), Some(1));
    let definition = runtime.npc_definition(1, "a").unwrap();
    assert_eq!(definition.name, "After");
    assert_eq!(
        (
            definition.hp,
            definition.team,
            definition.model,
            definition.weapon,
            definition.spawn,
            definition.think
        ),
        (900, 2, 4, 5, 8, 250),
    );
    assert!(runtime.npc_init_pending(1));
    assert!(runtime.host_mut().take_behavior_requests().is_empty());
    runtime.run_npc_init(1).unwrap();
    assert_eq!(runtime.host().output(), &["1", "900"]);
    assert_eq!(runtime.host_mut().take_behavior_requests()[0].1, "wait");
    assert!(
        runtime.npc_init_pending(1),
        "queued commands are not yet accepted"
    );
    runtime.acknowledge_npc_behavior(1, "wait");
    assert!(!runtime.npc_init_pending(1));
    assert!(!runtime.run_npc_init(1).unwrap());
}

#[test]
fn missing_or_noop_onstart_retains_init_and_temporary_callbacks_do_not_replace_it() {
    let mut missing = runtime("OnInit(int id) { Print(id); }");
    assert!(!missing.run_npc_init(1).unwrap());
    assert!(!missing.run_npc_init(1).unwrap());
    assert!(missing.npc_init_pending(1));
    assert!(missing.host_mut().take_behavior_requests().is_empty());

    let mut runtime = runtime("int n=0; OnStart(){ n=n+1; Print(n); }");
    for behavior in [
        "kill",
        "dodge",
        "attacktarget",
        "perform",
        "unknown",
        "Wait",
    ] {
        runtime.acknowledge_npc_behavior(1, behavior);
        assert!(runtime.npc_init_pending(1));
    }
    runtime.run_npc_init(1).unwrap();
    runtime.run_npc_init(1).unwrap();
    assert_eq!(runtime.host().output(), &["1", "2"]);
    runtime.host_mut().sync_fighter(1, 0.0, 100.0, 1.0);
    assert!(!runtime.run_npc_init(1).unwrap());
    assert_eq!(runtime.host().output(), &["1", "2"]);
}

#[test]
fn ai_reset_restarts_init_without_reloading_script_globals_or_oninit() {
    let mut runtime =
        runtime("int n=0; OnInit(int id){ Print(id); } OnStart(){ n=n+1; Print(n); }");
    runtime.run_npc_init(1).unwrap();
    for behavior in ["wait", "idle", "run", "follow", "patrol"] {
        runtime.acknowledge_npc_behavior(1, behavior);
        assert!(!runtime.npc_init_pending(1));
        runtime.reset_npc_init(1);
        assert!(runtime.npc_init_pending(1));
    }
    runtime.run_npc_init(1).unwrap();
    assert_eq!(runtime.host().output(), &["1", "1", "2"]);
}

#[test]
fn initial_hp_uses_loaded_signed_property_until_update_ai_attrib() {
    let mut runtime = runtime(
        r#"int HP=300; int self;
        OnInit(int id){ self=id; HP=400; }
        ChangeGlobal(){ HP=700; }
        Refresh(){ UpdateAIAttrib(self); }
        Signed(){ HP=65535; UpdateAIAttrib(self); }"#,
    );
    assert_eq!(runtime.npc_initial_hp(1), Some(400.0));
    runtime.call_npc_script(1, "ChangeGlobal", &[]).unwrap();
    assert_eq!(runtime.npc_initial_hp(1), Some(400.0));
    runtime.call_npc_script(1, "Refresh", &[]).unwrap();
    assert_eq!(runtime.npc_initial_hp(1), Some(700.0));
    runtime.call_npc_script(1, "Signed", &[]).unwrap();
    assert_eq!(runtime.npc_initial_hp(1), Some(-1.0));
}

#[test]
fn change_behavior_rejects_aliases_case_changes_and_invalid_numeric_targets() {
    let mut runtime = runtime("OnInit(int id) {}");
    for name in [
        "Wait", "FOLLOW", "stop", "chase", "attack", "fight", "defend", "escape",
    ] {
        let result = runtime.host_mut().builtin(
            "ChangeBehavior",
            &[
                PstValue::Num(1.0),
                PstValue::Str(name.into()),
                PstValue::Num(0.0),
            ],
        );
        assert_eq!(result, PstValue::Num(0.0), "{name}");
    }
    let change = |runtime: &mut MissionScriptRuntime, name: &str, target: PstValue| {
        runtime.host_mut().builtin(
            "ChangeBehavior",
            &[PstValue::Num(1.0), PstValue::Str(name.into()), target],
        )
    };
    assert_eq!(
        change(&mut runtime, "follow", PstValue::Num(99.0)),
        PstValue::Num(0.0)
    );
    assert_eq!(
        change(&mut runtime, "kill", PstValue::Str("player".into())),
        PstValue::Num(0.0)
    );
    assert!(runtime.host_mut().take_behavior_requests().is_empty());
    assert!(runtime.npc_init_pending(1));

    assert_eq!(
        change(&mut runtime, "follow", PstValue::Str("future actor".into())),
        PstValue::Num(1.0)
    );
    runtime.host_mut().sync_fighter(2, 0.0, 100.0, 2.0);
    for name in ["follow", "kill", "dodge"] {
        assert_eq!(
            change(&mut runtime, name, PstValue::Num(2.0)),
            PstValue::Num(1.0),
            "dead roster actor: {name}"
        );
    }
    assert_eq!(runtime.host_mut().take_behavior_requests().len(), 4);
    runtime.host_mut().remove_fighter(2);
    assert_eq!(
        change(&mut runtime, "follow", PstValue::Num(2.0)),
        PstValue::Num(0.0)
    );
}

#[test]
fn hard_reset_discards_older_performs_but_preserves_newer_and_temporary_commands() {
    let mut runtime = runtime(
        r#"int self; OnInit(int id){ self=id; }
        OnStart(){ Perform(self,"say","old"); ChangeBehavior(self,"wait");
            Print(IsPerforming(self)); Perform(self,"say","new");
            ChangeBehavior(self,"attacktarget",0); Print(IsPerforming(self)); }"#,
    );
    runtime.run_npc_init(1).unwrap();
    assert_eq!(runtime.host().output(), &["0", "1"]);
    assert_eq!(
        runtime.host_mut().take_perform_requests(),
        vec![(1, "say".into(), vec![PstValue::Str("new".into())]),]
    );
}

#[test]
fn callback_tail_belongs_only_to_the_actor_executing_onstart() {
    let source = "OnStart(){ AddNPC(\"a\"); AddNPC(\"b\"); ChangeBehavior(1,\"idle\"); }";
    let mut runtime = MissionScriptRuntime::new(source, parse_globals(source));
    runtime
        .register_npc_script(
            "a",
            r#"
        OnInit(int id){ ChangeBehavior(id,"wait"); }
        OnStart(){ ChangeBehavior(1,"run"); ChangeBehavior(2,"wait"); Call(2,"Nested"); }
        OnUpdate(){ ChangeBehavior(1,"idle"); }
    "#,
        )
        .unwrap();
    runtime
        .register_npc_script(
            "b",
            r#"
        Nested(){ ChangeBehavior(1,"attacktarget",0); ChangeBehavior(2,"run"); }
    "#,
        )
        .unwrap();
    runtime.start().unwrap();
    let origins = |runtime: &mut MissionScriptRuntime| {
        runtime
            .host_mut()
            .take_behavior_requests_with_context()
            .into_iter()
            .map(|(id, mode, _, _, tail)| (id, mode, tail))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        origins(&mut runtime),
        vec![(1, "wait".into(), false), (1, "idle".into(), false),],
        "OnInit and mission OnStart execute outside an NPC callback"
    );

    runtime.run_npc_init(1).unwrap();
    assert_eq!(
        origins(&mut runtime),
        vec![
            (1, "run".into(), true),
            (2, "wait".into(), false),
            (1, "attacktarget".into(), true),
            (2, "run".into(), false),
        ],
        "nested scripts retain the outer NPC callback owner"
    );

    runtime.update_npc_script(1).unwrap();
    assert_eq!(
        origins(&mut runtime),
        vec![(1, "idle".into(), false)],
        "NPC OnUpdate runs after the original callback tick increment"
    );
    runtime.call_npc_script(1, "OnStart", &[]).unwrap();
    assert!(
        origins(&mut runtime).iter().all(|(_, _, tail)| !tail),
        "an externally called OnStart does not gain a Think tail"
    );
}

fn clock_runtime() -> MissionScriptRuntime {
    let mut runtime = runtime("int self; int n=0; OnInit(int id){ self=GetSelf(id); } OnStart(){ Print(self); } OnUpdate(){ n=n+1; Print(n); }");
    runtime.run_npc_init(1).unwrap();
    runtime
}

#[test]
fn npc_script_333ms_gate_is_sampled_only_at_actual_think_tails_without_catch_up() {
    let mut runtime = clock_runtime();
    runtime.update(0.001).unwrap();
    assert_eq!(
        runtime.host().output(),
        &["1"],
        "frame/mission updates cannot execute NPC OnUpdate"
    );
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output(), &["1", "1"]);
    runtime.update(0.332).unwrap();
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output().len(), 2);
    runtime.update(0.001).unwrap();
    assert_eq!(runtime.host().output().len(), 2);
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output(), &["1", "1", "2"]);
    runtime.update(1.0).unwrap();
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output(), &["1", "1", "2", "3"]);
    runtime.update(0.332).unwrap();
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output().len(), 4);
    runtime.update(0.001).unwrap();
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output().last().unwrap(), "4");
}

#[test]
fn npc_script_death_retains_clock_and_reactivation_resets_it() {
    let mut runtime = clock_runtime();
    runtime.update(0.001).unwrap();
    runtime.update_npc_script(1).unwrap();
    runtime.host_mut().sync_fighter(1, 0.0, 100.0, 1.0);
    runtime.update(0.2).unwrap();
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output().len(), 2);
    runtime.host_mut().sync_fighter(1, 100.0, 100.0, 1.0);
    runtime.update(0.1).unwrap();
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output().len(), 2);
    runtime.update(0.033).unwrap();
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output().last().unwrap(), "2");
    runtime.runtime.activate_script(1).unwrap();
    assert_eq!(runtime.host().output().len(), 3);
    runtime.run_npc_init(1).unwrap();
    runtime.update(0.001).unwrap();
    runtime.update_npc_script(1).unwrap();
    assert_eq!(&runtime.host().output()[3..], &["1", "1"]);
}

#[test]
fn npc_scripts_without_oninit_still_activate_as_in_exe_loader() {
    let mut runtime = runtime("OnStart(){ Print(7); } OnUpdate(){ Print(8); }");
    assert!(runtime.host().output().is_empty());
    runtime.run_npc_init(1).unwrap();
    runtime.update(0.001).unwrap();
    runtime.update_npc_script(1).unwrap();
    assert_eq!(runtime.host().output(), &["7", "8"]);
}
