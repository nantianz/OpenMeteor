#[test]
fn owned_spear_launcher_reaction_is_shared_by_player_npc_and_passive_dummy() {
    for fps in [30, 60, 120] {
        let mut baseline = None;
        for identity in ["player", "npc", "dummy"] {
            let mut h = recovery_timing_harness(true);
            h.app
                .insert_resource(State::new(AppState::Playing))
                .add_plugins(crate::plugins::CharacterContactPlugin);
            let schedule = crate::plugins::fighter_frame::schedule(&h.app);
            h.app.add_systems(
                schedule,
                hit_reaction_root_motion
                    .after(crate::plugins::movement_plugin::attack_root_motion)
                    .in_set(crate::plugins::FighterFrameSet::RootMotion),
            );
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert(crate::components::fighter::VisualRootPin::default());
            if identity != "player" {
                h.app
                    .world_mut()
                    .entity_mut(h.fighter)
                    .remove::<LocalFighter>()
                    .insert((
                        NpcFighter,
                        NpcPerformance {
                            pause_remaining: if identity == "dummy" {
                                f32::INFINITY
                            } else {
                                0.0
                            },
                            ..default()
                        },
                    ));
            }

            let candidate_fighter = h.app.world().get::<Fighter>(h.fighter).unwrap().clone();
            h.app.world_mut().spawn((
                candidate_fighter,
                Transform::from_xyz(1.2, 0.0, 0.0),
                Health::default(),
                PoseRuntime::idle(),
                SmState::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                RemoteFighter {
                    player_id: "peer".into(),
                },
            ));
            let attack = h.app.world().resource::<CharacterAssetsSet>().0["p1"].poses[&289].attacks
                [0]
            .clone();
            assert_eq!(attack.target_pose_front, Some(100));
            install_timing_reaction(&mut h, 100, HitReactKind::BeHit, attack.target_value);
            h.app
                .world_mut()
                .get_mut::<FighterHitReaction>(h.fighter)
                .unwrap()
                .current
                .as_mut()
                .unwrap()
                .motion_scale = attack.target_move;
            let mut samples = Vec::new();
            for _ in 0..fps * 3 {
                h.step_dt(1.0 / fps as f32);
                let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                samples.push((pose.pose_id, pose.frame, h.position()));
                if h.app
                    .world()
                    .get::<FighterHitReaction>(h.fighter)
                    .unwrap()
                    .current
                    .is_none()
                {
                    break;
                }
            }
            assert!(samples.iter().any(|(pose, _, _)| *pose == 100));
            assert!(samples.iter().any(|(pose, _, _)| *pose == 118));
            if let Some(expected) = &baseline {
                assert_eq!(&samples, expected,
                    "{identity} must share the launcher clock, skeleton and both movers at {fps} FPS");
            } else {
                baseline = Some(samples);
            }
        }
    }
}

#[test]
fn spear_behit_entry_preserves_main_vertical_speed_and_settles_near_ground() {
    for fps in [30, 60, 120] {
        let dt = 1.0 / fps as f32;
        for identity in ["player", "npc", "dummy"] {
            for (height, incoming) in [(20.0, -3.0), (20.0, 3.0), (0.003, 0.0)] {
                let mut h = recovery_timing_harness(true);
                h.app.insert_resource(State::new(AppState::Playing));
                let schedule = crate::plugins::fighter_frame::schedule(&h.app);
                h.app.add_systems(
                    schedule,
                    hit_reaction_root_motion
                        .after(crate::plugins::movement_plugin::attack_root_motion)
                        .in_set(crate::plugins::FighterFrameSet::RootMotion),
                );
                if identity != "player" {
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .remove::<LocalFighter>()
                        .insert((
                            NpcFighter,
                            NpcPerformance {
                                pause_remaining: if identity == "dummy" {
                                    f32::INFINITY
                                } else {
                                    0.0
                                },
                                ..default()
                            },
                        ));
                }
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation
                    .y = height;
                h.app
                    .world_mut()
                    .get_mut::<MotionState>(h.fighter)
                    .unwrap()
                    .velocity = Vec3::Y * incoming;
                h.app
                    .world_mut()
                    .get_mut::<FighterAir>(h.fighter)
                    .unwrap()
                    .air
                    .airborne = height > 1.0;
                install_timing_reaction(&mut h, 100, HitReactKind::BeHit, 0.5);
                h.app
                    .world_mut()
                    .get_mut::<FighterHitReaction>(h.fighter)
                    .unwrap()
                    .bank_be_hit_carry(Vec3::Y * incoming, identity == "player");
                h.step_dt(dt);
                assert_eq!(h.pose_id(), 100);
                let frame = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                assert_eq!(
                    frame.frame, frame.start as f32,
                    "entry fade must hold the authored root"
                );
                let expected = if height > 1.0 {
                    height + (incoming - 105.84 * dt) * dt
                } else {
                    0.0
                };
                assert!((h.position().y - expected).abs() < 2e-5,
                    "{identity} {fps}Hz height={height} incoming={incoming}: {:?}, original expected {expected}", h.position());
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct SpearCloseGeometry(Vec<serde_json::Value>);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct SpearClosePair {
    attacker: Entity,
    defender: Entity,
}

#[cfg(not(target_arch = "wasm32"))]
fn export_spear_close_geometry(
    active: Option<Res<super::super::fighter_frame::ActiveFighter>>,
    mut audit: ResMut<SpearCloseGeometry>,
    pair: Res<SpearClosePair>,
    geometry: HitGeometryQueries,
    assets: Res<CharacterAssetsSet>,
    fighters: Query<(&Fighter, &WeaponForm, &PoseRuntime, &EntryBlend)>,
) {
    if !super::super::fighter_frame::accepts(active.as_deref(), pair.attacker) {
        return;
    }
    let Ok((attacker, form, pose, blend)) = fighters.get(pair.attacker) else {
        return;
    };
    if !((pose.pose_id == 285 && (2842.0..=2848.0).contains(&pose.frame))
        || (pose.pose_id == 289 && (3089.0..=3092.0).contains(&pose.frame))
        || (pose.pose_id == 291 && (3240.0..=3245.0).contains(&pose.frame)))
    {
        return;
    }
    let asset = &assets.0[&format!("p{}", attacker.character_id)];
    let Some((index, window)) = sampled_attack_window(asset, pose, blend) else {
        return;
    };
    let convert = (Mat4::from_scale(Vec3::splat(meteor_core::MODEL_SCALE))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2))
    .inverse();
    let vec = |v: meteor_core::Vec3| Vec3::new(v.x, v.y, v.z);
    let obb = |b: &Obb| {
        serde_json::json!({
            "center":convert.transform_point3(vec(b.center)).to_array(),
            "axes":b.axes.map(|axis|convert.transform_vector3(vec(axis)).normalize().to_array()),
            "half":(vec(b.half_size)/meteor_core::MODEL_SCALE).to_array(),
        })
    };
    let skeleton = |entity: Entity| {
        let model = find_fighter_model(entity, &geometry.models).unwrap();
        let mut bones = Vec::new();
        for (id, bone, propagated) in geometry
            .bones
            .iter_many(model_bones(model, &geometry.skeletons))
        {
            let animated = current_global_transform(id, propagated, &geometry.transforms);
            let bound = asset
                .bone_local_bounds
                .get(bone.skin_index)
                .and_then(Option::as_ref)
                .and_then(|bounds| {
                    bone_world_obb(&bone_world_matrix(asset, bone, &animated).unwrap(), bounds)
                });
            bones.push(serde_json::json!({"index":bone.skin_index,"name":bone.name,
                "animated":(convert*animated.to_matrix()).to_cols_array(),
                "inverse_bind":asset.skeleton.skin_world_bind_inverse[bone.skin_index].m,
                "obb":bound.as_ref().map(&obb)}));
        }
        bones.sort_by_key(|bone| bone["index"].as_u64().unwrap());
        bones
    };
    let source_dummies = {
        let model = find_fighter_model(pair.attacker, &geometry.models).unwrap();
        geometry
            .dummies
            .iter_many(model_dummies(model, &geometry.skeletons))
            .filter(|(_, dummy, _)| matches!(dummy.name.as_str(), "d_base" | "d_wpnR" | "d_wpnRP"))
            .map(|(id, dummy, propagated)| {
                let animated = current_global_transform(id, propagated, &geometry.transforms);
                serde_json::json!({"name":dummy.name,"index":dummy.dummy_index,
                    "animated":(convert*animated.to_matrix()).to_cols_array()})
            })
            .collect::<Vec<_>>()
    };
    let source_model_matrix = {
        let model = find_fighter_model(pair.attacker, &geometry.models).unwrap();
        let (_, _, propagated) = geometry.model_worlds.get(model).unwrap();
        (convert * current_global_transform(model, propagated, &geometry.transforms).to_matrix())
            .to_cols_array()
    };
    let defender_pose = fighters.get(pair.defender).unwrap().2;
    let attacks = geometry.attack_obbs(
        pair.attacker,
        attacker,
        form,
        pose.pose_id,
        asset,
        &window.bones,
    );
    let hurts = geometry.hurt_obbs(pair.defender, asset);
    let sample = audit.0.len();
    audit.0.push(
        serde_json::json!({"sample":sample,"attack_pose":pose.pose_id,
        "attack_index":index,"frame":pose.frame,"character":attacker.character_id,"target_character":attacker.character_id,
        "target_pose":defender_pose.pose_id,"target_frame":defender_pose.frame,
        "source_bones":skeleton(pair.attacker),"target_bones":skeleton(pair.defender),
        "source_dummies":source_dummies,"source_model_matrix":source_model_matrix,
        "attack_names":window.bones,"attack_obbs":attacks.iter().map(&obb).collect::<Vec<_>>(),"effects":[],
        "rust_body_hit":attacks.iter().any(|a|hurts.iter().any(|b|obbs_intersect(a,b))),
        "rust_effect_hit":false}),
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "writes close-range spear chain diagnostics under analysis"]
fn diagnose_mrc_spear_close_third_hit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let audit_output = std::env::var_os("METEOR_SPEAR_OUTPUT_DIR");
    let phase_audit = audit_output.is_some();
    let output = audit_output
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("analysis/mrc-moves-20260928"));
    std::fs::create_dir_all(&output).unwrap();
    let doc = meteor_asset::mrc::decode_mrc(
        &std::fs::read(root.join("mrcs/tests/长枪三连.mrc")).unwrap(),
    )
    .unwrap();
    let input: Vec<_> = doc
        .queue_b
        .iter()
        .filter(|event| (10.5..14.2).contains(&event.time))
        .cloned()
        .collect();
    let stage_only = std::env::var_os("METEOR_SPEAR_STAGE_ONLY").is_some();
    let oracle_only = std::env::var_os("METEOR_SPEAR_ORACLE_ONLY").is_some();
    let serial = std::env::var_os("METEOR_SPEAR_SERIAL").is_some();
    let seed_live289 = std::env::var_os("METEOR_SPEAR_SEED_LIVE_289").is_some();
    let compare_immobile =
        seed_live289 || std::env::var_os("METEOR_SPEAR_COMPARE_IMMOBILE").is_some();
    let original_target_yaw = std::env::var_os("METEOR_SPEAR_ORIGINAL_YAW").is_some();
    let mut cases = Vec::new();
    let distances: &[f32] = if seed_live289 {
        &[1.8]
    } else if compare_immobile {
        &[0.6, 0.8, 1.0, 1.8]
    } else {
        &[1.8, 2.6, 4.0]
    };
    for fps in [60_u32, 120] {
        for &distance in distances {
            for lateral in [-1.0_f32, -0.5, 0.0, 0.5, 1.0] {
                for (contact, lock_updates, target_remote, moving_npc, sn06, frozen_npc) in [
                    (false, false, false, false, false, false),
                    (true, false, false, false, false, false),
                    (true, true, false, false, false, false),
                    (true, false, true, false, false, false),
                    (true, true, false, true, false, false),
                    (true, true, false, false, true, false),
                    (true, true, false, false, true, true),
                    (true, true, false, true, true, false),
                    (true, true, false, true, true, true),
                ] {
                    if frozen_npc && !compare_immobile {
                        continue;
                    }
                    if sn06 && !moving_npc && !compare_immobile {
                        continue;
                    }
                    if compare_immobile && !(fps == 60 && lateral == -0.5 && sn06) {
                        continue;
                    }
                    if lateral != 0.0 && (!contact || target_remote) {
                        continue;
                    }
                    if lateral < 0.0 && !lock_updates {
                        continue;
                    }
                    if sn06 && ((!compare_immobile && distance != 1.8) || lateral.abs() != 0.5) {
                        continue;
                    }
                    if stage_only && !sn06 {
                        continue;
                    }
                    if original_target_yaw && !sn06 {
                        continue;
                    }
                    if oracle_only
                        && !(fps == 60
                            && distance == 1.8
                            && lateral == 0.5
                            && contact
                            && !lock_updates
                            && !target_remote
                            && !moving_npc
                            && !sn06)
                    {
                        continue;
                    }
                    let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(7, serial);
                    let actor_schedule = crate::plugins::fighter_frame::schedule(&h.app);
                    h.app.insert_resource(NetState::default());
                    h.app.insert_resource(State::new(AppState::Playing));
                    h.app.world_mut().resource_mut::<Settings>().auto_camera = lock_updates;
                    let models: Vec<_> = h
                        .app
                        .world_mut()
                        .query::<(Entity, &FighterModel)>()
                        .iter(h.app.world())
                        .map(|(entity, _)| entity)
                        .collect();
                    for model in models {
                        h.app.world_mut().despawn(model);
                    }
                    let p0 = crate::plugins::character_plugin::load_character_asset_for_test(
                        std::slice::from_ref(&root.join("origin_res")),
                        "p0",
                    )
                    .unwrap();
                    h.app
                        .world_mut()
                        .resource_mut::<CharacterAssetsSet>()
                        .0
                        .insert("p0".into(), p0);
                    for entity in [h.fighter, target] {
                        h.app
                            .world_mut()
                            .get_mut::<Fighter>(entity)
                            .unwrap()
                            .character_id = 0;
                        h.app
                            .world_mut()
                            .entity_mut(entity)
                            .insert(crate::components::fighter::VisualRootPin::default());
                        crate::plugins::character_plugin::spawn_fighter_model_for_test(
                            h.app.world_mut(),
                            entity,
                        );
                    }
                    if contact {
                        h.app.add_plugins(
                            crate::plugins::character_contact_plugin::CharacterContactPlugin,
                        );
                        if phase_audit {
                            let mut frame = h.app.world_mut().resource_mut::<
                                crate::plugins::character_contact_plugin::CharacterContactFrame>();
                            frame.audit_record_moves(h.fighter);
                            frame.audit_record_moves(target);
                        }
                    }
                    h.app
                        .world_mut()
                        .entity_mut(h.fighter)
                        .insert(HeadTrackState::default());
                    configure_lock_updates(&mut h.app);
                    if target_remote {
                        h.app
                            .world_mut()
                            .entity_mut(target)
                            .remove::<NpcFighter>()
                            .insert(crate::plugins::network_plugin::RemoteFighter {
                                player_id: "stock-2".into(),
                            });
                    }
                    if moving_npc {
                        h.app.world_mut().entity_mut(target).insert((
                            NpcFighter,
                            MissionNpc {
                                id: 1,
                                name: "SpearFollow".into(),
                                team: 2,
                                in_scene_roster: false,
                                view: crate::plugins::ai_plugin::STOCK_NPC_VIEW,
                                think: crate::plugins::ai_plugin::STOCK_NPC_THINK,
                                forced_behavior: Some(meteor_ai::behavior::BehaviorMode::Follow),
                                is_vip: false,
                                attack_target_slot: None,
                                behavior_args: vec![meteor_scene::pst_runtime::PstValue::Num(0.0)],
                                attack_target_remaining: 0,
                            },
                            AiIntent::default(),
                            crate::plugins::ai_plugin::NpcPatrol::default(),
                            NpcPerformance::default(),
                            NpcCombatProfile::web_debug(),
                            RangedFireGate::default(),
                            crate::components::fighter::FighterStatusEffects::default(),
                        ));
                    }
                    if frozen_npc {
                        h.app.world_mut().entity_mut(target).insert(NpcPerformance {
                            pause_remaining: f32::INFINITY,
                            ..Default::default()
                        });
                    }
                    let source_origin = if sn06 {
                        let center = meteor_scene::stage_loader::get_stage_center("sn06", None);
                        let stage = root.join("origin_res/sn06");
                        let mesh = meteor_asset::gmdl::decode_gmdl(
                            &std::fs::read(stage.join("sn06.gmb")).unwrap(),
                        )
                        .unwrap();
                        let description = meteor_asset::des::parse_stage_description(
                            &crate::text_codec::decode_text(
                                &std::fs::read(stage.join("sn06.des")).unwrap(),
                            ),
                        );
                        let collision = meteor_asset::cob::parse_cob(
                            &std::fs::read(stage.join("sn06.cob")).unwrap(),
                        )
                        .unwrap();
                        h.replace_stage_mesh(StageMeshCollider::from_gmdl(
                            &mesh,
                            &description,
                            Some(&collision.face_ids),
                            center,
                        ));
                        let anchor = [42.9795_f32, 35.520275, -23.650707];
                        Vec3::new(
                            (anchor[0] - center[0]) * meteor_core::MODEL_SCALE,
                            (anchor[2] - center[2]) * meteor_core::MODEL_SCALE,
                            (center[1] - anchor[1]) * meteor_core::MODEL_SCALE,
                        )
                    } else {
                        Vec3::ZERO
                    };
                    if seed_live289 {
                        use meteor_physics::collision::MeshCollider;
                        let stage = h.app.world().resource::<StageCollider>();
                        let mesh = stage.mesh.as_ref().unwrap();
                        let ray =
                            mesh.sample_ground_hit(51.9331207, -33.5220222, -64.0, -1.2657094);
                        let mut mover = meteor_physics::collision::CollisionSystem::new(
                            crate::plugins::movement_plugin::COLLIDER_RADIUS,
                            crate::plugins::movement_plugin::COLLIDER_HEIGHT,
                        );
                        mover.set_mesh_collider(mesh);
                        let support = mover.sample_ground_hit(51.9331207, -33.5220222, -1.2657094);
                        eprintln!("seeded live 289 stage ray={ray:?} support={support:?}");
                    }
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(h.fighter)
                        .unwrap()
                        .translation = source_origin;
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(target)
                        .unwrap()
                        .translation = source_origin + Vec3::new(lateral, 0.0, distance);
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .facing = 0.0;
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(target)
                        .unwrap()
                        .facing = if original_target_yaw {
                        let angle = 2.5_f32;
                        angle.cos().atan2(-angle.sin())
                    } else {
                        std::f32::consts::PI
                    };
                    h.app
                        .world_mut()
                        .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                        .yaw = 0.0;
                    h.app.add_systems(
                        actor_schedule,
                        crate::plugins::character_plugin::drive_fighter_animation_for_test
                            .after(harness_retire_entry_blend)
                            .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                    );
                    h.app.add_systems(
                        Update,
                        tick_target_hit_clocks
                            .in_set(crate::plugins::FighterFrameSet::PoseDecision),
                    );
                    h.app.add_systems(
                        actor_schedule,
                        attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
                    );
                    let oracle_case = fps == 60
                        && distance == 1.8
                        && lateral == 0.5
                        && contact
                        && !lock_updates
                        && !target_remote
                        && !sn06;
                    let stage_oracle = sn06 && fps == 60 && lateral == -0.5 && !compare_immobile;
                    if oracle_case || stage_oracle {
                        h.app
                            .insert_resource(SpearClosePair {
                                attacker: h.fighter,
                                defender: target,
                            })
                            .init_resource::<SpearCloseGeometry>()
                            .add_systems(
                                actor_schedule,
                                export_spear_close_geometry
                                    .in_set(crate::plugins::FighterFrameSet::HitJudge)
                                    .before(attack_windows),
                            );
                    }
                    h.app.add_systems(
                        actor_schedule,
                        hit_reaction_root_motion
                            .after(crate::plugins::movement_plugin::attack_root_motion)
                            .in_set(crate::plugins::FighterFrameSet::RootMotion),
                    );
                    let keys = crate::app_state::Keymap::default();
                    let mut next = 0;
                    let mut previous_hp = h.app.world().get::<Health>(target).unwrap().current;
                    let mut hits = Vec::new();
                    let mut entries = Vec::new();
                    let mut previous_pose = h.pose_id();
                    let mut samples = Vec::new();
                    let mut seeded = false;
                    for tick in 0..(3.7 * fps as f32) as usize {
                        let time = 10.5 + (tick + 1) as f32 / fps as f32;
                        while next < input.len() && input[next].time <= time {
                            let event = &input[next];
                            let binding = match event.code {
                                0x57 => keys.move_up,
                                0x53 => keys.move_down,
                                0x1000 => keys.attack,
                                0x1200 => keys.jump,
                                code => panic!("unexpected spear MRC input {code:x}"),
                            };
                            if event.value != 0 {
                                h.press_binding(binding);
                            } else {
                                h.release_binding(binding);
                            }
                            next += 1;
                        }
                        if seed_live289 && !seeded {
                            let source_pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                            if source_pose.pose_id == 289 && source_pose.frame >= 3088.5 {
                                h.app
                                    .world_mut()
                                    .get_mut::<Transform>(h.fighter)
                                    .unwrap()
                                    .translation = Vec3::new(51.9331207, -1.2657094, -33.5220222);
                                h.app
                                    .world_mut()
                                    .get_mut::<Transform>(target)
                                    .unwrap()
                                    .translation = Vec3::new(51.3300209, -1.2657123, -33.0614395);
                                h.app
                                    .world_mut()
                                    .get_mut::<MotionState>(h.fighter)
                                    .unwrap()
                                    .facing = -0.9082246;
                                h.app
                                    .world_mut()
                                    .get_mut::<MotionState>(target)
                                    .unwrap()
                                    .facing = -20.440937;
                                {
                                    let mut target_pose =
                                        h.app.world().get::<PoseRuntime>(target).unwrap().clone();
                                    h.app.world().resource::<CharacterAssetsSet>().0["p0"]
                                        .apply_pose(&mut target_pose, 118, true);
                                    target_pose.frame = 1903.9236;
                                    h.app.world_mut().entity_mut(target).insert(target_pose);
                                }
                                let mut blend =
                                    h.app.world_mut().get_mut::<EntryBlend>(target).unwrap();
                                *blend = EntryBlend::default();
                                seeded = true;
                            }
                        }
                        h.step_duration(std::time::Duration::from_nanos(
                            1_000_000_000 / fps as u64,
                        ));
                        let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                        if pose.pose_id != previous_pose {
                            if matches!(pose.pose_id, 289 | 291) {
                                entries.push((time, pose.pose_id));
                            }
                            previous_pose = pose.pose_id;
                        }
                        let hp = h.app.world().get::<Health>(target).unwrap().current;
                        let def_pose = h.app.world().get::<PoseRuntime>(target).unwrap();
                        let source = h.position();
                        let defender = h.app.world().get::<Transform>(target).unwrap().translation;
                        let yaw = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
                        let defender_yaw = h.app.world().get::<MotionState>(target).unwrap().facing;
                        let to_target = defender - source;
                        let forward_dot = (yaw.sin() * to_target.x + yaw.cos() * to_target.z)
                            / to_target.x.hypot(to_target.z).max(1e-5);
                        if hp < previous_hp {
                            hits.push(serde_json::json!({"time":time,"attackPose":pose.pose_id,
                            "attackFrame":pose.frame,"targetPose":def_pose.pose_id,
                            "attacker":source.to_array(),"target":defender.to_array(),
                            "attackerYaw":yaw,"defenderYaw":defender_yaw,"targetForwardDot":forward_dot,
                            "hpBefore":previous_hp,"hpAfter":hp}));
                        }
                        if phase_audit || matches!(pose.pose_id, 289 | 291) {
                            let phases = h.app.world().get_resource::<
                                crate::plugins::character_contact_plugin::CharacterContactFrame>();
                            let attacker_moves = phases
                                .map(|frame| frame.audit_moves(h.fighter))
                                .unwrap_or_default();
                            let target_moves = phases
                                .map(|frame| frame.audit_moves(target))
                                .unwrap_or_default();
                            samples.push(serde_json::json!({"time":time,"attackPose":pose.pose_id,
                            "attackFrame":pose.frame,"targetPose":def_pose.pose_id,
                            "targetFrame":def_pose.frame,"attacker":source.to_array(),
                            "target":defender.to_array(),"distance":source.distance(defender),
                            "attackerYaw":yaw,"defenderYaw":defender_yaw,"targetForwardDot":forward_dot,
                            "hp":hp,"attackerMoves":attacker_moves,"targetMoves":target_moves,
                            "attackerVelocity":h.app.world().get::<MotionState>(h.fighter).unwrap().velocity.to_array(),
                            "targetVelocity":h.app.world().get::<MotionState>(target).unwrap().velocity.to_array(),
                            "targetReaction":format!("{:?}", h.app.world().get::<FighterHitReaction>(target))}));
                        }
                        previous_hp = hp;
                    }
                    if seed_live289 {
                        assert!(seeded, "expected to seed the live 289 hit state");
                    }
                    eprintln!("spear fps={fps} distance={distance} lateral={lateral} contact={contact} lock={lock_updates} remote={target_remote} moving={moving_npc} sn06={sn06} frozen={frozen_npc} entries={entries:?} hit_poses={:?}",
                        hits.iter().map(|hit| hit["attackPose"].as_u64().unwrap()).collect::<Vec<_>>());
                    if oracle_case || stage_oracle {
                        let native = &h.app.world().resource::<SpearCloseGeometry>().0;
                        assert!(native.iter().any(|sample| sample["attack_pose"] == 289));
                        assert!(native.iter().any(|sample| sample["attack_pose"] == 291));
                        std::fs::write(
                            output.join(if stage_oracle && original_target_yaw {
                                "spear-sn06-original-yaw-native-geometry.json"
                            } else if stage_oracle {
                                "spear-sn06-native-geometry.json"
                            } else {
                                "spear-close-native-geometry.json"
                            }),
                            serde_json::to_vec_pretty(&serde_json::json!({"frames":native}))
                                .unwrap(),
                        )
                        .unwrap();
                    }
                    cases.push(
                        serde_json::json!({"fps":fps,"distance":distance,"lateral":lateral,"contact":contact,
                        "lock":lock_updates,"remote":target_remote,"movingNpc":moving_npc,"sn06":sn06,"frozenNpc":frozen_npc,"originalTargetYaw":original_target_yaw,"serial":serial,
                    "entries":entries,"hits":hits,"samples":samples}),
                );
                }
            }
        }
    }
    std::fs::write(
        output.join(if serial && compare_immobile {
            "spear-sn06-serial-identity-comparison.json"
        } else if serial {
            "spear-close-serial-live-chain.json"
        } else if seed_live289 {
            "spear-sn06-live289-seeded-comparison.json"
        } else if compare_immobile {
            "spear-sn06-immobile-comparison.json"
        } else if stage_only && original_target_yaw {
            "spear-sn06-original-yaw-live-chain.json"
        } else if stage_only {
            "spear-sn06-live-chain.json"
        } else if oracle_only {
            "spear-close-oracle-live-chain.json"
        } else {
            "spear-close-live-chain.json"
        }),
        serde_json::to_vec_pretty(&cases).unwrap(),
    )
    .unwrap();
}
