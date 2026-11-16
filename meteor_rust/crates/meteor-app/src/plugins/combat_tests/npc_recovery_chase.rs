fn npc_decision_seed(choice: u32) -> u64 {
    (1..100_000_u64)
        .find(|seed| {
            let mut x = *seed;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            ((x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as u32) % 8 == choice
        })
        .unwrap()
}

#[test]
fn production_npc_close_gun_switch_uses_marker_and_releases_aim() {
    for fps in [30, 60, 120] {
        let (mut h, target) = spawn_npc_combat_harness_with_order(3, [0.0; 3], true);
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
        {
            let mut fighter = h.app.world_mut().get_mut::<Fighter>(h.fighter).unwrap();
            fighter.weapon_slots = [3, 6];
            fighter.weapon_slot_variants = [0, 2];
        }
        h.app
            .world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::Z * 5.0;
        h.app
            .world_mut()
            .get_mut::<GunAimState>(h.fighter)
            .unwrap()
            .aiming = true;
        let mut started = false;
        let mut committed = false;
        for _ in 0..fps * 5 {
            h.step_dt(1.0 / fps as f32);
            let fighter = h.app.world().get::<Fighter>(h.fighter).unwrap();
            if let Some(switch) = h.app.world().get::<WeaponSwitchActive>(h.fighter) {
                started = true;
                assert_eq!(switch.pose_id, 24);
                assert_eq!(
                    h.pose_id(),
                    24,
                    "AI must not interrupt its switch with an attack"
                );
                if !switch.committed {
                    assert_eq!(fighter.weapon, 3, "inventory changes only at the marker");
                }
            }
            if fighter.weapon == 6 {
                committed = true;
                assert_eq!(fighter.weapon_variant, 2);
                assert_eq!(fighter.weapon_slot, 1);
            }
        }
        assert!(
            started && committed,
            "fps={fps} started={started} committed={committed} pose={:?} switch={:?}",
            h.app.world().get::<PoseRuntime>(h.fighter),
            h.app.world().get::<WeaponSwitchActive>(h.fighter)
        );
        assert!(h.app.world().get::<WeaponSwitchActive>(h.fighter).is_none());
        assert!(!h.app.world().get::<GunAimState>(h.fighter).unwrap().aiming);

        h.app
            .world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::Z * 20.0;
        h.steps(30);
        assert_eq!(h.app.world().get::<Fighter>(h.fighter).unwrap().weapon, 6);
        if fps == 30 {
            let mut switched_back = false;
            for _ in 0..fps * 24 {
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = h.position() + Vec3::Z * 20.0;
                h.step_dt(1.0 / fps as f32);
                if h.app.world().get::<Fighter>(h.fighter).unwrap().weapon == 3 {
                    switched_back = true;
                    break;
                }
            }
            assert!(
                switched_back,
                "out-of-range melee must eventually return to the carried gun"
            );
        }
    }
}

#[test]
fn production_npc_weapon_switch_rejects_empty_slot_and_hit_interrupts_marker() {
    for spare in [0, 6] {
        let (mut h, target) = spawn_npc_combat_harness_with_order(3, [0.0; 3], true);
        h.app
            .world_mut()
            .get_mut::<Fighter>(h.fighter)
            .unwrap()
            .weapon_slots = [3, spare];
        h.app
            .world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::Z * 5.0;
        for _ in 0..60 {
            h.step();
            if h.app.world().get::<WeaponSwitchActive>(h.fighter).is_some() {
                break;
            }
        }
        if spare == 0 {
            assert!(h.app.world().get::<WeaponSwitchActive>(h.fighter).is_none());
        } else {
            assert!(h.app.world().get::<WeaponSwitchActive>(h.fighter).is_some());
            install_timing_reaction(&mut h, 93, HitReactKind::BeHit, 0.8);
            h.step();
            assert!(h.app.world().get::<WeaponSwitchActive>(h.fighter).is_none());
            h.steps(10);
            assert_eq!(h.state(), FighterState::BeHit);
        }
        assert_eq!(h.app.world().get::<Fighter>(h.fighter).unwrap().weapon, 3);
    }
}

#[test]
fn production_npc_natural_recovery_audit_all_knockdown_chains() {
    let mut failures = Vec::new();
    for fps in [30, 60, 120] {
        for id in [95, 96, 97, 98, 99, 111, 112, 113, 114, 115, 118] {
            let (mut h, _) = spawn_npc_combat_harness_with_order(6, [0.0; 3], true);
            h.app.world_mut().entity_mut(h.fighter).insert((
                NpcCombatProfile::web_debug(),
                MissionNpc {
                    id: -1,
                    name: "DebugNPC_1".into(),
                    team: 0,
                    in_scene_roster: false,
                    view: 200.0,
                    think: 50.0,
                    forced_behavior: Some(meteor_ai::behavior::BehaviorMode::Follow),
                    is_vip: false,
                    attack_target_slot: None,
                    behavior_args: vec![meteor_scene::pst_runtime::PstValue::Num(0.0)],
                    attack_target_remaining: 0,
                },
            ));
            {
                let mut net = h.app.world_mut().resource_mut::<NetState>();
                net.status = NetStatus::InRoom;
                net.address = "udp-host://127.0.0.1:0".into();
                net.mode = "leader".into();
                net.stock_npc_actors.insert(h.fighter, 2);
            }
            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                h.app.world_mut(),
                h.fighter,
            );
            let schedule = super::super::fighter_frame::schedule(&h.app);
            h.app.add_systems(
                schedule,
                crate::plugins::character_plugin::drive_fighter_animation_for_test
                    .before(harness_retire_entry_blend)
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            );
            install_timing_reaction(&mut h, id, HitReactKind::BeHit, 0.8);
            let mut recovered = false;
            for frame in 0..fps * 12 {
                h.step_dt(1.0 / fps as f32);
                if h.state() == FighterState::Name {
                    eprintln!(
                        "natural recovery: initial={id} fps={fps} seconds={:.3}",
                        (frame + 1) as f32 / fps as f32
                    );
                    recovered = true;
                    break;
                }
            }
            if !recovered {
                failures.push(format!(
                    "initial={id} fps={fps} state={:?} pose={:?} reaction={:?}",
                    h.state(),
                    h.app.world().get::<PoseRuntime>(h.fighter),
                    h.app.world().get::<FighterHitReaction>(h.fighter)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn production_npc_down_roll_bypasses_all_weapon_tables_and_banks_carry() {
    use bevy::ecs::system::RunSystemOnce;
    for weapon in 1..=12 {
        let (mut h, _) = spawn_npc_combat_harness_with_order(weapon, [0.0; 3], true);
        for (pose_id, state) in [
            (112, FighterState::Down),
            (113, FighterState::Down),
            (117, FighterState::Fake),
        ] {
            for (direction, expected) in [
                (Vec2::Y, 176),
                (Vec2::NEG_Y, 179),
                (Vec2::X, 177),
                (Vec2::NEG_X, 178),
            ] {
                install_timing_reaction(&mut h, pose_id, HitReactKind::Down, 0.0);
                h.app
                    .world_mut()
                    .get_mut::<SmState>(h.fighter)
                    .unwrap()
                    .current = state;
                h.app
                    .world_mut()
                    .get_mut::<CombatState>(h.fighter)
                    .unwrap()
                    .state = state;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .velocity = Vec3::new(2.0, 0.0, -3.0);
                *h.app
                    .world_mut()
                    .get_mut::<FighterDodge>(h.fighter)
                    .unwrap() = FighterDodge::default();
                {
                    let mut intent = h.app.world_mut().get_mut::<AiIntent>(h.fighter).unwrap();
                    intent.dodge_burst = Some(false);
                    intent.dodge_dir = direction;
                }
                h.app
                    .world_mut()
                    .run_system_once(crate::plugins::ai_plugin::move_npc_fighters)
                    .unwrap();
                let dodge = h.app.world().get::<FighterDodge>(h.fighter).unwrap();
                assert_eq!(h.pose_id(), expected, "weapon={weapon} source={pose_id}");
                assert_eq!(
                    h.state(),
                    FighterState::Dodge,
                    "roll remains mode6, not QiBurst"
                );
                assert!(dodge.state.active && dodge.state.burst);
                assert_eq!(
                    (dodge.state.residual_x, dodge.state.residual_z),
                    (2.0, -3.0)
                );
                assert!(h
                    .app
                    .world()
                    .get::<FighterHitReaction>(h.fighter)
                    .unwrap()
                    .current
                    .is_none());
            }
        }
    }
}

#[test]
fn production_npc_down_recovery_uses_next_existing_think_without_repeating_stun() {
    for fps in [30, 60, 120] {
        for choice in 0..8 {
            let (mut h, _) = spawn_npc_combat_harness_with_order(6, [0.0; 3], true);

            {
                let mut net = h.app.world_mut().resource_mut::<NetState>();
                net.status = NetStatus::InRoom;
                net.address = "udp-host://127.0.0.1:0".into();
                net.mode = "leader".into();
                net.stock_npc_actors.insert(h.fighter, 2);
            }
            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                h.app.world_mut(),
                h.fighter,
            );
            let schedule = super::super::fighter_frame::schedule(&h.app);
            h.app.add_systems(
                schedule,
                crate::plugins::character_plugin::drive_fighter_animation_for_test
                    .before(harness_retire_entry_blend)
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            );

            install_timing_reaction(&mut h, 95, HitReactKind::BeHit, 0.8);
            for _ in 0..fps * 4 {
                h.step_dt(1.0 / fps as f32);
                if h.state() == FighterState::Down {
                    break;
                }
            }
            assert_eq!(h.state(), FighterState::Down, "fps={fps}");
            let reaction = h
                .app
                .world()
                .get::<FighterHitReaction>(h.fighter)
                .unwrap()
                .current
                .as_ref()
                .unwrap();
            assert_eq!(
                reaction.down_remaining, 0.0,
                "authored stun was already consumed"
            );
            let pose_instance = h
                .app
                .world()
                .get::<PoseRuntime>(h.fighter)
                .unwrap()
                .pose_instance_id;
            h.app
                .world_mut()
                .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
                .unwrap()
                .set_decision_for_test(0.12, npc_decision_seed(choice));
            h.step_dt(0.05);
            assert_eq!(h.state(), FighterState::Down, "no premature decision");
            assert_eq!(
                h.app
                    .world()
                    .get::<PoseRuntime>(h.fighter)
                    .unwrap()
                    .pose_instance_id,
                pose_instance
            );
            h.step_dt(0.05);
            assert_eq!(h.state(), FighterState::Down);
            h.step_dt(0.03);
            match choice {
                0..=3 => {
                    assert_eq!(h.state(), FighterState::Dodge, "roll={choice}, fps={fps}");
                    assert_eq!(
                        h.pose_id(),
                        [176, 179, 178, 177][choice as usize],
                        "mode11 must bypass weapon dodge remapping"
                    );
                    assert!(
                        h.app
                            .world()
                            .get::<FighterDodge>(h.fighter)
                            .unwrap()
                            .state
                            .burst,
                        "roll uses the common roll motion policy"
                    );
                    assert!(
                        h.app
                            .world()
                            .get::<FighterHitReaction>(h.fighter)
                            .unwrap()
                            .current
                            .is_none(),
                        "accepted roll releases the old reaction owner"
                    );
                    assert!(
                        h.app
                            .world()
                            .get::<FighterDodge>(h.fighter)
                            .unwrap()
                            .state
                            .active
                    );
                }
                4 => {
                    assert_eq!(h.state(), FighterState::BeHit, "attack must get up");
                    assert!(matches!(h.pose_id(), 119 | 120));
                }
                _ => {
                    assert_eq!(h.state(), FighterState::Down, "original wait outcome");
                    h.app
                        .world_mut()
                        .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
                        .unwrap()
                        .set_decision_for_test(0.0, npc_decision_seed(4));
                    h.step_dt(1.0 / fps as f32);
                    assert_eq!(h.state(), FighterState::BeHit, "next pulse retries");
                }
            }
            for _ in 0..fps * 3 {
                h.step_dt(1.0 / fps as f32);
            }
            assert!(!matches!(
                h.state(),
                FighterState::Down | FighterState::BeHit | FighterState::Dodge
            ));
        }
    }
}

fn npc_chase_fixture_graph() -> meteor_scene::stage_loader::WaypointGraph {
    meteor_scene::stage_loader::parse_waypoint_graph(
        "WayPoints 2\nPos 0 0 30\nSize 10\nLink 1\n1 0 500\nPos 0 -500 30\nSize 10\nLink 1\n0 0 500\n",
        [0.0; 3],
    )
}

#[test]
fn production_npc_chase_stops_for_parent_handoff_then_resumes_real_movement() {
    for fps in [30, 60, 120] {
        let (mut h, target) = spawn_npc_combat_harness_with_order(6, [0.0; 3], true);
        h.app.insert_resource(npc_chase_fixture_graph());
        h.app
            .world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(0.0, 0.0, 20.0);
        h.app
            .world_mut()
            .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
            .unwrap()
            .set_decision_for_test(0.0, 17);
        let initial = h.position();
        h.step_dt(1.0 / fps as f32);
        assert!(
            h.app
                .world()
                .get::<AiIntent>(h.fighter)
                .unwrap()
                .move_dir
                .length()
                > 0.9
        );
        assert!(
            h.position().z > initial.z,
            "initial route must reach the real mover"
        );
        let handoff_seed = (1..100_000_u64)
            .find(|seed| {
                let mut x = *seed;
                x ^= x >> 12;
                x ^= x << 25;
                x ^= x >> 27;
                ((x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as u32) % 30 == 0
            })
            .unwrap();
        h.app
            .world_mut()
            .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
            .unwrap()
            .set_decision_for_test(0.0, handoff_seed);
        let before_pop = h.position();
        h.step_dt(1.0 / fps as f32);
        let intent = h.app.world().get::<AiIntent>(h.fighter).unwrap();
        assert_eq!(intent.enemy_id, Some(0));
        assert_eq!(
            intent.move_dir,
            Vec2::ZERO,
            "503e1d pop stops the path at {fps} FPS"
        );
        assert!(
            (h.position() - before_pop).length() < 0.001,
            "the real mover must stop on pop"
        );
        h.step_dt(1.0 / fps as f32);
        assert_eq!(
            h.app.world().get::<AiIntent>(h.fighter).unwrap().move_dir,
            Vec2::ZERO,
            "the parent is not called again before Think"
        );
        h.app
            .world_mut()
            .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
            .unwrap()
            .set_decision_for_test(0.0, 17);
        let mut resumed = false;
        for _ in 0..fps {
            h.step_dt(1.0 / fps as f32);
            let intent = h.app.world().get::<AiIntent>(h.fighter).unwrap();
            assert_eq!(intent.enemy_id, Some(0));
            resumed |= intent.move_dir.length() > 0.9 && h.position().z > before_pop.z;
        }
        assert!(
            resumed,
            "parent callback must restore an effective route at {fps} FPS"
        );
    }
}

#[test]
fn production_npc_chases_outside_weapon_radius_and_across_height_difference() {
    for offset in [Vec3::new(0.0, 0.0, 7.8), Vec3::new(0.0, 8.0, 3.0)] {
        let (mut h, target) = spawn_npc_combat_harness_with_order(6, [0.0; 3], true);
        h.app.insert_resource(npc_chase_fixture_graph());

        h.step();
        h.app
            .world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = h.position() + offset;
        h.app
            .world_mut()
            .get_mut::<NpcCombatProfile>(h.fighter)
            .unwrap()
            .attack1 = 100.0;
        h.app
            .world_mut()
            .get_mut::<AiIntent>(h.fighter)
            .unwrap()
            .attack_cooldown = 0.0;

        h.app
            .world_mut()
            .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
            .unwrap()
            .set_decision_for_test(0.0, 17);
        h.step();
        let intent = h.app.world().get::<AiIntent>(h.fighter).unwrap();
        if offset.y == 0.0 {
            assert!(
                intent.move_dir.length() > 0.9,
                "out-of-range target must resume chase: {offset:?}"
            );
        }

        assert!(
            h.state() == FighterState::Name
                || (h.state() == FighterState::Attack1 && h.pose_id() == 32),
            "out-of-range action must be locomotion or the original taunt: {offset:?}"
        );
        assert!(!intent.want_attack);
    }
}

#[test]
fn production_npc_down_recovery_checks_landing_and_preserves_wall_collision() {
    for (mesh, wall) in [
        (harness_ground_with_front_wall(2.0), true),
        (StageMeshCollider::default(), false),
    ] {
        let (mut h, _) = spawn_npc_combat_harness_with_order(6, [0.0; 3], true);
        h.replace_stage_mesh(mesh);
        install_timing_reaction(&mut h, 112, HitReactKind::Down, 0.0);
        h.app
            .world_mut()
            .get_mut::<SmState>(h.fighter)
            .unwrap()
            .current = FighterState::Down;
        h.app
            .world_mut()
            .get_mut::<CombatState>(h.fighter)
            .unwrap()
            .state = FighterState::Down;
        h.app
            .world_mut()
            .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
            .unwrap()
            .set_decision_for_test(0.0, npc_decision_seed(0));
        let origin = h.position();
        h.step();
        if wall {
            assert_eq!(h.state(), FighterState::Dodge);
            h.steps(30);
            assert!(
                h.position().z < 2.0,
                "accepted roll must not penetrate the wall"
            );
            continue;
        }
        assert_eq!(
            h.state(),
            FighterState::Down,
            "unsafe forward roll must wait"
        );
        assert_eq!(h.position(), origin);
        assert!(
            !h.app
                .world()
                .get::<FighterDodge>(h.fighter)
                .unwrap()
                .state
                .active
        );
        h.app
            .world_mut()
            .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
            .unwrap()
            .set_decision_for_test(0.0, npc_decision_seed(4));
        h.step();
        assert!(
            matches!(h.pose_id(), 119 | 120),
            "stationary getup remains available"
        );
    }
}

#[test]
fn production_npc_down_recovery_does_not_deadlock_queued_story_or_wake_dummy() {
    for dummy in [false, true] {
        let (mut h, _) = spawn_npc_combat_harness_with_order(6, [0.0; 3], true);
        install_timing_reaction(&mut h, 112, HitReactKind::Down, 0.0);
        h.app
            .world_mut()
            .get_mut::<SmState>(h.fighter)
            .unwrap()
            .current = FighterState::Down;
        h.app
            .world_mut()
            .get_mut::<CombatState>(h.fighter)
            .unwrap()
            .state = FighterState::Down;
        {
            let mut performance = h
                .app
                .world_mut()
                .get_mut::<NpcPerformance>(h.fighter)
                .unwrap();
            performance
                .queue
                .push_back(crate::plugins::stage_plugin::NpcPerformCommand {
                    action: "say".into(),
                    args: vec![meteor_scene::pst_runtime::PstValue::Str(
                        "after recovery".into(),
                    )],
                });
            if dummy {
                performance.pause_remaining = f32::INFINITY;
            }
        }
        h.app
            .world_mut()
            .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
            .unwrap()
            .set_decision_for_test(0.0, npc_decision_seed(4));
        h.step();
        if dummy {
            assert_eq!(h.state(), FighterState::Down);
        } else {
            assert!(
                matches!(h.pose_id(), 119 | 120),
                "pending dialogue cannot prevent getup"
            );
            assert_eq!(
                h.app
                    .world()
                    .get::<NpcPerformance>(h.fighter)
                    .unwrap()
                    .queue
                    .len(),
                1
            );
        }
    }
}
