#[cfg(test)]
use super::movement_plugin::{COLLIDER_HEIGHT, COLLIDER_RADIUS};
use super::stage_collider::StageMeshCollider;
use bevy::prelude::*;
use meteor_ai::behavior::find_original_waypoint_path;
use meteor_core::MODEL_SCALE;
#[cfg(test)]
use meteor_physics::collision::CollisionSystem;
use meteor_physics::collision::{MeshCollider, Point3};
#[cfg(test)]
use meteor_physics::jump::STEP_DOWN;
use meteor_scene::stage_loader::WaypointGraph;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Leg {
    position: Vec3,
    id: Option<usize>,
    size: i16,
    jump: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Navigation {
    destination: Option<Vec3>,

    route: Vec<Leg>,
    active: Option<Leg>,
    failed: bool,
    activation_serial: u64,
}

fn ray_clear(mesh: Option<&StageMeshCollider>, from: Vec3, to: Vec3) -> bool {
    let delta = to - from;
    let length = delta.length();
    length == 0.0
        || mesh.is_none_or(|mesh| {
            mesh.raycast_hit(point(from), point(delta / length), length)
                .is_none()
        })
}

pub(super) fn reachable(mesh: Option<&StageMeshCollider>, from: Vec3, to: Vec3) -> bool {
    let delta = to - from;
    let length = delta.length() / MODEL_SCALE;
    let height = delta.y.abs() / MODEL_SCALE;
    if length > 400.0
        || height > 40.0
        || (height > 20.0 && delta.xz().length() <= delta.y.abs())
        || !ray_clear(mesh, from, to)
    {
        return false;
    }
    let Some(mesh) = mesh else {
        return true;
    };
    let samples = (length * 0.025) as usize;
    (1..samples).all(|i| {
        let at = from + delta * (i as f32 / samples as f32);
        let top = at - Vec3::Y * (20.0 * MODEL_SCALE);
        mesh.raycast_hit(point(top), point(Vec3::NEG_Y), 20.0 * MODEL_SCALE)
            .is_some()
    })
}

fn source_position(graph: &WaypointGraph, at: Vec3) -> [f32; 3] {
    [
        at.x / MODEL_SCALE + graph.center[0],
        -at.z / MODEL_SCALE + graph.center[1],
        at.y / MODEL_SCALE + graph.center[2],
    ]
}

fn grid_cell(bounds: [[f32; 3]; 2], at: [f32; 3]) -> Option<[i32; 2]> {
    let mut cell = [0; 2];
    for axis in 0..2 {
        let extent = bounds[1][axis] - bounds[0][axis];
        let count = ((extent * 0.0025) as i32).min(10);
        if count <= 0 {
            return None;
        }
        let width = (extent / count as f32) as i32;
        if width <= 0 {
            return None;
        }
        cell[axis] = (((at[axis] - bounds[0][axis]) as i32) / width).clamp(0, count - 1);
    }
    Some(cell)
}

pub(super) fn nearest_original(
    graph: &WaypointGraph,
    mesh: Option<&StageMeshCollider>,
    at: Vec3,
) -> Option<usize> {
    if graph.points.is_empty() {
        return None;
    }
    let query_world = at + Vec3::Y * (30.0 * MODEL_SCALE);
    let query = Vec3::from_array(source_position(graph, query_world));
    let distance = |id: usize| query.distance_squared(Vec3::from_array(graph.points[id].position));
    let visible = |id| {
        ray_clear(
            mesh,
            query_world,
            Vec3::from_array(graph.world_position(id).unwrap()),
        )
    };
    let mut best = None;
    let mut best_distance = 1.0e11;
    if let Some(bounds) = graph.navigation_bounds {
        if let Some(cell) = grid_cell(bounds, query.to_array()) {
            for p in &graph.points {
                if grid_cell(bounds, p.position) != Some(cell) {
                    continue;
                }
                let height = (p.position[2] - query.z).abs();
                let d2 = distance(p.id);
                if height > 140.0 || d2 >= best_distance || !visible(p.id) {
                    continue;
                }
                best = Some(p.id);
                best_distance = d2;
                if d2 < 1600.0 && height < 10.0 {
                    return best;
                }
            }
        }
    }
    if best.is_some() {
        return best;
    }
    for p in &graph.points {
        let d2 = distance(p.id);
        if d2 < best_distance && (d2 > 90000.0 || visible(p.id)) {
            best = Some(p.id);
            best_distance = d2;
            if d2 < 1600.0 && (p.position[2] - query.z).abs() < 10.0 {
                return best;
            }
        }
    }
    Some(best.unwrap_or(0))
}

impl Navigation {
    pub(super) fn activation_serial(&self) -> u64 {
        self.activation_serial
    }
    pub(super) fn failed(&self) -> bool {
        self.failed
    }
    pub(super) fn in_progress(&self) -> bool {
        self.active.is_some()
    }
    pub(super) fn remaining_waypoints(&self) -> usize {
        self.route.len()
    }
    pub(super) fn steering_target(&self) -> Option<Vec3> {
        self.active.map(|leg| leg.position)
    }
    pub(super) fn goal(&self) -> Option<Vec3> {
        self.destination
    }
    pub(super) fn jump_target(&self) -> Option<Vec3> {
        self.active.filter(|leg| leg.jump).map(|leg| leg.position)
    }
    pub(super) fn stop(&mut self) {
        *self = Self::default();
    }

    fn activate(&mut self, mut leg: Leg, roll: &mut impl FnMut() -> u32) {
        let width = if leg.id.is_none() {
            10
        } else {
            i32::from(leg.size) - 10
        };
        if width > 0 {
            leg.position.x += ((roll() % width as u32) as i32 - width / 2) as f32 * MODEL_SCALE;
            leg.position.z -= ((roll() % width as u32) as i32 - width / 2) as f32 * MODEL_SCALE;
        }
        self.active = Some(leg);
        self.activation_serial = self.activation_serial.wrapping_add(1);
    }

    pub(super) fn request_path(
        &mut self,
        graph: Option<&WaypointGraph>,
        mesh: Option<&StageMeshCollider>,
        from: Vec3,
        target: Vec3,
        mut roll: impl FnMut() -> u32,
    ) -> bool {
        self.stop();
        self.failed = true;
        let Some(graph) = graph.filter(|graph| !graph.points.is_empty()) else {
            return false;
        };
        let from = from + Vec3::Y * (30.0 * MODEL_SCALE);
        let start = nearest_original(graph, mesh, from).unwrap();
        let end = nearest_original(graph, mesh, target).unwrap();
        self.destination = Some(target);
        if reachable(mesh, from, target) {
            self.activate(
                Leg {
                    position: target,
                    id: None,
                    size: 0,
                    jump: false,
                },
                &mut roll,
            );
            self.failed = false;
            return true;
        }
        if start == end {
            return false;
        }
        let positions: Vec<_> = graph.points.iter().map(|p| p.position).collect();
        let Some(mut ids) = find_original_waypoint_path(&positions, start, end, 999, |id| {
            graph.navigation_links(id)
        }) else {
            return false;
        };
        let position = |id| Vec3::from_array(graph.world_position(id).unwrap());
        if ids.len() >= 2 && reachable(mesh, position(ids[ids.len() - 2]), target) {
            ids.pop();
        } else if !reachable(mesh, position(*ids.last().unwrap()), target) {
            self.destination = Some(position(*ids.last().unwrap()));
        }
        if ids.len() >= 2 && reachable(mesh, from, position(ids[1])) {
            ids.remove(0);
        }
        self.route = ids
            .iter()
            .enumerate()
            .map(|(index, &id)| Leg {
                position: position(id),
                id: Some(id),
                size: graph.points[id].size as i16,
                jump: index > 0 && graph.link_types.get(&(ids[index - 1], id)) == Some(&1),
            })
            .rev()
            .collect();
        self.activate(*self.route.last().unwrap(), &mut roll);
        self.failed = false;
        true
    }

    pub(super) fn follow(
        &mut self,
        from: Vec3,
        velocity: Vec3,
        mut roll: impl FnMut() -> u32,
    ) -> (Vec3, bool) {
        let Some(active) = self.active else {
            return (Vec3::ZERO, false);
        };
        let delta = active.position - from;
        let radius = velocity.length() * 0.125;
        if delta.xz().length_squared() < radius * radius {
            if delta.y.abs() >= 60.0 * MODEL_SCALE {
                self.stop();
                self.failed = true;
                return (Vec3::ZERO, false);
            }
            if active.id.is_none() {
                self.stop();
                return (Vec3::ZERO, true);
            }
            let next = self.route.pop().unwrap_or(Leg {
                position: self.destination.unwrap(),
                id: None,
                size: 0,
                jump: false,
            });
            self.activate(next, &mut roll);
        }
        let target = self.active.unwrap().position;
        (
            Vec3::new(target.x - from.x, 0.0, target.z - from.z).normalize_or_zero(),
            false,
        )
    }
}

#[cfg(test)]
pub(super) fn direct_combat_direction(
    mesh: &StageMeshCollider,
    from: Vec3,
    target: Vec3,
) -> Option<Vec3> {
    let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
    mover.set_mesh_collider(mesh);
    walkable(&mover, from, target)
        .then(|| Vec3::new(target.x - from.x, 0.0, target.z - from.z).normalize_or_zero())
}

fn point(v: Vec3) -> Point3 {
    Point3::new(v.x, v.y, v.z)
}

#[cfg(test)]
fn floor(mover: &CollisionSystem<&StageMeshCollider>, at: Vec3) -> Option<Vec3> {
    let hit = mover.sample_ground_hit(at.x, at.z, at.y)?;

    if (hit.y - at.y).abs() > 6.0 {
        return None;
    }
    let grounded = Vec3::new(at.x, hit.y, at.z);
    mover.can_occupy(point(grounded)).then_some(grounded)
}

#[cfg(test)]
fn walkable(mover: &CollisionSystem<&StageMeshCollider>, from: Vec3, to: Vec3) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(23);
    let Some(mut pos) = floor(mover, from) else {
        return false;
    };
    let Some(end) = floor(mover, to) else {
        return false;
    };
    let delta = Vec3::new(end.x - pos.x, 0.0, end.z - pos.z);
    let steps = (delta.length() / 0.6).ceil().max(1.0) as usize;
    let step = delta / steps as f32;
    for _ in 0..steps {
        let (x, z, y) = mover.resolve_xz(point(pos), step.x, step.z, false);
        if (x - step.x).abs() > 0.01 || (z - step.z).abs() > 0.01 {
            return false;
        }
        pos += Vec3::new(x, y, z);
        let Some(hit) = mover.sample_ground_hit(pos.x, pos.z, pos.y) else {
            return false;
        };
        if (hit.y - pos.y).abs() > STEP_DOWN || !mover.can_occupy(Point3::new(pos.x, hit.y, pos.z))
        {
            return false;
        }
        pos.y = hit.y;
    }
    (pos.y - end.y).abs() <= STEP_DOWN
}

#[cfg(test)]
pub(super) fn original_stage_for_test(stage: &str) -> (WaypointGraph, StageMeshCollider) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let center = meteor_scene::stage_loader::get_stage_center(stage, None);
    let mut graph = meteor_scene::stage_loader::parse_waypoint_graph(
        &std::fs::read_to_string(root.join(format!("Level/{stage}.wp"))).unwrap(),
        center,
    );
    let gmdl = meteor_asset::gmdl::decode_gmdl(
        &std::fs::read(root.join(format!("{stage}/{stage}.gmb"))).unwrap(),
    )
    .unwrap();
    graph.navigation_bounds = source_bounds_for_test(&gmdl);
    let mut des = meteor_asset::des::parse_stage_description(
        &std::fs::read_to_string(root.join(format!("{stage}/{stage}.des"))).unwrap(),
    );
    let cob = meteor_asset::cob::parse_cob(
        &std::fs::read(root.join(format!("{stage}/{stage}.cob"))).unwrap(),
    )
    .unwrap();
    let mut mesh = StageMeshCollider::from_gmdl(&gmdl, &des, Some(&cob.face_ids), center);
    use meteor_scene::scene_script_runtime::{SceneItemInstance, SceneScriptRuntime};
    let sources = [
        "PScriptLib/Def.h".to_owned(),
        format!("{stage}/{stage}.pst"),
    ]
    .map(|path| crate::text_codec::decode_text(&std::fs::read(root.join(path)).unwrap()))
    .join("\n");
    let instances = des
        .dummies
        .iter()
        .enumerate()
        .map(|(index, dummy)| SceneItemInstance {
            name: dummy.name.clone(),
            index,
            props: default(),
        })
        .collect();
    let mut scene = SceneScriptRuntime::load(&sources, instances, 250_000).unwrap();
    scene.call("Scene_OnLoad", &[]).unwrap();
    for (index, dummy) in des.dummies.iter_mut().enumerate() {
        if scene.host().instance(index).unwrap().collision_enabled() {
            dummy.collision_attr = Some(1);
        }
    }
    super::render_plugin::append_solid_props_for_test(&root, &des, center, &mut mesh);
    (graph, mesh)
}

#[cfg(test)]
pub(super) fn source_bounds_for_test(gmdl: &meteor_asset::gmdl::GmdlMesh) -> Option<[[f32; 3]; 2]> {
    let mut bounds = [[f32::INFINITY; 3], [f32::NEG_INFINITY; 3]];
    for point in gmdl
        .objects
        .iter()
        .flat_map(|object| object.positions.chunks_exact(3))
    {
        for axis in 0..3 {
            bounds[0][axis] = bounds[0][axis].min(point[axis]);
            bounds[1][axis] = bounds[1][axis].max(point[axis]);
        }
    }
    bounds[0][0].is_finite().then_some(bounds)
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use meteor_scene::stage_loader::WaypointPoint;

    fn graph_at(positions: &[[f32; 3]]) -> WaypointGraph {
        WaypointGraph {
            points: positions
                .iter()
                .enumerate()
                .map(|(id, &position)| WaypointPoint {
                    id,
                    position,
                    size: 10.,
                    links: Vec::new(),
                })
                .collect(),
            ..default()
        }
    }

    fn courtyard() -> (WaypointGraph, StageMeshCollider) {
        let mut mesh = StageMeshCollider::default();
        mesh.append_world_triangles(
            &[
                [-30., 0., -30.],
                [-30., 0., 30.],
                [30., 0., 30.],
                [30., 0., -30.],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
        mesh.append_world_triangles(
            &[[0., 0., -5.], [0., 8., -5.], [0., 8., 5.], [0., 0., 5.]],
            &[0, 1, 2, 0, 2, 3],
        );
        let positions = [
            (1.2, 0.),
            (-5., 0.),
            (-5., 8.),
            (5., 8.),
            (-5., -8.),
            (5., -8.),
        ];
        let edges: &[&[usize]] = &[&[3, 5], &[2, 4], &[1, 3], &[2, 0], &[1, 5], &[4, 0]];
        let mut graph = graph_at(&positions.map(|(x, z)| [x / MODEL_SCALE, -z / MODEL_SCALE, 30.]));
        for id in 0..graph.points.len() {
            graph.points[id].links = edges[id].iter().map(|&next| (next, 100.)).collect();
        }
        (graph, mesh)
    }

    #[test]
    fn no_waypoints_fails_even_when_the_direct_ray_is_clear() {
        let mut nav = Navigation::default();
        for graph in [None, Some(&WaypointGraph::default())] {
            assert!(!nav.request_path(graph, None, Vec3::ZERO, Vec3::X, || 5));
            assert!(nav.failed());
            assert!(!nav.in_progress());
        }
    }

    #[test]
    fn direct_path_is_active_even_with_disabled_waypoints() {
        let (mut graph, mesh) = courtyard();
        for id in 0..graph.points.len() {
            graph.set_enabled(id, false);
        }
        let mut nav = Navigation::default();
        let from = Vec3::new(-5., 0., 3.);
        let target = Vec3::new(-5., 30. * MODEL_SCALE, 6.);
        assert!(nav.request_path(Some(&graph), Some(&mesh), from, target, || 5));
        assert!(nav.in_progress());
        assert_eq!(nav.remaining_waypoints(), 0);
        assert_eq!(nav.steering_target(), Some(target));
        assert!(nav.follow(from, Vec3::ZERO, || 5).0.z > 0.99);
        nav.stop();
        assert!(!nav.in_progress());
        assert_eq!(nav.goal(), None);
    }

    #[test]
    fn nearest_uses_local_cell_before_global_distance_and_ignores_disabled() {
        let mut graph = graph_at(&[[350., 200., 30.], [400., 200., 30.]]);
        graph.navigation_bounds = Some([[0.; 3], [800.; 3]]);
        graph.set_enabled(0, false);
        let query = Vec3::new(399. * MODEL_SCALE, 0., -200. * MODEL_SCALE);
        assert_eq!(nearest_original(&graph, None, query), Some(0));
        graph.navigation_bounds = None;
        assert_eq!(nearest_original(&graph, None, query), Some(1));
    }

    #[test]
    fn nearest_local_early_exit_preserves_authored_point_order() {
        let mut graph = graph_at(&[[390., 200., 30.], [399., 200., 30.]]);
        graph.navigation_bounds = Some([[0.; 3], [800.; 3]]);
        assert_eq!(
            nearest_original(&graph, None, Vec3::new(399. * MODEL_SCALE, 0., -24.)),
            Some(0)
        );
    }

    #[test]
    fn nearest_rejects_near_occluded_point_but_far_scan_bypasses_rays() {
        let (mut graph, mesh) = courtyard();
        let from = Vec3::new(-1.2, 0., 0.);
        assert_eq!(nearest_original(&graph, Some(&mesh), from), Some(1));
        graph.points[1].position = [400., 0., 30.];
        graph.points.truncate(2);
        assert_eq!(nearest_original(&graph, Some(&mesh), from), Some(1));
        graph.points.truncate(1);

        assert_eq!(nearest_original(&graph, Some(&mesh), from), Some(0));
    }

    #[test]
    fn blocked_direct_same_node_fails_without_virtual_graph_exits() {
        let (mut graph, mesh) = courtyard();
        graph.points.truncate(1);
        let mut nav = Navigation::default();
        assert!(!nav.request_path(
            Some(&graph),
            Some(&mesh),
            Vec3::new(-1.2, 0., 0.),
            Vec3::new(1.2, 3.6, 0.),
            || 5
        ));
        assert!(nav.failed());
    }

    #[test]
    fn support_probe_is_twenty_to_forty_units_below_the_segment() {
        let (_, mesh) = courtyard();
        assert!(reachable(
            Some(&mesh),
            Vec3::new(-5., 3.6, -12.),
            Vec3::new(-5., 3.6, 12.)
        ));
        assert!(!reachable(
            Some(&mesh),
            Vec3::new(-5., 6., -12.),
            Vec3::new(-5., 6., 12.)
        ));
        assert!(!reachable(None, Vec3::ZERO, Vec3::X * (401. * MODEL_SCALE)));
        assert!(!reachable(
            None,
            Vec3::ZERO,
            Vec3::new(30. * MODEL_SCALE, 30. * MODEL_SCALE, 0.)
        ));
        assert!(reachable(
            None,
            Vec3::ZERO,
            Vec3::new(31. * MODEL_SCALE, 30. * MODEL_SCALE, 0.)
        ));
    }

    #[test]
    fn inaccessible_terminal_is_truncated_to_the_authored_last_point() {
        let (graph, mesh) = courtyard();
        let mut nav = Navigation::default();
        let target = Vec3::new(1.2, 15.6, 0.);
        assert!(nav.request_path(
            Some(&graph),
            Some(&mesh),
            Vec3::new(-5., 0., 0.),
            target,
            || 5
        ));
        assert_eq!(
            nav.goal(),
            Some(Vec3::from_array(graph.world_position(0).unwrap()))
        );
        assert_ne!(nav.goal(), Some(target));
    }

    #[test]
    fn jitter_uses_signed_size_and_inverts_the_source_y_axis() {
        let mut nav = Navigation::default();
        let mut rolls = [0, 9, 0, 19].into_iter();
        nav.activate(
            Leg {
                position: Vec3::ZERO,
                id: None,
                size: 0,
                jump: false,
            },
            &mut || rolls.next().unwrap(),
        );
        assert_eq!(
            nav.steering_target(),
            Some(Vec3::new(-5. * MODEL_SCALE, 0., -4. * MODEL_SCALE))
        );
        nav.activate(
            Leg {
                position: Vec3::ZERO,
                id: Some(0),
                size: 30,
                jump: false,
            },
            &mut || rolls.next().unwrap(),
        );
        assert_eq!(
            nav.steering_target(),
            Some(Vec3::new(-10. * MODEL_SCALE, 0., -9. * MODEL_SCALE))
        );
        nav.activate(
            Leg {
                position: Vec3::ZERO,
                id: Some(0),
                size: -1,
                jump: false,
            },
            &mut || panic!("negative Size does not draw"),
        );
        assert_eq!(nav.steering_target(), Some(Vec3::ZERO));
    }

    fn active_navigation(target: Vec3) -> Navigation {
        Navigation {
            destination: Some(Vec3::X * 20.),
            active: Some(Leg {
                position: target,
                id: Some(0),
                size: 10,
                jump: true,
            }),
            route: vec![Leg {
                position: Vec3::X * 10.,
                id: Some(1),
                size: 10,
                jump: false,
            }],
            ..default()
        }
    }

    #[test]
    fn ground_and_air_progress_use_strict_speed_and_height_gates() {
        let target = Vec3::new(3., 6., 0.);
        for (velocity, advances) in [
            (Vec3::X * 24., false),
            (Vec3::X * 24.1, true),
            (Vec3::Y * 24.1, true),
        ] {
            let mut nav = active_navigation(target);
            nav.follow(Vec3::ZERO, velocity, || 5);
            assert_eq!(
                nav.steering_target(),
                Some(if advances { Vec3::X * 10. } else { target })
            );
            assert_eq!(nav.jump_target().is_some(), !advances);
        }
        let mut nav = active_navigation(Vec3::Y * (60. * MODEL_SCALE));
        assert_eq!(nav.follow(Vec3::ZERO, Vec3::X, || 5), (Vec3::ZERO, false));
        assert!(nav.failed());
        assert!(!nav.in_progress());
    }

    #[test]
    fn first_waypoint_is_reactivated_once_and_each_frame_consumes_at_most_one() {
        let first = Leg {
            position: Vec3::X,
            id: Some(0),
            size: 10,
            jump: false,
        };
        let mut nav = Navigation {
            destination: Some(Vec3::X * 3.),
            active: Some(first),
            route: vec![first],
            ..default()
        };
        let first_activation = nav.activation_serial();
        nav.follow(Vec3::ZERO, Vec3::X * 80., || 5);
        assert_eq!(nav.activation_serial(), first_activation + 1);
        assert_eq!(nav.steering_target(), Some(Vec3::X));
        assert_eq!(nav.remaining_waypoints(), 0);
        nav.follow(Vec3::ZERO, Vec3::X * 80., || 5);
        assert_eq!(nav.steering_target(), Some(Vec3::X * 3.));
        assert!(nav.in_progress());
        assert!(nav.follow(Vec3::ZERO, Vec3::X * 80., || 5).1);
        assert!(!nav.in_progress());
    }

    #[test]
    fn stalled_path_keeps_its_leg_until_a_behavior_callback_requests_another() {
        let mut nav = active_navigation(Vec3::X * 10.);
        let before = nav.clone();
        for _ in 0..120 {
            nav.follow(Vec3::ZERO, Vec3::ZERO, || panic!("no activation"));
        }
        assert_eq!(nav.active, before.active);
        assert_eq!(nav.route, before.route);
    }

    #[test]
    fn authored_route_moves_around_a_wall_through_the_actual_collision_consumer() {
        let (graph, mesh) = courtyard();
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(&mesh);
        let mut pos = Vec3::new(-1.2, 0., 0.);
        let mut nav = Navigation::default();
        assert!(nav.request_path(
            Some(&graph),
            Some(&mesh),
            pos,
            Vec3::new(1.2, 3.6, 0.),
            || 5
        ));
        for _ in 0..600 {
            let (dir, done) = nav.follow(pos, Vec3::X * 9., || 5);
            if done {
                return;
            }
            let (x, z, y) = mover.resolve_xz(point(pos), dir.x * 0.15, dir.z * 0.15, false);
            pos += Vec3::new(x, y, z);
            assert!(
                pos.x.abs() >= COLLIDER_RADIUS - 0.05 || pos.z.abs() > 5.,
                "must not cross the wall: {pos:?}"
            );
        }
        panic!("did not arrive: {pos:?} {nav:?}");
    }

    #[test]
    fn sn04_messengers_and_sn15_reporters_build_original_authored_routes() {
        for (stage, starts, end) in [
            ("sn04", &[18, 72, 74, 138, 139][..], 1),
            ("sn15", &[114, 115, 5, 2][..], 73),
        ] {
            let (graph, mesh) = original_stage_for_test(stage);
            let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
            mover.set_mesh_collider(&mesh);
            let target = Vec3::from_array(graph.world_position(end).unwrap());
            for &start in starts {
                let from = floor(
                    &mover,
                    Vec3::from_array(graph.world_position(start).unwrap()),
                )
                .expect("authored spawn has support");
                let mut nav = Navigation::default();
                assert!(
                    nav.request_path(Some(&graph), Some(&mesh), from, target, || 5),
                    "{stage} {start}->{end}"
                );
                assert!(nav.in_progress());
            }
        }
    }

    #[test]
    fn sn04_disabled_destinations_fail_without_a_direct_fallback() {
        let (mut graph, mesh) = original_stage_for_test("sn04");
        let from = Vec3::from_array(graph.world_position(72).unwrap());
        let target = Vec3::from_array(graph.world_position(1).unwrap());
        for id in 0..graph.points.len() {
            graph.set_enabled(id, false);
        }
        let mut nav = Navigation::default();
        assert!(!nav.request_path(Some(&graph), Some(&mesh), from, target, || 5));
        assert_eq!(nav.follow(from, Vec3::ZERO, || 5), (Vec3::ZERO, false));
    }
}
