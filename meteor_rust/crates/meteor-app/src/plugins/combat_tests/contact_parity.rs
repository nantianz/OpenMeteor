#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_ground_lock_turns_after_both_movers() {
    #[derive(Resource, Default)]
    struct FacingSample(Option<(f32, bool)>);
    fn sample(
        active: Res<crate::plugins::fighter_frame::ActiveFighter>,
        lock: Res<LockOnController>,
        mut sample: ResMut<FacingSample>,
        local: Query<(Entity, &Transform, &MotionState, &FighterAir), With<LocalFighter>>,
        targets: Query<&Transform, Without<LocalFighter>>,
    ) {
        let (entity, p, motion, air) = local.single().unwrap();
        if active.0 != Some(entity) {
            return;
        }
        let t = targets.get(lock.lock_entity.unwrap()).unwrap().translation;
        let d = t - p.translation;
        let error = motion.facing - d.x.atan2(d.z);
        sample.0 = Some((
            error.sin().atan2(error.cos()).abs(),
            air.ground_lock_facing_pending,
        ));
    }
    for fps in [30, 60, 120] {
        for recovery in [false, true] {
            for player_first in [false, true] {
                let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(6, true);
                h.app
                    .insert_resource(NetState::default())
                    .insert_resource(State::new(AppState::Playing))
                    .init_resource::<FacingSample>()
                    .add_plugins(crate::plugins::CharacterContactPlugin);
                h.app.world_mut().entity_mut(target).insert((
                    NpcFighter,
                    AiIntent::default(),
                    crate::plugins::ai_plugin::NpcPatrol::default(),
                    NpcPerformance {
                        pause_remaining: 100.0,
                        ..default()
                    },
                ));
                let mut other = h.app.world().get::<Fighter>(target).unwrap().clone();
                other.player_slot = 2;
                h.app.world_mut().spawn((
                    other,
                    Transform::from_xyz(-2.0, 0.0, 1.5),
                    PoseRuntime::idle(),
                    SmState::default(),
                    Health::default(),
                    MotionState {
                        velocity: Vec3::ZERO,
                        facing: 0.0,
                        movement_mode: meteor_core::MovementMode::Idle,
                    },
                    crate::plugins::network_plugin::RemoteFighter {
                        player_id: "side".into(),
                    },
                ));
                if !player_first {
                    let fighter = h.app.world().get::<Fighter>(h.fighter).unwrap().clone();
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .remove::<Fighter>()
                        .insert(fighter);
                }
                if recovery {
                    h.app
                        .world_mut()
                        .get_mut::<FighterAir>(h.fighter)
                        .unwrap()
                        .recovery_ground_mode = Some(true);
                }
                {
                    let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                    lock.lock_entity = Some(target);
                    lock.system.lock_target = Some(target.to_bits());
                }
                h.app.add_systems(
                    crate::plugins::fighter_frame::FighterUpdate,
                    sample
                        .in_set(crate::plugins::FighterFrameSet::AttackFacing)
                        .after(crate::plugins::movement_plugin::finalize_ground_lock_facing),
                );
                h.press_key(KeyCode::KeyW);
                let mut max_lateral = 0.0_f32;
                for tick in 0..fps {
                    h.app.world_mut().resource_mut::<FacingSample>().0 = None;
                    h.step_dt(1.0 / fps as f32);
                    max_lateral = max_lateral.max(h.position().x.abs());
                    let (error, pending) = h.app.world().resource::<FacingSample>().0.unwrap();
                    assert!(error < 1e-5 && !pending,
                        "{fps} recovery={recovery} first={player_first} tick={tick}: {error}/{pending}");
                }
                assert!(
                    max_lateral > 0.001,
                    "side repulsion must exercise a non-collinear mover: fps={fps} recovery={recovery} first={player_first} lateral={max_lateral}"
                );
            }
        }
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn mrc_ground_lock_tracks_between_queue_e_samples_before_dagger_takeoff() {
    use crate::plugins::fighter_frame::FighterStepEnd;
    use crate::plugins::replay_plugin::{MrcFrameOverride, MrcPlayback, ReplayPlugin};
    use crate::plugins::{FrameClock, FrameClockInstalled};
    use meteor_asset::mrc::{
        MrcDocument, MrcQueueBEntry, MrcQueueEEntry, MRC_HEADER_SIZE, MRC_MAGIC,
    };
    use meteor_core::MovementMode;

    #[derive(Resource, Default)]
    struct FacingSamples(Vec<(Vec3, f32, MovementMode)>);

    let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(5, true);
    let target_position = Vec3::X * 8.0;
    let dt = 1.0 / 30.0;
    h.app
        .world_mut()
        .get_mut::<Transform>(target)
        .unwrap()
        .translation = target_position;
    {
        let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
        lock.lock_entity = Some(target);
        lock.system.lock_target = Some(target.to_bits());
    }
    h.app
        .world_mut()
        .get_mut::<MotionState>(h.fighter)
        .unwrap()
        .facing = -1.0;
    h.app
        .insert_resource(State::new(AppState::Playing))
        .insert_resource(FrameClockInstalled)
        .insert_resource(FrameClock {
            sim_dt: dt,
            wall_dt: dt,
            wall_step: dt,
            ..default()
        })
        .add_plugins(ReplayPlugin)
        .insert_resource(MrcPlayback {
            active: true,
            doc: Some(MrcDocument {
                magic: MRC_MAGIC,
                header_size: MRC_HEADER_SIZE,
                mark0: 0,
                mark1: 0,
                tail: 0,
                state: [0; meteor_asset::mrc::MRC_STATE_SIZE],
                meta: [0; 8],
                queue_a: Vec::new(),
                queue_c: Vec::new(),
                queue_d: Vec::new(),
                queue_b: [
                    (0.001, 0x57, 1),
                    (0.020, 0x1200, 1),
                    (0.029, 0x1200, 0),
                    (1.0, 0x57, 0),
                ]
                .into_iter()
                .map(|(time, code, value)| MrcQueueBEntry { time, code, value })
                .collect(),
                queue_e: vec![MrcQueueEEntry {
                    a: 0,
                    b: 1.0_f32.to_bits(),
                    time: 0.001,
                }],
            }),
            ..default()
        })
        .init_resource::<FacingSamples>()
        .add_systems(
            FighterStepEnd,
            |mut samples: ResMut<FacingSamples>,
             local: Query<(&Transform, &MotionState), With<LocalFighter>>| {
                let (transform, motion) = local.single().unwrap();
                samples
                    .0
                    .push((transform.translation, motion.facing, motion.movement_mode));
            },
        );

    let start = h.position();
    h.step_dt(dt);
    let world = h.app.world();
    let samples = &world.resource::<FacingSamples>().0;
    let [(first, recorded_yaw, _), (second, tracked_yaw, _), (_, before_jump_yaw, _), (_, takeoff_yaw, takeoff_mode)] =
        samples.as_slice()
    else {
        panic!("expected four controller steps: {samples:?}");
    };
    let to_first = target_position - *first;
    assert!(
        (recorded_yaw - to_first.x.atan2(to_first.z)).abs() < 1e-5,
        "ground Lock must still turn on the Queue E write step (REPLAY-026)"
    );
    assert!(
        first.distance(start) < 1e-5,
        "the Lock turn happens before the recorded W edge moves the fighter"
    );
    let to_target = target_position - *second;
    assert!((tracked_yaw - to_target.x.atan2(to_target.z)).abs() < 1e-5);
    assert_eq!(*takeoff_mode, MovementMode::Airborne);
    assert_eq!(*takeoff_yaw, *before_jump_yaw);
    assert_eq!(
        world.resource::<LockOnController>().lock_entity,
        Some(target)
    );
    let frame = world.resource::<MrcFrameOverride>();
    assert!(frame.facing_applied);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn mrc_remote_death_releases_lock_until_recorded_respawn() {
    use crate::plugins::replay_plugin::{MrcPlayback, ReplayPlugin};
    use crate::plugins::{FrameClock, FrameClockInstalled};
    use meteor_asset::mrc::{MrcDocument, MrcQueueAEntry, MRC_HEADER_SIZE, MRC_MAGIC};
    use meteor_network::{role_state, stock_protocol};

    let mut h = spawn_gameplay_harness_with_order(5, true).expect("original p1 resources");
    h.app
        .world_mut()
        .entity_mut(h.fighter)
        .insert(HeadTrackState::default());
    configure_lock_updates(&mut h.app);
    let role = role_state::role_state_from_runtime(role_state::RoleStateRuntime {
        position: role_state::RoleVec3 {
            x: 0.0,
            y: 0.0,
            z: 8.0,
        },
        forward: role_state::forward_from_yaw(0.0),
        velocity: role_state::RoleVec3::ZERO,
        pose_id: 0,
        grounded: true,
        guard_route: false,
        playback_arg: 0,
        pose_sequence: 1,
    });
    let snapshot = |actor: u16, time| {
        let mut blob = actor.to_le_bytes().to_vec();
        blob.extend_from_slice(&role_state::encode_event2_state(role));
        MrcQueueAEntry { id: 2, time, blob }
    };
    let life = |id, actor: u32, time| MrcQueueAEntry {
        id: u32::from(id),
        time,
        blob: actor.to_le_bytes().to_vec(),
    };
    let dt = 1.0 / 60.0;
    let mut state = [0; meteor_asset::mrc::MRC_STATE_SIZE];
    state[4..8].copy_from_slice(&1u32.to_le_bytes());
    h.app
        .insert_resource(NetState::default())
        .insert_resource(State::new(AppState::Playing))
        .insert_resource(FrameClockInstalled)
        .insert_resource(FrameClock {
            sim_dt: dt,
            wall_dt: dt,
            wall_step: dt,
            ..default()
        })
        .add_plugins(ReplayPlugin)
        .insert_resource(MrcPlayback {
            active: true,
            doc: Some(MrcDocument {
                magic: MRC_MAGIC,
                header_size: MRC_HEADER_SIZE,
                mark0: 0,
                mark1: 0,
                tail: 0,
                state,
                meta: [0; 8],
                queue_a: vec![
                    snapshot(3, 0.001),
                    life(stock_protocol::STOCK_EVENT_ENVIRONMENT_DEATH, 3, 0.020),
                    life(stock_protocol::STOCK_EVENT_ENVIRONMENT_DEATH, 4, 0.023),
                    snapshot(4, 0.030),
                    life(stock_protocol::STOCK_EVENT_RESPAWN, 3, 0.040),
                ],
                queue_b: Vec::new(),
                queue_c: Vec::new(),
                queue_d: Vec::new(),
                queue_e: Vec::new(),
            }),
            ..default()
        });

    h.step_dt(dt);
    let target = h
        .app
        .world_mut()
        .query::<(Entity, &RemoteFighter)>()
        .iter(h.app.world())
        .find(|(_, remote)| remote.player_id == "mrc:3")
        .unwrap()
        .0;
    let track = &h.app.world().resource::<LockOnController>();
    assert_eq!(track.track_entity, Some(target));
    {
        let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
        lock.system.lock_target = Some(target.to_bits());
        lock.lock_entity = Some(target);
    }

    h.step_dt(dt);
    let late = h
        .app
        .world_mut()
        .query::<(Entity, &RemoteFighter)>()
        .iter(h.app.world())
        .find(|(_, remote)| remote.player_id == "mrc:4")
        .unwrap()
        .0;
    for dead in [target, late] {
        assert_eq!(h.app.world().get::<Health>(dead).unwrap().current, 0.0);
        assert!(h.app.world().get::<DeathRespawn>(dead).unwrap().active);
    }
    let lock = h.app.world().resource::<LockOnController>();
    assert_eq!(lock.lock_entity, None);
    assert_eq!(lock.system.lock_target, None);
    assert_eq!(lock.track_entity, None);

    h.step_dt(dt);
    let world = h.app.world();
    assert!(!world.get::<DeathRespawn>(target).unwrap().active);
    assert_eq!(world.get::<Health>(target).unwrap().current, 2000.0);
    let lock = world.resource::<LockOnController>();
    assert_eq!(lock.track_entity, Some(target));
    assert_eq!(lock.lock_entity, None);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_overlapping_ground_and_attack_use_target_matrix_fallback() {
    use bevy::ecs::system::RunSystemOnce;
    for yaw in [0.5_f32, 1.57, -0.75] {
        for mode in ["ground", "attack", "npc"] {
            let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(6, true);
            let p = h.position();
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = p;
            h.app
                .world_mut()
                .get_mut::<MotionState>(target)
                .unwrap()
                .facing = yaw;
            {
                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                lock.lock_entity = Some(target);
                lock.system.lock_target = Some(target.to_bits());
            }
            if mode == "ground" {
                h.app
                    .world_mut()
                    .get_mut::<FighterAir>(h.fighter)
                    .unwrap()
                    .ground_lock_facing_pending = true;
                h.app
                    .world_mut()
                    .run_system_once(crate::plugins::movement_plugin::finalize_ground_lock_facing)
                    .unwrap();
            } else {
                h.app
                    .world_mut()
                    .get_mut::<SmState>(h.fighter)
                    .unwrap()
                    .current = FighterState::Attack1;
                let id = h.pose_id();
                *h.app.world_mut().get_mut::<EntryBlend>(h.fighter).unwrap() = EntryBlend {
                    pose_id: id,
                    duration: 0.12,
                    ..default()
                };
                if mode == "npc" {
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .remove::<LocalFighter>()
                        .insert(NpcFighter);
                    h.app
                        .world_mut()
                        .get_mut::<AiIntent>(h.fighter)
                        .unwrap()
                        .lock_target = Some(target);
                    h.app.world_mut().entity_mut(target).insert(LocalFighter);
                    h.app.world_mut().run_system_once(npc_lock_on_tick).unwrap();
                } else {
                    h.app
                        .world_mut()
                        .run_system_once(local_attack_lock_facing)
                        .unwrap();
                }
            }
            let actual = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
            let error = actual + yaw;
            assert!(
                error.sin().atan2(error.cos()).abs() < 1e-5,
                "{mode} {yaw}: {actual}"
            );
        }
    }
}
