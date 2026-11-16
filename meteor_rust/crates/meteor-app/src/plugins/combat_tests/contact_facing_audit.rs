#[test]
#[ignore = "writes locked-W contact audit CSVs; does not change production behavior"]
#[cfg(not(target_arch = "wasm32"))]
fn diagnostic_locked_w_contact_audit() {
    #[derive(Resource)]
    struct AuditOrder(bool, bool);
    fn bypass_primary_contact(
        active: Res<crate::plugins::fighter_frame::ActiveFighter>,
        order: Res<AuditOrder>,
        local: Query<Entity, With<LocalFighter>>,
        mut frame: ResMut<crate::plugins::character_contact_plugin::CharacterContactFrame>,
    ) {
        if order.1 {
            if let Ok(entity) = local.single() {
                if active.0 == Some(entity) {
                    frame.audit_bypass_primary_contact(entity);
                }
            }
        }
    }
    #[derive(Resource, Default)]
    struct AuditError(f32);
    fn observe_actor_facing(
        active: Res<crate::plugins::fighter_frame::ActiveFighter>,
        order: Res<AuditOrder>,
        lock: Res<LockOnController>,
        mut error: ResMut<AuditError>,
        mut player: Query<(Entity, &Transform, &SmState, &mut MotionState), With<LocalFighter>>,
        targets: Query<&Transform, Without<LocalFighter>>,
    ) {
        let Ok((entity, p, state, mut motion)) = player.single_mut() else {
            return;
        };
        if active.0 != Some(entity) || state.current != FighterState::Name {
            return;
        }
        let Some(t) = lock.lock_entity.and_then(|e| targets.get(e).ok()) else {
            return;
        };
        let d = t.translation - p.translation;
        let yaw = d.x.atan2(d.z);
        let e = yaw - motion.facing;
        error.0 = e.sin().atan2(e.cos()).abs();
        if order.0 {
            motion.facing = yaw;
        }
    }
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../analysis/contact-facing-audit-20260927");
    std::fs::create_dir_all(&out).unwrap();
    let mut summaries = Vec::new();
    for fps in [30, 60, 120] {
        for offset in [0.0, 0.1, 0.6] {
            for kind in ["passive", "npc", "remote", "side-neighbor"] {
                for (reorder, bypass) in [(false, false), (true, false), (false, true)] {
                    if bypass && (kind != "npc" || offset != 0.1) {
                        continue;
                    }
                    if kind == "side-neighbor" && offset != 0.1 {
                        continue;
                    }
                    let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(6, true);
                    h.app
                        .insert_resource(NetState::default())
                        .insert_resource(State::new(AppState::Playing))
                        .insert_resource(AuditOrder(reorder, bypass))
                        .init_resource::<AuditError>()
                        .add_plugins(crate::plugins::CharacterContactPlugin);
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .insert(HeadTrackState::default());
                    configure_lock_updates(&mut h.app);
                    let schedule = crate::plugins::fighter_frame::schedule(&h.app);
                    h.app.add_systems(
                        schedule,
                        bypass_primary_contact
                            .after(crate::plugins::FighterFrameSet::PoseDecision)
                            .before(crate::plugins::FighterFrameSet::Locomotion),
                    );
                    h.app.add_systems(
                        schedule,
                        observe_actor_facing
                            .in_set(crate::plugins::FighterFrameSet::AttackFacing)
                            .after(local_attack_lock_facing),
                    );
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(target)
                        .unwrap()
                        .translation = Vec3::new(offset, 0.0, 3.0);
                    if kind == "npc" || kind == "side-neighbor" {
                        h.app.world_mut().entity_mut(target).insert((
                            NpcFighter,
                            AiIntent::default(),
                            crate::plugins::ai_plugin::NpcPatrol::default(),
                            NpcPerformance {
                                pause_remaining: 100.0,
                                ..default()
                            },
                        ));
                    } else if kind == "remote" {
                        h.app.world_mut().entity_mut(target).insert(
                            crate::plugins::network_plugin::RemoteFighter {
                                player_id: "stationary-peer".into(),
                            },
                        );
                    }
                    if kind == "side-neighbor" {
                        let mut fighter = h.app.world().get::<Fighter>(target).unwrap().clone();
                        fighter.player_slot = 2;
                        h.app.world_mut().spawn((
                            fighter,
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
                                player_id: "side-neighbor".into(),
                            },
                        ));
                    }
                    let camera =
                        crate::plugins::camera_plugin::install_follow_camera_for_test(&mut h.app);
                    {
                        let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                        lock.lock_entity = Some(target);
                        lock.system.lock_target = Some(target.to_bits());
                    }
                    h.press_key(KeyCode::KeyW);
                    let mut csv = String::from(
                        "tick,px,pz,tx,tz,yaw,bearing,vx,vz,camera_yaw,actor_end_yaw_error\n",
                    );
                    let mut last_yaw = 0.0_f32;
                    let mut sum_turn = 0.0_f32;
                    let mut max_error = 0.0_f32;
                    let mut min_distance = f32::INFINITY;
                    let mut max_distance = 0.0_f32;
                    for tick in 0..fps * 5 {
                        h.step_dt(1.0 / fps as f32);
                        let world = h.app.world();
                        let p = h.position();
                        let t = world.get::<Transform>(target).unwrap().translation;
                        let motion = world.get::<MotionState>(h.fighter).unwrap();
                        let yaw = motion.facing;
                        let delta = yaw - last_yaw;
                        if tick >= fps {
                            sum_turn += delta.sin().atan2(delta.cos()).abs();
                            min_distance = min_distance.min(p.distance(t));
                            max_distance = max_distance.max(p.distance(t));
                        }
                        max_error = max_error.max(world.resource::<AuditError>().0);
                        last_yaw = yaw;
                        let forward =
                            world.get::<Transform>(camera).unwrap().rotation * Vec3::NEG_Z;
                        use std::fmt::Write;
                        writeln!(
                            csv,
                            "{tick},{},{},{},{},{yaw},{},{},{},{},{}",
                            p.x,
                            p.z,
                            t.x,
                            t.z,
                            (t.x - p.x).atan2(t.z - p.z),
                            motion.velocity.x,
                            motion.velocity.z,
                            forward.x.atan2(forward.z),
                            world.resource::<AuditError>().0
                        )
                        .unwrap();
                    }
                    let p = h.position();
                    let t = h.app.world().get::<Transform>(target).unwrap().translation;
                    let row = serde_json::json!({"fps":fps,"offset":offset,"kind":kind,"reorder":reorder,"bypassPrimaryCircle":bypass,
                        "turnAfter1sDeg":sum_turn.to_degrees(),"maxOwnFrameYawErrorDeg":max_error.to_degrees(),
                        "minDistance":min_distance,"maxDistance":max_distance,"player":p.to_array(),"target":t.to_array()});
                    eprintln!("{row}");
                    summaries.push(row);
                    std::fs::write(
                        out.join(format!("{fps}-{offset}-{kind}-{reorder}-{bypass}.csv")),
                        csv,
                    )
                    .unwrap();
                }
            }
        }
    }
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summaries).unwrap(),
    )
    .unwrap();
}
