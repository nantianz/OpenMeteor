#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct NinjaEntryGeometry(Vec<serde_json::Value>);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct NinjaEntryPair {
    attacker: Entity,
    defender: Entity,
}

#[cfg(not(target_arch = "wasm32"))]
fn export_ninja_entry_geometry(
    active: Option<Res<super::super::fighter_frame::ActiveFighter>>,
    mut audit: ResMut<NinjaEntryGeometry>,
    pair: Res<NinjaEntryPair>,
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
    if pose.pose_id != 474
        || (pose.frame - 15335.0).abs() > 0.01
        || blend.frame_record_source != Some((473, 15334))
        || audit.0.len() >= 8
    {
        return;
    }
    let asset = &assets.0["p0"];
    let Some((_, window)) = sampled_attack_window(asset, pose, blend) else {
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
    let target_pose = fighters.get(pair.defender).unwrap().2;
    let attacks = geometry.attack_obbs(pair.attacker, attacker, form, 474, asset, &window.bones);
    let hurts = geometry.hurt_obbs(pair.defender, asset);
    let effect_boxes = geometry.live_effect_obbs(pair.attacker, attacker, form, 474, asset);
    let whole = fighter_whole_body_obb(pair.defender, asset, &geometry);
    let sample = audit.0.len();
    audit.0.push(
        serde_json::json!({"sample":sample,"frame":pose.frame,"character":0,"target_character":0,
        "target_pose":target_pose.pose_id,"target_frame":target_pose.frame,
        "source_bones":skeleton(pair.attacker),"target_bones":skeleton(pair.defender),
        "attack_names":window.bones,"effects":[],
        "rust_body_hit":attacks.iter().any(|a|hurts.iter().any(|b|obbs_intersect(a,b))),
        "rust_effect_hit":whole.as_ref().is_some_and(|body|
            effect_boxes.iter().any(|effect|obbs_intersect(effect,body)))}),
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "writes Pose473 conditional lifecycle under analysis"]
fn diagnose_ninja_burn_multi_hits_in_attack_windows() {
    use crate::plugins::effect_plugin as fx;
    use bevy::ecs::system::RunSystemOnce;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for (pose_id, blended_entry) in [(473_u16, false), (474, false), (474, true)] {
        for fps in [60_u32, 120] {
            let (mut h, target, take_hits) = spawn_local_stock_hit_harness_with_order(12, true);
            h.app.insert_resource(State::new(AppState::Playing));
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
            h.app
                .world_mut()
                .entity_mut(target)
                .insert((FighterHitReaction::default(), GunAimState::default()));
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = if pose_id == 473 {
                Vec3::new(-4.0, 0.0, -4.0)
            } else {
                Vec3::new(-1.0, 0.0, -1.0)
            };
            h.app
                .world_mut()
                .get_mut::<MotionState>(target)
                .unwrap()
                .facing = std::f32::consts::FRAC_PI_4;
            *h.app.world_mut().get_mut::<Health>(target).unwrap() = Health {
                current: 2000.0,
                max: 2000.0,
            };
            let mut pose = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p0"]
                .apply_pose(&mut pose, pose_id, true);
            let blend = if blended_entry {
                EntryBlend {
                    pose_id,
                    duration: 0.15,
                    motion_source: Some((473, 15334)),
                    frame_record_source: Some((473, 15334)),
                    ..Default::default()
                }
            } else {
                EntryBlend::default()
            };
            h.app
                .world_mut()
                .entity_mut(h.fighter)
                .insert((pose, blend));
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
            h.app
                .init_resource::<Assets<fx::PonEfMaterial>>()
                .init_resource::<Assets<Mesh>>()
                .init_resource::<Assets<Image>>()
                .init_resource::<Assets<StandardMaterial>>()
                .init_resource::<fx::EfTextureCache>()
                .init_resource::<Assets<crate::plugins::stage_material::GmdlMaterial>>()
                .add_message::<fx::SceneEffectEvent>();
            h.app
                .world_mut()
                .run_system_once(fx::setup_effect_assets)
                .unwrap();
            fx::install_effect_playback_systems(&mut h.app);
            crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
            let schedule = super::super::fighter_frame::schedule(&h.app);
            h.app.add_systems(
                Update,
                tick_target_hit_clocks.in_set(crate::plugins::FighterFrameSet::PoseDecision),
            );
            h.app.add_systems(
                schedule,
                attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
            );
            if pose_id == 474 && blended_entry {
                h.app
                    .insert_resource(NinjaEntryPair {
                        attacker: h.fighter,
                        defender: target,
                    })
                    .init_resource::<NinjaEntryGeometry>()
                    .add_systems(
                        schedule,
                        export_ninja_entry_geometry
                            .in_set(crate::plugins::FighterFrameSet::HitJudge)
                            .before(attack_windows),
                    );
            }
            let mut rows = Vec::new();
            for tick in 0..fps * 2 {
                h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / fps as u64));
                let requests = take_hits();
                let source = h.app.world().get::<PoseRuntime>(h.fighter).unwrap().clone();
                let effects: Vec<_> = h
                    .app
                    .world_mut()
                    .query::<(&fx::PoseOwnedPonEf, &fx::PonEfInstance)>()
                    .iter(h.app.world())
                    .filter(|(owner, _)| owner.fighter == h.fighter)
                    .map(|(_, effect)| (effect.elapsed, effect.duration))
                    .collect();
                let attacker = h.fighter;
                let effect_shapes = h
                    .app
                    .world_mut()
                    .run_system_once(
                        move |geometry: HitGeometryQueries, assets: Res<CharacterAssetsSet>, fighters: Query<(&Fighter, &WeaponForm, &PoseRuntime)>| {
                            let (fighter, form, pose) = fighters.get(attacker).unwrap();
                            geometry.live_effect_obbs(attacker, fighter, form, pose.pose_id, &assets.0["p0"])
                        },
                    )
                    .unwrap();
                rows.push(serde_json::json!({"time":(tick+1) as f32/fps as f32,
                "pose":source.pose_id,"frame":source.frame,
                "effectAges":effects,
                "effectCenters":effect_shapes.iter().map(|shape|shape.center).collect::<Vec<_>>(),
                "requests":requests.iter().map(|(_,request)|serde_json::json!({
                    "target":request.target_actor_id,
                    "targetPose":request.target_pose(),"move":request.target_move(),
                    "minimum":request.target_value(),"strategy":request.hit_word>>16,
                })).collect::<Vec<_>>() }));
            }
            let claims: Vec<_> = rows
                .iter()
                .filter(|row| row["requests"].as_array().is_some_and(|a| !a.is_empty()))
                .map(|row| {
                    (
                        row["time"].as_f64().unwrap(),
                        row["frame"].as_f64().unwrap(),
                        row["requests"].clone(),
                    )
                })
                .collect();
            eprintln!("ninja{pose_id} fps={fps} blended={blended_entry}: claims={claims:?}");
            let path = root.join("analysis/mrc-moves-20260928").join(format!(
                "ninja{pose_id}-live-effect-{}-{fps}.json",
                if blended_entry { "blend" } else { "direct" }
            ));
            std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
            if pose_id == 474 && blended_entry && fps == 60 {
                let native = &h.app.world().resource::<NinjaEntryGeometry>().0;
                assert_eq!(
                    native.len(),
                    8,
                    "capture the conditional blend-entry collision matrices"
                );
                assert_eq!(
                    native
                        .iter()
                        .map(|sample| sample["rust_body_hit"].as_bool().unwrap())
                        .collect::<Vec<_>>(),
                    [false, true, false, false, false, false, false, false],
                    "the second mixed-entry sample is the conditional body contact"
                );
                assert!(native
                    .iter()
                    .all(|sample| sample["rust_effect_hit"] == false));
                std::fs::write(
                    root.join("analysis/mrc-moves-20260928/ninja474-entry-native-geometry.json"),
                    serde_json::to_vec_pretty(&serde_json::json!({"frames":native})).unwrap(),
                )
                .unwrap();
            }
            assert!(rows
                .iter()
                .any(|row| row["effectAges"].as_array().is_some_and(|a| !a.is_empty())));
            if pose_id == 473 {
                assert_eq!(
                    claims.len(),
                    3,
                    "fixed near target must take three Pose473 contacts at {fps} FPS"
                );
                for (index, (time, frame, requests)) in claims.iter().enumerate() {
                    assert!((15317.0..=15334.0).contains(frame));
                    assert_eq!(requests[0]["targetPose"], 109);
                    assert_eq!(requests[0]["target"], 2);
                    assert_eq!(requests[0]["move"], 0.0);
                    assert_eq!(requests[0]["minimum"], 3.0);
                    assert_eq!(requests[0]["strategy"], 11);
                    if index > 0 {
                        assert!(*time - claims[index - 1].0 > 0.2);
                    }
                }
            }
            if pose_id == 474 && !blended_entry {
                assert_eq!(
                    claims.len(),
                    7,
                    "fixed near target must take seven direct Pose474 contacts at {fps} FPS"
                );
                for (index, (time, frame, requests)) in claims.iter().enumerate() {
                    assert!((15335.0..=15392.5).contains(frame));
                    assert_eq!(requests[0]["targetPose"], 95);
                    assert_eq!(requests[0]["target"], 2);
                    assert_eq!(requests[0]["move"], 0.0);
                    assert_eq!(requests[0]["minimum"], 0.0);
                    assert_eq!(requests[0]["strategy"], 11);
                    if index > 0 {
                        assert!(*time - claims[index - 1].0 > 0.2);
                    }
                }
            }
            if pose_id == 474 && blended_entry {
                let first = rows
                    .iter()
                    .find(|row| {
                        row["requests"]
                            .as_array()
                            .is_some_and(|requests| !requests.is_empty())
                    })
                    .unwrap();
                assert_eq!(first["frame"], 15335.0);
                assert!(
                    first["effectAges"].as_array().unwrap().is_empty(),
                    "the conditional blend-entry contact is body/weapon, before new EF"
                );
            }
        }
    }
}
