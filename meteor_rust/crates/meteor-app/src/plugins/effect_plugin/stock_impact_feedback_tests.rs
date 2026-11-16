#[cfg(not(target_arch = "wasm32"))]
#[test]
fn production_stock_impact_feedback_mutes_prediction_and_plays_authority_audio() {
    use crate::plugins::impact_fx_plugin::{CombatImpactEvent, CombatImpactKind};
    use crate::plugins::network_plugin::{NetState, NetStatus, RemoteFighter};
    use crate::plugins::stage_plugin::LocalFighter;

    for address in ["udp-host://0.0.0.0:5230", "udp://127.0.0.1:5230"] {
        for guarded in [false, true] {
            for delayed in [false, true] {
                let mut app = feedback_test_app();
                app.insert_resource(NetState {
                    address: address.into(),
                    status: NetStatus::InRoom,
                    round_active: true,
                    ..Default::default()
                });
                let attacker = app
                    .world_mut()
                    .spawn((
                        LocalFighter,
                        feedback_fighter(6),
                        PoseRuntime::idle(),
                        Transform::default(),
                    ))
                    .id();
                let defender = app
                    .world_mut()
                    .spawn((
                        RemoteFighter {
                            player_id: "stock-2".into(),
                        },
                        feedback_fighter(6),
                        PoseRuntime::idle(),
                        Transform::default(),
                    ))
                    .id();
                app.update();
                let mut event = CombatImpactEvent {
                    kind: CombatImpactKind::Hit,
                    attacker,
                    defender,
                    reaction_pose: Some(93),
                    effect_strategy: None,
                    contact: Vec3::ZERO,
                    direction: Vec3::Z,
                };
                app.world_mut().write_message(event);
                if delayed {
                    app.update();
                    assert_eq!(
                        app.world_mut()
                            .query::<&bevy::audio::AudioPlayer>()
                            .iter(app.world())
                            .count(),
                        0,
                        "no predicted HIT sound before the reply"
                    );
                    assert_eq!(
                        app.world_mut()
                            .query::<&PonEfInstance>()
                            .iter(app.world())
                            .count(),
                        0,
                        "no predicted BL effect before the reply"
                    );
                }

                event.attacker = defender;
                event.effect_strategy = Some(4);
                event.kind = if guarded {
                    CombatImpactKind::Guard
                } else {
                    CombatImpactKind::Hit
                };
                event.reaction_pose = Some(if guarded { 56 } else { 93 });
                app.world_mut().write_message(event);
                app.update();
                assert_eq!(
                    app.world_mut()
                        .query::<&bevy::audio::AudioPlayer>()
                        .iter(app.world())
                        .count(),
                    1,
                    "{address} guard={guarded} delayed={delayed}: only authority audio may play"
                );
                app.update();
                assert_eq!(
                    app.world_mut()
                        .query::<&bevy::audio::AudioPlayer>()
                        .iter(app.world())
                        .count(),
                    1,
                    "no duplicate playback on the next frame"
                );
            }
        }
    }
}
