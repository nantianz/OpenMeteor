#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "COMBAT-075 ranged/periodic hit entry audit; writes analysis results"]
fn production_falling_ranged_and_periodic_hit_audit() {
    use crate::components::fighter::{FighterStatusEffects, VisualRootPin};
    use crate::components::projectile::RangedSegmentHit;
    use bevy::ecs::system::RunSystemOnce;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut cases = Vec::new();
    for fps in [30, 60, 120] {
        for (attack_pose, weapon) in [(200, 1), (210, 1), (206, 3), (218, 2), (224, 2)] {
            for late_owner in [false, true] {
                let (mut h, victim, _) = spawn_local_stock_hit_harness_with_order(weapon, true);
                let owner = h.fighter;
                h.app.world_mut().entity_mut(owner).remove::<LocalFighter>();
                h.app.world_mut().entity_mut(victim).insert((
                    LocalFighter,
                    AiIntent::default(),
                    FighterWall::default(),
                    VisualRootPin::default(),
                ));

                if late_owner {
                    let fighter = h.app.world().get::<Fighter>(owner).unwrap().clone();
                    h.app.world_mut().entity_mut(owner).remove::<Fighter>();
                    h.app.world_mut().entity_mut(owner).insert(fighter);
                }
                h.fighter = victim;
                h.app
                    .insert_resource(NetState::default())
                    .insert_resource(State::new(AppState::Playing))
                    .insert_resource(crate::plugins::FrameClock {
                        sim_dt: 1.0 / fps as f32,
                        ..default()
                    });
                configure_periodic_status_updates(&mut h.app);
                let schedule = super::super::fighter_frame::FighterUpdate;
                crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
                h.app.add_systems(
                    schedule.clone(),
                    (tick_target_hit_clocks, attack_windows)
                        .chain()
                        .in_set(crate::plugins::FighterFrameSet::HitJudge),
                );
                h.app.add_systems(
                    schedule,
                    hit_reaction_root_motion.in_set(crate::plugins::FighterFrameSet::RootMotion),
                );
                h.app
                    .world_mut()
                    .run_system_once(load_combat_resource_data)
                    .unwrap();
                let attack = h.app.world().resource::<CharacterAssetsSet>().0["p1"].poses
                    [&attack_pose]
                    .attacks[0]
                    .clone();
                let mut pose = PoseRuntime::idle();
                h.app.world().resource::<CharacterAssetsSet>().0["p1"]
                    .apply_pose(&mut pose, 152, true);
                h.app.world_mut().entity_mut(victim).insert(pose);
                h.app
                    .world_mut()
                    .get_mut::<Transform>(victim)
                    .unwrap()
                    .translation = Vec3::new(0.0, 350.0, 16.0);
                h.app.world_mut().get_mut::<Health>(victim).unwrap().current = 10000.0;
                h.app.world_mut().get_mut::<Health>(victim).unwrap().max = 10000.0;
                h.app
                    .world_mut()
                    .get_mut::<SmState>(victim)
                    .unwrap()
                    .current = FighterState::Jump;
                h.app
                    .world_mut()
                    .get_mut::<CombatState>(victim)
                    .unwrap()
                    .state = FighterState::Jump;
                {
                    let mut air = h.app.world_mut().get_mut::<FighterAir>(victim).unwrap();
                    air.air.start_fall(0.0, 0.0);
                    air.air.vertical_speed = -20.0;
                    air.ground_contact = false;
                }
                {
                    let mut motion = h.app.world_mut().get_mut::<MotionState>(victim).unwrap();
                    motion.facing = 0.0;
                    motion.velocity = Vec3::new(0.0, -20.0, 0.0);
                    motion.movement_mode = meteor_core::MovementMode::Airborne;
                }
                h.app
                    .world_mut()
                    .get_mut::<TargetHitClock>(victim)
                    .unwrap()
                    .spawn_elapsed = 4.0;
                let dt = 1.0 / fps as f32;
                let mut history = Vec::new();
                let mut old = None;
                let mut seen_status = false;
                let mut seen_periodic = false;
                let mut seen_airborne_periodic = false;
                for tick in 0..fps * 7 {
                    if tick == 0 || tick == fps / 3 {
                        let point = h.position();
                        let projectile = h.app.world_mut().spawn_empty().id();
                        h.app
                            .world_mut()
                            .resource_mut::<PendingRangedHits>()
                            .0
                            .push(RangedSegmentHit {
                                projectile,
                                source: RangedHitSource {
                                    owner,
                                    weapon,
                                    weapon_stats: None,
                                    pose_id: attack_pose,
                                    pose_frame: attack.start as f32,
                                    pose_start: attack.start,
                                    pose_instance_id: tick + 1,
                                    attack_index: 0,
                                    attack: attack.clone(),
                                    player_slot: 1,
                                    combat_team: 2,
                                    script_character_id: 1,
                                    script_team: None,
                                    attack_origin: point + Vec3::Z * 10.0,
                                    attack_forward: Vec3::NEG_Z,
                                },
                                start: point + Vec3::new(0.0, 2.0, 2.0),
                                end: point + Vec3::new(0.0, 2.0, -2.0),
                                hit_scale: if weapon == 2 { 2.6 } else { 1.0 },
                                kind: match weapon {
                                    1 => ProjectileKind::Dart,
                                    2 => ProjectileKind::FlyingWheel,
                                    _ => ProjectileKind::GunRay,
                                },
                                despawn_after: true,
                            });
                    }
                    let before = h.app.world().get::<Health>(victim).unwrap().current;
                    h.step_dt(dt);
                    if tick == 0 || tick == fps / 3 {
                        assert!(h.app.world().get::<Health>(victim).unwrap().current < before,
                            "ray must actually hit: attack={attack_pose} fps={fps} late={late_owner} tick={tick}");
                        assert_eq!(h.state(), FighterState::BeHit);
                    }
                    let pose = h.pose_id();
                    let reacting = h
                        .app
                        .world()
                        .get::<FighterHitReaction>(victim)
                        .unwrap()
                        .current
                        .is_some();
                    assert!(!reacting || !matches!(pose, 152 | 154 | 156 | 158 | 180),
                        "ranged response replaced by falling clip: attack={attack_pose} fps={fps} late={late_owner} tick={tick}");
                    let status = h.app.world().get::<FighterStatusEffects>(victim).unwrap();
                    seen_status |= !status.effects.active.is_empty();
                    seen_periodic |= tick >= fps && pose == 90;
                    seen_airborne_periodic |= tick >= fps && pose == 90 && h.airborne();
                    let key = (pose, h.state(), h.airborne());
                    if old != Some(key) {
                        history.push(serde_json::json!({"tick":tick,"pose":pose,"state":format!("{:?}",h.state()),
                            "air":h.airborne(),"position":h.position().to_array()}));
                        old = Some(key);
                    }
                }
                assert!(
                    !h.airborne(),
                    "ray aftermath must settle: attack={attack_pose} fps={fps} late={late_owner}"
                );
                assert!(matches!(h.state(), FighterState::Name | FighterState::Down),
                    "ray aftermath remains locked: attack={attack_pose} fps={fps} late={late_owner} state={:?} pose={}",h.state(),h.pose_id());
                if attack_pose == 210 {
                    assert!(
                        seen_status && seen_periodic && seen_airborne_periodic,
                        "poison dart must exercise its periodic Pose90 dispatch"
                    );
                }
                cases.push(
                    serde_json::json!({"attack":attack_pose,"fps":fps,"lateOwner":late_owner,
                    "seenStatus":seen_status,"seenPeriodic":seen_periodic,"seenAirbornePeriodic":seen_airborne_periodic,"history":history}),
                );
            }
        }
    }
    let output = falling_hit_audit_output(&root, "ranged-matrix.json");
    std::fs::write(output, serde_json::to_string_pretty(&cases).unwrap()).unwrap();
    eprintln!(
        "falling ranged/periodic audit: {} cases passed",
        cases.len()
    );
}
