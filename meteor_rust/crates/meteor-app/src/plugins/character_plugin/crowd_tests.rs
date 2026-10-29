#![cfg(feature = "original-assets")]

use super::*;
use crate::components::fighter::*;
use crate::plugins::{combat_plugin::HitGeometryQueries, fighter_frame};

fn sample_crowd_hurt_boxes(
    active: Res<fighter_frame::ActiveFighter>,
    assets: Res<CharacterAssetsSet>,
    fighters: Query<Entity, With<Fighter>>,
    geometry: HitGeometryQueries,
) {
    for entity in &fighters {
        if active.0 != Some(entity) {
            std::hint::black_box(geometry.hurt_obbs(entity, &assets.0["p1"]));
        }
    }
}

fn crowd_app(count: usize) -> App {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let character = load_character_asset_for_test(std::slice::from_ref(&root), "p1")
        .expect("original P1 assets");
    let common = parse_amb(&std::fs::read(root.join("Common/character.amb")).unwrap()).unwrap();
    let mut pose = PoseRuntime::idle();
    character.apply_pose(&mut pose, 218, true);
    pose.frame += 0.375;
    let mut app = App::new();
    crate::plugins::configure_fighter_frame_sets(&mut app);
    fighter_frame::install(&mut app);
    app.init_resource::<Time<bevy::time::Real>>()
        .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p1".into(),
            character,
        )])))
        .insert_resource(CommonAmb(common))
        .add_systems(
            fighter_frame::FighterUpdate,
            (
                drive_fighter_animation.in_set(crate::plugins::FighterFrameSet::SkeletonSample),
                sample_crowd_hurt_boxes.in_set(crate::plugins::FighterFrameSet::HitJudge),
            ),
        );
    for slot in 0..count {
        let fighter = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 1,
                    player_slot: slot as u8,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_xyz((slot % 4) as f32 * 2.0, 0.0, (slot / 4) as f32 * 2.0),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: slot as f32 * 0.4,
                },
                pose.clone(),
                EntryBlend::default(),
                SmState::default(),
                FighterHitReaction::default(),
            ))
            .id();
        spawn_fighter_model_for_test(app.world_mut(), fighter);
    }
    app
}

fn verify_crowd_geometry(
    assets: Res<CharacterAssetsSet>,
    common: Res<CommonAmb>,
    geometry: HitGeometryQueries,
    poses: Query<&PoseRuntime>,
    models: Query<(Entity, &FighterModel)>,
    bones: Query<(Entity, &FighterBone, &GlobalTransform)>,
    joints: Query<&Transform>,
    dummies: Query<(Entity, &FighterDummy)>,
    transforms: bevy::transform::helper::TransformHelper,
) {
    let assets = &assets.0["p1"];
    for (model_entity, model) in &models {
        let pose = poses.get(model.fighter).unwrap();
        let sampled = assets
            .amb_for_pose(&common.0, pose.pose_id)
            .sample_at(pose.frame);
        for (entity, bone, _) in bones.iter().filter(|(_, b, _)| b.model == model_entity) {
            let transform = joints.get(entity).unwrap();
            assert_eq!(
                transform.rotation.to_array(),
                sampled.bone_quats[bone.skin_index]
            );
            if bone.skin_index == 0 {
                assert_eq!(transform.translation.to_array(), sampled.root_pivot);
            }
        }
        for (entity, dummy) in dummies.iter().filter(|(_, d)| d.model == model_entity) {
            let transform = joints.get(entity).unwrap();
            assert_eq!(
                transform.translation.to_array(),
                sampled.dummy_pos[dummy.dummy_index]
            );
            assert_eq!(
                transform.rotation.to_array(),
                sampled.dummy_quats[dummy.dummy_index]
            );
        }

        let reference = meteor_combat::hitbox::body_hurt_obbs(
            bones
                .iter()
                .filter(|(_, bone, _)| bone.model == model_entity)
                .filter_map(|(entity, bone, propagated)| {
                    let global = transforms
                        .compute_global_transform(entity)
                        .unwrap_or(*propagated);
                    let world = meteor_core::Mat4 {
                        m: global.to_matrix().to_cols_array(),
                    }
                    .mul(
                        assets
                            .skeleton
                            .skin_world_bind_inverse
                            .get(bone.skin_index)?,
                    );
                    Some((
                        bone.name.as_str(),
                        world,
                        assets
                            .bone_local_bounds
                            .get(bone.skin_index)
                            .copied()
                            .flatten(),
                    ))
                }),
        );
        let actual = geometry.hurt_obbs(model.fighter, assets);
        fn values(boxes: Vec<meteor_combat::hitbox::Obb>) -> Vec<[f32; 15]> {
            let mut values: Vec<_> = boxes
                .into_iter()
                .map(|b| {
                    [
                        b.center.x,
                        b.center.y,
                        b.center.z,
                        b.half_size.x,
                        b.half_size.y,
                        b.half_size.z,
                        b.axes[0].x,
                        b.axes[0].y,
                        b.axes[0].z,
                        b.axes[1].x,
                        b.axes[1].y,
                        b.axes[1].z,
                        b.axes[2].x,
                        b.axes[2].y,
                        b.axes[2].z,
                    ]
                })
                .collect();
            values.sort_by(|a, b| {
                a.iter()
                    .zip(b)
                    .map(|(a, b)| a.total_cmp(b))
                    .find(|order| !order.is_eq())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            values
        }
        assert!(!actual.is_empty());
        assert_eq!(values(actual), values(reference));
    }
}

#[test]
fn twelve_fighter_geometry_matches_scan_through_animation_and_model_rebuild() {
    use bevy::ecs::system::RunSystemOnce;
    let mut app = crowd_app(12);
    for tick in 0..4 {
        let world = app.world_mut();
        for (mut pose, mut transform) in world
            .query::<(&mut PoseRuntime, &mut Transform)>()
            .iter_mut(world)
        {
            pose.frame += 0.375;
            transform.translation.x += 0.2;
        }

        if tick == 1 {
            let bone = world
                .query_filtered::<Entity, With<FighterBone>>()
                .iter(world)
                .next()
                .unwrap();
            world
                .entity_mut(bone)
                .insert(Name::new("changed archetype"));
        }

        if tick == 2 {
            let (entity, fighter) = world
                .query::<(Entity, &FighterModel)>()
                .iter(world)
                .next()
                .map(|(e, m)| (e, m.fighter))
                .unwrap();
            world.despawn(entity);
            spawn_fighter_model_for_test(world, fighter);
        }
        world.run_schedule(Update);
        world.run_system_once(verify_crowd_geometry).unwrap();
    }
}

#[test]
fn joint_index_tracks_removal_and_reparenting() {
    let mut world = World::new();
    let first = world.spawn_empty().id();
    let second = world.spawn_empty().id();
    let bone = world
        .spawn(FighterBone {
            model: first,
            name: "root".into(),
            skin_index: 0,
            node_index: 0,
        })
        .id();
    let dummy = world
        .spawn(FighterDummy {
            model: first,
            name: "d_base".into(),
            dummy_index: 0,
        })
        .id();
    assert_eq!(world.get::<FighterModelBones>(first).unwrap().0, [bone]);
    assert_eq!(world.get::<FighterModelDummies>(first).unwrap().0, [dummy]);
    world.entity_mut(bone).insert(FighterBone {
        model: second,
        name: "root".into(),
        skin_index: 0,
        node_index: 0,
    });
    assert!(world.get::<FighterModelBones>(first).is_none());
    assert_eq!(world.get::<FighterModelBones>(second).unwrap().0, [bone]);
    world.despawn(bone);
    world.despawn(dummy);
    assert!(world.get::<FighterModelBones>(second).is_none());
    assert!(world.get::<FighterModelDummies>(first).is_none());
}

#[test]
#[ignore = "CPU workload; run explicitly with --ignored --nocapture --test-threads=1"]
fn benchmark_crowd_animation_and_hit_geometry() {
    use bevy::ecs::schedule::{MultiThreadedExecutor, SingleThreadedExecutor};
    for count in [2, 12, 24] {
        let mut app = crowd_app(count);
        for single in [false, true] {
            app.edit_schedule(fighter_frame::FighterUpdate, |schedule| {
                if single {
                    schedule.set_executor(SingleThreadedExecutor::new());
                } else {
                    schedule.set_executor(MultiThreadedExecutor::new());
                }
            });
            for _ in 0..10 {
                app.world_mut().run_schedule(Update);
            }
            let start = std::time::Instant::now();
            for _ in 0..100 {
                app.world_mut().run_schedule(Update);
            }
            eprintln!(
                "crowd fighters={count} single={single} cpu_ms_per_frame={:.3}",
                start.elapsed().as_secs_f64() * 10.0
            );
        }
    }
}
