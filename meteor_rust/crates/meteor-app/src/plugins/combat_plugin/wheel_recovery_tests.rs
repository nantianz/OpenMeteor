use super::*;

#[test]
fn attack_loop_release_respects_controller_and_strategy_gates() {
    let h = spawn_gameplay_harness(2).expect("original wheel resources");
    let mut base = PoseRuntime::idle();
    h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut base, 3, true);
    let mut sm = SmState {
        current: FighterState::Attack1,
        ..default()
    };
    for (wheel_out, contact, strategy, releases) in [
        (true, true, 6, false),
        (false, false, 6, true),
        (false, false, 1, false),
        (false, true, 1, true),
    ] {
        let mut pose = base.clone();
        release_attack_pose_loop(
            &mut pose,
            &EntryBlend::default(),
            &sm,
            strategy,
            wheel_out,
            contact,
        );
        assert_eq!(!pose.loops, releases);
        if releases {
            assert_eq!(pose.frame, pose.loop_end as f32);
        }
    }
    let blend = EntryBlend {
        pose_id: 3,
        duration: 0.12,
        ..default()
    };
    let mut pose = base.clone();
    release_attack_pose_loop(&mut pose, &blend, &sm, 6, false, true);
    assert!(pose.loops, "entry morph is state5, not state0");
    sm.attack_queued = Some(QueuedAttack {
        source_pose_id: 3,
        next_pose_id: 218,
        last_evaluated_frame: 7342,
        transition_time: 0.12,
        anger_cost: 0,
        action_id: 6,
    });
    release_attack_pose_loop(&mut pose, &EntryBlend::default(), &sm, 6, false, true);
    assert!(pose.loops, "registered successor is state2, not state0");
    sm.attack_queued = None;
    sm.current = FighterState::Name;
    release_attack_pose_loop(&mut pose, &EntryBlend::default(), &sm, 6, false, true);
    assert!(pose.loops, "ordinary idle keeps looping");
    sm.current = FighterState::Attack1;
    pose.frame = (pose.loop_start - 1) as f32;
    release_attack_pose_loop(&mut pose, &EntryBlend::default(), &sm, 6, false, true);
    assert!(pose.loops, "lead-in has not reached the loop");

    pose.frame = pose.loop_end as f32 + 0.9;
    release_attack_pose_loop(&mut pose, &EntryBlend::default(), &sm, 6, false, true);
    assert!(!pose.loops);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn production_wheel_ultimate_real_projectile_returns_and_unlocks() {
    use bevy::ecs::system::RunSystemOnce;
    for fps in [30, 60, 120] {
        let (mut h, _, _) = spawn_local_stock_hit_harness_with_order(2, true);
        h.app.insert_resource(NetState::default());
        h.app
            .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>();
        h.app
            .world_mut()
            .run_system_once(load_combat_resource_data)
            .unwrap();
        h.app.world_mut().get_mut::<Qi>(h.fighter).unwrap().current = 100.0;

        let keys = h.app.world().resource::<crate::app_state::Keymap>().clone();
        {
            let mut stream = h.app.world_mut().resource_mut::<SyntheticInputStream>();
            for binding in [keys.move_down, keys.move_up, keys.move_up] {
                stream.push_mrc_direction(binding, true, None);
                stream.push_mrc_direction(binding, false, None);
            }
        }
        h.mobile_binding(
            keys.attack,
            true,
            Some(crate::plugins::input_plugin::SyntheticAction::Attack),
        );
        h.step_dt(1.0 / fps as f32);
        h.mobile_binding(
            keys.attack,
            false,
            Some(crate::plugins::input_plugin::SyntheticAction::Attack),
        );
        assert_eq!(h.pose_id(), 224, "{fps} FPS ultimate command");
        assert_eq!(h.app.world().get::<Qi>(h.fighter).unwrap().current, 0.0);
        let mut flew = false;
        let mut linked = false;
        let mut returned = false;
        let mut generated = std::collections::HashSet::new();
        for _ in 0..fps * 8 {
            h.step_dt(1.0 / fps as f32);
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            let world = h.app.world_mut();
            generated.extend(
                world
                    .query::<(Entity, &Projectile)>()
                    .iter(world)
                    .map(|(id, _)| id),
            );
            h.app
                .world_mut()
                .run_system_once(crate::plugins::ranged_plugin::update_projectiles)
                .unwrap();
            let out = h
                .app
                .world()
                .get::<RangedFireGate>(h.fighter)
                .unwrap()
                .wheel_out;
            flew |= out;
            linked |= h.pose_id() == 3;
            returned |= flew && !out;
            if returned && h.state() == FighterState::Name {
                break;
            }
        }
        assert!(
            flew && linked && returned,
            "{fps} FPS: emission, 224 -> 3 and actual return"
        );
        assert_eq!(generated.len(), 1, "one ultimate must emit one wheel");
        assert_eq!(
            h.state(),
            FighterState::Name,
            "{fps} FPS: return unlocks attack state"
        );
        h.press_binding(keys.move_up);
        let position = h.position();
        for _ in 0..3 {
            h.step_dt(1.0 / fps as f32);
        }
        assert!(h.position().distance(position) > 0.0, "movement resumes");
    }
}

#[test]
fn production_wheel_ultimate_releases_linked_loop_after_return() {
    for fps in [30, 60, 120] {
        for npc in [false, true] {
            let mut h =
                spawn_gameplay_harness_with_order(2, true).expect("original wheel resources");
            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                h.app.world_mut(),
                h.fighter,
            );
            crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
            if npc {
                h.app
                    .world_mut()
                    .entity_mut(h.fighter)
                    .remove::<LocalFighter>();
            }
            let character = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
            assert_eq!(character.poses[&224].link, Some(3));
            assert!(character.poses[&3].loops);
            let mut pose = PoseRuntime::idle();
            character.apply_pose(&mut pose, 224, true);
            let instance = pose.pose_instance_id;
            *h.app.world_mut().get_mut::<PoseRuntime>(h.fighter).unwrap() = pose;
            {
                let mut sm = h.app.world_mut().get_mut::<SmState>(h.fighter).unwrap();
                sm.current = FighterState::Attack1;
                sm.attack_stage = 1;
                sm.locked = true;
            }
            h.app
                .world_mut()
                .get_mut::<RangedFireGate>(h.fighter)
                .unwrap()
                .wheel_out = true;
            for _ in 0..fps * 5 {
                h.step_dt(1.0 / fps as f32);
            }
            assert_eq!(h.pose_id(), 3, "{fps} FPS npc={npc}: authored 224 -> 3");
            assert_eq!(h.state(), FighterState::Attack1, "wait for wheel return");
            let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
            assert!(pose.loops);
            assert_eq!(
                pose.pose_instance_id, instance,
                "natural Link keeps instance"
            );
            h.app
                .world_mut()
                .get_mut::<RangedFireGate>(h.fighter)
                .unwrap()
                .wheel_out = false;
            for _ in 0..fps {
                h.step_dt(1.0 / fps as f32);
                if h.state() == FighterState::Name {
                    break;
                }
            }
            assert_eq!(
                h.state(),
                FighterState::Name,
                "{fps} FPS npc={npc}: return must release Attack1"
            );
            assert!(h
                .app
                .world()
                .get::<SmState>(h.fighter)
                .unwrap()
                .linked_recovery_pose_id
                .is_none());

            if npc {
                h.app
                    .world_mut()
                    .get_mut::<AiIntent>(h.fighter)
                    .unwrap()
                    .want_attack = true;
            } else {
                h.press_mouse(MouseButton::Left);
            }
            h.step_dt(1.0 / fps as f32);
            assert!(
                is_attacking_state(h.state()),
                "{fps} FPS npc={npc}: fresh attack"
            );
            assert_eq!(h.pose_id(), 218);
        }
    }
}
