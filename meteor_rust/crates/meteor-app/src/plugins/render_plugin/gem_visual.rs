use crate::plugins::stage_material::FrameUniforms;
use bevy::prelude::*;
use meteor_core::constants::MODEL_SCALE;

#[derive(Component)]
pub(super) struct GemVisual;

pub(super) fn is_treasure_type(name: Option<&str>) -> bool {
    let name = name.unwrap_or_default().to_ascii_lowercase();
    !matches!(
        name.as_str(),
        "animation"
            | "hell"
            | "safea"
            | "safeb"
            | "safe"
            | "door"
            | "machine"
            | "normal"
            | "targeta"
            | "targetb"
    ) && !name.starts_with("damage")
        && !name.starts_with("user")
        && !name.starts_with("team")
}

pub(super) fn is_attribute_gem(model: &str) -> bool {
    matches!(
        model.to_ascii_lowercase().as_str(),
        "item09" | "item0901" | "item0902" | "item0903" | "item0904"
    )
}

fn gem_transform(parent: &Transform, seconds: f64) -> Transform {
    let height = (3.0 * (0.5 - (5.0 * seconds).sin())) as f32 * MODEL_SCALE;
    let local_offset = parent.rotation.inverse() * (Vec3::Y * height);
    Transform {
        translation: local_offset / parent.scale,
        rotation: Quat::from_rotation_z((2.0 * seconds).rem_euclid(std::f64::consts::TAU) as f32),
        ..default()
    }
}

pub(super) fn update_gem_visuals(
    frame: Res<FrameUniforms>,
    parents: Query<&Transform, Without<GemVisual>>,
    mut visuals: Query<(&ChildOf, &mut Transform), With<GemVisual>>,
) {
    for (parent, mut transform) in &mut visuals {
        if let Ok(base) = parents.get(parent.parent()) {
            if base.scale.abs().min_element() > f32::EPSILON {
                *transform = gem_transform(base, frame.seconds);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gem_motion_matches_original_extrema_and_keeps_pickup_root_fixed() {
        let mut world = World::new();
        world.init_resource::<FrameUniforms>();
        let base = Transform::from_xyz(12.0, 4.0, -7.0)
            .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
            .with_scale(Vec3::splat(MODEL_SCALE * 2.0));
        let parent = world.spawn(base).id();
        let child = world
            .spawn((GemVisual, Transform::default(), ChildOf(parent)))
            .id();
        use bevy::ecs::system::RunSystemOnce;
        for (time, expected_height) in [
            (0.0, 1.5),
            (std::f64::consts::PI / 10.0, -1.5),
            (3.0 * std::f64::consts::PI / 10.0, 4.5),
        ] {
            world.resource_mut::<FrameUniforms>().seconds = time;
            world.run_system_once(update_gem_visuals).unwrap();
            let visual = world.get::<Transform>(child).unwrap();
            let position = base.translation + base.rotation * (base.scale * visual.translation);
            assert!(
                (position - base.translation - Vec3::Y * (expected_height * MODEL_SCALE)).length()
                    < 1e-5
            );
            assert!(visual
                .rotation
                .abs_diff_eq(Quat::from_rotation_z(2.0 * time as f32), 1e-5));
            assert_eq!(*world.get::<Transform>(parent).unwrap(), base);
        }
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn original_gems_spawn_visual_children_and_unique_glow_tracks() {
        use super::super::*;
        use bevy::ecs::system::RunSystemOnce;
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .init_resource::<Assets<Image>>();
        app.world_mut().run_system_once(|mut commands: Commands,
            mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<GmdlMaterial>>,
            mut images: ResMut<Assets<Image>>| {
            let roots = stage_asset_roots(&std::env::current_dir().unwrap());
            let pak_root = pak_files_root(&roots, "sn01");
            spawn_stage_props(&mut commands, &mut meshes, &mut materials, &mut images,
                &mut PakCache::default(), &pak_root, &roots,
                &crate::app_state::LoadedStageInfo { stage_id: "sn01".into(), ..default() },
                Some("SceneObjects 0 DummeyObjects 2\nObject D_itGem1\n{\nPosition: 0 0 0\nQuaternion: 1 0 0 0\nCustom:\n{\nmodel=Item0902\n}\n}\nObject D_itGem2\n{\nPosition: 100 0 0\nQuaternion: 1 0 0 0\nCustom:\n{\nmodel=Item0902\n}\n}\n"),
                0, &mut Vec::new(), &mut StageMeshCollider::default(), None, None);
        }).unwrap();
        let world = app.world_mut();
        let mut visuals =
            world.query_filtered::<(&ChildOf, &MeshMaterial3d<GmdlMaterial>), With<GemVisual>>();
        assert_eq!(visuals.iter(world).count(), 6);
        for (parent, material) in visuals.iter(world) {
            assert!(world
                .get::<crate::plugins::item_plugin::StageProp>(parent.parent())
                .is_some());
            assert!(world
                .resource::<Assets<GmdlMaterial>>()
                .get(&material.0)
                .unwrap()
                .texture
                .is_some());
        }
        let tracks: Vec<_> = world
            .query::<&UvScroll>()
            .iter(world)
            .map(|track| (track.material.id(), track.u, track.v))
            .collect();
        assert_eq!(
            tracks.len(),
            2,
            "shared instances must not multiply UV speed"
        );
        assert_ne!(tracks[0].0, tracks[1].0);
        for (u, v) in [(-0.019, 0.087), (-0.018, 0.027)] {
            assert!(tracks
                .iter()
                .any(|track| (track.1 - u).abs() < 1e-6 && (track.2 - v).abs() < 1e-6));
        }
    }

    #[test]
    fn motion_is_scoped_to_attribute_gems() {
        assert!(is_attribute_gem("Item0902"));
        assert!(is_attribute_gem("Item09"));
        for model in ["Item01", "Item10", "W0_0", "Item09020"] {
            assert!(!is_attribute_gem(model));
        }
    }

    #[test]
    fn treasure_motion_uses_scene_type_not_gem_model_names() {
        for name in [None, Some("treasure"), Some("custom_item")] {
            assert!(is_treasure_type(name));
        }
        for name in [
            "machine",
            "door",
            "normal",
            "animation",
            "hell",
            "damage01",
            "safe",
            "safea",
            "safeb",
            "targeta",
            "targetb",
            "user01",
            "team01",
        ] {
            assert!(!is_treasure_type(Some(name)), "{name}");
        }
    }
}
