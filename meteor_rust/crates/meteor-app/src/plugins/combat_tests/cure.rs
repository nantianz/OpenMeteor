fn cure_harness(network: bool) -> (GameplayHarness, Entity) {
    attach_cure_fixture(recovery_timing_harness(true), network)
}
fn attach_cure_fixture(mut h: GameplayHarness, network: bool) -> (GameplayHarness, Entity) {
    let (level, _) = crate::plugins::stage_plugin::solo_identity_tests::sn13();
    h.app
        .insert_resource(crate::app_state::LaunchRequest::default())
        .insert_resource(level)
        .init_resource::<MatchEnd>()
        .add_message::<offline_cure::CureCompleted>()
        .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>();
    let schedule = super::super::fighter_frame::schedule(&h.app);
    h.app.add_systems(
        schedule,
        offline_cure::cure_mission_allies
            .after(combat_tick)
            .in_set(crate::plugins::FighterFrameSet::PoseDecision)
            .run_if(super::super::fighter_frame::current_is_local),
    );
    if network {
        let mut net = h.app.world_mut().resource_mut::<NetState>();
        net.status = NetStatus::InRoom;
        net.mode = "assassin".into();
        net.round_active = true;

        net.player_id = "local-vip".into();
        net.stock_vips[0] = Some(net.player_id.clone());
    }
    let position = h.position() + Vec3::X;
    let ally = offline_cure::tests::dead_npc(&mut h.app, 1, 1, position, true);
    (h, ally)
}
#[test]
fn production_assassin_cure_pressed_during_ledge_fall_keeps_movement() {
    for (physics_hz, raw_key) in [(120, true), (173, false)] {
        let (mut h, _) = attach_cure_fixture(dagger_ledge_harness(true, true), true);
        crate::plugins::install_frame_clock(&mut h.app);
        {
            let mut net = h.app.world_mut().resource_mut::<NetState>();
            net.address = "udp-host://127.0.0.1:5230".into();
            net.stock_physics_hz = physics_hz;
        }
        let dt = 1.0 / 60.0;
        h.step_dt(dt);
        h.press_key(KeyCode::KeyW);
        for _ in 0..120 {
            h.step_dt(dt);
            if h.airborne() && h.position().y < 29.99 {
                break;
            }
        }
        let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
        assert!(air.air.airborne && air.air.vertical_speed < 0.0);
        assert_eq!(air.recovery_ground_mode, Some(false));
        assert_eq!(
            h.state(),
            FighterState::Name,
            "walk-off ledge retains the combat state"
        );
        if raw_key {
            h.press_key(KeyCode::KeyT);
        } else {
            h.app
                .world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(KeyCode::KeyT);
        }
        h.step_dt(dt);
        let cast_in_air = h.state() == FighterState::Cure;
        for _ in 0..180 {
            h.step_dt(dt);
        }
        h.release_key(KeyCode::KeyT);
        assert!(!h.airborne());
        assert_eq!(
            h.state(),
            FighterState::Name,
            "stuck at pose {} after landing, physics_hz={physics_hz}, cast_in_air={cast_in_air}",
            h.pose_id()
        );
        assert!(!cast_in_air);
        assert!(h
            .app
            .world_mut()
            .resource_mut::<PendingNetActions>()
            .take()
            .is_empty());
        let landed = h.position();
        for _ in 0..12 {
            h.step_dt(dt);
        }
        assert!(h.position().distance(landed) > 0.5);
        h.release_key(KeyCode::KeyW);
        let cure_dt = 1.0 / physics_hz as f32;
        start_cure(&mut h, cure_dt);
        finish_cure(&mut h, cure_dt, physics_hz as usize);
        assert_eq!(
            h.app.world_mut().resource_mut::<PendingNetActions>().take(),
            ["cure"]
        );
    }
}
#[test]
fn production_cure_keeps_its_pose_when_ground_mode_has_no_walkable_support() {
    let (mut h, _) = cure_harness(true);
    crate::plugins::install_frame_clock(&mut h.app);
    let dt = 1.0 / 120.0;
    h.step_dt(dt);
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation
        .y = 6.0;
    {
        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
        air.air.start_fall(0.0, 0.0);
        air.air.vertical_speed = -8.0;
        air.ground_contact = false;
        air.recovery_ground_mode = Some(true);
    }
    let cast_position = h.position();
    start_cure(&mut h, dt);
    for _ in 0..12 {
        h.step_dt(dt);
        assert_eq!(h.pose_id(), 22);
        assert_eq!(h.position(), cast_position);
    }
    finish_cure(&mut h, dt, 120);
    assert_eq!(
        h.app.world_mut().resource_mut::<PendingNetActions>().take(),
        ["cure"]
    );
}

fn start_cure(h: &mut GameplayHarness, dt: f32) {
    let binding = h.app.world().resource::<crate::app_state::Keymap>().cure;
    h.press_binding(binding);
    h.step_dt(dt);
    h.release_binding(binding);
    assert_eq!(h.state(), FighterState::Cure);
    assert_eq!(h.pose_id(), 22);
}

fn finish_cure(h: &mut GameplayHarness, dt: f32, fps: usize) {
    let mut saw_recovery = false;
    for _ in 0..fps * 5 {
        let before = h.app.world().get::<PoseRuntime>(h.fighter).unwrap().clone();
        saw_recovery |= before.pose_id == 34;
        h.step_dt(dt);
        if h.state() == FighterState::Name {
            assert!(saw_recovery);
            assert_eq!(before.pose_id, 34);
            assert!(
                before.finished(),
                "Cure must finish the authored terminal clip"
            );
            return;
        }
    }
    panic!("Cure did not complete its authored chain");
}

#[test]
fn production_cure_revives_at_authored_completion_and_only_once() {
    for fps in [30, 60, 120] {
        let dt = 1.0 / fps as f32;
        let (mut h, ally) = cure_harness(false);
        start_cure(&mut h, dt);
        for _ in 0..fps * 5 {
            if h.pose_id() != 22 {
                break;
            }
            assert_eq!(h.app.world().get::<Health>(ally).unwrap().current, 0.0);
            h.step_dt(dt);
        }
        assert_eq!(h.pose_id(), 34);
        assert!(h.app.world().get::<DeathRespawn>(ally).unwrap().active);
        finish_cure(&mut h, dt, fps);
        assert_eq!(h.app.world().get::<Health>(ally).unwrap().current, 2000.0);
        assert!(!h.app.world().get::<DeathRespawn>(ally).unwrap().active);
        h.app
            .world_mut()
            .get_mut::<DeathRespawn>(ally)
            .unwrap()
            .active = true;
        h.app.world_mut().get_mut::<Health>(ally).unwrap().current = 0.0;
        h.steps(10);
        assert!(h.app.world().get::<DeathRespawn>(ally).unwrap().active);
    }
}

#[test]
fn production_cure_hit_interrupt_cancels_entry_recovery_and_terminal_frame() {
    for fps in [30, 60, 120] {
        for interrupt_at in [0, 1, 2] {
            let dt = 1.0 / fps as f32;
            let (mut h, ally) = cure_harness(false);
            start_cure(&mut h, dt);
            if interrupt_at > 0 {
                for _ in 0..fps * 4 {
                    let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                    if pose.pose_id == 34 && (interrupt_at == 1 || pose.finished()) {
                        break;
                    }
                    h.step_dt(dt);
                }
                assert_eq!(h.pose_id(), 34);
            }
            install_timing_reaction(&mut h, 93, HitReactKind::BeHit, 0.2);
            for _ in 0..fps * 5 {
                h.step_dt(dt);
                assert_eq!(
                    h.app.world().get::<Health>(ally).unwrap().current,
                    0.0,
                    "fps={fps}, interrupt_at={interrupt_at}"
                );
            }
            assert_eq!(h.state(), FighterState::Name);
            start_cure(&mut h, dt);
            finish_cure(&mut h, dt, fps);
            assert_eq!(
                h.app.world().get::<Health>(ally).unwrap().current,
                2000.0,
                "a new complete cast must remain usable after interruption"
            );
        }
    }
}

#[test]
fn production_network_cure_queues_only_the_completed_cast() {
    let dt = 1.0 / 60.0;
    let (mut h, ally) = cure_harness(true);
    start_cure(&mut h, dt);
    assert!(h
        .app
        .world_mut()
        .resource_mut::<PendingNetActions>()
        .take()
        .is_empty());
    install_timing_reaction(&mut h, 93, HitReactKind::BeHit, 0.2);
    for _ in 0..300 {
        h.step_dt(dt);
        assert!(h
            .app
            .world_mut()
            .resource_mut::<PendingNetActions>()
            .take()
            .is_empty());
    }
    start_cure(&mut h, dt);
    finish_cure(&mut h, dt, 60);
    assert_eq!(
        h.app.world_mut().resource_mut::<PendingNetActions>().take(),
        ["cure"]
    );
    assert_eq!(
        h.app.world().get::<Health>(ally).unwrap().current,
        0.0,
        "network revival remains authority-owned"
    );
    h.steps(5);
    assert!(h
        .app
        .world_mut()
        .resource_mut::<PendingNetActions>()
        .take()
        .is_empty());
}
