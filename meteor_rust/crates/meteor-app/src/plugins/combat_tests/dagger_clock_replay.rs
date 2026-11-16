#[derive(serde::Deserialize)]
struct DaggerClockCapture {
    sequence: u64,
    start_seconds: f64,
    phase: f32,
    steps: Vec<(u64, f32, f32)>,
}

fn dagger_clock_captures() -> Vec<DaggerClockCapture> {
    serde_json::from_str(include_str!("fixtures/dagger-120fps-capture.json")).unwrap()
}

#[test]
fn captured_dagger_clock_keeps_animation_and_hit_interval_on_elapsed_time() {
    let h = spawn_gameplay_harness(5).expect("original P1 assets");
    let data = &h.app.world().resource::<CharacterAssetsSet>().0["p1"].poses[&258];
    for capture in dagger_clock_captures() {
        let mut pose = PoseRuntime::idle();
        h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 258, true);
        pose.frame = 9440.0;
        let mut clock = crate::plugins::FrameClock::default();
        clock.target_fps = 120;
        clock.accumulator = capture.phase;
        let mut now = capture.start_seconds;
        let mut target = TargetHitClock::default();
        let mut counts = [0; 3];
        for (ns, _, _) in capture.steps {
            let dt = std::time::Duration::from_nanos(ns);
            now += dt.as_secs_f64();
            crate::plugins::advance_frame_clock(&mut clock, dt.as_secs_f32(), 120);
            if clock.sim_dt == 0.0 {
                continue;
            }
            pose.frame = advance_pose_cursor(data, &pose, clock.sim_dt);
            if let Some((index, _)) = active_attack_window(pose.frame, &data.attacks) {
                if target.accepts_target_hit(now) {
                    counts[index as usize] += 1;
                    target.reset(now);
                }
            }
        }
        eprintln!(
            "captured chain {} admission counts={counts:?}",
            capture.sequence
        );
        assert_eq!(counts, [1, 1, 1], "captured chain {}", capture.sequence);
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_dagger_captured_clock_emits_three_hits_and_three_lan_claims() {
    check_dagger_captured_clock(false);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_dagger_captured_clock_with_controller_substeps_preserves_three_hits() {
    check_dagger_captured_clock(true);
}

#[cfg(not(target_arch = "wasm32"))]
fn check_dagger_captured_clock(substeps: bool) {
    for stock in [false, true] {
        for capture in dagger_clock_captures() {
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
                .translation = Vec3::new(0.0, 0.0, 4.0);
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
            pose.frame = 9440.0;
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
            let mut clock = crate::plugins::FrameClock::default();
            clock.target_fps = 120;
            clock.accumulator = capture.phase;
            h.app.insert_resource(clock);
            if substeps {
                crate::plugins::install_frame_clock(&mut h.app);
                h.app.add_plugins(crate::plugins::CharacterContactPlugin);
                h.app.world_mut().resource_mut::<Settings>().target_fps = 120;
                if stock {
                    let mut net = h.app.world_mut().resource_mut::<NetState>();
                    net.stock_npc_ready.clear();
                    net.stock_npc_actors.clear();
                    h.app.world_mut().entity_mut(target).insert(RemoteFighter {
                        player_id: "stock-2".into(),
                    });
                    crate::plugins::network_plugin::install_hit_sender_for_test(&mut h.app);
                }
            }
            let mut counts = [0; 3];
            let mut hp = 10000.0;
            for (ns, _, _) in capture.steps {
                let dt = std::time::Duration::from_nanos(ns);
                if !substeps {
                    crate::plugins::advance_frame_clock(
                        &mut h
                            .app
                            .world_mut()
                            .resource_mut::<crate::plugins::FrameClock>(),
                        dt.as_secs_f32(),
                        120,
                    );
                }
                h.step_duration(dt);
                let after = h.app.world().get::<Health>(target).unwrap().current;
                let claims = take_hits();
                if after < hp || !claims.is_empty() {
                    let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                    let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
                    let (index, _) =
                        active_attack_window(pose.frame, &assets.poses[&258].attacks).unwrap();
                    counts[index as usize] += if stock { claims.len() } else { 1 };
                    assert_eq!(pose.pose_id, 258);
                    if stock {
                        assert_eq!(after, hp, "LAN must await authority");
                    }
                }
                hp = after;
            }
            eprintln!(
                "production captured chain {} stock={stock} counts={counts:?}",
                capture.sequence
            );
            assert_eq!(
                counts,
                [1, 1, 1],
                "chain={} stock={stock}",
                capture.sequence
            );
        }
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_short_attack_window_survives_a_hundred_ms_display_frame() {
    for substeps in [false, true] {
        let (mut h, target, take_hits) = spawn_local_stock_hit_harness_with_order(5, true);
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
            RemoteFighter {
                player_id: "stock-2".into(),
            },
        ));
        {
            let mut net = h.app.world_mut().resource_mut::<NetState>();
            net.stock_npc_ready.clear();
            net.stock_npc_actors.clear();
        }
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
            .translation = Vec3::new(0.0, 0.0, 4.0);
        h.app.world_mut().get_mut::<Health>(target).unwrap().current = 10000.0;
        let schedule = super::super::fighter_frame::schedule(&h.app);
        h.app.add_systems(
            schedule,
            crate::plugins::character_plugin::drive_fighter_animation_for_test
                .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        );
        h.app.add_systems(
            schedule,
            attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
        );
        h.app.add_plugins(crate::plugins::CharacterContactPlugin);
        crate::plugins::network_plugin::install_hit_sender_for_test(&mut h.app);
        crate::plugins::install_frame_clock(&mut h.app);
        if !substeps {
            h.app
                .world_mut()
                .remove_resource::<crate::plugins::FrameClockInstalled>();
        }
        let mut pose = PoseRuntime::idle();
        let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
        assets.apply_pose(&mut pose, 258, true);
        assert_eq!(
            (
                assets.poses[&258].attacks[0].start,
                assets.poses[&258].attacks[0].end
            ),
            (9448, 9450)
        );
        pose.frame = 9447.25;
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
        h.step_duration(std::time::Duration::from_millis(100));
        let claims = take_hits();
        assert_eq!(
            claims.len(),
            usize::from(substeps),
            "substeps={substeps}: sampled the real short-window OBB"
        );
        assert_eq!(
            h.app.world().get::<Health>(target).unwrap().current,
            10000.0,
            "LAN still awaits authority"
        );
        assert!(
            h.app.world().get::<PoseRuntime>(h.fighter).unwrap().frame > 9450.0,
            "the display frame must finish after the short window"
        );
    }
}

#[test]
fn substep_spawn_protection_reads_current_step_without_rewinding_clocks() {
    use IntersectingTargetAdmission::{Accepted, IntervalBlocked, SpawnProtected};
    let mut clock = TargetHitClock {
        spawn_elapsed: 2.0,
        last_hit_at: f64::NEG_INFINITY,
    };
    assert_eq!(
        admit_intersecting_target_at_substep(&mut clock, None, 19.99, 0.01),
        SpawnProtected
    );
    assert_eq!(
        clock.spawn_elapsed, 2.0,
        "the real accumulated age must remain unchanged"
    );
    assert_eq!(
        clock.last_hit_at, 19.99,
        "protected geometry still resets the original target clock"
    );
    assert_eq!(
        admit_intersecting_target_at_substep(&mut clock, None, 20.0, 0.0),
        IntervalBlocked
    );
    clock.clear_hit();
    assert_eq!(
        admit_intersecting_target_at_substep(&mut clock, None, 20.0, 0.0),
        Accepted
    );
    clock.clear_hit();
    clock.spawn_elapsed = 2.125;
    assert_eq!(
        admit_intersecting_target_at_substep(&mut clock, None, 20.0, 0.125),
        Accepted,
        "the strict protection boundary remains age < 2 seconds"
    );
}
