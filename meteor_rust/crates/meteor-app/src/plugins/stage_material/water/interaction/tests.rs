use super::*;

fn surface() -> LiquidSurface {
    LiquidSurface {
        indices: vec![[0, 1, 2]],
        triangles: vec![[
            Vec3::ZERO,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 10.0),
        ]],
        min: Vec3::ZERO,
        max: Vec3::new(10.0, 0.0, 10.0),
        kind: 1.0,
        depth_limit: 4.0,
    }
}
fn sample(feet: Vec3) -> Sample {
    Sample {
        entity: Entity::from_bits(1),
        feet,
        contact: surface().contact(feet).map(|center| Contact {
            surface: Entity::from_bits(2),
            center,
            kind: 1.0,
        }),
    }
}
#[test]
fn contact_uses_triangle_and_height_not_bounds_or_horizontal_fog() {
    let s = surface();
    assert!(s.contact(Vec3::new(1.0, -0.8, 1.0)).is_some());
    assert!(
        s.contact(Vec3::new(8.0, -0.8, 8.0)).is_none(),
        "outside triangle but inside bounds"
    );
    assert!(
        s.contact(Vec3::new(1.0, 2.0, 1.0)).is_none(),
        "bridge above water"
    );
    assert!(
        s.contact(Vec3::new(1.0, -10.0, 1.0)).is_none(),
        "deep below surface"
    );
}
#[test]
fn camera_submersion_uses_the_surface_triangle_and_depth() {
    let water = surface();
    assert_eq!(water.submersion(Vec3::new(1.0, -0.3, 1.0), None), Some(0.3));
    assert_eq!(water.submersion(Vec3::new(1.0, 0.3, 1.0), None), None);
    assert_eq!(water.submersion(Vec3::new(8.0, -0.3, 8.0), None), None);
    assert_eq!(water.submersion(Vec3::new(1.0, -5.0, 1.0), None), None);
    let mut lava = surface();
    lava.kind = 3.0;
    assert_eq!(lava.submersion(Vec3::new(1.0, -0.3, 1.0), None), None);
}

#[test]
fn camera_submersion_tracks_surface_and_setting_in_ecs() {
    let mut app = App::new();
    app.insert_resource(State::new(AppState::Playing))
        .insert_resource(Settings {
            water_enhance: true,
            ..default()
        })
        .insert_resource(Time::<()>::default())
        .init_resource::<FrameUniforms>()
        .init_resource::<LiquidInteractions>()
        .init_resource::<UnderwaterView>()
        .add_systems(Update, update);
    app.world_mut().spawn(surface());
    let camera = app
        .world_mut()
        .spawn((GameplayCamera, Transform::from_xyz(1.0, -0.3, 1.0)))
        .id();
    app.update();
    assert_eq!(
        *app.world().resource::<UnderwaterView>(),
        UnderwaterView {
            kind: 1.0,
            depth: 0.3
        }
    );
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .y = 0.3;
    app.update();
    assert_eq!(
        *app.world().resource::<UnderwaterView>(),
        UnderwaterView::default()
    );
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation
        .y = -0.3;
    app.world_mut().resource_mut::<Settings>().water_enhance = false;
    app.update();
    assert_eq!(
        *app.world().resource::<UnderwaterView>(),
        UnderwaterView::default()
    );
}
#[test]
fn moving_landing_stopping_teleport_and_pool_budget() {
    let mut state = LiquidInteractions::default();
    let mut p = Vec3::new(1.0, -0.8, 1.0);
    state.step(&[sample(p)], 0.016);
    assert!(
        state.ripples.is_empty(),
        "spawn inside water is not a splash"
    );
    p.x += 1.0;
    state.step(&[sample(p)], 0.1);
    assert_eq!(state.ripples.len(), 1);
    let walking_spray = state
        .splashes()
        .next()
        .expect("moving feet disturb water")
        .0
        .particles
        .iter()
        .map(super::super::spray::Particle::volume)
        .sum::<f32>();
    assert!(walking_spray > 0.0);
    for _ in 0..30 {
        state.step(&[sample(p)], 0.1);
    }
    assert!(state.ripples.is_empty(), "standing still must settle");
    p.y = 1.0;
    state.step(&[sample(p)], 0.1);
    p.y = -0.3;
    state.step(&[sample(p)], 0.1);
    assert_eq!(state.ripples.len(), 1);
    assert!(
        state.ripples[0].strength > 0.8,
        "landing stronger than walking"
    );
    assert!(
        state
            .splashes()
            .next()
            .unwrap()
            .0
            .particles
            .iter()
            .map(super::super::spray::Particle::volume)
            .sum::<f32>()
            > walking_spray
    );
    state.step(&[sample(Vec3::new(100.0, 1.0, 1.0))], 0.1);
    let count = state.ripples.len();
    state.step(&[sample(p)], 0.1);
    assert_eq!(state.ripples.len(), count, "teleport back must not splash");
    for _ in 0..30 {
        state.emit(p, 1.0, 0.5);
    }
    assert_eq!(state.ripples.len(), MAX_EVENTS);
    assert_eq!(state.uniforms().counts.x, MAX_RIPPLES as f32);
    state.step(&[], 2.5);
    assert!(state.ripples.is_empty());
    assert!(state.history.is_empty());
}

#[test]
fn animated_liquid_contacts_follow_current_mesh_vertices() {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::PrimitiveTopology;
    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD,
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![[0.0, 1.0, 0.0], [10.0, 1.0, 0.0], [0.0, 1.0, 10.0]],
    );
    let s = surface();
    let feet = Vec3::new(11.0, 0.5, 1.0);
    assert!(s.contact(feet).is_none());
    assert_eq!(
        s.contact_mesh(feet, &mesh, &Transform::from_xyz(10.0, 0.0, 0.0)),
        Some(Vec3::new(11.0, 1.0, 1.0))
    );
}

#[test]
fn ecs_pause_toggle_and_scene_replacement_do_not_leak_ripples() {
    let mut app = App::new();
    app.insert_resource(State::new(AppState::Playing))
        .insert_resource(Settings {
            water_enhance: true,
            ..default()
        })
        .insert_resource(Time::<()>::default())
        .init_resource::<FrameUniforms>()
        .init_resource::<LiquidInteractions>()
        .init_resource::<UnderwaterView>()
        .add_systems(Update, update);
    let water = app.world_mut().spawn(surface()).id();
    let actor = app
        .world_mut()
        .spawn((
            Fighter {
                character_id: 0,
                player_slot: 0,
                weapon: 6,
                weapon_slots: [6, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Transform::from_xyz(1.0, -0.8, 1.0),
        ))
        .id();
    let tick = |app: &mut App| {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.1));
        app.update();
    };
    tick(&mut app);
    app.world_mut()
        .get_mut::<Transform>(actor)
        .unwrap()
        .translation
        .x = 2.0;
    tick(&mut app);
    assert_eq!(
        app.world()
            .resource::<LiquidInteractions>()
            .splashes()
            .count(),
        1,
        "crossing the actual surface must emit a short splash"
    );
    let active = app.world().resource::<FrameUniforms>().liquid_contacts;
    assert_eq!(active.counts.x, 1.0);
    app.insert_resource(State::new(AppState::Paused));
    tick(&mut app);
    assert_eq!(
        app.world().resource::<FrameUniforms>().liquid_contacts,
        active
    );
    app.world_mut().resource_mut::<Settings>().water_enhance = false;
    tick(&mut app);
    assert_eq!(
        app.world()
            .resource::<FrameUniforms>()
            .liquid_contacts
            .counts
            .x,
        0.0
    );
    app.world_mut().resource_mut::<Settings>().water_enhance = true;
    app.insert_resource(State::new(AppState::Playing));
    tick(&mut app);
    assert_eq!(
        app.world()
            .resource::<FrameUniforms>()
            .liquid_contacts
            .counts
            .x,
        0.0
    );
    app.world_mut()
        .get_mut::<Transform>(actor)
        .unwrap()
        .translation
        .x = 3.0;
    tick(&mut app);
    assert_eq!(
        app.world()
            .resource::<FrameUniforms>()
            .liquid_contacts
            .counts
            .x,
        1.0
    );
    app.world_mut().despawn(water);
    app.world_mut().spawn(surface());
    tick(&mut app);
    assert_eq!(
        app.world()
            .resource::<FrameUniforms>()
            .liquid_contacts
            .counts
            .x,
        0.0
    );
    app.insert_resource(State::new(AppState::Menu));
    tick(&mut app);
    assert!(app
        .world()
        .resource::<LiquidInteractions>()
        .history
        .is_empty());
}

#[test]
fn walking_speed_and_descent_reach_the_spray_without_normalization() {
    let mut prior_volume = 0.0;
    for speed in [2.0, 6.0, 12.0, 24.0] {
        let mut state = LiquidInteractions::default();
        let start = Vec3::new(1.0, -0.8, 1.0);
        state.step(&[sample(start)], 1.0 / 60.0);
        for n in 1..100 {
            state.step(
                &[sample(start + Vec3::X * (speed * n as f32 / 60.0))],
                1.0 / 60.0,
            );
            if !state.ripples.is_empty() {
                break;
            }
        }
        let volume: f32 = state
            .splashes()
            .flat_map(|(s, _)| &s.particles)
            .map(super::super::spray::Particle::volume)
            .sum();
        if speed == 2.0 {
            assert_eq!(volume, 0.0, "slow wading only pushes the water");
        } else {
            assert!(
                volume > prior_volume,
                "speed={speed}: {volume} <= {prior_volume}"
            );
        }
        prior_volume = volume;
    }

    let mut state = LiquidInteractions::default();
    for y in [1.8, 0.2, -0.01] {
        state.step(&[sample(Vec3::new(1.0, y, 1.0))], 0.05);
    }
    let spray = state.splashes().next().unwrap().0;
    assert!(spray.particles.iter().any(|p| p.velocity.y > 8.0));
}
