#[test]
fn production_sn11_bridge_descent_uses_ordinary_contact_motion() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let gmdl = meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn11/sn11.gmb")).unwrap())
        .unwrap();
    let desc = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join("sn11/sn11.des")).unwrap(),
    );
    let cob =
        meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn11/sn11.cob")).unwrap()).unwrap();
    let wp = std::fs::read_to_string(root.join("Level/sn11.wp")).unwrap();
    let center = meteor_scene::stage_loader::get_stage_center("sn11", Some(&wp));
    let mesh = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), center);
    let keys = crate::app_state::Keymap::default();

    for fps in [30, 60, 89, 120] {
        let mut h = spawn_gameplay_harness_with_order(6, true).expect("original P1");
        h.replace_stage_mesh(mesh.clone());
        h.app
            .world_mut()
            .get_mut::<Transform>(h.fighter)
            .unwrap()
            .translation = Vec3::new(11.6, 49.8, 52.6);
        h.app
            .world_mut()
            .get_mut::<MotionState>(h.fighter)
            .unwrap()
            .facing = -6.32;
        h.app
            .world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = -6.32;
        h.press_binding(keys.move_up);
        let mut steep_contacts = 0;
        let mut contact_walk_frames = 0;
        let mut max_planar_speed = 0.0_f32;
        for tick in 0..fps * 5 {
            if tick == fps * 3 {
                h.release_binding(keys.move_up);
            }
            h.step_dt(1.0 / fps as f32);
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            let planar_speed = Vec2::new(air.air.planar_x, air.air.planar_z).length();
            max_planar_speed = max_planar_speed.max(planar_speed);
            if air.air.airborne && air.mover_vertical_contact && !air.ground_contact {
                steep_contacts += 1;

                assert_eq!(
                    air.recovery_ground_mode,
                    Some(true),
                    "fps={fps} tick={tick}"
                );
                assert!(air.air.vertical_speed.abs() < 1e-5, "fps={fps} tick={tick}");
                if tick >= fps && tick < fps * 3 && h.pose_id() == 144 {
                    contact_walk_frames += 1;
                }
            }
            if tick >= fps * 4 {
                assert_eq!(h.pose_id(), 0, "released input fps={fps} tick={tick}");

                assert!(
                    planar_speed < 2.0,
                    "released input fps={fps}: {planar_speed}"
                );
            }
        }
        assert!(
            steep_contacts > fps,
            "route missed bridge side at fps={fps}"
        );
        assert!(
            contact_walk_frames > fps,
            "contact retained air pose at fps={fps}"
        );

        assert!(max_planar_speed < 30.0, "fps={fps}: {max_planar_speed}");
        eprintln!(
            "sn11 fps={fps} peak_speed={max_planar_speed:.3} final={:?}",
            h.position()
        );
    }
}
