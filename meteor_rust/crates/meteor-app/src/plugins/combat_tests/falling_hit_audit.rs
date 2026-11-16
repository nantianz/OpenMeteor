fn falling_hit_audit_output(root: &std::path::Path, file: &str) -> std::path::PathBuf {
    let output = std::env::var_os("METEOR_HIT_AUDIT_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("analysis/hit-reaction-20261004"));
    std::fs::create_dir_all(&output).unwrap();
    output.join(file)
}

#[test]
#[ignore = "original-resource falling-hit audit; writes analysis results"]
fn production_falling_hit_state_and_visible_pose_audit() {
    use crate::components::fighter::VisualRootPin;
    use crate::plugins::character_plugin::{FighterBone, FighterModel};
    use meteor_physics::jump::AirMotionMode;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let seed = crate::plugins::character_plugin::load_character_asset_for_test(
        &[root.join("origin_res")],
        "p0",
    )
    .unwrap();
    let targets = seed
        .poses
        .values()
        .flat_map(|p| &p.attacks)
        .flat_map(|a| {
            [
                a.target_pose,
                a.target_pose_front,
                a.target_pose_back,
                a.target_pose_left,
                a.target_pose_right,
            ]
        })
        .flatten()
        .filter(|id| seed.poses.contains_key(id))
        .collect::<std::collections::BTreeSet<_>>();
    eprintln!("authored reaction targets: {targets:?}");
    let mut cases = Vec::new();
    let mut failures = Vec::new();
    for fps in [30, 60, 120] {
        for source in [152, 180] {
            for target in targets.iter().copied() {
                for height in [0.05, 100.0] {
                    for curve in [false, true] {
                        for repeat in [false, true] {
                            let mut h = spawn_gameplay_harness_with_order(6, true).unwrap();
                            let p0 =
                                crate::plugins::character_plugin::load_character_asset_for_test(
                                    &[root.join("origin_res")],
                                    "p0",
                                )
                                .unwrap();
                            h.app
                                .world_mut()
                                .resource_mut::<CharacterAssetsSet>()
                                .0
                                .insert("p0".into(), p0);
                            h.app
                                .world_mut()
                                .get_mut::<Fighter>(h.fighter)
                                .unwrap()
                                .character_id = 0;
                            h.app
                                .world_mut()
                                .entity_mut(h.fighter)
                                .insert(VisualRootPin::default());
                            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                                h.app.world_mut(),
                                h.fighter,
                            );
                            crate::plugins::character_plugin::install_crowd_animation_for_test(
                                &mut h.app,
                            );
                            h.app.add_systems(
                                super::super::fighter_frame::FighterUpdate,
                                hit_reaction_root_motion
                                    .in_set(crate::plugins::FighterFrameSet::RootMotion),
                            );
                            let dt = 1.0 / fps as f32;
                            h.app.insert_resource(crate::plugins::FrameClock {
                                sim_dt: dt,
                                ..default()
                            });
                            let mut source_pose = PoseRuntime::idle();
                            h.app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(
                                &mut source_pose,
                                source,
                                true,
                            );
                            h.app.world_mut().entity_mut(h.fighter).insert(source_pose);
                            h.app
                                .world_mut()
                                .get_mut::<Transform>(h.fighter)
                                .unwrap()
                                .translation
                                .y = height;
                            h.app
                                .world_mut()
                                .get_mut::<SmState>(h.fighter)
                                .unwrap()
                                .current = FighterState::Jump;
                            h.app
                                .world_mut()
                                .get_mut::<CombatState>(h.fighter)
                                .unwrap()
                                .state = FighterState::Jump;
                            {
                                let mut air =
                                    h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                                air.air.start_fall(0.0, 0.0);
                                air.air.vertical_speed = -20.0;
                                air.air.motion_mode = if curve {
                                    AirMotionMode::CurveExit
                                } else {
                                    AirMotionMode::Ballistic
                                };
                                air.air.prev_root_z = Some(0.0);
                                air.air.vertical_pose_id = 153;
                                air.ground_contact = false;
                            }
                            {
                                let mut motion =
                                    h.app.world_mut().get_mut::<MotionState>(h.fighter).unwrap();
                                motion.movement_mode = meteor_core::MovementMode::Airborne;
                                motion.velocity = Vec3::new(0.0, -20.0, 0.0);
                            }
                            crate::plugins::character_plugin::drive_fighter_animation_for_test(
                                h.app.world_mut(),
                            );
                            falling_hit_audit_inject(&mut h, target);
                            let mut history = Vec::new();
                            let mut entry_samples = Vec::new();
                            let mut previous = None;
                            let mut max_bone_error = 0.0_f32;
                            for tick in 0..fps * 4 {
                                if repeat && matches!(tick, 2 | 4) {
                                    falling_hit_audit_inject(
                                        &mut h,
                                        if tick == 2 { 93 } else { target },
                                    );
                                }
                                h.step_dt(dt);
                                let pose =
                                    h.app.world().get::<PoseRuntime>(h.fighter).unwrap().clone();
                                let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
                                let reacting = h
                                    .app
                                    .world()
                                    .get::<FighterHitReaction>(h.fighter)
                                    .unwrap()
                                    .current
                                    .is_some();
                                if tick < fps / 2 {
                                    entry_samples.push(serde_json::json!({"tick":tick,"pose":pose.pose_id,"frame":pose.frame,
                                    "duration":blend.duration,"elapsed":blend.elapsed,"source":blend.motion_source,
                                    "air":h.airborne(),"reaction":reacting}));
                                }
                                let key = (pose.pose_id, h.state(), h.airborne(), reacting);
                                if previous != Some(key) {
                                    history.push(serde_json::json!({"tick":tick,"pose":pose.pose_id,"frame":pose.frame,
                                    "state":format!("{:?}",h.state()),"air":h.airborne(),"reaction":reacting,
                                    "position":h.position().to_array()}));
                                    previous = Some(key);
                                }
                                if reacting && matches!(pose.pose_id, 152 | 154 | 156 | 158 | 180) {
                                    failures.push(format!("fall overwrote reaction: fps={fps} source={source} target={target} height={height} curve={curve} repeat={repeat} tick={tick}"));
                                    break;
                                }

                                if reacting
                                    && !blend.is_active_for(pose.pose_id)
                                    && !blend.preserve_sample_once
                                {
                                    let assets =
                                        &h.app.world().resource::<CharacterAssetsSet>().0["p0"];
                                    let common = &h.app.world().resource::<CommonAmb>().0;
                                    let sampled = assets
                                        .amb_for_pose(common, pose.pose_id)
                                        .sample_at(pose.frame);
                                    let mut q =
                                        h.app.world_mut().query::<(&FighterBone, &Transform)>();
                                    for (bone, transform) in q.iter(h.app.world()) {
                                        let model =
                                            h.app.world().get::<FighterModel>(bone.model).unwrap();
                                        if model.fighter != h.fighter
                                            || bone.skin_index >= sampled.bone_quats.len()
                                        {
                                            continue;
                                        }
                                        let q = sampled.bone_quats[bone.skin_index];
                                        let expected = Quat::from_array(q);
                                        let error = 1.0 - expected.dot(transform.rotation).abs();
                                        max_bone_error = max_bone_error.max(error);
                                        if error > 0.0001 {
                                            failures.push(format!("stale visible bone: fps={fps} source={source} target={target} height={height} curve={curve} repeat={repeat} tick={tick} pose={} bone={} error={error}", pose.pose_id, bone.skin_index));
                                            break;
                                        }
                                    }
                                }
                            }
                            if h.airborne()
                                || matches!(h.state(), FighterState::BeHit | FighterState::Jump)
                            {
                                failures.push(format!("not settled: fps={fps} source={source} target={target} height={height} curve={curve} repeat={repeat} state={:?} pose={} position={:?}",h.state(),h.pose_id(),h.position()));
                            }
                            cases.push(serde_json::json!({"fps":fps,"source":source,"target":target,"height":height,
                            "curve":curve,"repeat":repeat,"maxBoneError":max_bone_error,"history":history,"entrySamples":entry_samples}));
                        }
                    }
                }
            }
        }
    }
    let output = falling_hit_audit_output(&root, "production-matrix.json");
    std::fs::write(
        &output,
        serde_json::to_string_pretty(&serde_json::json!({"cases":cases,"failures":failures}))
            .unwrap(),
    )
    .unwrap();
    assert!(
        failures.is_empty(),
        "falling-hit audit failed: {failures:?}; output {}",
        output.display()
    );
    eprintln!(
        "falling-hit audit: {} cases passed; {}",
        cases.len(),
        output.display()
    );
}

fn falling_hit_audit_inject(h: &mut GameplayHarness, target: u16) {
    use meteor_combat::hit_reaction::FacingBucket;
    let entity = h.fighter;
    h.app
        .world_mut()
        .resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
            let character = &assets.0["p0"];

            let mut ids = character.poses.keys().copied().collect::<Vec<_>>();
            ids.sort_unstable();
            let (attack, bucket) = ids
                .into_iter()
                .flat_map(|id| &character.poses[&id].attacks)
                .find_map(|a| {
                    [
                        (a.target_pose, FacingBucket::Default),
                        (a.target_pose_front, FacingBucket::Front),
                        (a.target_pose_back, FacingBucket::Back),
                        (a.target_pose_left, FacingBucket::Left),
                        (a.target_pose_right, FacingBucket::Right),
                    ]
                    .into_iter()
                    .find(|(pose, _)| *pose == Some(target))
                    .map(|(_, bucket)| (a.clone(), bucket))
                })
                .unwrap();
            let available = character.poses.keys().copied().collect::<Vec<_>>();
            let mut query = world.query::<(
                &mut PoseRuntime,
                &mut EntryBlend,
                &mut CombatState,
                &mut Health,
                &mut Qi,
                &FighterAir,
                &mut FighterHitReaction,
                &MotionState,
            )>();
            let (mut pose, mut blend, mut combat, mut health, mut qi, air, mut runtime, motion) =
                query.get_mut(world, entity).unwrap();
            let mut spend = None;
            let mut view = CombatView {
                pose: &mut pose,
                blend: &mut blend,
                combat: &mut combat,
                health: &mut health,
                qi: &mut qi,
                air,
                assets: character,
                weapon: 6,
                guard_held_input: false,
                in_guard_hit_reaction: false,
                attack_directions: Vec::new(),
                weapon_form: 0,
                anger_spend: &mut spend,
                strategy_id: weapon_to_strategy(6),
                weapon_action_vetoed: false,
            };
            let hit = begin_be_hit(&mut view, Some(&attack), bucket, &available).unwrap();
            assert_eq!(hit.pose_id, Some(target));
            drop(view);
            runtime.begin(hit, air.air.airborne);
            runtime.bank_be_hit_carry(motion.velocity, true);
        });
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
}
