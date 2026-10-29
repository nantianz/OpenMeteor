use super::{item_plugin::StageProp, stage_plugin::StageScriptRuntime};
use bevy::prelude::*;
use meteor_scene::hazards::{Aabb, HazardMachine, StageHazards};

#[derive(Resource, Default)]
pub(crate) struct StageHazardRuntime(pub StageHazards);

#[derive(Component, Clone)]
pub(crate) struct HazardPart(pub Vec<Vec3>);

impl HazardPart {
    pub(crate) fn from_positions(positions: &[f32]) -> Self {
        Self(
            positions
                .chunks_exact(3)
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .collect(),
        )
    }

    fn world_box(&self, transform: &Transform) -> Option<Aabb> {
        if self.0.is_empty() {
            return None;
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for p in &self.0 {
            let p = transform.transform_point(*p);
            min = min.min(p);
            max = max.max(p);
        }
        Some(Aabb {
            min: (min - Vec3::splat(0.4)).to_array(),
            max: (max + Vec3::splat(0.4)).to_array(),
        })
    }
}

pub(crate) use meteor_scene::hazards::build_volumes;

pub(crate) fn sync_hazard_machines(
    mut hazards: ResMut<StageHazardRuntime>,
    script: Res<StageScriptRuntime>,
    parts: Query<(&StageProp, &Transform, &HazardPart)>,
) {
    hazards.0.machines.clear();
    let Some(script) = script.script() else {
        return;
    };
    for instance in script.host().instances() {
        let damage = instance.get("damage").as_f64() == 1.0;
        let amount = instance.get("damagevalue").as_f64() as i32;
        if !damage
            || amount <= 0
            || instance
                .props
                .get("_active_override")
                .is_some_and(|v| v.as_f64() == 0.0)
        {
            continue;
        }
        let boxes = parts
            .iter()
            .filter(|(prop, _, _)| prop.name == instance.name)
            .filter_map(|(_, transform, part)| part.world_box(transform))
            .collect();
        hazards.0.machines.push(HazardMachine {
            name: instance.name.clone(),
            damage,
            damage_value: amount,
            active: true,
            boxes,
        });
    }
}

pub(crate) fn publish_stock_environment_geometry(
    hazards: Res<StageHazardRuntime>,
    net: Res<super::network_plugin::NetState>,
    tx: Option<Res<super::network_plugin::NetTx>>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    if net.is_stock_host() {
        if let Some(tx) = tx {
            let mut geometry = StageHazards::new();
            geometry.build_from_stage(hazards.0.volumes.clone(), hazards.0.machines.clone());
            tx.send_stock_environment_geometry(geometry);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (hazards, net, tx);
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hazard_origin_stage_volumes_are_built_from_all_authored_trigger_meshes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut stages = Vec::new();
        let mut total = 0;
        for entry in std::fs::read_dir(root).unwrap() {
            let dir = entry.unwrap().path();
            let name = dir.file_name().unwrap().to_str().unwrap();
            if !name.starts_with("sn") {
                continue;
            }
            let Ok(des) = std::fs::read(dir.join(format!("{name}.des"))) else {
                continue;
            };
            let desc =
                meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(&des));
            let expected = desc
                .scene_objects
                .iter()
                .filter(|o| o.is_hell || o.damage_name > 0)
                .count();
            if expected == 0 {
                continue;
            }
            let mesh = meteor_asset::gmdl::decode_gmdl(
                &std::fs::read(dir.join(format!("{name}.gmb"))).unwrap(),
            )
            .unwrap();
            let volumes = build_volumes(&mesh, &desc, [0.0; 3]);
            assert_eq!(volumes.len(), expected, "{name}");
            for volume in &volumes {
                assert_eq!(
                    volume.period,
                    if volume.damage >= 100000 { 0.1 } else { 1.0 },
                    "{name}/{}",
                    volume.name,
                );
                let midpoint =
                    std::array::from_fn(|i| (volume.box_.min[i] + volume.box_.max[i]) * 0.5);
                let mut runtime = StageHazards::new();
                runtime.build_from_stage(vec![volume.clone()], vec![]);
                let mut dealt = 0;
                runtime.update(
                    0.016,
                    &[("player".into(), midpoint, false, false)],
                    &mut |_, _, amount, _| dealt += amount,
                );
                assert_eq!(dealt, volume.damage, "{name}/{}", volume.name);
                assert!(midpoint.iter().all(|v| v.is_finite()));
            }
            total += volumes.len();
            stages.push(name.to_owned());
        }
        assert!(stages.len() >= 10, "{stages:?}");
        assert!(total >= 14, "only {total} authored triggers");
        eprintln!(
            "hazard resources: {} stages, {total} volumes: {stages:?}",
            stages.len()
        );
    }

    #[test]
    fn hazard_machine_parts_follow_current_transform_without_filling_fragment_gaps() {
        let part = HazardPart(vec![-Vec3::ONE, Vec3::ONE]);
        let left = part
            .world_box(&Transform::from_xyz(-10.0, 0.0, 0.0))
            .unwrap();
        let right = part
            .world_box(&Transform::from_xyz(10.0, 0.0, 0.0))
            .unwrap();
        assert!(!StageHazards::point_in_aabb([0.0; 3], &left));
        assert!(!StageHazards::point_in_aabb([0.0; 3], &right));
        assert!(StageHazards::point_in_aabb([-10.0, 0.0, 0.0], &left));
        let moved = part
            .world_box(&Transform::from_xyz(20.0, 0.0, 0.0))
            .unwrap();
        assert!(!StageHazards::point_in_aabb([-10.0, 0.0, 0.0], &moved));
        assert!(StageHazards::point_in_aabb([20.0, 0.0, 0.0], &moved));
    }
}
