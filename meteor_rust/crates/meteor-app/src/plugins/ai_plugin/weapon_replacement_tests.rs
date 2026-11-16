fn replacement_seed(coin: u32) -> u64 {
    (1..10000)
        .find(|seed| {
            let mut rng = *seed;

            next_npc_rand(&mut rng);
            next_npc_rand(&mut rng);
            ((next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32) % 2 == coin
        })
        .unwrap()
}

fn replacement_graph() -> WaypointGraph {
    parse_waypoint_graph(
        "WayPoints 2\nPos 0 0 30\nSize 1\nLink 1\n1 0 125\nPos 125 0 30\nSize 1\nLink 1\n0 0 125\n",
        [0.0; 3],
    )
}

fn replacement_prop(model: &str) -> StageProp {
    StageProp {
        name: "D_replacement".into(),
        model: model.into(),
        center_offset: Vec3::Y,
        half_extents: Vec3::ONE,
        hit_count: 0,
        respawn_timer: 0.0,
        interactive: true,
        collision_enabled: false,
        breakable: false,
        broken: false,
    }
}

#[test]
fn npc_weapon_replacement_coin_empty_slot_duplicates_and_route_gates() {
    let graph = replacement_graph();
    let mut fighter = Fighter {
        character_id: 15,
        player_slot: 1,
        weapon: 8,
        weapon_variant: 3,
        weapon_slots: [8, 7],
        weapon_slot: 0,
        weapon_slot_variants: [3, 3],
    };
    let target = Vec3::new(15.0, 1.0, 0.0);
    for (spare, candidate, coin, expected, consumes_coin) in [
        (7, (5, 0), 0, Some(true), true),
        (7, (5, 0), 1, None, true),
        (0, (5, 0), 1, Some(false), false),
        (7, (8, 3), 0, None, false),
        (7, (7, 3), 0, None, false),
        (7, (8, 0), 0, Some(true), true),
    ] {
        fighter.weapon_slots[1] = spare;
        let seed = replacement_seed(coin);
        let mut patrol = NpcPatrol {
            rng: seed,
            ..default()
        };
        let result = weapon_replacement::admit_weapon_getitem(
            &mut patrol,
            &fighter,
            candidate,
            Vec3::ZERO,
            target,
            Some(&graph),
            None,
            1.0 / 60.0,
            true,
        );
        assert_eq!(result.as_ref().ok().map(|r| r.is_some()), expected);
        let duplicate = candidate == (8, 3) || (spare != 0 && candidate == (spare, 3));
        let mut expected_rng = seed;
        if !duplicate {
            next_npc_rand(&mut expected_rng);
            next_npc_rand(&mut expected_rng);
        }
        if consumes_coin {
            next_npc_rand(&mut expected_rng);
        }
        assert_eq!(
            patrol.rng, expected_rng,
            "only goal jitter and admitted spare coin consume RNG"
        );
        assert_eq!(
            fighter.weapon_slots,
            [8, spare],
            "decision must only request a drop"
        );
    }
    fighter.weapon_slots[1] = 7;
    for (graph, target, offline) in [
        (None, target, true),
        (Some(&graph), target + Vec3::Y * 12.0, true),
        (Some(&graph), target, false),
    ] {
        let mut patrol = NpcPatrol {
            rng: replacement_seed(0),
            ..default()
        };
        assert!(weapon_replacement::admit_weapon_getitem(
            &mut patrol,
            &fighter,
            (5, 0),
            Vec3::ZERO,
            target,
            graph,
            None,
            1.0 / 60.0,
            offline,
        )
        .is_err());
    }
    let mut disconnected = replacement_graph();
    disconnected.points[0].links.clear();
    disconnected.points[1].links.clear();
    disconnected.points[1].position[0] = 500.0;
    let mut patrol = NpcPatrol {
        rng: replacement_seed(0),
        ..default()
    };
    assert!(weapon_replacement::admit_weapon_getitem(
        &mut patrol,
        &fighter,
        (5, 0),
        Vec3::ZERO,
        Vec3::new(60.0, 3.6, 0.0),
        Some(&disconnected),
        None,
        1.0 / 60.0,
        true,
    )
    .is_err());
}

#[test]
fn npc_weapon_drop_consumer_handles_reversed_slots_and_rejects_stale_requests() {
    use crate::plugins::render_plugin::{install_npc_drop_test_systems, DynamicWeaponDrop};
    for case in ["accept", "stale", "dead", "room"] {
        let (mut app, npc) = perception_test_app(BehaviorMode::Wait, Vec3::splat(1000.0), None);
        install_npc_drop_test_systems(&mut app);
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(npc).unwrap();
            fighter.weapon = 8;
            fighter.weapon_variant = 3;
            fighter.weapon_slot = 1;
            fighter.weapon_slots = [7, 8];
            fighter.weapon_slot_variants = [3, 3];
        }
        app.world_mut()
            .entity_mut(npc)
            .insert(NpcWeaponDropRequest {
                spare: (7, if case == "stale" { 0 } else { 3 }),
                position: Vec3::ZERO,
                facing: std::f32::consts::FRAC_PI_2,
            });
        if case == "dead" {
            app.world_mut().get_mut::<Health>(npc).unwrap().current = 0.0;
        }
        if case == "room" {
            app.world_mut()
                .resource_mut::<crate::plugins::network_plugin::NetState>()
                .status = crate::plugins::network_plugin::NetStatus::InRoom;
        }
        app.update();
        let fighter = app.world().get::<Fighter>(npc).unwrap();
        assert_eq!(
            (fighter.weapon, fighter.weapon_variant, fighter.weapon_slot),
            (8, 3, 1)
        );
        assert_eq!(
            fighter.weapon_slots,
            if case == "accept" { [0, 8] } else { [7, 8] }
        );
        assert!(
            app.world().get::<NpcWeaponDropRequest>(npc).is_none(),
            "consume exactly once"
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<DynamicWeaponDrop>>()
                .iter(app.world())
                .count(),
            usize::from(case == "accept")
        );
        app.update();
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<DynamicWeaponDrop>>()
                .iter(app.world())
                .count(),
            usize::from(case == "accept")
        );
    }
}

#[test]
fn production_sn10_far_side_goal_allows_reacquiring_landed_spare() {
    sn10_replacement_contact_case(true, true);
}

#[test]
fn production_sn10_near_side_goal_allows_reacquiring_landed_spare() {
    sn10_replacement_contact_case(false, true);
}

#[test]
fn production_sn10_empty_spare_picks_selected_weapon() {
    sn10_replacement_contact_case(true, false);
}

fn sn10_replacement_contact_case(far_side_goal: bool, initially_has_spare: bool) {
    use crate::components::fighter::{Concealment, FighterStatusEffects, Qi, WornEffect};
    use crate::plugins::item_plugin::{item_pickup, ItemBuffs};
    use crate::plugins::render_plugin::{install_npc_drop_test_systems, DynamicWeaponDrop};
    use crate::plugins::stage_plugin::StageScriptRuntime;
    for fps in [30, 60, 120] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../origin_res/Level/npc10_01.pst");
        let source = meteor_scene::text::decode_cp936(&std::fs::read(root).unwrap());
        let definition = meteor_scene::mission_runtime::parse_npc_definition(&source, "npc10_01");
        let primary =
            crate::plugins::character_plugin::weapon_slot_from_item(definition.weapon).unwrap();
        let spare =
            crate::plugins::character_plugin::weapon_slot_from_item(definition.weapon2).unwrap();
        assert_eq!((primary, spare), ((8, 3), (7, 3)));
        let (mut app, npc) = perception_test_app(BehaviorMode::Wait, Vec3::splat(1000.0), None);
        let mut fighter = app.world_mut().get_mut::<Fighter>(npc).unwrap();
        fighter.character_id = definition.model as u8;
        fighter.weapon = primary.0;
        fighter.weapon_variant = primary.1;
        fighter.weapon_slots = [primary.0, if initially_has_spare { spare.0 } else { 0 }];
        fighter.weapon_slot_variants = [primary.1, spare.1];
        drop(fighter);
        {
            let mut mission = app.world_mut().get_mut::<MissionNpc>(npc).unwrap();
            mission.name = definition.name;
            mission.view = definition.view as f32;
            mission.think = definition.think as f32;
        }
        app.world_mut().entity_mut(npc).insert((
            NpcCombatProfile {
                get_item: definition.get_item as f32,
                ..default()
            },
            WornEffect::default(),
            Concealment::default(),
            FighterStatusEffects::default(),
            Qi::default(),
            FighterDodge::default(),
            PoseRuntime::default(),
            EntryBlend::default(),
            SmState::default(),
            CombatState::default(),
        ));
        app.world_mut().get_mut::<MotionState>(npc).unwrap().facing = std::f32::consts::FRAC_PI_2;
        app.insert_resource(replacement_graph());
        let mut floor = StageMeshCollider::default();
        floor.append_world_triangles(
            &[
                [-100.0, 0.0, -100.0],
                [100.0, 0.0, -100.0],
                [100.0, 0.0, 100.0],
                [-100.0, 0.0, 100.0],
            ],
            &[0, 2, 1, 0, 3, 2],
        );
        app.insert_resource(StageCollider { mesh: Some(floor) });
        app.init_resource::<ItemBuffs>()
            .init_resource::<StageScriptRuntime>()
            .add_systems(Update, item_pickup.before(drive_npc_ai))
            .add_systems(Update, move_npc_fighters.after(drive_npc_ai));
        install_npc_drop_test_systems(&mut app);
        let item = app
            .world_mut()
            .spawn((
                replacement_prop("w1_0"),
                Transform::from_xyz(15.0, 0.0, 0.0),
                Visibility::Visible,
            ))
            .id();
        let mut saw_drop = false;

        app.world_mut().get_mut::<NpcPatrol>(npc).unwrap().rng = 0;
        let mut saw_selected = false;
        let mut picked = false;
        let mut returned = false;
        let mut reacquired_spare = false;
        for _ in 0..fps * 12 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
            app.update();
            let fighter = app.world().get::<Fighter>(npc).unwrap();
            assert_eq!(
                (fighter.weapon, fighter.weapon_variant),
                primary,
                "drop must preserve the hand weapon"
            );
            let patrol = app.world().get::<NpcPatrol>(npc).unwrap();
            let first_selected = patrol.getitem_entity == Some(item) && !saw_selected;
            if patrol.getitem_entity == Some(item) {
                saw_selected = true;
            }
            if saw_selected && patrol.getitem_entity.is_none() {
                returned = true;
            }
            reacquired_spare |= saw_drop && fighter.weapon_slots[1] == spare.0;
            if initially_has_spare && fighter.weapon_slots[1] == 0 && !saw_drop {
                saw_drop = true;
                assert_eq!(
                    patrol.getitem_entity,
                    Some(item),
                    "drop keeps the chosen target: fps={fps} pos={:?} intent={:?} patrol={:?}",
                    app.world().get::<Transform>(npc),
                    app.world().get::<AiIntent>(npc),
                    patrol
                );
                let world = app.world_mut();
                let drops: Vec<_> = world
                    .query_filtered::<(&StageProp, &Transform), With<DynamicWeaponDrop>>()
                    .iter(world)
                    .collect();
                assert_eq!(
                    drops.len(),
                    1,
                    "one successful decision makes exactly one physical drop"
                );
                assert_eq!(drops[0].0.model.to_ascii_lowercase(), "w7_3");
                assert!(
                    drops[0].1.translation.x >= 0.0,
                    "drop follows the NPC's facing"
                );
                assert!(
                    !world
                        .query::<&MeshMaterial3d<crate::plugins::stage_material::GmdlMaterial>>()
                        .iter(world)
                        .collect::<Vec<_>>()
                        .is_empty(),
                    "original weapon must be rendered"
                );
            }
            if first_selected && far_side_goal {
                let seed = (1..100_000)
                    .find(|seed| {
                        let mut rng = *seed;
                        let x = (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32;
                        let z = (next_npc_rand(&mut rng) * 0x0100_0000 as f32) as u32;
                        x % 10 == 9 && z % 10 == 5
                    })
                    .unwrap();
                app.world_mut().get_mut::<NpcPatrol>(npc).unwrap().rng = seed;
            }
            if app.world().get::<Fighter>(npc).unwrap().weapon_slots[1] == 5 {
                picked = true;
                assert_eq!(
                    *app.world().get::<Visibility>(item).unwrap(),
                    Visibility::Hidden
                );
                break;
            }
            if returned {
                break;
            }
        }

        let correct_inventory = if initially_has_spare {
            reacquired_spare && !picked && returned
        } else {
            picked
        };
        assert!(
            saw_selected && saw_drop == initially_has_spare && correct_inventory,
            "fps={fps} far_side={far_side_goal} spare={initially_has_spare} selected={saw_selected} dropped={saw_drop} picked={picked} reacquired={reacquired_spare} returned={returned} pos={:?} fighter={:?} intent={:?} patrol={:?} air={:?}",
            app.world().get::<Transform>(npc), app.world().get::<Fighter>(npc),
            app.world().get::<AiIntent>(npc), app.world().get::<NpcPatrol>(npc),
            app.world().get::<FighterAir>(npc).map(|air| (
                air.air.airborne, air.recovery_ground_mode, air.ground_contact,
                air.air.planar_x, air.air.vertical_speed, air.air.planar_z))
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn npc_stock_drop_consumer_sends_own_role_once_and_waits_for_entry() {
    use crate::plugins::network_plugin::{npc_drop_test_channel, NetState, NetStatus};
    use crate::plugins::render_plugin::{install_npc_drop_test_systems, DynamicWeaponDrop};
    for ready in [false, true] {
        let (mut app, npc) = perception_test_app(BehaviorMode::Idle, Vec3::splat(1000.0), None);
        install_npc_drop_test_systems(&mut app);
        let (tx, received) = npc_drop_test_channel();
        app.insert_resource(tx);
        {
            let mut net = app.world_mut().resource_mut::<NetState>();
            net.status = NetStatus::InRoom;
            net.address = "udp-host://127.0.0.1:5230".into();
            net.round_active = true;
            if ready {
                net.stock_npc_ready.insert(npc);
            }
        }
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(npc).unwrap();
            fighter.weapon = 8;
            fighter.weapon_variant = 3;
            fighter.weapon_slots = [8, 7];
            fighter.weapon_slot_variants = [3, 3];
        }
        app.world_mut()
            .entity_mut(npc)
            .insert(NpcWeaponDropRequest {
                spare: (7, 3),
                position: Vec3::X * 12.0,
                facing: std::f32::consts::FRAC_PI_2,
            });
        app.update();
        let fighter = app.world().get::<Fighter>(npc).unwrap();
        assert_eq!((fighter.weapon, fighter.weapon_variant), (8, 3));
        assert_eq!(fighter.weapon_slots, if ready { [8, 0] } else { [8, 7] });
        let sent = received();
        assert_eq!(sent.len(), usize::from(ready));
        if ready {
            assert_eq!(sent[0].0, npc);
            assert_eq!(
                sent[0].1.item_id,
                meteor_network::stock_protocol::stock_weapon_item_id(7, 3).unwrap()
            );
            assert_eq!(
                sent[0].1.remaining_item_id,
                meteor_network::stock_protocol::stock_weapon_item_id(8, 3)
            );
        }
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<DynamicWeaponDrop>>()
                .iter(app.world())
                .count(),
            0,
            "the stock authority owns creation; no offline duplicate"
        );
        app.update();
        assert!(received().is_empty());
    }
}
