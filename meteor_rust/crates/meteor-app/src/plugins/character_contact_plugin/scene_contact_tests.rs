fn scene_contact_wall() -> StageMeshCollider {
    let mut mesh = StageMeshCollider::default();
    mesh.append_world_triangles(
        &[
            [2.0, 0.0, -10.0],
            [2.0, 10.0, -10.0],
            [2.0, 10.0, 10.0],
            [2.0, 0.0, 10.0],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    mesh
}

#[test]
fn scene_contact_records_real_body_rejection_without_actor_snapshot() {
    let mut world = World::new();
    let actor = world.spawn_empty().id();
    let mut frame = CharacterContactFrame::default();
    let mut stage = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    stage.set_mesh_collider(scene_contact_wall());
    assert!(!frame.last_mover_scene_contact(actor));
    let (dx, _, _, _, blocked, character_changed) = resolve_fighter_xz_with_contacts(
        Some(&mut frame),
        actor,
        &stage,
        Point3::ZERO,
        1.5,
        0.0,
        false,
    );
    assert!((0.0..1.5).contains(&dx));
    assert!(blocked[0]);
    assert!(!character_changed);
    assert!(frame.last_mover_scene_contact(actor));
}

#[test]
fn scene_contact_passes_actors_and_overwrites_previous_wall_hit() {
    let mut world = World::new();
    let actor = world.spawn_empty().id();
    let target = world.spawn_empty().id();
    let mut frame = CharacterContactFrame {
        views: vec![view(actor, 0.0), view(target, 4.0)],
        indices: HashMap::from([(actor, 0), (target, 1)]),
        ..default()
    };
    frame.system.begin_frame(vec![0, 1]);
    let mut wall = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    wall.set_mesh_collider(scene_contact_wall());
    resolve_fighter_xz(
        Some(&mut frame),
        actor,
        &wall,
        Point3::ZERO,
        1.5,
        0.0,
        false,
    );
    assert!(frame.last_mover_scene_contact(actor));

    let clear: CollisionSystem<StageMeshCollider> =
        CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    let (dx, _, _, _, blocked, character_changed) = resolve_fighter_xz_with_contacts(
        Some(&mut frame),
        actor,
        &clear,
        Point3::ZERO,
        3.0,
        0.0,
        false,
    );
    assert_eq!(dx, 3.0);
    assert!(!character_changed);
    assert_eq!(blocked, [false, false]);
    assert!(!frame.last_mover_scene_contact(actor));
}

#[test]
fn scene_contact_excludes_swept_foot_support() {
    let mut world = World::new();
    let actor = world.spawn_empty().id();
    let mut frame = CharacterContactFrame::default();
    let mut floor = StageMeshCollider::default();
    floor.append_world_triangles(
        &[
            [-10.0, 0.6, -10.0],
            [-10.0, 0.6, 10.0],
            [10.0, 0.6, 10.0],
            [10.0, 0.6, -10.0],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    let mut stage = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    stage.set_mesh_collider(floor);
    let (dx, _, dy, _, blocked, _) = resolve_fighter_xz_with_contacts(
        Some(&mut frame),
        actor,
        &stage,
        Point3::ZERO,
        0.5,
        0.0,
        false,
    );
    assert!((dx - 0.5).abs() < 1e-6);
    assert!((dy - 0.6).abs() < 1e-6);
    assert_eq!(blocked, [false, false]);
    assert!(!frame.last_mover_scene_contact(actor));
}

#[test]
fn scene_contact_survives_snapshot_frames_without_mover_and_prunes_despawned_actor() {
    let mut app = App::new();
    app.init_resource::<CharacterContactFrame>()
        .add_systems(Update, begin_character_contact_frame);
    let actor = app
        .world_mut()
        .spawn((
            fighter(0),
            Transform::default(),
            PoseRuntime::idle(),
            SmState::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            Health::default(),
        ))
        .id();
    app.update();
    let mut stage = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    stage.set_mesh_collider(scene_contact_wall());
    resolve_fighter_xz(
        Some(&mut app.world_mut().resource_mut::<CharacterContactFrame>()),
        actor,
        &stage,
        Point3::ZERO,
        1.5,
        0.0,
        false,
    );
    for _ in 0..3 {
        app.update();
        assert!(app
            .world()
            .resource::<CharacterContactFrame>()
            .last_mover_scene_contact(actor));
    }
    app.world_mut().despawn(actor);
    app.update();
    assert!(!app
        .world()
        .resource::<CharacterContactFrame>()
        .last_mover_scene_contact(actor));
}

#[test]
fn scene_contact_wall_mover_records_horizontal_body_bits() {
    let mut world = World::new();
    let actor = world.spawn_empty().id();
    let mut frame = CharacterContactFrame::default();
    let mut stage = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    stage.set_mesh_collider(scene_contact_wall());
    let result = resolve_fighter_wall_motion(
        Some(&mut frame),
        actor,
        &stage,
        Point3::ZERO,
        Point3::new(15.0, 0.0, 0.0),
        Point3::ZERO,
        0.1,
    );
    assert_ne!(result.flags & 5, 0);
    assert!(frame.last_mover_scene_contact(actor));
    let result = resolve_fighter_wall_motion(
        Some(&mut frame),
        actor,
        &stage,
        Point3::ZERO,
        Point3::new(-15.0, 0.0, 0.0),
        Point3::ZERO,
        0.1,
    );
    assert_eq!(result.flags & 5, 0);
    assert!(!frame.last_mover_scene_contact(actor));
}

#[test]
fn scene_contact_repulsion_second_mover_replaces_primary_result() {
    for serial in [false, true] {
        let (mut app, actor) =
            repulsion_app(serial, FighterState::Fake, meteor_core::MovementMode::Idle);
        app.insert_resource(StageCollider {
            mesh: Some(scene_contact_wall()),
        });
        let force = 1.5 * 60.0 * 60.0;
        app.world_mut()
            .resource_mut::<CharacterContactFrame>()
            .authored_repulsion
            .insert(actor, Vec3::X * force);
        app.world_mut().run_schedule(Update);
        assert!(
            app.world()
                .resource::<CharacterContactFrame>()
                .last_mover_scene_contact(actor),
            "serial={serial}"
        );
        app.world_mut()
            .resource_mut::<CharacterContactFrame>()
            .authored_repulsion
            .insert(actor, Vec3::NEG_X * force);
        app.world_mut().run_schedule(Update);
        assert!(
            !app.world()
                .resource::<CharacterContactFrame>()
                .last_mover_scene_contact(actor),
            "serial={serial}"
        );
    }
}
