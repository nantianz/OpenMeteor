use super::*;

#[cfg(feature = "original-assets")]
#[test]
fn origin_sn10_poison_and_both_healing_phases_target_the_authored_actor() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
    let read = |name: &str| crate::text::decode_cp936(&std::fs::read(root.join(name)).unwrap());
    let source = read("sn10_.pst");
    let mut runtime = MissionScriptRuntime::new(&source, parse_mission_definition(&source).values);
    for index in 1..=5 {
        let name = format!("npc10_{index:02}");
        runtime
            .register_npc_script(&name, &read(&format!("{name}.pst")))
            .unwrap();
    }
    runtime.start().unwrap();
    let boss = runtime.host().char_id("屠城").unwrap();
    runtime.host_mut().sync_fighter(0, 3000.0, 3000.0, 1.0);
    runtime.host_mut().sync_fighter(boss, 20000.0, 20000.0, 2.0);
    runtime.host_mut().sync_enemy(boss, Some(0));
    runtime.host_mut().sync_actor_position(0, [0.0; 3]);
    runtime
        .host_mut()
        .sync_actor_position(boss, [0.0, 0.0, 3.0]);
    runtime.update(0.1).unwrap();
    let player = runtime.host_mut().take_player_perform_requests();
    assert!(player
        .iter()
        .any(|(action, args)| action == "use" && args[0].as_f64() == 14.0));
    assert!(!player
        .iter()
        .any(|(action, args)| action == "use" && args[0].as_f64() == 15.0));
    runtime.host_mut().take_perform_requests();
    runtime.host_mut().sync_fighter(0, 400.0, 3000.0, 1.0);
    runtime.host_mut().sync_fighter(boss, 3000.0, 20000.0, 2.0);
    runtime.update(0.1).unwrap();
    runtime.update(7.0).unwrap();
    let commands = runtime.host_mut().take_perform_requests();
    let heals: Vec<_> = commands
        .iter()
        .filter(|(_, action, args)| action == "use" && args[0].as_f64() == 15.0)
        .collect();
    assert_eq!(
        heals.len(),
        4,
        "the story restores the boss with four +5000 uses"
    );
    assert!(heals.iter().all(|(id, _, _)| *id == boss));
    let player = runtime.host_mut().take_player_perform_requests();
    assert_eq!(
        player
            .iter()
            .filter(|(action, args)| action == "use" && args[0].as_f64() == 15.0)
            .count(),
        2,
        "the protagonist also heals twice during the late dialogue"
    );
    let player_uses: Vec<_> = player
        .iter()
        .filter(|(action, _)| action == "use")
        .map(|(_, args)| args[0].as_f64() as u32)
        .collect();
    assert_eq!(player_uses, vec![19, 15, 15, 20]);

    runtime.update(121.0).unwrap();
    let commands = runtime.host_mut().take_perform_requests();
    let heals: Vec<_> = commands
        .iter()
        .filter(|(_, action, args)| action == "use" && args[0].as_f64() == 15.0)
        .collect();
    assert_eq!(heals.len(), 1);
    assert_eq!(heals[0].0, boss);
    runtime.update(59.0).unwrap();
    assert!(!runtime
        .host_mut()
        .take_perform_requests()
        .iter()
        .any(|(_, action, args)| action == "use" && args[0].as_f64() == 15.0));
    runtime.host_mut().sync_fighter(boss, 5000.0, 20000.0, 2.0);
    runtime.update(2.0).unwrap();
    assert!(!runtime
        .host_mut()
        .take_perform_requests()
        .iter()
        .any(|(_, action, args)| action == "use" && args[0].as_f64() == 15.0));
    runtime.host_mut().sync_fighter(boss, 4999.0, 20000.0, 2.0);
    runtime.update(0.1).unwrap();
    let commands = runtime.host_mut().take_perform_requests();
    assert_eq!(
        commands
            .iter()
            .filter(|(id, action, args)| *id == boss && action == "use" && args[0].as_f64() == 15.0)
            .count(),
        1
    );
}
