#![cfg(feature = "original-assets")]
use super::*;
use crate::components::fighter::{FighterStatusEffects, WornEffect};
use crate::plugins::combat_plugin::{CombatResourceData, NpcFighter};
use crate::plugins::stage_plugin::{LocalFighter, MissionNpc, StageScriptRuntime};
use bevy::ecs::system::RunSystemOnce;

fn setup() -> (App, Entity, Entity) {
    let mut app = App::new();
    let table = meteor_asset::res::parse_meteor_res(include_bytes!(
        "../../../../../../origin_res/Meteor.res"
    ))
    .unwrap()
    .combat_table();
    app.init_resource::<Time>()
        .init_resource::<crate::plugins::FrameClock>()
        .init_resource::<ItemBuffs>()
        .init_resource::<StageScriptRuntime>()
        .insert_resource(CombatResourceData(table));
    let player = fighter(&mut app, 0);
    app.world_mut()
        .entity_mut(player)
        .insert((LocalFighter, Transform::from_xyz(100.0, 0.0, 0.0)));
    let npc = fighter(&mut app, 1);
    app.world_mut().entity_mut(npc).insert(NpcFighter);
    (app, player, npc)
}

fn fighter(app: &mut App, slot: u8) -> Entity {
    app.world_mut()
        .spawn((
            Fighter {
                character_id: 0,
                player_slot: slot,
                weapon: 6,
                weapon_slots: [6, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Health {
                current: 100.0,
                max: 1000.0,
            },
            Qi::default(),
            Transform::default(),
            Visibility::Visible,
            DeathRespawn::default(),
        ))
        .id()
}

fn prop(app: &mut App, name: &str, model: &str) -> Entity {
    app.world_mut()
        .spawn((
            StageProp {
                name: name.into(),
                model: model.into(),
                center_offset: Vec3::ZERO,
                half_extents: Vec3::ONE,
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: true,
                collision_enabled: false,
                breakable: false,
                broken: false,
            },
            Transform::default(),
            Visibility::Visible,
        ))
        .id()
}

fn pickup(app: &mut App) {
    app.world_mut()
        .resource_mut::<crate::plugins::FrameClock>()
        .sim_dt = 0.0;
    app.world_mut().run_system_once(item_pickup).unwrap();
    app.world_mut()
        .run_system_once(crate::plugins::status_plugin::tick_status_effects)
        .unwrap();
}

fn tick(app: &mut App, seconds: f32) {
    app.world_mut()
        .resource_mut::<crate::plugins::FrameClock>()
        .sim_dt = seconds;
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(seconds));
    app.world_mut()
        .run_system_once(tick_npc_item_visuals)
        .unwrap();
    app.world_mut()
        .run_system_once(crate::plugins::status_plugin::tick_status_effects)
        .unwrap();
}

#[test]
fn npc_pickup_heals_without_a_local_player_and_consumes_all_material_batches_once() {
    let (mut app, player, npc) = setup();
    app.world_mut().despawn(player);
    let a = prop(&mut app, "D_it01", "item01");
    let b = prop(&mut app, "D_it01", "item01");
    pickup(&mut app);
    pickup(&mut app);
    assert_eq!(app.world().get::<Health>(npc).unwrap().current, 400.0);
    for entity in [a, b] {
        let item = app.world().get::<StageProp>(entity).unwrap();
        assert_eq!(item.hit_count, 1);
        assert!(!item.interactive);
        assert_eq!(
            *app.world().get::<Visibility>(entity).unwrap(),
            Visibility::Hidden
        );
    }
}

#[test]
fn npc_pickup_admission_rejects_dead_hidden_remote_and_unavailable_items() {
    for case in 0..10 {
        let (mut app, _, npc) = setup();
        let item = prop(&mut app, "D_it01", "item01");
        match case {
            0 => app.world_mut().get_mut::<Health>(npc).unwrap().current = 0.0,
            1 => app.world_mut().get_mut::<DeathRespawn>(npc).unwrap().active = true,
            2 => {
                let mut state = SmState::default();
                state.current = FighterState::Dead;
                app.world_mut().entity_mut(npc).insert(state);
            }
            3 => *app.world_mut().get_mut::<Visibility>(npc).unwrap() = Visibility::Hidden,
            4 => {
                app.world_mut().entity_mut(npc).insert(
                    crate::plugins::network_plugin::RemoteFighter {
                        player_id: "remote".into(),
                    },
                );
            }
            5 => {
                app.world_mut()
                    .get_mut::<StageProp>(item)
                    .unwrap()
                    .interactive = false
            }
            6 => *app.world_mut().get_mut::<Visibility>(item).unwrap() = Visibility::Hidden,
            7 => {
                app.world_mut()
                    .get_mut::<StageProp>(item)
                    .unwrap()
                    .respawn_timer = 10.0
            }
            8 => {
                app.world_mut()
                    .get_mut::<Transform>(npc)
                    .unwrap()
                    .translation = Vec3::X * 10.0
            }
            9 => app.world_mut().get_mut::<StageProp>(item).unwrap().broken = true,
            _ => unreachable!(),
        }
        pickup(&mut app);
        assert_eq!(
            app.world().get::<StageProp>(item).unwrap().hit_count,
            0,
            "case {case}"
        );
    }
}

#[test]
fn npc_pickup_contact_has_no_idle_health_or_getitem_probability_requirement() {
    for state in [
        FighterState::Name,
        FighterState::Attack1,
        FighterState::Guard,
        FighterState::Jump,
    ] {
        let (mut app, _, npc) = setup();
        let mut sm = SmState::default();
        sm.current = state;
        sm.locked = true;
        app.world_mut().entity_mut(npc).insert((
            sm,
            crate::plugins::stage_plugin::NpcCombatProfile::default(),
        ));
        app.world_mut().get_mut::<Health>(npc).unwrap().current = 1000.0;
        let item = prop(&mut app, "D_it01", "item01");
        pickup(&mut app);
        assert_eq!(
            app.world().get::<StageProp>(item).unwrap().hit_count,
            1,
            "{state:?}"
        );
        assert_eq!(app.world().get::<Health>(npc).unwrap().current, 1000.0);
        assert_eq!(app.world().get::<SmState>(npc).unwrap().current, state);
    }
}

#[test]
fn npc_pickup_weapon_slots_and_refinement_use_the_collectors_inventory() {
    for (slots, variants, model, accepted) in [
        ([6, 0], [0, 0], "w1_0", true),
        ([6, 5], [0, 0], "w2_0", false),
        ([6, 0], [0, 0], "w0_0", false),
        ([6, 0], [0, 0], "w0_1", true),
    ] {
        let (mut app, player, npc) = setup();
        {
            let mut fighter = app.world_mut().get_mut::<Fighter>(npc).unwrap();
            fighter.weapon_slots = slots;
            fighter.weapon_slot_variants = variants;
        }
        let item = prop(&mut app, "D_wp01", model);
        pickup(&mut app);
        assert_eq!(
            app.world().get::<StageProp>(item).unwrap().hit_count,
            u32::from(accepted),
            "{model} {slots:?}"
        );
        assert_eq!(
            app.world().get::<Fighter>(player).unwrap().weapon_slots,
            [6, 0]
        );
    }
    let (mut app, player, npc) = setup();
    prop(&mut app, "D_gem", "item0901");
    pickup(&mut app);
    assert_eq!(app.world().get::<Fighter>(npc).unwrap().weapon_variant, 1);
    assert_eq!(
        app.world().get::<Fighter>(player).unwrap().weapon_variant,
        0
    );
    let drop = prop(&mut app, "D_drop", "w1_0");
    app.world_mut()
        .entity_mut(drop)
        .insert(crate::plugins::render_plugin::DynamicWeaponDrop {
            reference: "D_drop".into(),
        });
    pickup(&mut app);
    assert!(app.world().get_entity(drop).is_err());
}

#[test]
fn npc_pickup_unarmed_equips_first_weapon_and_unarmed_refinement_does_not_invent_one() {
    let (mut app, _, npc) = setup();
    {
        let mut fighter = app.world_mut().get_mut::<Fighter>(npc).unwrap();
        fighter.weapon = 0;
        fighter.weapon_slots = [0, 0];
    }
    let gem = prop(&mut app, "D_gem", "item0901");
    pickup(&mut app);
    assert_eq!(app.world().get::<StageProp>(gem).unwrap().hit_count, 1);
    assert_eq!(app.world().get::<Fighter>(npc).unwrap().weapon_variant, 0);
    prop(&mut app, "D_sword", "w0_0");
    pickup(&mut app);
    let fighter = app.world().get::<Fighter>(npc).unwrap();
    assert_eq!(fighter.weapon, 6);
    assert_eq!(fighter.weapon_slots, [6, 0]);
}

#[test]
fn npc_pickup_lethal_poison_enters_death_state_in_the_same_status_pass() {
    let (mut app, _, npc) = setup();
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 1.0;
    prop(&mut app, "D_poison", "item0103");
    pickup(&mut app);
    assert_eq!(app.world().get::<Health>(npc).unwrap().current, 0.0);
    assert!(app.world().get::<DeathRespawn>(npc).unwrap().active);
    assert!(app
        .world()
        .get::<FighterStatusEffects>(npc)
        .unwrap()
        .effects
        .active
        .is_empty());
}

#[test]
fn npc_pickup_buffs_are_independent_refresh_without_stacking_and_expire() {
    let (mut app, player, npc) = setup();
    for (name, model) in [
        ("D_attack", "item04"),
        ("D_defense", "item03"),
        ("D_speed", "item05"),
        ("D_max", "item02"),
        ("D_hide", "item07"),
    ] {
        prop(&mut app, name, model);
    }
    pickup(&mut app);
    tick(&mut app, 0.1);
    let status = app.world().get::<FighterStatusEffects>(npc).unwrap();
    assert_eq!(status.effects.attack_bonus(), 25);
    assert_eq!(status.effects.defense_bonus(), 20);
    assert_eq!(status.effects.speed_factor(1.0), 1.5);
    assert_eq!(app.world().get::<Health>(npc).unwrap().max, 1500.0);
    assert!(
        app.world()
            .get::<crate::components::fighter::Concealment>(npc)
            .unwrap()
            .remaining
            > 0.0
    );
    assert_eq!(
        app.world()
            .get::<FighterStatusEffects>(player)
            .unwrap()
            .effects
            .attack_bonus(),
        0
    );
    assert_eq!(app.world().resource::<ItemBuffs>().attack_bonus, 0);
    prop(&mut app, "D_max_again", "item02");
    pickup(&mut app);
    assert_eq!(app.world().get::<Health>(npc).unwrap().max, 1500.0);
    tick(&mut app, 1000.0);
    let status = app.world().get::<FighterStatusEffects>(npc).unwrap();
    assert_eq!(status.effects.attack_bonus(), 0);
    assert_eq!(status.effects.defense_bonus(), 0);
    assert_eq!(status.effects.speed_factor(1.0), 1.0);
    assert_eq!(app.world().get::<Health>(npc).unwrap().max, 1000.0);
    assert!(app
        .world()
        .get::<WornEffect>(npc)
        .unwrap()
        .effects
        .is_empty());
}

#[test]
fn npc_pickup_periodic_anger_poison_and_death_cleanup_stay_actor_local() {
    let (mut app, player, npc) = setup();
    prop(&mut app, "D_hot", "hot");
    prop(&mut app, "D_poison", "item0103");
    pickup(&mut app);
    let before = app.world().get::<Health>(npc).unwrap().current;
    let anger = app.world().get::<Qi>(npc).unwrap().current;
    tick(&mut app, 5.0);
    assert!(app.world().get::<Health>(npc).unwrap().current < before);
    assert!(app.world().get::<Qi>(npc).unwrap().current > anger);
    assert_eq!(app.world().get::<Health>(player).unwrap().current, 100.0);
    app.world_mut().get_mut::<Health>(npc).unwrap().current = 0.0;
    tick(&mut app, 0.1);
    assert!(app
        .world()
        .get::<FighterStatusEffects>(npc)
        .unwrap()
        .effects
        .active
        .is_empty());
    assert!(app
        .world()
        .get::<WornEffect>(npc)
        .unwrap()
        .effects
        .is_empty());
}

#[test]
fn npc_pickup_callback_uses_mission_id_and_competing_collectors_do_not_duplicate() {
    use meteor_scene::scene_script_runtime::{SceneItemInstance, SceneScriptRuntime};
    let (mut app, _, npc) = setup();
    let other = fighter(&mut app, 2);
    app.world_mut().entity_mut(other).insert(NpcFighter);
    app.world_mut().entity_mut(npc).insert(MissionNpc {
        id: 37,
        name: "collector".into(),
        team: 2,
        in_scene_roster: true,
        view: 100.0,
        think: 50.0,
        forced_behavior: None,
        is_vip: false,
        attack_target_slot: None,
        behavior_args: Vec::new(),
        attack_target_remaining: 0,
    });
    let script = SceneScriptRuntime::load(
        "int picked = -1; int D_it01_OnPickUp(int item, int actor) { picked = actor; return 1; } int Picked() { return picked; }",
        vec![SceneItemInstance { name: "D_it01".into(), ..Default::default() }], 10000,
    ).unwrap();
    app.insert_resource(StageScriptRuntime::from_script(script));
    prop(&mut app, "D_it01", "item01");
    pickup(&mut app);
    assert_eq!(
        app.world_mut()
            .resource_mut::<StageScriptRuntime>()
            .script_mut()
            .unwrap()
            .call("Picked", &[])
            .unwrap()
            .as_f64(),
        37.0
    );
    assert_eq!(app.world().get::<Health>(npc).unwrap().current, 400.0);
    assert_eq!(app.world().get::<Health>(other).unwrap().current, 100.0);
}
