#[test]
fn production_npc_ranged_shots_keep_actor_yaw_when_target_moves_behind() {
    use bevy::ecs::system::RunSystemOnce;
    for weapon in [1, 2, 3] {
        for fps in [30, 60, 120] {
            let (mut h, target) = spawn_npc_combat_harness(weapon, [100.0, 0.0, 0.0]);
            h.app
                .world_mut()
                .get_mut::<NpcCombatProfile>(h.fighter)
                .unwrap()
                .aim = 100.0;
            h.app
                .init_resource::<CombatResourceData>()
                .init_resource::<crate::plugins::audio_plugin::SfxCache>()
                .init_resource::<Assets<AudioSource>>()
                .init_resource::<crate::plugins::effect_plugin::EfCache>()
                .init_resource::<crate::plugins::item_plugin::PropHitGate>()
                .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
                .init_resource::<PendingRangedHits>()
                .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>()
                .add_message::<crate::plugins::render_plugin::OfflineWeaponDeathDrop>();
            crate::plugins::character_plugin::spawn_fighter_model_for_test(
                h.app.world_mut(),
                h.fighter,
            );
            let dt = 1.0 / fps as f32;
            let mut entered = false;
            for _ in 0..fps * 4 {
                h.step_dt(dt);
                if ranged_attack_profile(h.pose_id(), weapon, None)
                    .is_some_and(|profile| profile.entity || profile.ray)
                {
                    entered = true;
                    break;
                }
            }
            assert!(
                entered,
                "weapon={weapon}, fps={fps}: reaches its real firing pose"
            );
            let initial = h
                .app
                .world()
                .get::<AiIntent>(h.fighter)
                .unwrap()
                .ranged_aim
                .unwrap();
            let facing = h.app.world().get::<MotionState>(h.fighter).unwrap().facing;
            let actor = h.position();
            let side_yaw = facing + 15.0_f32.to_radians();
            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = actor + Vec3::new(side_yaw.sin() * 16.0, 5.0, side_yaw.cos() * 16.0);
            h.app
                .world_mut()
                .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
                .unwrap()
                .set_decision_for_test(0.0, 11);
            h.app
                .world_mut()
                .get_mut::<AiIntent>(h.fighter)
                .unwrap()
                .attack_cooldown = 0.0;
            h.step_dt(dt);
            let side = h
                .app
                .world()
                .get::<AiIntent>(h.fighter)
                .unwrap()
                .ranged_aim
                .unwrap();
            assert!(
                side.y > initial.y,
                "aligned decisions still update target pitch"
            );
            assert!(
                (side.x.atan2(side.z) - facing).abs() < 1e-5,
                "perfect Aim uses actor yaw, not a 15-degree target correction"
            );

            h.app
                .world_mut()
                .get_mut::<Transform>(target)
                .unwrap()
                .translation = actor - Vec3::new(facing.sin() * 16.0, 0.0, facing.cos() * 16.0);
            h.app
                .world_mut()
                .get_mut::<crate::plugins::ai_plugin::NpcPatrol>(h.fighter)
                .unwrap()
                .set_decision_for_test(0.0, 11);
            h.app
                .world_mut()
                .get_mut::<AiIntent>(h.fighter)
                .unwrap()
                .attack_cooldown = 0.0;
            h.step_dt(dt);
            let intent = h.app.world().get::<AiIntent>(h.fighter).unwrap();
            assert!(
                !intent.want_attack,
                "back-facing Think must not submit another attack"
            );
            assert_eq!(
                intent.ranged_aim,
                Some(side),
                "angle failure keeps the saved shot"
            );

            let mut released = false;
            for _ in 0..fps * 3 {
                crate::plugins::character_plugin::drive_fighter_animation_for_test(
                    h.app.world_mut(),
                );
                h.app.world_mut().run_system_once(attack_windows).unwrap();
                let world = h.app.world_mut();
                let mut query = world.query::<&Projectile>();
                if let Some(shot) = query.iter(world).next() {
                    assert_eq!(shot.source.owner, h.fighter);
                    let direction = shot.source.attack_forward;
                    assert!(
                        (direction.x.atan2(direction.z) - facing).abs() < 1e-5,
                        "weapon={weapon}, fps={fps}: actual release cannot fire behind"
                    );
                    assert_eq!(
                        query.iter(world).count(),
                        1,
                        "one release per firing instance"
                    );
                    released = true;
                    break;
                }
                h.step_dt(dt);
            }
            assert!(
                released,
                "weapon={weapon}, fps={fps}: authored Attack frame releases a shot"
            );

            h.app.world_mut().run_system_once(attack_windows).unwrap();
            let world = h.app.world_mut();
            let mut query = world.query::<&Projectile>();
            assert_eq!(query.iter(world).count(), 1);
        }
    }
}
