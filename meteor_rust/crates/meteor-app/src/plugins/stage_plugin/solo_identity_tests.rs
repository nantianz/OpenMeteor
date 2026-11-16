use super::*;
use meteor_scene::mission_runtime::{parse_mission_definition, parse_npc_definition};

pub(crate) fn sn13() -> (StageMissionRuntime, Vec<MissionNpc>) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
    let read =
        |name: &str| crate::text_codec::decode_text(&std::fs::read(root.join(name)).unwrap());
    let source = read("sn13_.pst");
    let mut runtime = MissionScriptRuntime::new(&source, parse_mission_definition(&source).values);
    runtime.host_mut().sync_fighter(0, 2000.0, 2000.0, 0.0);
    for index in 1..=8 {
        let name = format!("npc13_{index:02}");
        runtime
            .register_npc_script(&name, &read(&format!("{name}.pst")))
            .unwrap();
    }
    runtime.start().unwrap();
    let actors = (1..=8)
        .map(|id| {
            let name = format!("npc13_{id:02}");
            let definition = parse_npc_definition(&read(&format!("{name}.pst")), &name);
            MissionNpc {
                id,
                name: definition.name,
                team: definition.team as u8,
                in_scene_roster: true,
                view: definition.view as f32,
                think: definition.think as f32,
                forced_behavior: None,
                is_vip: false,
                attack_target_slot: None,
                behavior_args: vec![],
                attack_target_remaining: 0,
            }
        })
        .collect();
    (StageMissionRuntime::from_script(runtime), actors)
}

#[cfg(feature = "original-assets")]
#[test]
fn real_sn13_player_is_captain_and_lengyan_is_teammate() {
    let (level, actors) = sn13();
    assert_eq!(level.vip_team_for_actor(true, None), Some(1));
    assert_eq!(
        level.script().unwrap().host().char_id_for_team("vip", 1),
        Some(0)
    );
    assert_eq!(
        level.script().unwrap().host().char_id_for_team("vip", 2),
        Some(2)
    );
    let net = crate::plugins::network_plugin::NetState::default();
    assert_eq!(
        crate::plugins::combat_plugin::fighter_combat_team(&net, true, None, None),
        1
    );
    let mut app = App::new();
    app.insert_resource(crate::app_state::LaunchRequest {
        mode: "single".into(),
        stock_camp: 2,
        ..default()
    })
    .insert_resource(level)
    .add_systems(Update, assign_assassin_vip);
    for npc in actors {
        app.world_mut().spawn(npc);
    }
    app.update();
    for npc in app.world_mut().query::<&MissionNpc>().iter(app.world()) {
        assert_eq!(npc.is_vip, npc.id == 2, "{}", npc.name);
        assert_eq!(npc.team, if npc.id == 1 { 1 } else { 2 });
        assert_eq!(
            app.world()
                .resource::<StageMissionRuntime>()
                .vip_team_for_actor(false, Some(npc)),
            (npc.id == 2).then_some(2),
        );
    }
}
