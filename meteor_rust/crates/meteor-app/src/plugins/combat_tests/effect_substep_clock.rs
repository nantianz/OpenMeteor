#[test]
#[cfg(not(target_arch = "wasm32"))]
fn production_effect_attack_clocks_follow_each_substep_and_keep_new_spawn_age() {
    use crate::plugins::effect_plugin as fx;
    use bevy::ecs::system::RunSystemOnce;

    #[derive(Resource, Default)]
    struct Samples(Vec<(f64, Entity, f32, Vec<Obb>)>);
    let (mut h, target, take_hits) = spawn_local_stock_hit_harness_with_order(11, true);
    let attacker = h.fighter;
    for entity in [attacker, target] {
        h.app
            .world_mut()
            .entity_mut(entity)
            .insert(crate::components::fighter::VisualRootPin::default());
    }
    h.app.world_mut().entity_mut(target).insert((
        WeaponForm::default(),
        FighterHitReaction::default(),
        GunAimState::default(),
        RemoteFighter {
            player_id: "stock-2".into(),
        },
    ));
    {
        let mut net = h.app.world_mut().resource_mut::<NetState>();
        net.stock_npc_ready.clear();
        net.stock_npc_actors.clear();
    }
    h.app
        .world_mut()
        .get_mut::<MotionState>(attacker)
        .unwrap()
        .facing = 0.0;
    h.app
        .world_mut()
        .resource_mut::<crate::plugins::camera_plugin::CameraBasis>()
        .yaw = 0.0;
    h.app
        .world_mut()
        .get_mut::<Transform>(attacker)
        .unwrap()
        .translation = Vec3::ZERO;
    h.app
        .world_mut()
        .get_mut::<Transform>(target)
        .unwrap()
        .translation = Vec3::new(0.0, 0.0, 6.0);
    h.app.world_mut().get_mut::<Health>(target).unwrap().current = 10000.0;
    h.app
        .init_resource::<Assets<fx::PonEfMaterial>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<fx::EfTextureCache>()
        .init_resource::<Assets<crate::plugins::stage_material::GmdlMaterial>>()
        .init_resource::<Samples>()
        .add_message::<fx::SceneEffectEvent>();
    h.app
        .world_mut()
        .run_system_once(fx::setup_effect_assets)
        .unwrap();
    fx::install_effect_playback_systems(&mut h.app);
    crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
    h.app.add_plugins(crate::plugins::CharacterContactPlugin);
    crate::plugins::network_plugin::install_hit_sender_for_test(&mut h.app);
    crate::plugins::install_frame_clock(&mut h.app);
    let schedule = crate::plugins::fighter_frame::schedule(&h.app);
    h.app.add_systems(
        schedule,
        attack_windows.in_set(crate::plugins::FighterFrameSet::HitJudge),
    );
    h.app.add_systems(
        schedule,
        (move |clock: Res<crate::plugins::FrameClock>,
               geometry: HitGeometryQueries,
               assets: Res<CharacterAssetsSet>,
               fighters: Query<(&Fighter, &WeaponForm, &PoseRuntime)>,
               effects: Query<(Entity, &fx::PoseOwnedPonEf, &fx::PonEfInstance)>,
               mut samples: ResMut<Samples>| {
            let (fighter, form, pose) = fighters.get(attacker).unwrap();
            for (entity, owner, effect) in &effects {
                if owner.fighter != attacker {
                    continue;
                }
                samples.0.push((
                    crate::plugins::simulation_now(Some(&clock), -1.0),
                    entity,
                    effect.elapsed,
                    geometry.live_effect_obbs(
                        attacker,
                        fighter,
                        form,
                        pose.pose_id,
                        &assets.0["p1"],
                    ),
                ));
            }
        })
        .in_set(crate::plugins::FighterFrameSet::HitJudge)
        .before(attack_windows)
        .run_if(crate::plugins::fighter_frame::current_is_local),
    );

    let mut pose = PoseRuntime::idle();
    let assets = &h.app.world().resource::<CharacterAssetsSet>().0["p1"];
    assets.apply_pose(&mut pose, 416, true);
    assert_eq!(assets.poses[&416].attacks[0].bones, ["effect"]);
    pose.frame = assets.poses[&416].attacks[0].start as f32 - 0.25;
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

    h.step_duration(std::time::Duration::from_millis(100));
    let samples = &h.app.world().resource::<Samples>().0;
    assert_eq!(
        samples.len(),
        12,
        "spawn is visible to every actual controller hit scan"
    );
    let effect_entity = samples[0].1;
    let entry_age = samples[0].2;
    for pair in samples.windows(2) {
        assert_eq!(pair[0].1, effect_entity);
        assert!(
            ((pair[1].2 - pair[0].2) - (pair[1].0 - pair[0].0) as f32).abs() < 2e-6,
            "new EF must advance on subsequent substeps rather than retain its attachment offset"
        );
    }
    let first_frame_end_age = samples.last().unwrap().2;
    assert!((first_frame_end_age - entry_age - 11.0 / 120.0).abs() < 2e-6);
    assert!(
        (h.app
            .world()
            .get::<fx::PonEfInstance>(effect_entity)
            .unwrap()
            .elapsed
            - first_frame_end_age)
            .abs()
            < 2e-6,
        "frame-tail rendering must not reset PonEfStartTime or add the full frame again"
    );

    let mut claims = take_hits();
    h.step_duration(std::time::Duration::from_millis(100));
    claims.extend(take_hits());
    let samples = &h.app.world().resource::<Samples>().0;
    assert_eq!(samples.len(), 24);
    for pair in samples[11..].windows(2) {
        assert!(
            ((pair[1].2 - pair[0].2) - (pair[1].0 - pair[0].0) as f32).abs() < 2e-6,
            "existing EF must sample each current substep, not the display endpoint"
        );
    }
    let geometry_samples: Vec<_> = samples
        .iter()
        .filter(|sample| !sample.3.is_empty())
        .collect();
    assert!(
        geometry_samples.len() > 1,
        "original Pos416 attack nodes must be sampled"
    );
    assert!(
        geometry_samples
            .windows(2)
            .any(|pair| pair[0]
                .3
                .iter()
                .zip(&pair[1].3)
                .any(|(a, b)| (a.center - b.center).length() > 1e-5)),
        "the live effect attack shape moves during catch-up"
    );
    for _ in 0..10 {
        if !claims.is_empty() {
            break;
        }
        h.step_duration(std::time::Duration::from_millis(100));
        claims.extend(take_hits());
    }
    assert!(
        !claims.is_empty(),
        "the real effect-only attack must report a LAN hit at six units"
    );
    assert_eq!(
        h.app.world().get::<Health>(target).unwrap().current,
        10000.0,
        "LAN still awaits authority"
    );

    assert!(
        h.app
            .world()
            .get::<fx::PonEfInstance>(effect_entity)
            .is_some(),
        "interruption must remove a live effect, not merely observe normal expiry"
    );
    let samples_before_interrupt = h.app.world().resource::<Samples>().0.len();
    h.app
        .world_mut()
        .get_mut::<PoseRuntime>(attacker)
        .unwrap()
        .pose_instance_id += 1;
    h.app
        .world_mut()
        .get_mut::<CombatState>(attacker)
        .unwrap()
        .state = FighterState::BeHit;
    h.app
        .world_mut()
        .get_mut::<SmState>(attacker)
        .unwrap()
        .current = FighterState::BeHit;
    h.step_duration(std::time::Duration::from_millis(100));
    assert!(
        h.app.world().get_entity(effect_entity).is_err(),
        "interrupted EF removed within actor schedule"
    );
    assert!(
        !h.app.world().resource::<Samples>().0[samples_before_interrupt..]
            .iter()
            .any(|sample| sample.1 == effect_entity),
        "interrupted attack geometry must not survive into the following substeps"
    );
}
