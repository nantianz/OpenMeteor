#[test]
fn production_jump_links_morph_in_animation_tick_with_entry_specific_end_gate() {
    for character_id in [0, 1] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let key = format!("p{character_id}");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], &key)
                .expect("original character assets");
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
                key.clone(),
                character,
            )])))
            .add_systems(
                Update,
                (resolve_completed_pose_links, advance_poses).chain(),
            );
        for source in 151..=160 {
            let (mut pose, mut blend) = (PoseRuntime::idle(), EntryBlend::default());
            let character = &app.world().resource::<CharacterAssetsSet>().0[&key];
            let target = character.poses[&source].link.expect("authored jump Link");
            character.apply_pose_with_blend(
                &mut pose,
                &mut blend,
                source,
                true,
                None,
                EntryBlendKind::Generic,
                false,
                false,
                None,
            );

            pose.frame = pose.end as f32;
            let instance = pose.pose_instance_id;
            blend.duration = 0.0;
            let entity = app
                .world_mut()
                .spawn((
                    Fighter {
                        character_id,
                        player_slot: 0,
                        weapon: DAGGER,
                        weapon_slots: [DAGGER, 0],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    },
                    pose,
                    blend,
                    SmState::default(),
                    CombatState::default(),
                    Health::default(),
                    Qi::default(),
                    FighterAir::default(),
                    WeaponForm::default(),
                ))
                .id();
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::ZERO);
            app.update();
            if matches!(source, 152 | 154 | 156 | 158) {
                assert_eq!(
                    app.world().get::<PoseRuntime>(entity).unwrap().pose_id,
                    source,
                    "mapped fall must not Link at equality: p{character_id}/{source}"
                );
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_millis(1));
                app.update();
            }
            let pose = app.world().get::<PoseRuntime>(entity).unwrap();
            assert_eq!(
                pose.pose_id, target,
                "same animation tick: p{character_id}/{source}"
            );
            assert_eq!(pose.pose_instance_id, instance);
            assert_eq!(pose.frame, pose.start as f32);
            assert!(!pose.link_past_end, "natural non-loop Link selects mode 1");
            assert_eq!(
                app.world().get::<EntryBlend>(entity).unwrap().duration,
                0.12
            );

            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(33));
            app.update();
            let pose = app.world().get::<PoseRuntime>(entity).unwrap();
            assert_eq!(pose.pose_id, target);
            assert_eq!(pose.frame, pose.start as f32);
            if matches!(target, 152 | 154 | 156 | 158) {
                app.world_mut()
                    .get_mut::<EntryBlend>(entity)
                    .unwrap()
                    .duration = 0.0;
                let mut pose = app.world_mut().get_mut::<PoseRuntime>(entity).unwrap();
                pose.frame = pose.end as f32;
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::ZERO);
                app.update();
                assert_eq!(
                    app.world().get::<PoseRuntime>(entity).unwrap().pose_id,
                    180,
                    "naturally linked fall uses inclusive End, unlike mapped fall"
                );
            }
            app.world_mut().despawn(entity);
        }
    }
}

fn jump_pose_render_harness(character_id: u8) -> GameplayHarness {
    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).expect("original P1 assets");
    if character_id != 1 {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let key = format!("p{character_id}");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], &key)
                .expect("original character assets");
        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, 0, true);
        h.app
            .world_mut()
            .resource_mut::<CharacterAssetsSet>()
            .0
            .insert(key, character);
        h.app
            .world_mut()
            .get_mut::<Fighter>(h.fighter)
            .unwrap()
            .character_id = character_id;
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert((pose, EntryBlend::default()));
    }
    h.app
        .world_mut()
        .entity_mut(h.fighter)
        .insert(crate::components::fighter::VisualRootPin::default());
    crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
    let schedule = crate::plugins::fighter_frame::schedule(&h.app);
    h.app.add_systems(
        schedule,
        crate::plugins::character_plugin::drive_fighter_animation_for_test
            .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
    );
    h.steps(8);
    h
}

#[test]
fn production_jump_four_directions_keep_morph_while_physics_takes_off() {
    for (character_id, fps) in [0, 1]
        .into_iter()
        .flat_map(|id| [30, 60, 120, 165].map(|fps| (id, fps)))
    {
        for (direction, expected) in [
            (KeyCode::KeyW, 151),
            (KeyCode::KeyD, 153),
            (KeyCode::KeyA, 155),
            (KeyCode::KeyS, 157),
        ] {
            for long in [false, true] {
                let mut h = jump_pose_render_harness(character_id);
                let dt = 1.0 / fps as f32;
                h.press_key(direction);
                h.step_dt(dt);
                let jump = h.app.world().resource::<crate::app_state::Keymap>().jump;
                h.press_binding(jump);
                h.step_dt(dt);
                if !long {
                    h.release_binding(jump);
                }
                for _ in 0..fps {
                    if h.airborne() {
                        break;
                    }
                    h.step_dt(dt);
                }
                assert!(
                    h.airborne(),
                    "fps={fps} direction={direction:?} long={long}"
                );
                assert_eq!(h.pose_id(), expected);
                let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
                assert_eq!(blend.duration, 0.12);
                assert_eq!(pose.frame, pose.start as f32);
                assert!(
                    !blend.from_quats.is_empty(),
                    "real source skeleton captured"
                );
                assert!(
                    h.app
                        .world()
                        .get::<FighterAir>(h.fighter)
                        .unwrap()
                        .air
                        .vertical_speed
                        > 0.0,
                    "takeoff impulse must be installed during morph: p{character_id} fps={fps}"
                );
                h.release_binding(jump);
                let before = h.position();
                h.step_dt(dt);
                let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                assert_eq!(pose.frame, pose.start as f32, "target parked during morph");
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                if air.mover_vertical_contact {
                    assert!(air.ground_contact);
                    assert!(
                        air.air.vertical_speed * dt <= meteor_physics::collision::FOOT_PROBE_DOWN
                    );
                    assert_eq!(h.position().y, 0.0);
                } else {
                    assert!(
                        h.position().y > before.y,
                        "p{character_id} fps={fps} dir={direction:?} long={long}"
                    );
                }
                for _ in 0..fps * 3 {
                    h.step_dt(dt);
                }
                assert!(!h.airborne(), "jump must still land");
                assert!(h.position().y.abs() < 0.05);
            }
        }
    }
}

#[test]
fn production_clock_keeps_original_short_ascent_floor_probe() {
    for (fps, physics_hz) in [(120, 0), (150, 150), (240, 0)] {
        for held in [false, true] {
            let mut h = jump_pose_render_harness(1);
            {
                let mut settings = h
                    .app
                    .world_mut()
                    .resource_mut::<crate::app_state::Settings>();
                if physics_hz == 0 {
                    settings.variable_physics = true;
                } else {
                    settings.physics_hz = physics_hz;
                }
            }
            crate::plugins::install_frame_clock(&mut h.app);
            let dt = 1.0 / fps as f32;
            let jump = h.app.world().resource::<crate::app_state::Keymap>().jump;
            let floor = h.position().y;
            h.press_binding(jump);
            h.step_dt(dt);
            if !held {
                h.release_binding(jump);
            }
            let mut peak = floor;
            let mut upward_probe = false;
            let mut took_off = false;
            for _ in 0..fps {
                h.step_dt(dt);
                let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                took_off |= air.air.airborne && air.air.vertical_speed > 0.0;
                upward_probe |= air.air.vertical_speed > 0.0 && air.mover_vertical_contact;
                peak = peak.max(h.position().y);
            }
            assert!(
                took_off,
                "{fps} Hz, held={held}: jump impulse must be installed"
            );
            if fps == 240 || (physics_hz == 150 && !held) {
                assert!(
                    upward_probe && (peak - floor).abs() < 0.01,
                    "{fps} Hz, held={held}: preserve the original floor snap, peak {peak}"
                );
            } else {
                assert!(
                    peak - floor > 0.24,
                    "{fps} Hz, held={held}: rise above the probe"
                );
            }
        }
    }
}
#[test]
fn production_jump_wall_entries_keep_source_morph_and_kick_mapping() {
    for fps in [30, 60, 120] {
        for (side, cling, kick) in [(0, 161, 157), (1, 162, 159), (-1, 163, 160)] {
            let mut h = jump_pose_render_harness(1);
            let dt = 1.0 / fps as f32;
            h.replace_stage_mesh(if side == 0 {
                harness_ground_with_front_wall(2.0)
            } else {
                harness_ground_with_side_wall(-2.0 * side as f32)
            });
            let keys = crate::app_state::Keymap::default();
            h.press_binding(keys.move_up);
            if side != 0 {
                h.press_binding(if side > 0 {
                    keys.move_right
                } else {
                    keys.move_left
                });
            }
            h.press_binding(keys.jump);
            h.step_dt(dt);
            h.release_binding(keys.jump);
            for _ in 0..fps {
                h.step_dt(dt);
                if h.app
                    .world()
                    .get::<FighterWall>(h.fighter)
                    .unwrap()
                    .cling_active
                {
                    break;
                }
            }
            assert_eq!(h.pose_id(), cling, "fps={fps} side={side}");
            let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
            assert!(
                !air.late_visual_pose_snap,
                "never overwrite the source with the target"
            );
            let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
            assert_eq!(blend.pose_id, cling);
            assert_eq!(blend.duration, 0.12);
            h.step_dt(dt);
            assert!(!h
                .app
                .world()
                .get::<EntryBlend>(h.fighter)
                .unwrap()
                .from_quats
                .is_empty());
            h.press_binding(keys.jump);
            h.step_dt(dt);
            h.release_binding(keys.jump);
            h.step_dt(dt);
            assert_eq!(h.pose_id(), kick);
            let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
            assert_eq!(blend.pose_id, kick);
            assert_eq!(blend.duration, 0.12);
            let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
            assert_eq!(pose.frame, pose.start as f32);
        }
    }
}

#[test]
fn production_sn13_captured_slope_jump_keeps_model_root_neutral_through_landing() {
    use crate::plugins::character_plugin::{FighterDummy, FighterModel};
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("slope_jump_capture.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let model =
        meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn13/sn13.gmb")).unwrap())
            .unwrap();
    let desc = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join("sn13/sn13.des")).unwrap(),
    );
    let cob =
        meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn13/sn13.cob")).unwrap()).unwrap();
    let wp = std::fs::read_to_string(root.join("Level/sn13.wp")).unwrap();
    let mesh = StageMeshCollider::from_gmdl(
        &model,
        &desc,
        Some(&cob.face_ids),
        meteor_scene::stage_loader::get_stage_center("sn13", Some(&wp)),
    );
    for fps in [30, 60, 120, 165] {
        for entry in fixture["entries"].as_array().unwrap() {
            let mut h =
                jump_pose_render_harness(entry["fighter"]["character"].as_u64().unwrap() as u8);
            h.app.add_plugins(bevy::transform::TransformPlugin);
            h.replace_stage_mesh(mesh.clone());
            let vector = |v: &serde_json::Value| {
                Vec3::new(
                    v[0].as_f64().unwrap() as f32,
                    v[1].as_f64().unwrap() as f32,
                    v[2].as_f64().unwrap() as f32,
                )
            };
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = vector(&entry["actor"]["local"]["position"]);
            h.app
                .world_mut()
                .get_mut::<SmState>(h.fighter)
                .unwrap()
                .current = FighterState::Jump;
            {
                let mut motion = h.app.world_mut().get_mut::<MotionState>(h.fighter).unwrap();
                motion.movement_mode = meteor_core::MovementMode::Airborne;
                motion.facing = entry["motion"]["facing"].as_f64().unwrap() as f32;
                motion.velocity = vector(&entry["motion"]["velocity"]);
            }
            {
                let mut wall = h.app.world_mut().get_mut::<FighterWall>(h.fighter).unwrap();
                wall.kick_pose = entry["wall"]["kickPose"].as_u64().unwrap() as u16;
                wall.cling_pose = entry["wall"]["pose"].as_u64().unwrap() as u16;
                wall.wall_normal = vector(&entry["wall"]["normal"]);
            }
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut pose, 157, true);
            pose.frame = entry["pose"]["frame"].as_f64().unwrap() as f32;
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert((pose, EntryBlend::default()));
            {
                let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                let a = &entry["air"];
                air.air.airborne = true;
                air.air.air_time = a["time"].as_f64().unwrap() as f32;
                air.air.vertical_speed = a["verticalSpeed"].as_f64().unwrap() as f32;
                air.air.planar_x = a["planar"][0].as_f64().unwrap() as f32;
                air.air.planar_z = a["planar"][1].as_f64().unwrap() as f32;
                air.air.vertical_pose_id = 157;
                air.air.prev_root_z = Some(a["launch"]["prevRootZ"].as_f64().unwrap() as f32);
                air.ground_y = a["groundY"].as_f64().unwrap() as f32;
                air.ground_contact = a["groundContact"].as_bool().unwrap();
                air.mover_vertical_contact = a["verticalContact"].as_bool().unwrap();
                air.carry_no_support = a["noSupport"].as_bool().unwrap();
                air.ground_entry_blend_pending = a["groundBlendPending"].as_bool().unwrap();
                air.peak_lift = a["launch"]["peakLift"].as_f64().unwrap() as f32;
                air.motion_root_pin = Some([0.0, 0.0, 0.0035665035]);
                air.motion_root_pin_pose_id = 157;
                air.pin_root_z = 0.0035665035;
                air.jump_prev_pose_id = 157;
            }
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = entry["motion"]["facing"].as_f64().unwrap() as f32;
            if !entry["keys"].as_array().unwrap().is_empty() {
                h.press_key(KeyCode::KeyW);
            }
            let model_entity = h
                .app
                .world_mut()
                .query::<(Entity, &FighterModel)>()
                .iter(h.app.world())
                .find(|(_, m)| m.fighter == h.fighter)
                .unwrap()
                .0;
            let base = h
                .app
                .world_mut()
                .query::<(Entity, &FighterDummy)>()
                .iter(h.app.world())
                .find(|(_, d)| d.model == model_entity && d.name == "d_base")
                .unwrap()
                .0;
            let mut landed_in_jump = false;
            for tick in 0..fps {
                h.step_dt(1.0 / fps as f32);
                landed_in_jump |= !h.airborne() && h.pose_id() == 157;
                let rendered = h.app.world().get::<Transform>(base).unwrap().translation;
                assert!(rendered.length()<1e-3,"capture{} fps={fps} tick={tick} pose={} air={}: absolute d_base leaked into model {rendered:?}",entry["tick"],h.pose_id(),h.airborne());
                assert!(h
                    .app
                    .world()
                    .get::<GlobalTransform>(base)
                    .unwrap()
                    .to_matrix()
                    .is_finite());
            }
            assert!(
                landed_in_jump,
                "capture{} fps={fps}: must exercise grounded Pose157 tail",
                entry["tick"]
            );
            assert!(!h.airborne());
        }
    }
}
