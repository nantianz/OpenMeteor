use super::super::{
    load_character_asset_for_test, spawn_fighter_model_for_test, CharacterAssetsSet, CommonAmb,
    FighterBone, FighterDummy,
};
use super::*;
use crate::components::fighter::*;
use crate::plugins::combat_plugin::HitGeometryQueries;
use bevy::ecs::system::RunSystemOnce;
use meteor_combat::hit_reaction::HitReact;

fn reaction(pose_id: u16) -> FighterHitReaction {
    let mut runtime = FighterHitReaction::default();
    runtime.begin(
        HitReact {
            kind: HitReactKind::BeHit,
            pose_id: Some(pose_id),
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
    runtime
}

fn pose() -> PoseRuntime {
    PoseRuntime {
        pose_id: 93,
        end: 30,
        loops: false,
        ..PoseRuntime::idle()
    }
}

#[test]
fn hit_sway_is_bounded_and_releases_at_every_gameplay_boundary() {
    let mut pose = pose();
    let mut reaction = reaction(93);
    let mut sway = Sway::default();
    for i in 0..100 {
        pose.pose_instance_id = i;
        let angle = sway.sample(true, 1.0 / 120.0, &pose, &reaction);
        assert!(angle.is_finite() && angle.length() <= MAX_ANGLE);
    }
    for _ in 0..40 {
        sway.sample(true, 1.0 / 120.0, &pose, &reaction);
    }
    assert_eq!(sway.sample(true, 0.0, &pose, &reaction), Vec2::ZERO);
    let mut linked_pose = pose.clone();
    linked_pose.pose_id = 94;
    let mut linked_reaction = reaction.clone();
    linked_reaction.current.as_mut().unwrap().pose_id = Some(94);
    assert_eq!(
        sway.sample(true, 0.02, &linked_pose, &linked_reaction),
        Vec2::ZERO,
        "natural POS links cannot retrigger the impulse"
    );
    for kind in [
        HitReactKind::Guard,
        HitReactKind::Down,
        HitReactKind::GetUp,
        HitReactKind::Roll,
        HitReactKind::Dead,
    ] {
        let mut sway = Sway::default();
        reaction.current.as_mut().unwrap().kind = HitReactKind::BeHit;
        assert_ne!(sway.sample(true, 0.02, &pose, &reaction), Vec2::ZERO);
        reaction.current.as_mut().unwrap().kind = kind;
        assert_eq!(sway.sample(true, 0.02, &pose, &reaction), Vec2::ZERO);
    }
    reaction.current.as_mut().unwrap().kind = HitReactKind::BeHit;
    for boundary in 0..4 {
        let mut sway = Sway::default();
        let mut next = pose.clone();
        let mut runtime = reaction.clone();
        runtime.airborne = true;
        assert_ne!(sway.sample(true, 0.02, &next, &runtime), Vec2::ZERO);
        match boundary {
            0 => runtime.airborne = false,
            1 => next.pose_id = 218,
            2 => next.frame = next.end as f32,
            _ => {}
        }
        assert_eq!(
            sway.sample(boundary != 3, 0.02, &next, &runtime),
            Vec2::ZERO
        );
    }
}

#[test]
fn hit_sway_is_frame_rate_independent_and_freezes_with_simulation() {
    let mut angles = Vec::new();
    for fps in [30, 60, 120, 240] {
        let mut sway = Sway::default();
        let mut angle = Vec2::ZERO;
        for _ in 0..fps / 10 {
            angle = sway.sample(true, 1.0 / fps as f32, &pose(), &reaction(93));
        }
        assert_eq!(sway.sample(true, 0.0, &pose(), &reaction(93)), angle);
        angles.push(angle);
    }
    assert!(angles[0].length() > 0.001);
    assert!(angles
        .iter()
        .all(|angle| (*angle - angles[0]).length() < 1e-6));
}

fn fixture() -> (App, Entity, Entity) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let character = load_character_asset_for_test(std::slice::from_ref(&root), "p1").unwrap();
    let common =
        meteor_asset::amb::parse_amb(&std::fs::read(root.join("Common/character.amb")).unwrap())
            .unwrap();
    let mut pose = PoseRuntime::idle();
    character.apply_pose(&mut pose, 93, true);
    let mut app = App::new();
    app.add_plugins(TransformPlugin)
        .init_resource::<Time>()
        .init_resource::<Time<Real>>()
        .insert_resource(Settings::default())
        .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p1".into(),
            character,
        )])))
        .insert_resource(CommonAmb(common))
        .add_systems(Update, super::super::drive_fighter_animation);
    install(&mut app);
    let fighter = app
        .world_mut()
        .spawn((
            Fighter {
                character_id: 1,
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
            EntryBlend::default(),
            SmState::default(),
            GunAimState::default(),
            reaction(93),
        ))
        .id();
    spawn_fighter_model_for_test(app.world_mut(), fighter);
    let model = app
        .world_mut()
        .query_filtered::<Entity, With<RenderHitSkeleton>>()
        .single(app.world())
        .unwrap();
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
    app.update();
    (app, fighter, model)
}

fn hurt_boxes(
    assets: Res<CharacterAssetsSet>,
    geometry: HitGeometryQueries,
    fighters: Query<Entity, With<Fighter>>,
) -> Vec<[f32; 15]> {
    geometry
        .hurt_obbs(fighters.single().unwrap(), &assets.0["p1"])
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
fn hit_sway_real_skin_changes_but_authored_bones_boxes_and_clocks_do_not() {
    let (mut app, fighter, model) = fixture();

    let original_joints = app
        .world()
        .get::<RenderHitSkeleton>(model)
        .unwrap()
        .original_joints
        .clone();
    let render_joints = app
        .world()
        .get::<RenderHitSkeleton>(model)
        .unwrap()
        .render_joints
        .clone();
    assert!(!original_joints.is_empty());
    for entity in &render_joints {
        assert!(app.world().get::<Transform>(*entity).is_none());
        assert!(app.world().get::<FighterBone>(*entity).is_none());
        assert!(app.world().get::<FighterDummy>(*entity).is_none());
    }
    let baseline: Vec<_> = original_joints
        .iter()
        .map(|entity| {
            (
                *app.world().get::<Transform>(*entity).unwrap(),
                *app.world().get::<GlobalTransform>(*entity).unwrap(),
            )
        })
        .collect();
    let boxes = app.world_mut().run_system_once(hurt_boxes).unwrap();
    assert!(!boxes.is_empty());
    let position = *app.world().get::<Transform>(fighter).unwrap();
    app.world_mut().resource_mut::<Settings>().hit_sway = true;

    app.world_mut()
        .get_mut::<PoseRuntime>(fighter)
        .unwrap()
        .pose_instance_id += 1;
    let pose_before = format!("{:?}", app.world().get::<PoseRuntime>(fighter).unwrap());
    let reaction_before = format!(
        "{:?}",
        app.world().get::<FighterHitReaction>(fighter).unwrap()
    );
    app.update();
    assert!(app
        .world_mut()
        .query::<&SkinnedMesh>()
        .iter(app.world())
        .all(|skin| skin.joints == render_joints));
    assert!(original_joints.iter().zip(&render_joints).any(|(a, b)| app
        .world()
        .get::<GlobalTransform>(*a)
        .unwrap()
        .to_matrix()
        != app.world().get::<GlobalTransform>(*b).unwrap().to_matrix()));

    for _ in 0..3 {
        app.update();
        assert_eq!(*app.world().get::<Transform>(fighter).unwrap(), position);
        assert_eq!(
            format!("{:?}", app.world().get::<PoseRuntime>(fighter).unwrap()),
            pose_before
        );
        assert_eq!(
            format!(
                "{:?}",
                app.world().get::<FighterHitReaction>(fighter).unwrap()
            ),
            reaction_before
        );
        assert_eq!(app.world_mut().run_system_once(hurt_boxes).unwrap(), boxes);
        for (entity, (local, global)) in original_joints.iter().zip(&baseline) {
            assert_eq!(app.world().get::<Transform>(*entity).unwrap(), local);
            assert_eq!(app.world().get::<GlobalTransform>(*entity).unwrap(), global);
        }
    }

    let skeleton = app.world().get::<RenderHitSkeleton>(model).unwrap();
    for node in &skeleton.nodes {
        if let Some(bone) = app.world().get::<FighterBone>(node.source) {
            if bone.name.starts_with("bad_") {
                assert_eq!(
                    app.world().get::<GlobalTransform>(node.source),
                    app.world().get::<GlobalTransform>(node.render)
                );
            }
        }
    }
    app.world_mut().resource_mut::<Settings>().hit_sway = false;
    app.update();
    assert!(app
        .world_mut()
        .query::<&SkinnedMesh>()
        .iter(app.world())
        .all(|skin| skin.joints == original_joints));
    app.world_mut().despawn(model);
    assert!(render_joints
        .iter()
        .all(|entity| app.world().get_entity(*entity).is_err()));
}
