#[cfg(not(target_arch = "wasm32"))]
#[test]
fn production_stock_impact_feedback_preserves_contact_despite_visible_guard() {
    use crate::plugins::impact_fx_plugin::CombatImpactEvent;
    use bevy::ecs::system::RunSystemOnce;

    for address in [
        "udp-host://0.0.0.0:5230",
        "udp://127.0.0.1:5230",
        "ws://localhost",
    ] {
        let (mut h, target, _) = spawn_local_stock_hit_harness(6);
        {
            let mut net = h.app.world_mut().resource_mut::<NetState>();
            net.address = address.into();
            net.stock_npc_actors.clear();
            net.stock_npc_ready.clear();
        }
        h.app.world_mut().entity_mut(target).insert(RemoteFighter {
            player_id: "stock-2".into(),
        });

        let guard_pose = guard_pose_for_weapon(weapon_to_strategy(6)).unwrap();
        h.app
            .world_mut()
            .resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
                let mut pose = world.get_mut::<PoseRuntime>(target).unwrap();
                assets.0["p1"].apply_pose(&mut pose, guard_pose, true);
            });
        let mut cursor = bevy::ecs::message::MessageCursor::<CombatImpactEvent>::default();
        let mut feedback_count = 0;
        let mut claimed = false;
        h.press_mouse(MouseButton::Left);
        for tick in 0..150 {
            h.step();
            if tick == 0 {
                h.release_mouse(MouseButton::Left);
            }
            crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
            h.app.world_mut().run_system_once(attack_windows).unwrap();
            feedback_count += cursor
                .read(
                    h.app
                        .world()
                        .resource::<bevy::ecs::message::Messages<CombatImpactEvent>>(),
                )
                .count();
            if h.app.world().get::<PendingRemoteHit>(target).is_some() {
                claimed = true;
                break;
            }
        }
        assert!(
            claimed,
            "{address}: real sword geometry must still report the hit"
        );
        assert_eq!(
            h.app.world().get::<PoseRuntime>(target).unwrap().pose_id,
            guard_pose
        );
        assert_ne!(
            h.app.world().get::<CombatState>(target).unwrap().state,
            FighterState::Guard
        );
        assert_eq!(
            feedback_count,
            1,
            "{address}: retain immediate camera feedback; the effect consumer owns stock audio gating"
        );
        assert_eq!(h.app.world().get::<Health>(target).unwrap().current, 1000.0);
    }
}
