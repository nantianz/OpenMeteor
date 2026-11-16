fn r7_action_edge(
    h: &mut GameplayHarness,
    mobile: bool,
    binding: crate::app_state::InputBinding,
    action: crate::plugins::input_plugin::SyntheticAction,
    pressed: bool,
) {
    if mobile {
        h.mobile_binding(binding, pressed, Some(action));
    } else if pressed {
        h.press_binding(binding);
    } else {
        h.release_binding(binding);
    }
}

fn r7_cancel_jump(h: &mut GameplayHarness, binding: crate::app_state::InputBinding) {
    match binding {
        crate::app_state::InputBinding::Key(key) => {
            h.app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .reset(key);
        }
        crate::app_state::InputBinding::Mouse(button) => {
            h.app
                .world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .reset(button);
        }
    }
    h.app
        .world_mut()
        .resource_mut::<SyntheticInputStream>()
        .cancel_binding(binding);
}

#[test]
fn production_r7_jump_cancel_clears_live_charge_but_keeps_an_accepted_release() {
    use crate::plugins::input_plugin::SyntheticAction as Input;
    for fps in [30, 60, 120] {
        for precharged in [false, true] {
            let mut h = recovery_timing_harness(true);
            crate::plugins::install_frame_clock(&mut h.app);
            let keys = crate::app_state::Keymap::default();
            let dt = 1.0 / fps as f32;
            h.mobile_binding(keys.jump, true, Some(Input::Jump));
            if precharged {
                h.step_dt(dt);
                assert!(
                    h.app
                        .world()
                        .get::<FighterAir>(h.fighter)
                        .unwrap()
                        .jump_charge_active
                );
            }
            r7_cancel_jump(&mut h, keys.jump);
            h.step_dt(dt);
            let context = format!("fps={fps}, precharged={precharged}");
            assert!(
                !h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .jump_charge_active,
                "{context}"
            );
            assert!(
                !h.app.world().resource::<LocalJumpInput>().action_accepted,
                "{context}"
            );
            assert!(
                !h.airborne(),
                "cancel must not produce key-up takeoff: {context}"
            );
            h.mobile_binding(keys.jump, false, Some(Input::Jump));
            h.step_dt(0.2);
            assert!(
                !h.airborne(),
                "late release must not resurrect cancelled charge: {context}"
            );
            assert!(
                !h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .jump_charge_active,
                "{context}"
            );

            h.mobile_binding(keys.jump, true, Some(Input::Jump));
            h.mobile_binding(keys.jump, false, Some(Input::Jump));

            h.mobile_binding(keys.jump, true, Some(Input::Jump));
            r7_cancel_jump(&mut h, keys.jump);
            h.step_dt(dt);
            let input = h.app.world().resource::<LocalJumpInput>();
            assert!(
                input.action_accepted && input.charge_canceled,
                "{context}: {input:?}"
            );
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            assert!(
                air.air.airborne && air.air.vertical_speed > 0.0,
                "cancel must preserve the earlier accepted release: {context}"
            );
            assert!(!air.jump_charge_active, "{context}");
        }
    }
}

#[test]
fn production_r7_jump_charge_uses_key_down_order_across_guard_release() {
    use crate::plugins::input_plugin::SyntheticAction as Input;
    for fps in [30, 60, 120] {
        for mobile in [false, true] {
            for release_guard_first in [false, true] {
                for hold_ticks in [0, 1, fps / 5] {
                    let mut h = recovery_timing_harness(true);
                    crate::plugins::install_frame_clock(&mut h.app);
                    let keys = crate::app_state::Keymap::default();
                    let dt = 1.0 / fps as f32;
                    r7_action_edge(&mut h, mobile, keys.guard, Input::Guard, true);
                    h.step_dt(dt);
                    assert_eq!(h.state(), FighterState::Guard);
                    let origin = h.position();
                    if release_guard_first {
                        r7_action_edge(&mut h, mobile, keys.guard, Input::Guard, false);
                    }
                    r7_action_edge(&mut h, mobile, keys.jump, Input::Jump, true);
                    if !release_guard_first {
                        r7_action_edge(&mut h, mobile, keys.guard, Input::Guard, false);
                    }
                    for tick in 0..hold_ticks {
                        h.step_dt(dt);
                        if tick == 0 {
                            assert_eq!(
                                h.app.world().resource::<LocalJumpInput>().press_accepted,
                                Some(release_guard_first)
                            );
                            assert_eq!(
                                h.app
                                    .world()
                                    .get::<FighterAir>(h.fighter)
                                    .unwrap()
                                    .jump_charge_active,
                                release_guard_first
                            );
                        }
                    }
                    r7_action_edge(&mut h, mobile, keys.jump, Input::Jump, false);
                    h.step_dt(dt);
                    let context = format!(
                        "fps={fps}, mobile={mobile}, guard_first={release_guard_first}, hold={hold_ticks}"
                    );
                    assert_eq!(h.airborne(), release_guard_first, "{context}");
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    assert!(
                        !air.jump_charge_active,
                        "released charge must be consumed: {context}"
                    );
                    if release_guard_first {
                        assert!(
                            air.air.vertical_speed > 0.0,
                            "accepted press must really lift off: {context}"
                        );
                    } else {
                        assert_eq!(h.state(), FighterState::Name, "{context}");
                        assert!(
                            (h.position().y - origin.y).abs() < 0.01,
                            "rejected press must not lift off: {context}"
                        );
                        assert!(!h.app.world().resource::<LocalJumpInput>().action_accepted);
                    }
                }
            }
        }
    }
}

#[test]
fn production_r7_jump_and_attack_keep_device_event_order() {
    use crate::plugins::input_plugin::SyntheticAction as Input;
    for fps in [30, 60, 120] {
        for mobile in [false, true] {
            for jump_first in [false, true] {
                let mut h = recovery_timing_harness(true);
                crate::plugins::install_frame_clock(&mut h.app);
                let keys = crate::app_state::Keymap::default();
                if !jump_first {
                    r7_action_edge(&mut h, mobile, keys.attack, Input::Attack, true);
                }
                r7_action_edge(&mut h, mobile, keys.jump, Input::Jump, true);
                r7_action_edge(&mut h, mobile, keys.jump, Input::Jump, false);
                if jump_first {
                    r7_action_edge(&mut h, mobile, keys.attack, Input::Attack, true);
                }
                h.step_dt(1.0 / fps as f32);
                let context = format!("fps={fps}, mobile={mobile}, jump_first={jump_first}");
                assert!(is_attacking_state(h.state()), "{context}");
                assert_eq!(h.airborne(), jump_first, "{context}");
                let input = h.app.world().resource::<LocalJumpInput>();
                assert_eq!(input.press_accepted, Some(jump_first), "{context}");
                assert_eq!(input.action_accepted, jump_first, "{context}");
                assert!(!input.mode11_accepted);
                assert!(
                    !h.app
                        .world()
                        .get::<FighterAir>(h.fighter)
                        .unwrap()
                        .jump_charge_active
                );
            }
        }
    }
}

#[test]
fn production_r7_multiple_jump_taps_keep_the_accepted_release_and_duration() {
    use crate::plugins::input_plugin::SyntheticAction as Input;
    for fps in [30, 60, 120] {
        for accepted_first in [false, true] {
            for strong in [false, true] {
                let mut h = recovery_timing_harness(true);
                crate::plugins::install_frame_clock(&mut h.app);
                let keys = crate::app_state::Keymap::default();
                if !accepted_first {
                    h.press_binding(keys.guard);
                    h.step_dt(1.0 / fps as f32);
                    assert_eq!(h.state(), FighterState::Guard);
                }
                let accepted_hold = if strong { 0.18 } else { 0.01 };
                let rejected_hold = if strong { 0.01 } else { 0.18 };
                let first_hold = if accepted_first {
                    accepted_hold
                } else {
                    rejected_hold
                };
                let second_hold = if accepted_first {
                    rejected_hold
                } else {
                    accepted_hold
                };
                {
                    let mut stream = h.app.world_mut().resource_mut::<SyntheticInputStream>();

                    stream.push_action(keys.jump, Input::Jump, true, Some(10.0));
                    stream.push_action(keys.jump, Input::Jump, false, Some(10.0 + first_hold));
                }

                if accepted_first {
                    h.mobile_binding(keys.attack, true, Some(Input::Attack));
                } else {
                    h.mobile_binding(keys.guard, false, Some(Input::Guard));
                }
                {
                    let mut stream = h.app.world_mut().resource_mut::<SyntheticInputStream>();
                    stream.push_action(keys.jump, Input::Jump, true, Some(10.3));
                    stream.push_action(keys.jump, Input::Jump, false, Some(10.3 + second_hold));
                }
                h.step_dt(1.0 / fps as f32);
                let context =
                    format!("fps={fps}, accepted_first={accepted_first}, strong={strong}");
                let input = h.app.world().resource::<LocalJumpInput>();
                assert!(
                    input.action_accepted,
                    "{context}: state={:?}, input={input:?}",
                    h.state()
                );
                assert_eq!(input.press_accepted, Some(!accepted_first), "{context}");
                assert!(
                    (input.accepted_release_hold_seconds.unwrap() - accepted_hold as f32).abs()
                        < 1e-5,
                    "must retain the accepted event's duration: {context}"
                );
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                assert!(
                    air.air.airborne && air.air.vertical_speed > 0.0,
                    "{context}"
                );
                assert_eq!(
                    air.air.vertical_speed > meteor_physics::jump::JUMP_VERTICAL_SPEED,
                    strong,
                    "later rejected tap must not change jump strength: {context}"
                );
                assert!(!air.jump_charge_active, "{context}");
                assert_eq!(is_attacking_state(h.state()), accepted_first, "{context}");
            }
        }
    }
}

#[test]
fn production_r7_guard_repress_does_not_revoke_an_earlier_jump() {
    use crate::plugins::input_plugin::SyntheticAction as Input;
    for fps in [30, 60, 120] {
        for mobile in [false, true] {
            let mut h = recovery_timing_harness(true);
            crate::plugins::install_frame_clock(&mut h.app);
            let keys = crate::app_state::Keymap::default();
            let dt = 1.0 / fps as f32;
            r7_action_edge(&mut h, mobile, keys.guard, Input::Guard, true);
            h.step_dt(dt);
            assert_eq!(h.state(), FighterState::Guard);
            r7_action_edge(&mut h, mobile, keys.guard, Input::Guard, false);
            r7_action_edge(&mut h, mobile, keys.jump, Input::Jump, true);
            r7_action_edge(&mut h, mobile, keys.jump, Input::Jump, false);
            r7_action_edge(&mut h, mobile, keys.guard, Input::Guard, true);
            h.step_dt(dt);
            let context = format!("fps={fps}, mobile={mobile}");
            assert!(
                h.app.world().resource::<LocalGuardInput>().held,
                "{context}"
            );
            let input = h.app.world().resource::<LocalJumpInput>();
            assert!(input.action_accepted, "{context}: {input:?}");
            assert_eq!(input.press_accepted, Some(true), "{context}");
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            assert!(
                air.air.airborne && air.air.vertical_speed > 0.0,
                "{context}"
            );
            assert_eq!(h.state(), FighterState::Jump, "{context}");
        }
    }
}

#[test]
fn production_r7_mode11_air_jump_keeps_impulse_after_same_batch_attack() {
    use crate::plugins::input_plugin::SyntheticAction as Input;
    for fps in [30, 60, 120] {
        for mobile in [false, true] {
            for down in [false, true] {
                let mut h = recovery_timing_harness(true);
                crate::plugins::install_frame_clock(&mut h.app);
                let keys = crate::app_state::Keymap::default();
                let dt = 1.0 / fps as f32;
                if down {
                    install_timing_reaction(&mut h, 95, HitReactKind::BeHit, 0.2);
                    for _ in 0..fps * 2 {
                        h.step_dt(dt);
                        if h.state() == FighterState::Down {
                            break;
                        }
                    }
                    assert_eq!(h.state(), FighterState::Down);
                } else {
                    r7_action_edge(&mut h, mobile, keys.fake, Input::Fake, true);
                    h.step_dt(dt);
                    assert_eq!(h.state(), FighterState::Fake);
                }

                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation
                    .y += 10.0;
                {
                    let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                    air.air.airborne = true;
                    air.air.vertical_speed = -2.0;
                    air.ground_contact = false;
                }
                r7_action_edge(&mut h, mobile, keys.jump, Input::Jump, true);
                r7_action_edge(&mut h, mobile, keys.jump, Input::Jump, false);
                r7_action_edge(&mut h, mobile, keys.attack, Input::Attack, true);

                h.step_dt(0.0);
                let context = format!("fps={fps}, mobile={mobile}, down={down}");
                let input = h.app.world().resource::<LocalJumpInput>();
                assert_eq!(input.press_accepted, Some(true), "{context}");
                assert!(input.action_accepted && input.mode11_accepted, "{context}");
                assert!(is_attacking_state(h.state()), "{context}");
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                assert!(
                    air.air.airborne && air.air.vertical_speed > 0.0,
                    "mode11 must apply a real impulse despite later Attack: {context}"
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
