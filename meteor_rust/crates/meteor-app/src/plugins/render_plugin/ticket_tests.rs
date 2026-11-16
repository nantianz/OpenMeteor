use super::*;
use bevy::ecs::system::RunSystemOnce;
use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};

#[cfg(feature = "original-assets")]
#[test]
fn ticket_original_maps_filter_props_collision_scripts_and_preserve_indices() {
    for (stage, rule, excluded, retained) in [
        ("sn03", 1, "D_BBox01", "D_IPItem02"),
        ("sn03", 2, "", "D_IPItem02"),
        ("sn04", 1, "D_APdoor", "D_BBox01"),
        ("sn04", 3, "", "D_APdoor"),
    ] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let read =
            |name: &str| crate::text_codec::decode_text(&std::fs::read(root.join(name)).unwrap());
        let des = read(&format!("{stage}/{stage}.des"));
        let desc = meteor_asset::des::parse_stage_description(&des);
        let mask = 1u32 << rule;
        let mut sources = ["Def.h", "box.pst", "desk.pst", "chair.pst", "jug.pst"]
            .map(|name| read(&format!("PScriptLib/{name}")))
            .to_vec();
        sources.push(read(&format!("{stage}/{stage}.pst")));
        let instances = desc
            .dummies
            .iter()
            .enumerate()
            .map(|(index, d)| SceneItemInstance {
                index,
                name: d.name.clone(),
                props: crate::plugins::stage_plugin::scene_item_props_from_dummy(d),
            })
            .collect();
        let mut script = SceneScriptRuntime::load(&sources.join("\n"), instances, 250_000).unwrap();
        script.host_mut().exclude_instances(
            desc.dummies
                .iter()
                .enumerate()
                .filter(|(_, d)| !d.admits_ticket(mask))
                .map(|(i, _)| i),
        );
        script.start().unwrap();
        script.start().unwrap();
        for (index, d) in desc.dummies.iter().enumerate() {
            assert_eq!(
                script.host().resolve_name(&d.name),
                d.admits_ticket(mask).then_some(index),
                "{stage}/{rule}: {} must keep its original UDP slot",
                d.name
            );
            if !d.admits_ticket(mask) {
                assert!(script.host().instance(index).is_none());
                assert_eq!(
                    script.host().instances()[index].get("active"),
                    PstValue::Num(0.0)
                );
            }
        }
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<GmdlMaterial>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<StageCollider>();
        world
            .insert_resource(crate::plugins::stage_plugin::StageScriptRuntime::from_script(script));
        world.run_system_once(move |mut commands: Commands,
            mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<GmdlMaterial>>,
            mut images: ResMut<Assets<Image>>, mut collider: ResMut<StageCollider>,
            script: Res<crate::plugins::stage_plugin::StageScriptRuntime>| {
            let mesh = collider.mesh.get_or_insert_with(StageMeshCollider::default);
            spawn_stage_props(&mut commands, &mut meshes, &mut materials, &mut images,
                &mut PakCache::default(), &root, &[root.clone()],
                &crate::app_state::LoadedStageInfo { stage_id: stage.into(), ..default() },
                Some(&des), 0, &mut Vec::new(), mesh, script.script(), None, Some(mask));
        }).unwrap();
        let mut query = world.query::<(
            &crate::plugins::item_plugin::StageProp,
            Option<&PropColliderOwner>,
        )>();
        let props: Vec<_> = query.iter(&world).collect();
        assert!(!props.is_empty());
        for (prop, _) in &props {
            let d = desc.dummies.iter().find(|d| d.name == prop.name).unwrap();
            assert!(
                d.admits_ticket(mask),
                "excluded prop/collider created: {}",
                prop.name
            );
        }
        if !excluded.is_empty() {
            assert!(!props.iter().any(|(p, _)| p.name == excluded));
        }
        if stage == "sn03" && rule == 1 {
            assert!(!props.iter().any(|(p, _)| p.name == retained));
        } else {
            assert!(
                props.iter().any(|(p, _)| p.name == retained),
                "missing {stage}/{rule}/{retained}"
            );
        }
        if stage == "sn04" && rule == 3 {
            assert!(
                props
                    .iter()
                    .any(|(p, c)| p.name == "D_APd02Box" && c.is_some()),
                "siege collision proxy must survive"
            );
        }
    }
}
