#[test]
#[ignore = "audits the user's sn19 recordings without game/UI operation"]
fn diagnose_user_lightfoot_mrc_paths() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../");
    let output = root.join("analysis/lightfoot-mrc-20261004");
    let wp = std::fs::read_to_string(root.join("origin_res/Level/sn19.wp")).unwrap();
    let center = meteor_scene::stage_loader::get_stage_center("sn19", Some(&wp));
    let mesh = super::landing_roof_tests::original_stage_mesh("sn19");
    let mut runs = Vec::new();
    for file in ["default.mrc", "default1.mrc"] {
        let doc =
            meteor_asset::mrc::decode_mrc(&std::fs::read(output.join(file)).unwrap()).unwrap();
        assert_eq!((doc.mark0, doc.mark1), (9, 7));
        assert_eq!(u32::from_le_bytes(doc.state[..4].try_into().unwrap()), 55);
        let identity = doc.queue_a.iter().find(|e| e.id == 9).unwrap();
        assert_eq!(
            meteor_network::stock_protocol::StockRoleEnteredEvent::decode(&identity.blob)
                .unwrap()
                .hero,
            0
        );
        let position = |e: &meteor_asset::mrc::MrcQueueDEntry| {
            Vec3::new(
                (f32::from_bits(e.a) - center[0]) * 0.12,
                (f32::from_bits(e.c) - center[2]) * 0.12,
                -(f32::from_bits(e.b) - center[1]) * 0.12,
            )
        };
        for fps in [60, 120] {
            for corrected in [false, true] {
                let mut h = jump_pose_render_harness(0);
                h.replace_stage_mesh(mesh.clone());
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = position(&doc.queue_d[0]);
                {
                    let mut fighter = h.app.world_mut().get_mut::<Fighter>(h.fighter).unwrap();
                    fighter.player_slot = 1;
                    fighter.weapon = 12;
                    fighter.weapon_slots = [12, 0];
                }
                h.app
                    .world_mut()
                    .entity_mut(h.fighter)
                    .insert((FighterAir::default(), FighterWall::default()));
                h.app.insert_resource(crate::plugins::FrameClock::default());
                let start = doc.queue_d[0].time;
                crate::plugins::replay_plugin::install_mrc_motion_audit(
                    &mut h.app,
                    doc.clone(),
                    start,
                    meteor_scene::stage_loader::parse_waypoint_graph(&wp, center),
                    corrected,
                );
                let mut index = 0;
                let mut previous_pose = h.pose_id();
                let mut transitions = Vec::new();
                let mut anchors = Vec::new();
                let mut frames = Vec::new();
                for _tick in 0..((doc.duration() - start) * fps as f32).ceil() as usize {
                    crate::plugins::advance_frame_clock(
                        &mut h
                            .app
                            .world_mut()
                            .resource_mut::<crate::plugins::FrameClock>(),
                        1.0 / fps as f32,
                        120,
                    );
                    let now = h
                        .app
                        .world()
                        .resource::<crate::plugins::replay_plugin::MrcPlayback>()
                        .elapsed
                        + h.app
                            .world()
                            .resource::<crate::plugins::FrameClock>()
                            .sim_dt;

                    let mut last = None;
                    while index < doc.queue_d.len() && doc.queue_d[index].time <= now {
                        last = Some(&doc.queue_d[index]);
                        index += 1;
                    }
                    if let Some(e) = last {
                        let wanted = position(e);
                        let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                        let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
                        anchors.push(serde_json::json!({"time":e.time,"tickTime":now,"expected":wanted.to_array(),"actual":h.position().to_array(),"error":h.position().distance(wanted),"pose":h.pose_id(),"airborne":h.airborne(),"wall":wall.cling_active,"velocity":[air.air.planar_x,air.air.vertical_speed,air.air.planar_z]}));
                    }
                    h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / fps as u64));
                    let pose = h.pose_id();
                    if previous_pose != pose {
                        transitions.push(
                            serde_json::json!({"time":now,"pose":pose,"p":h.position().to_array()}),
                        );
                        previous_pose = pose;
                    }
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    let wall = h.app.world().get::<FighterWall>(h.fighter).unwrap();
                    let mode = if wall.cling_active {
                        2
                    } else {
                        air.recovery_ground_mode.map_or(
                            if air.air.airborne { 0 } else { 1 },
                            |ground| if ground { 1 } else { 0 },
                        )
                    };
                    let motion = h.app.world().get::<MotionState>(h.fighter).unwrap();
                    frames.push(serde_json::json!({"time":now,"pose":pose,"p":h.position().to_array(),"wall":wall.cling_active,"wallTime":wall.remaining,"airborne":h.airborne(),"mode":mode,"support":air.ground_contact,"armed":air.air.cling_armed,"charge":air.jump_charge_active,"hold":air.jump_hold,"facing":motion.facing,"motionV":motion.velocity.to_array(),"v":[air.air.planar_x,air.air.vertical_speed,air.air.planar_z]}));
                }
                let max_error = anchors
                    .iter()
                    .map(|a| a["error"].as_f64().unwrap())
                    .fold(0.0_f64, f64::max);
                eprintln!("{file} fps={fps} corrected={corrected}: max={max_error:.3}, end={:?}, wall_entries={}", h.position(), transitions.iter().filter(|a| matches!(a["pose"].as_u64(),Some(161..=163))).count());
                runs.push(serde_json::json!({"file":file,"fps":fps,"corrected":corrected,"center":center,"start":start,"anchors":anchors,"transitions":transitions,"frames":frames}));
            }
        }
    }
    std::fs::write(
        output.join("production-paths.json"),
        serde_json::to_string_pretty(&runs).unwrap(),
    )
    .unwrap();
}

#[test]
fn production_mrc_jump_charge_starts_at_consumption_frame() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../");
    let mut doc = meteor_asset::mrc::decode_mrc(
        &std::fs::read(root.join("analysis/lightfoot-mrc-20261004/default1.mrc")).unwrap(),
    )
    .unwrap();

    doc.queue_b
        .retain(|e| e.code == 0x1200 && e.time >= 10.37 && e.time < 10.58);
    assert_eq!(doc.queue_b.len(), 2);
    doc.queue_a.clear();
    doc.queue_c.clear();
    doc.queue_d.clear();
    doc.queue_e.clear();
    let mut h = jump_pose_render_harness(0);
    h.app.insert_resource(crate::plugins::FrameClock::default());
    crate::plugins::replay_plugin::install_mrc_motion_audit(
        &mut h.app,
        doc,
        10.366_472,
        meteor_scene::stage_loader::parse_waypoint_graph("", [0.0; 3]),
        false,
    );
    for tick in 0..=14 {
        crate::plugins::advance_frame_clock(
            &mut h
                .app
                .world_mut()
                .resource_mut::<crate::plugins::FrameClock>(),
            1.0 / 120.0,
            120,
        );
        let before = h.position();
        h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / 120));
        let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
        if tick == 13 {
            assert!(
                air.jump_charge_active && !air.air.airborne,
                "native timer age is 0.108332, even though file-edge age is 0.110485"
            );
            assert!((air.jump_hold - 0.108_332).abs() < 1e-5);
        }
        if tick == 14 {
            assert!(!air.jump_charge_active && air.air.airborne);
            assert!(
                (h.position() - before).length() < 1e-5,
                "native auto-release returns before moving in its consumption tick"
            );
        }
    }
}
