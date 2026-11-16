#![cfg(feature = "original-assets")]
use super::*;
use crate::components::fighter::{AiIntent, FighterStatusEffects};
use crate::plugins::combat_plugin::{fighter_speed_factor, CombatResourceData, NpcFighter};

fn original_resources() -> CombatResourceData {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Meteor.res");
    CombatResourceData(
        meteor_asset::res::parse_meteor_res(&std::fs::read(path).unwrap())
            .unwrap()
            .combat_table(),
    )
}

fn sword() -> Fighter {
    Fighter {
        character_id: 0,
        player_slot: 0,
        weapon: 6,
        weapon_slots: [6, 5],
        weapon_slot: 0,
        weapon_slot_variants: [0, 3],
        weapon_variant: 0,
    }
}

#[test]
fn weapon_speed_uses_every_original_variant_and_adds_item_status_deltas() {
    let resources = original_resources();
    let mut fighter = sword();
    let mut slow = FighterStatusEffects::default();
    slow.apply(resources.0.status_effect(10).unwrap());
    for weapon in 1..=12 {
        for (variant, percent) in [100, 90, 90, 120, 110].into_iter().enumerate() {
            if weapon >= 10 && variant == 4 {
                continue;
            }
            fighter.weapon = weapon;
            fighter.weapon_variant = variant as u8;
            let expected = percent as f32 / 100.0;
            for (item, status, delta) in [
                (1.0, None, 0.0),
                (1.5, None, 0.5),
                (1.0, Some(&slow), -0.5),
                (1.5, Some(&slow), 0.0),
            ] {
                let actual = fighter_speed_factor(Some(&resources), &fighter, item, status);
                assert!(
                    (actual - expected - delta).abs() < 1e-6,
                    "weapon={weapon} variant={variant}: {actual}"
                );
            }
        }
    }
    for (weapon, expected) in [(1, 1.1), (2, 1.0)] {
        fighter.weapon = weapon;
        fighter.weapon_variant = 5;
        assert!(
            (fighter_speed_factor(Some(&resources), &fighter, 1.0, None) - expected).abs() < 1e-6
        );
    }
    fighter.weapon = 0;
    assert_eq!(
        fighter_speed_factor(Some(&resources), &fighter, 1.0, None),
        1.0
    );
    assert_eq!(fighter_speed_factor(None, &sword(), 1.5, None), 1.5);
}

fn movement_app(npc: bool, airborne: bool) -> (App, Entity) {
    let mut app = App::new();
    crate::plugins::configure_fighter_frame_sets(&mut app);
    let mut mesh = crate::plugins::stage_collider::StageMeshCollider::default();
    mesh.append_world_triangles(
        &[
            [-600.0, 0.0, -600.0],
            [-600.0, 0.0, 600.0],
            [600.0, 0.0, 600.0],
            [600.0, 0.0, -600.0],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    app.init_resource::<Time>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<CameraBasis>()
        .init_resource::<LockOnController>()
        .init_resource::<crate::plugins::item_plugin::ItemBuffs>()
        .init_resource::<LocalCrouchInput>()
        .init_resource::<LocalGuardInput>()
        .init_resource::<crate::app_state::Keymap>()
        .insert_resource(original_resources())
        .insert_resource(StageCollider { mesh: Some(mesh) });
    if npc {
        app.add_systems(
            Update,
            (
                crate::plugins::ai_plugin::move_npc_fighters,
                crate::plugins::ai_plugin::npc_ballistic_air_root_motion,
            )
                .chain(),
        );
    } else {
        app.add_plugins(MovementPlugin);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
    }
    let mut air = FighterAir::default();
    if airborne {
        air.air.start_fall(0.0, 0.0);
    }
    let entity = app
        .world_mut()
        .spawn((
            sword(),
            Transform::from_xyz(0.0, if airborne { 100.0 } else { 0.0 }, 0.0),
            MotionState {
                movement_mode: if airborne {
                    MovementMode::Airborne
                } else {
                    MovementMode::Run
                },
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            air,
            FighterDodge::default(),
            FighterWall::default(),
            PoseRuntime::idle(),
            EntryBlend::default(),
            SmState::default(),
            CombatState::default(),
        ))
        .id();
    if npc {
        app.world_mut().entity_mut(entity).insert((
            NpcFighter,
            AiIntent {
                move_dir: Vec2::Y,
                locomotion_input: Vec2::Y,
                navigation_jump_target: airborne.then_some(Vec3::new(0.0, 100.0, 100.0)),
                ..default()
            },
        ));
    } else {
        app.world_mut().entity_mut(entity).insert(LocalFighter);
    }
    (app, entity)
}

#[test]
fn weapon_speed_reaches_player_and_npc_ground_and_air_movers() {
    for npc in [false, true] {
        for airborne in [false, true] {
            for fps in [30.0, 60.0, 120.0] {
                let mut distances = Vec::new();
                for variant in 0..4 {
                    let (mut app, entity) = movement_app(npc, airborne);
                    {
                        let mut fighter = app.world_mut().get_mut::<Fighter>(entity).unwrap();
                        fighter.weapon_variant = variant;
                        fighter.weapon_slot_variants[0] = variant;
                    }
                    app.world_mut()
                        .resource_mut::<Time>()
                        .advance_by(std::time::Duration::from_secs_f32(1.0 / fps));
                    let ticks = if airborne { 1 } else { fps as usize };
                    for _ in 0..ticks {
                        app.update();
                    }
                    distances.push(app.world().get::<Transform>(entity).unwrap().translation.z);
                }
                assert!(
                    distances[3] > distances[0] && distances[0] > distances[1],
                    "npc={npc}, air={airborne}, fps={fps}: {distances:?}"
                );
                assert!((distances[1] - distances[2]).abs() < 1e-6, "fire=mountain");
                assert!(distances[1] > 0.0);
            }
        }
    }
}

#[test]
fn weapon_speed_changes_live_with_refinement_and_active_slot() {
    let (mut app, entity) = movement_app(false, false);
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));

    let mut distances = Vec::new();
    for (slot, variants) in [
        (0, [0, 3]),
        (0, [3, 3]),
        (1, [3, 1]),
        (0, [3, 1]),
        (0, [0, 1]),
    ] {
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(entity).unwrap();
            fighter.weapon_slots = [6, 6];
            fighter.weapon_slot = slot;
            fighter.weapon_slot_variants = variants;
            fighter.weapon_variant = variants[usize::from(slot)];
        }
        app.world_mut()
            .get_mut::<Transform>(entity)
            .unwrap()
            .translation = Vec3::ZERO;
        app.world_mut()
            .get_mut::<MotionState>(entity)
            .unwrap()
            .velocity = Vec3::ZERO;

        *app.world_mut().get_mut::<FighterAir>(entity).unwrap() = FighterAir::default();
        for _ in 0..60 {
            app.update();
        }
        distances.push(app.world().get::<Transform>(entity).unwrap().translation.z);
    }
    assert!(
        distances[1] > distances[0] && distances[0] > distances[2],
        "{distances:?}"
    );
    assert!((distances[1] - distances[3]).abs() < 1e-6);
    assert!((distances[0] - distances[4]).abs() < 1e-6);
}
