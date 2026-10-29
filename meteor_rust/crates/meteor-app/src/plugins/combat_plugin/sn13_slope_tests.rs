#![cfg(feature = "original-assets")]
use super::*;
use crate::plugins::movement_plugin::FighterAttackMotion;
use serde_json::Value;

fn recording() -> (std::path::PathBuf, Value) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../analysis/sn13-slope-capture");
    let data =
        std::fs::read_to_string(dir.join("motion-hook-20260922-220903.parity.json")).unwrap();
    (dir, serde_json::from_str(&data).unwrap())
}

fn scalar(v: &Value) -> f32 {
    v.as_f64().unwrap() as f32
}

#[test]
fn recorded_sn13_free_descent_speed_and_displacement() {
    let data: Value = serde_json::from_str(include_str!("fixtures/sn13-descent.json")).unwrap();
    let mut failures = Vec::new();
    for case in data["cases"].as_array().unwrap() {

        if case["result"] != 0 {
            continue;
        }
        let mut h = harness();
        let source = &case["before"];
        let p = position(&source["positionSource"], &data["center"]);
        let v = vector(&source["velocitySource"]);
        let id = source["model"]["pose"].as_u64().unwrap() as u16;
        let mut pose = PoseRuntime::idle();
        h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, id, true);
        h.app.world_mut().entity_mut(h.fighter).insert(pose);
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = p;
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
            let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
            air.air.start_fall(v.x, v.z);
            air.air.vertical_speed = v.y;
            air.air.air_time = 0.5;
            air.ground_contact = source["support35c"] == 1;
            air.ground_external_velocity = vector(&source["groundExternalSource"]);
        }
        {
            let mut motion = h.app.world_mut().get_mut::<MotionState>(h.fighter).unwrap();
            motion.facing = scalar(&case["yaw"]);
            motion.velocity = v;
            motion.movement_mode = meteor_core::MovementMode::Airborne;
        }
        h.app
            .world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = scalar(&case["yaw"]);
        let axes = &case["enter"]["inputAxes"];
        for (axis, positive, negative) in [
            (0, KeyCode::KeyW, KeyCode::KeyS),
            (1, KeyCode::KeyD, KeyCode::KeyA),
        ] {
            match axes[axis].as_i64().unwrap() {
                1 => h.press_key(positive),
                -1 => h.press_key(negative),
                _ => (),
            }
        }
        h.step_dt(scalar(&case["dt"]));
        let expected = position(&case["after"]["positionSource"], &data["center"]);
        let expected_v = vector(&case["after"]["velocitySource"]);
        let actual_v = h
            .app
            .world()
            .get::<FighterAir>(h.fighter)
            .unwrap()
            .air
            .vertical_speed;
        if (h.position().y - expected.y).abs() > 0.02 || (actual_v - expected_v.y).abs() > 0.15 {
            failures.push(format!(
                "source={} t={} flags={} dy={} stock_dy={} vy={} stock_vy={}",
                case["source"],
                case["t"],
                case["result"],
                h.position().y - p.y,
                expected.y - p.y,
                actual_v,
                expected_v.y
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "compare no-input recovery fall duration across simulation rates"]
fn diagnose_sn13_descent_frame_rates() {
    let (_, data) = recording();
    for index in [0, 5, 6, 7, 8, 9] {
        let episode = &data["episodes"][index];
        let source = &episode["frames"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| scalar(&r["t"]) > scalar(&episode["next_pose"]["ms"]) / 1000.0)
            .unwrap()["motion"];
        for fps in [60.0, 125.0, 165.0] {
            let mut h = harness();
            let p = position(&source["positionSource"], &data["center"]);
            let v = vector(&source["velocitySource"]);
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = p;
            {
                let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                air.air.start_fall(v.x, v.z);
                air.air.vertical_speed = v.y;
                air.recovery_ground_mode = Some(source["rawDc"] == 1);
                air.ground_contact = source["support35c"] == 1;
                air.ground_external_velocity = vector(&source["groundExternalSource"]);
            }
            for _ in 0..(fps * 0.7) as usize {
                h.step_dt(1.0 / fps);
            }
            eprintln!(
                "episode={index} fps={fps} start={} final={} drop={}",
                p.y,
                h.position().y,
                p.y - h.position().y
            );
        }
    }
}

#[test]
fn recorded_sn13_ramp_jumps_do_not_enter_wall_run() {
    let data: Value = serde_json::from_str(include_str!("fixtures/sn13-ramp.json")).unwrap();
    let mut failures = Vec::new();
    for case in data["jumps"].as_array().unwrap() {

        for fps in [1.0 / scalar(&case["dt"])] {
            let mut h = harness();
            let p = position(&case["position"], &data["center"]);
            let v = vector(&case["velocity"]);
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = p;
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 151, true);
            h.app.world_mut().entity_mut(h.fighter).insert(pose);
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
                let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                air.air.start_fall(v.x, v.z);
                air.air.vertical_speed = v.y;
                air.air.air_time = scalar(&case["dt"]);
                air.air.cling_armed = true;
                air.ground_contact = case["support"] == 1;
            }
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = scalar(&case["yaw"]);
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = scalar(&case["yaw"]);
            h.press_key(KeyCode::KeyW);
            for tick in 0..(fps * 0.5) as usize {
                let followup = case["followups"].as_array().unwrap().get(tick);
                let speed_reference = (scalar(&case["t"]) - 34.1901).abs() < 0.001;
                if speed_reference {
                    if let Some(row) = followup {
                        h.app
                            .world_mut()
                            .get_mut::<MotionState>(h.fighter)
                            .unwrap()
                            .facing = scalar(&row["yaw"]);
                        h.app
                            .world_mut()
                            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                            .yaw = scalar(&row["yaw"]);
                    }
                }
                h.step_dt(if speed_reference {
                    followup.map_or(1.0 / fps, |row| scalar(&row["dt"]))
                } else {
                    1.0 / fps
                });
                if speed_reference {
                    if let Some(row) = followup {
                        let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                        let actual = Vec2::new(air.air.planar_x, air.air.planar_z).length();
                        let expected = vector(&row["velocity"]);
                        let expected = Vec2::new(expected.x, expected.z).length();
                        if (actual - expected).abs() > 0.15 {
                            failures.push(format!("ramp acceleration tick={tick} actual={actual} stock={expected} support={}", air.ground_contact));
                        }
                    }
                }
                if speed_reference && tick == 4 {
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    assert!(
                        !air.air.cling_armed,
                        "rising floor contact disarms wall entry"
                    );
                    assert!(
                        air.air.vertical_speed > 20.0,
                        "contact must not kill rising speed"
                    );
                    assert_eq!(h.pose_id(), 151, "contact must not force early landing");
                }
                if matches!(h.pose_id(), 161 | 162 | 163) {
                    failures.push(format!(
                        "stock t={} fps={fps} tick={tick} pose={}",
                        case["t"],
                        h.pose_id()
                    ));
                    break;
                }
            }
            if (scalar(&case["t"]) - 34.1901).abs() < 0.001 {
                let mut held = false;
                let mut jumps = 0;
                let mut peak_speed = 0.0_f32;
                for _ in 0..(fps * 1.5) as usize {
                    let was_airborne = h.airborne();
                    if held {
                        h.release_key(KeyCode::Space);
                        held = false;
                    } else if !was_airborne {
                        h.press_key(KeyCode::Space);
                        held = true;
                    }
                    h.step_dt(1.0 / fps);
                    if !was_airborne && h.airborne() {
                        jumps += 1;
                    }
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    peak_speed = peak_speed.max(air.air.planar_x.hypot(air.air.planar_z));
                    assert!(!matches!(h.pose_id(), 161 | 162 | 163));
                }
                assert!(jumps >= 2, "repeated ramp takeoffs: {jumps}");
                assert!(
                    peak_speed > 40.0,
                    "repeated jumps must accumulate speed: {peak_speed}"
                );
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
fn vector(v: &Value) -> Vec3 {
    Vec3::new(scalar(&v[0]), scalar(&v[2]), -scalar(&v[1])) * 0.12
}
fn position(v: &Value, center: &Value) -> Vec3 {
    vector(v) - Vec3::new(scalar(&center[0]), 0.0, -scalar(&center[1])) * 0.12
}

fn harness() -> GameplayHarness {
    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
    h.replace_stage_mesh(sn13_test_mesh());
    h.app.world_mut().entity_mut(h.fighter).insert((
        HeadTrackState::default(),
        crate::components::fighter::VisualRootPin::default(),
    ));
    crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
    let schedule = crate::plugins::fighter_frame::schedule(&h.app);
    h.app.add_systems(
        schedule,
        crate::plugins::character_plugin::drive_fighter_animation_for_test
            .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
    );
    h
}

fn input(h: &mut GameplayHarness, row: &Value, old_mask: &mut u64) {
    let mask = row["keys"].as_u64().unwrap();
    for (i, key) in [
        KeyCode::KeyW,
        KeyCode::KeyS,
        KeyCode::KeyA,
        KeyCode::KeyD,
        KeyCode::Space,
    ]
    .into_iter()
    .enumerate()
    {
        if (mask ^ *old_mask) & (1 << i) != 0 {
            if mask & (1 << i) != 0 {
                h.press_key(key);
            } else {
                h.release_key(key);
            }
        }
    }
    if (mask ^ *old_mask) & 32 != 0 {
        if mask & 32 != 0 {
            h.press_mouse(MouseButton::Left);
        } else {
            h.release_mouse(MouseButton::Left);
        }
    }
    *old_mask = mask;
    let yaw = scalar(&row["yaw"]);
    h.app
        .world_mut()
        .get_mut::<MotionState>(h.fighter)
        .unwrap()
        .facing = yaw;
    h.app
        .world_mut()
        .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
        .yaw = yaw;
}

#[test]
fn recorded_sn13_recovery_tail_hands_back_to_stock_locomotion() {
    let data: Value = serde_json::from_str(include_str!("fixtures/sn13-recovery.json")).unwrap();
    let mut report = String::from("episode,t,pose,frame,loops,air,mode,x,y,z\n");
    let mut failures = Vec::new();
    for (index, episode) in data["episodes"].as_array().unwrap().iter().enumerate() {
        let mut h = harness();
        let source = &episode["before_release"]["character"];
        let id = episode["pose"].as_u64().unwrap() as u16;
        let p = position(&source["positionSource"], &data["center"]);
        let v = vector(&source["velocitySource"]);
        let carry = vector(&source["carrySource"]);
        let external = vector(&source["groundExternalSource"]);
        let mut pose = PoseRuntime::idle();
        let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
        assets.apply_pose(&mut pose, id, true);
        let root = assets.motion_root_at(
            &h.app
                .world()
                .resource::<crate::plugins::character_plugin::CommonAmb>()
                .0,
            id,
            pose.start as f32,
        );
        h.app.world_mut().entity_mut(h.fighter).insert(pose);
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = p;
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
        {
            let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
            air.air.start_fall(v.x, v.z);
            air.air.start_curve(false, id);
            air.air.vertical_speed = v.y;
            air.air.air_time = 1.0;
            air.ground_contact = source["support35c"] == 1;
            air.mover_vertical_contact = source["support360"] == 1;
            air.ground_external_velocity = external;
            air.carry_decay_steep = source["support35c"] == 0;
            air.pin_root_z = 0.0;
            air.motion_root_pin = Some([root[0], root[1], 0.0]);
            air.motion_root_pin_pose_id = id;

            if episode["next_pose"]["target"] == 151 {
                air.jump_charge_active = true;
                air.jump_hold = scalar(&episode["held_jump_seconds"]);
            }
        }
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert(FighterAttackMotion {
                carry_x: carry.x,
                carry_z: carry.z,
                carry_banked: true,
                ..default()
            });
        h.app
            .world_mut()
            .get_mut::<MotionState>(h.fighter)
            .unwrap()
            .movement_mode = meteor_core::MovementMode::Airborne;
        let mut mask = 0;

        input(&mut h, &episode["frames"][0], &mut mask);
        h.app
            .world_mut()
            .resource_mut::<Messages<WindowEvent>>()
            .clear();
        h.app
            .world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        h.app
            .world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        if mask & 16 != 0 {
            let mut jump = h.app.world_mut().resource_mut::<LocalJumpInput>();
            jump.held = true;
            jump.hold_seconds = scalar(&episode["held_jump_seconds"]);
        }
        let mut first_change = None;
        let mut first_release = None;
        let mut terminal_seen = false;
        let mut following = Vec::new();
        for row in episode["frames"].as_array().unwrap() {
            input(&mut h, row, &mut mask);
            let before_step = h.position();
            h.step_dt(scalar(&row["dt"]));
            let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            if pose.pose_id == id && !pose.loops && first_release.is_none() {
                first_release = Some(scalar(&row["t"]));
            }
            if pose.pose_id == id && pose.finished() {
                terminal_seen = true;
                assert!(
                    h.position().distance(before_step) < 1e-5,
                    "episode {index}: state 4 must return before its mover"
                );
            }
            if first_change.is_none() && pose.pose_id != id {
                first_change = Some((pose.pose_id, scalar(&row["t"])));
            }
            if first_change.is_some() && following.last().is_none_or(|&(id, _)| id != pose.pose_id)
            {
                following.push((pose.pose_id, scalar(&row["t"])));
            }
            use std::fmt::Write;
            let p = h.position();
            writeln!(
                report,
                "{index},{},{},{},{},{},{:?},{},{},{}",
                row["t"],
                pose.pose_id,
                pose.frame,
                pose.loops,
                air.air.airborne,
                air.air.motion_mode,
                p.x,
                p.y,
                p.z
            )
            .unwrap();
        }
        let expected = episode["next_pose"]["target"].as_u64().unwrap() as u16;
        let expected_time = scalar(&episode["next_pose"]["ms"]) / 1000.0;
        let release_time = scalar(&episode["release"]["ms"]) / 1000.0;
        assert!(
            first_release.is_some_and(|t| (t - release_time).abs() < 0.002),
            "episode {index}: release {first_release:?}, expected {release_time}"
        );
        assert!(
            terminal_seen,
            "episode {index}: missing terminal recovery sample"
        );
        if !first_change
            .is_some_and(|(pose, t)| pose == expected && (t - expected_time).abs() < 0.02)
        {
            failures.push(format!(
                "episode {index}: actual {first_change:?}, expected {expected} at {expected_time}"
            ));
        }
        let expected_following = episode["following_poses"].as_array().unwrap();
        let expected_ids: Vec<_> = expected_following
            .iter()
            .map(|r| r["pose"].as_u64().unwrap() as u16)
            .collect();

        if let Some(stop) = following
            .iter()
            .position(|&(id, _)| matches!(id, 151 | 153 | 155 | 157 | 245 | 246))
        {
            following.truncate(stop + 1);
        }
        let actual_ids: Vec<_> = following.iter().map(|&(id, _)| id).collect();
        if actual_ids != expected_ids {
            failures.push(format!(
                "episode {index}: fall sequence {actual_ids:?}, stock {expected_ids:?}"
            ));
        } else {
            for (&(id, t), expected) in following.iter().zip(expected_following) {
                let delta = t - scalar(&expected["t"]);
                if delta.abs() > 0.002 {
                    failures.push(format!(
                        "episode {index}: pose {id} at {t}, stock {}, delta {delta}",
                        expected["t"]
                    ));
                }
            }
        }
    }
    if let Ok(path) = std::env::var("METEOR_SN13_RECOVERY_REPORT") {
        std::fs::write(path, report).unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "full user input trajectory diagnostic; exports every frame, not a parity acceptance"]
fn diagnose_recorded_sn13_full_route() {
    let (dir, data) = recording();
    let mut h = harness();
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation = position(&data["initial"]["positionSource"], &data["center"]);
    let mut mask = 0;
    let mut report = String::from("t,pose,x,y,z,stock_pose,stock_x,stock_y,stock_z,error\n");
    for row in data["frames"].as_array().unwrap() {
        input(&mut h, row, &mut mask);
        h.step_dt(scalar(&row["dt"]));
        if row["motion"].is_null() {
            continue;
        }
        let p = h.position();
        let stock = position(&row["motion"]["positionSource"], &data["center"]);
        use std::fmt::Write;
        writeln!(
            report,
            "{},{},{},{},{},{},{},{},{},{}",
            row["t"],
            h.pose_id(),
            p.x,
            p.y,
            p.z,
            row["motion"]["model"]["pose"],
            stock.x,
            stock.y,
            stock.z,
            p.distance(stock)
        )
        .unwrap();
    }
    std::fs::write(dir.join("rust-full-route.csv"), report).unwrap();
}

#[test]
#[ignore = "Diagnostic: synthetic repeated ramp inputs, not a stock trajectory reference"]
fn diagnose_repeated_ramp_jump_endpoints() {
    let data: Value = serde_json::from_str(include_str!("fixtures/sn13-ramp.json")).unwrap();
    let case = data["jumps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| (scalar(&case["t"]) - 34.1901).abs() < 0.001)
        .unwrap();
    for fps in [60.0, 125.0, 165.0, 240.0, 360.0] {
        let mut h = harness();
        let p = position(&case["position"], &data["center"]);
        let v = vector(&case["velocity"]);
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = p;
        let mut pose = PoseRuntime::idle();
        h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 151, true);
        h.app.world_mut().entity_mut(h.fighter).insert(pose);
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
            let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
            air.air.start_fall(v.x, v.z);
            air.air.vertical_speed = v.y;
            air.air.air_time = scalar(&case["dt"]);
            air.air.cling_armed = true;
            air.ground_contact = case["support"] == 1;
        }
        h.app
            .world_mut()
            .get_mut::<MotionState>(h.fighter)
            .unwrap()
            .facing = scalar(&case["yaw"]);
        h.app
            .world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = scalar(&case["yaw"]);
        h.press_key(KeyCode::KeyW);

        let mut held = false;
        for tick in 0..(fps * 3.0) as usize {
            let before = h.position();
            let old_pose = h.pose_id();
            let was_airborne = h.airborne();
            if held {
                h.release_key(KeyCode::Space);
                held = false;
            } else if !was_airborne {
                h.press_key(KeyCode::Space);
                held = true;
            }
            h.step_dt(1.0 / fps);
            let after = h.position();
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            if old_pose != h.pose_id()
                || was_airborne != h.airborne()
                || (after.y - before.y).abs() > 0.75
            {
                eprintln!("ramp fps={fps} tick={tick} pose={old_pose}->{} airborne={was_airborne}->{} pos={before:?}->{after:?} vy={} ground={} pin={:?}", h.pose_id(), h.airborne(), air.air.vertical_speed, air.ground_y, air.motion_root_pin);
            }
        }
    }
}
