fn dagger_ledge_harness(serial: bool, platform: bool) -> GameplayHarness {
    let mut h = spawn_gameplay_harness_with_order(DAGGER, serial).expect("original P1 resources");

    h.app
        .world_mut()
        .entity_mut(h.fighter)
        .insert(crate::components::fighter::VisualRootPin::default());
    install_guard_test_animation(&mut h);
    let mut mesh = harness_ground();
    let height = if platform { 30.0 } else { 0.0 };
    if platform {
        mesh.append_world_triangles(
            &[
                [-20.0, height, -20.0],
                [-20.0, height, 0.0],
                [20.0, height, 0.0],
                [20.0, height, -20.0],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
    }
    h.app.world_mut().resource_mut::<StageCollider>().mesh = Some(mesh);
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation = Vec3::new(0.0, height, -2.0);
    h.app
        .world_mut()
        .get_mut::<FighterAir>(h.fighter)
        .unwrap()
        .ground_y = height;
    h
}

fn start_dagger_ledge_dash(h: &mut GameplayHarness, dt: f32) {
    h.step_dt(dt);
    h.press_key(KeyCode::KeyW);
    h.step_dt(dt);
    h.release_key(KeyCode::KeyW);
    h.step_dt(dt);
    h.press_key(KeyCode::KeyW);
    h.step_dt(dt);
    h.release_key(KeyCode::KeyW);
    assert_eq!(h.state(), FighterState::Dodge);
}

fn assert_dagger_ledge_visual_root_is_pinned(h: &mut GameplayHarness) {
    let world = h.app.world_mut();
    let mut roots = world.query::<(&crate::plugins::character_plugin::FighterDummy, &Transform)>();
    let mut found = false;
    for (dummy, transform) in roots.iter(world) {
        if dummy.name == "d_base" {
            found = true;
            assert!(
                transform.translation.length() < 0.0001,
                "authored root must not add a second visible displacement: {:?}",
                transform.translation
            );
        }
    }
    assert!(found, "real character skeleton must be sampled");
}

#[test]
fn production_dagger_ledge_launch_descends_before_authored_lift() {
    for serial in [false, true] {
        for fps in [30, 60, 120, 165] {
            for platform in [false, true] {
                let mut h = dagger_ledge_harness(serial, platform);
                let dt = 1.0 / fps as f32;
                start_dagger_ledge_dash(&mut h, dt);
                for _ in 0..(0.16 * fps as f32).round() as usize {
                    h.step_dt(dt);
                }

                assert!(
                    h.app
                        .world()
                        .get::<FighterAir>(h.fighter)
                        .unwrap()
                        .ground_contact
                );
                h.press_mouse(MouseButton::Left);
                h.step_dt(dt);
                h.release_mouse(MouseButton::Left);
                assert_eq!(h.pose_id(), 253, "serial={serial} fps={fps}");
                assert_dagger_ledge_visual_root_is_pinned(&mut h);
                let mut min_y = h.position().y;
                let mut lifted = false;
                for _ in 0..fps * 2 {
                    let before = h.position();
                    h.step_dt(dt);
                    assert_dagger_ledge_visual_root_is_pinned(&mut h);
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();

                    assert!((h.position() - before).length() <= 72.0 * dt + 0.001);
                    if air.air.launch_lifted {
                        assert!(h.position().y > before.y);
                        assert!(
                            h.position().y > 0.0,
                            "must lift while still above the lower floor"
                        );
                        lifted = true;
                        break;
                    }
                    min_y = min_y.min(h.position().y);
                }
                assert!(lifted, "serial={serial} fps={fps} platform={platform}");
                if platform {
                    assert!(
                        min_y < 29.0,
                        "charge must fall off the ledge: {min_y}, fps={fps}"
                    );
                } else {
                    assert!(
                        min_y.abs() < 0.001,
                        "flat ground must support the charge: {min_y}"
                    );
                }
            }
        }
    }
}

#[test]
fn production_dagger_ledge_dash_falls_and_late_attack_uses_air_command() {
    for fps in [30, 60, 120, 165] {
        let dt = 1.0 / fps as f32;
        for attack in [false, true] {
            let mut h = dagger_ledge_harness(true, true);
            start_dagger_ledge_dash(&mut h, dt);
            let mut left_edge = false;
            for _ in 0..fps {
                h.step_dt(dt);
                if h.position().z > 0.6 {
                    left_edge = true;
                    break;
                }
            }
            assert!(left_edge);
            assert!(h.position().y < 30.0);
            assert!(h.airborne());
            assert_eq!(h.state(), FighterState::Dodge);
            if attack {
                h.press_mouse(MouseButton::Left);
                h.step_dt(dt);
                h.release_mouse(MouseButton::Left);
                assert_eq!(
                    h.pose_id(),
                    245,
                    "lost contact must use the original air command gate"
                );
                assert!(
                    !h.app
                        .world()
                        .get::<FighterDodge>(h.fighter)
                        .unwrap()
                        .state
                        .active
                );
                assert!(
                    !h.app
                        .world()
                        .get::<FighterAir>(h.fighter)
                        .unwrap()
                        .dodge_owns_motion_tick
                );
            } else {
                let mut finished = false;
                for _ in 0..fps {
                    let before = h.position();
                    h.step_dt(dt);
                    if !h
                        .app
                        .world()
                        .get::<FighterDodge>(h.fighter)
                        .unwrap()
                        .state
                        .active
                    {
                        assert_eq!(
                            h.position(),
                            before,
                            "mode6 terminal tick must not run another mover"
                        );
                        finished = true;
                        break;
                    }
                    assert!(
                        h.position().y < before.y,
                        "unsupported dash must continue falling"
                    );
                }
                assert!(finished);
                let before = h.position();
                h.step_dt(dt);
                assert!(
                    h.position().y < before.y,
                    "next tick must resume ordinary descent"
                );
            }
            for _ in 0..fps * 3 {
                h.step_dt(dt);
            }
            assert!(h.position().y.abs() < 0.001, "must land on the lower floor");
            assert!(!h.airborne());
        }
    }
}

#[test]
fn production_dagger_ledge_npc_has_one_vertical_mover_and_keeps_hit_owner() {
    let mut seed = dagger_ledge_harness(false, true);
    let assets = seed
        .app
        .world_mut()
        .remove_resource::<CharacterAssetsSet>()
        .unwrap();
    let common = seed.app.world_mut().remove_resource::<CommonAmb>().unwrap();
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<LocalGuardInput>()
        .insert_resource(assets)
        .insert_resource(common)
        .insert_resource(StageCollider {
            mesh: Some(harness_ground()),
        })
        .add_systems(
            Update,
            (
                crate::plugins::movement_plugin::dodge_root_motion,
                crate::plugins::ai_plugin::npc_ballistic_air_root_motion,
            )
                .chain(),
        );
    for fps in [30, 60, 120, 165] {
        let dt = 1.0 / fps as f32;
        let mut pose = PoseRuntime::idle();
        app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 166, true);
        let mut dodge = FighterDodge::default();
        dodge.pose = 166;
        dodge.state.start(166, 0.0, false, 0.0, 0.0);
        let entity = app
            .world_mut()
            .spawn((
                NpcFighter,
                Fighter {
                    character_id: 1,
                    player_slot: 1,
                    weapon: DAGGER,
                    weapon_slots: [DAGGER, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_xyz(0.0, 20.0, 0.0),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Dodge,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                FighterAir::default(),
                dodge,
                pose,
                EntryBlend::default(),
                SmState {
                    current: FighterState::Dodge,
                    ..default()
                },
                CombatState {
                    state: FighterState::Dodge,
                },
            ))
            .id();
        for tick in 1..=3 {
            let before = app.world().get::<Transform>(entity).unwrap().translation.y;
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(dt));
            app.update();
            let air = app.world().get::<FighterAir>(entity).unwrap();
            assert!(
                (air.air.vertical_speed - meteor_physics::jump::GRAVITY * dt * tick as f32).abs()
                    < 0.0001
            );
            let y = app.world().get::<Transform>(entity).unwrap().translation.y;
            assert!((y - before - air.air.vertical_speed * dt).abs() < 0.0001);
        }

        app.world_mut().get_mut::<SmState>(entity).unwrap().current = FighterState::BeHit;
        let mut reaction = getup_reaction_for_pose(93, 0, 1);
        reaction.kind = HitReactKind::BeHit;
        app.world_mut()
            .get_mut::<FighterHitReaction>(entity)
            .unwrap()
            .begin(reaction, true);
        let before = app.world().get::<Transform>(entity).unwrap().translation;
        app.update();
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            before
        );
        assert!(
            !app.world()
                .get::<FighterDodge>(entity)
                .unwrap()
                .state
                .active
        );
        assert_eq!(
            app.world().get::<SmState>(entity).unwrap().current,
            FighterState::BeHit
        );
        app.world_mut().despawn(entity);
    }
}

#[test]
fn production_npc_dagger_autonomous_launcher_lifts_at_high_frame_rate() {
    for fps in [60, 120] {
        let (mut h, target) = spawn_npc_combat_harness_with_order(DAGGER, [0.0, 100.0, 0.0], true);

        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .remove::<FighterAir>()
            .remove::<FighterWall>()
            .insert(FighterAir::default());
        install_guard_test_animation(&mut h);

        let mut seen_launch = false;
        let mut peak = 0.0_f32;
        let mut poses = std::collections::BTreeSet::new();
        for _ in 0..fps * 30 {
            h.step_dt(1.0 / fps as f32);
            poses.insert(h.pose_id());
            seen_launch |= h.pose_id() == 253;
            if h.pose_id() == 253 {
                peak = peak.max(h.position().y);
            }

            let p = h.position();
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(p.x, 0.0, p.z + 3.0);
            if peak > 1.0 {
                break;
            }
        }
        assert!(seen_launch && peak > 1.0,
            "NPC dagger must really lift: fps={fps}, launch={seen_launch}, peak={peak}, poses={poses:?}, air={:?}",
            h.app.world().get::<FighterAir>(h.fighter).unwrap().air);
    }
}
