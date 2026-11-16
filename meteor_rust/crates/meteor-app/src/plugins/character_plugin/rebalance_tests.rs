use super::*;
use crate::components::fighter::{Health, MotionState, PoseRuntime};
use crate::plugins::network_plugin::{NetState, NetStatus};
use crate::plugins::rebalance_plugin::{
    sync_afterimage, sync_glow, BalanceAfterimage, BalanceGlow,
};
use crate::plugins::stage_material::{GmdlMaterialParams, GmdlMode};
use bevy::render::mesh::skinning::SkinnedMesh;

#[test]
fn rebalance_body_glow_covers_all_skin_groups_and_restores_authored_material() {
    let mut app = App::new();
    app.insert_resource(Time::<()>::default())
        .insert_resource(State::new(crate::app_state::AppState::Playing))
        .insert_resource(NetState {
            status: NetStatus::InRoom,
            battle_rules: meteor_combat::battle_rules::BattleRules::bundled(),
            ..Default::default()
        })
        .init_resource::<Assets<GmdlMaterial>>()
        .add_systems(Update, sync_glow);
    let mut pose = PoseRuntime::idle();
    pose.pose_id = 302;
    let actor = app
        .world_mut()
        .spawn((Health::default(), pose, BalanceGlow::armor(302)))
        .id();
    let other_actor = app.world_mut().spawn_empty().id();
    let mut handles = Vec::new();
    for owner in [actor, actor, other_actor] {
        let material = GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 0.1,
                alpha_cutoff: 0.5,
                mode: GmdlMode::Blend.as_u32(),
                has_texture: 0,
                uv_offset: Vec2::ZERO,
            },
            None,
            true,
            GmdlMode::Blend,
        );
        let handle = app
            .world_mut()
            .resource_mut::<Assets<GmdlMaterial>>()
            .add(material);
        app.world_mut().spawn((
            FighterSkin {
                fighter: owner,
                base_alpha_cutoff: 0.5,
                concealed: true,
            },
            MeshMaterial3d(handle.clone()),
        ));
        handles.push(handle);
    }
    app.update();
    let materials = app.world().resource::<Assets<GmdlMaterial>>();
    let mut armor_style = Vec4::ZERO;
    for handle in &handles[..2] {
        let material = materials.get(handle).unwrap();
        assert!(material.body_glow.x > material.body_glow.z);
        assert!(material.body_glow.w > 0.0);
        assert!(material.body_style.x > 0.0 && material.body_style.z > 0.0);
        assert_eq!(material.params.opacity, 0.1);
        assert_eq!(material.params.alpha_cutoff, 0.5);
        assert_eq!(material.mode, GmdlMode::Blend);
        armor_style = material.body_style;
    }
    assert_eq!(materials.get(&handles[2]).unwrap().body_glow, Vec4::ZERO);
    app.world_mut()
        .entity_mut(actor)
        .insert(BalanceGlow::immunity());
    app.update();
    for handle in &handles[..2] {
        let material = app
            .world()
            .resource::<Assets<GmdlMaterial>>()
            .get(handle)
            .unwrap();
        assert!(material.body_glow.z > material.body_glow.x);

        assert!(material.body_style.x > armor_style.x);
    }
    app.world_mut().entity_mut(actor).remove::<BalanceGlow>();
    app.update();
    for handle in &handles {
        let material = app
            .world()
            .resource::<Assets<GmdlMaterial>>()
            .get(handle)
            .unwrap();
        assert_eq!(material.body_glow, Vec4::ZERO);
        assert_eq!(material.body_style, Vec4::ZERO);
    }
}

#[test]
fn balance_glow_spawns_a_trailing_dark_afterimage_and_tears_it_down() {
    let mut app = App::new();
    app.insert_resource(Time::<()>::default())
        .insert_resource(State::new(crate::app_state::AppState::Playing))
        .insert_resource(NetState {
            status: NetStatus::InRoom,
            battle_rules: meteor_combat::battle_rules::BattleRules::bundled(),
            ..Default::default()
        })
        .init_resource::<Assets<GmdlMaterial>>()
        .add_systems(PostUpdate, sync_afterimage);

    let fighter = app
        .world_mut()
        .spawn((
            BalanceGlow::armor(302),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
        ))
        .id();
    let source_a = app
        .world_mut()
        .spawn(GlobalTransform::from_xyz(0.0, 1.0, 0.0))
        .id();
    let source_b = app
        .world_mut()
        .spawn(GlobalTransform::from_xyz(0.0, 2.0, 0.0))
        .id();
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.0,
                mode: GmdlMode::Opaque.as_u32(),
                has_texture: 0,
                uv_offset: Vec2::ZERO,
            },
            None,
            false,
            GmdlMode::Opaque,
        ));
    let model = app
        .world_mut()
        .spawn((
            FighterModel {
                fighter,
                character: "p0".into(),
            },
            GlobalTransform::from_xyz(0.0, 0.0, 0.0),
        ))
        .id();
    app.world_mut().spawn((
        FighterSkin {
            fighter,
            base_alpha_cutoff: 0.0,
            concealed: false,
        },
        Mesh3d(Handle::<Mesh>::default()),
        MeshMaterial3d(material),
        SkinnedMesh {
            inverse_bindposes: Handle::default(),
            joints: vec![source_a, source_b],
        },
        ChildOf(model),
    ));

    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(100));
    app.update();
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_millis(100));
    app.update();

    let host = app
        .world()
        .get::<BalanceAfterimage>(model)
        .expect("afterimage host");
    assert_eq!(host.source_joints.len(), 2);
    assert_eq!(host.ghost_joints.len(), 2);
    assert_eq!(host.ghost_meshes.len(), 1);
    let ghost = host.ghost_joints[0];
    let ghost_pose = app.world().get::<GlobalTransform>(ghost).unwrap();

    assert!(ghost_pose.translation().distance(Vec3::new(0.0, 1.0, 0.0)) > 0.05);
    assert!(ghost_pose.translation().z < 0.0);

    assert_eq!(
        app.world().get::<Visibility>(host.ghost_meshes[0]),
        Some(&Visibility::Visible)
    );

    app.world_mut().entity_mut(fighter).remove::<BalanceGlow>();
    app.update();
    assert!(app.world().get::<BalanceAfterimage>(model).is_none());
    assert!(app.world().get_entity(ghost).is_err());
}
