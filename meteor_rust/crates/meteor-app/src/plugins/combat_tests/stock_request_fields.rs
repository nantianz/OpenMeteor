#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_stock_melee_claim_preserves_both_authored_reaction_branches() {
    use bevy::ecs::system::RunSystemOnce;

    for guarded_projection in [false, true] {
        let (mut h, target, take_hits) = spawn_local_stock_hit_harness(DAGGER);
        let attacker = h.fighter;
        for entity in [attacker, target] {
            h.app
                .world_mut()
                .entity_mut(entity)
                .insert(crate::components::fighter::VisualRootPin::default());
        }
        h.app.world_mut().entity_mut(target).insert((
            RemoteFighter {
                player_id: "stock-2".into(),
            },
            WeaponForm::default(),
            FighterHitReaction::default(),
            GunAimState::default(),
        ));
        {
            let mut net = h.app.world_mut().resource_mut::<NetState>();
            net.stock_npc_ready.clear();
            net.stock_npc_actors.clear();
        }
        crate::plugins::network_plugin::install_hit_sender_for_test(&mut h.app);
        h.app.world_mut().get_mut::<Health>(target).unwrap().current = 10000.0;
        h.app
            .world_mut()
            .get_mut::<CombatState>(target)
            .unwrap()
            .state = if guarded_projection {
            FighterState::Guard
        } else {
            FighterState::Name
        };
        let mut pose = PoseRuntime::idle();
        let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
        assets.apply_pose(&mut pose, 258, true);
        let attack = assets.poses[&258].attacks[2].clone();
        assert!(attack.defense_value > 0.0 && attack.target_value > 0.0);
        assert_eq!(
            attack.attack_type, 0,
            "guarded case must exercise the predicted guard branch"
        );
        pose.frame = (attack.start + attack.end) as f32 * 0.5;
        h.app
            .world_mut()
            .entity_mut(attacker)
            .insert((pose, EntryBlend::default()));
        h.app
            .world_mut()
            .get_mut::<SmState>(attacker)
            .unwrap()
            .current = FighterState::Attack1;
        h.app
            .world_mut()
            .get_mut::<CombatState>(attacker)
            .unwrap()
            .state = FighterState::Attack1;
        crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
        place_target_in_current_attack(&mut h, target);
        crate::plugins::character_plugin::drive_fighter_animation_for_test(h.app.world_mut());
        h.app.world_mut().run_system_once(attack_windows).unwrap();
        h.app.world_mut().run_schedule(PostUpdate);
        let requests = take_hits();
        let [(None, request)] = requests.as_slice() else {
            panic!("expected one real melee claim, guard={guarded_projection}: {requests:?}");
        };

        let authored = meteor_network::stock_protocol::StockHitRequest::from_attack(
            2,
            258,
            DAGGER,
            &attack,
            FacingBucket::Front,
            &[],
        );
        assert_eq!((request.moves_word, request.values_word), (authored.moves_word, authored.values_word),
            "both server reaction branches must retain the encoded author values, guard={guarded_projection}");
        assert_eq!(
            h.app.world().get::<Health>(target).unwrap().current,
            10000.0,
            "remote vitals remain authority-owned"
        );
    }
}
