fn cure_harness(network: bool) -> (GameplayHarness, Entity) {
    let mut h = recovery_timing_harness(true);
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
