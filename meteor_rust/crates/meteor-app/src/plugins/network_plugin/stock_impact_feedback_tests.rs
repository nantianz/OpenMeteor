#[cfg(not(target_arch = "wasm32"))]
#[test]
fn production_stock_impact_feedback_uses_authoritative_guard_once() {
    use crate::plugins::impact_fx_plugin::{CombatImpactEvent, CombatImpactKind};
    for (guarded, local) in [(false, false), (true, false), (true, true)] {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p0")
                .expect("original p0 reactions");
        let mut net = NetState {
            status: NetStatus::InRoom,
            address: "udp://127.0.0.1:5230".into(),
            player_id: "stock-1".into(),
            round_active: true,
            ..NetState::default()
        };
        handle_stock_game_event(
            &mut net,
            &mut LaunchRequest::default(),
            &mut MatchEnd::default(),
            &mut NetTx::default(),
            &mut NextState::<AppState>::default(),
            AppState::Playing,
            StockGameEvent::HitReaction {
                guarded,
                event: meteor_network::stock_protocol::StockHitReactionEvent {
                    pose_id: if guarded { 98 << 16 } else { 93 },
                    move_hundred: 100,
                    value_hundred: 30,
                    context: if guarded { 1 } else { 4 },
                    target_actor_id: if local { 1 } else { 2 },
                },
            },
        );
        let mut app = App::new();
        app.insert_resource(net)
            .insert_resource(crate::plugins::character_plugin::CharacterAssetsSet(
                std::collections::HashMap::from([("p0".into(), character)]),
            ))
            .add_message::<CombatImpactEvent>()
            .add_systems(Update, (net_handle_hit, net_handle_remote_hits).chain());
        let mut target = app.world_mut().spawn((
            Fighter {
                character_id: 0,
                player_slot: 1,
                weapon: 5,
                weapon_slots: [5, 6],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Transform::from_xyz(0.0, 0.0, 3.0),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            Health::default(),
            PoseRuntime::idle(),
            EntryBlend::default(),
            CombatState::default(),
            DeathRespawn::default(),
            FighterAir::default(),
            RemoteHitMotionWindow::default(),
            RemoteAuthorityGeneration::default(),
        ));
        if local {
            target.insert((
                LocalFighter,
                Qi::default(),
                SmState::default(),
                FighterHitReaction::default(),
                FighterAttackMotion::default(),
            ));
        } else {
            target.insert(RemoteFighter {
                player_id: "stock-2".into(),
            });
        }
        let target = target.id();
        let mut cursor = bevy::ecs::message::MessageCursor::<CombatImpactEvent>::default();
        app.update();
        let messages = app
            .world()
            .resource::<bevy::ecs::message::Messages<CombatImpactEvent>>();
        let events: Vec<_> = cursor.read(messages).collect();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].kind,
            if guarded {
                CombatImpactKind::Guard
            } else {
                CombatImpactKind::Hit
            }
        );
        assert_eq!(events[0].defender, target);
        assert_eq!(events[0].effect_strategy, Some(if guarded { 1 } else { 4 }));
        assert_eq!(events[0].reaction_pose, local.then_some(98));
        let reaction = app
            .world()
            .get::<FighterHitReaction>(target)
            .unwrap()
            .current
            .as_ref()
            .expect("authoritative reaction");
        if local {
            assert!(matches!(reaction.pose_id, Some(56..=59)));
        }
        assert_eq!(
            reaction.kind,
            if guarded {
                meteor_combat::hit_reaction::HitReactKind::Guard
            } else {
                meteor_combat::hit_reaction::HitReactKind::BeHit
            }
        );
        app.update();
        assert_eq!(
            cursor
                .read(
                    app.world()
                        .resource::<bevy::ecs::message::Messages<CombatImpactEvent>>()
                )
                .count(),
            0,
            "a consumed authority event must not emit feedback again"
        );
    }
}
