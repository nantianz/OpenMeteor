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
            let sample_path = (weapon == 5 && fps == 120)
                .then(|| std::env::var_os("METEOR_MRC_DAGGER_SAMPLE"))
                .flatten();
            let mut frames = Vec::new();
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
                let active = sampled_attack_window(assets, runtime, blend).map(|(index, _)| index);
                if let Some(index) = active {
                    windows.push((time, pose, runtime.frame, index));
                }
                if sample_path.is_some() {
                    let world = h.app.world();
                    let state = world.get::<SmState>(h.fighter).unwrap();
                    frames.push(serde_json::json!({
                        "tick": tick + 1, "time": time, "pose": pose, "pose_frame": runtime.frame,
                        "fighter_state": format!("{:?}", state.current), "airborne": h.airborne(),
                        "position": h.position().to_array(), "attack_window": active,
                    }));
                }
            }
            if let Some(path) = sample_path {
                let edges: Vec<_> = input
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "time": e.time, "code": e.code, "value": e.value,
                        })
                    })
                    .collect();
                let out = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
                    .unwrap();
                serde_json::to_writer_pretty(out, &serde_json::json!({
                    "file": file, "simulation_hz": fps, "weapon_kind": weapon,
                    "start": from, "end_exclusive": until, "training_eligible": false,
                    "provenance": "conditional local input replay; initial state is not recorded",
                    "queue_b": edges, "frames": frames,
                })).unwrap();
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

#[cfg(not(target_arch = "wasm32"))]
mod pilot_v6_mrc_audit {
    include!("pilot_v6_mrc_audit.rs");
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "writes a reconstructed down/getup sample from a stock MRC"]
fn diagnose_recorded_down_getup_sample() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let file = std::env::var("METEOR_MRC_DOWN_FILE").expect("METEOR_MRC_DOWN_FILE");
    let output = std::env::var("METEOR_MRC_DOWN_OUT").expect("METEOR_MRC_DOWN_OUT");
    let from: f32 = std::env::var("METEOR_MRC_DOWN_FROM")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(44.0);
    let until: f32 = std::env::var("METEOR_MRC_DOWN_UNTIL")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(50.0);
    let fps: u32 = std::env::var("METEOR_MRC_DOWN_FPS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(120);
    let source = std::path::PathBuf::from(&file);
    let source = if source.is_absolute() {
        source
    } else {
        root.join("mrcs").join(source)
    };
    let bytes = std::fs::read(&source).unwrap();
    let doc = meteor_asset::mrc::decode_mrc(&bytes).unwrap();
    let local_slot = u32::from_le_bytes(doc.state[4..8].try_into().unwrap()) as u16;
    let weapon_item = u32::from_le_bytes(doc.state[..4].try_into().unwrap()) as i64;
    let (weapon, _) = crate::plugins::character_plugin::weapon_slot_from_item(weapon_item)
        .expect("recorded local weapon item");
    assert_eq!(weapon, 5, "down/getup sample is dagger-only");
    let queue_a = doc.queue_a.clone();
    let queue_b = doc.queue_b.clone();
    let stage = std::env::var("METEOR_MRC_DOWN_STAGE").unwrap_or_else(|_| "sn06".into());
    let stage_assets_available = root.join(format!("origin_res/Level/{stage}.wp")).exists()
        && root
            .join(format!("origin_res/{stage}/{stage}.gmb"))
            .exists();
    let collision_stage = if stage_assets_available {
        stage.clone()
    } else {
        std::env::var("METEOR_MRC_DOWN_COLLISION_STAGE").unwrap_or_else(|_| "sn06".into())
    };
    let wp = std::fs::read_to_string(root.join(format!("origin_res/Level/{collision_stage}.wp")))
        .unwrap();
    let center = meteor_scene::stage_loader::get_stage_center(&collision_stage, Some(&wp));
    let to_root = |source: Vec3| {
        Vec3::new(
            (source.x - center[0]) * meteor_core::MODEL_SCALE,
            (source.z - center[2]) * meteor_core::MODEL_SCALE,
            (center[1] - source.y) * meteor_core::MODEL_SCALE,
        )
    };
    let mesh = landing_roof_tests::original_stage_mesh(&collision_stage);
    let (mut h, _target, _hits) = spawn_local_stock_hit_harness_with_order(weapon, true);
    h.app.insert_resource(State::new(AppState::Playing));
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
    h.app
        .world_mut()
        .get_mut::<Fighter>(h.fighter)
        .unwrap()
        .character_id = 0;
    h.app
        .world_mut()
        .entity_mut(h.fighter)
        .insert(crate::components::fighter::VisualRootPin::default());
    crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
    crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
    let first = &doc.queue_d[0];
    h.app
        .world_mut()
        .get_mut::<Transform>(h.fighter)
        .unwrap()
        .translation = to_root(Vec3::new(
        f32::from_bits(first.a),
        f32::from_bits(first.b),
        f32::from_bits(first.c),
    ));
    h.replace_stage_mesh(mesh);
    h.app
        .insert_resource(meteor_scene::stage_loader::parse_waypoint_graph(
            &wp, center,
        ));
    h.app
        .add_plugins(crate::plugins::replay_plugin::ReplayPlugin);
    h.app
        .insert_resource(crate::plugins::replay_plugin::MrcPlayback {
            active: true,
            doc: Some(doc),
            ..default()
        });
    h.app
        .add_plugins(crate::plugins::character_contact_plugin::CharacterContactPlugin);
    let actor_schedule = super::super::fighter_frame::schedule(&mut h.app);
    h.app.add_systems(
        actor_schedule,
        hit_reaction_root_motion
            .in_set(crate::plugins::FighterFrameSet::RootMotion)
            .after(crate::plugins::movement_plugin::attack_root_motion)
            .after(crate::plugins::movement_plugin::dodge_root_motion),
    );
    let dt = std::time::Duration::from_nanos(1_000_000_000 / u64::from(fps));
    let mut frames = Vec::new();
    let mut transitions = Vec::new();
    let mut previous_pose = h.pose_id();
    let mut previous_state = String::new();
    for tick in 0..(until * fps as f32).ceil() as usize {
        h.step_duration(dt);
        let time = h
            .app
            .world()
            .resource::<crate::plugins::replay_plugin::MrcPlayback>()
            .elapsed;
        let pose = h.pose_id();
        let state_now = h
            .app
            .world()
            .get::<SmState>(h.fighter)
            .map(|state| format!("{:?}", state.current))
            .unwrap_or_default();
        if time >= from {
            let world = h.app.world();
            let sm = world.get::<SmState>(h.fighter).unwrap();
            let combat = world.get::<CombatState>(h.fighter).unwrap();
            let motion = world.get::<MotionState>(h.fighter).unwrap();
            let runtime = world.get::<PoseRuntime>(h.fighter).unwrap();
            let blend = world.get::<EntryBlend>(h.fighter).unwrap();
            let health = world.get::<Health>(h.fighter).unwrap();
            let assets = &world.resource::<CharacterAssetsSet>().0["p0"];
            let active = sampled_attack_window(assets, runtime, blend).map(|(index, _)| index);
            let reaction = world
                .get::<FighterHitReaction>(h.fighter)
                .and_then(|runtime| {
                    runtime.current.as_ref().map(|reaction| {
                        serde_json::json!({
                            "kind": format!("{:?}", reaction.kind),
                            "pose": reaction.pose_id,
                            "lock_remaining": reaction.lock_remaining,
                            "minimum_remaining": reaction.minimum_remaining,
                            "authored_minimum": reaction.authored_minimum,
                            "motion_scale": reaction.motion_scale,
                            "down_remaining": reaction.down_remaining,
                            "root_started": runtime.root_started,
                            "held": runtime.entry_blend_held,
                        })
                    })
                });
            if pose != previous_pose || state_now != previous_state {
                transitions.push(serde_json::json!({
                    "tick": tick + 1, "time": time, "pose": pose,
                    "fighter_state": state_now,
                }));
            }
            frames.push(serde_json::json!({
                "tick": tick + 1,
                "time": time,
                "pose": pose,
                "pose_frame": runtime.frame,
                "fighter_state": state_now,
                "previous_state": format!("{:?}", sm.previous),
                "state_locked": sm.locked,
                "time_in_state": sm.time_in_state,
                "combat_state": format!("{:?}", combat.state),
                "airborne": h.airborne(),
                "motion_mode": format!("{:?}", motion.movement_mode),
                "position": h.position().to_array(),
                "facing": motion.facing,
                "velocity": motion.velocity.to_array(),
                "health": health.current,
                "attack_window": active,
                "reaction": reaction,
            }));
        }
        previous_pose = pose;
        previous_state = state_now;
    }
    let mut events = Vec::new();
    for entry in &queue_a {
        if entry.time < from || entry.time >= until {
            continue;
        }
        match entry.id {
            50 | 51 => {
                if let Ok(event) =
                    meteor_network::stock_protocol::StockPoseChainEvent::decode(&entry.blob)
                {
                    events.push(serde_json::json!({
                        "time": entry.time, "id": entry.id, "actor": event.actor_id,
                        "pose": event.pose_id, "minimum": event.minimum_seconds,
                        "sequence": event.sequence, "blend": event.blend_seconds,
                    }));
                }
            }
            11 | 14 => {
                if let Ok(event) =
                    meteor_network::stock_protocol::StockHitReactionEvent::decode(&entry.blob)
                {
                    events.push(serde_json::json!({
                        "time": entry.time, "id": entry.id, "target": event.target_actor_id,
                        "pose_word": event.pose_id, "move": event.move_hundred,
                        "value": event.value_hundred, "context": event.context,
                    }));
                }
            }
            12 if entry.blob.len() >= 8 => {
                events.push(serde_json::json!({
                    "time": entry.time, "id": entry.id,
                    "item": u32::from_le_bytes(entry.blob[..4].try_into().unwrap()),
                    "actor": u32::from_le_bytes(entry.blob[4..8].try_into().unwrap()),
                }));
            }
            _ => {}
        }
    }
    let inputs: Vec<_> = queue_b
        .iter()
        .filter(|entry| entry.time >= from && entry.time < until)
        .map(|entry| {
            serde_json::json!({
                "time": entry.time, "code": entry.code, "value": entry.value,
            })
        })
        .collect();
    let out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)
        .unwrap();
    serde_json::to_writer_pretty(
        out,
        &serde_json::json!({
            "file": source.file_name().unwrap().to_string_lossy(),
            "stage": stage,
            "collision_stage": collision_stage,
            "stage_assets_available": stage_assets_available,
            "simulation_hz": fps,
            "weapon_kind": weapon,
            "local_slot": local_slot,
            "local_hero": 0,
            "start": from,
            "end_exclusive": until,
            "training_eligible": false,
            "provenance": if stage_assets_available {
                "production ReplayPlugin + hit reaction + character contact replay; initial controller state before recording start and the original client's down mode are not recorded"
            } else {
                "production ReplayPlugin + hit reaction; recording-stage assets are unavailable, so collision and position are not reproducible; state transitions use the recorded hit/input timing"
            },
            "unknown": if stage_assets_available {
                vec![
                    "recorded_controller_down_mode",
                    "getup_pose_choice_not_broadcast",
                    "opponent_state_not_reconstructed",
                ]
            } else {
                vec![
                    "recorded_controller_down_mode",
                    "getup_pose_choice_not_broadcast",
                    "opponent_state_not_reconstructed",
                    "recording_stage_assets_unavailable_positions_not_reproducible",
                ]
            },
            "events": events,
            "queue_b": inputs,
            "transitions": transitions,
            "frames": frames,
        }),
    )
    .unwrap();
    eprintln!(
        "down/getup sample: {} frames, {} events, {} inputs -> {output}",
        frames.len(),
        events.len(),
        inputs.len()
    );
}
