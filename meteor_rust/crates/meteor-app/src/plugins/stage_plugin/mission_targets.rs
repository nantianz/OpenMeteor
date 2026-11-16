use super::{StageMissionRuntime, StageScriptRuntime};
use crate::app_state::{LoadedStageAssets, LoadedStageInfo};
use crate::plugins::{
    escort_assets::StageAssetReader,
    hazard_plugin::HazardPart,
    item_plugin::{SceneRecordOrder, StageProp},
    render_plugin::{load_prop_model_with, model_stems, prop_transform},
};
use bevy::prelude::*;
use meteor_asset::{des::StageDescription, gmdl::GmdlMesh, res::MeteorRes};
use meteor_scene::{
    hazards::{Aabb, StageHazards},
    pst_runtime::PstValue,
};
use std::collections::HashMap;

struct SceneTargetRecord {
    order: usize,
    dummy_index: Option<usize>,
    script_type: String,
    flag: bool,
    center: Option<Vec3>,
}

#[derive(Default)]
pub(super) struct SceneTargetCache {
    key: Option<(String, [u32; 3], Option<u32>)>,
    records: Vec<SceneTargetRecord>,
}

fn union(bounds: &mut Option<Aabb>, next: Aabb) {
    if !next
        .min
        .iter()
        .chain(&next.max)
        .all(|value| value.is_finite())
    {
        return;
    }
    if let Some(bounds) = bounds {
        for axis in 0..3 {
            bounds.min[axis] = bounds.min[axis].min(next.min[axis]);
            bounds.max[axis] = bounds.max[axis].max(next.max[axis]);
        }
    } else {
        *bounds = Some(next);
    }
}

fn midpoint(bounds: Aabb) -> Vec3 {
    (Vec3::from_array(bounds.min) + Vec3::from_array(bounds.max)) * 0.5
}

fn build_records(
    desc: &StageDescription,
    mesh: &GmdlMesh,
    registry: Option<&MeteorRes>,
    center: [f32; 3],
    mut load_model: impl FnMut(&str) -> Option<GmdlMesh>,
) -> Vec<SceneTargetRecord> {
    let mut records = Vec::new();
    for (order, object) in desc.scene_objects.iter().enumerate() {
        if !object.ticket_admitted {
            continue;
        }
        let center = mesh
            .objects
            .iter()
            .find(|part| part.name.eq_ignore_ascii_case(&object.name))
            .and_then(|part| {
                StageHazards::gmdl_object_world_aabb(
                    &part.positions,
                    center,
                    meteor_core::MODEL_SCALE,
                )
            })
            .map(midpoint);
        records.push(SceneTargetRecord {
            order,
            dummy_index: None,
            script_type: object
                .name_attr
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase(),
            flag: false,
            center,
        });
    }
    let mut models = HashMap::<String, Option<GmdlMesh>>::new();
    for (index, object) in desc.dummies.iter().enumerate() {
        if !object.ticket_admitted {
            continue;
        }
        let mut bounds = None;
        if let Some(model) = object.model.as_deref() {
            if let Some(mesh) = models
                .entry(model.to_ascii_lowercase())
                .or_insert_with(|| load_model(model))
                .as_ref()
            {
                let transform = prop_transform(object, center);
                for part in &mesh.objects {
                    if let Some(part) =
                        HazardPart::from_positions(&part.positions).world_box(&transform)
                    {
                        union(&mut bounds, part);
                    }
                }
            }
        }
        let flag = registry.is_some_and(|registry| {
            registry.records.iter().any(|record| {
                record.category == 4
                    && object.resource_key.map_or_else(
                        || {
                            object
                                .model
                                .as_deref()
                                .is_some_and(|model| record.name.eq_ignore_ascii_case(model))
                        },
                        |key| key == [record.category, record.id],
                    )
            })
        });
        records.push(SceneTargetRecord {
            order: desc.scene_objects.len() + index,
            dummy_index: Some(index),
            script_type: object
                .name_attr
                .as_deref()
                .unwrap_or("")
                .to_ascii_lowercase(),
            flag,
            center: bounds.map(midpoint),
        });
    }
    records
}

pub(super) fn sync_mission_scene_targets(
    assets: Option<Res<LoadedStageAssets>>,
    info: Option<Res<LoadedStageInfo>>,
    scene: Option<Res<StageScriptRuntime>>,
    mut mission: ResMut<StageMissionRuntime>,
    props: Query<(
        &StageProp,
        &Transform,
        &SceneRecordOrder,
        Option<&HazardPart>,
    )>,
    mut cache: Local<SceneTargetCache>,
) {
    let Some(runtime) = mission.script_mut() else {
        cache.key = None;
        cache.records.clear();
        return;
    };
    let host = runtime.host_mut();
    host.begin_scene_target_sync();
    let (Some(assets), Some(info)) = (assets, info) else {
        return;
    };
    let key = (
        info.stage_id.clone(),
        info.center.map(f32::to_bits),
        assets.ticket_mask,
    );
    if cache.key.as_ref() != Some(&key) || assets.is_changed() {
        cache.records.clear();
        if let Some((des, mesh)) =
            assets
                .des
                .as_deref()
                .zip(assets.gmb.as_deref())
                .and_then(|(des, bytes)| {
                    meteor_asset::gmdl::decode_gmdl(bytes)
                        .ok()
                        .map(|mesh| (des, mesh))
                })
        {
            let mut desc =
                meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(des));
            if let Some(mask) = assets.ticket_mask {
                desc.apply_ticket_mask(mask);
            }
            let reader = StageAssetReader::new(&info.stage_id);
            let registry = reader
                .read("Meteor.res")
                .and_then(|bytes| meteor_asset::res::parse_meteor_res(&bytes).ok());
            cache.records = build_records(&desc, &mesh, registry.as_ref(), info.center, |model| {
                load_prop_model_with(&model_stems(model), |path| reader.read(path))
            });
        }
        cache.key = Some(key);
    }
    let mut selected = Vec::new();
    for record in &cache.records {
        let instance = if let Some(index) = record.dummy_index {
            match scene.as_deref().and_then(StageScriptRuntime::script) {
                Some(scene) => {
                    let Some(instance) = scene.host().instance(index) else {
                        continue;
                    };
                    Some(instance)
                }
                None => None,
            }
        } else {
            None
        };
        let script_type = instance
            .and_then(|instance| instance.props.get("script_type"))
            .and_then(|value| match value {
                PstValue::Str(value) => Some(value.as_str()),
                _ => None,
            })
            .unwrap_or(&record.script_type);
        let kind = if script_type.eq_ignore_ascii_case("safea") {
            Some(("safe", 1))
        } else if script_type.eq_ignore_ascii_case("safeb") {
            Some(("safe", 2))
        } else if script_type.eq_ignore_ascii_case("door") {
            Some(("door", 0))
        } else {
            None
        };
        if record.flag || kind.is_some() {
            selected.push((record, kind));
        }
    }
    let mut live = HashMap::<usize, Option<Aabb>>::new();
    for (prop, transform, order, part) in &props {
        if !selected.iter().any(|(record, _)| record.order == order.0) {
            continue;
        }

        let bounds = part
            .and_then(|part| part.world_box(transform))
            .unwrap_or_else(|| {
                let center = prop.world_center(transform);
                let rotation = transform.rotation;
                let half = (rotation * Vec3::X * prop.half_extents.x).abs()
                    + (rotation * Vec3::Y * prop.half_extents.y).abs()
                    + (rotation * Vec3::Z * prop.half_extents.z).abs();
                Aabb {
                    min: (center - half).to_array(),
                    max: (center + half).to_array(),
                }
            });
        union(live.entry(order.0).or_default(), bounds);
    }
    for (record, kind) in selected {
        let Some(center) = live
            .get(&record.order)
            .copied()
            .flatten()
            .map(midpoint)
            .or(record.center)
        else {
            continue;
        };

        if record.flag {
            host.sync_scene_target("flag", 0, record.order, center.to_array());
        }
        if let Some((kind, team)) = kind {
            host.sync_scene_target(kind, team, record.order, center.to_array());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_scene::{
        mission_runtime::{parse_mission_definition, MissionScriptRuntime},
        pst_runtime::PstHost,
        scene_script_runtime::{SceneItemInstance, SceneScriptRuntime},
    };

    fn original_assets() -> (
        LoadedStageAssets,
        LoadedStageInfo,
        StageDescription,
        GmdlMesh,
        MeteorRes,
    ) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let des = std::fs::read(root.join("sn03/sn03.des")).unwrap();
        let gmb = std::fs::read(root.join("sn03/sn03.gmb")).unwrap();
        let desc =
            meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&des));
        let mesh = meteor_asset::gmdl::decode_gmdl(&gmb).unwrap();
        let registry =
            meteor_asset::res::parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap())
                .unwrap();
        let center = meteor_scene::stage_loader::get_stage_center(
            "sn03",
            Some(&std::fs::read_to_string(root.join("Level/sn03.wp")).unwrap()),
        );
        (
            LoadedStageAssets {
                des: Some(des),
                gmb: Some(gmb),
                ..default()
            },
            LoadedStageInfo {
                stage_id: "sn03".into(),
                ready: true,
                center,
                ..default()
            },
            desc,
            mesh,
            registry,
        )
    }

    fn target(app: &mut App, kind: &str, team: i64) -> Option<[f64; 3]> {
        let mut mission = app.world_mut().resource_mut::<StageMissionRuntime>();
        let host = mission.script_mut().unwrap().host_mut();
        let success = host.builtin(
            "SetTarget",
            &[
                PstValue::Num(0.0),
                PstValue::Str(kind.into()),
                PstValue::Num(team as f64),
            ],
        );
        (success.as_f64() == 1.0).then(|| host.target_position(0).unwrap())
    }

    fn close(actual: [f64; 3], expected: Vec3) {
        for axis in 0..3 {
            assert!(
                (actual[axis] - expected[axis] as f64).abs() < 0.001,
                "{actual:?} != {expected:?}"
            );
        }
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn mission_targets_sn03_are_ready_before_rendering_or_mission_start() {
        let (assets, info, desc, mesh, registry) = original_assets();
        let reader = StageAssetReader::new("sn03");
        let escort = meteor_network::escort_definition::EscortDefinition::from_assets(
            &desc,
            &mesh,
            &registry,
            info.center,
            |model| load_prop_model_with(&model_stems(model), |path| reader.read(path)),
        )
        .unwrap();
        let source =
            "OnStart(){ SetTarget(0,\"safe\",1); SetTarget(1,\"safe\",2); SetTarget(2,\"flag\"); }";
        let mut app = App::new();
        app.insert_resource(assets)
            .insert_resource(info)
            .insert_resource(StageMissionRuntime(Some(MissionScriptRuntime::new(
                source,
                parse_mission_definition(source).values,
            ))))
            .add_systems(Update, sync_mission_scene_targets);
        app.update();
        app.world_mut()
            .resource_mut::<StageMissionRuntime>()
            .script_mut()
            .unwrap()
            .start()
            .unwrap();
        for (slot, team) in [(0, 1), (1, 2)] {
            let safe = escort
                .stage
                .safe_zones
                .iter()
                .find(|safe| safe.team == team)
                .unwrap();
            let position = app
                .world()
                .resource::<StageMissionRuntime>()
                .script()
                .unwrap()
                .host()
                .target_position(slot)
                .unwrap();
            close(
                position,
                Vec3::new(safe.center_x, safe.center_y + 2.4, safe.center_z),
            );
        }
        let flag = app
            .world()
            .resource::<StageMissionRuntime>()
            .script()
            .unwrap()
            .host()
            .target_position(2)
            .unwrap();
        let min = escort
            .vertices
            .iter()
            .map(|point| Vec3::from_array(*point))
            .fold(Vec3::splat(f32::INFINITY), Vec3::min);
        let max = escort
            .vertices
            .iter()
            .map(|point| Vec3::from_array(*point))
            .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
        let home = escort.stage.flag_home;
        close(
            flag,
            Vec3::new(home.x, home.y, home.z) + (min + max) * 0.5 + Vec3::Y * 2.4,
        );
        assert!(
            target(&mut app, "safe", 0).is_none(),
            "common safe is not a team selector fallback"
        );
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn mission_target_records_use_resource_identity_ticket_and_authored_transforms() {
        let (_, _, _, _, registry) = original_assets();
        let mut desc = meteor_asset::des::parse_stage_description(
            r#"
SceneObjects 1 DummeyObjects 3
Object SafeFirst {
Position: 0 0 0
Quaternion: 1 0 0 0
Custom: { name="safeA" visible=0 }
}
Object D_Renamed {
Position: 100 200 300
Quaternion: 0.70710678 0 0 0.70710678
Custom: { model="Renamed" resource=4,2 name="door" scale=2 ticket="2" }
}
Object D_Excluded {
Position: 0 0 0
Quaternion: 1 0 0 0
Custom: { model="Renamed" resource=4,2 ticket="6" }
}
Object D_NameOnly {
Position: 0 0 0
Quaternion: 1 0 0 0
Custom: { model="Renamed" name="flag" ticket="2" }
}
"#,
        );
        desc.apply_ticket_mask(1 << 2);
        let part = |name: &str, positions: Vec<f32>| meteor_asset::gmdl::GmdlObject {
            name: name.into(),
            num_verts: positions.len() / 3,
            num_faces: 0,
            positions,
            normals: vec![],
            uvs: vec![],
            colors: vec![],
            face_mat: vec![],
            indices: vec![],
        };
        let mesh = GmdlMesh {
            textures: vec![],
            materials: vec![],
            objects: vec![part("SafeFirst", vec![0.0, 0.0, 0.0, 10.0, 20.0, 30.0])],
        };
        let mut loads = 0;
        let records = build_records(&desc, &mesh, Some(&registry), [1.0, 2.0, 3.0], |name| {
            assert_eq!(name, "Renamed");
            loads += 1;
            Some(GmdlMesh {
                textures: vec![],
                materials: vec![],
                objects: vec![part("box", vec![2.0, 4.0, 6.0, 10.0, 12.0, 14.0])],
            })
        });
        assert_eq!(
            loads, 1,
            "one model parse shared by all its admitted instances"
        );
        assert_eq!(
            records
                .iter()
                .map(|record| record.order)
                .collect::<Vec<_>>(),
            [0, 1, 3]
        );
        assert_eq!(records[0].script_type, "safea");
        close(
            records[0].center.unwrap().to_array().map(f64::from),
            Vec3::new(0.48, 1.44, -0.96),
        );
        assert!(
            records[1].flag,
            "explicit resource identity survives a renamed model"
        );
        assert_eq!(
            records[1].script_type, "door",
            "flag matching is independent of contact type"
        );
        close(
            records[1].center.unwrap().to_array().map(f64::from),
            Vec3::new(9.96, 38.04, -25.2),
        );
        assert!(
            !records[2].flag,
            "name=flag alone does not create a resource category-4 object"
        );
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn mission_targets_live_parts_keep_order_hidden_state_and_runtime_classification() {
        let (assets, info, desc, _, _) = original_assets();
        let indices: Vec<_> = desc
            .dummies
            .iter()
            .enumerate()
            .filter(|(_, dummy)| dummy.model.is_some())
            .take(2)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(indices.len(), 2);
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, dummy)| SceneItemInstance {
                name: dummy.name.clone(),
                index,
                props: super::super::scene_item_props_from_dummy(dummy),
            })
            .collect();
        let script = SceneScriptRuntime::load("", instances, 10000).unwrap();
        let mut app = App::new();
        app.insert_resource(assets)
            .insert_resource(info)
            .insert_resource(StageScriptRuntime(Some(script)))
            .insert_resource(StageMissionRuntime(Some(MissionScriptRuntime::new(
                "",
                Default::default(),
            ))))
            .add_systems(Update, sync_mission_scene_targets);
        for &index in &indices {
            assert!(!desc.dummies[index]
                .name_attr
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case("door")));
            let mut script = app.world_mut().resource_mut::<StageScriptRuntime>();
            let instance = script
                .script_mut()
                .unwrap()
                .host_mut()
                .instance_mut(index)
                .unwrap();
            instance.set("script_type", PstValue::Str("door".into()));
            instance.set("active", PstValue::Num(0.0));
        }

        app.update();
        assert!(
            target(&mut app, "door", 0).is_some(),
            "runtime type selects resource geometry before any ECS prop exists"
        );
        let mut parts = Vec::new();

        for (&index, x) in indices.iter().rev().zip([500.0, 10.0]) {
            for y in [0.0, 8.0, 8.0] {
                parts.push(
                    app.world_mut()
                        .spawn((
                            StageProp {
                                name: desc.dummies[index].name.clone(),
                                model: "test".into(),
                                center_offset: Vec3::ZERO,
                                half_extents: Vec3::ONE,
                                hit_count: 0,
                                respawn_timer: 10.0,
                                interactive: false,
                                collision_enabled: false,
                                breakable: false,
                                broken: true,
                            },
                            SceneRecordOrder(desc.scene_objects.len() + index),
                            Transform::from_translation(Vec3::new(x, y, 20.0)),
                            Visibility::Hidden,
                            HazardPart::from_positions(&[-1.0, -1.0, -1.0, 1.0, 1.0, 1.0]),
                        ))
                        .id(),
                );
            }
        }
        app.update();
        close(
            target(&mut app, "door", 0).unwrap(),
            Vec3::new(10.0, 6.4, 20.0),
        );

        app.world_mut()
            .get_mut::<Transform>(parts[3])
            .unwrap()
            .translation
            .y = -4.0;
        app.update();
        close(
            target(&mut app, "door", 0).unwrap(),
            Vec3::new(10.0, 4.4, 20.0),
        );
        {
            let mut script = app.world_mut().resource_mut::<StageScriptRuntime>();
            script
                .script_mut()
                .unwrap()
                .host_mut()
                .instance_mut(indices[0])
                .unwrap()
                .set("script_type", PstValue::Str("normal".into()));
        }
        app.update();
        close(
            target(&mut app, "door", 0).unwrap(),
            Vec3::new(500.0, 6.4, 20.0),
        );
        app.world_mut()
            .resource_mut::<StageScriptRuntime>()
            .script_mut()
            .unwrap()
            .host_mut()
            .exclude_instances([indices[1]]);
        app.update();
        assert!(target(&mut app, "door", 0).is_none());
    }
}
