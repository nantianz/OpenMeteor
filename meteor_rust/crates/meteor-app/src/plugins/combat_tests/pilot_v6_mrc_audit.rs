use super::*;
use crate::plugins::network_plugin::RemoteFighter;
use crate::plugins::replay_plugin::{MrcPlayback, ReplayPlugin};
use meteor_anim::pose_controller::pose_elapsed_seconds_at_frame;
use meteor_network::role_state::decode_event2_state;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct Selection {
    selected: Vec<ReplayCase>,
}

#[derive(Deserialize)]
struct ReplayCase {
    file: String,
    sha256: String,
    stage: String,
    local: u32,
    remote: u8,
    local_hero: u8,
    remote_hero: u8,
    audit_start: f32,
    audit_end: f32,
    local_spawn_item: u32,
    local_window_item: u32,

    plane_world_y: f32,
}

#[test]
#[ignore = "writes headless Pilot replay audit traces"]
fn diagnose_pilot_v6_dagger_replay_batch() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let selection = std::env::var_os("METEOR_PILOT_AUDIT_SELECTION").expect("audit selection path");
    let output = std::env::var_os("METEOR_PILOT_AUDIT_OUT").expect("audit output directory");
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir(&output).unwrap();
    let cases: Selection =
        serde_json::from_reader(std::fs::File::open(selection).unwrap()).unwrap();
    assert!(!cases.selected.is_empty());
    for case in &cases.selected {
        let trace = replay_case(&root, case);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output.join(format!("{}.json", case.sha256)))
            .unwrap();
        serde_json::to_writer(std::io::BufWriter::new(file), &trace).unwrap();
        eprintln!(
            "{}: {} observations, {} edges",
            case.file,
            trace["observations"].as_array().unwrap().len(),
            trace["edges"].as_array().unwrap().len()
        );
    }
}

fn replay_case(root: &std::path::Path, case: &ReplayCase) -> Value {
    let bytes = std::fs::read(root.join("mrcs").join(&case.file)).unwrap();
    let doc = meteor_asset::mrc::decode_mrc(&bytes).unwrap();
    assert_eq!((doc.mark0, doc.mark1), (9, 7));

    let playback = MrcPlayback {
        active: true,
        doc: Some(doc),
        ..default()
    };
    assert_eq!(playback.stage_id().as_deref(), Some(case.stage.as_str()));
    assert_eq!(playback.local_slot(), Some(case.local));
    assert_eq!(playback.local_character(), Some(case.local_hero));

    let (spawn_weapon, spawn_variant) =
        crate::plugins::character_plugin::weapon_slot_from_item(i64::from(case.local_spawn_item))
            .expect("recorded local spawn item is a weapon");
    assert_eq!(
        playback.local_weapon(),
        Some((spawn_weapon, spawn_variant)),
        "{} header weapon",
        case.file
    );
    let (window_weapon, _) =
        crate::plugins::character_plugin::weapon_slot_from_item(i64::from(case.local_window_item))
            .expect("selected window item is a weapon");
    assert!(case.audit_start < case.audit_end && case.audit_end <= playback.duration());

    let waypoint_text = std::fs::read_to_string(
        root.join("origin_res/Level")
            .join(format!("{}.wp", case.stage)),
    )
    .ok();
    let center =
        meteor_scene::stage_loader::get_stage_center(&case.stage, waypoint_text.as_deref());
    let plane_y = case.plane_world_y - center[2] * meteor_core::MODEL_SCALE;
    let mut h = spawn_gameplay_harness_with_order(spawn_weapon, true).unwrap();
    h.replace_stage_mesh(audit_plane(plane_y));
    h.app.insert_resource(State::new(AppState::Playing));
    for hero in [case.local_hero, case.remote_hero] {
        let name = format!("p{hero}");
        let assets = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root.join("origin_res")),
            &name,
        )
        .expect("recorded character asset");
        h.app
            .world_mut()
            .resource_mut::<CharacterAssetsSet>()
            .0
            .insert(name, assets);
    }
    let spawn = playback
        .first_stock_local_position(center)
        .expect("first local position");
    {
        let mut entity = h.app.world_mut().entity_mut(h.fighter);
        let mut fighter = entity.get_mut::<Fighter>().unwrap();
        fighter.character_id = case.local_hero;
        fighter.weapon = spawn_weapon;
        fighter.weapon_slots = [spawn_weapon, 0];
        fighter.weapon_slot = 0;
        fighter.weapon_variant = spawn_variant;
        fighter.weapon_slot_variants = [spawn_variant, 0];
        entity.get_mut::<Transform>().unwrap().translation = spawn;
        entity.insert((
            crate::components::fighter::VisualRootPin::default(),
            HeadTrackState::default(),
        ));
    }
    crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), h.fighter);
    crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
    configure_lock_updates(&mut h.app);
    let graph = waypoint_text.as_deref().map_or_else(
        || meteor_scene::stage_loader::WaypointGraph {
            center,
            ..Default::default()
        },
        |text| meteor_scene::stage_loader::parse_waypoint_graph(text, center),
    );
    h.app.insert_resource(graph);
    h.app
        .add_plugins((ReplayPlugin, crate::plugins::CharacterContactPlugin));
    let local_equips: Vec<(f32, u32)> = playback
        .doc
        .as_ref()
        .unwrap()
        .queue_a
        .iter()
        .filter_map(|entry| {
            if entry.id != 12 || entry.blob.len() < 8 {
                return None;
            }
            let item = u32::from_le_bytes(entry.blob[0..4].try_into().ok()?);
            let slot = u32::from_le_bytes(entry.blob[4..8].try_into().ok()?);
            (slot == case.local).then_some((entry.time, item))
        })
        .collect();
    h.app.insert_resource(playback);
    let schedule = crate::plugins::fighter_frame::schedule(&h.app);
    h.app.add_systems(
        schedule,
        hit_reaction_root_motion
            .in_set(crate::plugins::FighterFrameSet::RootMotion)
            .after(crate::plugins::movement_plugin::attack_root_motion)
            .after(crate::plugins::movement_plugin::dodge_root_motion),
    );

    let mut remotes = h
        .app
        .world_mut()
        .query_filtered::<(Entity, &Fighter), With<RemoteFighter>>();
    let dt = std::time::Duration::from_nanos(1_000_000_000 / 120);
    let mut observations = Vec::new();
    let mut edges = Vec::new();
    let mut pose_entries = Vec::new();
    let mut jump_accepts = Vec::new();
    let mut previous = None;
    let mut previous_pose_instance = None;
    let mut next_sample = (f64::from(case.audit_start) * 20.0).ceil() as u64;
    let mut a_cursor = 0;
    let mut equip_cursor = 0;
    let mut applied_equipment = Vec::new();
    let mut window_weapon_mismatch = 0u32;
    let mut first_weapon_mismatch = None;
    let mut received = None;
    let mut direct_hit_events = Vec::new();
    loop {
        let elapsed = h.app.world().resource::<MrcPlayback>().elapsed;
        while equip_cursor < local_equips.len()
            && local_equips[equip_cursor].0 <= elapsed + dt.as_secs_f32()
        {
            let (stamp, item) = local_equips[equip_cursor];
            let applied = crate::plugins::character_plugin::weapon_slot_from_item(i64::from(item));
            if let Some((weapon, variant)) = applied {
                let mut entity = h.app.world_mut().entity_mut(h.fighter);
                {
                    let mut fighter = entity.get_mut::<Fighter>().unwrap();
                    fighter.weapon = weapon;
                    fighter.weapon_slots = [weapon, 0];
                    fighter.weapon_slot = 0;
                    fighter.weapon_variant = variant;
                    fighter.weapon_slot_variants = [variant, 0];
                }
                entity.insert(crate::components::fighter::WeaponVariantChanged);
            }
            applied_equipment.push(json!([stamp, elapsed, item, applied]));
            equip_cursor += 1;
        }
        h.step_duration(dt);
        let world = h.app.world();
        let replay = world.resource::<MrcPlayback>();
        let time = replay.elapsed;
        if time >= case.audit_end {
            break;
        }
        if time >= case.audit_start {
            let weapon = world.get::<Fighter>(h.fighter).unwrap().weapon;
            if weapon != window_weapon {
                window_weapon_mismatch += 1;
                first_weapon_mismatch.get_or_insert((time, weapon, window_weapon));
            }
        }
        let doc = replay.doc.as_ref().unwrap();
        for (index, event) in doc.queue_a[a_cursor..replay.index_a].iter().enumerate() {
            if event.id == 2 {
                for chunk in event.blob.chunks_exact(16) {
                    if u16::from_le_bytes([chunk[0], chunk[1]]) == u16::from(case.remote) {
                        if let Ok(state) = decode_event2_state(&chunk[2..]) {
                            received = Some((event.time, index + a_cursor, state.state_words));
                        }
                    }
                }
            }
            if time >= case.audit_start && matches!(event.id, 11 | 14 | 50 | 51) {
                direct_hit_events.push(json!([event.time, time, event.id, index + a_cursor]));
            }
        }
        a_cursor = replay.index_a;
        let sm = world.get::<SmState>(h.fighter).unwrap();
        let pose = world.get::<PoseRuntime>(h.fighter).unwrap();
        let reaction = world.get::<FighterHitReaction>(h.fighter).unwrap();
        let reaction_kind = reaction.current.as_ref().map(|r| r.kind);
        let air = world.get::<FighterAir>(h.fighter).unwrap();
        let signature = (sm.current, pose.pose_id, reaction_kind, air.air.airborne);
        let pose_entry = previous_pose_instance.is_some_and(|id| id != pose.pose_instance_id);
        if time >= case.audit_start {
            if previous != Some(signature) || edges.is_empty() {
                edges.push(json!({"time": time, "state": format!("{:?}", sm.current), "pose": pose.pose_id,
                    "reaction": reaction_kind.map(|kind| format!("{kind:?}")), "airborne": air.air.airborne}));
            }
            if pose_entry {
                pose_entries.push(json!([
                    time,
                    pose.pose_id,
                    format!("{:?}", sm.current),
                    air.air.airborne
                ]));
            }
            if world.resource::<LocalJumpInput>().action_accepted {
                jump_accepts.push(time);
            }
        }
        previous = Some(signature);
        previous_pose_instance = Some(pose.pose_instance_id);
        if f64::from(time) < next_sample as f64 / 20.0 {
            continue;
        }
        next_sample += 1;
        let local = snapshot(world, h.fighter);
        let remote = remotes
            .iter(world)
            .find(|(_, fighter)| fighter.player_slot == case.remote)
            .map(|(entity, _)| snapshot(world, entity));
        let packet = received.map(|(stamp, index, words)| {
            json!({"time": stamp, "index": index,
            "pose": words.pose_id, "grounded": words.grounded, "guard_route": words.guard_route})
        });
        let lock_entity = world.resource::<LockOnController>().lock_entity;
        let lock_actor = lock_entity.and_then(|entity| {
            world.get::<RemoteFighter>(entity)?;
            world
                .get::<Fighter>(entity)
                .map(|fighter| fighter.player_slot)
        });
        observations.push(
            json!({"time": time, "local": local, "remote": remote, "packet": packet,
            "input_cursor": replay.index_b,
            "lock_active": lock_entity.is_some(), "lock_actor": lock_actor}),
        );
    }
    assert!(!observations.is_empty(), "{}", case.file);
    assert_eq!(window_weapon, 5, "{} window weapon is a dagger", case.file);
    assert_eq!(
        window_weapon_mismatch, 0,
        "{} local weapon left the window item: {:?}",
        case.file, first_weapon_mismatch
    );
    json!({"file": case.file, "stage": case.stage, "local_actor": case.local, "remote_actor": case.remote,
        "training_eligible": false, "start": case.audit_start, "end_exclusive": case.audit_end,
        "simulation_hz": 120, "observation_hz": 20, "scene_center": center,
        "plane_world_y": case.plane_world_y, "plane_y": plane_y,
        "local_spawn_item": case.local_spawn_item, "local_window_item": case.local_window_item,
        "local_equipment": {"applied": applied_equipment, "window_weapon_mismatch": window_weapon_mismatch},
        "observations": observations, "edges": edges,
        "pose_entries": pose_entries, "jump_accepts": jump_accepts, "consumed_queue_a_events": direct_hit_events})
}

fn audit_plane(y: f32) -> StageMeshCollider {
    let mut mesh = StageMeshCollider::default();
    const R: f32 = 600.0;
    mesh.append_world_triangles(
        &[[-R, y, -R], [-R, y, R], [R, y, R], [R, y, -R]],
        &[0, 1, 2, 0, 2, 3],
    );
    mesh
}

fn snapshot(world: &World, entity: Entity) -> Value {
    let fighter = world.get::<Fighter>(entity).unwrap();
    let pose = world.get::<PoseRuntime>(entity).unwrap();
    let blend = world.get::<EntryBlend>(entity).unwrap();
    let motion = world.get::<MotionState>(entity).unwrap();
    let air = world.get::<FighterAir>(entity).unwrap();
    let sm = world.get::<SmState>(entity).unwrap();
    let health = world.get::<Health>(entity).unwrap();
    let qi = world.get::<Qi>(entity).unwrap();
    let reaction = world.get::<FighterHitReaction>(entity);
    let assets = world.resource::<CharacterAssetsSet>();
    let character = assets.0.get(&format!("p{}", fighter.character_id));
    let pose_data = character.and_then(|character| character.poses.get(&pose.pose_id));
    let pose_age = pose_data.map(|data| pose_elapsed_seconds_at_frame(data, pose.frame));
    let pose_remaining = pose_data
        .zip(pose_age)
        .map(|(data, age)| (pose_elapsed_seconds_at_frame(data, data.end as f32) - age).max(0.0));

    let pilot_attack = pose_data.map(|data| {
        data.attacks
            .iter()
            .any(|attack| (attack.start as f32..=attack.end as f32).contains(&pose.frame))
    });
    let combat_window = character.and_then(|character| {
        sampled_attack_window(character, pose, blend).map(|(index, _)| index)
    });
    json!({"actor": fighter.player_slot, "hero": fighter.character_id, "weapon": fighter.weapon, "pose": pose.pose_id,
        "pose_frame": pose.frame, "pose_age": pose_age, "pose_remaining": pose_remaining,
        "pilot_attack_active": pilot_attack, "combat_attack_window": combat_window,
        "state": format!("{:?}", sm.current), "movement": format!("{:?}", motion.movement_mode),
        "has_reaction_component": reaction.is_some(), "reaction": reaction.and_then(|r| r.current.as_ref())
            .map(|r| format!("{:?}", r.kind)), "grounded": !air.air.airborne,
        "air_time": if air.air.airborne { air.air.air_time } else { 0.0 },
        "position": world.get::<Transform>(entity).unwrap().translation.to_array(),
        "velocity": motion.velocity.to_array(), "facing": motion.facing,
        "hp": health.current, "max_hp": health.max, "anger": qi.current})
}
