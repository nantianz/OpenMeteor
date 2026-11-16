use super::*;
use crate::components::fighter::*;
use crate::plugins::combat_plugin::HitGeometryQueries;
use crate::plugins::rebalance_plugin::{sync_afterimage, BalanceAfterimage, BalanceGlow};
use bevy::ecs::system::RunSystemOnce;

fn spring_node() -> DisplayNode {
    DisplayNode {
        source: Entity::PLACEHOLDER,
        joint: Entity::PLACEHOLDER,
        control: Control {
            name: "ribbon_L_tip".into(),
            anchor: 2,
            pivot: [0.0; 3],
            kind: "ribbon".into(),
            side: -1.0,
            limit: 0.18,
        },
        angle: Vec2::ZERO,
        target: Vec2::ZERO,
        angular_velocity: Vec2::ZERO,
        pivot: None,
        pivot_velocity: Vec3::ZERO,
        gravity_axis: Vec3::NEG_Z,
    }
}

#[test]
fn remake_inertial_spring_coasts_rebounds_and_settles_independently_of_frame_rate() {
    let mut results = Vec::new();
    for fps in [30, 60, 144] {
        let mut node = spring_node();
        node.target = Vec2::new(0.065, 0.0);
        for _ in 0..fps {
            advance_spring(&mut node, 1.0 / fps as f32);
        }
        node.target = Vec2::ZERO;
        advance_spring(&mut node, 1.0 / fps as f32);
        assert!(node.angle.x > 0.04, "stopping must retain momentum");
        let mut crossed_rest = false;
        for _ in 1..fps * 3 {
            advance_spring(&mut node, 1.0 / fps as f32);
            crossed_rest |= node.angle.x < -0.001;
            assert!(node.angle.length() <= node.control.limit + 1e-6);
        }
        assert!(crossed_rest, "the damped ribbon must rebound");
        assert!(node.angle.length() < 1e-5 && node.angular_velocity.length() < 1e-4);
        results.push((node.angle, node.angular_velocity));
    }
    for pair in results.windows(2) {
        assert!(pair[0].0.distance(pair[1].0) < 1e-6);
        assert!(pair[0].1.distance(pair[1].1) < 1e-6);
    }
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_spring_extreme_forcing_stays_bounded_and_disable_clears_momentum() {
    let mut node = spring_node();
    for i in 0..1000 {
        node.target = Vec2::new(if i % 7 < 3 { 0.18 } else { -0.18 }, 0.0);
        advance_spring(&mut node, if i % 3 == 0 { 0.12 } else { 1.0 / 144.0 });
        assert!(node.angle.is_finite() && node.angular_velocity.is_finite());
        assert!(node.angle.length() <= node.control.limit + 1e-6);
    }
    let (mut app, fighter, model) = fixture();
    app.world_mut()
        .get_mut::<MotionState>(fighter)
        .unwrap()
        .velocity = Vec3::new(4.0, 0.0, 6.0);
    for _ in 0..10 {
        app.update();
    }
    assert!(app
        .world()
        .get::<SecondaryMotion>(model)
        .unwrap()
        .nodes
        .iter()
        .any(|n| n.angular_velocity.length() > 1e-4));
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .character_motion = false;
    app.update();
    assert!(app
        .world()
        .get::<SecondaryMotion>(model)
        .unwrap()
        .nodes
        .iter()
        .all(|n| n.angle == Vec2::ZERO && n.angular_velocity == Vec2::ZERO && n.pivot.is_none()));
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_hair_measures_head_motion_without_moving_the_waist_or_stock_bones() {
    let (mut app, _, model) = fixture();
    for _ in 0..4 {
        app.world_mut().run_system_once(update).unwrap();
    }
    let display = app.world().get::<DisplaySkin>(model).unwrap().0;
    let head = app.world().get::<SkinnedMesh>(display).unwrap().joints[13];
    let before = *app.world().get::<GlobalTransform>(head).unwrap();
    let shifted = GlobalTransform::from(
        Mat4::from_translation(Vec3::new(0.3, 0.0, 0.0)) * before.to_matrix(),
    );
    app.world_mut().entity_mut(head).insert(shifted);
    for _ in 0..2 {
        app.world_mut().run_system_once(update).unwrap();
    }
    let motion = app.world().get::<SecondaryMotion>(model).unwrap();
    assert!(motion
        .nodes
        .iter()
        .filter(|n| n.control.kind == "hair")
        .all(|n| n.pivot_velocity.length() > 3.0));
    assert!(motion
        .nodes
        .iter()
        .filter(|n| n.control.kind == "ribbon")
        .all(|n| n.pivot_velocity.length() < 1e-5));
    assert_eq!(
        *app.world().get::<GlobalTransform>(head).unwrap(),
        shifted,
        "secondary motion must never write back to its source"
    );
}

fn prepared_for(skeleton: &Skeleton) -> VisualAssets {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/generated/p0-remake");
    prepared(
        skeleton,
        std::fs::read(root.join("metadata.json")).unwrap(),
        |name| std::fs::read(root.join(name)).ok(),
    )
    .unwrap()
}

fn character() -> CharacterAssets {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let mut assets = load_character_asset_for_test(&[root], "p0").unwrap();
    assets.visual = Some(prepared_for(&assets.skeleton));
    assets
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_rejects_changed_stock_bind_and_reuses_gpu_mesh_texture_and_inverse() {
    let character = character();
    let visual = character.visual.as_ref().unwrap();
    assert!(visual.skins[0].triangle_count <= 48_000);
    assert!(visual.skins[1].triangle_count <= 8_000);
    assert_eq!(visual.controls.len(), 8);
    for skin in &visual.skins {
        assert!(skin.skin_indices.iter().any(|i| *i >= 30));
        for weights in skin.skin_weights.chunks_exact(4) {
            assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        }
        let mut seams = std::collections::HashMap::new();
        for (i, p) in skin.positions.chunks_exact(3).enumerate() {
            let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
            let weights = (
                &skin.skin_indices[i * 4..i * 4 + 4],
                &skin.skin_weights[i * 4..i * 4 + 4],
            );
            if let Some(previous) = seams.insert(key, weights) {
                assert_eq!(
                    previous, weights,
                    "UV seams must remain welded while deforming"
                );
            }
        }
    }
    let mut meshes = Assets::<Mesh>::default();
    assert_eq!(visual.mesh(&mut meshes), visual.mesh(&mut meshes));
    assert_eq!(meshes.len(), 1);
    let mut images = Assets::<bevy::image::Image>::default();
    let mut materials = Assets::<GmdlMaterial>::default();
    let mut quality = crate::plugins::pak_textures::QualityTextureHandles::default();
    let a = visual.material(0, &mut images, &mut materials, 0, &mut quality);
    let b = visual.material(0, &mut images, &mut materials, 0, &mut quality);
    assert_ne!(
        a, b,
        "buffs/concealment must keep instance materials separate"
    );
    assert_eq!(images.len(), 1, "actor copies share the large texture");
    visual.material(1, &mut images, &mut materials, 0, &mut quality);
    visual.material(2, &mut images, &mut materials, 0, &mut quality);
    assert_eq!(images.len(), 3, "camp variants have separate cached images");
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/generated/p0-remake");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("metadata.json")).unwrap()).unwrap();
    meta["bindNodes"][17]["pivot"][0] = serde_json::json!(100);
    assert!(prepared(
        &character.skeleton,
        serde_json::to_vec(&meta).unwrap(),
        |_| None
    )
    .err()
    .unwrap()
    .contains("stock bind mismatch"));
}

fn fixture() -> (App, Entity, Entity) {
    fixture_with_models(true)
}

fn fixture_with_models(new_character_models: bool) -> (App, Entity, Entity) {
    let character = character();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let common = parse_amb(&std::fs::read(root.join("Common/character.amb")).unwrap()).unwrap();
    let mut pose = PoseRuntime::idle();
    character.apply_pose(&mut pose, 93, true);
    let mut app = App::new();
    app.add_plugins(TransformPlugin)
        .init_resource::<Time>()
        .init_resource::<Time<Real>>()
        .insert_resource(crate::app_state::Settings {
            new_character_models,
            ..Default::default()
        })
        .insert_resource(State::new(crate::app_state::AppState::Playing))
        .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p0".into(),
            character,
        )])))
        .insert_resource(CommonAmb(common))
        .init_resource::<Assets<GmdlMaterial>>()
        .insert_resource(crate::plugins::network_plugin::NetState {
            status: crate::plugins::network_plugin::NetStatus::InRoom,
            battle_rules: meteor_combat::battle_rules::BattleRules::bundled(),
            ..Default::default()
        })
        .add_systems(Update, super::super::drive_fighter_animation);
    install(&mut app);
    super::super::hit_sway::install(&mut app);
    app.add_systems(
        PostUpdate,
        sync_afterimage.after(bevy::transform::TransformSystems::Propagate),
    );
    let fighter = app
        .world_mut()
        .spawn((
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 5,
                weapon_slots: [5, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Transform::from_xyz(4.0, 2.0, -3.0),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.7,
            },
            pose,
            Health::default(),
            EntryBlend::default(),
            SmState::default(),
            GunAimState::default(),
            FighterHitReaction::default(),
        ))
        .id();
    spawn_fighter_model_for_test(app.world_mut(), fighter);
    let model = app
        .world_mut()
        .query_filtered::<Entity, With<FighterModel>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
    app.update();
    (app, fighter, model)
}

fn boxes(
    assets: Res<CharacterAssetsSet>,
    geometry: HitGeometryQueries,
    fighters: Query<Entity, With<Fighter>>,
) -> Vec<[f32; 15]> {
    geometry
        .hurt_obbs(fighters.single().unwrap(), &assets.0["p0"])
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
        .collect()
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_motion_keeps_authored_bones_hands_weapon_dummies_boxes_and_pose_identical() {
    let (mut app, fighter, model) = fixture();
    let stock: Vec<_> = app
        .world_mut()
        .query::<(Entity, &FighterBone, &Transform, &GlobalTransform)>()
        .iter(app.world())
        .map(|(e, _, local, world)| (e, *local, *world))
        .collect();
    let dummies: Vec<_> = app
        .world_mut()
        .query_filtered::<(Entity, &Transform, &GlobalTransform), With<FighterDummy>>()
        .iter(app.world())
        .map(|(e, a, b)| (e, *a, *b))
        .collect();
    let before_boxes = app.world_mut().run_system_once(boxes).unwrap();
    assert!(!before_boxes.is_empty());
    let pose = format!("{:?}", app.world().get::<PoseRuntime>(fighter).unwrap());
    for i in 0..30 {
        app.world_mut()
            .get_mut::<MotionState>(fighter)
            .unwrap()
            .velocity = Vec3::new(i as f32 * 0.2, 0.0, 3.0);
        app.update();
        assert_eq!(
            format!("{:?}", app.world().get::<PoseRuntime>(fighter).unwrap()),
            pose
        );
        assert_eq!(
            app.world_mut().run_system_once(boxes).unwrap(),
            before_boxes
        );
        for (e, a, b) in stock.iter().chain(&dummies) {
            assert_eq!(app.world().get::<Transform>(*e).unwrap(), a);
            assert_eq!(app.world().get::<GlobalTransform>(*e).unwrap(), b);
        }
    }
    let motion = app.world().get::<SecondaryMotion>(model).unwrap();
    assert_eq!(motion.nodes.len(), 8);
    assert!(motion.nodes.iter().any(|n| n.angle.length() > 0.001));
    for n in &motion.nodes {
        assert!(app.world().get::<Transform>(n.joint).is_none());
        assert!(app.world().get::<FighterBone>(n.joint).is_none());
        assert!(app.world().get::<FighterDummy>(n.joint).is_none());
        assert!(n.angle.length() <= n.control.limit + 1e-6);
    }
    let angles: Vec<_> = motion.nodes.iter().map(|n| n.angle).collect();
    app.world_mut()
        .insert_resource(crate::plugins::FrameClock::default());
    app.update();
    assert_eq!(
        app.world()
            .get::<SecondaryMotion>(model)
            .unwrap()
            .nodes
            .iter()
            .map(|n| n.angle)
            .collect::<Vec<_>>(),
        angles,
        "hit stop / skipped simulation frames freeze cosmetic motion"
    );
    app.world_mut()
        .remove_resource::<crate::plugins::FrameClock>();
    app.world_mut()
        .insert_resource(State::new(crate::app_state::AppState::Paused));
    app.update();
    assert_eq!(
        app.world()
            .get::<SecondaryMotion>(model)
            .unwrap()
            .nodes
            .iter()
            .map(|n| n.angle)
            .collect::<Vec<_>>(),
        angles
    );
    app.world_mut()
        .insert_resource(State::new(crate::app_state::AppState::Playing));
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .character_motion = false;
    app.update();
    assert!(app
        .world()
        .get::<SecondaryMotion>(model)
        .unwrap()
        .nodes
        .iter()
        .all(|n| n.angle == Vec2::ZERO));
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .character_motion = true;
    app.world_mut()
        .get_mut::<Transform>(fighter)
        .unwrap()
        .translation
        .x += 100.0;
    app.update();
    assert!(app
        .world()
        .get::<SecondaryMotion>(model)
        .unwrap()
        .nodes
        .iter()
        .all(|n| n.angle == Vec2::ZERO));
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_rebalance_afterimage_copies_all_38_live_joints_and_releases() {
    let (mut app, fighter, model) = fixture();
    app.world_mut()
        .entity_mut(fighter)
        .insert(BalanceGlow::armor(93));
    app.update();
    app.update();
    let ghost = app.world().get::<BalanceAfterimage>(model).unwrap();
    assert_eq!(ghost.source_joints.len(), 38);
    assert_eq!(ghost.ghost_joints.len(), 38);
    for entity in &ghost.source_joints[..30] {
        assert!(app.world().get::<FighterBone>(*entity).is_some());
    }
    for entity in &ghost.source_joints[30..] {
        assert!(app.world().get::<SecondaryJoint>(*entity).is_some());
    }
    app.world_mut().entity_mut(fighter).remove::<BalanceGlow>();
    app.update();
    assert!(app.world().get::<BalanceAfterimage>(model).is_none());
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_toggle_replaces_only_body_draws_and_updates_ghosts_and_joint_counts() {
    let (mut app, fighter, model) = fixture();
    app.world_mut()
        .entity_mut(fighter)
        .insert(BalanceGlow::armor(93));
    app.update();
    app.update();
    let source: Vec<_> = app.world_mut()
        .query_filtered::<(Entity, &Transform, &GlobalTransform), Or<(With<FighterBone>, With<FighterDummy>)>>()
        .iter(app.world()).map(|(e, a, b)| (e, *a, *b)).collect();
    let weapons: Vec<_> = app
        .world_mut()
        .query_filtered::<Entity, With<crate::components::fighter::WeaponModel>>()
        .iter(app.world())
        .collect();
    let pose = format!("{:?}", app.world().get::<PoseRuntime>(fighter).unwrap());
    let hit_boxes = app.world_mut().run_system_once(boxes).unwrap();
    let old_ghost_parts = app
        .world()
        .get::<BalanceAfterimage>(model)
        .unwrap()
        .ghost_joints
        .clone();
    let mut warmed_textures = None;
    for (index, enabled) in [false, true, false, true].into_iter().enumerate() {
        app.world_mut()
            .resource_mut::<crate::app_state::Settings>()
            .new_character_models = enabled;
        app.update();
        app.update();
        assert_eq!(
            app.world().get::<Appearance>(model).unwrap().enabled,
            enabled
        );
        assert_eq!(app.world().get::<SecondaryMotion>(model).is_some(), enabled);
        assert_eq!(app.world().get::<DisplaySkin>(model).is_some(), enabled);
        assert_eq!(
            format!("{:?}", app.world().get::<PoseRuntime>(fighter).unwrap()),
            pose
        );
        assert_eq!(app.world_mut().run_system_once(boxes).unwrap(), hit_boxes);
        for (e, local, world) in &source {
            assert_eq!(*app.world().get::<Transform>(*e).unwrap(), *local);
            assert_eq!(*app.world().get::<GlobalTransform>(*e).unwrap(), *world);
        }
        for e in &weapons {
            assert!(app.world().get_entity(*e).is_ok());
        }
        let expected = if enabled { 38 } else { 30 };
        for (parent, skin) in app
            .world_mut()
            .query::<(&ChildOf, &SkinnedMesh)>()
            .iter(app.world())
        {
            if parent.parent() == model {
                assert_eq!(skin.joints.len(), expected);
                assert_eq!(
                    app.world()
                        .resource::<Assets<SkinnedMeshInverseBindposes>>()
                        .get(&skin.inverse_bindposes)
                        .unwrap()
                        .len(),
                    expected
                );
            }
        }
        let ghost = app.world().get::<BalanceAfterimage>(model).unwrap();
        assert_eq!(ghost.source_joints.len(), expected);
        assert_eq!(ghost.ghost_joints.len(), expected);
        for mesh in &ghost.ghost_meshes {
            assert_eq!(
                app.world().get::<SkinnedMesh>(*mesh).unwrap().joints,
                ghost.ghost_joints,
                "afterimage draws must retain their offset joints, not the body's joints"
            );
        }
        if index == 0 {
            warmed_textures = Some(
                app.world()
                    .resource::<crate::plugins::pak_textures::QualityTextureHandles>()
                    .0
                    .len(),
            );
            assert!(old_ghost_parts
                .iter()
                .all(|e| app.world().get_entity(*e).is_err()));
        }
        assert_eq!(
            app.world()
                .resource::<crate::plugins::pak_textures::QualityTextureHandles>()
                .0
                .len(),
            warmed_textures.unwrap(),
            "repeated toggles must not retain new texture copies"
        );
        assert_eq!(
            app.world_mut()
                .query::<&SecondaryJoint>()
                .iter(app.world())
                .count(),
            if enabled { 8 } else { 0 },
            "cosmetic joints must not accumulate"
        );
    }
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_toggle_preserves_concealment_while_settings_pause_the_session() {
    let (mut app, fighter, model) = fixture();
    app.world_mut()
        .insert_resource(State::new(crate::app_state::AppState::Settings));
    for mut skin in app
        .world_mut()
        .query::<&mut FighterSkin>()
        .iter_mut(app.world_mut())
    {
        skin.concealed = true;
    }
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .new_character_models = false;
    app.update();
    assert!(app.world().get::<SecondaryMotion>(model).is_none());
    let mut found = 0;
    for (skin, material) in app
        .world_mut()
        .query::<(&FighterSkin, &MeshMaterial3d<GmdlMaterial>)>()
        .iter(app.world())
    {
        if skin.fighter == fighter {
            found += 1;
            assert!(skin.concealed);
            let material = app
                .world()
                .resource::<Assets<GmdlMaterial>>()
                .get(&material.0)
                .unwrap();
            assert_eq!(material.params.opacity, 0.1);
            assert_eq!(material.mode, GmdlMode::Blend);
        }
    }
    assert!(found > 1, "stock P0 has multiple material groups");
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_can_start_stock_then_enable_and_reuse_the_original_draws() {
    let (mut app, _, model) = fixture_with_models(false);
    assert!(app.world().get::<SecondaryMotion>(model).is_none());
    assert!(!app.world().get::<Appearance>(model).unwrap().enabled);
    let original_meshes: Vec<_> = app
        .world()
        .get::<Appearance>(model)
        .unwrap()
        .stock
        .as_ref()
        .unwrap()
        .draws
        .iter()
        .map(|draw| draw.mesh.id())
        .collect();
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .new_character_models = true;
    app.update();
    assert_eq!(
        app.world()
            .get::<SecondaryMotion>(model)
            .unwrap()
            .nodes
            .len(),
        8
    );
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .new_character_models = false;
    app.update();
    let mut restored: Vec<_> = app
        .world_mut()
        .query::<(&FighterSkin, &Mesh3d)>()
        .iter(app.world())
        .map(|(_, mesh)| mesh.0.id())
        .collect();
    let mut expected = original_meshes;
    restored.sort();
    expected.sort();
    assert_eq!(restored, expected);
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_hit_sway_keeps_cosmetic_suffix_and_authored_hand_matrices() {
    use meteor_combat::hit_reaction::{FacingBucket, HitReact, HitReactKind};
    let (mut app, fighter, model) = fixture();
    app.world_mut()
        .get_mut::<FighterHitReaction>(fighter)
        .unwrap()
        .begin(
            HitReact {
                kind: HitReactKind::BeHit,
                pose_id: Some(93),
                head_pose_id: None,
                lock_remaining: 0.3,
                minimum_remaining: 0.3,
                authored_minimum: 0.3,
                motion_scale: 5.0,
                down_remaining: 0.0,
                bucket: FacingBucket::Front,
                environmental: false,
                selector: 0,
                rewind_count: 0,
                play_to_end_pose_id: None,
            },
            false,
        );
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .hit_sway = true;
    app.world_mut()
        .get_mut::<PoseRuntime>(fighter)
        .unwrap()
        .pose_instance_id += 1;
    app.update();
    let hands: Vec<_> = app
        .world_mut()
        .query::<(Entity, &FighterBone)>()
        .iter(app.world())
        .filter(|(_, bone)| bone.name == "bau_L_Hand" || bone.name == "bau_R_Hand")
        .map(|(e, bone)| (e, bone.skin_index))
        .collect();
    let skin_joints = app
        .world_mut()
        .query::<(&SkinnedMesh, &ChildOf)>()
        .iter(app.world())
        .find(|(_, parent)| parent.parent() == model)
        .unwrap()
        .0
        .joints
        .clone();
    assert_eq!(skin_joints.len(), 38);
    assert!(skin_joints[30..]
        .iter()
        .all(|e| app.world().get::<SecondaryJoint>(*e).is_some()));
    for (source, i) in hands {
        assert_eq!(
            app.world().get::<GlobalTransform>(source).unwrap(),
            app.world().get::<GlobalTransform>(skin_joints[i]).unwrap()
        );
    }
    app.world_mut()
        .entity_mut(fighter)
        .insert(BalanceGlow::armor(93));
    app.update();
    let ghost = app.world().get::<BalanceAfterimage>(model).unwrap();
    assert_eq!(ghost.source_joints.len(), 38);
    assert!(ghost.source_joints[..30]
        .iter()
        .all(|e| app.world().get::<FighterBone>(*e).is_some()));
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .hit_sway = false;
    app.update();
    let joints = app
        .world_mut()
        .query::<(&SkinnedMesh, &ChildOf)>()
        .iter(app.world())
        .find(|(_, parent)| parent.parent() == model)
        .unwrap()
        .0
        .joints
        .clone();
    assert_eq!(joints.len(), 38);
    assert!(joints[..30]
        .iter()
        .all(|e| app.world().get::<FighterBone>(*e).is_some()));
}

#[cfg(feature = "original-assets")]
#[test]
fn remake_secondary_update_is_isolated_across_real_stock_poses() {
    let (mut app, fighter, _) = fixture();
    for id in [0, 10, 144, 151, 257, 93, 112] {
        let mut pose = app.world().get::<PoseRuntime>(fighter).unwrap().clone();
        app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut pose, id, true);
        app.world_mut().entity_mut(fighter).insert(pose);
        app.update();
        let boxes_before = app.world_mut().run_system_once(boxes).unwrap();
        let matrices: Vec<_> = app.world_mut().query_filtered::<(Entity, &Transform, &GlobalTransform), Or<(With<FighterBone>, With<FighterDummy>)>>()
            .iter(app.world()).map(|(e, a, b)| (e, *a, *b)).collect();
        app.world_mut().run_system_once(update).unwrap();
        assert_eq!(
            app.world_mut().run_system_once(boxes).unwrap(),
            boxes_before
        );
        for (e, a, b) in matrices {
            assert_eq!(*app.world().get::<Transform>(e).unwrap(), a);
            assert_eq!(*app.world().get::<GlobalTransform>(e).unwrap(), b);
        }
    }
}
