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
