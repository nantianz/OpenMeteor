#[test]
fn production_sn05_authored_fanxuan_patrol_reaches_each_destination() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let center = meteor_scene::stage_loader::get_stage_center("sn05", None);
    let mut graph = parse_waypoint_graph(
        &std::fs::read_to_string(root.join("Level/sn05.wp")).unwrap(),
        center,
    );
    let gmdl = meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn05/sn05.gmb")).unwrap())
        .unwrap();
    graph.navigation_bounds = super::super::npc_navigation::source_bounds_for_test(&gmdl);
    let mut des = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join("sn05/sn05.des")).unwrap(),
    );
    let cob =
        meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn05/sn05.cob")).unwrap()).unwrap();
    let mut mesh = StageMeshCollider::from_gmdl(&gmdl, &des, Some(&cob.face_ids), center);

    use meteor_scene::scene_script_runtime::{SceneItemInstance, SceneScriptRuntime};
    let sources = ["PScriptLib/Def.h", "sn05/sn05.pst"]
        .map(|path| crate::text_codec::decode_text(&std::fs::read(root.join(path)).unwrap()))
        .join("\n");
    let instances = des
        .dummies
        .iter()
        .enumerate()
        .map(|(index, dummy)| SceneItemInstance {
            name: dummy.name.clone(),
            index,
            props: default(),
        })
        .collect();
    let mut scene = SceneScriptRuntime::load(&sources, instances, 250_000).unwrap();
    scene.call("Scene_OnLoad", &[]).unwrap();
    for (index, dummy) in des.dummies.iter_mut().enumerate() {
        if scene.host().instance(index).unwrap().collision_enabled() {
            dummy.collision_attr = Some(1);
        }
    }
    super::super::render_plugin::append_solid_props_for_test(&root, &des, center, &mut mesh);
    let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    mover.set_mesh_collider(&mesh);
    let grounded = |id| {
        let p = Vec3::from_array(graph.world_position(id).unwrap());
        let hit = mover.sample_ground_hit(p.x, p.z, p.y).unwrap();
        Vec3::new(p.x, hit.y, p.z)
    };

    let destinations = [69, 75, 76, 77, 74].map(grounded);

    let start = grounded(69);
    let target = destinations[1];
    let fps = 120;
    let character = crate::plugins::character_plugin::load_character_asset_for_test(
        std::slice::from_ref(&root),
        "p16",
    )
    .unwrap();
    let common =
        meteor_asset::amb::parse_amb(&std::fs::read(root.join("Common/character.amb")).unwrap())
            .unwrap();
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<CharacterContactFrame>()
        .insert_resource(graph)
        .insert_resource(StageCollider { mesh: Some(mesh) })
        .insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p16".into(),
            character,
        )])))
        .insert_resource(crate::plugins::character_plugin::CommonAmb(common))
        .add_systems(
            Update,
            (
                super::super::character_contact_plugin::begin_character_contact_frame,
                drive_npc_ai,
                finish_npc_script_tick,
                move_npc_fighters,
                npc_ballistic_air_root_motion,
            )
                .chain(),
        );
    app.world_mut().spawn((
        LocalFighter,
        Health::default(),
        Transform::from_xyz(1000.0, 0.0, 1000.0),
    ));
    let npc = app
        .world_mut()
        .spawn((
            NpcFighter,
            Fighter {
                character_id: 16,
                player_slot: 1,
                weapon: crate::plugins::character_plugin::weapon_slot_from_item(33)
                    .unwrap()
                    .0,
                weapon_slots: [6, 5],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Health::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: yaw_towards(target - start).unwrap(),
            },
            AiIntent::default(),
            Transform::from_translation(start),
            FighterAir::default(),
            FighterDodge::default(),
            (
                NpcPatrol {
                    route: vec![69, 75, 76, 77, 74],
                    ..default()
                },
                NpcCombatProfile {
                    look: 50.0,
                    ..default()
                },
            ),
            SmState::default(),
            CombatState::default(),
            PoseRuntime::idle(),
            EntryBlend::default(),
            NpcPerformance::default(),
            MissionNpc {
                id: 1,
                name: "范璇".into(),
                team: 2,
                in_scene_roster: true,
                view: 150.0,
                think: 100.0,
                forced_behavior: Some(BehaviorMode::Patrol),
                is_vip: false,
                attack_target_slot: None,
                behavior_args: vec![],
                attack_target_remaining: 0,
            },
        ))
        .id();
    let mut visited = std::collections::BTreeSet::new();
    for tick in 0..60 * fps {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(1.0 / fps as f32));
        app.update();
        let p = app.world().get::<Transform>(npc).unwrap().translation;
        for (id, destination) in [69, 75, 76, 77, 74].into_iter().zip(destinations) {
            if p.distance(destination) < 2.0 {
                visited.insert(id);
            }
        }
        if tick % fps == 0 {
            eprintln!(
                "sn05 t={} p={p:?} visited={visited:?} dest={:?} index={}",
                tick / fps,
                app.world()
                    .get::<NpcPatrol>(npc)
                    .unwrap()
                    .patrol_destination,
                app.world().get::<NpcPatrol>(npc).unwrap().index
            );
        }
        if visited.len() == 5 {
            return;
        }
    }
    panic!("Fanxuan patrol did not reach author destinations: visited={visited:?}, final={:?}, nav={:?}",
            app.world().get::<Transform>(npc).unwrap().translation,
            app.world().get::<NpcPatrol>(npc).unwrap());
}
