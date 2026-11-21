use super::*;
use bevy::ecs::{message::MessageCursor, system::RunSystemOnce};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

mod substeps {
    include!("duel_lab_substeps.rs");
}

fn duel_harness(distance: f32, weapon: u8) -> (GameplayHarness, Entity) {
    let (mut h, npc, _) = spawn_local_stock_hit_harness_with_loadouts(weapon, weapon, true);
    h.app.insert_resource(NetState::default());
    h.app.insert_resource(State::new(AppState::Playing));
    h.app.add_plugins(crate::plugins::CharacterContactPlugin);
    configure_lock_updates(&mut h.app);
    h.app
        .world_mut()
        .run_system_once(load_combat_resource_data)
        .unwrap();
    assert!(
        h.app
            .world()
            .resource::<CombatResourceData>()
            .0
            .weapon_stats_count()
            > 0
    );
    h.app
        .world_mut()
        .entity_mut(h.fighter)
        .insert(HeadTrackState::default());

    crate::plugins::camera_plugin::install_camera_input_for_test(&mut h.app);
    h.app.world_mut().spawn((
        crate::plugins::camera_plugin::GameplayCamera,
        crate::plugins::camera_plugin::ThirdPersonState::default(),
    ));
    h.app.world_mut().entity_mut(npc).insert((
        NpcFighter,
        crate::plugins::ai_plugin::NpcPatrol::default(),
        NpcPerformance::default(),
        NpcCombatProfile {
            attack1: 100.0,
            ..default()
        },
        AiIntent::default(),
        GunAimState::default(),
        RangedFireGate::default(),
        FighterHitReaction::default(),
        WeaponForm::default(),
        FighterWall::default(),
    ));
    {
        let world = h.app.world_mut();
        world.get_mut::<Fighter>(npc).unwrap().player_slot = 1;
        world.get_mut::<Transform>(npc).unwrap().translation.z = distance;
        for entity in [h.fighter, npc] {
            world
                .get_mut::<crate::components::fighter::TargetHitClock>(entity)
                .unwrap()
                .spawn_elapsed = 3.0;
        }
    }
    crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
    let schedule = super::super::super::fighter_frame::schedule(&h.app);
    h.app.add_systems(
        Update,
        tick_target_hit_clocks.in_set(crate::plugins::FighterFrameSet::PoseDecision),
    );
    h.app.add_systems(
        schedule,
        attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
    );
    h.app.add_systems(
        schedule,
        hit_reaction_root_motion
            .in_set(crate::plugins::FighterFrameSet::RootMotion)
            .after(crate::plugins::movement_plugin::attack_root_motion)
            .after(crate::plugins::movement_plugin::dodge_root_motion),
    );
    (h, npc)
}

fn attack_recovery(world: &World, entity: Entity, state: FighterState, pose: &PoseRuntime) -> bool {
    if !matches!(
        state,
        FighterState::Attack1 | FighterState::Attack2 | FighterState::Attack3
    ) || !pose.plays_to_end()
    {
        return false;
    }
    let blend = world.get::<EntryBlend>(entity).unwrap();
    if blend.is_active_for(pose.pose_id) {
        return false;
    }
    let (_, frame) = blend.frame_record_for(pose);
    let fighter = world.get::<Fighter>(entity).unwrap();
    let assets = world.resource::<CharacterAssetsSet>();
    assets
        .0
        .get(&format!("p{}", fighter.character_id))
        .and_then(|assets| assets.poses.get(&pose.pose_id))
        .and_then(|pose| pose.attacks.iter().map(|window| window.end).max())
        .is_some_and(|last_end| frame > last_end)
}

fn body(world: &World, entity: Entity) -> Value {
    let p = world.get::<Transform>(entity).unwrap().translation;
    let motion = world.get::<MotionState>(entity).unwrap();
    let f = attack_forward_from_facing(motion.facing);
    let hp = world.get::<Health>(entity).unwrap();
    let pose = world.get::<PoseRuntime>(entity).unwrap();
    let state = world.get::<SmState>(entity).unwrap().current;
    json!({"position": [p.x, p.y, -p.z], "forward_xz": [f.x, -f.z],
        "pose_id": pose.pose_id,
        "grounded": !world.get::<FighterAir>(entity).unwrap().air.airborne,
        "guard_route": state == FighterState::Guard,
        "state": format!("{state:?}"),
        "attack_recovery": attack_recovery(world, entity, state, pose),
        "weapon_class": world.get::<Fighter>(entity).unwrap().weapon,
        "hp": hp.current, "max_hp": hp.max, "anger": world.get::<Qi>(entity).unwrap().current})
}

#[test]
fn recovery_cue_starts_after_the_inclusive_final_attack_frame() {
    let (h, _) = duel_harness(3.0, DAGGER);
    let world = h.app.world();
    let fighter = world.get::<Fighter>(h.fighter).unwrap();
    let assets = &world.resource::<CharacterAssetsSet>().0[&format!("p{}", fighter.character_id)];
    let (mut pose, last_end) = assets
        .poses
        .iter()
        .find_map(|(&id, authored)| {
            let last_end = authored.attacks.iter().map(|window| window.end).max()?;
            let mut pose = PoseRuntime::idle();
            assets.apply_pose(&mut pose, id, true);
            (pose.plays_to_end() && last_end < pose.end).then_some((pose, last_end))
        })
        .expect("stock Poses include an attack with a recovery tail");
    pose.frame = last_end as f32 + 0.5;
    assert!(!attack_recovery(
        world,
        h.fighter,
        FighterState::Attack1,
        &pose
    ));
    pose.frame = (last_end + 1) as f32;
    assert!(attack_recovery(
        world,
        h.fighter,
        FighterState::Attack1,
        &pose
    ));
    assert!(!attack_recovery(
        world,
        h.fighter,
        FighterState::Name,
        &pose
    ));
}

fn binding(keymap: &crate::app_state::Keymap, name: &str) -> crate::app_state::InputBinding {
    match name {
        "forward" => keymap.move_up,
        "backward" => keymap.move_down,
        "left" => keymap.move_left,
        "right" => keymap.move_right,
        "attack" => keymap.attack,
        "guard" => keymap.guard,
        "jump" => keymap.jump,
        "crouch" => keymap.crouch,
        "fake" => keymap.fake,
        "qi_burst" => keymap.qi_burst,
        "lock_toggle" => keymap.lock_toggle,
        "left_turn" => keymap.left_turn,
        "right_turn" => keymap.right_turn,
        "switch_weapon" => keymap.weapon_switch,
        "drop_weapon" => keymap.drop_weapon,
        "drop_item" => keymap.drop_item,
        "cure" => keymap.cure,
        _ => panic!("unknown lab input {name}"),
    }
}

#[test]
#[ignore = "local JSONL driver: tools/mrc_duel_lab.py"]
fn headless_duel_stdio() {
    use crate::plugins::impact_fx_plugin::{CombatImpactEvent, CombatImpactKind};
    #[derive(serde::Deserialize)]
    struct LabRequest {
        distance: f32,
        windows: u32,
        #[serde(default)]
        weapon: Option<u8>,
    }
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum LabInput {
        Ordered(Vec<(String, bool)>),
        OpeningGuard {
            guard_at_open: bool,
            #[serde(default)]
            edges: Vec<(String, bool)>,
        },
    }
    let mut lines = io::stdin().lock().lines();
    let request: LabRequest = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let LabRequest {
        distance,
        windows,
        weapon,
    } = request;
    let weapon = weapon.unwrap_or(DAGGER);
    assert!(
        (2.0..=12.0).contains(&distance)
            && (1..=600).contains(&windows)
            && (1..=12).contains(&weapon)
    );
    let (mut h, npc) = duel_harness(distance, weapon);
    let keymap = h.app.world().resource::<crate::app_state::Keymap>().clone();
    let mut output = io::stdout().lock();
    let mut impacts = MessageCursor::<CombatImpactEvent>::default();
    let mut hits = [0u32; 2];
    let mut guards = [0u32; 2];
    let mut early_guard_used = false;
    for tick in 0..=windows {
        let world = h.app.world();
        let own = body(world, h.fighter);
        let other = body(world, npc);
        let camera = json!({
            "basis_yaw": world.resource::<crate::plugins::camera_plugin::CameraBasis>().yaw,
            "locked": world.resource::<LockOnController>().lock_entity.is_some(),
        });
        let done = tick == windows
            || own["hp"].as_f64().unwrap() <= 0.0
            || other["hp"].as_f64().unwrap() <= 0.0;
        writeln!(
            output,
            "DUEL {}",
            json!({"tick":tick,"done":done,"self":own,"opponent":other,"hits":hits,"guards":guards,
                "camera":camera})
        )
        .unwrap();
        output.flush().unwrap();
        if done {
            return;
        }
        let line = lines.next().expect("missing input").unwrap();
        let (edges, early_guard) = match serde_json::from_str(&line).unwrap() {
            LabInput::Ordered(edges) => (edges, false),
            LabInput::OpeningGuard {
                guard_at_open,
                edges,
            } => {
                assert!(guard_at_open && tick > 0 && weapon == DAGGER && !early_guard_used);
                let edges = if edges.is_empty() {
                    vec![(String::from("guard"), true)]
                } else {
                    edges
                };
                assert_eq!(
                    edges
                        .iter()
                        .filter(|(name, pressed)| name == "guard" && *pressed)
                        .count(),
                    1
                );
                assert!(edges
                    .iter()
                    .all(|(name, pressed)| name != "guard" || *pressed));
                early_guard_used = true;
                (edges, true)
            }
        };
        assert!(edges.len() <= 8);

        for frame in 0..3 {
            if early_guard && frame == 0 {
                h.press_binding(binding(&keymap, "guard"));
            }
            let first = frame * edges.len() / 3;
            let last = (frame + 1) * edges.len() / 3;
            for (name, pressed) in &edges[first..last] {
                if early_guard && name == "guard" {
                    continue;
                }
                let key = binding(&keymap, name);
                if *pressed {
                    h.press_binding(key);
                } else {
                    h.release_binding(key);
                }
            }
            h.step();
            let world = h.app.world();
            for e in impacts.read(world.resource::<Messages<CombatImpactEvent>>()) {
                let side = if e.attacker == h.fighter {
                    0
                } else if e.attacker == npc {
                    1
                } else {
                    continue;
                };
                match e.kind {
                    CombatImpactKind::Hit | CombatImpactKind::Kill => hits[side] += 1,
                    CombatImpactKind::Guard => guards[side] += 1,
                    _ => (),
                }
            }
            if [h.fighter, npc]
                .into_iter()
                .any(|id| world.get::<Health>(id).unwrap().current <= 0.0)
            {
                break;
            }
        }
    }
}
