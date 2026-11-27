use super::*;
use crate::app_state::{LoadedStageAssets, LoadedStageInfo, Settings};
use crate::plugins::{
    escort_assets::LoadedEscort,
    graphics_pack::{load_with_reader, GraphicsPacks},
    hazard_plugin::StageHazardRuntime,
    stage_plugin::StageScriptRuntime,
};
use bevy::{
    ecs::system::RunSystemOnce,
    render::renderer::{RenderAdapterInfo, WgpuWrapper},
};

fn assets_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn demo_packs(edit: impl FnOnce(&mut serde_json::Value)) -> GraphicsPacks {
    let config = assets_dir().join("meteor_rust/config");
    let manifest = "graphics-packs/meshlet-demo/pack.json";
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config.join(manifest)).unwrap()).unwrap();
    edit(&mut value);
    let packs = load_with_reader(
        br#"{"schema_version":1,"enabled":true,"packs":["graphics-packs/meshlet-demo/pack.json"]}"#,
        |name| {
            assert_eq!(name, manifest, "boot must not read model geometry");
            Ok(serde_json::to_vec(&value).unwrap())
        },
    )
    .unwrap();
    packs.with_test_loose_root(config)
}

fn scene(
    packs: GraphicsPacks,
    backend: wgpu::Backend,
) -> (usize, usize, Vec<(usize, usize)>, StageMeshCollider) {
    let source = assets_dir().join("origin_res/sn07");
    let gmb = std::fs::read(source.join("sn07.gmb")).unwrap();
    let cob = std::fs::read(source.join("sn07.cob")).unwrap();
    let face_ids = meteor_asset::cob::parse_cob(&cob).unwrap().face_ids;
    let center = [12., -8., 5.];
    let stage = meteor_asset::gmdl::decode_gmdl(&gmb).unwrap();
    let object = stage.objects.iter().find(|obj| obj.name == "Cy01").unwrap();
    let anchor = crate::plugins::graphics_pack::meshlets::source_anchor(object, center).unwrap();

    let mut app = App::new();
    app.insert_resource(packs)
        .insert_resource(RenderAdapterInfo(WgpuWrapper::new(wgpu::AdapterInfo {
            name: "test adapter".into(),
            vendor: 0,
            device: 0,
            device_type: wgpu::DeviceType::Other,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend,
            subgroup_min_size: 4,
            subgroup_max_size: 32,
            transient_saves_memory: false,
        })))
        .insert_resource(LoadedStageInfo {
            stage_id: "sn07".into(),
            center,
            ..default()
        })
        .insert_resource(LoadedStageAssets {
            gmb: Some(gmb),
            des: Some(std::fs::read(source.join("sn07.des")).unwrap()),
            cob: Some(cob),
            ..default()
        })
        .insert_resource(StageCollision {
            face_ids,
            ..default()
        })
        .insert_resource(Settings {
            hd_scene: false,
            ..default()
        })
        .init_resource::<StageCollider>()
        .init_resource::<StageHazardRuntime>()
        .init_resource::<StageScriptRuntime>()
        .init_resource::<LoadedEscort>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<GmdlMaterial>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<Image>>();
    app.world_mut().run_system_once(setup_stage_scene).unwrap();
    let mut query = app
        .world_mut()
        .query_filtered::<(&Mesh3d, &Transform), With<PlayingSceneEntity>>();
    let count = query.iter(app.world()).count();
    let meshes = app.world().resource::<Assets<Mesh>>();
    let replaced: Vec<_> = query
        .iter(app.world())
        .filter_map(|(handle, transform)| {
            if transform.translation.distance(anchor) > 1e-5 {
                return None;
            }
            let m = meshes.get(&handle.0).unwrap();
            Some((m.count_vertices(), m.indices().unwrap().len() / 3))
        })
        .collect();
    let drawn_triangles = query
        .iter(app.world())
        .map(|(handle, _)| {
            let mesh = meshes.get(&handle.0).unwrap();
            mesh.indices()
                .map_or_else(|| mesh.count_vertices(), Indices::len)
                / 3
        })
        .sum::<usize>();
    let object_triangles = if replaced.is_empty() {
        object.num_faces
    } else {
        replaced.iter().map(|(_, triangles)| triangles).sum()
    };
    let collider = app
        .world_mut()
        .resource_mut::<StageCollider>()
        .mesh
        .take()
        .unwrap();
    (
        count,
        drawn_triangles - object_triangles,
        replaced,
        collider,
    )
}

#[test]
fn meshlet_stage_demo_replaces_only_display_and_preserves_every_collision_triangle() {
    let (_, stock_triangles, stock, stock_collider) =
        scene(GraphicsPacks::default(), wgpu::Backend::Dx12);
    assert!(stock.is_empty());
    let (_, meshlet_triangles, meshes, collider) = scene(demo_packs(|_| {}), wgpu::Backend::Dx12);
    assert_eq!(meshes.len(), 74);
    assert_eq!(meshes.iter().map(|(_, tris)| tris).sum::<usize>(), 6912);
    assert!(meshes
        .iter()
        .all(|&(verts, tris)| verts <= 64 && tris <= 124));
    assert_eq!(meshlet_triangles, stock_triangles);
    assert_eq!(collider.positions, stock_collider.positions);
    assert_eq!(collider.indices, stock_collider.indices);
    assert_eq!(collider.wall_tris, stock_collider.wall_tris);
    assert_eq!(collider.floor_tris, stock_collider.floor_tris);
    assert_eq!(collider.steep_tris, stock_collider.steep_tris);
    assert_eq!(collider.ceiling_tris, stock_collider.ceiling_tris);
}

#[test]
fn meshlet_stage_demo_falls_back_per_object() {
    let (stock, _, _, _) = scene(GraphicsPacks::default(), wgpu::Backend::Dx12);
    let (gl, _, meshes, _) = scene(demo_packs(|_| {}), wgpu::Backend::Gl);
    assert_eq!(gl, stock);
    assert!(meshes.is_empty());
    for invalid in ["missing", "mismatch", "conflict"] {
        let packs = demo_packs(|json| match invalid {
            "missing" => {
                json["meshlet_models"]["ornament"]["file"] = "models/missing.mlmesh".into()
            }
            "mismatch" => {
                json["scenes"][0]["meshlet_replacements"][0]["source_sha256"] =
                    "0".repeat(64).into()
            }
            "conflict" => {
                let replacements = json["scenes"][0]["meshlet_replacements"]
                    .as_array_mut()
                    .unwrap();
                replacements.push(replacements[0].clone());
            }
            _ => unreachable!(),
        });
        let (count, _, meshes, _) = scene(packs, wgpu::Backend::Dx12);
        assert_eq!(count, stock, "{invalid}");
        assert!(meshes.is_empty(), "{invalid}");
    }
}

#[test]
fn meshlet_props_add_display_instances_without_replacing_geometry_or_collision() {
    let (stock, _, _, original) = scene(GraphicsPacks::default(), wgpu::Backend::Dx12);
    let props = || {
        demo_packs(|json| {
            let stage = &mut json["scenes"][0];
            stage["meshlet_replacements"] = serde_json::json!([]);
            stage["meshlet_props"] = serde_json::json!([{
                "model": "ornament",
                "placements": [
                    {"offset": [12.0, 0.0, 15.0]},
                    {"offset": [-12.0, 0.0, 15.0], "yaw_degrees": 36.0}
                ]
            }]);
        })
    };
    let (display, _, replaced, collider) = scene(props(), wgpu::Backend::Dx12);
    assert_eq!(display, stock + 2 * 74);
    assert!(replaced.is_empty());
    assert_eq!(collider.positions, original.positions);
    assert_eq!(collider.indices, original.indices);
    assert_eq!(scene(props(), wgpu::Backend::Gl).0, stock);
}
