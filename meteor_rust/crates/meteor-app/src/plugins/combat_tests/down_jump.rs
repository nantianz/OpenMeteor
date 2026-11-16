#[test]
fn production_down_jump_release_has_impulse_and_leaves_ground() {
    for fps in [30, 60, 120] {
        for source in [95, 96] {
            for hold_ticks in [0, 1, fps / 5] {
                let long = hold_ticks > 1;
                for mobile in [false, true] {
                    let mut h = recovery_timing_harness(true);
                    install_timing_reaction(&mut h, source, HitReactKind::BeHit, 0.2);
                    let dt = 1.0 / fps as f32;
                    for _ in 0..fps * 2 {
                        h.step_dt(dt);
                        if h.state() == FighterState::Down {
                            break;
                        }
                    }
                    assert_eq!(h.state(), FighterState::Down);
                    assert!(matches!(h.pose_id(), 112 | 113));
                    let origin = h.position();
                    let jump = h.app.world().resource::<crate::app_state::Keymap>().jump;
                    if mobile {
                        h.mobile_binding(
                            jump,
                            true,
                            Some(crate::plugins::input_plugin::SyntheticAction::Jump),
                        );
                    } else {
                        h.press_binding(jump);
                    }
                    for _ in 0..hold_ticks {
                        h.step_dt(dt);
                        assert_eq!(h.state(), FighterState::Down, "mode11 waits for release");
                    }
                    if mobile {
                        h.mobile_binding(
                            jump,
                            false,
                            Some(crate::plugins::input_plugin::SyntheticAction::Jump),
                        );
                    } else {
                        h.release_binding(jump);
                    }
                    h.step_dt(dt);
                    let air = &h.app.world().get::<FighterAir>(h.fighter).unwrap().air;
                    assert!(air.airborne && air.vertical_speed > 0.0,
                        "accepted Down jump must install physics: source={source}, fps={fps}, long={long}, mobile={mobile}, state={:?}, airborne={}, vy={}",
                        h.state(), air.airborne, air.vertical_speed);
                    assert_eq!(
                        air.vertical_speed > meteor_physics::jump::JUMP_VERTICAL_SPEED,
                        long,
                        "release duration must select the normal/strong impulse"
                    );
                    assert_eq!(h.state(), FighterState::Jump);
                    assert_eq!(h.pose_id(), 151);
                    assert!(h
                        .app
                        .world()
                        .get::<FighterHitReaction>(h.fighter)
                        .unwrap()
                        .current
                        .is_none());
                    let mut peak = h.position().y;
                    for _ in 0..fps {
                        h.step_dt(dt);
                        peak = peak.max(h.position().y);
                    }
                    assert!(peak > origin.y + 1.0, "must actually lift off");
                    assert!(!h.airborne(), "must land after the jump");
                    assert_eq!(h.state(), FighterState::Name);
                }
            }
        }
    }
}

#[test]
fn production_down_jump_ignores_release_of_key_pressed_during_hit_stun() {
    for fps in [30, 60, 120] {
        for mobile in [false, true] {
            let mut h = recovery_timing_harness(true);
            install_timing_reaction(&mut h, 102, HitReactKind::BeHit, 0.4);
            let dt = 1.0 / fps as f32;
            let jump = h.app.world().resource::<crate::app_state::Keymap>().jump;
            if mobile {
                h.mobile_binding(
                    jump,
                    true,
                    Some(crate::plugins::input_plugin::SyntheticAction::Jump),
                );
            } else {
                h.press_binding(jump);
            }
            h.step_dt(dt);
            assert_eq!(h.state(), FighterState::BeHit);
            assert!(
                !h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .jump_charge_active
            );
            for _ in 0..fps * 2 {
                h.step_dt(dt);
                if h.state() == FighterState::Down {
                    break;
                }
            }
            assert_eq!(h.state(), FighterState::Down);
            if mobile {
                h.mobile_binding(
                    jump,
                    false,
                    Some(crate::plugins::input_plugin::SyntheticAction::Jump),
                );
            } else {
                h.release_binding(jump);
            }
            h.step_dt(dt);
            assert_eq!(
                h.state(),
                FighterState::Down,
                "rejected key-down cannot release Down"
            );
            assert_eq!(h.pose_id(), 113);
            assert!(!h.airborne());
            assert!(h
                .app
                .world()
                .get::<FighterHitReaction>(h.fighter)
                .unwrap()
                .current
                .is_some());
        }
    }
}

#[test]
fn production_guard_hit_cannot_start_jump_charge() {
    let mut h = recovery_timing_harness(true);
    install_timing_reaction(&mut h, 60, HitReactKind::Guard, 0.4);
    let jump = h.app.world().resource::<crate::app_state::Keymap>().jump;
    h.press_binding(jump);
    h.step();
    assert!(
        !h.app
            .world()
            .get::<FighterAir>(h.fighter)
            .unwrap()
            .jump_charge_active
    );
    h.release_binding(jump);
    h.step();
    assert_eq!(h.state(), FighterState::Guard);
    assert!(!h.airborne());
    assert!(h
        .app
        .world()
        .get::<FighterHitReaction>(h.fighter)
        .unwrap()
        .current
        .is_some());
}

#[test]
fn production_guard_hit_rejects_jump_tap_and_preexisting_charge() {
    for serial in [false, true] {
        for fps in [30, 60, 120] {
            for mobile in [false, true] {
                for precharged in [false, true] {
                    let mut h = recovery_timing_harness(serial);
                    let keys = crate::app_state::Keymap::default();
                    let dt = 1.0 / fps as f32;
                    h.press_binding(keys.guard);
                    h.step_dt(dt);
                    if precharged {
                        if mobile {
                            h.mobile_binding(
                                keys.jump,
                                true,
                                Some(crate::plugins::input_plugin::SyntheticAction::Jump),
                            );
                        } else {
                            h.press_binding(keys.jump);
                        }
                        h.step_dt(dt);
                        assert!(
                            h.app
                                .world()
                                .get::<FighterAir>(h.fighter)
                                .unwrap()
                                .jump_charge_active
                        );
                    }
                    install_timing_reaction(&mut h, 60, HitReactKind::Guard, 0.4);
                    h.app
                        .world_mut()
                        .get_mut::<FighterHitReaction>(h.fighter)
                        .unwrap()
                        .current
                        .as_mut()
                        .unwrap()
                        .motion_scale = 8.0;

                    h.app
                        .world_mut()
                        .get_mut::<EntryBlend>(h.fighter)
                        .unwrap()
                        .duration = 0.0;
                    h.step_dt(dt);
                    if !precharged {
                        if mobile {
                            h.mobile_binding(
                                keys.jump,
                                true,
                                Some(crate::plugins::input_plugin::SyntheticAction::Jump),
                            );
                        } else {
                            h.press_binding(keys.jump);
                        }
                    }
                    if mobile {
                        h.mobile_binding(
                            keys.jump,
                            false,
                            Some(crate::plugins::input_plugin::SyntheticAction::Jump),
                        );
                    } else {
                        h.release_binding(keys.jump);
                    }
                    h.step_dt(dt);
                    let context = format!(
                        "serial={serial} fps={fps} mobile={mobile} precharged={precharged}"
                    );
                    assert_eq!(h.state(), FighterState::Guard, "{context}");
                    assert!(!h.airborne(), "{context}");
                    assert_eq!(h.pose_id(), 60, "{context}");
                    assert!(
                        h.app
                            .world()
                            .get::<FighterHitReaction>(h.fighter)
                            .unwrap()
                            .current
                            .is_some(),
                        "{context}"
                    );
                    assert!(
                        !h.app
                            .world()
                            .get::<FighterAir>(h.fighter)
                            .unwrap()
                            .jump_charge_active,
                        "{context}"
                    );
                }
            }
        }
    }
}
