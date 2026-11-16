#[test]
#[ignore = "COMBAT-075 original SN13 contact audit; writes analysis results"]
fn production_falling_hit_sn13_contact_audit() {
    use crate::components::fighter::VisualRootPin;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mesh = sn13_test_mesh();
    let mut cases = Vec::new();
    for fps in [30, 60, 120] {
        for (source, position, speed) in [
            (154, Vec3::new(-104.11785, 4.876955, 76.775986), -6.899291),
            (152, Vec3::new(69.53656, 2.206058, -50.233547), -24.748028),
        ] {
            for target in [93, 94, 100, 106, 109, 115] {
                for repeat in [false, true] {
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
                    h.replace_stage_mesh(mesh.clone());
                    let dt = 1.0 / fps as f32;
                    h.app.insert_resource(crate::plugins::FrameClock {
                        sim_dt: dt,
                        ..default()
                    });
                    let mut pose = PoseRuntime::idle();
                    h.app.world().resource::<CharacterAssetsSet>().0["p0"]
                        .apply_pose(&mut pose, source, true);
                    h.app.world_mut().entity_mut(h.fighter).insert(pose);
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(h.fighter)
                        .unwrap()
                        .translation = position;
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
                        air.air.start_fall(0.0, 0.0);
                        air.air.vertical_speed = speed;
                        air.ground_contact = false;
                    }
                    {
                        let mut motion =
                            h.app.world_mut().get_mut::<MotionState>(h.fighter).unwrap();
                        motion.velocity = Vec3::new(0.0, speed, 0.0);
                        motion.movement_mode = meteor_core::MovementMode::Airborne;
                    }
                    crate::plugins::character_plugin::drive_fighter_animation_for_test(
                        h.app.world_mut(),
                    );
                    falling_hit_audit_inject(&mut h, target);
                    let mut history = Vec::new();
                    let mut old = None;
                    for tick in 0..fps * 6 {
                        if repeat && tick == fps / 4 {
                            falling_hit_audit_inject(&mut h, target);
                        }
                        h.step_dt(dt);
                        let pose = h.pose_id();
                        let reacting = h
                            .app
                            .world()
                            .get::<FighterHitReaction>(h.fighter)
                            .unwrap()
                            .current
                            .is_some();
                        assert!(!reacting || !matches!(pose, 152 | 154 | 156 | 158 | 180),
                            "SN13 falling pose overwrote response: fps={fps} source={source} target={target} repeat={repeat} tick={tick}");
                        let key = (pose, h.state(), h.airborne());
                        if old != Some(key) {
                            history.push(serde_json::json!({"tick":tick,"pose":pose,"state":format!("{:?}",h.state()),
                                "air":h.airborne(),"position":h.position().to_array()}));
                            old = Some(key);
                        }
                    }
                    assert!(matches!(h.state(), FighterState::Name | FighterState::Down),
                        "SN13 response did not release: fps={fps} source={source} target={target} repeat={repeat} state={:?} pose={} air={} position={:?}",h.state(),h.pose_id(),h.airborne(),h.position());
                    cases.push(
                        serde_json::json!({"fps":fps,"source":source,"start":position.to_array(),
                        "speed":speed,"target":target,"repeat":repeat,"history":history}),
                    );
                }
            }
        }
    }
    let output = falling_hit_audit_output(&root, "sn13-matrix.json");
    std::fs::write(output, serde_json::to_string_pretty(&cases).unwrap()).unwrap();
    eprintln!(
        "SN13 falling-hit contact audit: {} cases passed",
        cases.len()
    );
}
