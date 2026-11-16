#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "writes fist-chain frame/Attack diagnostics under analysis"]
fn diagnose_recorded_fist_ultimate_attack_records() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for fps in [30_u32, 60, 120] {
        let mut h = spawn_gameplay_harness_with_order(11, true).unwrap();
        let p0 = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root.join("origin_res")),
            "p0",
        )
        .unwrap();
        h.app
            .world_mut()
            .resource_mut::<CharacterAssetsSet>()
            .0
            .insert("p0".into(), p0);
        h.app
            .world_mut()
            .get_mut::<Fighter>(h.fighter)
            .unwrap()
            .character_id = 0;
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert(crate::components::fighter::VisualRootPin::default());
        crate::plugins::character_plugin::spawn_fighter_model_for_test(
            h.app.world_mut(),
            h.fighter,
        );
        crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
        let mut pose = PoseRuntime::idle();
        h.app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut pose, 421, true);
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert((pose, EntryBlend::default()));
        h.app
            .world_mut()
            .get_mut::<SmState>(h.fighter)
            .unwrap()
            .current = FighterState::Attack1;
        h.app
            .world_mut()
            .get_mut::<CombatState>(h.fighter)
            .unwrap()
            .state = FighterState::Attack1;
        let mut rows = Vec::new();
        let mut transitions = Vec::new();
        let mut selected_records: Vec<(u16, u32, Option<u16>, f32, f32)> = Vec::new();
        let mut previous = h.pose_id();
        for tick in 0..fps * 7 {
            h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / fps as u64));
            let pose = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
            let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
            let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p0"];
            let selected = sampled_attack_window(assets, pose, blend);
            if let Some((index, window)) = selected {
                if selected_records
                    .last()
                    .is_none_or(|(prior_pose, prior_index, ..)| {
                        *prior_pose != pose.pose_id || *prior_index != index
                    })
                {
                    selected_records.push((
                        pose.pose_id,
                        index,
                        window.target_pose,
                        window.target_move,
                        window.target_value,
                    ));
                }
            }
            if pose.pose_id != previous {
                transitions.push((tick, pose.pose_id));
                previous = pose.pose_id;
            }
            rows.push(serde_json::json!({
                "tick":tick,"time":(tick+1) as f32/fps as f32,
                "pose":pose.pose_id,"frame":pose.frame,
                "blend":blend.duration,
                "record":blend.frame_record_for(pose),
                "attack":selected.map(|(index, window)|serde_json::json!({
                    "index":index,"start":window.start,"end":window.end,
                    "targetPose":window.target_pose,"targetMove":window.target_move,
                    "targetValue":window.target_value,
                })),
            }));
            if pose.pose_id == 488 && pose.frame >= assets.poses[&488].end as f32 {
                break;
            }
        }
        let output = root
            .join("analysis/mrc-moves-20260928")
            .join(format!("fist-chain-attack-records-{fps}.json"));
        std::fs::write(output, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
        eprintln!(
            "fist fps={fps}: {transitions:?}; selected={} ticks",
            rows.iter().filter(|row| !row["attack"].is_null()).count()
        );
        for id in [422, 423, 424, 425, 426, 488] {
            assert!(
                transitions.iter().any(|(_, pose)| *pose == id),
                "fist fps={fps} must naturally link to {id}: {transitions:?}"
            );
        }
        assert_eq!(
            selected_records,
            vec![
                (421, 0, Some(109), 0.0, 1.0),
                (422, 0, Some(110), 3.0, 2.0),
                (423, 0, Some(121), 1.0, 2.0),
                (423, 1, Some(110), 1.0, 2.0),
                (423, 2, Some(121), 1.0, 2.0),
                (423, 3, Some(110), 1.0, 2.0),
                (423, 4, Some(109), 1.0, 3.0),
                (424, 0, Some(100), 10.0, 2.0),
                (425, 0, Some(95), 0.5, 2.0),
                (425, 1, Some(102), 1.0, 2.0),
                (426, 0, Some(114), 1.0, 0.1),
            ],
            "fist fps={fps} Attack metadata must cover original chain"
        );
    }
}
