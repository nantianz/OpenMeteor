fn recovery_timing_harness(serial: bool) -> GameplayHarness {
    let mut h = spawn_gameplay_harness_with_order(5, serial).expect("original P1 resources");
    crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
    let schedule = super::super::fighter_frame::schedule(&h.app);
    h.app.add_systems(
        schedule,
        crate::plugins::character_plugin::drive_fighter_animation_for_test
            .before(harness_retire_entry_blend)
            .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
    );
    h
}

fn install_timing_reaction(h: &mut GameplayHarness, id: u16, kind: HitReactKind, minimum: f32) {
    let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
    let mut pose = PoseRuntime::idle();
    let mut blend = EntryBlend::default();
    assets.apply_pose_with_blend(
        &mut pose,
        &mut blend,
        id,
        true,
        Some(0.12),
        EntryBlendKind::Generic,
        false,
        true,
        None,
    );
    let mut reaction = getup_reaction_for_pose(id, pose.start, pose.end);
    reaction.kind = kind;
    reaction.minimum_remaining = minimum;
    reaction.authored_minimum = minimum;
    let state = if kind == HitReactKind::Guard {
        FighterState::Guard
    } else {
        FighterState::BeHit
    };
    h.app
        .world_mut()
        .entity_mut(h.fighter)
        .insert((pose, blend));
    let mut sm = h.app.world_mut().get_mut::<SmState>(h.fighter).unwrap();
    sm.current = state;
    sm.locked = true;
    h.app
        .world_mut()
        .get_mut::<CombatState>(h.fighter)
        .unwrap()
        .state = state;
    h.app
        .world_mut()
        .get_mut::<FighterHitReaction>(h.fighter)
        .unwrap()
        .begin(reaction, false);
}

#[test]
fn production_recovery_minimum_starts_after_blend_retirement() {
    for serial in [false, true] {
        for fps in [30, 60, 120, 125, 165] {
            for (id, kind) in [
                (93, HitReactKind::BeHit),
                (60, HitReactKind::Guard),
                (113, HitReactKind::BeHit),
            ] {
                let mut h = recovery_timing_harness(serial);
                if kind == HitReactKind::Guard {
                    h.press_binding(crate::app_state::Keymap::default().guard);
                }

                for _ in 0..2 {
                    install_timing_reaction(&mut h, id, kind, 0.4);
                    let dt = 1.0 / fps as f32;
                    let mut retired = false;
                    for _ in 0..fps {
                        h.step_dt(dt);
                        let reaction = h.app.world().get::<FighterHitReaction>(h.fighter).unwrap();
                        assert_eq!(
                            reaction.current.as_ref().unwrap().minimum_remaining,
                            0.4,
                            "serial={serial} fps={fps} pose={id}: fade consumed minimum"
                        );
                        if h.app.world().get::<EntryBlend>(h.fighter).unwrap().duration == 0.0 {
                            retired = true;
                            break;
                        }
                    }
                    assert!(retired, "fade must retire");
                    h.step_dt(dt);
                    let remaining = h
                        .app
                        .world()
                        .get::<FighterHitReaction>(h.fighter)
                        .unwrap()
                        .current
                        .as_ref()
                        .unwrap()
                        .minimum_remaining;
                    assert!((remaining - (0.4 - dt)).abs() < 1e-6,
                        "serial={serial} fps={fps} pose={id}: playback must consume exactly one step");
                }
            }
        }
    }
}

#[test]
fn production_getup_waits_for_slow_animation_and_rejects_jump() {
    for fps in [30, 60, 120, 125, 165] {
        for id in [35, 119, 120] {
            let mut h = recovery_timing_harness(true);
            let rules = meteor_combat::battle_rules::BattleRules::from_json(&format!(
                r#"{{"schemaVersion":2,"rebalance":{{"guardChipPercent":0,"guardChipPoses":[],"poseOverrides":[{{"poseId":{id},"speedPercent":25}}]}}}}"#
            )).unwrap();
            {
                let mut net = h.app.world_mut().resource_mut::<NetState>();
                net.status = NetStatus::InRoom;
                net.round_active = true;
                net.battle_rules = rules;
            }
            install_timing_reaction(&mut h, id, HitReactKind::GetUp, 0.0);
            let keys = crate::app_state::Keymap::default();
            let mut saw_expired_but_locked = false;
            let mut recovered = false;
            for _ in 0..fps * 8 {
                h.press_binding(keys.jump);
                h.step_dt(1.0 / fps as f32);
                h.release_binding(keys.jump);
                let runtime = h.app.world().get::<FighterHitReaction>(h.fighter).unwrap();
                if let Some(reaction) = &runtime.current {
                    saw_expired_but_locked |= reaction.lock_remaining < 0.0;
                    assert_eq!(h.state(), FighterState::BeHit, "pose={id} fps={fps}");
                    assert!(
                        !h.app
                            .world()
                            .get::<FighterAir>(h.fighter)
                            .unwrap()
                            .air
                            .airborne
                    );
                } else {
                    recovered = true;
                    break;
                }
            }
            assert!(
                saw_expired_but_locked,
                "exercise timeout: pose={id} fps={fps}"
            );
            assert!(recovered, "getup must recover: pose={id} fps={fps}");
        }
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
#[ignore = "COMBAT-084 real-resource hit/health diagnostic, no interactive game"]
fn diagnose_dagger_last_stab_repeated_hits() {
    check_dagger_last_stab(&[60, 120, 125, 165], &[3.0, 4.0, 5.0, 6.0], false);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_dagger_last_stab_hits_once_at_all_frame_rates() {
    check_dagger_last_stab(&[60, 120, 125, 165], &[3.0], true);
}

#[cfg(not(target_arch = "wasm32"))]
fn check_dagger_last_stab(frame_rates: &[u32], distances: &[f32], require_three_hits: bool) {
    check_dagger_last_stab_path(frame_rates, distances, require_three_hits, false, false);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
#[ignore = "COMBAT-084 stock claims and jitter diagnostic; no game UI"]
fn diagnose_dagger_last_stab_stock_claims_and_jitter() {
    for stock in [false, true] {
        for jitter in [false, true] {
            check_dagger_last_stab_path(&[120], &[4.0], false, stock, jitter);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn check_dagger_last_stab_path(
    frame_rates: &[u32],
    distances: &[f32],
    require_three_hits: bool,
    stock: bool,
    jitter: bool,
) {
    for &fps in frame_rates {
        for &distance in distances {
            let (mut h, target, take_hits) = spawn_local_stock_hit_harness_with_order(5, true);
            if !stock {
                h.app.insert_resource(NetState::default());
            }
            for entity in [h.fighter, target] {
                h.app
                    .world_mut()
                    .entity_mut(entity)
                    .insert(crate::components::fighter::VisualRootPin::default());
            }
            h.app.world_mut().entity_mut(target).insert((
                WeaponForm::default(),
                FighterHitReaction::default(),
                GunAimState::default(),
            ));
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = 0.0;
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = 0.0;
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::ZERO;
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(0.0, 0.0, distance);
            h.app.world_mut().get_mut::<Health>(target).unwrap().current = 10000.0;
            let schedule = super::super::fighter_frame::schedule(&h.app);
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
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 258, true);
            h.app.world_mut().entity_mut(h.fighter).insert(pose);
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = FighterState::Attack1;
            h.app
                .world_mut()
                .get_mut::<CombatState>(h.fighter)
                .unwrap()
                .state = FighterState::Attack1;
            let mut previous_hp = 10000.0;
            let mut hits = Vec::new();
            let mut window_ticks = 0;
            for tick in 0..fps * 5 {
                let last_hit_at = h
                    .app
                    .world()
                    .get::<TargetHitClock>(target)
                    .unwrap()
                    .last_hit_at;
                let dt = 1.0 / fps as f32
                    + if jitter {
                        [-0.0005, 0.0005][tick as usize % 2]
                    } else {
                        0.0
                    };
                h.step_dt(dt);
                let now = h.app.world().resource::<Time>().elapsed_secs_f64();
                let source = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                window_ticks +=
                    usize::from(source.pose_id == 258 && (9479.0..9482.0).contains(&source.frame));
                let hp = h.app.world().get::<Health>(target).unwrap().current;
                let claims = take_hits();
                if hp < previous_hp || !claims.is_empty() {
                    let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                    hits.push((
                        tick,
                        pose.pose_id,
                        pose.frame,
                        previous_hp - hp,
                        now,
                        now - last_hit_at,
                        claims.len(),
                    ));
                }
                previous_hp = hp;
            }
            eprintln!("dagger last stab stock={stock} jitter={jitter} fps={fps} distance={distance} window_ticks={window_ticks} hits(tick,pose,frame,damage,now,interval,claims)={hits:?}");
            assert!(window_ticks > 0, "must exercise the final Attack window");
            if require_three_hits {
                assert_eq!(hits.len(), 3, "fps={fps} distance={distance}: {hits:?}");
                for (hit, (start, end)) in
                    hits.iter()
                        .zip([(9448.0, 9451.0), (9463.0, 9466.0), (9479.0, 9482.0)])
                {
                    assert_eq!(hit.1, 258);
                    assert!(
                        (start..end).contains(&hit.2),
                        "one hit per distinct stab: {hits:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn production_spear_reaction_holds_authored_loop_before_tail() {
    for serial in [false, true] {
        for fps in [30, 60, 120, 165] {
            for initial_pose in [100, 118] {
                let mut h = recovery_timing_harness(serial);
                let attack = h.app.world().resource::<CharacterAssetsSet>().0["p1"].poses[&289]
                    .attacks[0]
                    .clone();
                assert_eq!(attack.target_pose, Some(100));
                assert_eq!(attack.target_value, 0.5);
                install_timing_reaction(
                    &mut h,
                    initial_pose,
                    HitReactKind::BeHit,
                    attack.target_value,
                );
                let mut held_frames = 0;
                let mut recovered = false;
                for _ in 0..fps * 3 {
                    h.step_dt(1.0 / fps as f32);
                    let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                    let runtime = h.app.world().get::<FighterHitReaction>(h.fighter).unwrap();
                    let Some(hit) = runtime.current.as_ref() else {
                        recovered = true;
                        break;
                    };
                    if pose.pose_id == 118
                        && hit.minimum_remaining > 1.0 / fps as f32
                        && pose.frame >= 1905.0
                    {
                        assert_eq!(pose.frame, 1905.0,
                            "118 must hold its authored frame, not replay its tail: serial={serial} fps={fps} initial={initial_pose} remaining={}", hit.minimum_remaining);
                        held_frames += 1;
                    }
                    assert_eq!(hit.rewind_count, 0, "118 tail must never rewind");
                }
                assert!(held_frames > 0);
                assert!(recovered);
            }
        }
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_reaction_ninja_back_chains_do_not_replay_hit_tails() {
    for followup in [471, 466] {
        run_ninja_spacing_case(
            &[60, 120],
            &[true],
            &[2.0, 3.0, 4.0],
            true,
            followup,
            Some(&format!(
                "../../../analysis/reaction-loop-20260926/ninja-{followup}.csv"
            )),
        );
    }
}

#[test]
fn production_target_hit_clock_advances_during_offline_hitstop() {
    use bevy::ecs::system::RunSystemOnce;
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<crate::plugins::impact_fx_plugin::ImpactSimulationScale>();
    let target = app
        .world_mut()
        .spawn(TargetHitClock {
            last_hit_at: 0.0,
            spawn_elapsed: 0.0,
        })
        .id();
    let fresh = app.world_mut().spawn(TargetHitClock::default()).id();
    app.world_mut()
        .resource_mut::<crate::plugins::impact_fx_plugin::ImpactSimulationScale>()
        .frozen = true;
    for step in 1..=30 {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(10));
        app.world_mut()
            .run_system_once(tick_target_hit_clocks)
            .unwrap();
        let now = app.world().resource::<Time>().elapsed_secs_f64();
        assert_eq!(
            app.world()
                .get::<TargetHitClock>(target)
                .unwrap()
                .accepts_target_hit(now),
            step > 20
        );
    }
    let now = app.world().resource::<Time>().elapsed_secs_f64();
    assert!(
        app.world()
            .get::<TargetHitClock>(target)
            .unwrap()
            .accepts_target_hit(now),
        "QPC target time must keep advancing while animation is frozen"
    );
    assert_eq!(
        app.world()
            .get::<TargetHitClock>(target)
            .unwrap()
            .last_hit_at,
        0.0
    );
    assert!(
        app.world()
            .get::<TargetHitClock>(target)
            .unwrap()
            .spawn_elapsed
            > 0.29
    );
    assert!(app
        .world()
        .get::<TargetHitClock>(fresh)
        .unwrap()
        .accepts_target_hit(now));
    app.world_mut()
        .resource_mut::<crate::plugins::impact_fx_plugin::ImpactSimulationScale>()
        .frozen = false;
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(201));
    app.world_mut()
        .run_system_once(tick_target_hit_clocks)
        .unwrap();
    let now = app.world().resource::<Time>().elapsed_secs_f64();
    assert!(app
        .world()
        .get::<TargetHitClock>(target)
        .unwrap()
        .accepts_target_hit(now));
}
