#[test]
fn production_fake_pushed_by_remote_stops_then_jumps_or_recovers_without_launching() {
    for serial in [false, true] {
        for fps in [30, 60, 120] {
            for exit in ["jump", "roll", "getup"] {
                let mut h = recovery_timing_harness(serial);
                let schedule = super::super::fighter_frame::schedule(&h.app);
                h.app.add_systems(
                    schedule,
                    hit_reaction_root_motion.in_set(crate::plugins::FighterFrameSet::RootMotion),
                );
                h.app
                    .insert_resource(State::new(AppState::Playing))
                    .add_plugins(crate::plugins::CharacterContactPlugin);
                let dt = 1.0 / fps as f32;
                let keys = h.app.world().resource::<crate::app_state::Keymap>().clone();
                h.press_binding(keys.fake);
                h.step_dt(dt);
                h.release_binding(keys.fake);
                for _ in 0..(fps * 3) {
                    h.step_dt(dt);
                }
                assert_eq!(h.state(), FighterState::Fake);
                let initial_peer = h.position() - Vec3::Z * 1.8;
                let peer = h
                    .app
                    .world_mut()
                    .spawn((
                        Fighter {
                            character_id: 1,
                            player_slot: 1,
                            weapon: DAGGER,
                            weapon_slots: [DAGGER, 0],
                            weapon_slot: 0,
                            weapon_slot_variants: [0, 0],
                            weapon_variant: 0,
                        },
                        Transform::from_translation(initial_peer),
                        PoseRuntime::idle(),
                        SmState::default(),
                        Health::default(),
                        MotionState {
                            velocity: Vec3::ZERO,
                            facing: 0.0,
                            movement_mode: meteor_core::MovementMode::Idle,
                        },
                        crate::plugins::network_plugin::RemoteFighter {
                            player_id: "pusher".into(),
                        },
                    ))
                    .id();

                let start = h.position();
                for _ in 0..fps {
                    let peer_position = h.position() - Vec3::Z * 1.8;
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(peer)
                        .unwrap()
                        .translation = peer_position;
                    h.step_dt(dt);
                    let speed = h
                        .app
                        .world()
                        .get::<MotionState>(h.fighter)
                        .unwrap()
                        .velocity
                        .length();
                    assert!(
                        speed < 4.0,
                        "{serial}/{fps}/{exit}: accumulated push {speed}"
                    );
                }
                assert!(
                    (h.position() - start).length() > 0.005,
                    "must actually be pushed"
                );
                h.app
                    .world_mut()
                    .get_mut::<Transform>(peer)
                    .unwrap()
                    .translation = Vec3::splat(100.0);
                for _ in 0..3 {
                    h.step_dt(dt);
                }
                assert!(
                    h.app
                        .world()
                        .get::<MotionState>(h.fighter)
                        .unwrap()
                        .velocity
                        .length()
                        < 0.001
                );
                match exit {
                    "jump" => {
                        h.press_binding(keys.jump);
                        h.step_dt(dt);
                        h.release_binding(keys.jump);
                        h.step_dt(dt);
                        assert!(h.airborne(), "Fake must still accept Jump");
                    }
                    "roll" => {
                        h.press_key(KeyCode::KeyW);
                        h.step_dt(dt);
                        h.release_key(KeyCode::KeyW);
                        let dodge = h.app.world().get::<FighterDodge>(h.fighter).unwrap();
                        assert!(dodge.state.active);
                        assert!(
                            dodge.state.residual_x.abs() + dodge.state.residual_z.abs() < 0.001
                        );
                        continue;
                    }
                    _ => {
                        h.press_binding(keys.guard);
                        h.step_dt(dt);
                        h.release_binding(keys.guard);
                        assert_eq!(h.pose_id(), 35);
                        for _ in 0..(fps * 3) {
                            h.step_dt(dt);
                        }
                        h.press_binding(keys.jump);
                        h.step_dt(dt);
                        h.release_binding(keys.jump);
                        h.step_dt(dt);
                        if !h.airborne() {
                            h.trace(&format!("getup-jump-failed-{serial}-{fps}"));
                        }
                        assert!(h.airborne(), "completed GetUp must accept Jump");
                    }
                }
                let jump_start = h.position();
                for _ in 0..fps {
                    h.step_dt(dt);
                }
                let delta = h.position() - jump_start;
                assert!(
                    Vec2::new(delta.x, delta.z).length() < 0.1,
                    "{serial}/{fps}/{exit}: idle jump flew sideways {delta:?}"
                );
            }
        }
    }
}

fn fake_test_visual_root(h: &mut GameplayHarness) -> Vec3 {
    use crate::plugins::character_plugin::{FighterDummy, FighterModel};
    let mut query = h.app.world_mut().query::<(&FighterDummy, &Transform)>();
    query
        .iter(h.app.world())
        .find_map(|(dummy, transform)| {
            (dummy.name == "d_base"
                && h.app
                    .world()
                    .get::<FighterModel>(dummy.model)
                    .unwrap()
                    .fighter
                    == h.fighter)
                .then_some(transform.translation)
        })
        .expect("sampled local d_base")
}

#[test]
fn production_fake_takeoff_keeps_capsule_and_visual_motion_root_continuous() {
    for fps in [30, 60, 120] {
        for pushed in [false, true] {
            let mut h = recovery_timing_harness(true);
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert(crate::components::fighter::VisualRootPin::default());
            h.app
                .insert_resource(State::new(AppState::Playing))
                .add_plugins(crate::plugins::CharacterContactPlugin);
            let dt = 1.0 / fps as f32;
            let keys = h.app.world().resource::<crate::app_state::Keymap>().clone();
            let expected = {
                let asset = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
                let common = &h.app.world().resource::<CommonAmb>().0;
                let pose = &asset.poses[&117];
                let a = asset.motion_root_at(common, 117, pose.start as f32);
                let b = asset.motion_root_at(common, 117, pose.end as f32);
                let (x, z) = meteor_anim::root_motion::root_delta_to_world_xz(
                    b[0] - a[0],
                    b[1] - a[1],
                    0.0,
                    1.0,
                );
                Vec3::new(x, 0.0, z)
            };
            assert!(expected.length() > 2.0, "exercise nonzero original motion");
            h.press_binding(keys.fake);
            h.step_dt(dt);
            h.release_binding(keys.fake);
            for _ in 0..fps * 3 {
                h.step_dt(dt);
                assert!(
                    fake_test_visual_root(&mut h).truncate().length() < 1e-5,
                    "Fake must not leave physical displacement on the rendered dummy"
                );
            }
            assert!((h.position()-expected).length() < 0.15,
                "{fps}: original Fake motion must reach the collision/network body: {:?} vs {expected:?}", h.position());
            if pushed {
                let initial = h.position() - Vec3::Z * 1.8;
                let peer = h
                    .app
                    .world_mut()
                    .spawn((
                        Fighter {
                            character_id: 1,
                            player_slot: 1,
                            weapon: 6,
                            weapon_slots: [6, 0],
                            weapon_slot: 0,
                            weapon_slot_variants: [0, 0],
                            weapon_variant: 0,
                        },
                        Transform::from_translation(initial),
                        PoseRuntime::idle(),
                        SmState::default(),
                        Health::default(),
                        MotionState {
                            velocity: Vec3::ZERO,
                            facing: 0.0,
                            movement_mode: meteor_core::MovementMode::Idle,
                        },
                        crate::plugins::network_plugin::RemoteFighter {
                            player_id: "pusher".into(),
                        },
                    ))
                    .id();
                for _ in 0..fps {
                    let at = h.position() - Vec3::Z * 1.8;
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(peer)
                        .unwrap()
                        .translation = at;
                    h.step_dt(dt);
                }
            }
            assert!(fake_test_visual_root(&mut h).truncate().length() < 1e-5);
            h.press_binding(keys.jump);
            h.step_dt(dt);
            let before = h.position();
            let v = h
                .app
                .world()
                .get::<MotionState>(h.fighter)
                .unwrap()
                .velocity;
            h.release_binding(keys.jump);
            h.step_dt(dt);
            let after = h.position();
            assert!(h.airborne());
            assert!(fake_test_visual_root(&mut h).truncate().length() < 1e-5);
            let delta = after - before;
            eprintln!("fake takeoff {fps}Hz pushed={pushed} before={before:?} after={after:?} velocity={v:?}");
            assert!(Vec2::new(delta.x, delta.z).length() <= v.length() * dt + 0.15);
        }
    }
}

#[test]
fn production_fake_authored_motion_respects_the_stage_wall() {
    let mut h = recovery_timing_harness(true);
    h.app
        .world_mut()
        .entity_mut(h.fighter)
        .insert(crate::components::fighter::VisualRootPin::default());
    h.replace_stage_mesh(harness_ground_with_front_wall(1.0));
    let keys = h.app.world().resource::<crate::app_state::Keymap>().clone();
    h.press_binding(keys.fake);
    h.step();
    h.release_binding(keys.fake);
    for _ in 0..90 {
        h.step();
        assert!(
            h.position().z < 1.0,
            "Fake root must pass through the stage mover"
        );
        assert!(fake_test_visual_root(&mut h).truncate().length() < 1e-5);
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_fake_with_default_sword_npc_keeps_takeoff_origin() {
    for fps in [30, 60, 120] {
        let (mut h, npc, _) = spawn_local_stock_hit_harness_with_order(6, true);
        h.app
            .insert_resource(NetState::default())
            .insert_resource(State::new(AppState::Playing))
            .add_plugins(crate::plugins::CharacterContactPlugin);
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert(crate::components::fighter::VisualRootPin::default());
        h.app.world_mut().entity_mut(npc).insert((
            NpcFighter,
            AiIntent::default(),
            crate::plugins::ai_plugin::NpcPatrol::default(),
            NpcPerformance {
                pause_remaining: f32::INFINITY,
                ..default()
            },
            NpcCombatProfile::web_debug(),
            crate::components::fighter::VisualRootPin::default(),
        ));
        let schedule = crate::plugins::fighter_frame::schedule(&h.app);
        h.app.add_systems(
            Update,
            tick_target_hit_clocks.in_set(crate::plugins::FighterFrameSet::PoseDecision),
        );
        h.app.add_systems(
            schedule,
            crate::plugins::character_plugin::drive_fighter_animation_for_test
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        );
        h.app.add_systems(
            schedule,
            attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
        );
        h.app.add_systems(
            schedule,
            hit_reaction_root_motion
                .after(crate::plugins::movement_plugin::attack_root_motion)
                .in_set(crate::plugins::FighterFrameSet::RootMotion),
        );
        let keys = h.app.world().resource::<crate::app_state::Keymap>().clone();
        let dt = 1.0 / fps as f32;
        h.press_binding(keys.fake);
        h.step_dt(dt);
        h.release_binding(keys.fake);
        for _ in 0..fps * 3 {
            h.step_dt(dt);
        }
        let at = h.position() + Vec3::Z * 2.0;
        h.app
            .world_mut()
            .get_mut::<Transform>(npc)
            .unwrap()
            .translation = at;
        h.app
            .world_mut()
            .get_mut::<NpcPerformance>(npc)
            .unwrap()
            .pause_remaining = 0.0;
        let mut sword_attack = false;
        let mut fake_frames = 0;
        for _ in 0..fps * 3 {
            h.step_dt(dt);
            let pose = h.app.world().get::<PoseRuntime>(npc).unwrap().pose_id;
            sword_attack |= (260..=299).contains(&pose);
            if h.state() == FighterState::Fake {
                fake_frames += 1;
                assert!(fake_test_visual_root(&mut h).truncate().length() < 1e-5);
            }
        }
        assert!(
            sword_attack && fake_frames > 0,
            "must exercise default sword NPC attacks against Fake"
        );
        h.app
            .world_mut()
            .get_mut::<NpcPerformance>(npc)
            .unwrap()
            .pause_remaining = f32::INFINITY;

        h.app
            .world_mut()
            .get_mut::<Transform>(npc)
            .unwrap()
            .translation = Vec3::splat(100.0);
        let mut jumped = false;
        for _ in 0..fps * 5 {
            h.press_binding(keys.jump);
            h.step_dt(dt);
            let before = h.position();
            h.release_binding(keys.jump);
            h.step_dt(dt);
            if h.airborne() {
                let delta = h.position() - before;
                assert!(
                    Vec2::new(delta.x, delta.z).length() < 0.5,
                    "{fps}: sword NPC -> Jump teleported {delta:?}"
                );
                assert!(fake_test_visual_root(&mut h).truncate().length() < 1e-5);
                jumped = true;
                break;
            }
        }
        assert!(
            jumped,
            "must reach a real jump after the NPC attack sequence"
        );
    }
}
