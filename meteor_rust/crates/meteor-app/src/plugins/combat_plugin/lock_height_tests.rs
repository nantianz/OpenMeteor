use super::*;

#[test]
fn production_track_filters_relative_height_and_uses_equipped_weapon_range() {
    let mut app = App::new();
    let mut time: Time = Time::default();
    time.advance_by(std::time::Duration::from_secs_f32(0.05));
    app.insert_resource(time)
        .init_resource::<NetState>()
        .init_resource::<LockOnController>()
        .add_systems(Update, lock_on_tick);
    let fighter = |weapon| Fighter {
        character_id: 0,
        player_slot: 0,
        weapon,
        weapon_slots: [weapon, 5],
        weapon_slot: 0,
        weapon_slot_variants: [0, 0],
        weapon_variant: 0,
    };
    let local = app
        .world_mut()
        .spawn((
            LocalFighter,
            fighter(6),
            Health::default(),
            SmState::default(),
            PoseRuntime::default(),
            EntryBlend::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            Transform::from_xyz(0.0, 40.0, 0.0),
            HeadTrackState::default(),
        ))
        .id();
    let target = app
        .world_mut()
        .spawn((
            fighter(6),
            Health::default(),
            Transform::from_xyz(0.0, 100.0, 3.0),
        ))
        .id();
    for (weapon, target_y, expected) in [
        (6, 100.0, None),
        (6, -20.0, None),
        (6, 40.0, Some(target)),
        (6, 100.0, None),
        (2, 100.0, Some(target)),
        (2, 112.0, None),
        (6, 100.0, None),
    ] {
        app.world_mut().get_mut::<Fighter>(local).unwrap().weapon = weapon;
        app.world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation
            .y = target_y;

        for _ in 0..8 {
            app.update();
        }
        let lock = app.world().resource::<LockOnController>();
        assert_eq!(lock.track_entity, expected, "weapon={weapon}, y={target_y}");
        assert!(lock.lock_entity.is_none());
    }
}

#[test]
fn production_q_and_attack_use_remote_three_dimensional_position() {
    for attack in [false, true] {
        for target_y in [-60.0, 0.0, 60.0] {
            let mut h = spawn_gameplay_harness(6).expect("original combat assets");
            let target = h
                .app
                .world_mut()
                .spawn((
                    RemoteFighter {
                        player_id: "peer".into(),
                    },
                    Fighter {
                        character_id: 1,
                        player_slot: 1,
                        weapon: 6,
                        weapon_slots: [6, 5],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    },
                    Health::default(),
                    Transform::from_xyz(0.0, target_y, 3.0),
                ))
                .id();
            let binding = {
                let keymap = h.app.world().resource::<crate::app_state::Keymap>();
                if attack {
                    keymap.attack
                } else {
                    keymap.lock_toggle
                }
            };
            h.press_binding(binding);
            h.step();
            let lock = h.app.world().resource::<LockOnController>();
            assert_eq!(
                lock.lock_entity,
                (target_y == 0.0).then_some(target),
                "attack={attack}, y={target_y}"
            );
            if attack {
                assert!(
                    is_attacking_state(h.state()),
                    "attack input must have reached combat"
                );
            }
        }
    }
}
