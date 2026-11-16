#[test]
fn grounded_attacks_compose_authored_xyz_before_floor_rejection() {
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/ground-clamp-oracle.json")).unwrap();
    for sample in golden["samples"].as_array().unwrap() {
        for npc in [false, true] {
            let mut h = spawn_gameplay_harness_with_order(5, true).unwrap();
            if npc {
                h.app
                    .world_mut()
                    .entity_mut(h.fighter)
                    .remove::<LocalFighter>();
            }
            let id = sample["pose"].as_u64().unwrap() as u16;
            let dt = 1.0 / sample["fps"].as_u64().unwrap() as f32;
            let carry = sample["carry"].as_f64().unwrap() as f32;
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, id, true);

            pose.frame = sample["frame"].as_f64().unwrap() as f32 - 15.0 * dt;
            h.app.world_mut().entity_mut(h.fighter).insert(pose);
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::ZERO;
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .velocity = Vec3::Z * carry;
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
            h.step_dt(dt);
            let expected = Vec3::from_array(std::array::from_fn(|i| {
                sample["flatAcceptedWorld"][i].as_f64().unwrap() as f32
            }));
            assert!(
                (h.position() - expected).length() < 2e-5,
                "original XYZ mover: {sample}, NPC={npc}, actual={:?}",
                h.position()
            );
            if h.airborne() {
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                assert!(
                    (air.air.planar_x - expected.x / dt).abs() < 2e-4
                        && (air.air.planar_z - expected.z / dt).abs() < 2e-4,
                    "an off-floor authored step must retain primary planar speed at handoff"
                );
            }
        }
    }
}

#[test]
fn sampled_hit_boxes_follow_animation_refresh_instead_of_parent_movement() {
    use bevy::ecs::system::RunSystemOnce;
    fn boxes(
        geometry: HitGeometryQueries,
        assets: Res<CharacterAssetsSet>,
        pair: Res<CollisionSamplePair>,
    ) -> (Vec<Obb>, Option<Obb>, Vec<Obb>) {
        let asset = &assets.0["p1"];
        (
            geometry.hurt_obbs(pair.0, asset),
            fighter_whole_body_obb(pair.0, asset, &geometry),
            geometry.attack_obbs(
                pair.0,
                &pair.1,
                &WeaponForm::default(),
                253,
                asset,
                &["weapon".into(), "bau R Hand".into()],
            ),
        )
    }
    #[derive(Resource)]
    struct CollisionSamplePair(Entity, Fighter);
    let (mut h, _, _) = spawn_local_stock_hit_harness_with_order(5, true);
    let meta = h.app.world().get::<Fighter>(h.fighter).unwrap().clone();
    h.app.insert_resource(CollisionSamplePair(h.fighter, meta));
    crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
    let before = h.app.world_mut().run_system_once(boxes).unwrap();
    assert!(!before.0.is_empty() && before.1.is_some() && !before.2.is_empty());

    let shift = Vec3::new(10.0, 2.0, -4.0) * meteor_core::MODEL_SCALE;
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation += shift;
    let after_move = h.app.world_mut().run_system_once(boxes).unwrap();
    let centers = |values: &(Vec<Obb>, Option<Obb>, Vec<Obb>)| {
        values
            .0
            .iter()
            .chain(values.1.iter())
            .chain(values.2.iter())
            .map(|b| Vec3::new(b.center.x, b.center.y, b.center.z))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        centers(&before),
        centers(&after_move),
        "model movement must retain sampled boxes"
    );
    h.app
        .world_mut()
        .get_mut::<EntryBlend>(h.fighter)
        .unwrap()
        .preserve_sample_once = true;
    crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
    assert_eq!(
        centers(&before),
        centers(&h.app.world_mut().run_system_once(boxes).unwrap()),
        "a Link installation without sampling cannot refresh collision matrices"
    );
    h.app
        .world_mut()
        .get_mut::<EntryBlend>(h.fighter)
        .unwrap()
        .preserve_sample_once = false;
    crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
    let refreshed = h.app.world_mut().run_system_once(boxes).unwrap();
    for (a, b) in centers(&before).iter().zip(centers(&refreshed)) {
        assert!(
            (b - a - shift).length() < 1e-5,
            "animation refresh must adopt the new model position"
        );
    }
}

#[test]
#[ignore = "Original combat movement audit; writes per-tick trajectories"]
fn diagnose_dagger_launch_and_stomp_trajectories() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut rows = Vec::new();
    for fps in [30, 60, 120, 165] {
        for source_pose in [253, 256] {
            for npc in [false, true] {
                let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(5, true);
                h.app.insert_resource(NetState::default());
                if npc {
                    h.app.insert_resource(State::new(AppState::Playing));
                    h.app.add_plugins(crate::plugins::CharacterContactPlugin);
                    h.app.world_mut().entity_mut(target).insert((
                        NpcFighter,
                        AiIntent::default(),
                        crate::plugins::ai_plugin::NpcPatrol::default(),
                        NpcPerformance {
                            pause_remaining: 100.0,
                            ..default()
                        },
                        NpcCombatProfile {
                            attack1: 0.0,
                            attack2: 0.0,
                            attack3: 0.0,
                            ..default()
                        },
                    ));
                }
                h.app
                    .world_mut()
                    .entity_mut(target)
                    .insert((WeaponForm::default(), FighterHitReaction::default()));
                for entity in [h.fighter, target] {
                    h.app
                        .world_mut()
                        .entity_mut(entity)
                        .insert(crate::components::fighter::VisualRootPin::default());
                    h.app.world_mut().get_mut::<Health>(entity).unwrap().current = 10000.0;
                }
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
                    crate::plugins::ai_plugin::npc_ballistic_air_root_motion
                        .after(crate::plugins::movement_plugin::attack_root_motion)
                        .before(hit_reaction_root_motion)
                        .in_set(crate::plugins::FighterFrameSet::RootMotion),
                );
                h.app.add_systems(
                    schedule,
                    hit_reaction_root_motion
                        .after(crate::plugins::movement_plugin::attack_root_motion)
                        .in_set(crate::plugins::FighterFrameSet::RootMotion),
                );
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = Vec3::new(0.0, 0.0, 2.0);
                let mut pose = PoseRuntime::idle();
                h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(
                    &mut pose,
                    source_pose,
                    true,
                );
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
                if source_pose == 256 {
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(h.fighter)
                        .unwrap()
                        .translation
                        .y = 3.5;
                    h.app
                        .world_mut()
                        .get_mut::<FighterAir>(h.fighter)
                        .unwrap()
                        .air
                        .start_fall(0.0, 0.0);
                    h.app
                        .world_mut()
                        .get_mut::<FighterAir>(h.fighter)
                        .unwrap()
                        .air
                        .vertical_speed = -14.0;
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .velocity
                        .y = -14.0;
                }
                let mut hp = 10000.0;
                let mut hits = 0;
                let mut peak = 0.0_f32;
                for tick in 0..fps * 3 {
                    let before_pose = h.app.world().get::<PoseRuntime>(target).unwrap().clone();
                    let before = h.app.world().get::<Transform>(target).unwrap().translation;
                    let before_v = h
                        .app
                        .world()
                        .get::<FighterHitReaction>(target)
                        .unwrap()
                        .vertical_velocity;
                    h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / fps as u64));
                    let p = h.app.world().get::<PoseRuntime>(target).unwrap();
                    let position = h.app.world().get::<Transform>(target).unwrap().translation;
                    let reaction = h.app.world().get::<FighterHitReaction>(target).unwrap();
                    let health = h.app.world().get::<Health>(target).unwrap().current;
                    let blend = h.app.world().get::<EntryBlend>(target).unwrap();
                    hits += usize::from(health < hp);
                    peak = peak.max(position.y);
                    rows.push(
                    serde_json::json!({"fps":fps,"source":source_pose,"npcWithContact":npc,"tick":tick,
                    "attacker":h.position().to_array(),"attackerPose":h.pose_id(),
                    "attackerFrame":h.app.world().get::<PoseRuntime>(h.fighter).unwrap().frame,
                    "beforePose":before_pose.pose_id,"beforeFrame":before_pose.frame,
                    "pose":p.pose_id,"frame":p.frame,"position":position.to_array(),
                    "before":before.to_array(),"beforeVy":before_v,"vy":reaction.vertical_velocity,
                    "move":reaction.current.as_ref().map(|r|r.motion_scale),
                    "blendHeld":reaction.entry_blend_held,"retain":blend.retains_root_delta(),
                    "rootCache":blend.last_root_sample,"hit":health<hp}),
                );
                    hp = health;
                }
                eprintln!(
                    "dagger trajectory source={source_pose} fps={fps} npcWithContact={npc} hits={hits} targetPeak={peak}"
                );
            }
        }
    }
    std::fs::write(
        root.join("analysis/combat-motion-20261005/trajectories.json"),
        serde_json::to_vec_pretty(&rows).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "Original sword air-A down-readiness audit; writes isolated timing output"]
fn diagnose_sword_air_hit_down_readiness() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut rows = Vec::new();
    for pose_id in [97, 98] {
        for fps in [30, 60, 120, 165] {
            let mut h = reaction_timing_fixture(pose_id, fps, 0.0, false);
            let mut ready = None;
            for tick in 0..fps * 3 {
                h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / fps as u64));
                if h.app.world().get::<SmState>(h.fighter).unwrap().current == FighterState::Down {
                    ready = Some(tick as f64 / fps as f64);
                    break;
                }
            }
            eprintln!("sword targetPose={pose_id} fps={fps} downReady={ready:?}");
            assert!(
                ready.is_some(),
                "pose={:?} state={:?} air={:?}",
                h.app.world().get::<PoseRuntime>(h.fighter),
                h.app.world().get::<SmState>(h.fighter),
                h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .map(|air| air.air.airborne)
            );
            rows.push(serde_json::json!({"targetPose":pose_id,"fps":fps,"downReady":ready}));
        }
    }
    std::fs::write(
        root.join("analysis/combat-motion-20261005/sword-down-readiness.json"),
        serde_json::to_vec_pretty(&rows).unwrap(),
    )
    .unwrap();
}

fn reaction_timing_fixture(pose_id: u16, fps: u32, minimum: f32, npc: bool) -> GameplayHarness {
    let mut h = spawn_gameplay_harness_with_order(6, true).unwrap();
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation = Vec3::ZERO;
    let mut pose = PoseRuntime::idle();
    h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, pose_id, true);
    pose.link_past_end = true;
    let mut reaction = getup_reaction_for_pose(pose_id, pose.start, pose.end);
    reaction.kind = HitReactKind::BeHit;
    reaction.minimum_remaining = minimum;
    reaction.authored_minimum = minimum;
    reaction.pose_id = Some(pose_id);
    reaction.motion_scale = 6.0;
    let mut runtime = FighterHitReaction::default();
    runtime.begin(reaction, false);
    let to_frame = pose.start as f32;
    let first_tick_time = h
        .app
        .world()
        .resource::<Time<bevy::time::Real>>()
        .elapsed_secs_f64()
        + 1.0 / fps as f64;
    h.app.world_mut().entity_mut(h.fighter).insert((
        pose,
        EntryBlend {
            pose_id,
            duration: 0.12,
            started_at_seconds: Some(first_tick_time),
            to_frame,
            ..default()
        },
        runtime,
        crate::components::fighter::VisualRootPin::default(),
    ));
    h.app
        .world_mut()
        .get_mut::<SmState>(h.fighter)
        .unwrap()
        .current = FighterState::BeHit;
    h.app
        .world_mut()
        .get_mut::<CombatState>(h.fighter)
        .unwrap()
        .state = FighterState::BeHit;
    crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
    let schedule = super::super::fighter_frame::schedule(&h.app);
    h.app.add_systems(
        schedule,
        crate::plugins::character_plugin::drive_fighter_animation_for_test
            .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
    );
    h.app.add_systems(
        schedule,
        hit_reaction_root_motion
            .after(crate::plugins::movement_plugin::attack_root_motion)
            .in_set(crate::plugins::FighterFrameSet::RootMotion),
    );
    if npc {
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .remove::<LocalFighter>();
    }
    h
}

#[test]
fn owning_reaction_minimum_and_down_lifetime_match_original_controller() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/reaction-lifetime-oracle.json")).unwrap();
    for sample in oracle["samples"].as_array().unwrap() {
        for npc in [false, true] {
            let fps = sample["fps"].as_u64().unwrap() as u32;
            let pose_id = sample["entryPose"].as_u64().unwrap() as u16;
            let minimum = sample["minimum"].as_f64().unwrap() as f32;
            let mut h = reaction_timing_fixture(pose_id, fps, minimum, npc);
            let mut ready = None;
            for tick in 0..fps * 5 {
                h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / fps as u64));
                let runtime = h.app.world().get::<FighterHitReaction>(h.fighter).unwrap();
                if runtime.current.is_none()
                    || runtime
                        .current
                        .as_ref()
                        .is_some_and(|hit| hit.kind == HitReactKind::Down)
                {
                    ready = Some(tick as f64 / fps as f64);
                    break;
                }
            }
            let expected = sample["ready"].as_f64().unwrap();
            let actual =
                ready.expect("reaction must become ready within the oracle's five-second bound");
            assert!(
                (actual - expected).abs() <= 1.01 / fps as f64,
                "native controller lifetime: pose={pose_id} min={minimum} fps={fps} NPC={npc}, expected={expected}, actual={actual}"
            );
            if pose_id == 97 {
                assert_eq!(
                    h.pose_id(),
                    sample["finalPose"].as_u64().unwrap() as u16,
                    "down readiness must retain the original hold pose"
                );
            } else {
                assert_eq!(
                    h.app.world().get::<SmState>(h.fighter).unwrap().current,
                    FighterState::Name,
                    "a normal minimum expiry must release the owning fighter"
                );
            }
        }
    }
}
