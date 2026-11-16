use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct Waypoint {
    pub id: usize,

    pub links: Vec<(usize, f32)>,
}

pub fn find_waypoint_path(points: &[Waypoint], start_id: usize, goal_id: usize) -> Vec<usize> {
    if start_id >= points.len() || goal_id >= points.len() {
        return Vec::new();
    }
    if start_id == goal_id {
        return vec![goal_id];
    }

    let mut distance: HashMap<usize, f32> = HashMap::new();
    let mut previous: HashMap<usize, usize> = HashMap::new();
    let mut open: HashSet<usize> = HashSet::new();
    distance.insert(start_id, 0.0);
    open.insert(start_id);

    while !open.is_empty() {
        let mut current: Option<usize> = None;
        let mut best = f32::INFINITY;
        for &id in &open {
            let value = distance.get(&id).copied().unwrap_or(f32::INFINITY);
            if value < best {
                best = value;
                current = Some(id);
            }
        }
        let current = match current {
            Some(c) => c,
            None => break,
        };
        open.remove(&current);
        if current == goal_id {
            break;
        }
        for (target, edge_distance) in &points[current].links {
            if *target >= points.len() {
                continue;
            }
            let candidate = best + (*edge_distance).max(0.0);
            let existing = distance.get(target).copied().unwrap_or(f32::INFINITY);
            if candidate >= existing {
                continue;
            }
            distance.insert(*target, candidate);
            previous.insert(*target, current);
            open.insert(*target);
        }
    }

    if !previous.contains_key(&goal_id) {
        return Vec::new();
    }
    let mut path = vec![goal_id];
    while path[0] != start_id {
        let prev = previous[&path[0]];
        path.insert(0, prev);
    }
    path[1..].to_vec()
}

pub fn find_original_waypoint_path(
    positions: &[[f32; 3]],
    start: usize,
    goal: usize,
    budget: usize,
    mut links: impl FnMut(usize) -> Vec<(usize, f32)>,
) -> Option<Vec<usize>> {
    if start >= positions.len() || goal >= positions.len() {
        return None;
    }
    #[derive(Clone, Copy)]
    struct Node {
        id: usize,
        g: f32,
        f: f32,
        parent: Option<usize>,
    }

    fn push_up(heap: &mut [usize], nodes: &[Node], mut hole: usize, top: usize, value: usize) {
        while hole > top {
            let parent = (hole - 1) / 2;
            if nodes[heap[parent]].f <= nodes[value].f {
                break;
            }
            heap[hole] = heap[parent];
            hole = parent;
        }
        heap[hole] = value;
    }
    fn adjust(heap: &mut [usize], nodes: &[Node], top: usize, value: usize) {
        let mut hole = top;
        let mut right = 2 * hole + 2;
        while right < heap.len() {
            let child = if nodes[heap[right]].f > nodes[heap[right - 1]].f {
                right - 1
            } else {
                right
            };
            heap[hole] = heap[child];
            hole = child;
            right = 2 * hole + 2;
        }
        if right == heap.len() {
            heap[hole] = heap[right - 1];
            hole = right - 1;
        }
        push_up(heap, nodes, hole, top, value);
    }
    let heuristic = |id: usize| {
        positions[id]
            .iter()
            .zip(positions[goal])
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f32>()
            .sqrt()
    };
    let mut nodes = vec![Node {
        id: start,
        g: 0.0,
        f: heuristic(start),
        parent: None,
    }];
    let mut open = vec![0usize];
    let mut best = vec![None; positions.len()];
    best[start] = Some(0usize);
    let mut current = 0;
    for _ in 0..=budget {
        if open.is_empty() {
            return None;
        }
        current = open[0];
        let tail = open.pop().unwrap();
        if !open.is_empty() {
            adjust(&mut open, &nodes, 0, tail);
        }
        let node = nodes[current];
        if node.id == goal {
            break;
        }
        for (next, cost) in links(node.id) {
            if next >= positions.len() || node.parent.is_some_and(|p| nodes[p].id == next) {
                continue;
            }
            let g = node.g + cost;
            if let Some(old) = best[next] {
                if nodes[old].g <= g {
                    continue;
                }
                if let Some(index) = open.iter().position(|entry| *entry == old) {
                    open.remove(index);
                    if open.len() >= 2 {
                        for parent in (0..=(open.len() - 2) / 2).rev() {
                            let value = open[parent];
                            adjust(&mut open, &nodes, parent, value);
                        }
                    }
                }
            }
            let index = nodes.len();
            nodes.push(Node {
                id: next,
                g,
                f: g + heuristic(next),
                parent: Some(current),
            });
            best[next] = Some(index);
            open.push(index);
            let hole = open.len() - 1;
            push_up(&mut open, &nodes, hole, 0, index);
        }
    }
    let mut path = Vec::new();
    loop {
        path.push(nodes[current].id);
        let Some(parent) = nodes[current].parent else {
            break;
        };
        current = parent;
    }
    path.reverse();
    Some(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BehaviorMode {
    Idle,

    Patrol,

    Follow,

    Attack,

    Defend,

    Wait,

    Run,

    AttackTarget,

    Kill,

    Dodge,

    GetItem,
}

#[derive(Debug, Clone, Copy)]
pub struct BehaviorContext {
    pub target_distance: f32,

    pub attack_range: f32,

    pub hp_ratio: f32,

    pub defend_hp_threshold: f32,

    pub has_target: bool,

    pub has_route: bool,
}

pub fn decide_behavior(ctx: BehaviorContext) -> BehaviorMode {
    if !ctx.has_target {
        return if ctx.has_route {
            BehaviorMode::Patrol
        } else {
            BehaviorMode::Idle
        };
    }
    if ctx.hp_ratio < ctx.defend_hp_threshold {
        return BehaviorMode::Defend;
    }
    if ctx.target_distance <= ctx.attack_range {
        return BehaviorMode::Attack;
    }
    BehaviorMode::Follow
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: usize, links: &[(usize, f32)]) -> Waypoint {
        Waypoint {
            id,
            links: links.to_vec(),
        }
    }

    #[test]
    fn path_same_node() {
        let pts = vec![w(0, &[])];
        assert_eq!(find_waypoint_path(&pts, 0, 0), vec![0]);
    }

    #[test]
    fn path_simple() {
        let pts = vec![
            w(0, &[(1, 1.0)]),
            w(1, &[(0, 1.0), (2, 2.0)]),
            w(2, &[(1, 2.0)]),
        ];
        assert_eq!(find_waypoint_path(&pts, 0, 2), vec![1, 2]);
    }

    #[test]
    fn path_prefers_shorter() {
        let pts = vec![
            w(0, &[(1, 10.0), (2, 1.0)]),
            w(1, &[(0, 10.0), (3, 1.0)]),
            w(2, &[(0, 1.0), (3, 1.0)]),
            w(3, &[(1, 1.0), (2, 1.0)]),
        ];
        let path = find_waypoint_path(&pts, 0, 1);
        assert_eq!(path, vec![2, 3, 1]);
    }

    #[test]
    fn path_no_route() {
        let pts = vec![w(0, &[]), w(1, &[])];
        assert!(find_waypoint_path(&pts, 0, 1).is_empty());
    }

    #[test]
    fn path_invalid_ids() {
        let pts = vec![w(0, &[])];
        assert!(find_waypoint_path(&pts, 0, 99).is_empty());
        assert!(find_waypoint_path(&pts, 99, 0).is_empty());
    }

    fn ctx(
        target_distance: f32,
        hp_ratio: f32,
        has_target: bool,
        has_route: bool,
    ) -> BehaviorContext {
        BehaviorContext {
            target_distance,
            attack_range: 3.0,
            hp_ratio,
            defend_hp_threshold: 0.2,
            has_target,
            has_route,
        }
    }

    #[test]
    fn behavior_no_target_no_route_idles() {
        assert_eq!(
            decide_behavior(ctx(0.0, 1.0, false, false)),
            BehaviorMode::Idle
        );
    }

    #[test]
    fn behavior_no_target_with_route_patrols() {
        assert_eq!(
            decide_behavior(ctx(0.0, 1.0, false, true)),
            BehaviorMode::Patrol
        );
    }

    #[test]
    fn behavior_with_target_far_follows() {
        assert_eq!(
            decide_behavior(ctx(5.0, 1.0, true, false)),
            BehaviorMode::Follow
        );
    }

    #[test]
    fn behavior_low_hp_defends() {
        assert_eq!(
            decide_behavior(ctx(1.0, 0.1, true, false)),
            BehaviorMode::Defend
        );
    }

    #[test]
    fn behavior_in_range_attacks() {
        assert_eq!(
            decide_behavior(ctx(2.0, 0.8, true, false)),
            BehaviorMode::Attack
        );
    }

    #[test]
    fn behavior_far_follows_not_attacks() {
        assert_eq!(
            decide_behavior(ctx(10.0, 0.8, true, false)),
            BehaviorMode::Follow
        );
    }
}

#[cfg(test)]
mod original_path_tests {
    use super::find_original_waypoint_path;

    #[test]
    fn euclidean_heuristic_can_choose_a_different_route_than_dijkstra() {
        let positions = [[0., 0., 0.], [100., 0., 0.], [0., 0., 0.], [0., 0., 0.]];
        let route = find_original_waypoint_path(&positions, 0, 3, 999, |id| match id {
            0 => vec![(1, 1.), (2, 3.)],
            1 => vec![(3, 1.)],
            2 => vec![(3, 1.)],
            _ => vec![],
        });
        assert_eq!(route, Some(vec![0, 2, 3]));
    }

    #[test]
    fn budget_counts_one_more_pop_and_keeps_last_popped_chain() {
        let positions = vec![[0.; 3]; 1002];
        let route =
            find_original_waypoint_path(&positions, 0, 1001, 999, |id| vec![(id + 1, 1.)]).unwrap();
        assert_eq!(route, (0..1000).collect::<Vec<_>>());
        assert_eq!(
            find_original_waypoint_path(&positions, 0, 1, 0, |_| vec![(1, 1.)]),
            Some(vec![0])
        );
        assert_eq!(
            find_original_waypoint_path(&positions, 0, 1, 1, |_| vec![]),
            None
        );
    }

    #[test]
    fn equal_f_heap_uses_right_child_on_pop() {
        let positions = [[0.; 3]; 6];
        let route = find_original_waypoint_path(&positions, 0, 5, 2, |id| match id {
            0 => vec![(1, 1.), (2, 1.), (3, 1.), (4, 1.)],
            _ => vec![],
        });
        assert_eq!(route, Some(vec![0, 3]));
    }

    #[test]
    fn improved_closed_node_is_reopened() {
        let positions = [[0.; 3], [0.; 3], [10., 0., 0.], [0.; 3]];
        let route = find_original_waypoint_path(&positions, 0, 3, 999, |id| match id {
            0 => vec![(1, 5.), (2, 1.)],
            1 => vec![(3, 100.)],
            2 => vec![(1, 1.)],
            _ => vec![],
        });
        assert_eq!(route, Some(vec![0, 2, 1, 3]));
    }
}
