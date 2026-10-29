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
