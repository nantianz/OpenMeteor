use super::*;

fn packed_scene(stage: &str) -> SceneScriptRuntime {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../asset-tools/packs");
    let packs = meteor_asset::pack_repository::PackRepository::open(&root).unwrap();
    let read = |path: &str| {
        let entry = meteor_asset::runtime_assets::entry_name_for_legacy_path(path)?;
        packs.read(&entry).ok()
    };
    let path = format!("{stage}/{stage}.pst");
    let text = crate::text_codec::decode_text(&read(&path).unwrap());

    let (source, _, missing) = expand_scene_pst_includes(&path, &text, &mut |p| read(p));
    assert!(
        missing.is_empty(),
        "{stage}: missing packed includes {missing:?}"
    );
    let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(
        &read(&format!("{stage}/{stage}.des")).unwrap(),
    ));
    let instances = desc
        .dummies
        .iter()
        .enumerate()
        .map(|(index, dummy)| SceneItemInstance {
            name: dummy.name.clone(),
            index,
            props: scene_item_props_from_dummy(dummy),
        })
        .collect();
    let mut runtime = SceneScriptRuntime::load(&source, instances, 250_000).unwrap();
    runtime.host_mut().set_game_mode(5);
    runtime.start().unwrap();
    runtime
}

fn property(runtime: &SceneScriptRuntime, name: &str, key: &str) -> f64 {
    let index = runtime.host().resolve_name(name).unwrap();
    runtime.host().instance(index).unwrap().get(key).as_f64()
}

#[cfg(feature = "original-assets")]
#[test]
fn official_packed_scene_includes_are_complete() {
    for index in 1..=22 {
        packed_scene(&format!("sn{index:02}"));
    }
}

#[cfg(feature = "original-assets")]
#[test]
fn packed_christmas_boxes_hide_loot_until_broken_and_star_releases_shared_candy() {
    let mut host = packed_scene("sn21");
    let mut peer = packed_scene("sn21");
    for item in host.host().instances() {
        let name = item.name.to_ascii_lowercase();
        if name.starts_with("d_itbbox") || name.starts_with("d_wpbbox") {
            assert_eq!(item.get("active").as_f64(), 0.0, "{}", item.name);
            assert_eq!(item.get("interactive").as_f64(), 0.0, "{}", item.name);
        }
        if name.starts_with("d_bbox") {
            assert_eq!(
                item.get("script_type"),
                PstValue::Str("machine".into()),
                "{}",
                item.name
            );
            assert_eq!(item.get("pose").as_f64(), 0.0);
            assert!(item.collision_enabled());
        }
    }

    for index in 1..=103 {
        let active = f64::from(index < 5);
        assert_eq!(
            property(&host, &format!("D_Item{index:02}"), "active"),
            active
        );
        assert_eq!(
            property(&host, &format!("D_Item{index:02}"), "interactive"),
            active
        );
    }
    for (box_name, loot) in [("D_BBox01", "D_ItBBox01"), ("D_BBox09", "D_wpBBox09")] {
        host.call_item(
            box_name,
            "OnAttack",
            &[PstValue::Num(0.0), PstValue::Num(50.0)],
        )
        .unwrap();
        assert_eq!(property(&host, box_name, "pose"), 2.0);
        assert_eq!(property(&host, loot, "active"), 0.0);
        host.call_item(
            box_name,
            "OnAttack",
            &[PstValue::Num(0.0), PstValue::Num(50.0)],
        )
        .unwrap();
        assert_eq!(property(&host, box_name, "pose"), 1.0);
        assert_eq!(property(&host, box_name, "collision"), 0.0);
        assert_eq!(property(&host, loot, "active"), 1.0);
        assert_eq!(property(&host, loot, "interactive"), 1.0);
    }
    host.host_mut().take_net_batches();
    host.call_item(
        "D_sn21st01",
        "OnAttack",
        &[PstValue::Num(0.0), PstValue::Num(50.0)],
    )
    .unwrap();
    let batches = host.host_mut().take_net_batches();

    assert_eq!(
        batches.iter().map(Vec::len).collect::<Vec<_>>(),
        [63, 63, 63, 11]
    );
    for op in batches.iter().flatten() {
        apply_network_scene_op(&mut peer, &scene_op_from_runtime(op));
    }
    for index in 5..=103 {
        let name = format!("D_Item{index:02}");
        for runtime in [&host, &peer] {
            assert_eq!(property(runtime, &name, "active"), 1.0, "{name}");
            assert_eq!(property(runtime, &name, "interactive"), 1.0, "{name}");
        }
    }
}
