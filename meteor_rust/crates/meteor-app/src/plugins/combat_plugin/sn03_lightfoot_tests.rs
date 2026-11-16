use super::*;

include!("sn03_wall_scan.rs");

#[test]
fn production_held_jump_waits_for_the_next_simulation_tick() {
    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
    let mut clock = crate::plugins::FrameClock::default();
    clock.sim_dt = 0.01;
    clock.wall_dt = 0.01;
    clock.wall_step = 0.01;
    h.app.insert_resource(clock);
    h.press_key(KeyCode::KeyW);
    h.press_key(KeyCode::Space);
    h.step_dt(0.01);
    let start = h.position();
    {
        let mut clock = h
            .app
            .world_mut()
            .resource_mut::<crate::plugins::FrameClock>();
        clock.sim_dt = 0.0;
        clock.wall_dt = 0.12;
        clock.wall_step = 0.0;
    }
    h.step_dt(0.12);
    assert!(
        !h.airborne(),
        "a render-only frame cannot run the original held-release mover branch"
    );
    assert!(
        h.app
            .world()
            .get::<FighterAir>(h.fighter)
            .unwrap()
            .jump_charge_active
    );
    {
        let mut clock = h
            .app
            .world_mut()
            .resource_mut::<crate::plugins::FrameClock>();
        clock.sim_dt = 0.05;
        clock.wall_dt = 0.01;
        clock.wall_step = 0.13;
    }
    h.step_dt(0.01);
    assert!(h.airborne());
    assert!((h.position() - start).length() < 1e-5);
    assert!(
        (h.app
            .world()
            .get::<FighterAir>(h.fighter)
            .unwrap()
            .air
            .vertical_speed
            - 45.0)
            .abs()
            < 1e-5
    );
}

#[test]
fn production_held_jump_trigger_defers_original_mover_for_one_tick() {
    for fps in [30, 60, 120, 165] {
        for entry in ["ground", "air", "wall"] {
            let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
            if entry != "ground" {
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = Vec3::new(0.0, 30.0, 0.0);
                h.app
                    .world_mut()
                    .get_mut::<SmState>(h.fighter)
                    .unwrap()
                    .current = FighterState::Jump;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .movement_mode = meteor_core::MovementMode::Airborne;
                let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                air.recovery_ground_mode = None;
                air.air.start_fall(0.0, 0.0);
                air.air.vertical_speed = 30.0;
                air.ground_contact = false;
            }
            if entry == "wall" {
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation
                    .y = 10.0;
                h.replace_stage_mesh(harness_ground_with_front_wall(2.0));
                h.app
                    .world_mut()
                    .get_mut::<FighterAir>(h.fighter)
                    .unwrap()
                    .air
                    .motion_mode = meteor_physics::jump::AirMotionMode::Wall;
                {
                    let mut wall = h.app.world_mut().get_mut::<FighterWall>(h.fighter).unwrap();
                    wall.cling_active = true;
                    wall.cling_pose = 161;
                    wall.wall_normal = Vec3::new(0.0, 0.0, -1.0);
                    wall.remaining = 1.8;
                    wall.external_accel = Vec3::new(0.0, 0.0, 60.0);
                }
                let mut pose = PoseRuntime::idle();
                h.app.world().resource::<CharacterAssetsSet>().0["p1"]
                    .apply_pose(&mut pose, 161, true);
                h.app.world_mut().entity_mut(h.fighter).insert(pose);
            }
            h.press_key(KeyCode::KeyW);
            h.press_key(KeyCode::Space);
            let mut consumed = false;
            for tick in 0..fps / 2 {
                let p = h.position();
                let active = h
                    .app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .jump_charge_active;
                let old_v = h
                    .app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .air
                    .vertical_speed;
                h.step_dt(1.0 / fps as f32);
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                if active && !air.jump_charge_active {
                    assert!(
                        (h.position() - p).length() < 1e-5,
                        "original held trigger returns before movement: {entry}/{fps} tick={tick} delta={:?}",
                        h.position() - p
                    );
                    if entry == "ground" {
                        assert!((air.air.vertical_speed - 45.0).abs() < 1e-5);
                    }
                    if entry == "air" {
                        assert!((air.air.vertical_speed - old_v).abs() < 1e-5);
                    }
                    if entry == "wall" {
                        assert!((air.air.vertical_speed - 45.0 / 2.0_f32.sqrt()).abs() < 1e-5);
                    }
                    let next_start = h.position();
                    h.step_dt(1.0 / fps as f32);
                    assert!(
                        (h.position() - next_start).length() > 0.01,
                        "the next actor tick must consume motion"
                    );
                    consumed = true;
                    break;
                }
            }
            assert!(consumed, "must consume a single held charge: {entry}/{fps}");
        }
    }
}

#[test]
fn production_sn03_single_held_jump_enters_wall_run_and_upper_terrain() {
    let mesh = super::landing_roof_tests::original_stage_mesh("sn03");
    for fps in [60, 120] {
        for delta in [-0.045, 0.0, 0.045] {
            let mut h = jump_pose_render_harness(0);
            h.replace_stage_mesh(mesh.clone());
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(-9.2 + delta, -0.2, -96.8 + delta);
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
            let mut saw_wall = false;
            let mut saw_upper_support = false;
            for _ in 0..fps * 3 {
                {
                    let mut clock = h
                        .app
                        .world_mut()
                        .resource_mut::<crate::plugins::FrameClock>();
                    crate::plugins::advance_frame_clock(&mut clock, 1.0 / fps as f32, 120);
                }
                h.step_dt(1.0 / fps as f32);
                let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
                saw_wall |= wall.cling_active && h.pose_id() == 161;
                assert_eq!(
                    wall.kick_pose, 0,
                    "a single held charge cannot turn into repeated wall kicks"
                );
                saw_upper_support |= saw_wall && !h.airborne() && h.position().y > 40.0;
                assert!(h.position().is_finite());
            }
            assert!(
                saw_wall && saw_upper_support,
                "fps={fps}, delta={delta}: one held jump must run up the slope to upper support"
            );
        }
    }
}

#[test]
#[ignore = "diagnostic matrix; approach paths and FPS are not stock capture parity assertions"]
fn diagnose_sn03_single_held_jump_entry_variants() {
    let mesh = super::landing_roof_tests::original_stage_mesh("sn03");
    let mut failures = Vec::new();
    for fps in [30, 60, 120, 165] {
        for (dx, dz, approach) in [
            (0.0, 0.0, 0.0),
            (-0.045, -0.045, 0.0),
            (0.045, 0.045, 0.0),
            (0.0, 0.0, 0.25),
            (0.0, 0.0, 0.5),
        ] {
            let mut h = jump_pose_render_harness(0);
            h.replace_stage_mesh(mesh.clone());
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(-9.2 + dx, -0.2, -96.8 + dz);
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
            let mut entered = None;
            let mut release = None;
            let mut max_y = -0.2_f32;
            let mut entered_top = false;
            for tick in 0..fps * 5 {
                if tick == (approach * fps as f32) as usize {
                    h.press_key(KeyCode::Space);
                }
                {
                    let mut clock = h
                        .app
                        .world_mut()
                        .resource_mut::<crate::plugins::FrameClock>();
                    crate::plugins::advance_frame_clock(&mut clock, 1.0 / fps as f32, 120);
                }
                h.step_dt(1.0 / fps as f32);
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
                max_y = max_y.max(h.position().y);
                if entered.is_none() && wall.cling_active {
                    entered = Some((tick, h.position()));
                }
                if entered.is_some() && !wall.cling_active && release.is_none() {
                    release = Some((tick, h.position(), air.air.vertical_speed));
                }
                entered_top |= entered.is_some() && !h.airborne() && h.position().y > 40.0;
            }
            eprintln!(
                "ONE HOLD fps={fps} delta=({dx},{dz}) approach={approach} entered={entered:?} released={release:?} max_y={max_y} top={entered_top} end={:?}",
                h.position()
            );
            if entered.is_none() || !entered_top {
                failures.push((fps, dx, dz, approach, max_y));
            }
        }
    }
    eprintln!("UNVERIFIED ENTRY VARIANTS THAT DID NOT REACH UPPER TERRAIN: {failures:?}");
}

#[test]
#[ignore = "exports premature wall release snapshots for untouched EXE comparison"]
fn export_sn03_early_wall_release() {
    let mesh = super::landing_roof_tests::original_stage_mesh("sn03");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../");
    let wp = std::fs::read_to_string(root.join("origin_res/Level/sn03.wp")).unwrap();
    let center = meteor_scene::stage_loader::get_stage_center("sn03", Some(&wp));
    let mut cases = Vec::new();
    let mut steps = Vec::new();
    for fps in [30, 165] {
        let mut h = jump_pose_render_harness(0);
        h.replace_stage_mesh(mesh.clone());
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = Vec3::new(-9.2, -0.2, -96.8);
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
        for tick in 0..fps * 2 {
            {
                let mut clock = h
                    .app
                    .world_mut()
                    .resource_mut::<crate::plugins::FrameClock>();
                crate::plugins::advance_frame_clock(&mut clock, 1.0 / fps as f32, 120);
            }
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
            let active = wall.cling_active;
            let elapsed = 1.8 - wall.remaining;
            let snapshot = serde_json::json!({"fps":fps,"tick":tick,"p":h.position().to_array(),"v":[air.air.planar_x,air.air.vertical_speed,air.air.planar_z],"normal":wall.wall_normal.to_array(),"external":wall.external_accel.to_array(),"elapsed":elapsed,"dt":h.app.world().resource::<crate::plugins::FrameClock>().sim_dt});
            h.step_dt(1.0 / fps as f32);
            if active {
                let mut step = snapshot.clone();
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                step["after"] = serde_json::json!({"p":h.position().to_array(),"v":[air.air.planar_x,air.air.vertical_speed,air.air.planar_z],"wall":h.app.world().get::<FighterWall>(h.fighter).unwrap().cling_active});
                steps.push(step);
            }
            if active
                && elapsed < 1.7
                && !h
                    .app
                    .world()
                    .get::<FighterWall>(h.fighter)
                    .unwrap()
                    .cling_active
            {
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                let mut case = snapshot;
                case["after"] = serde_json::json!({"p":h.position().to_array(),"v":[air.air.planar_x,air.air.vertical_speed,air.air.planar_z],"pose":h.pose_id()});
                cases.push(case);
                break;
            }
        }
    }
    assert_eq!(cases.len(), 2);
    std::fs::write(
        root.join("analysis/sn03-slope-20261004/early-release.json"),
        serde_json::to_string_pretty(
            &serde_json::json!({"center":center,"cases":cases,"steps":steps}),
        )
        .unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "diagnostic for the screenshot slope, not a stock-game trajectory"]
fn diagnose_sn03_screenshot_slope_jump() {
    use meteor_physics::collision::{CollisionSystem, Point3};
    let mesh = super::landing_roof_tests::original_stage_mesh("sn03");
    let start = Vec3::new(-9.2, -0.2, -96.8);
    let mut collision = CollisionSystem::new(0.96, 3.84);
    collision.mesh = Some(mesh.clone());
    eprintln!(
        "SLOPE ground={:?} occupy={}",
        collision.sample_ground_hit(start.x, start.z, start.y),
        collision.can_occupy(Point3::new(start.x, start.y, start.z))
    );
    for delta in [-1.4, -0.02, 0.3] {
        eprintln!(
            "SLOPE vertical {delta}: {:?}",
            collision.resolve_y(Point3::new(start.x, start.y, start.z), delta)
        );
    }
    for fps in [60, 120] {
        for style in ["hold", "tap"] {
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
            h.press_key(KeyCode::KeyW);
            let mut previous = (u16::MAX, false, false);
            let mut max_y = start.y;
            for tick in 0..fps * 3 {
                if style == "hold" {
                    if tick == 0 {
                        h.press_key(KeyCode::Space);
                    }
                } else {
                    if tick % (fps / 5) == 0 {
                        h.press_key(KeyCode::Space);
                    }
                    if tick % (fps / 5) == fps / 10 {
                        h.release_key(KeyCode::Space);
                    }
                }
                h.step_dt(1.0 / fps as f32);
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
                let current = (h.pose_id(), h.airborne(), wall.cling_active);
                max_y = max_y.max(h.position().y);
                if previous != current || tick < 18 || tick % (fps / 4) == 0 {
                    eprintln!(
                        "SLOPE fps={fps} style={style} tick={tick} pose={} pos={:?} vy={} planar=({},{}) support={} vertical={} armed={} mode={:?} wall={} hold={} charge={}",
                        h.pose_id(),
                        h.position(),
                        air.air.vertical_speed,
                        air.air.planar_x,
                        air.air.planar_z,
                        air.ground_contact,
                        air.mover_vertical_contact,
                        air.air.cling_armed,
                        air.recovery_ground_mode,
                        wall.cling_active,
                        air.jump_hold,
                        air.jump_charge_active
                    );
                }
                previous = current;
            }
            eprintln!(
                "SLOPE RESULT fps={fps} style={style} max_y={max_y} end={:?}",
                h.position()
            );
        }
    }
}

#[test]
fn production_wall_timer_ages_on_render_frames_and_unclamped_wall_time() {
    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
    h.app.insert_resource(crate::plugins::FrameClock::default());
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation
        .y = 10.0;
    {
        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
        air.recovery_ground_mode = None;
        air.air.start_fall(0.0, 0.0);
        air.air.motion_mode = meteor_physics::jump::AirMotionMode::Wall;
    }
    {
        let mut wall = h.app.world_mut().get_mut::<FighterWall>(h.fighter).unwrap();
        wall.cling_active = true;
        wall.remaining = 1.8;
    }
    h.press_key(KeyCode::KeyW);
    for _ in 0..10 {
        let mut clock = h
            .app
            .world_mut()
            .resource_mut::<crate::plugins::FrameClock>();
        clock.sim_dt = 0.0;
        clock.wall_dt = 0.01;
        clock.wall_step = 0.0;
        drop(clock);
        h.step_dt(0.01);
    }
    assert!(
        (h.app
            .world()
            .get::<FighterWall>(h.fighter)
            .unwrap()
            .remaining
            - 1.7)
            .abs()
            < 1e-6
    );
    {
        let mut clock = h
            .app
            .world_mut()
            .resource_mut::<crate::plugins::FrameClock>();
        clock.sim_dt = 0.05;
        clock.wall_dt = 2.0;
        clock.wall_step = 2.0;
    }
    h.step_dt(2.0);
    assert!(
        !h.app
            .world()
            .get::<FighterWall>(h.fighter)
            .unwrap()
            .cling_active
    );
    assert!(
        (h.position().y - 10.0).abs() < 1e-5,
        "expired mode2 returns before integration"
    );
}
