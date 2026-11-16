#[test]
fn npc_sn10_boss_returns_to_combat_after_script_dodge() {
    use crate::plugins::ai_plugin::NpcPatrol;
    use meteor_ai::behavior::BehaviorMode;
    use meteor_scene::mission_runtime::{
        parse_mission_definition, parse_npc_definition, MissionScriptRuntime,
    };
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
    let read =
        |name: &str| meteor_scene::text::decode_cp936(&std::fs::read(root.join(name)).unwrap());
    let source = read("sn10_.pst");
    let definition = parse_npc_definition(&read("npc10_01.pst"), "npc10_01");
    let mut runtime = MissionScriptRuntime::new(&source, parse_mission_definition(&source).values);
    runtime
        .register_npc_script("npc10_01", &read("npc10_01.pst"))
        .unwrap();
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
    runtime.host_mut().take_behavior_requests();
    runtime.update(81.0).unwrap();
    let requests = runtime.host_mut().take_behavior_requests();
    let (_, behavior, args) = requests
        .iter()
        .find(|(id, behavior, _)| *id == boss && behavior == "dodge")
        .expect("authored timed dodge");
    eprintln!(
        "SN10 script emits {behavior} args={args:?}, weights={}/{}/{} Burst={}",
        definition.attack1, definition.attack2, definition.attack3, definition.burst
    );
    for (mode, distance) in [
        (BehaviorMode::Wait, 5.0),
        (BehaviorMode::Dodge, 5.0),
        (BehaviorMode::Dodge, 40.0),
    ] {
        let (mut h, target) = spawn_npc_combat_harness_with_order(8, [10.0, 40.0, 30.0], true);
        crate::plugins::character_plugin::spawn_fighter_model_for_test(
            h.app.world_mut(),
            h.fighter,
        );
        let schedule = super::super::fighter_frame::schedule(&h.app);
        h.app.add_systems(
            schedule,
            crate::plugins::character_plugin::drive_fighter_animation_for_test
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        );
        h.app.world_mut().entity_mut(h.fighter).insert(MissionNpc {
            id: boss,
            name: definition.name.clone(),
            team: 2,
            in_scene_roster: true,
            view: definition.view as f32,
            think: definition.think as f32,
            forced_behavior: Some(mode),
            is_vip: false,
            attack_target_slot: None,
            behavior_args: args.clone(),
            attack_target_remaining: 0,
        });
        if mode == BehaviorMode::Dodge {
            let mut npc = h.app.world().get::<MissionNpc>(h.fighter).unwrap().clone();
            npc.forced_behavior = Some(BehaviorMode::Wait);
            npc.behavior_args.clear();
            let intent = h.app.world().get::<AiIntent>(h.fighter).unwrap().clone();
            h.app
                .world_mut()
                .get_mut::<NpcPatrol>(h.fighter)
                .unwrap()
                .prepare_behavior_change(Some(&npc), &intent, Some(mode));
        }
        *h.app
            .world_mut()
            .get_mut::<NpcCombatProfile>(h.fighter)
            .unwrap() = NpcCombatProfile {
            attack1: definition.attack1 as f32,
            attack2: definition.attack2 as f32,
            attack3: definition.attack3 as f32,
            guard: definition.guard as f32,
            dodge: definition.dodge as f32,
            jump: definition.jump as f32,
            look: definition.look as f32,
            burst: definition.burst as f32,
            aim: definition.aim as f32,
            get_item: definition.get_item as f32,
        };
        h.app.world_mut().get_mut::<Health>(target).unwrap().current = 1_000_000.0;
        let mut attack_ticks = 0;
        let mut enemy_ticks = 0;
        let mut moving_ticks = 0;
        for _ in 0..1200 {
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = h.position() + Vec3::Z * distance;
            h.step_dt(1.0 / 60.0);
            attack_ticks += usize::from(is_attacking_state(h.state()));
            let intent = h.app.world().get::<AiIntent>(h.fighter).unwrap();
            enemy_ticks += usize::from(intent.enemy_id.is_some());
            moving_ticks += usize::from(intent.move_dir.length_squared() > 0.01);
        }
        assert_eq!(
            h.app
                .world()
                .get::<MissionNpc>(h.fighter)
                .unwrap()
                .forced_behavior,
            Some(BehaviorMode::Wait),
            "temporary dodge must return to authored wait"
        );
        if distance < 10.0 {
            assert!(attack_ticks > 0, "boss must resume actual attacks");
            assert!(enemy_ticks > 0);
        }
        eprintln!("mode={mode:?} distance={distance} seconds=20 attack_ticks={attack_ticks} enemy_ticks={enemy_ticks} moving_ticks={moving_ticks} final_behavior={:?}", h.app.world().get::<MissionNpc>(h.fighter).unwrap().forced_behavior);
    }
}
