use super::*;
fn sheet() -> topology::Topology {
    let mut t = topology::Topology {
        points: Vec::new(),
        skin_vertices: Vec::new(),
        freedom: Vec::new(),
        triangles: Vec::new(),
        area: vec![0.; 25],
        edges: Vec::new(),
        bends: Vec::new(),
        bindings: Vec::new(),
        normal_vertices: Vec::new(),
        normal_triangles: Vec::new(),
    };
    for y in 0..5 {
        for x in 0..5 {
            t.points.push(Vec3::new(x as f32 * 0.1, 1., y as f32 * 0.1));
            t.freedom.push(if y == 0 { 0. } else { 1. });
        }
    }
    for y in 0..4 {
        for x in 0..4 {
            let a = y * 5 + x;
            t.triangles.push([a, a + 5, a + 1]);
            t.triangles.push([a + 1, a + 5, a + 6]);
        }
    }
    t.constraints();
    t
}
#[test]
fn cloth_dihedral_gradient_matches_finite_differences() {
    let p = [
        Vec3::new(0.1, 0.2, 0.3),
        Vec3::new(1., 0.1, 0.2),
        Vec3::new(0.3, 1., 0.1),
        Vec3::new(0.4, -0.8, 0.5),
    ];
    let (_, grad) = dihedral(p).unwrap();
    for i in 0..4 {
        for axis in 0..3 {
            let mut a = p;
            let mut b = p;
            a[i][axis] += 0.0001;
            b[i][axis] -= 0.0001;
            let numeric = wrap_angle(dihedral(a).unwrap().0 - dihedral(b).unwrap().0) / 0.0002;
            assert!(
                (grad[i][axis] - numeric).abs() < 0.003,
                "{i}/{axis}: {} != {numeric}",
                grad[i][axis]
            );
        }
    }
}
#[test]
fn cloth_triangle_sheet_sags_with_fixed_seam_and_bounded_stretch_at_render_rates() {
    let data = sheet();
    let mut endpoints = Vec::new();
    for fps in [30, 60, 120, 144] {
        let mut sim = Simulation::default();
        sim.reset(&data.points);
        let config = Clothing {
            motion_limit: 0.8,
            ..default()
        };
        for _ in 0..fps * 2 {
            sim.advance(
                &data,
                &data.points,
                &[],
                1. / fps as f32,
                Vec3::ZERO,
                1.,
                &config,
            );
        }
        for i in 0..5 {
            assert_eq!(sim.positions[i], data.points[i]);
        }
        assert!(sim.positions[22].y < 0.8, "cloth must sag under gravity");
        for e in &data.edges {
            let ratio = sim.positions[e.ids[0]].distance(sim.positions[e.ids[1]]) / e.rest;
            assert!(ratio < 1.10 && ratio > 0.85, "stretch ratio {ratio}");
        }
        assert!(sim.positions.iter().all(|p| p.is_finite()));
        endpoints.push(sim.positions[22]);
    }
    for pair in endpoints.windows(2) {
        assert!(pair[0].distance(pair[1]) < 0.002, "{:?}", endpoints);
    }
}
#[test]
fn cloth_hinge_constraint_reduces_fold_error() {
    let mut data = sheet();
    data.points[22].y += 0.2;
    let initial: f32 = data
        .bends
        .iter()
        .filter_map(|b| {
            dihedral(b.ids.map(|i| data.points[i])).map(|(a, _)| wrap_angle(a - b.rest).abs())
        })
        .sum();
    let mut sim = Simulation::default();
    sim.reset(&data.points);
    sim.advance(
        &data,
        &data.points,
        &[],
        1. / 30.,
        Vec3::ZERO,
        1.,
        &Clothing {
            bend_compliance: 0.,
            motion_limit: 0.8,
            iterations: 8,
            ..default()
        },
    );
    let solved: f32 = data
        .bends
        .iter()
        .filter_map(|b| {
            dihedral(b.ids.map(|i| sim.positions[i])).map(|(a, _)| wrap_angle(a - b.rest).abs())
        })
        .sum();
    assert!(solved < initial * 0.5, "{solved} vs {initial}");
}
#[test]
fn cloth_capsules_and_particle_self_contacts_separate_without_moving_pins() {
    let capsule = Capsule {
        a: Vec3::ZERO,
        b: Vec3::Y,
        radius: 0.2,
    };
    let mut p = Vec3::new(0.01, 0.5, 0.);
    project_capsule_limited(&mut p, capsule, 0.01, f32::INFINITY);
    assert!((p.x - 0.21).abs() < 1e-6);
    let mut data = sheet();
    let mut sim = Simulation::default();
    sim.reset(&data.points);
    sim.positions[24] = sim.positions[0] + Vec3::X * 0.001;
    data.freedom[0] = 0.;
    let mut weights = vec![1.; 25];
    weights[0] = 0.;
    let pinned = sim.positions[0];
    sim.collide_self(&data, &weights, 0.01, 1.);
    assert_eq!(sim.positions[0], pinned);
    assert!(sim.positions[24].distance(pinned) >= 0.0199);
}
#[test]
fn scene_cloth_capsules_limit_contact_and_keep_continuing_actor_history() {
    let data = sheet();
    let mut cloth = SceneCloth::new(data.points.clone(), data.freedom, data.triangles);
    let mut world = World::new();
    let actor = world.spawn_empty().id();
    let capsule = Capsule {
        a: Vec3::new(0.2, 0.0, 0.15),
        b: Vec3::new(0.2, 2.0, 0.15),
        radius: 0.3,
    };
    let config = Clothing {
        gravity_scale: 0.0,
        collision_friction: 0.25,
        ..default()
    };
    let dt = 1.0 / 240.0;
    cloth.advance(dt, Vec3::ZERO, &[(actor, capsule)], &config);
    let updated = cloth.advance(dt, Vec3::ZERO, &[(actor, capsule)], &config);
    assert_eq!(updated[..5], data.points[..5]);
    let movement = updated
        .iter()
        .zip(&data.points)
        .map(|(position, rest)| position.distance(*rest))
        .fold(0.0, f32::max);
    assert!(
        (0.07..0.18).contains(&movement),
        "first contact moved {movement}"
    );
    assert!(cloth
        .simulation
        .velocities
        .iter()
        .all(|velocity| velocity.length() < 12.0));
    let moved = Capsule {
        a: capsule.a + Vec3::Z * 0.08,
        b: capsule.b + Vec3::Z * 0.08,
        ..capsule
    };
    let newcomer = world.spawn_empty().id();
    cloth.advance(
        dt,
        Vec3::ZERO,
        &[(actor, moved), (newcomer, capsule)],
        &config,
    );
    assert_eq!(cloth.simulation.capsules[0].a, capsule.a);
    assert_eq!(cloth.simulation.capsules[1].a, capsule.a);
}
#[test]
fn cloth_reset_clears_particle_momentum_and_zero_dt_is_frozen() {
    let data = sheet();
    let mut sim = Simulation::default();
    sim.reset(&data.points);
    sim.advance(
        &data,
        &data.points,
        &[],
        0.1,
        Vec3::new(3., 0., 2.),
        1.,
        &default(),
    );
    assert!(sim.velocities.iter().any(|v| v.length() > 0.01));
    let before = sim.positions.clone();
    sim.advance(&data, &data.points, &[], 0., Vec3::ZERO, 1., &default());
    assert_eq!(sim.positions, before);
    sim.reset(&data.points);
    assert_eq!(sim.positions, data.points);
    assert!(sim.velocities.iter().all(|v| *v == Vec3::ZERO));
}

#[test]
fn cloth_bend_compliance_changes_folding_without_relaxing_stretch() {
    let data = sheet();
    let mut errors = Vec::new();
    for compliance in [0., 1000.] {
        let mut sim = Simulation::default();
        sim.reset(&data.points);

        sim.positions[22].y += 0.12;
        let config = Clothing {
            bend_compliance: compliance,
            iterations: 8,
            motion_limit: 0.8,
            ..default()
        };
        sim.advance(&data, &data.points, &[], 1. / 60., Vec3::ZERO, 1., &config);
        let error: f32 = data
            .bends
            .iter()
            .filter_map(|b| {
                dihedral(b.ids.map(|i| sim.positions[i])).map(|(a, _)| wrap_angle(a - b.rest).abs())
            })
            .sum();
        errors.push(error);
    }
    assert!(
        errors[1] > errors[0] * 2.,
        "flexible material must retain a fold: {errors:?}"
    );
}
