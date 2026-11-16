#[test]
fn idle_random_facing_continues_between_thought_ticks_and_resets_on_behavior_change() {
    for mode in [BehaviorMode::Wait, BehaviorMode::Run] {
        let (mut app, npc) = perception_test_app(mode, Vec3::splat(1000.0), None);
        app.world_mut().entity_mut(npc).insert(NpcCombatProfile {
            look: 10.0,
            ..default()
        });
        let seed = (1..10000)
            .find(|seed| {
                let mut rng = *seed;
                if mode == BehaviorMode::Run {
                    let mut roll = || (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32;
                    roll() % 7 != 0 && roll() % 100 < 10 && roll() % 360 != 0
                } else {
                    next_npc_rand(&mut rng) < 0.1
                }
            })
            .unwrap();
        app.world_mut().get_mut::<NpcPatrol>(npc).unwrap().rng = seed;
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
        let target = app
            .world()
            .get::<NpcPatrol>(npc)
            .unwrap()
            .idle_facing
            .expect("random target");
        let first = app.world().get::<MotionState>(npc).unwrap().facing;
        assert!(first.abs() > 0.001);
        app.world_mut()
            .get_mut::<NpcPatrol>(npc)
            .unwrap()
            .think_remaining = 100.0;
        for _ in 0..60 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(16));
            app.update();
        }
        let final_yaw = app.world().get::<MotionState>(npc).unwrap().facing;
        assert!(
            npc_facing_error(target, final_yaw).abs() < 1.0_f32.to_radians(),
            "original integer degree dead zone: mode={mode:?} target={target} final={final_yaw}"
        );
        assert!((final_yaw - first).abs() > 0.001);
        app.world_mut()
            .get_mut::<MissionNpc>(npc)
            .unwrap()
            .forced_behavior = Some(BehaviorMode::Idle);
        app.update();
        assert!(app
            .world()
            .get::<NpcPatrol>(npc)
            .unwrap()
            .idle_facing
            .is_none());
        let mut patrol = app.world_mut().get_mut::<NpcPatrol>(npc).unwrap();
        patrol.idle_facing = Some(1.0);
        patrol.reset_behavior_state();
        assert!(patrol.idle_facing.is_none());
    }
}
