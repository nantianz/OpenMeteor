#[test]
fn fake_push_velocity_does_not_accumulate_and_stop_preserves_entry_decay() {
    use crate::plugins::stage_plugin::LocalFighter;
    for serial in [false, true] {
        for fps in [30.0_f32, 60.0, 120.0] {
            let dt = 1.0 / fps;
            let (mut app, actor) =
                repulsion_app(serial, FighterState::Fake, meteor_core::MovementMode::Idle);
            app.world_mut().entity_mut(actor).insert(LocalFighter);
            app.world_mut()
                .get_mut::<PoseRuntime>(actor)
                .unwrap()
                .pose_id = 117;
            app.world_mut()
                .get_mut::<MotionState>(actor)
                .unwrap()
                .velocity = Vec3::X * 12.0;
            app.world_mut()
                .resource_mut::<crate::plugins::FrameClock>()
                .sim_dt = dt;
            let mut carry = 12.0;
            for tick in 0..(fps as usize * 2) {
                let pushing = tick < fps as usize;
                let force = if pushing { Vec3::X * 108.0 } else { Vec3::ZERO };
                app.world_mut()
                    .resource_mut::<CharacterContactFrame>()
                    .authored_repulsion
                    .insert(actor, force);
                let before = app.world().get::<Transform>(actor).unwrap().translation;
                app.world_mut().run_schedule(Update);
                carry *= 1.0 - 9.0 * dt;
                let expected = carry;
                let actual = app.world().get::<MotionState>(actor).unwrap().velocity.x;
                let after = app.world().get::<Transform>(actor).unwrap().translation;
                assert!(
                    (actual - expected).abs() < 2e-4,
                    "serial={serial} fps={fps} tick={tick}: {actual} != {expected}"
                );
                assert!(((after.x - before.x) / dt - (actual + force.x * dt)).abs() < 2e-4);
            }
            assert!(
                app.world()
                    .get::<MotionState>(actor)
                    .unwrap()
                    .velocity
                    .length()
                    < 0.001
            );

            app.world_mut()
                .get_mut::<PoseRuntime>(actor)
                .unwrap()
                .pose_instance_id += 1;
            app.world_mut()
                .get_mut::<MotionState>(actor)
                .unwrap()
                .velocity = Vec3::Z * 6.0;
            app.world_mut().run_schedule(Update);
            assert!(
                (app.world().get::<MotionState>(actor).unwrap().velocity.z
                    - 6.0 * (1.0 - 9.0 * dt))
                    .abs()
                    < 1e-5
            );
        }
    }
}

fn repulsion_app(
    serial: bool,
    state: FighterState,
    mode: meteor_core::MovementMode,
) -> (App, Entity) {
    let mut app = App::new();
    crate::plugins::configure_fighter_frame_sets(&mut app);
    if serial {
        crate::plugins::fighter_frame::install(&mut app);
    }
    let schedule = super::super::fighter_frame::schedule(&app);
    app.add_systems(
        schedule,
        crate::plugins::movement_plugin::fake_root_motion
            .in_set(crate::plugins::FighterFrameSet::RootMotion),
    );
    app.insert_resource(State::new(AppState::Playing))
        .init_resource::<Time>()
        .insert_resource(crate::plugins::FrameClock {
            sim_dt: 1.0 / 60.0,
            ..default()
        })
        .add_plugins(CharacterContactPlugin);
    let actor = app
        .world_mut()
        .spawn((
            fighter(0),
            Transform::default(),
            Health::default(),
            PoseRuntime {
                pose_id: 176,
                start: 100,
                end: 110,
                frame: 101.0,
                loops: false,
                ..PoseRuntime::idle()
            },
            SmState {
                current: state,
                ..default()
            },
            MotionState {
                movement_mode: mode,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            FighterAir::default(),
            FighterHitReaction::default(),
        ))
        .id();
    app.world_mut()
        .resource_mut::<CharacterContactFrame>()
        .authored_repulsion
        .insert(actor, Vec3::X * 108.0);
    (app, actor)
}

#[test]
fn player_npc_and_passive_dummy_share_force_save_and_consume() {
    use crate::plugins::{combat_plugin::NpcFighter, stage_plugin::LocalFighter};
    use meteor_core::MovementMode as M;
    for serial in [false, true] {
        for fps in [30.0_f32, 60.0, 120.0] {
            for (state, mode) in [
                (FighterState::Name, M::Idle),
                (FighterState::BeHit, M::Idle),
            ] {
                for identity in ["player", "npc", "dummy"] {
                    let (mut app, actor) = repulsion_app(serial, state, mode);
                    let dt = 1.0 / fps;
                    app.world_mut()
                        .resource_mut::<crate::plugins::FrameClock>()
                        .sim_dt = dt;
                    if identity == "player" {
                        app.world_mut().entity_mut(actor).insert(LocalFighter);
                    } else {
                        app.world_mut().entity_mut(actor).insert((
                            NpcFighter,
                            NpcPerformance {
                                pause_remaining: if identity == "dummy" {
                                    f32::INFINITY
                                } else {
                                    0.0
                                },
                                ..default()
                            },
                        ));
                    }

                    let candidate = app
                        .world_mut()
                        .spawn((
                            fighter(1),
                            Transform::from_xyz(1.2, 0.0, 0.0),
                            Health::default(),
                            PoseRuntime::idle(),
                            SmState::default(),
                            MotionState {
                                movement_mode: M::Idle,
                                velocity: Vec3::ZERO,
                                facing: 0.0,
                            },
                            RemoteFighter {
                                player_id: "peer".into(),
                            },
                        ))
                        .id();
                    app.world_mut()
                        .resource_mut::<CharacterContactFrame>()
                        .authored_repulsion
                        .clear();
                    app.update();
                    let force = app
                        .world()
                        .resource::<CharacterContactFrame>()
                        .authored_repulsion
                        .get(&actor)
                        .copied()
                        .unwrap_or(Vec3::ZERO);

                    assert!((force.x + 171.36).abs() < 0.001,
                        "{identity} state={state:?} serial={serial} must save the shared force: {force:?}");
                    app.update();
                    let position = app.world().get::<Transform>(actor).unwrap().translation;
                    assert!((position.x + 171.36 * dt * dt).abs() < 1e-5,
                        "{identity} state={state:?} serial={serial} must consume the preceding force: {position:?}");
                    assert_eq!(
                        app.world().get::<MotionState>(actor).unwrap().velocity,
                        Vec3::ZERO
                    );
                    assert_eq!(
                        app.world().get::<Transform>(candidate).unwrap().translation,
                        Vec3::X * 1.2
                    );
                }
            }
        }
    }
}

#[test]
fn stock_repulsion_dispatches_ground_air_guard_down_and_dodge_at_each_rate() {
    use meteor_core::MovementMode as M;

    for serial in [false, true] {
        for fps in [30.0, 60.0, 120.0] {
            for (state, mode, consumes) in [
                (FighterState::Name, M::Idle, true),
                (FighterState::Name, M::Crouch, true),
                (FighterState::Jump, M::Airborne, true),
                (FighterState::Guard, M::Idle, true),
                (FighterState::Down, M::Idle, true),
                (FighterState::Fake, M::Idle, true),
                (FighterState::Dodge, M::Dodge, true),
                (FighterState::Burst, M::Dodge, true),
                (FighterState::BeHit, M::Airborne, true),
                (FighterState::Attack1, M::Idle, true),
                (FighterState::Jump, M::WallRun, false),
                (FighterState::Cure, M::Idle, false),
            ] {
                let (mut app, actor) = repulsion_app(serial, state, mode);
                app.world_mut()
                    .resource_mut::<crate::plugins::FrameClock>()
                    .sim_dt = 1.0 / fps;
                app.world_mut().run_schedule(Update);
                let x = app.world().get::<Transform>(actor).unwrap().translation.x;
                let expected = if consumes { 108.0 / (fps * fps) } else { 0.0 };
                assert!(
                    (x - expected).abs() < 1e-6,
                    "{state:?}/{mode:?} {fps}Hz serial={serial}: {x} != {expected}"
                );
                assert_eq!(
                    app.world().get::<MotionState>(actor).unwrap().velocity,
                    Vec3::ZERO,
                    "{state:?}/{mode:?}: second mover must not seed pose/jump/network velocity"
                );
            }
        }
    }
}

#[test]
fn finished_dodge_does_not_gain_a_ground_push_in_the_same_tick() {
    fn finish(mut actors: Query<&mut SmState>) {
        for mut state in &mut actors {
            state.current = FighterState::Name;
        }
    }
    for serial in [false, true] {
        let (mut app, actor) = repulsion_app(
            serial,
            FighterState::Dodge,
            meteor_core::MovementMode::Dodge,
        );
        app.world_mut().get_mut::<PoseRuntime>(actor).unwrap().frame = 110.0;
        let schedule = super::super::fighter_frame::schedule(&app);
        app.add_systems(
            schedule,
            finish.in_set(crate::plugins::FighterFrameSet::RootMotion),
        );
        app.world_mut().run_schedule(Update);
        assert_eq!(
            app.world().get::<SmState>(actor).unwrap().current,
            FighterState::Name
        );
        assert_eq!(
            app.world().get::<Transform>(actor).unwrap().translation,
            Vec3::ZERO
        );
    }
}

#[test]
fn down_getup_transition_skips_old_mover_but_next_getup_tick_consumes_force() {
    #[derive(Resource)]
    struct Transition(bool);
    fn getup(
        mut first: ResMut<Transition>,
        mut actors: Query<(&mut SmState, &mut FighterHitReaction)>,
    ) {
        if !first.0 {
            return;
        }
        first.0 = false;
        for (mut state, mut reaction) in &mut actors {
            state.current = FighterState::BeHit;
            reaction.begin(
                meteor_combat::hit_reaction::getup_reaction_for_pose(119, 100, 110),
                false,
            );
        }
    }
    for serial in [false, true] {
        let (mut app, actor) =
            repulsion_app(serial, FighterState::Down, meteor_core::MovementMode::Idle);
        let schedule = super::super::fighter_frame::schedule(&app);
        app.insert_resource(Transition(true)).add_systems(
            schedule,
            getup.in_set(crate::plugins::FighterFrameSet::Locomotion),
        );
        app.world_mut().run_schedule(Update);
        assert_eq!(
            app.world().get::<Transform>(actor).unwrap().translation,
            Vec3::ZERO
        );
        app.world_mut()
            .resource_mut::<CharacterContactFrame>()
            .authored_repulsion
            .insert(actor, Vec3::X * 108.0);
        app.world_mut().run_schedule(Update);
        assert!((app.world().get::<Transform>(actor).unwrap().translation.x - 0.03).abs() < 1e-6);
    }
}

#[test]
fn attack_candidates_keep_pre_move_distance_until_the_next_frame() {
    use bevy::ecs::system::RunSystemOnce;
    let (mut app, actor) = repulsion_app(
        false,
        FighterState::Attack1,
        meteor_core::MovementMode::Idle,
    );
    let target = app
        .world_mut()
        .spawn((
            fighter(1),
            Transform::from_xyz(26.0 * meteor_core::MODEL_SCALE, 0.0, 0.0),
            PoseRuntime::idle(),
            SmState::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            Health::default(),
            FighterAir::default(),
            RemoteFighter {
                player_id: "candidate".into(),
            },
        ))
        .id();
    app.world_mut()
        .run_system_once(begin_character_contact_frame)
        .unwrap();
    app.world_mut()
        .resource_mut::<CharacterContactFrame>()
        .cache_candidates(
            actor,
            Vec3::ZERO,
            [(target, Vec3::X * (26.0 * meteor_core::MODEL_SCALE))].into_iter(),
        );
    app.world_mut()
        .get_mut::<Transform>(target)
        .unwrap()
        .translation
        .x = 10.0 * meteor_core::MODEL_SCALE;
    app.world_mut()
        .run_system_once(snapshot_authored_repulsion)
        .unwrap();

    assert_eq!(
        app.world()
            .resource::<CharacterContactFrame>()
            .authored_repulsion[&actor],
        Vec3::ZERO
    );
    app.world_mut()
        .run_system_once(begin_character_contact_frame)
        .unwrap();
    app.world_mut()
        .run_system_once(snapshot_authored_repulsion)
        .unwrap();
    let force = app
        .world()
        .resource::<CharacterContactFrame>()
        .authored_repulsion[&actor];
    assert!((force.x + 1428.0 * meteor_core::MODEL_SCALE).abs() < 0.001);
}

#[test]
fn death_and_weapon_switch_consume_until_playback_finishes() {
    for serial in [false, true] {
        for dead in [false, true] {
            let (mut app, actor) = repulsion_app(
                serial,
                if dead {
                    FighterState::Dead
                } else {
                    FighterState::Name
                },
                meteor_core::MovementMode::Idle,
            );
            if dead {
                app.world_mut().get_mut::<Health>(actor).unwrap().current = 0.0;
            } else {
                app.world_mut()
                    .entity_mut(actor)
                    .insert(WeaponSwitchActive::default());
            }
            app.world_mut().run_schedule(Update);
            let first = app.world().get::<Transform>(actor).unwrap().translation;
            assert!((first.x - 0.03).abs() < 1e-6);
            app.world_mut().get_mut::<PoseRuntime>(actor).unwrap().frame = 110.0;
            app.world_mut()
                .resource_mut::<CharacterContactFrame>()
                .authored_repulsion
                .insert(actor, Vec3::X * 108.0);
            app.world_mut().run_schedule(Update);
            assert_eq!(
                app.world().get::<Transform>(actor).unwrap().translation,
                first
            );
        }
    }
}

#[test]
fn second_repulsion_mover_replaces_native_contact_external_slot() {
    use bevy::ecs::system::RunSystemOnce;
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("second_mover_contact_oracle.json")).unwrap();
    for case in oracle["cases"].as_array().unwrap() {
        let fps = case["fps"].as_u64().unwrap() as f32;
        let dt = 1.0 / fps;
        let external = case["primary"]["externalSource"][0].as_f64().unwrap() as f32
            * meteor_core::MODEL_SCALE;
        let expected = case["secondary"]["externalSource"][0].as_f64().unwrap() as f32
            * meteor_core::MODEL_SCALE;
        for state in [
            FighterState::Name,
            FighterState::Attack1,
            FighterState::BeHit,
        ] {
            for identity in ["player", "npc", "dummy", "remote"] {
                for force in [0.0, 5.0 * meteor_core::MODEL_SCALE, 108.0] {
                    let (mut app, actor) =
                        repulsion_app(false, state, meteor_core::MovementMode::Idle);
                    app.world_mut()
                        .resource_mut::<crate::plugins::FrameClock>()
                        .sim_dt = dt;
                    let main = Vec3::new(7.0, 13.0, -11.0) * meteor_core::MODEL_SCALE;
                    app.world_mut()
                        .get_mut::<MotionState>(actor)
                        .unwrap()
                        .velocity = main;
                    {
                        let mut air = app.world_mut().get_mut::<FighterAir>(actor).unwrap();
                        air.ground_external_velocity = Vec3::X * external;
                        air.carry_decay_steep = true;
                    }
                    if identity == "remote" {
                        app.world_mut().entity_mut(actor).insert(RemoteFighter {
                            player_id: "peer".into(),
                        });
                    } else if identity != "player" {
                        app.world_mut().entity_mut(actor).insert((
                            crate::plugins::combat_plugin::NpcFighter,
                            NpcPerformance {
                                pause_remaining: if identity == "dummy" {
                                    f32::INFINITY
                                } else {
                                    0.0
                                },
                                ..default()
                            },
                        ));
                    }
                    app.world_mut()
                        .resource_mut::<CharacterContactFrame>()
                        .authored_repulsion
                        .insert(actor, Vec3::X * force);
                    if state == FighterState::Name {
                        app.world_mut()
                            .run_system_once(apply_locomotion_repulsion)
                            .unwrap();
                    } else {
                        app.world_mut()
                            .run_system_once(apply_authored_repulsion)
                            .unwrap();
                    }
                    let runs = identity != "remote" && force > 5.0 * meteor_core::MODEL_SCALE;
                    let air = app.world().get::<FighterAir>(actor).unwrap();
                    assert!(
                        (air.ground_external_velocity.x - if runs { expected } else { external })
                            .abs()
                            < 1e-5,
                        "fps={fps} state={state:?} identity={identity} force={force}: {:?}",
                        air.ground_external_velocity
                    );
                    assert!(
                        air.carry_decay_steep,
                        "zero vertical second mover preserves support"
                    );
                    assert_eq!(
                        app.world().get::<MotionState>(actor).unwrap().velocity,
                        main
                    );
                    let after = app.world().get::<Transform>(actor).unwrap().translation;
                    assert!((after.x - if runs { force * dt * dt } else { 0.0 }).abs() < 1e-6);
                }
            }
        }
    }
}
