#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "diagnostic for recorded small-jump inputs"]
fn diagnose_recorded_small_jump_entries() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for (file, weapon, from, until, expected_pose) in [
        ("剑小跳.mrc", 6, 12.5_f32, 13.3_f32, 260_u16),
        ("匕首小跳.mrc", 5, 14.6_f32, 15.4_f32, 245_u16),
    ] {
        let doc = meteor_asset::mrc::decode_mrc(
            &std::fs::read(root.join("mrcs/tests").join(file)).unwrap(),
        )
        .unwrap();
        for fps in [60_u32, 120] {
            let mut h = spawn_gameplay_harness_with_order(weapon, true).unwrap();
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
            let keymap = crate::app_state::Keymap::default();
            let input: Vec<_> = doc
                .queue_b
                .iter()
                .filter(|entry| (from..until).contains(&entry.time))
                .collect();
            let mut next = 0;
            let mut previous = h.pose_id();
            let mut transitions = Vec::new();
            let mut windows = Vec::new();
            for tick in 0..((until - from) * fps as f32).ceil() as usize {
                let time = from + (tick + 1) as f32 / fps as f32;
                while next < input.len() && input[next].time <= time {
                    let entry = input[next];
                    let binding = match entry.code {
                        0x43 => Some(keymap.crouch),
                        0x57 => Some(keymap.move_up),
                        0x53 => Some(keymap.move_down),
                        0x41 => Some(keymap.move_left),
                        0x44 => Some(keymap.move_right),
                        0x51 => Some(keymap.lock_toggle),
                        0x1000 => Some(keymap.attack),
                        0x1200 => Some(keymap.jump),
                        _ => None,
                    };
                    if let Some(binding) = binding {
                        if entry.value != 0 {
                            h.press_binding(binding);
                        } else {
                            h.release_binding(binding);
                        }
                    }
                    next += 1;
                }
                h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / fps as u64));
                let pose = h.pose_id();
                if pose != previous {
                    transitions.push((time, pose));
                    previous = pose;
                }
                let runtime = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
                let blend = h.app.world().get::<EntryBlend>(h.fighter).unwrap();
                let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p0"];
                if let Some((index, _)) = sampled_attack_window(assets, runtime, blend) {
                    windows.push((time, pose, runtime.frame, index));
                }
            }
            eprintln!("{file} fps={fps}: transitions={transitions:?}; windows={windows:?}");
            assert!(
                transitions.iter().any(|(_, pose)| *pose == expected_pose),
                "{file} fps={fps}: recorded input should enter Pose {expected_pose}"
            );
            assert!(
                windows.iter().any(|(_, pose, _, _)| *pose == expected_pose),
                "{file} fps={fps}: original Pose {expected_pose} attack window should be sampled"
            );
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "conditional MRC anchor geometry; not an original-game trajectory"]
fn diagnose_recorded_small_jump_stock_contacts() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");

    for (file, weapon, from, until, pose_id, reaction, target_offset, attacker_yaw, target_yaw) in [
        (
            "剑小跳.mrc",
            6,
            12.5_f32,
            13.3_f32,
            260_u16,
            97_u16,
            Vec3::new(-2.271, 0.0, 2.077),
            -0.008877_f32,
            2.270796_f32,
        ),
        (
            "匕首小跳.mrc",
            5,
            14.6_f32,
            15.4_f32,
            245_u16,
            98_u16,
            Vec3::new(-4.314, 0.0, -0.037),
            -1.049689_f32,
            std::f32::consts::FRAC_PI_2,
        ),
    ] {
        let doc = meteor_asset::mrc::decode_mrc(
            &std::fs::read(root.join("mrcs/tests").join(file)).unwrap(),
        )
        .unwrap();
        for fps in [60_u32, 120] {
            let (mut h, target, take_hits) = spawn_local_stock_hit_harness_with_order(weapon, true);
            let models: Vec<_> = h
                .app
                .world_mut()
                .query::<(Entity, &FighterModel)>()
                .iter(h.app.world())
                .map(|(entity, _)| entity)
                .collect();
            for model in models {
                h.app.world_mut().despawn(model);
            }
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
            for entity in [h.fighter, target] {
                h.app
                    .world_mut()
                    .get_mut::<Fighter>(entity)
                    .unwrap()
                    .character_id = 0;
                h.app
                    .world_mut()
                    .entity_mut(entity)
                    .insert(crate::components::fighter::VisualRootPin::default());
                crate::plugins::character_plugin::spawn_fighter_model_for_test(
                    h.app.world_mut(),
                    entity,
                );
            }
            let mut down = PoseRuntime::idle();
            h.app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut down, 10, true);
            h.app
                .world_mut()
                .entity_mut(target)
                .insert((down, FighterHitReaction::default()));
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = target_offset;
            h.app
                .world_mut()
                .get_mut::<MotionState>(h.fighter)
                .unwrap()
                .facing = attacker_yaw;
            h.app
                .world_mut()
                .get_mut::<MotionState>(target)
                .unwrap()
                .facing = target_yaw;
            h.app
                .world_mut()
                .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
                .yaw = attacker_yaw;
            let schedule = super::super::fighter_frame::schedule(&h.app);
            h.app.add_systems(
                schedule,
                crate::plugins::character_plugin::drive_fighter_animation_for_test
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample),
            );
            h.app.add_systems(
                schedule,
                attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
            );
            h.app.add_systems(
                Update,
                tick_target_hit_clocks.in_set(crate::plugins::FighterFrameSet::PoseDecision),
            );
            let keymap = crate::app_state::Keymap::default();
            let input: Vec<_> = doc
                .queue_b
                .iter()
                .filter(|entry| (from..until).contains(&entry.time))
                .collect();
            let mut next = 0;
            let mut rows = Vec::new();
            for tick in 0..((until - from) * fps as f32).ceil() as usize {
                let time = from + (tick + 1) as f32 / fps as f32;
                while next < input.len() && input[next].time <= time {
                    let entry = input[next];
                    let binding = match entry.code {
                        0x43 => Some(keymap.crouch),
                        0x57 => Some(keymap.move_up),
                        0x53 => Some(keymap.move_down),
                        0x41 => Some(keymap.move_left),
                        0x44 => Some(keymap.move_right),
                        0x51 => Some(keymap.lock_toggle),
                        0x1000 => Some(keymap.attack),
                        0x1200 => Some(keymap.jump),
                        _ => None,
                    };
                    if let Some(binding) = binding {
                        if entry.value != 0 {
                            h.press_binding(binding);
                        } else {
                            h.release_binding(binding);
                        }
                    }
                    next += 1;
                }
                h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / fps as u64));
                let claims = take_hits();
                if h.pose_id() == pose_id || !claims.is_empty() {
                    rows.push(serde_json::json!({"time":time,"pose":h.pose_id(),
                        "frame":h.app.world().get::<PoseRuntime>(h.fighter).unwrap().frame,
                        "attacker":h.position().to_array(),
                        "target":h.app.world().get::<Transform>(target).unwrap().translation.to_array(),
                        "claims":claims.iter().map(|(_, request)|serde_json::json!({
                            "target":request.target_actor_id,"pose":request.target_pose(),
                            "move":request.target_move(),"minimum":request.target_value(),
                        })).collect::<Vec<_>>() }));
                }
            }
            let claims = rows
                .iter()
                .filter(|row| row["claims"].as_array().is_some_and(|a| !a.is_empty()))
                .count();
            eprintln!("{file} fps={fps} pose={pose_id} conditional claims={claims}");
            let output = root.join("analysis/mrc-moves-20260928").join(format!(
                "small-jump-{}-{fps}.json",
                if weapon == 6 { "sword" } else { "dagger" }
            ));
            std::fs::write(output, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
            assert!(
                rows.iter().any(|row| row["pose"] == pose_id),
                "recorded inputs enter {pose_id}"
            );
            assert_eq!(claims, 1, "{file} fps={fps}: one conditional request5");
            let claim = rows
                .iter()
                .find_map(|row| row["claims"].as_array().and_then(|claims| claims.first()))
                .unwrap();
            assert_eq!(claim["pose"], reaction);
            assert_eq!(claim["move"], weapon as f32);
            assert_eq!(claim["minimum"], 0.0);
        }
    }
}
