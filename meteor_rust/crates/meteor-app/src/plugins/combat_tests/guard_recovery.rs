fn check_weapon_guard_after_recovery(reaction_poses: &[u16]) {
    for serial in [false, true] {
        for fps in [30, 60, 120] {
            for weapon in [1, 2, 3, 6] {
                for &id in reaction_poses {
                    for release_early in [false, true] {
                        let mut h = spawn_gameplay_harness_with_order(weapon, serial)
                            .expect("original P1 resources");
                        install_guard_test_animation(&mut h);
                        let keys = crate::app_state::Keymap::default();
                        let dt = 1.0 / fps as f32;
                        let context = format!(
                            "weapon={weapon} pose={id} serial={serial} fps={fps} release={release_early}"
                        );

                        if id == 117 {
                            h.press_binding(keys.fake);
                            h.step_dt(dt);
                            h.release_binding(keys.fake);
                            for _ in 0..fps * 2 {
                                h.step_dt(dt);
                            }
                            assert_eq!(h.state(), FighterState::Fake, "{context}");
                        } else {
                            install_timing_reaction(&mut h, id, HitReactKind::BeHit, 0.2);
                            if matches!(id, 112 | 113) {
                                for _ in 0..fps * 3 {
                                    h.step_dt(dt);
                                    if h.state() == FighterState::Down {
                                        break;
                                    }
                                }
                                assert_eq!(h.state(), FighterState::Down, "{context}");
                            }
                        }

                        h.press_binding(keys.guard);
                        h.step_dt(dt);
                        assert_eq!(h.state(), FighterState::BeHit, "{context}");
                        if id != 93 {
                            assert_eq!(
                                h.pose_id(),
                                match id {
                                    117 => 35,
                                    113 => 120,
                                    _ => 119,
                                },
                                "guard must still initiate getup: {context}"
                            );
                        }
                        if release_early {
                            h.release_binding(keys.guard);
                        }

                        let mut recovered = false;
                        for _ in 0..fps * 4 {
                            h.step_dt(dt);
                            if h.app
                                .world()
                                .get::<FighterHitReaction>(h.fighter)
                                .unwrap()
                                .current
                                .is_none()
                            {
                                recovered = true;
                                break;
                            }
                            assert_eq!(h.state(), FighterState::BeHit, "{context}");
                        }
                        assert!(recovered, "reaction did not finish: {context}");
                        let expected = if weapon == 6 && !release_early {
                            FighterState::Guard
                        } else {
                            FighterState::Name
                        };

                        for _ in 0..3 {
                            assert_eq!(h.state(), expected, "{context}");
                            assert_eq!(
                                h.app.world().get::<CombatState>(h.fighter).unwrap().state,
                                expected,
                                "hit resolution projection: {context}"
                            );
                            h.step_dt(dt);
                        }
                        h.release_binding(keys.guard);
                        h.step_dt(dt);
                        assert_eq!(h.state(), FighterState::Name, "{context}");
                        h.press_binding(keys.guard);
                        h.step_dt(dt);
                        assert_eq!(h.state() == FighterState::Guard, weapon == 6, "{context}");
                    }
                }
            }
        }
    }
}

#[test]
fn production_weapon_guard_eligibility_after_hit_recovery() {
    check_weapon_guard_after_recovery(&[93]);
}

#[test]
fn production_weapon_guard_eligibility_after_down_and_fake_getup() {
    check_weapon_guard_after_recovery(&[112, 113, 117]);
}

#[test]
fn production_weapon_guard_eligibility_after_dodge_and_burst() {
    for serial in [false, true] {
        for fps in [30, 60, 120] {
            for weapon in [1, 2, 3, 6] {
                for burst in [false, true] {
                    let mut h = spawn_gameplay_harness_with_order(weapon, serial)
                        .expect("original P1 resources");
                    install_guard_test_animation(&mut h);
                    let keys = crate::app_state::Keymap::default();
                    let dt = 1.0 / fps as f32;
                    let context =
                        format!("weapon={weapon} burst={burst} serial={serial} fps={fps}");
                    if burst {
                        h.app
                            .world_mut()
                            .resource_mut::<SyntheticInputStream>()
                            .push_dodge(meteor_input::command_parser::Dir::Up, true);
                    } else {
                        h.press_binding(keys.dodge);
                    }
                    h.step_dt(dt);
                    if !burst {
                        h.release_binding(keys.dodge);
                    }
                    let action = if burst {
                        FighterState::Burst
                    } else {
                        FighterState::Dodge
                    };
                    assert_eq!(h.state(), action, "{context}");
                    assert!(
                        h.app
                            .world()
                            .get::<FighterDodge>(h.fighter)
                            .unwrap()
                            .state
                            .active
                    );
                    h.press_binding(keys.guard);
                    for _ in 0..fps * 3 {
                        h.step_dt(dt);
                        if !h
                            .app
                            .world()
                            .get::<FighterDodge>(h.fighter)
                            .unwrap()
                            .state
                            .active
                        {
                            break;
                        }
                        assert_eq!(h.state(), action, "guard must not interrupt: {context}");
                    }
                    assert!(
                        !h.app
                            .world()
                            .get::<FighterDodge>(h.fighter)
                            .unwrap()
                            .state
                            .active
                    );
                    let expected = if weapon == 6 {
                        FighterState::Guard
                    } else {
                        FighterState::Name
                    };
                    for _ in 0..3 {
                        assert_eq!(h.state(), expected, "{context}");
                        assert_eq!(
                            h.app.world().get::<CombatState>(h.fighter).unwrap().state,
                            expected,
                            "{context}"
                        );
                        h.step_dt(dt);
                    }
                    h.release_binding(keys.guard);
                    h.step_dt(dt);
                    assert_eq!(h.state(), FighterState::Name, "{context}");
                }
            }
        }
    }
}

#[test]
fn production_weapon_guard_dodge_exit_checks_npc_owner_and_replaced_pose() {
    let mut seed = spawn_gameplay_harness(6).expect("original P1 resources");
    let assets = seed
        .app
        .world_mut()
        .remove_resource::<CharacterAssetsSet>()
        .unwrap();
    let common = seed.app.world_mut().remove_resource::<CommonAmb>().unwrap();
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<LocalGuardInput>()
        .insert_resource(StageCollider {
            mesh: Some(harness_ground()),
        })
        .insert_resource(assets)
        .insert_resource(common)
        .add_systems(Update, crate::plugins::movement_plugin::dodge_root_motion);
    for weapon in [1, 2, 3, 6] {
        for local in [false, true] {
            for held in [false, true] {
                for replaced in [false, true] {
                    for burst in [false, true] {
                        let mut pose = PoseRuntime::idle();
                        let dodge_pose = if burst { 176 } else { 164 };
                        app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(
                            &mut pose,
                            if replaced { 0 } else { dodge_pose },
                            true,
                        );
                        if !replaced {
                            pose.frame = pose.end as f32;
                        }
                        let mut dodge = FighterDodge::default();
                        dodge.pose = dodge_pose;
                        dodge.pose_entered = true;
                        dodge.state.start(dodge_pose, 0.0, burst, 0.0, 0.0);
                        let action = if burst {
                            FighterState::Burst
                        } else {
                            FighterState::Dodge
                        };
                        let mut actor = app.world_mut().spawn((
                            Fighter {
                                character_id: 1,
                                player_slot: 0,
                                weapon,
                                weapon_slots: [weapon, 0],
                                weapon_slot: 0,
                                weapon_slot_variants: [0, 0],
                                weapon_variant: 0,
                            },
                            FighterAir::default(),
                            Transform::default(),
                            MotionState {
                                movement_mode: meteor_core::MovementMode::Dodge,
                                velocity: Vec3::ZERO,
                                facing: 0.0,
                            },
                            pose,
                            dodge,
                            SmState {
                                current: action,
                                ..default()
                            },
                            CombatState { state: action },
                            NpcPerformance {
                                guard_remaining: if held { 1.0 } else { 0.0 },
                                ..default()
                            },
                        ));
                        if local {
                            actor.insert(LocalFighter);
                        }
                        let entity = actor.id();

                        app.world_mut().resource_mut::<LocalGuardInput>().held =
                            if local { held } else { !held };
                        app.world_mut()
                            .resource_mut::<Time>()
                            .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
                        app.update();
                        let expected = if held && weapon == 6 {
                            FighterState::Guard
                        } else {
                            FighterState::Name
                        };
                        let context = format!("weapon={weapon} local={local} held={held} replaced={replaced} burst={burst}");
                        assert_eq!(
                            app.world().get::<SmState>(entity).unwrap().current,
                            expected,
                            "{context}"
                        );
                        assert_eq!(
                            app.world().get::<CombatState>(entity).unwrap().state,
                            expected,
                            "{context}"
                        );
                        assert!(
                            !app.world()
                                .get::<FighterDodge>(entity)
                                .unwrap()
                                .state
                                .active,
                            "{context}"
                        );
                        app.world_mut().despawn(entity);
                    }
                }
            }
        }
    }
}
