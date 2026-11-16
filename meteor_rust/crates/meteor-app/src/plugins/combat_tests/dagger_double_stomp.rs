fn stomp_audit_output(root: &std::path::Path, filename: &str) -> std::path::PathBuf {
    let folder = std::env::var_os("METEOR_STOMP_AUDIT_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("analysis/dagger-double-stomp-20260929"));
    std::fs::create_dir_all(&folder).unwrap();
    folder.join(filename)
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "writes a contact scan under analysis; run explicitly for Pose 256 investigation"]
fn diagnose_dagger_double_stomp_recorded_contact() {
    use bevy::ecs::system::RunSystemOnce;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let effect = meteor_asset::pon_ef::parse_pon_ef(
        &std::fs::read(root.join("origin_res/SFX/pos256.ef")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        effect
            .nodes
            .iter()
            .filter(|node| node.is_attack_node())
            .count(),
        0,
        "Pos256 is visual-only; its flash cannot supply the second hit"
    );
    let (mut h, target, take_hits) = spawn_local_stock_hit_harness_with_order(DAGGER, true);
    let p0 = crate::plugins::character_plugin::load_character_asset_for_test(
        std::slice::from_ref(&root.join("origin_res")),
        "p0",
    )
    .expect("original P0");
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
        crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), entity);
    }
    h.app
        .world_mut()
        .get_mut::<MotionState>(h.fighter)
        .unwrap()
        .facing = -2.338;
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
    h.app
        .world_mut()
        .get_mut::<FighterAir>(h.fighter)
        .unwrap()
        .air
        .airborne = true;
    h.app
        .world_mut()
        .get_mut::<SmState>(target)
        .unwrap()
        .current = FighterState::Down;
    h.app
        .world_mut()
        .get_mut::<CombatState>(target)
        .unwrap()
        .state = FighterState::Down;
    let mut down = PoseRuntime::idle();
    h.app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut down, 112, true);
    h.app.world_mut().entity_mut(target).insert(down);
    let mut pose = PoseRuntime::idle();
    h.app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut pose, 256, true);
    h.app.world_mut().entity_mut(h.fighter).insert(pose);
    #[derive(Resource)]
    struct Pair {
        source: Entity,
        target: Entity,
    }
    fn probe(
        geometry: HitGeometryQueries,
        assets: Res<CharacterAssetsSet>,
        pair: Res<Pair>,
        actors: Query<(&Fighter, &WeaponForm, &PoseRuntime, &EntryBlend)>,
    ) -> (usize, usize, bool, bool, [f32; 3], [f32; 3], f32) {
        let (fighter, form, pose, blend) = actors.get(pair.source).unwrap();
        let (window, active) = sampled_attack_window(&assets.0["p0"], pose, blend)
            .map_or((false, None), |(_, w)| (true, Some(w)));
        let attacks = active.map_or(Vec::new(), |w| {
            geometry.attack_obbs(
                pair.source,
                fighter,
                form,
                pose.pose_id,
                &assets.0["p0"],
                &w.bones,
            )
        });
        let hurts = geometry.hurt_obbs(pair.target, &assets.0["p0"]);
        let hit = attacks
            .iter()
            .any(|a| hurts.iter().any(|b| obbs_intersect(a, b)));
        let closest = attacks
            .iter()
            .flat_map(|a| {
                hurts.iter().map(move |b| {
                    (
                        a,
                        b,
                        (a.center.x - b.center.x)
                            .hypot(a.center.y - b.center.y)
                            .hypot(a.center.z - b.center.z),
                    )
                })
            })
            .min_by(|a, b| a.2.total_cmp(&b.2));
        let (attack_center, hurt_center, center_distance) =
            closest.map_or(([0.0; 3], [0.0; 3], f32::INFINITY), |(a, b, d)| {
                (
                    [a.center.x, a.center.y, a.center.z],
                    [b.center.x, b.center.y, b.center.z],
                    d,
                )
            });
        (
            attacks.len(),
            hurts.len(),
            hit,
            window,
            attack_center,
            hurt_center,
            center_distance,
        )
    }
    h.app.insert_resource(Pair {
        source: h.fighter,
        target,
    });
    let mut rows = Vec::new();
    for frame in 9384..=9392 {
        for (dx, dz, height) in [
            (-5.59, -1.80, 4.75),
            (-5.59, -1.80, 0.0),
            (-4.5, -1.8, 3.4),
            (-4.5, -1.8, 0.0),
            (-3.0, -1.2, 2.0),
            (-3.0, -1.2, 0.0),
            (-1.5, -0.6, 1.0),
            (-1.5, -0.6, 0.0),
            (0.0, 0.0, 1.0),
            (0.0, 0.0, 0.0),
        ] {
            h.app
                .world_mut()
                .get_mut::<PoseRuntime>(h.fighter)
                .unwrap()
                .frame = frame as f32;
            h.app
                .world_mut()
                .get_mut::<Transform>(h.fighter)
                .unwrap()
                .translation = Vec3::new(0.0, height, 0.0);
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = Vec3::new(dx, 0.0, dz);
            h.app
                .world_mut()
                .get_mut::<TargetHitClock>(target)
                .unwrap()
                .last_hit_at = f64::NEG_INFINITY;
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            let (
                attack_count,
                hurt_count,
                direct_hit,
                active_window,
                attack_center,
                hurt_center,
                center_distance,
            ) = h.app.world_mut().run_system_once(probe).unwrap();
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            let hits = take_hits();
            rows.push(serde_json::json!({"frame":frame,"target":[dx,0.0,dz],"height":height,
                "attack_count":attack_count,"hurt_count":hurt_count,"direct_hit":direct_hit,"active_window":active_window,
                "attack_center":attack_center,"hurt_center":hurt_center,"center_distance":center_distance,
                "claims":hits.iter().map(|(_,r)|r.target_actor_id).collect::<Vec<_>>() }));
        }
    }
    let out = stomp_audit_output(&root, "pose256-contact-scan.json");
    std::fs::write(out, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    eprintln!(
        "claims {} of {}",
        rows.iter()
            .filter(|r| r["claims"].as_array().unwrap().len() > 0)
            .count(),
        rows.len()
    );
    assert!(rows.iter().any(|r| {
        r["claims"]
            .as_array()
            .is_some_and(|claims| !claims.is_empty())
    }));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "writes recorded-input diagnostics under analysis"]
fn diagnose_dagger_double_stomp_recorded_input_entry() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let doc = meteor_asset::mrc::decode_mrc(
        &std::fs::read(root.join("mrcs/tests/匕首小跳双踩.mrc")).unwrap(),
    )
    .unwrap();
    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).unwrap();
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
    let keys = crate::app_state::Keymap::default();
    let input: Vec<_> = doc
        .queue_b
        .iter()
        .filter(|b| (10.75..12.30).contains(&b.time))
        .collect();
    let mut next = 0;
    let mut rows = Vec::new();
    for tick in 0..(1.55 * 60.0) as usize {
        let time = 10.75 + (tick + 1) as f32 / 60.0;
        while next < input.len() && input[next].time <= time {
            let event = input[next];
            let press = event.value != 0;
            let binding = match event.code {
                0x57 => Some(keys.move_up),
                0x53 => Some(keys.move_down),
                0x41 => Some(keys.move_left),
                0x44 => Some(keys.move_right),
                0x1000 => Some(keys.attack),
                0x1200 => Some(keys.jump),
                0x51 => Some(keys.lock_toggle),
                _ => None,
            };
            if let Some(binding) = binding {
                if press {
                    h.press_binding(binding)
                } else {
                    h.release_binding(binding)
                }
            }
            next += 1;
        }
        h.step_dt(1.0 / 60.0);
        rows.push(
            serde_json::json!({"time":time,"pose":h.pose_id(),"state":format!("{:?}",h.state()),
            "position":h.position().to_array(),"airborne":h.airborne()}),
        );
    }
    std::fs::write(
        stomp_audit_output(&root, "recorded-input-entry.json"),
        serde_json::to_vec_pretty(&rows).unwrap(),
    )
    .unwrap();
    let poses: std::collections::BTreeSet<_> =
        rows.iter().filter_map(|r| r["pose"].as_u64()).collect();
    eprintln!("recorded input poses {poses:?}");
    assert!(
        poses.contains(&256),
        "recorded input must enter the double-stomp pose"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
#[ignore = "writes conditional stock-NPC/remote traces under analysis"]
fn diagnose_dagger_double_stomp_recorded_lifecycle() {
    check_dagger_double_stomp_recorded_lifecycle(false);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn production_dagger_recorded_second_stomp_emits_two_distinct_103_claims() {
    check_dagger_double_stomp_recorded_lifecycle(true);
}

#[cfg(not(target_arch = "wasm32"))]
fn check_dagger_double_stomp_recorded_lifecycle(regression: bool) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let doc = meteor_asset::mrc::decode_mrc(
        &std::fs::read(root.join("mrcs/tests/匕首小跳双踩.mrc")).unwrap(),
    )
    .unwrap();
    let inputs: Vec<_> = doc
        .queue_b
        .iter()
        .filter(|b| (10.75..12.75).contains(&b.time))
        .collect();
    let stage = root.join("origin_res/sn06");
    let stage_model =
        meteor_asset::gmdl::decode_gmdl(&std::fs::read(stage.join("sn06.gmb")).unwrap()).unwrap();
    let stage_description = meteor_asset::des::parse_stage_description(
        &crate::text_codec::decode_text(&std::fs::read(stage.join("sn06.des")).unwrap()),
    );
    let stage_collision =
        meteor_asset::cob::parse_cob(&std::fs::read(stage.join("sn06.cob")).unwrap()).unwrap();

    let stage_center = [-41.5666885, -8.820775, -23.5];
    let stage_mesh = StageMeshCollider::from_gmdl(
        &stage_model,
        &stage_description,
        Some(&stage_collision.face_ids),
        stage_center,
    );
    let second_stage_center = [-72.195, 39.778, -23.5];
    let second_stage_mesh = StageMeshCollider::from_gmdl(
        &stage_model,
        &stage_description,
        Some(&stage_collision.face_ids),
        second_stage_center,
    );
    for (
        fps,
        remote_target,
        entry_velocity,
        original_stage,
        recorded_stream,
        recorded_lock,
        full_history,
    ) in [
        (
            60,
            false,
            Some([-2.11, -13.90, 6.05]),
            false,
            false,
            false,
            false,
        ),
        (
            120,
            false,
            Some([-2.11, -13.90, 6.05]),
            false,
            false,
            false,
            false,
        ),
        (
            60,
            true,
            Some([-2.11, -13.90, 6.05]),
            false,
            false,
            false,
            false,
        ),
        (
            120,
            true,
            Some([-2.11, -13.90, 6.05]),
            false,
            false,
            false,
            false,
        ),
        (
            60,
            true,
            Some([0.0, -13.90, 0.0]),
            false,
            false,
            false,
            false,
        ),
        (
            60,
            true,
            Some([-2.11, -13.90, 6.05]),
            true,
            false,
            false,
            false,
        ),
        (
            120,
            true,
            Some([-2.11, -13.90, 6.05]),
            true,
            false,
            false,
            false,
        ),
        (60, true, None, false, false, false, false),
        (120, true, None, false, false, false, false),
        (
            60,
            true,
            Some([-2.11, -13.90, 6.05]),
            true,
            true,
            false,
            false,
        ),
        (
            120,
            true,
            Some([-2.11, -13.90, 6.05]),
            true,
            true,
            false,
            false,
        ),
        (
            60,
            true,
            Some([-2.11, -13.90, 6.05]),
            true,
            true,
            true,
            false,
        ),
        (
            120,
            true,
            Some([-2.11, -13.90, 6.05]),
            true,
            true,
            true,
            false,
        ),
        (
            60,
            true,
            Some([-2.11, -13.90, 6.05]),
            true,
            true,
            false,
            true,
        ),
        (
            120,
            true,
            Some([-2.11, -13.90, 6.05]),
            true,
            true,
            false,
            true,
        ),
        (60, true, Some([7.8, -10.8, 1.94]), true, true, false, true),
        (120, true, Some([7.8, -10.8, 1.94]), true, true, false, true),
    ] {
        let second_pair = entry_velocity == Some([7.8, -10.8, 1.94]);
        if regression && !(second_pair && fps == 120) {
            continue;
        }
        let stage_center = if second_pair {
            second_stage_center
        } else {
            stage_center
        };
        let inputs: Vec<_> = if second_pair {
            doc.queue_b
                .iter()
                .filter(|b| (14.9..18.4).contains(&b.time))
                .collect()
        } else {
            inputs.clone()
        };
        let (mut h, target, take_hits) = spawn_local_stock_hit_harness_with_order(DAGGER, true);
        if original_stage {
            h.replace_stage_mesh(if second_pair {
                second_stage_mesh.clone()
            } else {
                stage_mesh.clone()
            });
        }
        if remote_target {
            h.app.world_mut().entity_mut(target).insert(RemoteFighter {
                player_id: if recorded_stream {
                    "stock-1"
                } else {
                    "stock-2"
                }
                .into(),
            });
            if recorded_stream {
                use crate::plugins::network_plugin as network;
                h.app.world_mut().entity_mut(target).insert((
                    network::RemoteTransformTarget {
                        translation: Vec3::new(-5.392, 0.0, -2.379),
                        yaw: 0.7755659_f32.atan2(0.63126665),
                    },
                    network::RemoteContactSegment::default(),
                    network::RemoteHitMotionWindow::default(),
                    network::RemoteAuthorityGeneration::default(),
                ));
                let fighter = h.app.world().get::<Fighter>(h.fighter).unwrap().clone();
                h.app
                    .world_mut()
                    .entity_mut(h.fighter)
                    .remove::<Fighter>()
                    .insert(fighter);
                network::install_stock_observer_projection_for_test(&mut h.app);
            } else {
                crate::plugins::network_plugin::install_hit_sender_for_test(&mut h.app);
            }
            let mut net = h.app.world_mut().resource_mut::<NetState>();
            net.stock_npc_actors.remove(&target);
            net.stock_npc_ready.remove(&target);
            if recorded_stream {
                net.player_id = "stock-2".into();
                net.address = "udp://127.0.0.1:5230".into();
                net.remote_actor_profiles
                    .insert("stock-1".into(), (0, DAGGER));
            }
        }
        h.app.insert_resource(State::new(AppState::Playing));
        if recorded_lock {
            configure_lock_updates(&mut h.app);
        }
        h.app.add_plugins(crate::plugins::CharacterContactPlugin);
        h.app
            .world_mut()
            .entity_mut(h.fighter)
            .insert(HeadTrackState::default());
        let p0 = crate::plugins::character_plugin::load_character_asset_for_test(
            std::slice::from_ref(&root.join("origin_res")),
            "p0",
        )
        .unwrap();
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
        crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
        let schedule = super::super::fighter_frame::schedule(&h.app);
        h.app.insert_resource(StompTracePair {
            attacker: h.fighter,
            defender: target,
        });
        h.app.init_resource::<StompJudgeTrace>();
        if original_stage && fps == 60 && !recorded_stream {
            h.app.init_resource::<StompNativeGeometry>();
        }
        h.app.add_systems(
            schedule,
            (
                observe_stomp_hit_judge,
                export_stomp_geometry,
                attack_windows,
            )
                .chain()
                .in_set(crate::plugins::FighterFrameSet::HitJudge),
        );
        h.app.add_systems(
            schedule,
            hit_reaction_root_motion
                .in_set(crate::plugins::FighterFrameSet::RootMotion)
                .after(crate::plugins::movement_plugin::attack_root_motion)
                .after(crate::plugins::movement_plugin::dodge_root_motion)
                .after(crate::plugins::ai_plugin::npc_ballistic_air_root_motion),
        );
        let mut down = PoseRuntime::idle();
        h.app.world().resource::<CharacterAssetsSet>().0["p0"].apply_pose(&mut down, 112, true);
        h.app.world_mut().entity_mut(target).insert(down);
        h.app
            .world_mut()
            .get_mut::<SmState>(target)
            .unwrap()
            .current = FighterState::Down;
        h.app
            .world_mut()
            .get_mut::<CombatState>(target)
            .unwrap()
            .state = FighterState::Down;
        h.app
            .world_mut()
            .get_mut::<Transform>(target)
            .unwrap()
            .translation = Vec3::new(100.0, 0.0, 100.0);
        let yaw = if second_pair {
            (-0.716_f32).atan2(0.698)
        } else {
            (-0.7199105_f32).atan2(-0.69406694)
        };
        h.app
            .world_mut()
            .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
            .yaw = yaw;
        h.app
            .world_mut()
            .get_mut::<MotionState>(h.fighter)
            .unwrap()
            .facing = yaw;
        let mut received = Vec::new();
        let mut initial_role = None;
        let stream_start = if second_pair {
            14.9
        } else if full_history {
            9.9
        } else {
            11.374609
        };
        let stream_end = if second_pair { 18.4 } else { 12.75 };
        if recorded_stream {
            use meteor_network::stock_game::StockGameEvent;
            use meteor_network::stock_protocol::{decode_role_state_event, StockSceneFrame};
            let scene = StockSceneFrame(stage_center);
            for row in &doc.queue_a {
                if row.id == 2 {
                    for mut role in decode_role_state_event(&row.blob)
                        .unwrap()
                        .into_iter()
                        .filter(|role| role.actor_id == 1)
                    {
                        role.state = scene.to_scene(role.state);
                        if row.time <= stream_start {
                            initial_role = Some(role);
                        } else if row.time < stream_end {
                            received.push((row.time, StockGameEvent::RoleStates(vec![role])));
                        }
                    }
                } else if row.time > stream_start && row.time < stream_end {
                    let event = match row.id {
                        11 | 14 => {
                            let event =
                                meteor_network::stock_protocol::StockHitReactionEvent::decode(
                                    &row.blob,
                                )
                                .unwrap();
                            (event.target_actor_id == 1).then_some(StockGameEvent::HitReaction {
                                guarded: row.id == 14,
                                event,
                            })
                        }
                        51 => {
                            let event =
                                meteor_network::stock_protocol::StockPoseChainEvent::decode(
                                    &row.blob,
                                )
                                .unwrap();
                            (event.actor_id == 1).then_some(StockGameEvent::PoseChain(event))
                        }
                        _ => None,
                    };
                    if let Some(event) = event {
                        received.push((row.time, event));
                    }
                }
            }
            received.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        if full_history {
            crate::plugins::network_plugin::feed_stock_observer_event_for_test(
                h.app.world_mut(),
                meteor_network::stock_game::StockGameEvent::RoleStates(vec![initial_role
                    .take()
                    .unwrap()]),
            );
        }
        let keymap = crate::app_state::Keymap::default();
        let mut next = 0;
        let mut next_received = 0;
        let mut anchored = false;
        let mut prepared_for_lock = false;
        let mut reacted = false;
        let mut first_claim = false;
        let mut rows = Vec::new();
        let base = if second_pair {
            14.9_f32
        } else if full_history {
            9.9_f32
        } else {
            10.75_f32
        };
        let end = if second_pair { 18.4_f32 } else { 12.75_f32 };
        let anchor_time = if second_pair { 15.494482 } else { 11.3746 };
        let anchor_height = if second_pair { 2.875 } else { 3.43 };
        for tick in 0..((end - base) * fps as f32) as usize {
            let t = base + (tick + 1) as f32 / fps as f32;
            if recorded_lock && !prepared_for_lock && t >= 11.35 {
                prepared_for_lock = true;

                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = Vec3::new(0.2008, 4.75, -0.5745);
                h.app
                    .world_mut()
                    .get_mut::<Transform>(target)
                    .unwrap()
                    .translation = Vec3::new(-5.392, 0.0, -2.379);
                crate::plugins::network_plugin::feed_stock_observer_event_for_test(
                    h.app.world_mut(),
                    meteor_network::stock_game::StockGameEvent::RoleStates(vec![initial_role
                        .take()
                        .unwrap()]),
                );
            }
            while next < inputs.len() && inputs[next].time <= t {
                let e = inputs[next];
                let binding = match e.code {
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
                    if e.value != 0 {
                        h.press_binding(binding)
                    } else {
                        h.release_binding(binding)
                    }
                }
                next += 1;
            }
            if !anchored && t >= anchor_time {
                anchored = true;
                h.app
                    .world_mut()
                    .get_mut::<Transform>(h.fighter)
                    .unwrap()
                    .translation = Vec3::new(0.0, anchor_height, 0.0);

                if let Some(velocity) = entry_velocity {
                    let observed = Vec3::from_array(velocity);
                    h.app
                        .world_mut()
                        .get_mut::<MotionState>(h.fighter)
                        .unwrap()
                        .velocity = observed;
                    let mut air = h.app.world_mut().get_mut::<FighterAir>(h.fighter).unwrap();
                    air.air.planar_x = observed.x;
                    air.air.planar_z = observed.z;
                    air.air.vertical_speed = observed.y;
                } else {
                    let motion = h.app.world().get::<MotionState>(h.fighter).unwrap();
                    let air = h.app.world().get::<FighterAir>(h.fighter).unwrap();
                    eprintln!(
                        "native {fps} entry velocity {:?}, air planar [{}, {}]",
                        motion.velocity, air.air.planar_x, air.air.planar_z
                    );
                }
                if !full_history {
                    h.app
                        .world_mut()
                        .get_mut::<Transform>(target)
                        .unwrap()
                        .translation = Vec3::new(-5.392, 0.0, -2.379);
                }
                if recorded_stream && !recorded_lock && !full_history {
                    crate::plugins::network_plugin::feed_stock_observer_event_for_test(
                        h.app.world_mut(),
                        meteor_network::stock_game::StockGameEvent::RoleStates(vec![initial_role
                            .take()
                            .unwrap()]),
                    );
                }
            }
            while next_received < received.len() && received[next_received].0 <= t {
                crate::plugins::network_plugin::feed_stock_observer_event_for_test(
                    h.app.world_mut(),
                    received[next_received].1.clone(),
                );
                next_received += 1;
            }
            if !recorded_stream && first_claim && !reacted && t >= 11.70 {
                reacted = true;
                let mut p = PoseRuntime::idle();
                h.app.world().resource::<CharacterAssetsSet>().0["p0"]
                    .apply_pose(&mut p, 103, true);
                let mut reaction = getup_reaction_for_pose(103, p.start, p.end);
                reaction.kind = HitReactKind::BeHit;
                reaction.motion_scale = 1.5;
                h.app
                    .world_mut()
                    .entity_mut(target)
                    .insert((p, EntryBlend::default()));
                h.app
                    .world_mut()
                    .get_mut::<SmState>(target)
                    .unwrap()
                    .current = FighterState::BeHit;
                h.app
                    .world_mut()
                    .get_mut::<CombatState>(target)
                    .unwrap()
                    .state = FighterState::BeHit;
                h.app
                    .world_mut()
                    .get_mut::<FighterHitReaction>(target)
                    .unwrap()
                    .begin(reaction, false);
            }
            h.step_duration(std::time::Duration::from_nanos(1_000_000_000 / fps as u64));
            let sent = take_hits();
            let claims: Vec<_> = sent.iter().map(|(_, r)| r.target_actor_id).collect();
            let claim_details: Vec<_> = sent
                .iter()
                .map(|(_, request)| {
                    serde_json::json!({
                        "target":request.target_actor_id,"reaction":request.target_pose(),
                        "direction":request.direction_selector(),"move":request.target_move(),
                        "minimum":request.target_value(),"strategy":request.hit_word>>16,
                    })
                })
                .collect();
            first_claim |= !claims.is_empty();
            let p = h.app.world().get::<PoseRuntime>(h.fighter).unwrap();
            let defender_pose = h.app.world().get::<PoseRuntime>(target).unwrap();
            let a = h.position();
            let b = h.app.world().get::<Transform>(target).unwrap().translation;
            rows.push(
                serde_json::json!({"time":t,"pose":p.pose_id,"frame":p.frame,
                "defender_pose":defender_pose.pose_id,"defender_frame":defender_pose.frame,
                "attacker":a.to_array(),"target":b.to_array(),"airborne":h.airborne(),
                "attacker_yaw":h.app.world().get::<MotionState>(h.fighter).unwrap().facing,
                "locked":h.app.world().resource::<LockOnController>().lock_entity.is_some(),
                "claims":claims,"claim_details":claim_details}),
            );
        }
        let mode = if second_pair {
            "remote-second"
        } else if full_history {
            "remote-recorded-history"
        } else if recorded_lock {
            "remote-recorded-lock"
        } else if recorded_stream {
            "remote-recorded"
        } else if original_stage {
            "remote-sn06"
        } else if !remote_target {
            "npc"
        } else if entry_velocity.is_none() {
            "remote-native"
        } else if entry_velocity == Some([0.0, -13.90, 0.0]) {
            "remote-zero-planar"
        } else {
            "remote"
        };
        if regression {
            let claimed: Vec<_> = rows
                .iter()
                .flat_map(|row| row["claim_details"].as_array().unwrap())
                .map(|claim| claim["reaction"].as_u64().unwrap())
                .collect();
            assert_eq!(
                claimed,
                vec![103, 103],
                "original second pair: two independent hits, not permanent window dedup"
            );
            let times: Vec<_> = rows
                .iter()
                .filter(|row| !row["claims"].as_array().unwrap().is_empty())
                .map(|row| row["time"].as_f64().unwrap())
                .collect();
            assert!(
                times[1] - times[0] > 0.2 && times[1] - times[0] < 0.23,
                "strict target interval on integer-nanosecond steps: {times:?}"
            );
            continue;
        }
        let out = stomp_audit_output(&root, &format!("recorded-lifecycle-{mode}-{fps}.json"));
        std::fs::write(out, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
        let judge_out = stomp_audit_output(&root, &format!("judge-{mode}-{fps}.json"));
        std::fs::write(
            judge_out,
            serde_json::to_vec_pretty(&h.app.world().resource::<StompJudgeTrace>().0).unwrap(),
        )
        .unwrap();
        if let Some(geometry) = h.app.world().get_resource::<StompNativeGeometry>() {
            let out = stomp_audit_output(&root, "native-geometry.json");
            std::fs::write(
                out,
                serde_json::to_vec_pretty(&serde_json::json!({"frames":geometry.0})).unwrap(),
            )
            .unwrap();
        }
        eprintln!(
            "{mode} {fps} FPS claims={} pose256-window-rows={}",
            rows.iter()
                .map(|r| r["claims"].as_array().unwrap().len())
                .sum::<usize>(),
            rows.iter()
                .filter(|r| r["pose"] == 256
                    && r["frame"]
                        .as_f64()
                        .is_some_and(|f| (9384.0..9393.0).contains(&f)))
                .count()
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct StompTracePair {
    attacker: Entity,
    defender: Entity,
}
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct StompJudgeTrace(Vec<serde_json::Value>);
#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
struct StompNativeGeometry(Vec<serde_json::Value>);

#[cfg(not(target_arch = "wasm32"))]
fn export_stomp_geometry(
    active: Option<Res<super::super::fighter_frame::ActiveFighter>>,
    mut audit: Option<ResMut<StompNativeGeometry>>,
    pair: Res<StompTracePair>,
    geometry: HitGeometryQueries,
    assets: Res<CharacterAssetsSet>,
    fighters: Query<(&Fighter, &WeaponForm, &PoseRuntime, &EntryBlend)>,
) {
    let Some(audit) = audit.as_deref_mut() else {
        return;
    };
    if !super::super::fighter_frame::accepts(active.as_deref(), pair.attacker) {
        return;
    }
    let Ok((attacker, form, pose, blend)) = fighters.get(pair.attacker) else {
        return;
    };
    if pose.pose_id != 256
        || ![9385.5, 9391.5]
            .iter()
            .any(|frame| (pose.frame - frame).abs() < 0.01)
    {
        return;
    }
    let asset = &assets.0["p0"];
    let Some((_, window)) = sampled_attack_window(asset, pose, blend) else {
        return;
    };
    let convert = (Mat4::from_scale(Vec3::splat(meteor_core::MODEL_SCALE))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2))
    .inverse();
    let vec = |v: meteor_core::Vec3| Vec3::new(v.x, v.y, v.z);
    let obb = |b: &Obb| {
        serde_json::json!({
            "center":convert.transform_point3(vec(b.center)).to_array(),
            "axes":b.axes.map(|axis|convert.transform_vector3(vec(axis)).normalize().to_array()),
            "half":(vec(b.half_size)/meteor_core::MODEL_SCALE).to_array(),
        })
    };
    let skeleton = |entity: Entity| {
        let model = find_fighter_model(entity, &geometry.models).unwrap();
        let mut bones = Vec::new();
        for (id, bone, propagated) in geometry
            .bones
            .iter_many(model_bones(model, &geometry.skeletons))
        {
            let animated = geometry.sampled_transform(id, propagated);
            let bound = asset
                .bone_local_bounds
                .get(bone.skin_index)
                .and_then(Option::as_ref)
                .and_then(|bounds| {
                    bone_world_obb(&bone_world_matrix(asset, bone, &animated).unwrap(), bounds)
                });
            bones.push(serde_json::json!({"index":bone.skin_index,"name":bone.name,
                "animated":(convert*animated.to_matrix()).to_cols_array(),
                "inverse_bind":asset.skeleton.skin_world_bind_inverse[bone.skin_index].m,
                "obb":bound.as_ref().map(&obb)}));
        }
        bones.sort_by_key(|bone| bone["index"].as_u64().unwrap());
        bones
    };
    let target_pose = fighters.get(pair.defender).unwrap().2;
    let attacks = geometry.attack_obbs(pair.attacker, attacker, form, 256, asset, &window.bones);
    let hurts = geometry.hurt_obbs(pair.defender, asset);
    audit.0.push(
        serde_json::json!({"frame":pose.frame,"character":0,"target_character":0,
        "target_pose":target_pose.pose_id,"target_frame":target_pose.frame,
        "source_bones":skeleton(pair.attacker),"target_bones":skeleton(pair.defender),
        "attack_names":window.bones,"effects":[],
        "rust_body_hit":attacks.iter().any(|a|hurts.iter().any(|b|obbs_intersect(a,b))),
        "rust_effect_hit":false}),
    );
}
#[cfg(not(target_arch = "wasm32"))]
fn observe_stomp_hit_judge(
    active_fighter: Option<Res<super::super::fighter_frame::ActiveFighter>>,
    geometry: HitGeometryQueries,
    assets: Res<CharacterAssetsSet>,
    pair: Res<StompTracePair>,
    actors: Query<(
        &Fighter,
        &PoseRuntime,
        &EntryBlend,
        &WeaponForm,
        &Transform,
        &MotionState,
    )>,
    clocks: Query<&TargetHitClock>,
    time: Res<Time>,
    mut trace: ResMut<StompJudgeTrace>,
) {
    if !super::super::fighter_frame::accepts(active_fighter.as_deref(), pair.attacker) {
        return;
    }
    let Ok((fighter, pose, blend, form, transform, motion)) = actors.get(pair.attacker) else {
        return;
    };
    if pose.pose_id != 256 {
        return;
    }
    let Some((_, window)) = sampled_attack_window(&assets.0["p0"], pose, blend) else {
        return;
    };
    let attack = geometry.attack_obbs(
        pair.attacker,
        fighter,
        form,
        256,
        &assets.0["p0"],
        &window.bones,
    );
    let hurt = geometry.hurt_obbs(pair.defender, &assets.0["p0"]);
    let intersect = attack
        .iter()
        .any(|a| hurt.iter().any(|b| obbs_intersect(a, b)));
    let distance = attack
        .iter()
        .flat_map(|a| {
            hurt.iter().map(move |b| {
                let v = a.center - b.center;
                (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
            })
        })
        .fold(f32::INFINITY, f32::min);
    let forward_hit_range = if [9386.0, 9391.5]
        .iter()
        .any(|frame| (pose.frame - frame).abs() < 0.01)
    {
        let forward = attack_forward_from_facing(motion.facing);
        let valid: Vec<_> = (-24..=48)
            .map(|step| step as f32 * 0.125)
            .filter(|shift| {
                attack.iter().any(|original| {
                    let mut shifted = original.clone();
                    shifted.center.x += forward.x * shift;
                    shifted.center.z += forward.z * shift;
                    hurt.iter().any(|body| obbs_intersect(&shifted, body))
                })
            })
            .collect();
        valid
            .first()
            .zip(valid.last())
            .map(|(first, last)| [*first, *last])
    } else {
        None
    };
    let hit_at = clocks.get(pair.defender).unwrap().last_hit_at;
    let defender = actors.get(pair.defender).unwrap().4.translation;
    trace.0.push(
        serde_json::json!({"time":time.elapsed_secs_f64(),"pose_frame":pose.frame,
        "attacker":transform.translation.to_array(),"defender":defender.to_array(),
        "intersect":intersect,"center_distance":distance,"forward_hit_range":forward_hit_range,
        "clock_delta":time.elapsed_secs_f64()-hit_at}),
    );
}
