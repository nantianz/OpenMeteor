#[test]
fn production_sn03_nearby_wall_routes_reach_upper_support() {
    let mesh = super::landing_roof_tests::original_stage_mesh("sn03");
    for start in [
        Vec3::new(-12.047134, -0.21929741, -97.74542),
        Vec3::new(-8.884861, -0.21737146, -97.74905),
        Vec3::new(-6.3528647, -0.21485043, -95.85458),
    ] {
        let mut h = jump_pose_render_harness(0);
        h.replace_stage_mesh(mesh.clone());
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = start;
        h.app
            .world_mut()
            .get_mut::<MotionState>(h.fighter)
            .unwrap()
            .facing = -6.55;
        h.app
            .world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = -6.55;
        h.app.insert_resource(crate::plugins::FrameClock::default());
        h.press_key(KeyCode::KeyW);
        h.press_key(KeyCode::Space);
        let mut wall_entered = false;
        let mut upper_support = false;
        for _ in 0..360 {
            {
                let mut clock = h
                    .app
                    .world_mut()
                    .resource_mut::<crate::plugins::FrameClock>();
                crate::plugins::advance_frame_clock(&mut clock, 1.0 / 120.0, 120);
            }
            h.step_dt(1.0 / 120.0);
            let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
            wall_entered |= wall.cling_active && wall.cling_pose == 161;
            assert_eq!(
                wall.kick_pose, 0,
                "one held jump must not issue another kick"
            );
            upper_support |= wall_entered && !h.airborne() && h.position().y > 40.0;
            assert!(h.position().is_finite());
        }
        assert!(
            upper_support,
            "nearby start {start:?} must reach the upper surface"
        );
    }
}

#[test]
#[ignore = "scans nearby starts on the screenshot wall using automated production updates"]
fn diagnose_sn03_wall_neighborhood() {
    use meteor_physics::collision::{CollisionSystem, Point3};
    let mesh = super::landing_roof_tests::original_stage_mesh("sn03");
    let mut collision = CollisionSystem::new(0.96, 3.84);
    collision.mesh = Some(mesh.clone());
    let anchor = Vec3::new(-9.2, -0.2, -96.8);
    let outward = Vec3::new(0.315139, 0.0, -0.949045);
    let along = Vec3::new(0.949045, 0.0, 0.315139);
    let fps = 120;
    let mut rows = Vec::new();
    let full_paths = std::env::var_os("METEOR_WALL_FULL_PATHS").is_some();
    for lateral in [-6.0, -4.5, -3.0, -1.5, 0.0, 1.5, 3.0, 4.5, 6.0] {
        for distance in [-0.5, 0.0, 0.5, 1.0, 1.5, 2.0] {
            let extra_lateral =
                lateral == -4.5 || lateral == -1.5 || lateral == 1.5 || lateral == 4.5;
            if extra_lateral != (distance == 0.5 || distance == 1.5) {
                continue;
            }
            for turn in [-0.25, 0.0, 0.25] {
                let mut start = anchor + along * lateral + outward * distance;
                let ground = collision.sample_ground_hit(start.x, start.z, anchor.y);
                if let Some(hit) = ground {
                    start.y = hit.y;
                }
                let facing = -6.55 + turn;
                let mut row = serde_json::json!({"lateral":lateral,"distance":distance,"turn":turn,"start":start.to_array(),"facing":facing,"fps":fps});
                if ground.is_none() || !collision.can_occupy(Point3::new(start.x, start.y, start.z))
                {
                    row["result"] = "invalid-start".into();
                    rows.push(row);
                    continue;
                }
                let mut h = jump_pose_render_harness(0);
                h.replace_stage_mesh(mesh.clone());
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = start;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .facing = facing;
                h.app
                    .world_mut()
                    .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw = facing;
                h.app.insert_resource(crate::plugins::FrameClock::default());
                h.press_key(KeyCode::KeyW);
                h.press_key(KeyCode::Space);
                let mut entry = None;
                let mut exit = None;
                let mut max_y = start.y;
                let mut upper_support = None;
                let mut frames = Vec::new();
                for tick in 0..fps * 3 {
                    {
                        let mut clock = h
                            .app
                            .world_mut()
                            .resource_mut::<crate::plugins::FrameClock>();
                        crate::plugins::advance_frame_clock(&mut clock, 1.0 / fps as f32, 120);
                    }
                    let before = full_paths.then(|| sn03_path_snapshot(&h));
                    h.step_dt(1.0 / fps as f32);
                    if let Some(before) = before {
                        frames.push(serde_json::json!({"tick":tick,"dt":1.0 / fps as f32,"before":before,"after":sn03_path_snapshot(&h)}));
                    }
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
                    max_y = max_y.max(h.position().y);
                    if entry.is_none() && wall.cling_active {
                        entry = Some(
                            serde_json::json!({"tick":tick,"p":h.position().to_array(),"pose":wall.cling_pose,"normal":wall.wall_normal.to_array(),"v":[air.air.planar_x,air.air.vertical_speed,air.air.planar_z],"external":wall.external_accel.to_array()}),
                        );
                    }
                    if entry.is_some() && exit.is_none() && !wall.cling_active {
                        exit = Some(
                            serde_json::json!({"tick":tick,"p":h.position().to_array(),"remaining":wall.remaining,"v":[air.air.planar_x,air.air.vertical_speed,air.air.planar_z]}),
                        );
                    }
                    if entry.is_some() && !h.airborne() && h.position().y > 40.0 {
                        upper_support = Some(
                            serde_json::json!({"tick":tick,"p":h.position().to_array(),"pose":h.pose_id()}),
                        );
                        if !full_paths {
                            break;
                        }
                    }
                }
                let result = if upper_support.is_some() {
                    "climbed"
                } else if entry.is_some() {
                    "wall-released-before-top"
                } else {
                    "no-wall-entry"
                };
                if full_paths {
                    row["frames"] = serde_json::json!(frames);
                }
                row["result"] = result.into();
                row["entry"] = entry.into();
                row["exit"] = exit.into();
                row["upperSupport"] = upper_support.into();
                row["maxY"] = max_y.into();
                row["end"] = serde_json::json!(h.position().to_array());
                eprintln!(
                    "WALL SCAN lateral={lateral} distance={distance} turn={turn}: {result} start={start:?} max_y={max_y} end={:?}",
                    h.position()
                );
                rows.push(row);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../");
    let output = root.join(if full_paths {
        "analysis/sn03-slope-20261004/full-rust-paths.json"
    } else {
        "analysis/sn03-slope-20261004/wall-neighborhood-scan-expanded.json"
    });
    std::fs::write(output, serde_json::to_string_pretty(&rows).unwrap()).unwrap();
    assert!(
        rows.iter().any(|r| r["result"] == "climbed"),
        "the screenshot wall must have a reachable route"
    );
}

fn sn03_path_snapshot(h: &GameplayHarness) -> serde_json::Value {
    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
    let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
    let motion = h.app.world().get::<MotionState>(h.fighter).unwrap();
    let mode = if wall.cling_active {
        2
    } else {
        air.recovery_ground_mode
            .map_or(i32::from(!h.airborne()), i32::from)
    };
    let velocity = if mode == 1 && air.recovery_ground_mode.is_none() {
        motion.velocity
    } else {
        Vec3::new(air.air.planar_x, air.air.vertical_speed, air.air.planar_z)
    };
    serde_json::json!({"p":h.position().to_array(),"v":velocity.to_array(),"mode":mode,"airborne":h.airborne(),"pose":h.pose_id(),"support":air.ground_contact,"armed":air.air.cling_armed,"airTime":air.air.air_time,"remaining":wall.remaining,"normal":wall.wall_normal.to_array(),"wallExternal":wall.external_accel.to_array(),"external":air.ground_external_velocity.to_array(),"verticalContact":air.mover_vertical_contact,"charge":air.jump_charge_active,"hold":air.jump_hold})
}

#[test]
fn production_sn03_paths_match_original_controller_checkpoints() {
    let mesh = super::landing_roof_tests::original_stage_mesh("sn03");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../");
    let routes: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            root.join("analysis/sn03-slope-20261004/original-path-checkpoints.json"),
        )
        .unwrap(),
    )
    .unwrap();
    for route in routes.as_array().unwrap() {
        let p = route["start"].as_array().unwrap();
        let yaw = route["facing"].as_f64().unwrap() as f32;
        let mut h = jump_pose_render_harness(0);
        h.replace_stage_mesh(mesh.clone());
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = Vec3::new(
            p[0].as_f64().unwrap() as f32,
            p[1].as_f64().unwrap() as f32,
            p[2].as_f64().unwrap() as f32,
        );
        h.app
            .world_mut()
            .get_mut::<MotionState>(h.fighter)
            .unwrap()
            .facing = yaw;
        h.app
            .world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = yaw;
        h.app.insert_resource(crate::plugins::FrameClock::default());
        h.press_key(KeyCode::KeyW);
        h.press_key(KeyCode::Space);
        for tick in 0..360 {
            crate::plugins::advance_frame_clock(
                &mut h
                    .app
                    .world_mut()
                    .resource_mut::<crate::plugins::FrameClock>(),
                1.0 / 120.0,
                120,
            );
            h.step_dt(1.0 / 120.0);
            if let Some(expected) = route["checkpoints"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["tick"].as_u64().unwrap() == tick)
            {
                let p = expected["p"].as_array().unwrap();
                let target = Vec3::new(
                    p[0].as_f64().unwrap() as f32,
                    p[1].as_f64().unwrap() as f32,
                    p[2].as_f64().unwrap() as f32,
                );
                assert!(
                    (h.position() - target).abs().max_element()
                        < route["positionTolerance"].as_f64().unwrap() as f32,
                    "original case={} tick={tick}: target={target:?}, actual={:?}",
                    route["case"],
                    h.position()
                );
                assert_eq!(
                    h.app
                        .world()
                        .get::<FighterWall>(h.fighter)
                        .unwrap()
                        .cling_active,
                    expected["wall"].as_bool().unwrap(),
                    "original mode2 timing case={} tick={tick}",
                    route["case"]
                );
            }
        }
    }
}
