use super::*;

#[test]
fn p0_collision_bounds_match_stock_capture_without_replacing_render_skin() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let asset =
        crate::plugins::character_plugin::load_character_asset_for_test(&[root.clone()], "p0")
            .unwrap();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("p0_stock_collision_bounds.json")).unwrap();
    for bone in expected["bones"].as_array().unwrap() {
        let index = bone["index"].as_u64().unwrap() as usize;
        let actual = asset.bone_local_bounds.get(index).and_then(Option::as_ref);
        if bone["half"].is_null() {
            assert!(actual.is_none(), "unexpected geometry: {}", bone["name"]);
        } else {
            let half = actual.unwrap().half_size;
            for (axis, value) in [half.x, half.y, half.z].into_iter().enumerate() {
                assert!(
                    (value - bone["half"][axis].as_f64().unwrap() as f32).abs() < 1e-5,
                    "stock collision half differs: {} axis={axis}",
                    bone["name"]
                );
            }
        }
    }
    let render = meteor_asset::skc::parse_skin(&crate::text_codec::decode_text(
        &std::fs::read(root.join("pmodel/p0.skc")).unwrap(),
    ));
    assert_eq!(asset.skin.positions, render.positions);
}

pub(super) fn install_pose_effects(app: &mut App) {
    use crate::plugins::effect_plugin::*;
    use bevy::ecs::system::RunSystemOnce;
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<PonEfMaterial>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<crate::plugins::stage_material::GmdlMaterial>>()
        .init_resource::<EfCache>()
        .init_resource::<EfTextureCache>()
        .add_message::<SceneEffectEvent>();
    app.world_mut()
        .run_system_once(setup_effect_assets)
        .unwrap();
    install_effect_playback_systems(app);
}

#[test]
#[ignore = "requires local COMBAT-080 original capture fixture"]
fn captured_dagger_crouch_skeleton_matches_production_sampling() {
    use crate::components::fighter::VisualRootPin;
    use crate::plugins::character_plugin::{
        drive_fighter_animation_for_test, load_character_asset_for_test,
        spawn_fighter_model_for_test, FighterModel,
    };
    use bevy::ecs::system::SystemState;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(root.join(
        "analysis/dagger-crouch-20260924/live/dagger-replay-hook-20260925-091609.skeleton-fixture.json"
    )).unwrap()).unwrap();
    let (mut h, _, _) = spawn_local_stock_hit_harness_with_order(DAGGER, false);
    let world = h.app.world_mut();
    let old_models: Vec<_> = world
        .query_filtered::<Entity, With<FighterModel>>()
        .iter(world)
        .collect();
    for model in old_models {
        world.despawn(model);
    }
    world.resource_mut::<CharacterAssetsSet>().0.insert(
        "p0".into(),
        load_character_asset_for_test(&[root.join("origin_res")], "p0").unwrap(),
    );
    world.get_mut::<Fighter>(h.fighter).unwrap().character_id = 0;
    world
        .entity_mut(h.fighter)
        .insert((VisualRootPin::default(), EntryBlend::default()));
    spawn_fighter_model_for_test(world, h.fighter);
    let convert = Mat4::from_scale(Vec3::splat(meteor_core::MODEL_SCALE))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let matrix = |v: &serde_json::Value| {
        Mat4::from_cols_array(&std::array::from_fn(|i| v[i].as_f64().unwrap() as f32))
    };
    for row in fixture["cases"].as_array().unwrap() {
        let pose_id = row["pose"].as_u64().unwrap() as u16;
        let frame = row["frame"].as_f64().unwrap() as f32;
        let model = matrix(&row["model"]);
        world.resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
            let mut pose = world.get_mut::<PoseRuntime>(h.fighter).unwrap();
            assets.0["p0"].apply_pose(&mut pose, pose_id, true);
            pose.frame = frame;
        });
        world.get_mut::<SmState>(h.fighter).unwrap().current = if pose_id == 257 {
            FighterState::Attack1
        } else {
            FighterState::Name
        };
        world.get_mut::<MotionState>(h.fighter).unwrap().facing =
            model.x_axis.y.atan2(model.x_axis.x);
        world.get_mut::<Transform>(h.fighter).unwrap().translation =
            convert.transform_point3(model.w_axis.truncate());
        *world.get_mut::<EntryBlend>(h.fighter).unwrap() = EntryBlend::default();
        *world.get_mut::<VisualRootPin>(h.fighter).unwrap() = VisualRootPin::default();
        drive_fighter_animation_for_test(world);

        let mut query = world.query::<(&FighterBone, &mut Transform)>();
        for (bone, mut transform) in query.iter_mut(world) {
            if let Some(captured) = row["bones"]
                .as_array()
                .unwrap()
                .iter()
                .find(|b| b["index"].as_u64() == Some(bone.skin_index as u64))
            {
                transform.rotation *= Quat::from_mat4(&matrix(&captured["extra"]));
            }
        }
        let mut state = SystemState::<HitGeometryQueries>::new(world);
        let geometry = state.get(world).unwrap();
        let model_entity = find_fighter_model(h.fighter, &geometry.models).unwrap();
        let mut actual = std::collections::HashMap::new();
        for (entity, bone, propagated) in geometry
            .bones
            .iter_many(model_bones(model_entity, &geometry.skeletons))
        {
            actual.insert(
                bone.skin_index,
                current_global_transform(entity, propagated, &geometry.transforms).to_matrix(),
            );
        }
        let captured = row["bones"].as_array().unwrap();
        let expected_root = convert * matrix(&captured[0]["animated"]);
        let cache_delta = actual[&0].w_axis.truncate() - expected_root.w_axis.truncate();
        let mut worst = 0.0_f32;
        for bone in captured {
            let index = bone["index"].as_u64().unwrap() as usize;
            let expected = convert * matrix(&bone["animated"]);
            let mut sampled = actual[&index];
            sampled.w_axis -= cache_delta.extend(0.0);
            let error = sampled
                .to_cols_array()
                .into_iter()
                .zip(expected.to_cols_array())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            worst = worst.max(error);
            assert!(
                error < 0.002 * meteor_core::MODEL_SCALE,
                "pose={pose_id} frame={frame} bone={} error={error} cache_delta={cache_delta:?}",
                bone["name"]
            );
        }
        eprintln!(
            "LIVE_SKELETON pose={pose_id} frame={frame} max_error={worst} cache_delta={cache_delta:?}"
        );
    }
}

#[test]
fn production_p0_dagger_push_against_crouch() {
    run_p0_dagger_push_against_crouch(false);
}

#[test]
fn production_ws_p0_dagger_push_against_crouch() {
    run_p0_dagger_push_against_crouch(true);
}

fn run_p0_dagger_push_against_crouch(ws: bool) {
    use crate::components::fighter::VisualRootPin;
    use crate::plugins::character_plugin::{spawn_fighter_model_for_test, FighterModel};
    for fps in [60, 120] {
        for crouch in [true, false] {
            for distance in [1.92, 2.4, 3.0] {
                let (mut h, target, _) = spawn_local_stock_hit_harness_with_order(DAGGER, true);
                let world = h.app.world_mut();
                let models: Vec<_> = world
                    .query_filtered::<Entity, With<FighterModel>>()
                    .iter(world)
                    .collect();
                for e in models {
                    world.despawn(e);
                }
                let root =
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
                let character =
                    crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p0")
                        .unwrap();
                world
                    .resource_mut::<CharacterAssetsSet>()
                    .0
                    .insert("p0".into(), character);
                for e in [h.fighter, target] {
                    world.get_mut::<Fighter>(e).unwrap().character_id = 0;
                    world.entity_mut(e).insert((
                        VisualRootPin::default(),
                        WeaponForm::default(),
                        GunAimState::default(),
                        HeadTrackState::default(),
                        FighterHitReaction::default(),
                    ));
                    world.get_mut::<Health>(e).unwrap().current = 10000.;
                    spawn_fighter_model_for_test(world, e);
                }
                world.get_mut::<Transform>(target).unwrap().translation =
                    Vec3::new(0., 0., distance);
                world.get_mut::<MotionState>(target).unwrap().movement_mode = if crouch {
                    meteor_core::MovementMode::Crouch
                } else {
                    meteor_core::MovementMode::Idle
                };
                world.resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
                    assets.0["p0"].apply_pose(
                        &mut world.get_mut::<PoseRuntime>(target).unwrap(),
                        if crouch { 10 } else { 0 },
                        true,
                    );
                });
                h.app
                    .insert_resource(NetState::default())
                    .insert_resource(State::new(AppState::Playing))
                    .add_plugins(crate::plugins::character_contact_plugin::CharacterContactPlugin);
                if ws {
                    h.app.world_mut().entity_mut(target).insert(RemoteFighter {
                        player_id: "remote-1".into(),
                    });
                    h.app.insert_resource(NetState {
                        status: NetStatus::InRoom,
                        address: "ws://localhost:9999".into(),
                        player_id: "local-1".into(),
                        round_active: true,
                        ..default()
                    });
                }
                let schedule = crate::plugins::fighter_frame::schedule(&h.app);
                h.app.add_systems(
                    schedule,
                    crate::plugins::character_plugin::drive_fighter_animation_for_test
                        .before(harness_retire_entry_blend)
                        .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                );
                h.app.add_systems(
                    schedule,
                    (tick_target_hit_clocks, attack_windows)
                        .chain()
                        .in_set(crate::plugins::FighterFrameSet::HitJudge),
                );
                h.app.add_systems(
                    schedule,
                    hit_reaction_root_motion
                        .after(crate::plugins::movement_plugin::attack_root_motion)
                        .in_set(crate::plugins::FighterFrameSet::RootMotion),
                );

                for _ in 0..3 {
                    h.step_dt(1. / fps as f32);
                }
                h.press_key(KeyCode::KeyS);
                h.step_dt(1. / fps as f32);
                h.release_key(KeyCode::KeyS);
                h.press_key(KeyCode::KeyW);
                h.press_mouse(MouseButton::Left);
                let mut saw = false;
                let mut hit257 = false;
                let mut hit258 = false;
                let mut hp = 10000.;
                let mut min_distance = f32::INFINITY;
                for tick in 0..fps * 2 {
                    let target_before = h.app.world().get::<PoseRuntime>(target).unwrap();
                    if crouch && hp == 10000. {
                        assert_eq!(
                            target_before.pose_id, 10,
                            "target stopped crouching before hit"
                        );
                    }
                    let before_sample = (target_before.pose_id, target_before.frame);
                    h.step_dt(1. / fps as f32);
                    if tick == 0 {
                        h.release_key(KeyCode::KeyW);
                        h.release_mouse(MouseButton::Left);
                    }
                    let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                    let now = h.app.world().get::<Health>(target).unwrap().current;
                    if pose.pose_id == 257 {
                        saw = true;
                        min_distance = min_distance.min(
                            (h.position()
                                - h.app.world().get::<Transform>(target).unwrap().translation)
                                .length(),
                        );
                        if now < hp {
                            eprintln!(
                                "HIT257 ws={ws} target_before={before_sample:?} frame={} source={:?} target={:?}",
                                pose.frame,
                                h.position(),
                                h.app.world().get::<Transform>(target).unwrap().translation
                            );
                            hit257 = true;
                        }
                    }
                    if pose.pose_id == 258 && now < hp {
                        hit258 = true;
                    }
                    if ws {
                        if let Some(pending) = h.app.world().get::<PendingRemoteHit>(target) {
                            hit257 |= pending.pose_id == 257;
                            hit258 |= pending.pose_id == 258;
                        }
                        h.app
                            .world_mut()
                            .entity_mut(target)
                            .remove::<PendingRemoteHit>();
                    }
                    hp = now;
                }
                eprintln!(
                    "P0_PUSH ws={ws} fps={fps} crouch={crouch} distance={distance} saw={saw} hit257={hit257} min_distance={min_distance} hp={hp}"
                );
                assert!(saw);
                if crouch {
                    assert!(!hit257, "fps={fps} distance={distance}");
                    assert!(
                        hit258,
                        "three stabs must still hit: ws={ws} fps={fps} distance={distance}"
                    );
                } else {
                    assert!(hit257, "standing control fps={fps} distance={distance}");
                }
            }
        }
    }
}
