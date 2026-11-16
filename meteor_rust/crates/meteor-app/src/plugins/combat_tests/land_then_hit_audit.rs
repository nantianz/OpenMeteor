#[test]
fn production_air_attack_down_recovery_restores_movement_without_jump() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut evidence = Vec::new();
    let mut failures = Vec::new();
    for fps in [30, 60, 120] {
        for recovery in ["guard", "hit", "roll"] {
            let second = (recovery == "hit").then_some(93);
            let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
            let p0 = crate::plugins::character_plugin::load_character_asset_for_test(
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
                .insert(crate::components::fighter::VisualRootPin::default());
            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                h.app.world_mut(),
                h.fighter,
            );
            crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
            h.app.add_systems(
                super::super::fighter_frame::FighterUpdate,
                hit_reaction_root_motion.in_set(crate::plugins::FighterFrameSet::RootMotion),
            );
            let dt = 1.0 / fps as f32;
            h.app.insert_resource(crate::plugins::FrameClock {
                sim_dt: dt,
                ..default()
            });
            let keymap = h.app.world().resource::<crate::app_state::Keymap>().clone();
            h.press_binding(keymap.jump);
            for _ in 0..fps / 5 {
                h.step_dt(dt);
            }
            h.release_binding(keymap.jump);
            h.step_dt(dt);
            assert!(h.airborne());
            h.press_binding(keymap.attack);
            h.step_dt(dt);
            h.release_binding(keymap.attack);
            h.step_dt(dt);
            let entry = serde_json::json!({"pose":h.pose_id(),"state":format!("{:?}",h.state()),
                "air":format!("{:?}",h.app.world().get::<FighterAir>(h.fighter).unwrap().air)});
            assert_eq!(
                h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .air
                    .motion_mode,
                meteor_physics::jump::AirMotionMode::Curve,
                "real air attack must enter Curve"
            );
            falling_hit_audit_inject(&mut h, 104);
            for _ in 0..fps * 5 {
                h.step_dt(dt);
                if h.state() == FighterState::Down {
                    break;
                }
            }
            assert_eq!(h.state(), FighterState::Down);
            assert!(!h.airborne());
            if let Some(second) = second {
                falling_hit_audit_inject(&mut h, second);
            }
            let mut got_up = false;
            if recovery == "roll" {
                h.press_binding(keymap.move_up);
                h.step_dt(dt);
                h.release_binding(keymap.move_up);
                got_up = true;
                assert_eq!(h.state(), FighterState::Dodge);
            }
            for _ in 0..fps * 5 {
                if h.state() == FighterState::Down && !got_up {
                    h.press_binding(keymap.guard);
                    got_up = true;
                } else if got_up {
                    h.release_binding(keymap.guard);
                }
                h.step_dt(dt);
            }
            let stuck = serde_json::json!({"pose":h.pose_id(),"state":format!("{:?}",h.state()),"grounded":!h.airborne(),
                "air":format!("{:?}",h.app.world().get::<FighterAir>(h.fighter).unwrap().air),
                "reaction":format!("{:?}",h.app.world().get::<FighterHitReaction>(h.fighter).unwrap())});
            assert!(!h.airborne() && h.state() == FighterState::Name);
            assert!(!matches!(h.pose_id(), 152 | 180), "completed grounded recovery must not select a fall pose: fps={fps} second={second:?}");
            let origin = h.position();
            h.press_binding(keymap.move_up);
            for _ in 0..fps {
                h.step_dt(dt);
            }
            h.release_binding(keymap.move_up);
            let displacement = (h.position() - origin).length();
            h.press_binding(keymap.jump);
            for _ in 0..fps / 5 {
                h.step_dt(dt);
            }
            h.release_binding(keymap.jump);
            for _ in 0..fps * 3 {
                h.step_dt(dt);
            }
            let after_jump = h.position();
            h.press_binding(keymap.move_up);
            for _ in 0..fps / 2 {
                h.step_dt(dt);
            }
            h.release_binding(keymap.move_up);
            let rescued_displacement = (h.position() - after_jump).length();
            evidence.push(
                serde_json::json!({"fps":fps,"recovery":recovery,"second":second,"entry":entry,"stuck":stuck,
                "displacement":displacement,"afterJumpDisplacement":rescued_displacement}),
            );
            if displacement < 0.05 {
                failures.push(format!("fps={fps} second={second:?}: movement={displacement}, after jump={rescued_displacement}"));
            }
        }
    }
    if let Some(output) = std::env::var_os("METEOR_HIT_AUDIT_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            output.join("natural-air-attack-down-recovery.json"),
            serde_json::to_string_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
    assert!(
        failures.is_empty(),
        "natural air attack re-hit recovery failures: {failures:#?}"
    );
}

#[test]
#[ignore = "COMBAT-075 Down re-hit, getup and directional-input investigation"]
fn production_down_rehit_getup_movement_audit() {
    use crate::components::fighter::VisualRootPin;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut evidence = Vec::new();
    let mut failures = Vec::new();
    for fps in [30, 60, 120] {
        for scene in [false, true] {
            for curve_entry in [false, true] {
                for first in [104, 105] {
                    for second in [
                        None,
                        Some(90),
                        Some(93),
                        Some(100),
                        Some(104),
                        Some(106),
                        Some(109),
                    ] {
                        let mut h = spawn_gameplay_harness_with_order(5, true).unwrap();
                        let p0 = crate::plugins::character_plugin::load_character_asset_for_test(
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
                        if scene {
                            let stage = "sn06";
                            let assets = root.join("origin_res");
                            let gmdl = meteor_asset::gmdl::decode_gmdl(
                                &std::fs::read(assets.join(format!("{stage}/{stage}.gmb")))
                                    .unwrap(),
                            )
                            .unwrap();
                            let desc = meteor_asset::des::parse_stage_description(
                                &std::fs::read_to_string(
                                    assets.join(format!("{stage}/{stage}.des")),
                                )
                                .unwrap(),
                            );
                            let cob = meteor_asset::cob::parse_cob(
                                &std::fs::read(assets.join(format!("{stage}/{stage}.cob")))
                                    .unwrap(),
                            )
                            .unwrap();
                            let wp =
                                std::fs::read_to_string(assets.join(format!("Level/{stage}.wp")))
                                    .unwrap();
                            let center =
                                meteor_scene::stage_loader::get_stage_center(stage, Some(&wp));
                            h.replace_stage_mesh(StageMeshCollider::from_gmdl(
                                &gmdl,
                                &desc,
                                Some(&cob.face_ids),
                                center,
                            ));
                            h.app
                                .world_mut()
                                .get_mut::<Transform>(h.fighter)
                                .unwrap()
                                .translation = Vec3::new(24.0, -2.9, -5.1);
                        }
                        for _ in 0..fps {
                            h.step_dt(dt);
                        }
                        if curve_entry {
                            h.app
                                .world_mut()
                                .get_mut::<Transform>(h.fighter)
                                .unwrap()
                                .translation
                                .y += 1.0;
                            let mut air =
                                h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                            air.air.start_fall(0.0, 0.0);
                            air.air.motion_mode = meteor_physics::jump::AirMotionMode::Curve;
                            air.air.vertical_speed = -5.0;
                            air.ground_contact = false;
                            drop(air);
                            h.app
                                .world_mut()
                                .get_mut::<MotionState>(h.fighter)
                                .unwrap()
                                .velocity
                                .y = -5.0;
                        }
                        falling_hit_audit_inject(&mut h, first);
                        for _ in 0..fps * 5 {
                            h.step_dt(dt);
                            if h.state() == FighterState::Down {
                                break;
                            }
                        }
                        assert_eq!(
                            h.state(),
                            FighterState::Down,
                            "first hit must naturally reach Down: {fps}/{scene}/{first}"
                        );
                        assert!(!h.airborne());
                        if let Some(second) = second {
                            falling_hit_audit_inject(&mut h, second);
                        }
                        let mut history = Vec::new();
                        let mut previous = None;
                        let mut requested_getup = false;
                        let keymap = h.app.world().resource::<crate::app_state::Keymap>().clone();
                        for tick in 0..fps * 7 {
                            if h.state() == FighterState::Down && !requested_getup {
                                h.press_binding(keymap.guard);
                                requested_getup = true;
                            } else if requested_getup {
                                h.release_binding(keymap.guard);
                            }
                            h.step_dt(dt);
                            let key = (h.state(), h.pose_id(), h.airborne());
                            if previous != Some(key) {
                                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                                let runtime =
                                    h.app.world().get::<FighterHitReaction>(h.fighter).unwrap();
                                history.push(serde_json::json!({"tick":tick,"state":format!("{:?}",h.state()),
                                "pose":h.pose_id(),"air":format!("{:?}",air.air),"position":h.position().to_array(),
                                "reaction":format!("{:?}",runtime),"groundMode":air.recovery_ground_mode}));
                                previous = Some(key);
                            }
                        }
                        let before_move = h.position();
                        let mut max_displacement = 0.0_f32;
                        for binding in [
                            keymap.move_up,
                            keymap.move_right,
                            keymap.move_down,
                            keymap.move_left,
                        ] {
                            h.press_binding(binding);
                            for _ in 0..fps / 2 {
                                h.step_dt(dt);
                                max_displacement =
                                    max_displacement.max((h.position() - before_move).length());
                            }
                            h.release_binding(binding);
                            h.step_dt(dt);
                        }
                        let settled = !h.airborne() && h.state() == FighterState::Name;
                        let before_jump = serde_json::json!({"state":format!("{:?}",h.state()),"pose":h.pose_id(),
                        "position":h.position().to_array(),"air":format!("{:?}",h.app.world().get::<FighterAir>(h.fighter).unwrap().air)});
                        let mut rescue_displacement = None;
                        if !settled || max_displacement < 0.05 {
                            failures.push(format!("fps={fps} scene={scene} curve={curve_entry} first={first} second={second:?} state={:?} pose={} air={} displacement={max_displacement}",h.state(),h.pose_id(),h.airborne()));
                            h.press_binding(keymap.jump);
                            for _ in 0..fps / 5 {
                                h.step_dt(dt);
                            }
                            h.release_binding(keymap.jump);
                            for _ in 0..fps * 3 {
                                h.step_dt(dt);
                            }
                            let rescued_start = h.position();
                            h.press_binding(keymap.move_up);
                            for _ in 0..fps / 2 {
                                h.step_dt(dt);
                            }
                            h.release_binding(keymap.move_up);
                            rescue_displacement = Some((h.position() - rescued_start).length());
                        }
                        evidence.push(serde_json::json!({"fps":fps,"scene":scene,"curveEntry":curve_entry,"first":first,"second":second,
                        "getupRequested":requested_getup,"history":history,"settled":settled,"maxDisplacement":max_displacement,
                        "beforeJump":before_jump,"jumpRescueDisplacement":rescue_displacement}));
                    }
                }
            }
        }
    }
    std::fs::write(
        root.join("analysis/hit-reaction-20261005-fix/down-rehit-sn06-getup-movement.json"),
        serde_json::to_string_pretty(&evidence).unwrap(),
    )
    .unwrap();
    assert!(
        failures.is_empty(),
        "Down re-hit movement failures: {failures:#?}"
    );
    eprintln!(
        "Down re-hit, getup and movement: {} cases passed",
        evidence.len()
    );
}

#[test]
#[ignore = "COMBAT-075 completed landing followed by a new hit; writes diagnostics"]
fn production_land_then_hit_state_and_bones_audit() {
    use crate::components::fighter::VisualRootPin;
    use crate::plugins::character_plugin::{FighterBone, FighterModel};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mesh = sn13_test_mesh();
    let mut cases = Vec::new();
    for fps in [30, 60, 120] {
        for scene in [false, true] {
            for first_hit in [None, Some(93), Some(100)] {
                for second_hit in [93, 94, 100, 106, 109] {
                    let mut h = spawn_gameplay_harness_with_order(6, true).unwrap();
                    let p0 = crate::plugins::character_plugin::load_character_asset_for_test(
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
                    crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
                    h.app.add_systems(
                        super::super::fighter_frame::FighterUpdate,
                        hit_reaction_root_motion
                            .in_set(crate::plugins::FighterFrameSet::RootMotion),
                    );
                    if scene {
                        h.replace_stage_mesh(mesh.clone());
                    }
                    let dt = 1.0 / fps as f32;
                    h.app.insert_resource(crate::plugins::FrameClock {
                        sim_dt: dt,
                        ..default()
                    });
                    let mut pose = PoseRuntime::idle();
                    h.app.world().resource::<CharacterAssetsSet>().0["p0"]
                        .apply_pose(&mut pose, 0, true);
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .insert((pose, EntryBlend::default()));
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(h.fighter)
                        .unwrap()
                        .translation = if scene {
                        Vec3::new(-104.11785, 0.05, 76.775986)
                    } else {
                        Vec3::ZERO
                    };
                    for _ in 0..fps {
                        h.step_dt(dt);
                    }
                    assert!(
                        !h.airborne(),
                        "natural jump requires initial ground support"
                    );
                    let jump = crate::app_state::Keymap::default().jump;
                    h.press_binding(jump);
                    for _ in 0..fps / 5 {
                        h.step_dt(dt);
                    }
                    h.release_binding(jump);
                    h.step_dt(dt);
                    let mut descending = false;
                    for _ in 0..fps * 2 {
                        h.step_dt(dt);
                        let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                        if h.airborne() && air.air.vertical_speed < -5.0 {
                            descending = true;
                            break;
                        }
                    }
                    assert!(
                        descending,
                        "real charged jump must reach descent: fps={fps} scene={scene}"
                    );
                    if let Some(hit) = first_hit {
                        falling_hit_audit_inject(&mut h, hit);
                    }
                    let mut stable = 0;
                    for _ in 0..fps * 8 {
                        h.step_dt(dt);
                        if !h.airborne() && h.state() == FighterState::Name && h.pose_id() == 0 {
                            stable += 1;
                            if stable >= 3 {
                                break;
                            }
                        } else {
                            stable = 0;
                        }
                    }
                    assert!(stable>=3,"initial landing failed: fps={fps} scene={scene} first={first_hit:?} state={:?} pose={} pos={:?}",h.state(),h.pose_id(),h.position());
                    let landed = {
                        let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                        serde_json::json!({"position":h.position().to_array(),"air":format!("{:?}",air.air),
                            "groundContact":air.ground_contact,"groundMode":air.recovery_ground_mode,
                            "ledgeFall":air.ledge_fall,"pendingLedgeFall":air.pending_ledge_fall,
                            "rootPin":air.motion_root_pin,"rootPinPose":air.motion_root_pin_pose_id,
                            "lateSnap":air.late_visual_pose_snap})
                    };
                    falling_hit_audit_inject(&mut h, second_hit);
                    assert_eq!(h.pose_id(), second_hit);
                    let mut history = Vec::new();
                    let mut old = None;
                    for tick in 0..fps * 5 {
                        h.step_dt(dt);
                        let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap().clone();
                        let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
                        let reacting = h
                            .app
                            .world()
                            .get::<FighterHitReaction>(h.fighter)
                            .unwrap()
                            .current
                            .is_some();
                        assert!(!reacting || !matches!(pose.pose_id,152|154|156|158|180),
                            "landed victim uses fall while hit-owned: fps={fps} scene={scene} first={first_hit:?} second={second_hit} tick={tick}");
                        if reacting
                            && !blend.is_active_for(pose.pose_id)
                            && !blend.preserve_sample_once
                        {
                            let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p0"];
                            let common = &h.app.world().resource::<CommonAmb>().0;
                            let sampled = assets
                                .amb_for_pose(common, pose.pose_id)
                                .sample_at(pose.frame);
                            let mut query = h.app.world_mut().query::<(&FighterBone, &Transform)>();
                            for (bone, transform) in query.iter(h.app.world()) {
                                if h.app
                                    .world()
                                    .get::<FighterModel>(bone.model)
                                    .unwrap()
                                    .fighter
                                    != h.fighter
                                {
                                    continue;
                                }
                                if let Some(q) = sampled.bone_quats.get(bone.skin_index) {
                                    assert!(Quat::from_array(*q).dot(transform.rotation).abs()>0.9999,
                                        "landed-hit stale visual bone: fps={fps} scene={scene} first={first_hit:?} second={second_hit} tick={tick} bone={}",bone.skin_index);
                                }
                            }
                        }
                        let key = (pose.pose_id, h.state(), h.airborne());
                        if old != Some(key) {
                            history.push(serde_json::json!({"tick":tick,"pose":pose.pose_id,
                                "state":format!("{:?}",h.state()),"air":h.airborne(),"position":h.position().to_array()}));
                            old = Some(key);
                        }
                    }
                    assert!(!h.airborne() && matches!(h.state(),FighterState::Name|FighterState::Down),
                        "post-landing new hit fails to settle: fps={fps} scene={scene} first={first_hit:?} second={second_hit} state={:?} pose={} pos={:?}",h.state(),h.pose_id(),h.position());
                    cases.push(
                        serde_json::json!({"fps":fps,"scene":scene,"first":first_hit,
                        "second":second_hit,"landed":landed,"history":history}),
                    );
                }
            }
        }
    }
    let output = falling_hit_audit_output(&root, "land-then-hit-matrix.json");
    std::fs::write(output, serde_json::to_string_pretty(&cases).unwrap()).unwrap();
    eprintln!(
        "completed landing then new hit audit: {} cases passed",
        cases.len()
    );
}

#[test]
fn production_hit_finish_on_contact_must_not_select_fall() {
    let mut evidence = Vec::new();
    for fps in [30, 60, 120] {
        let mut h = spawn_gameplay_harness(6).unwrap();
        h.app.add_systems(
            Update,
            hit_reaction_root_motion.in_set(crate::plugins::FighterFrameSet::RootMotion),
        );
        let dt = 1.0 / fps as f32;
        h.app.insert_resource(crate::plugins::FrameClock {
            sim_dt: dt,
            ..default()
        });
        let mut pose = PoseRuntime::idle();
        h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 93, true);
        pose.loops = false;
        pose.frame = pose.end as f32;
        let mut hit = getup_reaction_for_pose(93, pose.start, pose.end);
        hit.kind = HitReactKind::BeHit;
        hit.minimum_remaining = 0.0;
        hit.authored_minimum = 0.0;
        hit.play_to_end_pose_id = Some(93);
        let mut runtime = FighterHitReaction::default();
        runtime.begin(hit, true);
        runtime.bank_be_hit_carry(Vec3::new(0.0, -3.0, 0.0), true);
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert((pose, runtime));
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation
            .y = 0.01;
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
        {
            let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
            air.air.start_fall(0.0, 0.0);
            air.air.vertical_speed = -3.0;
            air.ground_contact = false;
        }
        {
            let mut motion = h.app.world_mut().get_mut::<MotionState>(h.fighter).unwrap();
            motion.velocity = Vec3::new(0.0, -3.0, 0.0);
            motion.movement_mode = meteor_core::MovementMode::Airborne;
        }
        h.step_dt(dt);
        let contact_pose = h.pose_id();
        let contact_state = h.state();
        let grounded = !h.airborne();
        let movement = h
            .app
            .world()
            .get::<MotionState>(h.fighter)
            .unwrap()
            .movement_mode;
        let position = h.position();
        h.step_dt(dt);
        evidence.push(serde_json::json!({"fps":fps,"contactPose":contact_pose,
            "contactState":format!("{contact_state:?}"),"grounded":grounded,
            "movement":format!("{movement:?}"),"position":position.to_array(),
            "nextPose":h.pose_id(),"nextState":format!("{:?}",h.state()),"nextAir":h.airborne()}));
    }
    if let Some(output) = std::env::var_os("METEOR_HIT_AUDIT_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            output.join("hit-finish-on-contact.json"),
            serde_json::to_string_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
    eprintln!("contact boundary evidence: {evidence:?}");
    assert!(
        evidence
            .iter()
            .all(|case| case["grounded"] == true && case["contactPose"] != 180),
        "physical contact must not install ordinary fall"
    );
}

#[test]
#[ignore = "COMBAT-075 inspect mode1 steep contact before a later hit"]
fn production_contact_mode_then_hit_audit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut cases = Vec::new();
    for fps in [30, 60, 120] {
        let mut h = spawn_gameplay_harness(6).unwrap();

        let mut mesh = StageMeshCollider::default();
        mesh.append_world_triangles(
            &[
                [-20.0, -60.0, -20.0],
                [20.0, 60.0, -20.0],
                [20.0, 60.0, 20.0],
                [-20.0, -60.0, 20.0],
            ],
            &[0, 2, 1, 0, 3, 2],
        );
        h.replace_stage_mesh(mesh);
        h.app.add_systems(
            Update,
            hit_reaction_root_motion.in_set(crate::plugins::FighterFrameSet::RootMotion),
        );
        let dt = 1.0 / fps as f32;
        h.app.insert_resource(crate::plugins::FrameClock {
            sim_dt: dt,
            ..default()
        });
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = Vec3::new(0.0, 8.0, 0.0);
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
            .vertical_speed = -10.0;
        let mut entry = None;
        for tick in 0..fps * 3 {
            h.step_dt(dt);
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            if air.recovery_ground_mode == Some(true)
                && h.state() == FighterState::Name
                && h.airborne()
            {
                entry = Some(
                    serde_json::json!({"tick":tick,"pose":h.pose_id(),"position":h.position().to_array(),
                    "groundMode":air.recovery_ground_mode,"groundContact":air.ground_contact,"air":format!("{:?}",air.air)}),
                );
                break;
            }
        }
        if entry.is_none() {
            cases.push(serde_json::json!({"fps":fps,"entry":null}));
            continue;
        }

        let p0 = crate::plugins::character_plugin::load_character_asset_for_test(
            &[root.join("origin_res")],
            "p0",
        )
        .unwrap();
        h.app
            .world_mut()
            .resource_mut::<CharacterAssetsSet>()
            .0
            .insert("p0".into(), p0);
        falling_hit_audit_inject(&mut h, 93);
        let mut history = Vec::new();
        let mut old = None;
        for tick in 0..fps * 2 {
            h.step_dt(dt);
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            let key = (
                h.pose_id(),
                h.state(),
                h.airborne(),
                air.recovery_ground_mode,
            );
            if old != Some(key) {
                history.push(serde_json::json!({"tick":tick,"pose":h.pose_id(),"state":format!("{:?}",h.state()),
                    "air":h.airborne(),"mode":air.recovery_ground_mode,"position":h.position().to_array()}));
                old = Some(key);
            }
        }
        cases.push(serde_json::json!({"fps":fps,"entry":entry,"history":history}));
    }
    let output = root.join("analysis/hit-reaction-20261004/contact-mode-then-hit.json");
    std::fs::write(output, serde_json::to_string_pretty(&cases).unwrap()).unwrap();
    eprintln!(
        "steep contact followed by hit diagnostic: {} rates",
        cases.len()
    );
}
