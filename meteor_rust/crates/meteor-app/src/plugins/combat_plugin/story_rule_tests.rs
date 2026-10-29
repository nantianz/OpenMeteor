#![cfg(feature = "original-assets")]
use super::*;
use crate::plugins::stage_plugin::{MissionNpc, StageMissionRuntime};
use bevy::ecs::system::RunSystemOnce;
use meteor_scene::mission_runtime::{parse_mission_definition, MissionScriptRuntime};
use meteor_scene::pst_runtime::{PstHost, PstValue};

fn actor(app: &mut App, id: i64, team: u8, dead: bool) -> Entity {
    let mut entity = app.world_mut().spawn((
        Fighter {
            player_slot: if id == 0 { 0 } else { 1 },
            character_id: 0,
            weapon: 5,
            weapon_slots: [5, 0],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        },
        DeathRespawn {
            active: dead,
            ..default()
        },
    ));
    if id != 0 {
        entity.insert(MissionNpc {
            id,
            name: format!("actor{id}"),
            team,
            in_scene_roster: true,
            view: 400.0,
            think: 50.0,
            forced_behavior: None,
            is_vip: false,
            attack_target_slot: None,
            behavior_args: vec![],
            attack_target_remaining: 0,
        });
    }
    entity.id()
}

#[test]
fn story_rule_dispatch_does_not_apply_deathmatch_to_objectives() {
    for rule in [1, 2, 3, 5, 10] {
        let source = format!("int Rule={rule};");
        let definition = parse_mission_definition(&source);
        let mut app = settlement_test_app();
        app.insert_resource(StageMissionRuntime::from_script(MissionScriptRuntime::new(
            &source,
            definition.values,
        )));
        actor(&mut app, 0, 1, false);
        actor(&mut app, 1, 2, true);
        app.world_mut().run_system_once(evaluate_match_end).unwrap();
        assert_eq!(
            app.world().resource::<MatchEnd>().result,
            if rule >= 5 { 4 } else { 0 },
            "Rule {rule}"
        );
    }
}

#[test]
fn story_rule4_only_selected_vip_death_or_removal_ends_round() {
    for (dead, removed, expected) in [(3, false, 0), (2, false, 4), (2, true, 4), (0, false, 2)] {
        let source = "int Rule=4;";
        let mut script = MissionScriptRuntime::new(source, parse_mission_definition(source).values);
        script.host_mut().register_vip_candidate(2, 2);
        let mut app = settlement_test_app();
        app.insert_resource(StageMissionRuntime::from_script(script));
        actor(&mut app, 0, 1, dead == 0);
        if !removed {
            actor(&mut app, 2, 2, dead == 2);
        }
        actor(&mut app, 3, 2, dead == 3);
        app.world_mut().run_system_once(evaluate_match_end).unwrap();
        assert_eq!(app.world().resource::<MatchEnd>().result, expected);
    }
}

#[test]
fn story_sn09_final_settlement_obeys_real_script_death_and_survival() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
    let source = crate::text_codec::decode_text(&std::fs::read(root.join("sn09_.pst")).unwrap());
    for dead in [false, true] {
        let mut script =
            MissionScriptRuntime::new(&source, parse_mission_definition(&source).values);
        for index in 1..=2 {
            let name = format!("npc09_{index:02}");
            let text = crate::text_codec::decode_text(
                &std::fs::read(root.join(format!("{name}.pst"))).unwrap(),
            );
            script.register_npc_script(&name, &text).unwrap();
        }
        script.host_mut().sync_fighter(0, 3000.0, 3000.0, 0.0);
        script.start().unwrap();
        script.update(0.1).unwrap();
        script.update(20.0).unwrap();
        let leng = script.host().char_id("冷燕").unwrap();
        let mut app = settlement_test_app();
        actor(&mut app, 0, 1, false);
        actor(&mut app, leng, 2, dead);
        if dead {
            script.host_mut().sync_fighter(leng, 0.0, 3000.0, 2.0);
            script.update(0.1).unwrap();
            app.insert_resource(StageMissionRuntime::from_script(script));
            app.world_mut().run_system_once(evaluate_match_end).unwrap();
            assert!(!app.world().resource::<MatchEnd>().settled);
            app.world_mut()
                .resource_mut::<StageMissionRuntime>()
                .script_mut()
                .unwrap()
                .update(3.1)
                .unwrap();
        } else {
            for _ in 0..1850 {
                script.update(0.1).unwrap();
            }
            app.insert_resource(StageMissionRuntime::from_script(script));
        }
        app.world_mut().run_system_once(evaluate_match_end).unwrap();
        assert_eq!(
            app.world().resource::<MatchEnd>().result,
            if dead { 2 } else { 4 }
        );

        *app.world_mut().resource_mut::<MatchEnd>() = MatchEnd {
            elapsed: 10000.0,
            ..default()
        };
        app.world_mut()
            .resource_mut::<StageMissionRuntime>()
            .script_mut()
            .unwrap()
            .host_mut()
            .builtin("GameOver", &[PstValue::Num(-1.0)]);
        app.world_mut().run_system_once(evaluate_match_end).unwrap();
        assert_eq!(app.world().resource::<MatchEnd>().result, 1);
    }
}
