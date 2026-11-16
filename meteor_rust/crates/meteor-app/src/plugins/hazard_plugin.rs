use super::{item_plugin::StageProp, stage_plugin::StageScriptRuntime};
use bevy::prelude::*;
use meteor_physics::collision::{BODY_BOX_HALF_XY, BODY_BOX_Z_HIGH};
use meteor_scene::hazards::{obb_world_aabb, Aabb, HazardMachine, MachinePartObb, StageHazards};
use std::collections::HashMap;

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

    pub(crate) fn world_box(&self, transform: &Transform) -> Option<Aabb> {
        self.world_obb(transform).map(|obb| obb_world_aabb(&obb))
    }

    pub(crate) fn world_obb(&self, transform: &Transform) -> Option<MachinePartObb> {
        if self.0.is_empty() {
            return None;
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for p in &self.0 {
            min = min.min(*p);
            max = max.max(*p);
        }
        let local_center = (min + max) * 0.5;
        let local_half = (max - min) * 0.5;

        let center = transform.transform_point(local_center);
        let half = local_half * transform.scale.abs();
        let r = transform.rotation;
        Some(MachinePartObb {
            center: center.to_array(),
            half: half.to_array(),
            axes: [
                (r * Vec3::X).normalize_or_zero().to_array(),
                (r * Vec3::Y).normalize_or_zero().to_array(),
                (r * Vec3::Z).normalize_or_zero().to_array(),
            ],
        })
    }
}

#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct DamageSensorOwner(pub i32);

pub(crate) use meteor_scene::hazards::build_volumes;
pub(crate) use meteor_scene::hazards::build_world_bounds;

pub(crate) fn sync_hazard_machines(
    mut hazards: ResMut<StageHazardRuntime>,
    script: Res<StageScriptRuntime>,
    parts: Query<(&StageProp, &Transform, &HazardPart)>,
) {
    hazards.0.machines.clear();
    let Some(script) = script.script() else {
        return;
    };

    let mut obbs_by_name: HashMap<String, Vec<MachinePartObb>> = HashMap::new();
    for (prop, transform, part) in &parts {
        if let Some(obb) = part.world_obb(transform) {
            obbs_by_name.entry(prop.name.clone()).or_default().push(obb);
        }
    }

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

        let obbs = obbs_by_name
            .get(&instance.name)
            .cloned()
            .unwrap_or_default();
        let boxes = obbs.iter().map(obb_world_aabb).collect();
        hazards.0.machines.push(HazardMachine {
            name: instance.name.clone(),
            damage,
            damage_value: amount,
            active: true,
            boxes,
            obbs,
            collider_owners: vec![],
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
            geometry.world_bounds = hazards.0.world_bounds;
            tx.send_stock_environment_geometry(geometry);
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (hazards, net, tx);
}

pub(crate) fn is_damage_sensor_prop(dummy: &meteor_asset::des::StageObject) -> bool {
    !dummy.visible && dummy.model.is_some()
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
                .filter(|o| o.ticket_admitted && (o.is_hell || o.damage_name > 0))
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
                    if volume.damage >= 100000 { 0.0 } else { 1.0 },
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

    #[test]
    fn body_box_constants_match_exe_mover() {
        assert!((BODY_BOX_HALF_XY - 0.96).abs() < 1e-6);
        assert!((BODY_BOX_Z_HIGH - 3.84).abs() < 1e-6);
    }

    #[test]
    fn damage_proxy_obbs_do_not_borrow_colocated_solid() {
        use bevy::ecs::system::RunSystemOnce;
        use meteor_scene::pst_runtime::PstValue;
        use meteor_scene::scene_script_runtime::SceneItemInstance;
        let proxy = SceneItemInstance {
            name: "D_proxy".into(),
            index: 1,
            props: std::collections::HashMap::from([
                ("damage".into(), PstValue::Num(1.0)),
                ("damagevalue".into(), PstValue::Num(300.0)),
            ]),
        };
        let solid = SceneItemInstance {
            name: "D_solid".into(),
            index: 0,
            props: Default::default(),
        };
        let runtime = meteor_scene::scene_script_runtime::SceneScriptRuntime::load(
            "",
            vec![solid, proxy],
            1000,
        )
        .unwrap();
        let mut app = App::new();
        app.init_resource::<StageHazardRuntime>()
            .insert_resource(StageScriptRuntime::from_script(runtime));
        app.world_mut().spawn((
            StageProp {
                name: "D_solid".into(),
                model: "solid".into(),
                center_offset: Vec3::ZERO,
                half_extents: Vec3::ONE,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: false,
                collision_enabled: true,
                breakable: false,
                broken: false,
            },
            Transform::IDENTITY,
            HazardPart(vec![
                Vec3::new(-10.0, 0.0, -10.0),
                Vec3::new(10.0, 2.0, 10.0),
            ]),
        ));
        app.world_mut().spawn((
            StageProp {
                name: "D_proxy".into(),
                model: "proxy".into(),
                center_offset: Vec3::ZERO,
                half_extents: Vec3::ONE,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: false,
                collision_enabled: false,
                breakable: false,
                broken: false,
            },
            Transform::IDENTITY,
            HazardPart(vec![Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5)]),
        ));
        app.world_mut()
            .run_system_once(sync_hazard_machines)
            .unwrap();
        let machine = app
            .world()
            .resource::<StageHazardRuntime>()
            .0
            .machines
            .iter()
            .find(|m| m.name == "D_proxy")
            .cloned()
            .expect("damage proxy");
        assert_eq!(machine.obbs.len(), 1);
        assert!(
            machine.obbs[0].half[0] < 1.0,
            "proxy half={:?}",
            machine.obbs[0].half
        );

        assert!(!StageHazards::actor_touches_machine(
            [8.0, 0.5, 8.0],
            &machine
        ));
        assert!(StageHazards::actor_touches_machine(
            [0.0, 0.2, 0.0],
            &machine
        ));
    }
}
