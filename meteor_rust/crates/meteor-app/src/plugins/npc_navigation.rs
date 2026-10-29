use super::movement_plugin::{COLLIDER_HEIGHT, COLLIDER_RADIUS};
use super::stage_collider::StageMeshCollider;
use bevy::prelude::*;
use meteor_physics::collision::{CollisionSystem, Point3};
use meteor_physics::jump::STEP_DOWN;
use meteor_scene::stage_loader::WaypointGraph;

const ARRIVE: f32 = 1.2;
const RETRY: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Leg {
    position: Vec3,
    jump: bool,
}

impl Leg {
    fn walk(position: Vec3) -> Self {
        Self {
            position,
            jump: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Navigation {
    goal: Option<usize>,
    destination: Option<Vec3>,
    route: Vec<Leg>,
    planned: bool,
    retry: f32,
    progress_origin: Option<Vec3>,
    progress_time: f32,
}

fn point(v: Vec3) -> Point3 {
    Point3::new(v.x, v.y, v.z)
}

fn floor(mover: &CollisionSystem<&StageMeshCollider>, at: Vec3) -> Option<Vec3> {
    let hit = mover.sample_ground_hit(at.x, at.z, at.y)?;

    if (hit.y - at.y).abs() > 6.0 {
        return None;
    }
    let grounded = Vec3::new(at.x, hit.y, at.z);
    mover.can_occupy(point(grounded)).then_some(grounded)
}

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

fn nearest(graph: &WaypointGraph, at: Vec3) -> Option<usize> {
    graph
        .points
        .iter()
        .filter(|p| graph.is_enabled(p.id))
        .min_by(|a, b| {
            let distance =
                |id| Vec3::from_array(graph.world_position(id).unwrap()).distance_squared(at);
            distance(a.id)
                .total_cmp(&distance(b.id))
                .then(a.id.cmp(&b.id))
        })
        .map(|p| p.id)
}

fn plan(
    graph: &WaypointGraph,
    mover: &CollisionSystem<&StageMeshCollider>,
    from: Vec3,
    target: Vec3,
    goal: usize,
) -> Vec<Leg> {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(22);
    #[cfg(not(target_arch = "wasm32"))]
    let start_scan_perf = super::perf_plugin::Scope::new(24);
    let position = |id| Vec3::from_array(graph.world_position(id).unwrap());
    let mut starts: Vec<_> = graph
        .points
        .iter()
        .filter(|p| graph.is_enabled(p.id))
        .map(|p| p.id)
        .collect();
    starts.sort_by(|a, b| {
        position(*a)
            .distance_squared(from)
            .total_cmp(&position(*b).distance_squared(from))
            .then(a.cmp(b))
    });
    let Some(start) = starts
        .into_iter()
        .find(|id| walkable(mover, from, position(*id)))
    else {
        return Vec::new();
    };
    #[cfg(not(target_arch = "wasm32"))]
    drop(start_scan_perf);

    if !walkable(mover, position(goal), target) {
        return Vec::new();
    }
    let ids = validated_route(graph, start, goal, |a, b| {
        if graph.link_types.get(&(a, b)) == Some(&1) {
            floor(mover, position(a)).is_some() && floor(mover, position(b)).is_some()
        } else {
            walkable(mover, position(a), position(b))
        }
    });
    if ids.is_empty() {
        return Vec::new();
    }
    let mut route = vec![Leg::walk(position(ids[0]))];
    route.extend(ids.windows(2).map(|edge| Leg {
        position: position(edge[1]),
        jump: graph.link_types.get(&(edge[0], edge[1])) == Some(&1),
    }));
    route.push(Leg::walk(target));
    route
}

fn validated_route(
    graph: &WaypointGraph,
    start: usize,
    goal: usize,
    mut traversable: impl FnMut(usize, usize) -> bool,
) -> Vec<usize> {
    let mut checked = std::collections::HashMap::new();
    loop {
        let mut distance = vec![f32::INFINITY; graph.points.len()];
        let mut previous = vec![None; graph.points.len()];
        let mut closed = vec![false; graph.points.len()];
        distance[start] = 0.0;
        loop {
            let current = (0..distance.len())
                .filter(|i| !closed[*i] && distance[*i].is_finite())
                .min_by(|a, b| distance[*a].total_cmp(&distance[*b]).then(a.cmp(b)));
            let Some(current) = current else {
                return Vec::new();
            };
            if current == goal {
                break;
            }
            closed[current] = true;
            for (next, weight) in graph.navigation_links(current) {
                if checked.get(&(current, next)) == Some(&false) {
                    continue;
                }
                let cost = distance[current] + weight;
                if cost < distance[next] {
                    distance[next] = cost;
                    previous[next] = Some(current);
                }
            }
        }
        let mut ids = vec![goal];
        while *ids.last().unwrap() != start {
            ids.push(previous[*ids.last().unwrap()].unwrap());
        }
        ids.reverse();
        let valid = ids.windows(2).all(|edge| {
            *checked
                .entry((edge[0], edge[1]))
                .or_insert_with(|| traversable(edge[0], edge[1]))
        });
        if valid {
            return ids;
        }
    }
}

impl Navigation {
    pub(super) fn jump_target(&self) -> Option<Vec3> {
        self.route
            .first()
            .filter(|leg| leg.jump)
            .map(|leg| leg.position)
    }

    pub(super) fn step(
        &mut self,
        graph: Option<&WaypointGraph>,
        mesh: &StageMeshCollider,
        from: Vec3,
        target: Vec3,
        dt: f32,
    ) -> (Vec3, bool) {
        #[cfg(not(target_arch = "wasm32"))]
        let _perf = super::perf_plugin::Scope::new(21);
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(mesh);
        let goal = graph.and_then(|g| nearest(g, target));
        let dt = dt.clamp(0.0, 0.1);
        self.retry = (self.retry - dt).max(0.0);
        let moved_target = self
            .destination
            .is_none_or(|old| old.distance_squared(target) > ARRIVE * ARRIVE);

        if self.goal != goal || (moved_target && self.route.len() <= 1) {
            self.planned = false;
            self.retry = 0.0;
        }
        if !self.route.is_empty() {
            let origin = self.progress_origin.get_or_insert(from);
            if origin.distance_squared(from) >= 0.25 {
                *origin = from;
                self.progress_time = 0.0;
            } else {
                self.progress_time += dt;
            }
            if self.progress_time >= RETRY {
                self.planned = false;
                self.retry = 0.0;
            }
        }
        if !self.planned && self.retry <= 0.0 {
            self.goal = goal;
            self.destination = Some(target);
            self.route = match (graph, goal) {
                (Some(g), Some(goal)) => plan(g, &mover, from, target, goal),
                (Some(g), None) if !g.disabled.is_empty() => Vec::new(),
                _ if walkable(&mover, from, target) => vec![Leg::walk(target)],
                _ => Vec::new(),
            };
            self.planned = true;
            self.retry = RETRY;
            self.progress_origin = Some(from);
            self.progress_time = 0.0;
        }
        while let Some(next) = self.route.first() {
            let flat = Vec3::new(next.position.x - from.x, 0.0, next.position.z - from.z);
            if flat.length() > ARRIVE
                || floor(&mover, next.position).is_none_or(|end| (end.y - from.y).abs() > STEP_DOWN)
            {
                break;
            }

            if let Some(after) = self.route.get(1) {
                if !after.jump && !walkable(&mover, from, after.position) {
                    break;
                }
            }
            self.route.remove(0);
        }
        if moved_target && self.route.len() <= 1 {
            self.planned = false;
        }
        if let Some(next) = self.route.first() {
            (
                Vec3::new(next.position.x - from.x, 0.0, next.position.z - from.z)
                    .normalize_or_zero(),
                false,
            )
        } else {
            let flat = Vec3::new(target.x - from.x, 0.0, target.z - from.z);
            let arrived = flat.length() <= ARRIVE
                && floor(&mover, target).is_some_and(|end| (end.y - from.y).abs() <= STEP_DOWN);
            if !arrived {
                self.planned = false;
            }
            (Vec3::ZERO, arrived)
        }
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

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
        let points = positions
            .iter()
            .enumerate()
            .map(|(id, &(x, z))| meteor_scene::stage_loader::WaypointPoint {
                id,
                position: [
                    x / meteor_core::MODEL_SCALE,
                    -z / meteor_core::MODEL_SCALE,
                    0.,
                ],
                size: 1.,
                links: edges[id].iter().map(|&next| (next, 1.)).collect(),
            })
            .collect();
        (
            WaypointGraph {
                points,
                ..default()
            },
            mesh,
        )
    }

    #[test]
    fn lazy_validation_only_probes_candidate_route_and_replans_blocked_edges() {
        let (graph, _) = courtyard();
        let mut probes = Vec::new();
        let route = validated_route(&graph, 1, 0, |a, b| {
            probes.push((a, b));
            true
        });
        assert_eq!(route, vec![1, 2, 3, 0]);
        assert_eq!(probes, vec![(1, 2), (2, 3), (3, 0)]);

        let mut probes = std::collections::HashSet::new();
        let detour = validated_route(&graph, 1, 0, |a, b| {
            assert!(
                probes.insert((a, b)),
                "an edge must be probed at most once per plan"
            );
            (a, b) != (2, 3)
        });
        assert_eq!(detour, vec![1, 4, 5, 0]);

        assert_eq!(validated_route(&graph, 1, 0, |_, _| true), route);
        assert!(validated_route(&graph, 1, 0, |_, b| b != 0).is_empty());
    }

    #[test]
    fn lazy_validation_keeps_directed_edges_and_zero_length_routes() {
        let (graph, _) = courtyard();
        assert_eq!(
            validated_route(&graph, 0, 0, |_, _| panic!("no edge needed")),
            vec![0]
        );
        assert_eq!(
            validated_route(&graph, 0, 1, |a, b| (a, b) != (2, 3)),
            vec![0, 3, 2, 1]
        );
    }

    #[test]
    fn nearest_waypoint_across_wall_uses_reachable_graph_entry() {
        let (graph, mesh) = courtyard();
        let mut nav = Navigation::default();
        let from = Vec3::new(-1.2, 0., 0.);
        let target = Vec3::new(1.2, 0., 0.);
        assert_eq!(graph.nearest(from.to_array()), Some(0));
        let (direction, arrived) = nav.step(Some(&graph), &mesh, from, target, 1. / 60.);
        assert!(!arrived);
        assert!(
            direction.x < -0.99,
            "must approach reachable entry before following links: {direction:?}"
        );
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(&mesh);
        let mut pos = from;
        for _ in 0..600 {
            let (dir, done) = nav.step(Some(&graph), &mesh, pos, target, 1. / 60.);
            if done {
                return;
            }
            let (x, z, y) = mover.resolve_xz(point(pos), dir.x * 0.15, dir.z * 0.15, false);
            pos += Vec3::new(x, y, z);
        }
        panic!("failed to go around wall: {pos:?} {:?}", nav.route);
    }

    #[test]
    fn stalled_cached_route_replans_around_new_obstacle() {
        let (graph, mut mesh) = courtyard();
        let mut nav = Navigation::default();
        let from = Vec3::new(-5., 0., 0.);
        let target = Vec3::new(1.2, 0., 0.);
        assert!(nav.step(Some(&graph), &mesh, from, target, 1. / 60.).0.z > 0.99);
        mesh.append_world_triangles(
            &[[-10., 0., 4.], [-10., 8., 4.], [-1., 8., 4.], [-1., 0., 4.]],
            &[0, 1, 2, 0, 2, 3],
        );

        let stopped = Vec3::new(-5., 0., 3.);
        let mut direction = Vec3::ZERO;
        for _ in 0..40 {
            direction = nav.step(Some(&graph), &mesh, stopped, target, 1. / 60.).0;
        }
        assert!(
            direction.z < -0.99,
            "must return to the lower route: {direction:?} {:?}",
            nav.route
        );
    }

    #[test]
    fn unreachable_route_stops_and_retries_after_obstacle_removed() {
        let (mut graph, mut mesh) = courtyard();
        graph.set_enabled(4, false);
        graph.set_enabled(5, false);
        let wall = mesh.append_world_triangles(
            &[[-10., 0., 4.], [-10., 8., 4.], [-1., 8., 4.], [-1., 0., 4.]],
            &[0, 1, 2, 0, 2, 3],
        );
        let from = Vec3::new(-5., 0., 0.);
        let target = Vec3::new(1.2, 0., 0.);
        let mut nav = Navigation::default();
        assert_eq!(
            nav.step(Some(&graph), &mesh, from, target, 1. / 60.).0,
            Vec3::ZERO
        );
        mesh.set_owner_enabled(wall, false);
        let mut direction = Vec3::ZERO;
        for _ in 0..40 {
            direction = nav.step(Some(&graph), &mesh, from, target, 1. / 60.).0;
        }
        assert!(direction.z > 0.99);
    }

    #[test]
    fn moving_target_in_same_region_keeps_intermediate_route() {
        let (graph, mesh) = courtyard();
        let mut nav = Navigation::default();
        let from = Vec3::new(-5., 0., 0.);
        let target = Vec3::new(1.2, 0., 0.);
        nav.step(Some(&graph), &mesh, from, target, 1. / 60.);
        let old_route = nav.route.clone();
        let moved_target = target + Vec3::new(0., 0., 2.);
        assert_eq!(nearest(&graph, target), nearest(&graph, moved_target));
        nav.step(
            Some(&graph),
            &mesh,
            from + Vec3::Z * 0.2,
            moved_target,
            1. / 60.,
        );
        assert_eq!(nav.route, old_route);
    }

    #[test]
    fn vertically_stacked_waypoints_select_correct_floor() {
        let (mut graph, _) = courtyard();
        let mut upper = graph.points[0].clone();
        upper.id = graph.points.len();
        upper.position[2] = 12. / meteor_core::MODEL_SCALE;
        graph.points.push(upper);
        assert_eq!(nearest(&graph, Vec3::new(1.2, 12., 0.)), Some(6));
        assert_eq!(nearest(&graph, Vec3::new(1.2, 0., 0.)), Some(0));
    }

    fn sn04() -> (WaypointGraph, StageMeshCollider) {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let center = meteor_scene::stage_loader::get_stage_center("sn04", None);
        let graph = meteor_scene::stage_loader::parse_waypoint_graph(
            &std::fs::read_to_string(root.join("Level/sn04.wp")).unwrap(),
            center,
        );
        let gmdl =
            meteor_asset::gmdl::decode_gmdl(&std::fs::read(root.join("sn04/sn04.gmb")).unwrap())
                .unwrap();
        let des = meteor_asset::des::parse_stage_description(
            &std::fs::read_to_string(root.join("sn04/sn04.des")).unwrap(),
        );
        let cob = meteor_asset::cob::parse_cob(&std::fs::read(root.join("sn04/sn04.cob")).unwrap())
            .unwrap();
        (
            graph,
            StageMeshCollider::from_gmdl(&gmdl, &des, Some(&cob.face_ids), center),
        )
    }

    #[test]
    fn sn04_messengers_reach_waypoint_one_using_actual_mover() {
        let (graph, mesh) = sn04();
        let mut mover = CollisionSystem::new(COLLIDER_RADIUS, COLLIDER_HEIGHT);
        mover.set_mesh_collider(&mesh);
        let target = Vec3::from_array(graph.world_position(1).unwrap());
        for start in [18, 72, 74, 138, 139] {
            let mut pos = floor(
                &mover,
                Vec3::from_array(graph.world_position(start).unwrap()),
            )
            .unwrap();
            let mut nav = Navigation::default();
            let mut arrived = false;
            for _ in 0..3600 {
                let (dir, done) = nav.step(Some(&graph), &mesh, pos, target, 1.0 / 60.0);
                if done {
                    arrived = true;
                    break;
                }
                let (x, z, y) = mover.resolve_xz(point(pos), dir.x * 0.2, dir.z * 0.2, false);
                pos += Vec3::new(x, y, z);
                if let Some(hit) = mover.sample_ground_hit(pos.x, pos.z, pos.y) {
                    if (pos.y - hit.y).abs() <= STEP_DOWN
                        && mover.can_occupy(Point3::new(pos.x, hit.y, pos.z))
                    {
                        pos.y = hit.y;
                    }
                }
            }
            assert!(
                arrived,
                "start={start} stopped at {pos:?}, route={:?}",
                nav.route
            );
        }
    }

    #[test]
    fn sn04_disabled_graph_does_not_fall_back_through_walls() {
        let (mut graph, mesh) = sn04();
        let from = Vec3::from_array(graph.world_position(72).unwrap());
        let target = Vec3::from_array(graph.world_position(1).unwrap());
        for id in 0..graph.points.len() {
            graph.set_enabled(id, false);
        }
        let mut nav = Navigation::default();
        for _ in 0..120 {
            let (direction, arrived) = nav.step(Some(&graph), &mesh, from, target, 1.0 / 60.0);
            assert_eq!(direction, Vec3::ZERO);
            assert!(!arrived);
        }
    }
}
