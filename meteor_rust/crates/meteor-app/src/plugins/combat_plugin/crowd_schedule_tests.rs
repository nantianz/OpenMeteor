#![cfg(feature = "original-assets")]

use super::*;
use crate::plugins::fighter_frame::FighterUpdate;
use bevy::ecs::schedule::{MultiThreadedExecutor, SingleThreadedExecutor};

fn crowd_harness(count: usize, single: bool) -> GameplayHarness {
    use meteor_physics::collision::MeshCollider;
    let mut h = spawn_gameplay_harness_with_order(6, true).expect("P1 resources");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/sn06");
    let gmdl =
        meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn06.gmb")).unwrap()).unwrap();
    let desc = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join("sn06.des")).unwrap(),
    );
    let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn06.cob")).unwrap()).unwrap();
    let collider = StageMeshCollider::from_gmdl(
        &gmdl,
        &desc,
        Some(&cob.face_ids),
        meteor_scene::stage_loader::get_stage_center("sn06", None),
    );
    assert_eq!(collider.triangle_count(), 3528);
    h.app
        .insert_resource(State::new(AppState::Playing))
        .init_resource::<CombatResourceData>()
        .init_resource::<crate::plugins::item_plugin::PropHitGate>()
        .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
        .init_resource::<PendingRangedHits>()
        .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
        .add_message::<crate::plugins::render_plugin::OfflineWeaponDeathDrop>()
        .add_plugins((
            crate::plugins::AiPlugin,
            crate::plugins::CharacterContactPlugin,
        ))
        .add_systems(
            Update,
            tick_target_hit_clocks.before(crate::plugins::fighter_frame::SerialFighterSet),
        )
        .add_systems(
            FighterUpdate,
            (
                attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
                npc_lock_on_tick.in_set(crate::plugins::FighterFrameSet::AttackFacing),
            ),
        );
    crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
    let mut actors = vec![h.fighter];

    for slot in 1..count {
        let entity = h
            .app
            .world_mut()
            .entity_mut(h.fighter)
            .clone_and_spawn_with_opt_out(|builder| {
                builder.deny::<LocalFighter>();
            });
        h.app.world_mut().entity_mut(entity).insert((
            NpcFighter,
            crate::plugins::ai_plugin::NpcPatrol::default(),
            NpcPerformance::default(),
            NpcCombatProfile::web_debug(),
        ));
        h.app
            .world_mut()
            .get_mut::<Fighter>(entity)
            .unwrap()
            .player_slot = slot as u8;
        actors.push(entity);
    }
    for (slot, entity) in actors.into_iter().enumerate() {
        let x = -29.0 + (slot % 4) as f32 * 1.8;
        let z = 6.0 + (slot / 4) as f32 * 1.8;
        let ground = collider
            .sweep_foot_segment(x, 40.0, z, -40.0)
            .expect("sn06 floor");
        h.app
            .world_mut()
            .get_mut::<Transform>(entity)
            .unwrap()
            .translation = Vec3::new(x, ground.y, z);
        h.app.world_mut().get_mut::<Health>(entity).unwrap().current = 100_000.0;
        h.app.world_mut().get_mut::<Health>(entity).unwrap().max = 100_000.0;
        crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), entity);
    }
    h.app.insert_resource(StageCollider {
        mesh: Some(collider),
    });
    h.app.edit_schedule(FighterUpdate, |schedule| {
        if single {
            schedule.set_executor(SingleThreadedExecutor::new());
        } else {
            schedule.set_executor(MultiThreadedExecutor::new());
        }
    });
    h
}

fn frame_state(h: &mut GameplayHarness) -> Vec<(u8, u16, [u32; 6], FighterState)> {
    let world = h.app.world_mut();
    let mut rows: Vec<_> = world
        .query::<(
            &Fighter,
            &Transform,
            &PoseRuntime,
            &Health,
            &MotionState,
            &SmState,
        )>()
        .iter(world)
        .map(|(fighter, transform, pose, health, motion, sm)| {
            (
                fighter.player_slot,
                pose.pose_id,
                [
                    transform.translation.x.to_bits(),
                    transform.translation.y.to_bits(),
                    transform.translation.z.to_bits(),
                    pose.frame.to_bits(),
                    health.current.to_bits(),
                    motion.facing.to_bits(),
                ],
                sm.current,
            )
        })
        .collect();
    rows.sort_by_key(|row| row.0);
    rows
}

#[test]
fn sn06_twelve_fighter_executors_preserve_combat_frames() {
    let mut multi = crowd_harness(12, false);
    let mut single = crowd_harness(12, true);
    let mut saw_attack = false;
    let mut saw_damage = false;
    for tick in 0..180 {
        for h in [&mut multi, &mut single] {
            if tick % 45 == 0 {
                h.press_mouse(MouseButton::Left);
            }
            if tick % 45 == 1 {
                h.release_mouse(MouseButton::Left);
            }
            h.step_dt(1.0 / 60.0);
        }
        let expected = frame_state(&mut multi);
        let actual = frame_state(&mut single);
        saw_attack |= actual.iter().any(|row| is_attacking_state(row.3));
        saw_damage |= actual
            .iter()
            .any(|row| f32::from_bits(row.2[4]) < 100_000.0);
        assert_eq!(
            actual, expected,
            "executor must preserve gameplay at tick {tick}"
        );
    }
    assert!(
        saw_attack && saw_damage,
        "workload must reach real attack windows and damage"
    );
}

#[test]
#[ignore = "headless gameplay CPU benchmark; run explicitly with --ignored --nocapture"]
fn benchmark_sn06_crowd_fighter_schedule() {
    for count in [12, 22] {
        for single in [false, true] {
            let mut h = crowd_harness(count, single);
            for _ in 0..30 {
                h.step_dt(1.0 / 60.0);
            }
            let start = std::time::Instant::now();
            for _ in 0..180 {
                h.step_dt(1.0 / 60.0);
            }
            eprintln!(
                "sn06 fighters={count} single={single} cpu_ms_per_frame={:.3}",
                start.elapsed().as_secs_f64() * 1000.0 / 180.0
            );
        }
    }
}
