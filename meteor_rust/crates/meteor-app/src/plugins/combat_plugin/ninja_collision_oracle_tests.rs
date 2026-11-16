use super::*;

#[derive(Resource, Default)]
struct NinjaGeometryAudit(Vec<serde_json::Value>);

pub(super) fn install(app: &mut App) {
    if std::env::var_os("METEOR_NINJA_GEOMETRY_DIR").is_none() {
        return;
    }
    app.init_resource::<NinjaGeometryAudit>();
    let schedule = crate::plugins::fighter_frame::schedule(app);
    app.add_systems(
        schedule,
        capture_geometry
            .before(attack_windows)
            .in_set(crate::plugins::FighterFrameSet::HitJudge),
    );
}

pub(super) fn save(world: &World, fps: u32, contacts: bool, distance: f32) {
    let Some(directory) = std::env::var_os("METEOR_NINJA_GEOMETRY_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let data = serde_json::json!({"fps":fps, "contacts":contacts, "distance":distance,
        "frames": world.resource::<NinjaGeometryAudit>().0});
    std::fs::write(
        directory.join(format!("ninja-{fps}-{contacts}-{distance}.json")),
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
}

fn capture_geometry(
    active: Option<Res<crate::plugins::fighter_frame::ActiveFighter>>,
    assets: Res<CharacterAssetsSet>,
    geometry: HitGeometryQueries,
    fighters: Query<(Entity, &Fighter, &WeaponForm, &PoseRuntime)>,
    mut audit: ResMut<NinjaGeometryAudit>,
) {
    let convert = (Mat4::from_scale(Vec3::splat(meteor_core::MODEL_SCALE))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2))
    .inverse();
    let vec = |v: meteor_core::Vec3| Vec3::new(v.x, v.y, v.z);
    let obb = |b: &Obb| {
        serde_json::json!({
            "center": convert.transform_point3(vec(b.center)).to_array(),
            "axes": b.axes.map(|a| convert.transform_vector3(vec(a)).normalize().to_array()),
            "half": (vec(b.half_size) / meteor_core::MODEL_SCALE).to_array(),
        })
    };
    let skeleton = |entity: Entity, asset: &CharacterAssets| {
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
                .and_then(|b| {
                    bone_world_obb(&bone_world_matrix(asset, bone, &animated).unwrap(), b)
                });
            bones.push(
                serde_json::json!({"index":bone.skin_index, "name":bone.name,
                "animated":(convert * animated.to_matrix()).to_cols_array(),
                "inverse_bind":asset.skeleton.skin_world_bind_inverse[bone.skin_index].m,
                "obb":bound.as_ref().map(&obb)}),
            );
        }
        bones.sort_by_key(|b| b["index"].as_u64().unwrap());
        bones
    };
    for (entity, fighter, form, pose) in &fighters {
        if !crate::plugins::fighter_frame::accepts(active.as_deref(), entity) || pose.pose_id != 471
        {
            continue;
        }
        let asset = &assets.0[&format!("p{}", fighter.character_id)];
        let Some((_, window)) = active_attack_window(pose.frame, &asset.poses[&471].attacks) else {
            continue;
        };
        let bodies =
            geometry.attack_obbs(entity, fighter, form, pose.pose_id, asset, &window.bones);
        let effects = geometry.live_effect_obbs(entity, fighter, form, pose.pose_id, asset);
        for (target, defender, _, target_pose) in &fighters {
            if target == entity {
                continue;
            }
            let target_asset = &assets.0[&format!("p{}", defender.character_id)];
            let hurt = fighter_body_hurt_obbs(target, target_asset, &geometry);
            let whole = fighter_whole_body_obb(target, target_asset, &geometry).unwrap();
            audit.0.push(serde_json::json!({"frame":pose.frame,
                "character":fighter.character_id, "target_character":defender.character_id,
                "target_pose":target_pose.pose_id,"target_frame":target_pose.frame,
                "source_bones":skeleton(entity, asset),"target_bones":skeleton(target, target_asset),
                "attack_names":window.bones,"effects":effects.iter().map(&obb).collect::<Vec<_>>(),
                "whole":obb(&whole),
                "rust_body_hit":bodies.iter().any(|a|hurt.iter().any(|b|obbs_intersect(a,b))),
                "rust_effect_hit":effects.iter().any(|a|obbs_intersect(a,&whole))}));
        }
    }
}

#[test]
#[ignore = "exports six conditional Ninja geometry cases for the stock EXE oracle"]
fn export_ninja_collision_geometry() {
    assert!(std::env::var_os("METEOR_NINJA_GEOMETRY_DIR").is_some());
    run_ninja_spacing(&[60, 120], &[true], &[2.0, 3.0, 4.0], false);
}
