use super::*;
use crate::plugins::character_contact_plugin::{CharacterContactFrame, CharacterContactPlugin};
use crate::plugins::movement_plugin::{COLLIDER_HEIGHT, COLLIDER_RADIUS};
use crate::plugins::network_plugin::RemoteContactSegment;
use meteor_physics::collision::{CollisionSystem, Point3};

fn sample_vec3(value: &serde_json::Value) -> Vec3 {
    Vec3::new(
        value[0].as_f64().unwrap() as f32,
        value[1].as_f64().unwrap() as f32,
        value[2].as_f64().unwrap() as f32,
    )
}

fn contact_harness(peer_position: Vec3, remote: bool) -> (GameplayHarness, Entity) {
    let mut h = spawn_gameplay_harness_with_order(DAGGER, true).expect("original P1 assets");
    h.app
        .insert_resource(State::new(AppState::Playing))
        .add_plugins(CharacterContactPlugin);
    let mut pose = PoseRuntime::idle();
    h.app.world().resource::<CharacterAssetsSet>().0["p1"].apply_pose(&mut pose, 0, true);
    let mut peer = h.app.world().get::<Fighter>(h.fighter).unwrap().clone();
    peer.player_slot = 1;
    let target = h
        .app
        .world_mut()
        .spawn((
            peer,
            Transform::from_translation(peer_position),
            pose,
            EntryBlend::default(),
            SmState::default(),
            Health::default(),
            FighterHitReaction::default(),
            GunAimState::default(),
            WeaponForm::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: std::f32::consts::PI,
            },
        ))
        .id();
    if remote {
        h.app.world_mut().entity_mut(target).insert(RemoteFighter {
            player_id: "stock-2".into(),
        });
        let mut net = h.app.world_mut().resource_mut::<NetState>();
        net.status = NetStatus::InRoom;
        net.address = "udp://127.0.0.1:5230".into();
        net.player_id = "stock-1".into();
        net.mode = "leader".into();
        net.round_active = true;
    }
    for entity in [h.fighter, target] {
        crate::plugins::character_plugin::spawn_fighter_model_for_test(h.app.world_mut(), entity);
        h.app
            .world_mut()
            .resource_mut::<CharacterContactFrame>()
            .audit_record_moves(entity);
    }
    crate::plugins::character_plugin::install_crowd_animation_for_test(&mut h.app);
    (h, target)
}

pub(super) fn walk_and_authored_attack_respect_walls() {
    for fps in [30, 60, 120] {
        for attack in [false, true] {
            let target_start = Vec3::new(0.0, 0.0, 2.5);
            let (mut h, target) = contact_harness(target_start, false);
            let mesh = harness_ground_with_front_wall(4.8);
            h.replace_stage_mesh(mesh.clone());
            let mut stage = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
            stage.set_mesh_collider(mesh);
            if attack {
                h.app
                    .world_mut()
                    .resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
                        assets.0["p1"].apply_pose(
                            &mut world.get_mut::<PoseRuntime>(h.fighter).unwrap(),
                            258,
                            true,
                        );
                    });
                h.app
                    .world_mut()
                    .get_mut::<SmState>(h.fighter)
                    .unwrap()
                    .current = FighterState::Attack1;
                h.app
                    .world_mut()
                    .get_mut::<CombatState>(h.fighter)
                    .unwrap()
                    .state = FighterState::Attack1;
            } else {
                h.press_binding(crate::app_state::Keymap::default().move_up);
            }
            let dt = 1.0 / fps as f32;
            let mut previous_target = target_start;
            let mut repulsion_steps = 0;
            let mut primary_travel = 0.0;
            let mut nearest = f32::INFINITY;
            for tick in 0..fps * 2 {
                h.step_dt(dt);
                let mover = h.position();
                let pushed = h.app.world().get::<Transform>(target).unwrap().translation;
                let frame = h.app.world().resource::<CharacterContactFrame>();
                let mut accepted_repulsion = Vec3::ZERO;
                for movement in frame.audit_moves(target) {
                    if movement["kind"] == "repulsion" {
                        repulsion_steps += 1;
                        let force = sample_vec3(&movement["force"]);

                        let expected_request = (force * dt * dt)
                            .clamp_length_max(600.0 * meteor_core::MODEL_SCALE * dt);
                        let requested = sample_vec3(&movement["requested"]);
                        assert!(
                            (requested - expected_request).length() < 2e-5,
                            "{fps} FPS attack={attack} tick={tick}: second-mover input {movement}"
                        );
                        accepted_repulsion += sample_vec3(&movement["accepted"]);
                    }
                }
                assert!((pushed - previous_target - accepted_repulsion).length() < 2e-5,
                    "{fps} FPS attack={attack} tick={tick}: a passive target may move only by its own saved force; previous={previous_target:?}, next={pushed:?}, force_move={accepted_repulsion:?}");
                assert_eq!(h.app.world().get::<MotionState>(target).unwrap().velocity, Vec3::ZERO,
                    "{fps} FPS attack={attack} tick={tick}: contact must not seed the target's persistent velocity");
                for movement in frame.audit_moves(h.fighter) {
                    if movement["kind"] == "primary" {
                        let accepted = sample_vec3(&movement["accepted"]);
                        let scene = sample_vec3(&movement["stage"]);
                        assert!(
                            (accepted - scene).length() < 1e-6,
                            "actor proximity must not add a hard-circle veto: {movement}"
                        );
                        primary_travel += accepted.xz().length();
                    }
                }
                assert!(stage.can_occupy(Point3::new(mover.x, mover.y, mover.z)),
                    "{fps} FPS attack={attack} tick={tick}: mover crossed the scene wall: {mover:?}");
                assert!(stage.can_occupy(Point3::new(pushed.x, pushed.y, pushed.z)),
                    "{fps} FPS attack={attack} tick={tick}: repulsion crossed the scene wall: {pushed:?}");
                nearest = nearest.min((pushed - mover).xz().length());
                previous_target = pushed;
            }
            assert!(
                primary_travel > 0.1,
                "must exercise real walking/authored root movement"
            );
            assert!(
                repulsion_steps > 0 && previous_target.z > target_start.z + 0.01,
                "must exercise the target's formula repulsion at {fps} FPS attack={attack}"
            );
            if !attack {
                assert!(
                    nearest < 16.0 * meteor_core::MODEL_SCALE - 0.01,
                    "walking must reach inside the old 8+8 hard-circle diameter: {nearest}"
                );
            }
        }
    }
}

#[test]
fn lan_remote_contact_preserves_primary_motion_and_peer_ownership() {
    for fps in [30, 60, 120] {
        let dt = 1.0 / fps as f32;
        let near_position = Vec3::new(0.0, 0.0, 1.2);
        let (mut near, peer) = contact_harness(near_position, true);
        let (mut empty, _) = contact_harness(Vec3::X * 30.0, true);
        let forward = crate::app_state::Keymap::default().move_up;
        near.press_binding(forward);
        empty.press_binding(forward);

        near.step_dt(dt);
        empty.step_dt(dt);
        assert!(
            empty.position().z > 0.0,
            "positive control must actually walk"
        );
        assert!((near.position() - empty.position()).length() < 1e-6,
            "{fps} FPS: an observed peer must not hard-block primary motion; nearby={:?}, control={:?}", near.position(), empty.position());
        assert_eq!(
            near.app.world().get::<Transform>(peer).unwrap().translation,
            near_position,
            "the observing client cannot move the peer on its owner's behalf"
        );
        assert_eq!(
            near.app.world().get::<MotionState>(peer).unwrap().velocity,
            Vec3::ZERO
        );
    }
}

#[test]
fn lan_remote_segment_cannot_add_fixed_push_or_persistent_velocity() {
    for fps in [30, 60, 120] {
        let dt = 1.0 / fps as f32;
        let peer_position = Vec3::X * 1.2;
        let peer_velocity = Vec3::new(0.7, 0.0, -0.2);
        let (mut h, peer) = contact_harness(peer_position, true);
        h.app
            .world_mut()
            .get_mut::<MotionState>(peer)
            .unwrap()
            .velocity = peer_velocity;
        h.app
            .world_mut()
            .entity_mut(peer)
            .insert(RemoteContactSegment {
                from: Vec3::new(-3.0, 0.0, 0.0),
                to: peer_position,
                valid: true,
                role_driven: true,
                ..default()
            });
        let initial = h.position();
        h.step_dt(dt);
        assert!(
            (h.position() - initial).xz().length() < 1e-6,
            "{fps} FPS: the peer's displayed segment must not enqueue a fixed 20-source/s push"
        );
        assert_eq!(
            h.app
                .world()
                .get::<MotionState>(h.fighter)
                .unwrap()
                .velocity
                .xz(),
            Vec2::ZERO
        );

        let saved = h
            .app
            .world()
            .resource::<CharacterContactFrame>()
            .audit_moves(h.fighter)
            .iter()
            .find(|movement| movement["kind"] == "repulsion-save")
            .expect("must execute the owned actor's force producer");
        assert!((sample_vec3(&saved["force"]).x + 171.36).abs() < 0.001);
        h.step_dt(dt);
        assert!(
            (h.position().x - initial.x + 171.36 * dt * dt).abs() < 2e-5,
            "{fps} FPS: local displacement must be exactly the previous soft force: {:?}",
            h.position()
        );
        assert_eq!(
            h.app
                .world()
                .get::<MotionState>(h.fighter)
                .unwrap()
                .velocity
                .xz(),
            Vec2::ZERO,
            "soft displacement must not become carried/network velocity"
        );
        assert_eq!(
            h.app.world().get::<Transform>(peer).unwrap().translation,
            peer_position
        );
        assert_eq!(
            h.app.world().get::<MotionState>(peer).unwrap().velocity,
            peer_velocity
        );

        h.app
            .world_mut()
            .get_mut::<Transform>(peer)
            .unwrap()
            .translation = Vec3::X * 30.0;
        h.step_dt(dt);
        let after_cached_force = h.position();
        for _ in 0..4 {
            h.step_dt(dt);
            assert!(
                (h.position() - after_cached_force).xz().length() < 1e-6,
                "{fps} FPS: no fixed-push residual after the saved force is consumed"
            );
            assert_eq!(
                h.app
                    .world()
                    .get::<MotionState>(h.fighter)
                    .unwrap()
                    .velocity
                    .xz(),
                Vec2::ZERO
            );
        }
    }
}

#[test]
fn lan_remote_contact_uses_complete_production_clock_substeps() {
    use crate::plugins::character_contact_plugin::CharacterContactFinishSet;
    use crate::plugins::fighter_frame::FighterStepEnd;

    #[derive(Resource, Default)]
    struct Steps(Vec<(f32, Vec3, Vec3, Vec3, Vec3, Vec<serde_json::Value>)>);

    let mut endpoints = Vec::new();
    for fps in [30, 60, 120] {
        let peer_position = Vec3::X * 1.2;
        let peer_velocity = Vec3::new(0.7, 0.0, -0.2);
        let (mut h, peer) = contact_harness(peer_position, true);
        let local = h.fighter;
        h.app
            .world_mut()
            .get_mut::<MotionState>(peer)
            .unwrap()
            .velocity = peer_velocity;
        crate::plugins::install_frame_clock(&mut h.app);
        h.app.init_resource::<Steps>().add_systems(
            FighterStepEnd,
            (move |clock: Res<crate::plugins::FrameClock>,
                   frame: Res<CharacterContactFrame>,
                   actors: Query<(&Transform, &MotionState)>,
                   mut steps: ResMut<Steps>| {
                let (local_transform, local_motion) = actors.get(local).unwrap();
                let (peer_transform, peer_motion) = actors.get(peer).unwrap();
                steps.0.push((
                    clock.sim_dt,
                    local_transform.translation,
                    local_motion.velocity,
                    peer_transform.translation,
                    peer_motion.velocity,
                    frame.audit_moves(local).to_vec(),
                ));
            })
            .after(CharacterContactFinishSet),
        );
        let initial = h.position();
        for _ in 0..fps / 2 {
            h.step_dt(1.0 / fps as f32);
        }
        let steps = &h.app.world().resource::<Steps>().0;
        assert_eq!(
            steps.len(),
            60,
            "{fps} FPS: all complete 120 Hz controller phases must execute"
        );
        let mut previous = initial;
        for (
            index,
            (dt, local_position, local_velocity, observed_position, observed_velocity, moves),
        ) in steps.iter().enumerate()
        {
            assert!((*dt - 1.0 / 120.0).abs() < 1e-8);
            let mut displacement = Vec3::ZERO;
            for movement in moves
                .iter()
                .filter(|movement| movement["kind"] == "repulsion")
            {
                let force = sample_vec3(&movement["force"]);
                let expected =
                    (force * *dt * *dt).clamp_length_max(600.0 * meteor_core::MODEL_SCALE * *dt);
                assert!((sample_vec3(&movement["requested"]) - expected).length() < 2e-5);
                displacement += sample_vec3(&movement["accepted"]);
            }
            assert!(
                (*local_position - previous - displacement).xz().length() < 2e-5,
                "{fps} FPS step={index}: contact must consume exactly one saved force per substep"
            );
            assert_eq!(local_velocity.xz(), Vec2::ZERO);
            assert_eq!(
                *observed_position, peer_position,
                "contact cannot integrate the observed peer"
            );
            assert_eq!(*observed_velocity, peer_velocity);
            previous = *local_position;
        }
        assert!(
            (steps[0].1 - initial).xz().length() < 1e-6,
            "first step has no saved force"
        );
        assert!(
            (steps[1].1.x - initial.x + 171.36 / (120.0 * 120.0)).abs() < 2e-5,
            "second complete step consumes the source-distance-10 PE oracle force"
        );
        assert!(
            h.position().x < -0.1,
            "positive control: soft separation must actually move the owner"
        );
        endpoints.push(h.position());
    }
    for endpoint in endpoints.iter().skip(1) {
        assert!((*endpoint - endpoints[0]).length() < 2e-5,
            "identical 120 Hz controller steps must yield the same contact endpoint across display rates: {endpoints:?}");
    }
}

#[test]
fn lan_remote_prediction_and_contact_share_production_substep_time() {
    use crate::plugins::character_contact_plugin::CharacterContactFinishSet;
    use crate::plugins::fighter_frame::FighterStepEnd;
    use crate::plugins::network_plugin::{install_stock_observer_smoothing, RemoteTransformTarget};

    #[derive(Clone, Debug)]
    struct Sample {
        dt: f32,
        local: Vec3,
        peer: Vec3,
        target: Vec3,
        state_age: f32,
        hit_age: f32,
        dirty: bool,
        pose_frame: f32,
    }
    #[derive(Resource, Default)]
    struct Samples(Vec<Sample>);

    for (speed, pending, initial_hit_age) in
        [(3.0, true, 10.0), (0.6, true, 10.0), (0.6, false, 0.38)]
    {
        let mut reference: Option<Vec<Sample>> = None;
        for fps in [120, 60, 30] {
            let (mut h, peer) = contact_harness(Vec3::X * 1.2, true);
            let local = h.fighter;
            let peer_velocity = Vec3::X * speed;
            let initial_target = Vec3::X * 1.4;
            h.app
                .world_mut()
                .get_mut::<MotionState>(peer)
                .unwrap()
                .velocity = peer_velocity;
            h.app.world_mut().entity_mut(peer).insert((
                RemoteTransformTarget {
                    translation: initial_target,
                    yaw: 0.5,
                },
                RemoteContactSegment {
                    role_driven: true,
                    pending_stock_state: pending,
                    hit_age: initial_hit_age,
                    ..default()
                },
            ));
            crate::plugins::install_frame_clock(&mut h.app);
            install_stock_observer_smoothing(&mut h.app);
            h.app.init_resource::<Samples>().add_systems(
                FighterStepEnd,
                (move |clock: Res<crate::plugins::FrameClock>, frame: Res<CharacterContactFrame>,
                       actors: Query<(&Transform, &MotionState, &PoseRuntime)>,
                       remotes: Query<(&RemoteTransformTarget, &RemoteContactSegment)>,
                       mut samples: ResMut<Samples>| {
                    let (local_transform, local_motion, _) = actors.get(local).unwrap();
                    let (peer_transform, peer_motion, peer_pose) = actors.get(peer).unwrap();
                    let (target, segment) = remotes.get(peer).unwrap();
                    assert_eq!(peer_motion.velocity, peer_velocity,
                        "observer contact cannot rewrite an authority velocity");
                    assert_eq!(local_motion.velocity.xz(), Vec2::ZERO,
                        "a moving observed peer cannot seed local carry velocity");
                    let saved = frame.audit_moves(local).iter()
                        .find(|movement| movement["kind"] == "repulsion-save").unwrap();
                    let offset = sample_vec3(&saved["position"]) - peer_transform.translation;
                    let source_distance_sq = offset.length_squared() / meteor_core::MODEL_SCALE.powi(2);
                    let expected_force = if source_distance_sq < 576.0 {
                        offset.normalize() * (300.0 * (576.0 / source_distance_sq - 1.0)
                            * meteor_core::MODEL_SCALE)
                    } else { Vec3::ZERO };
                    assert!((sample_vec3(&saved["force"]) - expected_force).length() < 0.002,
                        "force save must observe this substep's remote endpoint, not an earlier display endpoint");
                    samples.0.push(Sample {
                        dt: clock.sim_dt, local: local_transform.translation,
                        peer: peer_transform.translation, target: target.translation,
                        state_age: segment.state_age, hit_age: segment.hit_age,
                        dirty: segment.skip_stock_motion, pose_frame: peer_pose.frame,
                    });
                }).after(CharacterContactFinishSet),
            );
            for _ in 0..fps * 3 / 5 {
                h.step_dt(1.0 / fps as f32);
            }
            let samples = &h.app.world().resource::<Samples>().0;
            assert_eq!(
                samples.len(),
                72,
                "{fps} FPS: exactly 0.6s of controller updates"
            );
            assert_eq!(
                samples.iter().filter(|sample| sample.dirty).count(),
                usize::from(pending),
                "a newly received role skips only its first complete controller update"
            );
            let mut previous_target = initial_target;
            let mut moving_steps = 0;
            let mut expired_steps = 0;
            for sample in samples {
                let eligible = !sample.dirty
                    && sample.state_age <= 0.5
                    && (speed <= 1.2 || sample.state_age <= 0.2)
                    && sample.hit_age >= 0.4;
                let expected_delta = if eligible {
                    peer_velocity * sample.dt
                } else {
                    Vec3::ZERO
                };
                assert!((sample.target - previous_target - expected_delta).length() < 1e-6,
                    "{fps} FPS speed={speed}: observer prediction must integrate exactly once inside age gates: {sample:?}");
                moving_steps += usize::from(eligible);
                expired_steps +=
                    usize::from(sample.state_age > if speed > 1.2 { 0.2 } else { 0.5 });
                previous_target = sample.target;
            }
            assert!(
                moving_steps > 10 && expired_steps > 5,
                "exercise both live and expired predictions"
            );
            assert!(
                samples[0].peer.x > 1.2,
                "positive control: correction must run"
            );
            assert!(
                samples[1].pose_frame > samples[0].pose_frame,
                "remote authored animation must sample each controller step"
            );
            if let Some(reference) = &reference {
                for (index, (sample, expected)) in samples.iter().zip(reference).enumerate() {
                    assert!((sample.peer - expected.peer).length() < 2e-5
                        && (sample.target - expected.target).length() < 2e-5
                        && (sample.local - expected.local).length() < 2e-5,
                        "{fps} FPS step={index}: observer and contact timelines must match 120 Hz: {sample:?} != {expected:?}");
                    assert_eq!(sample.dirty, expected.dirty);
                    assert!((sample.state_age - expected.state_age).abs() < 2e-6);
                    assert!((sample.hit_age - expected.hit_age).abs() < 2e-6);
                    assert!((sample.pose_frame - expected.pose_frame).abs() < 2e-5);
                }
            } else {
                reference = Some(samples.clone());
            }
        }
    }
}
