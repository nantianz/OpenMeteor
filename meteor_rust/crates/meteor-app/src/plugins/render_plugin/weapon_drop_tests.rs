#![cfg(feature = "original-assets")]
use super::*;
use crate::plugins::item_plugin::StageProp;
use crate::plugins::network_plugin::{AuthorityWeaponDrop, NetState, NetStatus};

fn wall(mesh: &mut StageMeshCollider, z: f32, reversed: bool) -> i32 {
    mesh.append_world_triangles(
        &[
            [-10.0, -10.0, z],
            [10.0, -10.0, z],
            [10.0, 15.0, z],
            [-10.0, 15.0, z],
        ],
        if reversed {
            &[0, 2, 1, 0, 3, 2]
        } else {
            &[0, 1, 2, 0, 2, 3]
        },
    )
}

fn harness(mesh: StageMeshCollider, start: Vec3, velocity: Vec3) -> (App, Entity) {
    let mut app = App::new();
    app.init_resource::<Time>()
        .init_resource::<crate::plugins::FrameClock>()
        .insert_resource(StageCollider { mesh: Some(mesh) })
        .add_systems(
            Update,
            (
                advance_network_weapon_drop_flight,
                advance_dynamic_weapon_drop_pickup_delay,
            )
                .chain(),
        );
    let reference = "D_wpNet1".to_owned();
    let entity = app
        .world_mut()
        .spawn((
            DynamicWeaponDrop {
                reference: reference.clone(),
            },
            DynamicWeaponDropFlight {
                start,
                velocity,
                elapsed: 0.0,
            },
            DynamicWeaponDropPickupDelay::default(),
            Transform::from_translation(start),
            StageProp {
                name: reference,
                model: "w0_0".into(),
                center_offset: Vec3::ZERO,
                half_extents: Vec3::splat(0.5),
                hit_count: 0,
                respawn_timer: 0.0,
                interactive: false,
                collision_enabled: false,
                breakable: false,
                broken: false,
            },
        ))
        .id();
    (app, entity)
}

fn step(app: &mut App, dt: f32) {
    app.world_mut()
        .resource_mut::<crate::plugins::FrameClock>()
        .sim_dt = dt;
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(std::time::Duration::from_secs_f32(dt));
    app.update();
}

#[test]
fn weapon_throw_stops_at_nearest_thin_wall_on_both_sides_in_one_long_frame() {
    for reversed in [false, true] {
        for sign in [-1.0, 1.0] {
            let mut mesh = StageMeshCollider::default();
            wall(&mut mesh, sign * 2.0, reversed);
            wall(&mut mesh, sign, reversed);
            let (mut app, entity) = harness(mesh, Vec3::Y * 6.0, Vec3::Z * (12.0 * sign));
            step(&mut app, 0.25);
            let position = app.world().get::<Transform>(entity).unwrap().translation;
            assert!((position.z - sign).abs() < 0.0001, "{position:?}");
            assert!(
                position.y > 0.0,
                "a wall hit must not be projected onto the floor"
            );
            assert!(app.world().get::<DynamicWeaponDropFlight>(entity).is_none());
            assert!(app.world().get::<StageProp>(entity).unwrap().interactive);
            step(&mut app, 1.0);
            assert_eq!(
                app.world().get::<Transform>(entity).unwrap().translation,
                position
            );
        }
    }
}

#[test]
fn weapon_throw_accepts_wall_at_segment_start_and_ignores_disabled_props() {
    for enabled in [false, true] {
        let mut mesh = StageMeshCollider::default();
        let owner = wall(&mut mesh, 0.0, false);
        mesh.set_owner_enabled(owner, enabled);
        let (mut app, entity) = harness(mesh, Vec3::Y * 6.0, Vec3::Z * 12.0);
        step(&mut app, 0.1);
        let position = app.world().get::<Transform>(entity).unwrap().translation;
        if enabled {
            assert_eq!(position, Vec3::Y * 6.0);
            assert!(app.world().get::<StageProp>(entity).unwrap().interactive);
        } else {
            assert!(position.z > 1.0);
            assert!(app.world().get::<DynamicWeaponDropFlight>(entity).is_some());
            assert!(!app.world().get::<StageProp>(entity).unwrap().interactive);
        }
    }
}

#[test]
fn weapon_throw_hits_sloped_surface_at_swept_intersection() {
    let mut mesh = StageMeshCollider::default();

    mesh.append_world_triangles(
        &[
            [-10.0, -8.0, -10.0],
            [10.0, -8.0, -10.0],
            [10.0, 12.0, 10.0],
            [-10.0, 12.0, 10.0],
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    let (mut app, entity) = harness(mesh, Vec3::Y * 6.0, Vec3::Z * 12.0);
    step(&mut app, 0.3);
    let position = app.world().get::<Transform>(entity).unwrap().translation;
    assert!(
        (position.y - 2.0 - position.z).abs() < 0.0001,
        "{position:?}"
    );
    assert!(position.z < 3.6);
    assert!(app.world().get::<StageProp>(entity).unwrap().interactive);
}

#[test]
fn weapon_throw_without_collision_expires_instead_of_creating_a_flat_floor_pickup() {
    let (mut app, entity) = harness(StageMeshCollider::default(), Vec3::Y * 6.0, Vec3::Z * 12.0);
    step(&mut app, 0.5);
    assert!(!app.world().get::<StageProp>(entity).unwrap().interactive);
    step(&mut app, 4.501);
    assert!(app.world().get_entity(entity).is_err());
}

#[test]
fn network_weapon_wall_contact_holds_position_without_local_pickup_admission() {
    let mut mesh = StageMeshCollider::default();
    wall(&mut mesh, 1.0, false);
    let (mut app, entity) = harness(mesh, Vec3::Y * 6.0, Vec3::Z * 12.0);
    let mut net = NetState {
        status: NetStatus::InRoom,
        ..default()
    };
    net.dynamic_weapon_drops.insert(
        "D_wpNet1".into(),
        AuthorityWeaponDrop {
            reference: "D_wpNet1".into(),
            model: "w0_0".into(),
            position: [0.0, 6.0, 0.0],
            velocity: Some([0.0, 0.0, 12.0]),
            settled: false,
            pickup_ready: false,
        },
    );
    app.insert_resource(net);
    step(&mut app, 0.25);
    let position = app.world().get::<Transform>(entity).unwrap().translation;
    assert!((position.z - 1.0).abs() < 0.0001);
    assert!(app.world().get::<DynamicWeaponDropFlight>(entity).is_none());
    assert!(!app.world().get::<StageProp>(entity).unwrap().interactive);
    let view = &app.world().resource::<NetState>().dynamic_weapon_drops["D_wpNet1"];
    assert_eq!(view.position, position.to_array());
    assert!(view.settled);
    assert!(!view.pickup_ready);
}

#[test]
fn original_sn01_and_sn08_walls_stop_weapon_flight() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    for stage in ["sn01", "sn08"] {
        let folder = root.join(stage);
        let gmdl = meteor_asset::gmdl::decode_gmdl(
            &std::fs::read(folder.join(format!("{stage}.gmb"))).unwrap(),
        )
        .unwrap();
        let desc = meteor_asset::des::parse_stage_description(&crate::text_codec::decode_text(
            &std::fs::read(folder.join(format!("{stage}.des"))).unwrap(),
        ));
        let grid = meteor_asset::cob::parse_cob(
            &std::fs::read(folder.join(format!("{stage}.cob"))).unwrap(),
        )
        .unwrap();
        let mesh = StageMeshCollider::from_gmdl(
            &gmdl,
            &desc,
            Some(&grid.face_ids),
            meteor_scene::stage_loader::get_stage_center(stage, None),
        );
        let walls: Vec<_> = mesh
            .wall_tris
            .iter()
            .filter_map(|index| {
                let i = *index as usize * 3;
                let [a, b, c] = std::array::from_fn::<_, 3, _>(|j| {
                    Vec3::from_array(mesh.positions[mesh.indices[i + j] as usize])
                });
                let cross = (b - a).cross(c - a);
                let normal = cross.normalize_or_zero();
                (cross.length() > 1.0 && normal.y.abs() < 0.05)
                    .then_some(((a + b + c) / 3.0, normal))
            })
            .take(8)
            .collect();
        assert_eq!(walls.len(), 8, "{stage} has real wall fixtures");
        for (center, normal) in walls {
            let start = center + normal * 0.15;
            let velocity = -Vec3::new(normal.x, 0.0, normal.z).normalize() * 12.0;
            let (mut app, entity) = harness(mesh.clone(), start, velocity);
            step(&mut app, 0.025);
            let actual = app.world().get::<Transform>(entity).unwrap().translation;
            assert!(
                app.world().get::<DynamicWeaponDropFlight>(entity).is_none(),
                "{stage}: {center:?}"
            );
            assert!(
                (actual - center).dot(normal) >= -0.001,
                "{stage}: crossed wall: {actual:?}"
            );
        }
    }
}
