#[cfg(not(target_arch = "wasm32"))]
#[test]
fn production_held_forward_fake_roll_passes_opponent_at_60_and_120_fps() {
    let mut failed = Vec::new();

    let rates = if std::env::var_os("METEOR_AUDIT_ROLL_30FPS").is_some() {
        vec![30]
    } else {
        vec![60, 120]
    };
    for fps in rates {
        for (locked, remote) in [false, true]
            .into_iter()
            .flat_map(|l| [(l, false), (l, true)])
        {
            let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(6, true);
            h.app.insert_resource(State::new(AppState::Playing));
            h.app.add_plugins(crate::plugins::CharacterContactPlugin);
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert(HeadTrackState::default());
            configure_lock_updates(&mut h.app);
            install_guard_test_animation(&mut h);
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(0.0, 0.0, 2.2);
            if remote {
                h.app.world_mut().entity_mut(target).insert(RemoteFighter {
                    player_id: "stock-2".into(),
                });
            }
            if locked {
                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                lock.lock_entity = Some(target);
                lock.system.lock_target = Some(target.to_bits());
            }
            h.press_key(KeyCode::KeyW);
            for _ in 0..fps / 2 {
                h.step_dt(1.0 / fps as f32);
            }
            let start = h.position();
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::character_contact_plugin::CharacterContactFrame>()
                .audit_record_moves(h.fighter);
            let keymap = crate::app_state::Keymap::default();
            h.press_binding(keymap.fake);
            let mut trace = Vec::new();
            let mut rolled = false;
            let mut crossed = false;
            for tick in 0..fps * 2 {
                h.step_dt(1.0 / fps as f32);
                if tick == 0 {
                    h.release_binding(keymap.fake);
                }
                let w = h.app.world();
                let p = w.get::<PoseRuntime>(h.fighter).unwrap();
                let b = w.get::<EntryBlend>(h.fighter).unwrap();
                let m = w.get::<MotionState>(h.fighter).unwrap();
                let target_pos = w.get::<Transform>(target).unwrap().translation;
                rolled |= p.pose_id == 176;
                crossed |= h.position().z > target_pos.z + 0.5;
                trace.push(serde_json::json!({"tick":tick,"pose":p.pose_id,"frame":p.frame,
                    "position":h.position().to_array(),"target":target_pos.to_array(),"facing":m.facing,
                    "blend":b.duration,"state":format!("{:?}",h.state()),
                    "carry":[w.get::<FighterDodge>(h.fighter).unwrap().state.residual_x,w.get::<FighterDodge>(h.fighter).unwrap().state.residual_z],
                    "movement":w.resource::<crate::plugins::character_contact_plugin::CharacterContactFrame>().audit_moves(h.fighter)}));
                if rolled && !w.get::<FighterDodge>(h.fighter).unwrap().state.active {
                    break;
                }
            }
            let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../analysis/fake-forward-roll-20260928");
            std::fs::create_dir_all(&out).unwrap();
            std::fs::write(
                out.join(format!("production-{fps}-{locked}-{remote}.json")),
                serde_json::to_vec_pretty(&trace).unwrap(),
            )
            .unwrap();
            eprintln!("{fps} FPS locked={locked}, remote={remote}, start={start:?}, end={:?}, rolled={rolled}, crossed={crossed}", h.position());
            if !rolled || !crossed {
                failed.push((fps, locked, remote));
            }
        }
    }
    assert!(failed.is_empty(), "held W then Fake failed: {failed:?}");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "RUST-BEVY-115: user reports original locked dodge crosses; Rust repulsion holds this fixture in front; original candidate gate needs a live sample"]
fn diagnose_locked_forward_dodge_contact() {
    let mut failures = Vec::new();
    for fps in [60, 120] {
        for locked in [false, true] {
            let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(DAGGER, true);
            h.app.insert_resource(State::new(AppState::Playing));
            h.app.add_plugins(crate::plugins::CharacterContactPlugin);
            configure_lock_updates(&mut h.app);
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert(HeadTrackState::default());
            install_guard_test_animation(&mut h);
            h.app.world_mut().entity_mut(target).insert(RemoteFighter {
                player_id: "stock-2".into(),
            });
            let target_pos = Vec3::new(0.6, 0.0, 2.2);
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = target_pos;
            if locked {
                let mut lock = h.app.world_mut().resource_mut::<LockOnController>();
                lock.lock_entity = Some(target);
                lock.system.lock_target = Some(target.to_bits());
            }
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::character_contact_plugin::CharacterContactFrame>()
                .audit_record_moves(h.fighter);
            h.press_key(KeyCode::KeyW);
            h.step_dt(1.0 / fps as f32);
            h.release_key(KeyCode::KeyW);
            h.step_dt(1.0 / fps as f32);
            if locked {
                assert_eq!(
                    h.app.world().resource::<LockOnController>().lock_entity,
                    Some(target)
                );
            }
            h.press_key(KeyCode::KeyW);
            let mut dodged = false;
            let mut farthest = h.position().z;
            let mut repulsion_z = 0.0;
            for tick in 0..fps {
                h.step_dt(1.0 / fps as f32);
                if tick == 0 {
                    h.release_key(KeyCode::KeyW);
                }
                dodged |= h.pose_id() == 166;
                farthest = farthest.max(h.position().z);
                for movement in h
                    .app
                    .world()
                    .resource::<crate::plugins::character_contact_plugin::CharacterContactFrame>()
                    .audit_moves(h.fighter)
                {
                    if movement["kind"] == "repulsion" {
                        repulsion_z += movement["accepted"][2].as_f64().unwrap() as f32;
                    }
                }
            }
            eprintln!(
                "{fps}FPS locked={locked}: end={:?} farthest={farthest} repulsion_z={repulsion_z}",
                h.position()
            );
            assert!(dodged, "double tap must enter Pose166");
            if farthest <= target_pos.z + 0.5 {
                failures.push((fps, locked, farthest, repulsion_z));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "Pose166 did not pass the nearby fighter: {failures:?}"
    );
}

#[test]
fn production_roll_root_matches_original_integer_frame_xy() {
    use bevy::ecs::system::RunSystemOnce;
    let oracle: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../analysis/fake-forward-roll-20260928/probe-roll-root.json"
    )))
    .unwrap();
    let mut h = spawn_gameplay_harness_with_order(6, true).expect("original resources");
    for row in oracle["rows"].as_array().unwrap() {
        let pose_id = row["pose"].as_u64().unwrap() as u16;
        let frame = row["frame"].as_u64().unwrap() as f32;
        let fps = row["fps"].as_u64().unwrap() as f32;
        let dt = 1.0 / fps;
        let mut pose = PoseRuntime::idle();
        h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, pose_id, true);
        pose.frame = frame + 0.125;
        h.app
            .world_mut()
            .get_mut::<SmState>(h.fighter)
            .unwrap()
            .current = FighterState::Dodge;
        h.app.world_mut().entity_mut(h.fighter).insert((
            pose,
            EntryBlend::default(),
            FighterAir::default(),
            Transform::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Dodge,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
        ));
        h.app
            .world_mut()
            .get_mut::<FighterDodge>(h.fighter)
            .unwrap()
            .state
            .start(pose_id, 0.0, true, 0.0, 0.0);
        {
            let mut dodge = h
                .app
                .world_mut()
                .get_mut::<FighterDodge>(h.fighter)
                .unwrap();
            dodge.pose = pose_id;
            dodge.pose_entered = false;
        }
        h.app
            .world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(dt));
        h.app
            .world_mut()
            .run_system_once(crate::plugins::movement_plugin::dodge_root_motion)
            .unwrap();
        let expected = Vec3::new(
            row["world"][0].as_f64().unwrap() as f32,
            0.0,
            row["world"][2].as_f64().unwrap() as f32,
        );
        let actual = h.position();
        assert!(
            Vec2::new(actual.x - expected.x, actual.z - expected.z).length() < 2e-5,
            "original Pose{pose_id} frame{frame} {fps}FPS: actual={actual:?} expected={expected:?}"
        );
    }
}

#[test]
fn production_fake_forward_roll_still_stops_at_stage_wall() {
    for fps in [30, 60, 120] {
        let mut h = spawn_gameplay_harness_with_order(6, true).expect("original resources");
        install_guard_test_animation(&mut h);
        h.replace_stage_mesh(harness_ground_with_front_wall(3.0));
        let keys = crate::app_state::Keymap::default();
        h.press_key(KeyCode::KeyW);
        for _ in 0..fps / 2 {
            h.step_dt(1.0 / fps as f32);
        }
        h.press_binding(keys.fake);
        let mut rolled = false;
        for tick in 0..fps {
            h.step_dt(1.0 / fps as f32);
            if tick == 0 {
                h.release_binding(keys.fake);
            }
            rolled |= h.pose_id() == 176;
            assert!(
                h.position().z < 3.0,
                "roll must not cross the wall at {fps} FPS"
            );
        }
        assert!(rolled);
        assert!(
            !h.app
                .world()
                .get::<FighterDodge>(h.fighter)
                .unwrap()
                .state
                .active
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn production_walk_over_prone_target_does_not_use_standing_capsule() {
    for fps in [30, 60, 120] {
        for (pose_id, remote) in [112, 113].into_iter().flat_map(|p| [(p, false), (p, true)]) {
            let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(6, true);
            h.app.insert_resource(State::new(AppState::Playing));
            h.app.add_plugins(crate::plugins::CharacterContactPlugin);
            install_guard_test_animation(&mut h);
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"]
                .apply_pose(&mut pose, pose_id, true);
            h.app.world_mut().entity_mut(target).insert(pose);
            if remote {
                h.app.world_mut().entity_mut(target).insert(RemoteFighter {
                    player_id: "stock-2".into(),
                });
            }
            h.app
                .world_mut()
                .get_mut::<SmState>(target)
                .unwrap()
                .current = FighterState::Down;
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(0.0, 0.0, 3.0);
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation
                .x = 0.6;
            h.press_key(KeyCode::KeyW);
            let mut crossing = None;
            for _ in 0..fps * 2 {
                h.step_dt(1.0 / fps as f32);
                let relative =
                    h.position() - h.app.world().get::<Transform>(target).unwrap().translation;
                if relative.z >= 0.0 {
                    crossing = Some(relative);
                    break;
                }
            }
            let crossing = crossing.expect("must reach the far side of a prone target");
            eprintln!("prone {pose_id} {fps}FPS remote={remote} crossing={crossing:?}");
            assert!(crossing.x.abs() < 1.8, "must cross inside the old 1.92-wide standing exclusion: {pose_id} {fps}FPS {crossing:?}");
            assert!(crossing.y.abs() < 0.05);
        }
    }
}
