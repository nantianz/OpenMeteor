#![cfg(feature = "original-assets")]
use super::*;
use crate::components::fighter::{
    EntryBlend, Fighter, LockOnController, MotionState, PoseRuntime, SmState,
};
use crate::plugins::character_plugin::{
    drive_fighter_animation_for_test, load_character_asset_for_test, spawn_fighter_model_for_test,
    CharacterAssetsSet, CommonAmb,
};
use bevy::ecs::system::RunSystemOnce;

fn fixture() -> (App, Entity, Entity, Entity) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let assets = load_character_asset_for_test(std::slice::from_ref(&root), "p0").unwrap();
    let common =
        meteor_asset::amb::parse_amb(&std::fs::read(root.join("Common/character.amb")).unwrap())
            .unwrap();
    let mut pose = PoseRuntime::idle();
    assets.apply_pose(&mut pose, 218, true);
    let mut app = App::new();
    app.init_resource::<Time<bevy::time::Real>>()
        .init_resource::<Time>()
        .init_resource::<Settings>()
        .init_resource::<StageCollider>()
        .init_resource::<LockOnController>()
        .init_resource::<spectator::SpectatorState>()
        .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p0".into(),
            assets,
        )])))
        .insert_resource(CommonAmb(common));
    let camera = install_follow_camera_for_test(&mut app);
    install(&mut app);
    app.add_systems(Update, sample_head.before(follow_fighter_camera));
    let fighter = app
        .world_mut()
        .spawn((
            LocalFighter,
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 5,
                weapon_slots: [5, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Transform::from_xyz(15.0, 3.0, -10.0),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.7,
            },
            pose,
            EntryBlend::default(),
            SmState::default(),
            crate::components::fighter::FighterHitReaction::default(),
            crate::components::fighter::GunAimState::default(),
        ))
        .id();
    spawn_fighter_model_for_test(app.world_mut(), fighter);
    drive_fighter_animation_for_test(app.world_mut());
    let head = app
        .world_mut()
        .query::<(Entity, &FighterBone)>()
        .iter(app.world())
        .find(|(_, b)| b.name == "bau_Head")
        .unwrap()
        .0;
    (app, fighter, camera, head)
}

fn current_head(world: &mut World, head: Entity) -> Vec3 {
    world
        .run_system_once(move |helper: TransformHelper| {
            helper.compute_global_transform(head).unwrap().translation()
        })
        .unwrap()
}

#[test]
fn first_person_tracks_current_animated_head_and_restores_third_person() {
    let (mut app, fighter, camera, head) = fixture();
    app.update();
    assert!(
        !app.world()
            .get::<ThirdPersonState>(camera)
            .unwrap()
            .first_person_active
    );
    app.world_mut().resource_mut::<Settings>().first_person = true;
    for (pose_id, frame, facing, pitch) in [
        (218, 5.0, 0.7, 0.3),
        (93, 12.0, -2.1, -0.4),
        (113, 3.0, 3.5, 1.0),
    ] {
        app.world_mut()
            .resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
                let mut pose = world.get_mut::<PoseRuntime>(fighter).unwrap();
                assets.0["p0"].apply_pose(&mut pose, pose_id, true);
                pose.frame = frame;
            });
        app.world_mut()
            .get_mut::<MotionState>(fighter)
            .unwrap()
            .facing = facing;
        app.world_mut()
            .get_mut::<Transform>(fighter)
            .unwrap()
            .translation += Vec3::new(2.0, 1.0, -3.0);
        app.world_mut()
            .get_mut::<ThirdPersonState>(camera)
            .unwrap()
            .pitch = pitch;
        drive_fighter_animation_for_test(app.world_mut());
        let expected = current_head(app.world_mut(), head);
        let before: Vec<_> = app
            .world_mut()
            .query::<(Entity, &Transform)>()
            .iter(app.world())
            .filter(|(e, _)| *e != camera)
            .map(|(e, t)| (e, *t))
            .collect();
        app.update();
        let view = app.world().get::<Transform>(camera).unwrap();
        assert!(
            view.translation.distance(expected) < 1e-5,
            "same-frame head including root, scale and animation"
        );
        assert!(Vec3::from(view.forward()).distance(view_direction(facing, pitch)) < 1e-5);
        for (entity, transform) in before {
            assert_eq!(
                *app.world().get::<Transform>(entity).unwrap(),
                transform,
                "camera cannot write actor/bones"
            );
        }
        let Projection::Perspective(lens) = app.world().get::<Projection>(camera).unwrap() else {
            panic!()
        };
        assert_eq!(lens.near, NEAR);
    }
    app.world_mut().resource_mut::<Settings>().first_person = false;
    app.update();
    assert!(
        !app.world()
            .get::<ThirdPersonState>(camera)
            .unwrap()
            .first_person_active
    );
    let head_position = current_head(app.world_mut(), head);
    assert!(
        app.world()
            .get::<Transform>(camera)
            .unwrap()
            .translation
            .distance(head_position)
            > 5.0
    );
    let Projection::Perspective(lens) = app.world().get::<Projection>(camera).unwrap() else {
        panic!()
    };
    assert_eq!(lens.near, CAMERA_NEAR);
}

#[test]
fn first_person_keeps_head_anchor_when_locked_and_yields_to_spectator_and_missing_head() {
    let (mut app, _, camera, head) = fixture();
    app.world_mut().resource_mut::<Settings>().first_person = true;
    let target = app
        .world_mut()
        .spawn(Transform::from_xyz(10.0, 8.0, 15.0))
        .id();
    app.world_mut()
        .resource_mut::<LockOnController>()
        .lock_entity = Some(target);
    app.update();
    let expected = current_head(app.world_mut(), head);
    assert!(
        app.world()
            .get::<Transform>(camera)
            .unwrap()
            .translation
            .distance(expected)
            < 1e-5
    );
    app.world_mut()
        .resource_mut::<spectator::SpectatorState>()
        .active = true;
    app.world_mut()
        .resource_mut::<spectator::SpectatorState>()
        .target = Some(target);
    app.update();
    assert!(app.world().resource::<HeadAnchor>().0.is_none());
    app.world_mut()
        .resource_mut::<spectator::SpectatorState>()
        .active = false;
    app.world_mut().despawn(head);
    app.update();
    assert!(
        !app.world()
            .get::<ThirdPersonState>(camera)
            .unwrap()
            .first_person_active
    );
}

#[test]
fn first_person_keeps_body_and_portrait_layers_and_limits_clip_to_local_material() {
    use crate::plugins::stage_material::{GmdlMaterialParams, GmdlMode};
    use bevy::camera::visibility::RenderLayers;
    let (mut app, fighter, _, _) = fixture();
    app.init_resource::<Assets<GmdlMaterial>>();
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 0.37,
                alpha_cutoff: 0.5,
                mode: 0,
                has_texture: 0,
                uv_offset: Vec2::ZERO,
            },
            None,
            true,
            GmdlMode::Opaque,
        ));
    let copy = app
        .world()
        .resource::<Assets<GmdlMaterial>>()
        .get(&material)
        .unwrap()
        .clone();
    let untouched = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(copy);
    let skins: Vec<_> = app
        .world_mut()
        .query::<(Entity, &FighterSkin)>()
        .iter(app.world())
        .filter(|(_, s)| s.fighter == fighter)
        .map(|(e, _)| e)
        .collect();
    assert!(!skins.is_empty());
    for entity in &skins {
        app.world_mut().entity_mut(*entity).insert((
            MeshMaterial3d(material.clone()),
            RenderLayers::default().with(31),
            Visibility::Visible,
        ));
    }
    for enabled in [true, true, false, true] {
        app.world_mut().resource_mut::<Settings>().first_person = enabled;
        app.update();
        for entity in &skins {
            assert_eq!(
                *app.world().get::<RenderLayers>(*entity).unwrap(),
                RenderLayers::default().with(31)
            );
            assert_eq!(
                *app.world().get::<Visibility>(*entity).unwrap(),
                Visibility::Visible
            );
        }
        let assets = app.world().resource::<Assets<GmdlMaterial>>();
        let local = assets.get(&material).unwrap();
        assert_eq!(
            local.first_person_clip.w,
            if enabled { SELF_CLIP_RADIUS } else { 0.0 }
        );
        assert_eq!(
            local.params.opacity, 0.37,
            "authored/hidden material alpha retains its owner"
        );
        assert_eq!(local.params.alpha_cutoff, 0.5);
        assert_eq!(
            assets.get(&untouched).unwrap().first_person_clip,
            Vec4::ZERO
        );
    }
    app.world_mut().resource_mut::<CameraMode>().fly = true;
    app.update();
    let local = app
        .world()
        .resource::<Assets<GmdlMaterial>>()
        .get(&material)
        .unwrap();
    assert_eq!(local.first_person_clip, Vec4::ZERO);
    assert!(!local.camera_occlusion_enabled);
    assert!(app.world().resource::<SelfClipMaterials>().0.is_empty());
}

#[test]
fn first_person_eye_uniform_survives_disabled_scenery_fade_and_clears_on_exit() {
    use crate::plugins::stage_material::{install_camera_uniforms_for_test, FrameUniforms};
    let (mut app, _, camera, head) = fixture();
    app.insert_resource(State::new(AppState::Playing));
    install_camera_uniforms_for_test(&mut app);
    app.world_mut().resource_mut::<Settings>().first_person = true;
    for _ in 0..3 {
        app.world_mut()
            .get_mut::<Transform>(head)
            .unwrap()
            .translation
            .z += 0.2;
        app.update();
        let eye = app.world().get::<Transform>(camera).unwrap().translation;
        let params = app.world().resource::<FrameUniforms>().camera;
        assert_eq!(params.eye.w, 0.0, "scene fading is disabled");
        assert_eq!(params.self_eye, eye.extend(1.0));
    }
    app.world_mut().resource_mut::<Settings>().first_person = false;
    app.update();
    assert_eq!(
        app.world().resource::<FrameUniforms>().camera.self_eye,
        Vec4::ZERO
    );
}
