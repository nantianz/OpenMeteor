use crate::escort::{EscortStage, SafeZone};
use crate::role_state::RoleVec3;
use meteor_asset::{des::StageDescription, gmdl::GmdlMesh, res::MeteorRes};

#[derive(Debug, Clone, PartialEq)]
pub struct EscortDefinition {
    pub stage: EscortStage,
    pub scene_name: String,
    pub scene_object_id: i32,
    pub item_id: u32,
    pub model: String,
    pub vertices: Vec<[f32; 3]>,
    pub indices: Vec<usize>,
}

impl EscortDefinition {
    pub fn intersects(&self, player: RoleVec3, position: RoleVec3) -> bool {
        crate::escort::character_capsule_intersects_mesh(
            player,
            position,
            &self.vertices,
            &self.indices,
        )
    }

    pub fn from_assets(
        desc: &StageDescription,
        stage_mesh: &GmdlMesh,
        registry: &MeteorRes,
        center: [f32; 3],
        mut load_model: impl FnMut(&str) -> Option<GmdlMesh>,
    ) -> Result<Self, String> {
        let candidates: Vec<_> = desc
            .dummies
            .iter()
            .enumerate()
            .filter_map(|(index, object)| {
                let model = object.model.as_deref()?;
                let record = registry.records.iter().find(|r| {
                    r.category == 4
                        && object.resource_key.map_or_else(
                            || r.name.eq_ignore_ascii_case(model),
                            |key| key == [r.category, r.id],
                        )
                })?;
                let admitted = object.ticket.as_deref().is_none_or(|ticket| {
                    ticket
                        .split(',')
                        .any(|rule| matches!(rule.trim(), "2" | "6"))
                });
                admitted.then_some((index, object, record))
            })
            .collect();
        let [(index, object, record)] = candidates.as_slice() else {
            return Err(format!(
                "劫镖地图必须配置一个 category-4 镖物，当前为 {} 个",
                candidates.len()
            ));
        };
        let model = object.model.as_deref().expect("candidate has model");
        let mesh = load_model(model).ok_or_else(|| format!("无法加载镖物模型 {model}"))?;
        let home = world(
            [object.position.x, object.position.y, object.position.z],
            center,
        );
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let q = object.quaternion;
        let rotation = [q.x, q.y, q.z, q.w];
        for part in &mesh.objects {
            let base = vertices.len();
            for p in part.positions.chunks_exact(3) {
                let p = rotate(
                    [
                        p[0] * object.scale,
                        p[1] * object.scale,
                        p[2] * object.scale,
                    ],
                    rotation,
                );
                vertices.push([p[0] * 0.12, p[2] * 0.12, -p[1] * 0.12]);
            }
            indices.extend(part.indices.iter().map(|i| base + *i as usize));
        }
        if vertices.is_empty()
            || indices.is_empty()
            || indices.len() % 3 != 0
            || vertices.iter().flatten().any(|v| !v.is_finite())
            || indices.iter().any(|i| *i >= vertices.len())
            || ![home.x, home.y, home.z].iter().all(|v| v.is_finite())
        {
            return Err("镖物碰撞网格无效".into());
        }
        let zone = |name: &str| -> Result<SafeZone, String> {
            let objects: Vec<_> = desc
                .scene_objects
                .iter()
                .filter(|o| {
                    o.name_attr
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                })
                .collect();
            let [object] = objects.as_slice() else {
                return Err(format!("劫镖地图必须配置唯一 {name} 安全区"));
            };
            let mesh = stage_mesh
                .objects
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(&object.name))
                .ok_or_else(|| format!("安全区 {name} 缺少几何"))?;
            let mut min = [f32::INFINITY; 3];
            let mut max = [f32::NEG_INFINITY; 3];
            for p in mesh.positions.chunks_exact(3) {
                let p = world([p[0], p[1], p[2]], center);
                for (i, value) in [p.x, p.y, p.z].into_iter().enumerate() {
                    if !value.is_finite() {
                        return Err(format!("安全区 {name} 坐标无效"));
                    }
                    min[i] = min[i].min(value);
                    max[i] = max[i].max(value);
                }
            }
            if !min.iter().chain(&max).all(|v| v.is_finite())
                || min[0] >= max[0]
                || min[2] >= max[2]
            {
                return Err(format!("安全区 {name} 没有有效面积"));
            }
            Ok(SafeZone {
                min_x: min[0],
                min_y: min[1],
                min_z: min[2],
                max_x: max[0],
                max_y: max[1],
                max_z: max[2],
                center_x: (min[0] + max[0]) * 0.5,
                center_y: (min[1] + max[1]) * 0.5,
                center_z: (min[2] + max[2]) * 0.5,
            })
        };
        Ok(Self {
            stage: EscortStage {
                flag_home: home,
                safe_zones: [zone("safeA")?, zone("safeB")?],
                return_seconds: 5.0,
            },
            scene_name: object.name.clone(),
            scene_object_id: (desc.scene_objects.len() + index) as i32,
            item_id: record.id,
            model: model.into(),
            vertices,
            indices,
        })
    }
}

fn world(p: [f32; 3], center: [f32; 3]) -> RoleVec3 {
    RoleVec3 {
        x: (p[0] - center[0]) * 0.12,
        y: (p[2] - center[2]) * 0.12,
        z: -(p[1] - center[1]) * 0.12,
    }
}

fn rotate(p: [f32; 3], q: [f32; 4]) -> [f32; 3] {
    let norm = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    let q = q.map(|v| v / norm);
    let t = [
        2.0 * (q[1] * p[2] - q[2] * p[1]),
        2.0 * (q[2] * p[0] - q[0] * p[2]),
        2.0 * (q[0] * p[1] - q[1] * p[0]),
    ];
    [
        p[0] + q[3] * t[0] + q[1] * t[2] - q[2] * t[1],
        p[1] + q[3] * t[1] + q[2] * t[0] - q[0] * t[2],
        p[2] + q[3] * t[2] + q[0] * t[1] - q[1] * t[0],
    ]
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn authored_definition(custom: bool) -> EscortDefinition {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut desc = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn03/sn03.des")).unwrap(),
        );
        let mut mesh =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn03/sn03.gmb")).unwrap())
                .unwrap();
        let mut registry =
            meteor_asset::res::parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap())
                .unwrap();
        if custom {
            let object = desc
                .dummies
                .iter_mut()
                .find(|o| o.model.as_deref() == Some("IPItem02"))
                .unwrap();
            object.name = "D_editor_relic".into();
            object.model = Some("BBox02".into());
            object.resource_key = Some([4, 37]);
            object.position.x += 1000.0;
            object.position.z += 200.0;
            object.scale = 4.0;
            object.quaternion = meteor_core::Quat {
                x: 0.0,
                y: 0.0,
                z: std::f32::consts::FRAC_1_SQRT_2,
                w: std::f32::consts::FRAC_1_SQRT_2,
            };
            registry
                .records
                .iter_mut()
                .find(|r| r.category == 4 && r.id == 2)
                .unwrap()
                .id = 37;
            for object in &desc.scene_objects {
                if object
                    .name_attr
                    .as_deref()
                    .is_some_and(|n| matches!(n, "safeA" | "safeB"))
                {
                    for point in mesh
                        .objects
                        .iter_mut()
                        .find(|m| m.name == object.name)
                        .unwrap()
                        .positions
                        .chunks_exact_mut(3)
                    {
                        point[0] += 1500.0;
                    }
                }
            }
        }
        EscortDefinition::from_assets(
            &desc,
            &mesh,
            &registry,
            [-69.897672, 242.305639, 0.0],
            |model| {
                meteor_asset::gmdl::decode_gmdl(
                    &std::fs::read(root.join(format!("cmodel/{model}.gmb"))).ok()?,
                )
                .ok()
            },
        )
        .unwrap()
    }

    #[test]
    fn original_data_matches_the_previously_verified_fixture() {
        let definition = authored_definition(false);
        let expected = crate::escort::SN03;
        assert_eq!(definition.item_id, 2);
        assert_eq!(definition.scene_object_id, 215);
        assert!((definition.stage.flag_home.x - expected.flag_home.x).abs() < 0.001);
        for (actual, expected) in definition.stage.safe_zones.iter().zip(expected.safe_zones) {
            assert!((actual.center_x - expected.center_x).abs() < 0.001);
            assert!((actual.center_z - expected.center_z).abs() < 0.001);
        }
        assert!(definition.intersects(definition.stage.flag_home, definition.stage.flag_home));
    }

    #[test]
    fn renamed_scaled_rotated_model_and_moved_targets_drive_rules() {
        let definition = authored_definition(true);
        assert_eq!(definition.scene_name, "D_editor_relic");
        assert_eq!(definition.model, "BBox02");
        assert_eq!(definition.item_id, 37);
        let vertex = definition
            .vertices
            .iter()
            .max_by(|a, b| (a[0] * a[0] + a[2] * a[2]).total_cmp(&(b[0] * b[0] + b[2] * b[2])))
            .unwrap();
        let home = definition.stage.flag_home;
        let contact = RoleVec3 {
            x: home.x + vertex[0],
            y: home.y + vertex[1] - 0.96,
            z: home.z + vertex[2],
        };
        assert!(definition.intersects(contact, home));
        assert!(
            !crate::escort::character_capsule_intersects_flag(contact, home),
            "new model must use its own geometry"
        );
        let old_safe = crate::escort::SN03.safe_zones[0];
        assert!(!crate::escort::position_inside_safe_zone(
            RoleVec3 {
                x: old_safe.center_x,
                y: 0.0,
                z: old_safe.center_z
            },
            1,
            &definition.stage
        ));
    }
}
