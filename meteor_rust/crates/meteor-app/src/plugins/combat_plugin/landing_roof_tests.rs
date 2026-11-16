use super::*;

fn sn13_mesh() -> StageMeshCollider {
    original_stage_mesh("sn13")
}

pub(super) fn original_stage_mesh(stage: &str) -> StageMeshCollider {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let model = meteor_asset::gmdl::decode_gmdl(
        &std::fs::read(root.join(format!("{stage}/{stage}.gmb"))).unwrap(),
    )
    .unwrap();
    let desc = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join(format!("{stage}/{stage}.des"))).unwrap(),
    );
    let cob = meteor_asset::cob::parse_cob(
        &std::fs::read(root.join(format!("{stage}/{stage}.cob"))).unwrap(),
    )
    .unwrap();
    let wp = std::fs::read_to_string(root.join(format!("Level/{stage}.wp"))).unwrap();
    StageMeshCollider::from_gmdl(
        &model,
        &desc,
        Some(&cob.face_ids),
        meteor_scene::stage_loader::get_stage_center(stage, Some(&wp)),
    )
}

#[test]
fn production_sn05_pose_strategy_uses_entry_mode_before_contact_changes() {
    let mut h = jump_pose_render_harness(0);
    h.replace_stage_mesh(original_stage_mesh("sn05"));
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation = Vec3::new(22.55, 20.0, -37.55);
    h.app
        .world_mut()
        .get_mut::<MotionState>(h.fighter)
        .unwrap()
        .movement_mode = meteor_core::MovementMode::Airborne;
    {
        let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
        air.air.start_fall(0.0, 0.0);
        air.air.vertical_speed = -6.0;
        air.recovery_ground_mode = Some(true);
    }
    let mut pose = PoseRuntime::idle();
    h.app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut pose, 144, true);
    h.app
        .world_mut()
        .entity_mut(h.fighter)
        .insert((pose, EntryBlend::default()));
    h.press_key(KeyCode::KeyW);
    let instance = h
        .app
        .world()
        .get::<PoseRuntime>(h.fighter)
        .unwrap()
        .pose_instance_id;
    h.step_dt(1.0 / 120.0);
    assert_eq!(
        h.app
            .world()
            .get::<FighterAir>(h.fighter)
            .unwrap()
            .recovery_ground_mode,
        Some(false)
    );
    assert!(
        h.airborne(),
        "the unsupported mode1 mover must actually leave support"
    );
    assert_eq!(
        h.pose_id(),
        144,
        "4cd4f3 uses entry mode1 even with a saved descent speed below -40"
    );
    assert_eq!(
        h.app
            .world()
            .get::<PoseRuntime>(h.fighter)
            .unwrap()
            .pose_instance_id,
        instance,
        "mode1 must not emit a fall pose and overwrite it with run in the same tick"
    );
    h.step_dt(1.0 / 120.0);
    assert_eq!(
        h.pose_id(),
        152,
        "the next mode0 tick must consume the original fall-speed gate"
    );
}

#[test]
fn production_sn05_eave_preserves_run_pose_until_stock_fall_gate() {
    let mesh = original_stage_mesh("sn05");
    for character_id in [0, 1] {
        for fps in [30, 60, 120, 165] {
            for (from_air, offset) in [(false, 0.05), (true, 0.05)] {
                let mut h = jump_pose_render_harness(character_id);
                h.app
                    .world_mut()
                    .get_mut::<Fighter>(h.fighter)
                    .unwrap()
                    .weapon = 6;
                h.replace_stage_mesh(mesh.clone());
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = Vec3::new(22.5 + offset, 8.4, -37.6 + offset);
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .facing = 1.49;
                h.app
                    .world_mut()
                    .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                    .yaw = 1.49;
                {
                    let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                    if from_air {
                        air.air.start_fall(0.0, 0.0);
                        air.air.vertical_speed = -6.0;
                        air.ledge_fall = false;
                    } else {
                        air.recovery_ground_mode = Some(true);
                        air.ground_contact = true;
                    }
                }
                if from_air {
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .movement_mode = meteor_core::MovementMode::Airborne;
                    h.app
                        .world_mut()
                        .get_mut::<SmState>(h.fighter)
                        .unwrap()
                        .current = FighterState::Jump;
                    let mut pose = PoseRuntime::idle();
                    h.app.world().resource::<CharacterAssetsSet>().0[&format!("p{character_id}")]
                        .apply_pose(&mut pose, 180, true);
                    h.app.world_mut().entity_mut(h.fighter).insert(pose);
                }
                h.press_binding(crate::app_state::Keymap::default().move_up);
                let mut saw_run = false;
                let mut saw_mode0 = false;
                let mut saw_fall = false;
                for tick in 0..fps * 2 {
                    let source = h.pose_id();
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    let mode = air.recovery_ground_mode;
                    let speed = air.air.vertical_speed;
                    let before_x = h.position().x;
                    h.step_dt(1.0 / fps as f32);
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    saw_run |= h.pose_id() == 144;
                    saw_mode0 |= mode == Some(false) && before_x < 29.0;
                    if source == 144
                        && mode == Some(false)
                        && speed >= meteor_physics::jump::FALL_POSE_SPEED
                    {
                        assert_eq!(
                            h.pose_id(),
                            144,
                            "p{character_id}/{fps}FPS tick={tick}: ordinary mode0 below the fall gate must retain run; speed={speed}, pos={:?}",
                            h.position()
                        );
                    }
                    if saw_run && before_x < 29.0 {
                        assert_ne!(
                            h.pose_id(),
                            180,
                            "p{character_id}/{fps}FPS tick={tick}: roof contact gaps must not inject Pose180"
                        );
                    }
                    saw_fall |= h.pose_id() == 152 && h.position().x > 29.0;
                    assert!(
                        !h.app
                            .world()
                            .get::<FighterWall>(h.fighter)
                            .unwrap()
                            .cling_active
                    );
                    assert!(
                        !air.air.cling_armed,
                        "walking off a roof does not arm lightfoot"
                    );
                    assert!(h.position().is_finite());
                }
                assert!(
                    saw_run && saw_mode0 && saw_fall,
                    "must cover roof running, actual mode0 contact gaps and genuine departure: p{character_id}/{fps}FPS run={saw_run} mode0={saw_mode0} fall={saw_fall} pos={:?} air={:?}",
                    h.position(),
                    h.app.world().get::<FighterAir>(h.fighter).unwrap().air
                );
                assert!(
                    h.position().y < 1.0,
                    "leaving the eave must fall to the street"
                );
            }
        }
    }
}

#[test]
fn production_sn13_landing_roof_pose_must_not_restart() {
    use meteor_physics::collision::{CollisionSystem, Point3};
    let mesh = sn13_mesh();
    let start = Vec3::new(-26.699581, 7.3694787, 2.643674);
    let mut collision = CollisionSystem::new(0.96, 3.84);
    collision.mesh = Some(mesh.clone());
    for delta in [-1.4, -0.02, -0.008] {
        eprintln!(
            "ROOF descent {delta}: {:?}",
            collision.resolve_descent(Point3::new(start.x, start.y, start.z), delta)
        );
    }
    for fps in [30, 60, 120, 165] {
        let mut h = spawn_gameplay_harness_with_order(5, true).unwrap();
        h.replace_stage_mesh(mesh.clone());

        h.app.add_systems(
            super::super::super::fighter_frame::FighterUpdate,
            harness_retire_entry_blend.in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        );
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = start;
        let mut changes = 0;
        let mut previous = h.pose_id();
        for tick in 0..fps * 2 {
            h.step_dt(1.0 / fps as f32);
            if h.pose_id() != previous {
                changes += 1;
            }
            previous = h.pose_id();
            if tick < 8 {
                eprintln!(
                    "ROOF fps={fps} tick={tick} pose={} pos={:?} air={}",
                    h.pose_id(),
                    h.position(),
                    h.airborne()
                );
            }
        }
        assert!(
            changes <= 4,
            "{fps} FPS: stationary roof restarted Pose {changes} times"
        );
        assert!(!h.airborne());
        let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
        assert_eq!(pose.pose_id, 0);
        assert!(
            pose.frame > pose.start as f32 + 20.0,
            "idle must actually advance: frame={} blend={:?}",
            pose.frame,
            h.app.world().get::<EntryBlend>(h.fighter).unwrap()
        );
        assert!(!h
            .app
            .world()
            .get::<EntryBlend>(h.fighter)
            .unwrap()
            .is_active_for(0));

        let keymap = crate::app_state::Keymap::default();
        h.app
            .world_mut()
            .get_mut::<MotionState>(h.fighter)
            .unwrap()
            .facing = 2.33;
        h.app
            .world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = 2.33;
        h.press_binding(keymap.move_up);
        for _ in 0..fps * 2 {
            h.step_dt(1.0 / fps as f32);
        }
        assert!(
            h.position().y < 1.0,
            "{fps}: leaving the eave must fall to street: {:?}",
            h.position()
        );
    }
}

#[test]
fn production_sn13_landing_from_captured_air_entry() {
    let mesh = sn13_mesh();

    for fps in [30, 60, 120, 165] {
        let mut h = spawn_gameplay_harness_with_order(5, true).unwrap();
        h.replace_stage_mesh(mesh.clone());

        h.app.add_systems(
            super::super::super::fighter_frame::FighterUpdate,
            harness_retire_entry_blend.in_set(crate::plugins::FighterFrameSet::SkeletonSample),
        );
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = Vec3::new(-27.016592, 7.602396, 2.539668);
        h.app
            .world_mut()
            .get_mut::<SmState>(h.fighter)
            .unwrap()
            .current = FighterState::Jump;
        {
            let mut motion = h.app.world_mut().get_mut::<MotionState>(h.fighter).unwrap();
            motion.movement_mode = meteor_core::MovementMode::Airborne;
            motion.facing = -2.2462392;
            motion.velocity = Vec3::new(-1.5398362, -17.027739, 1.9219264);
        }
        let mut pose = PoseRuntime::default();
        h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 152, true);
        h.app.world_mut().entity_mut(h.fighter).insert(pose);
        let now = h
            .app
            .world()
            .resource::<Time<bevy::time::Real>>()
            .elapsed_secs_f64();
        h.app.world_mut().entity_mut(h.fighter).insert(EntryBlend {
            pose_id: 152,
            to_frame: 670.0,
            duration: 0.12,
            elapsed: 0.09706796,
            started_at_seconds: Some(now - 0.09706796),
            ..Default::default()
        });
        {
            let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
            air.air.airborne = true;
            air.air.vertical_speed = -17.02771;
            air.air.planar_x = -1.5398362;
            air.air.planar_z = 1.9219264;
            air.air.air_time = 0.5768226;
            air.carry_no_support = true;
            air.ground_y = 0.12067127;
        }
        let mut changes = 0;
        let mut roof_contact = false;
        let mut stable_ticks = 0;
        let mut previous = h.pose_id();
        for _ in 0..fps * 2 {
            h.step_dt(1.0 / fps as f32);
            if h.pose_id() != previous {
                changes += 1;
                stable_ticks = 0;
            } else {
                stable_ticks += 1;
            }
            previous = h.pose_id();
            roof_contact |= h.position().y > 7.0
                && h.app
                    .world()
                    .get::<FighterAir>(h.fighter)
                    .unwrap()
                    .recovery_ground_mode
                    == Some(true);
        }
        eprintln!(
            "AIR ENTRY fps={fps} changes={changes} roof_contact={roof_contact} end={:?}",
            h.position()
        );
        assert!(
            roof_contact,
            "must consume the roof contact before any later slide"
        );

        assert!(
            stable_ticks >= fps / 2,
            "{fps}: air entry still restarts after settling ({changes} changes)"
        );
        assert!(!h.airborne());

        let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
        assert_eq!(pose.pose_id, 0);
        assert!(pose.frame > pose.start as f32, "settled idle must advance");
        assert!(!h
            .app
            .world()
            .get::<EntryBlend>(h.fighter)
            .unwrap()
            .is_active_for(0));
    }
}
