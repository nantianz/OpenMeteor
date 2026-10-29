use crate::locomotion::is_dodge_pose_id;
use meteor_core::MODEL_SCALE;

pub const CHARACTER_CONTACT_RADIUS: f32 = 8.0 * MODEL_SCALE;

pub const CHARACTER_CONTACT_HEIGHT: f32 = 32.0 * MODEL_SCALE;

pub const CHARACTER_PUSH_SPEED: f32 = 20.0 * MODEL_SCALE;

pub const CHARACTER_GROUND_CONTACT_DISTANCE: f32 = 4.0 * MODEL_SCALE;

pub const CHARACTER_CONTACT_MAX_SPEED: f32 = 500.0 * MODEL_SCALE;

pub const CONTACT_EPSILON: f32 = 1e-5;

pub const MAX_SLIDE_ITERATIONS: usize = 4;

pub fn authored_repulsion_force(delta: (f32, f32, f32), candidate_distance: f32) -> (f32, f32) {
    let (x, y, z) = delta;
    if !x.is_finite()
        || !y.is_finite()
        || !z.is_finite()
        || !candidate_distance.is_finite()
        || candidate_distance < 0.0
        || candidate_distance >= 24.0 * MODEL_SCALE
    {
        return (0.0, 0.0);
    }
    let d2 = (x * x + z * z) / (MODEL_SCALE * MODEL_SCALE);
    if !(2.0..625.0).contains(&d2) {
        return (0.0, 0.0);
    }
    let coefficient = 300.0 * (576.0 / d2 - 1.0) / d2.sqrt();
    (x * coefficient, z * coefficient)
}

pub const IGNORE_CHARACTER_COLLISION_POSES: &[u16] = &[
    100, 118, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 163, 180, 203, 204, 226,
    235, 252, 256, 259, 265, 282, 283, 292, 298, 312, 313, 314, 318, 325, 326, 327, 331, 332, 333,
    334, 338, 339, 340, 341, 342, 405, 406, 410, 414, 415, 425, 426, 427, 428, 429, 434, 446, 458,
    459, 465, 470, 478, 479, 483, 484, 488, 530, 531, 532, 533, 556, 558, 559, 562,
];

pub fn ignores_character_collision(pose_id: u16, explicit_ignore: bool) -> bool {
    if explicit_ignore {
        return true;
    }
    is_dodge_pose_id(pose_id) || IGNORE_CHARACTER_COLLISION_POSES.contains(&pose_id)
}

pub trait CharacterView {
    fn position(&self) -> (f32, f32, f32);
    fn pose_id(&self) -> u16;
    fn in_guard(&self) -> bool;
    fn airborne(&self) -> bool;
    fn visible(&self) -> bool;
    fn alive(&self) -> bool;
    fn explicit_ignore(&self) -> bool;

    fn ground_y(&self) -> f32 {
        0.0
    }

    fn pushable(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy)]
struct ContactHit {
    target: usize,
    nx: f32,
    nz: f32,
    time: f32,
    inside: bool,
}

#[derive(Debug, Clone)]
pub struct CharacterContactSystem {
    pub fighters: Vec<usize>,

    pending: std::collections::HashMap<usize, (f32, f32)>,

    deferred: std::collections::HashMap<usize, (f32, f32)>,

    deferred_sources: std::collections::HashMap<usize, Vec<usize>>,

    contact_sources: std::collections::HashMap<usize, Vec<usize>>,

    pub contact_velocities: std::collections::HashMap<usize, (f32, f32)>,
    finishing: bool,

    processed_targets: std::collections::HashSet<usize>,
}

impl CharacterContactSystem {
    pub fn new() -> Self {
        Self {
            fighters: Vec::new(),
            pending: std::collections::HashMap::new(),
            deferred: std::collections::HashMap::new(),
            deferred_sources: std::collections::HashMap::new(),
            contact_sources: std::collections::HashMap::new(),
            contact_velocities: std::collections::HashMap::new(),
            finishing: false,
            processed_targets: std::collections::HashSet::new(),
        }
    }

    pub fn begin_frame(&mut self, fighters: Vec<usize>) {
        self.fighters = fighters;
        self.pending = std::mem::take(&mut self.deferred);
        self.contact_sources = std::mem::take(&mut self.deferred_sources);
        self.contact_velocities.clear();
        self.finishing = false;
        self.processed_targets.clear();
    }

    pub fn remap_deferred_targets(&mut self, mut remap: impl FnMut(usize) -> Option<usize>) {
        let old = std::mem::take(&mut self.deferred);
        for (index, velocity) in old {
            if let Some(target) = remap(index) {
                let entry = self.deferred.entry(target).or_insert((0.0, 0.0));
                entry.0 += velocity.0;
                entry.1 += velocity.1;
            }
        }
        for (index, sources) in std::mem::take(&mut self.deferred_sources) {
            if let Some(target) = remap(index) {
                let sources = sources.into_iter().filter_map(&mut remap).collect();
                self.deferred_sources.insert(target, sources);
            }
        }
    }

    pub fn resolve_xz<V: CharacterView>(
        &mut self,
        mover: usize,
        pos: (f32, f32),
        delta: (f32, f32),
        views: &[V],
    ) -> (f32, f32, usize) {
        let mover_view = views.get(mover);
        let mover_active = mover_view.is_some_and(|view| view.visible() && view.alive());
        let ignored = ignores_character_collision(
            mover_view.map(|v| v.pose_id()).unwrap_or(0),
            mover_view.map(|v| v.explicit_ignore()).unwrap_or(false),
        );
        if self.fighters.len() < 2
            || !mover_active
            || ignored
            || (delta.0.abs() < CONTACT_EPSILON && delta.1.abs() < CONTACT_EPSILON)
        {
            return (delta.0, delta.1, 0);
        }

        let start = pos;
        let mut current = pos;
        let mut remaining = delta;
        let mut contacts = 0usize;

        for _ in 0..MAX_SLIDE_ITERATIONS {
            if remaining.0.abs() < CONTACT_EPSILON && remaining.1.abs() < CONTACT_EPSILON {
                break;
            }
            let hit = self.first_contact(mover, current, remaining, views);
            let Some(hit) = hit else {
                current.0 += remaining.0;
                current.1 += remaining.1;
                break;
            };

            if !hit.inside {
                let travel = (hit.time - CONTACT_EPSILON).max(0.0);
                current.0 += remaining.0 * travel;
                current.1 += remaining.1 * travel;
                remaining = (
                    remaining.0 * (1.0 - hit.time),
                    remaining.1 * (1.0 - hit.time),
                );
            }

            let inward = remaining.0 * hit.nx + remaining.1 * hit.nz;
            if inward > 0.0 {
                self.queue_push(mover, hit.target, hit.nx, hit.nz, views);

                remaining = (remaining.0 - hit.nx * inward, remaining.1 - hit.nz * inward);
            } else {
                break;
            }
            contacts += 1;
        }

        (current.0 - start.0, current.1 - start.1, contacts)
    }

    fn first_contact<V: CharacterView>(
        &self,
        mover: usize,
        current: (f32, f32),
        delta: (f32, f32),
        views: &[V],
    ) -> Option<ContactHit> {
        let mover_y = views.get(mover)?.position().1;
        let combined_radius = CHARACTER_CONTACT_RADIUS * 2.0;
        let combined_radius_sq = combined_radius * combined_radius;
        let a = delta.0 * delta.0 + delta.1 * delta.1;
        let mut best: Option<ContactHit> = None;

        for &target in &self.fighters {
            if target == mover {
                continue;
            }
            let tv = views.get(target)?;
            if !tv.visible() || !tv.alive() {
                continue;
            }
            if ignores_character_collision(tv.pose_id(), tv.explicit_ignore()) {
                continue;
            }
            let (tx, ty, tz) = tv.position();
            if (ty - mover_y).abs() >= CHARACTER_CONTACT_HEIGHT {
                continue;
            }

            let rx = current.0 - tx;
            let rz = current.1 - tz;
            let distance_sq = rx * rx + rz * rz;
            if distance_sq < combined_radius_sq - CONTACT_EPSILON {
                let distance = distance_sq.max(0.0).sqrt();

                if distance <= CONTACT_EPSILON {
                    continue;
                }
                let (nx, nz) = ((tx - current.0) / distance, (tz - current.1) / distance);
                if delta.0 * nx + delta.1 * nz <= CONTACT_EPSILON * a.sqrt() {
                    continue;
                }
                return Some(ContactHit {
                    target,
                    nx,
                    nz,
                    time: 0.0,
                    inside: true,
                });
            }

            if a <= CONTACT_EPSILON * CONTACT_EPSILON {
                continue;
            }
            let b = 2.0 * (rx * delta.0 + rz * delta.1);
            if b >= 0.0 {
                continue;
            }
            let c = distance_sq - combined_radius_sq;
            let discriminant = b * b - 4.0 * a * c;
            if discriminant < 0.0 {
                continue;
            }

            let time = if c <= 0.0 {
                0.0
            } else {
                (2.0 * c) / (-b + discriminant.sqrt())
            };
            if !(0.0..=1.0).contains(&time) {
                continue;
            }
            if let Some(best_hit) = &best {
                if time >= best_hit.time {
                    continue;
                }
            }

            let contact_x = current.0 + delta.0 * time;
            let contact_z = current.1 + delta.1 * time;
            let normal_length = ((tx - contact_x).powi(2) + (tz - contact_z).powi(2)).sqrt();
            if normal_length <= CONTACT_EPSILON {
                continue;
            }
            best = Some(ContactHit {
                target,
                nx: (tx - contact_x) / normal_length,
                nz: (tz - contact_z) / normal_length,
                time,
                inside: false,
            });
        }
        best
    }

    fn queue_push<V: CharacterView>(
        &mut self,
        mover: usize,
        target: usize,
        nx: f32,
        nz: f32,
        views: &[V],
    ) -> bool {
        let sources = self.contact_sources.entry(target).or_default();
        if sources.contains(&mover) {
            return false;
        }
        sources.push(mover);

        let tv = views.get(target).unwrap();
        if !tv.pushable() {
            return false;
        }
        if tv.in_guard() {
            return false;
        }
        if !near_ground(tv.position().1, tv.airborne(), tv.ground_y()) {
            return false;
        }

        let queue = if self.finishing || self.processed_targets.contains(&target) {
            self.deferred_sources.entry(target).or_default().push(mover);
            &mut self.deferred
        } else {
            &mut self.pending
        };
        let entry = queue.entry(target).or_insert((0.0, 0.0));
        entry.0 += nx * CHARACTER_PUSH_SPEED;
        entry.1 += nz * CHARACTER_PUSH_SPEED;
        true
    }

    pub fn observe_segment<V: CharacterView>(
        &mut self,
        mover: usize,
        from: (f32, f32),
        to: (f32, f32),
        views: &[V],
    ) -> usize {
        let Some(mover_view) = views.get(mover) else {
            return 0;
        };
        if !mover_view.visible()
            || !mover_view.alive()
            || ignores_character_collision(mover_view.pose_id(), mover_view.explicit_ignore())
        {
            return 0;
        }
        let dx = to.0 - from.0;
        let dz = to.1 - from.1;
        let length_sq = dx * dx + dz * dz;
        if length_sq <= CONTACT_EPSILON * CONTACT_EPSILON {
            return 0;
        }

        let combined_radius = CHARACTER_CONTACT_RADIUS * 2.0;
        let combined_radius_sq = combined_radius * combined_radius;
        let mover_y = mover_view.position().1;
        let mut contacts = 0;
        for target in self.fighters.clone() {
            if target == mover {
                continue;
            }
            let Some(target_view) = views.get(target) else {
                continue;
            };
            if !target_view.pushable()
                || !target_view.visible()
                || !target_view.alive()
                || ignores_character_collision(target_view.pose_id(), target_view.explicit_ignore())
                || (target_view.position().1 - mover_y).abs() >= CHARACTER_CONTACT_HEIGHT
            {
                continue;
            }

            let target_pos = target_view.position();
            let rx = from.0 - target_pos.0;
            let rz = from.1 - target_pos.2;
            let b = 2.0 * (rx * dx + rz * dz);
            let c = rx * rx + rz * rz - combined_radius_sq;
            let discriminant = b * b - 4.0 * length_sq * c;
            let time = if c <= 0.0 {
                Some(0.0)
            } else if b < 0.0 && discriminant >= 0.0 {
                let candidate = (-b - discriminant.sqrt()) / (2.0 * length_sq);
                (0.0..=1.0).contains(&candidate).then_some(candidate)
            } else {
                None
            };
            let Some(time) = time else {
                continue;
            };

            let contact_x = from.0 + dx * time;
            let contact_z = from.1 + dz * time;
            let nx_raw = target_pos.0 - contact_x;
            let nz_raw = target_pos.2 - contact_z;
            let normal_length = (nx_raw * nx_raw + nz_raw * nz_raw).sqrt();
            if normal_length <= CONTACT_EPSILON {
                continue;
            }
            let nx = nx_raw / normal_length;
            let nz = nz_raw / normal_length;
            if dx * nx + dz * nz <= 0.0 {
                continue;
            }
            if self.queue_push(mover, target, nx, nz, views) {
                contacts += 1;
            }
        }
        contacts
    }

    pub fn discard_ineligible_pushes<V: CharacterView>(&mut self, views: &[V]) {
        self.pending.retain(|target, _| {
            views.get(*target).is_some_and(|view| {
                view.visible()
                    && view.alive()
                    && view.pushable()
                    && !view.in_guard()
                    && !ignores_character_collision(view.pose_id(), view.explicit_ignore())
                    && near_ground(view.position().1, view.airborne(), view.ground_y())
            })
        });
    }

    pub fn take_actor_push<V: CharacterView>(
        &mut self,
        target: usize,
        dt: f32,
        views: &[V],
    ) -> Option<(f32, f32)> {
        if !self.processed_targets.insert(target) {
            return None;
        }
        self.discard_ineligible_pushes(views);
        let raw = self.pending.remove(&target)?;
        if !dt.is_finite() || dt <= 0.0 {
            return None;
        }
        let velocity = (
            raw.0
                .clamp(-CHARACTER_CONTACT_MAX_SPEED, CHARACTER_CONTACT_MAX_SPEED),
            raw.1
                .clamp(-CHARACTER_CONTACT_MAX_SPEED, CHARACTER_CONTACT_MAX_SPEED),
        );
        self.contact_velocities.insert(target, velocity);
        Some(velocity)
    }

    pub fn finish_frame(&mut self, dt: f32) {
        if dt.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) || self.pending.is_empty() {
            self.pending.clear();
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        for (target, raw) in pending {
            let vx = raw
                .0
                .clamp(-CHARACTER_CONTACT_MAX_SPEED, CHARACTER_CONTACT_MAX_SPEED);
            let vz = raw
                .1
                .clamp(-CHARACTER_CONTACT_MAX_SPEED, CHARACTER_CONTACT_MAX_SPEED);
            self.contact_velocities.insert(target, (vx, vz));
        }
        self.finishing = true;
    }

    pub fn end_frame(&mut self) {
        self.finishing = false;
    }
}

impl Default for CharacterContactSystem {
    fn default() -> Self {
        Self::new()
    }
}

fn near_ground(pos_y: f32, airborne: bool, ground_y: f32) -> bool {
    if airborne {
        return false;
    }
    let height = pos_y - ground_y;
    (-CONTACT_EPSILON..=CHARACTER_GROUND_CONTACT_DISTANCE + CONTACT_EPSILON).contains(&height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_repulsion_replays_stock_knuckle_mover_inputs() {
        let fixture = include_str!("../tests/fixtures/stock-knuckle-repulsion.txt");
        let mut count = 0;
        for line in fixture.lines().filter(|line| !line.starts_with('#')) {
            let v: Vec<f32> = line
                .split_whitespace()
                .map(|s| s.parse().unwrap())
                .collect();
            let delta = (v[0] * MODEL_SCALE, v[2] * MODEL_SCALE, -v[1] * MODEL_SCALE);
            let distance = (delta.0 * delta.0 + delta.1 * delta.1 + delta.2 * delta.2).sqrt();
            let force = authored_repulsion_force(delta, distance);
            let observed = (v[4] * MODEL_SCALE, -v[5] * MODEL_SCALE);
            assert!(
                (force.0 * v[3] - observed.0).hypot(force.1 * v[3] - observed.1)
                    < 0.001 * MODEL_SCALE,
                "stock sample {count}: {line}, force={force:?}"
            );
            count += 1;
        }
        assert_eq!(count, 1008);
    }

    #[test]
    fn authored_repulsion_has_three_dimensional_and_inner_distance_gates() {
        assert_eq!(authored_repulsion_force((0.0, 0.0, 0.0), 0.0), (0.0, 0.0));
        assert_eq!(
            authored_repulsion_force((MODEL_SCALE, 0.0, 0.0), MODEL_SCALE),
            (0.0, 0.0)
        );

        assert_eq!(authored_repulsion_force((0.5, 3.0, 0.0), 3.04), (0.0, 0.0));
        assert_eq!(
            authored_repulsion_force((1.0, 0.0, 0.0), 24.0 * MODEL_SCALE),
            (0.0, 0.0)
        );
        assert_eq!(
            authored_repulsion_force((f32::NAN, 0.0, 0.0), 1.0),
            (0.0, 0.0)
        );
        let (x, z) = authored_repulsion_force((12.0 * MODEL_SCALE, 0.0, 0.0), 12.0 * MODEL_SCALE);
        assert!((x - 900.0 * MODEL_SCALE).abs() < 1e-4);
        assert_eq!(z, 0.0);
    }

    struct StubView {
        pos: (f32, f32, f32),
        pose: u16,
        guard: bool,
        air: bool,
        visible: bool,
        alive: bool,
    }
    impl CharacterView for StubView {
        fn position(&self) -> (f32, f32, f32) {
            self.pos
        }
        fn pose_id(&self) -> u16 {
            self.pose
        }
        fn in_guard(&self) -> bool {
            self.guard
        }
        fn airborne(&self) -> bool {
            self.air
        }
        fn visible(&self) -> bool {
            self.visible
        }
        fn alive(&self) -> bool {
            self.alive
        }
        fn explicit_ignore(&self) -> bool {
            false
        }

        fn pushable(&self) -> bool {
            self.pose != 999
        }
    }

    #[test]
    fn serial_contact_pushes_follow_controller_order_and_are_consumed_once() {
        let radius = CHARACTER_CONTACT_RADIUS * 2.0;
        let views = [0.0, radius, radius * 2.0].map(|x| StubView {
            pos: (x, 0.0, 0.0),
            pose: 0,
            guard: false,
            air: false,
            visible: true,
            alive: true,
        });
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1, 2]);
        sys.resolve_xz(0, (0.0, 0.0), (0.1, 0.0), &views);
        assert_eq!(sys.take_actor_push(0, 1.0 / 60.0, &views), None);
        assert_eq!(
            sys.take_actor_push(1, 1.0 / 60.0, &views),
            Some((CHARACTER_PUSH_SPEED, 0.0))
        );
        assert_eq!(sys.take_actor_push(1, 1.0 / 60.0, &views), None);

        sys.resolve_xz(1, (radius, 0.0), (0.04, 0.0), &views);
        assert_eq!(
            sys.take_actor_push(2, 1.0 / 60.0, &views),
            Some((CHARACTER_PUSH_SPEED, 0.0))
        );
        sys.resolve_xz(1, (radius, 0.0), (-0.04, 0.0), &views);
        assert_eq!(sys.take_actor_push(0, 1.0 / 60.0, &views), None);
        sys.end_frame();
        sys.begin_frame(vec![0, 1, 2]);

        sys.resolve_xz(1, (radius, 0.0), (-0.04, 0.0), &views);
        assert_eq!(
            sys.take_actor_push(0, 1.0 / 60.0, &views),
            Some((-CHARACTER_PUSH_SPEED, 0.0))
        );
    }

    #[test]
    fn restoring_contact_preserves_fist_419_outward_step_without_pop() {
        let mut views = [(-0.08804786, 0.0, 1.876738), (0.0, 0.0, 2.2)].map(|pos| StubView {
            pos,
            pose: 0,
            guard: false,
            air: false,
            visible: true,
            alive: true,
        });
        let mut sys = CharacterContactSystem::new();
        for fps in [30.0, 60.0, 120.0] {
            let requested = (-0.02882178 * 120.0 / fps, -0.1056904 * 120.0 / fps);
            for pose in [118, 110] {
                views[1].pose = pose;
                sys.begin_frame(vec![0, 1]);
                let (dx, dz, _) =
                    sys.resolve_xz(0, (views[0].pos.0, views[0].pos.2), requested, &views);
                assert!(
                    (dx - requested.0).abs() < 1e-6 && (dz - requested.1).abs() < 1e-6,
                    "restored pose {pose} at {fps} FPS added displacement: {dx} {dz}"
                );
                sys.finish_frame(1.0 / fps);
                assert!(sys.contact_velocities.is_empty());
            }
        }
    }

    #[test]
    fn overlapping_contacts_block_entry_allow_exit_and_do_not_create_motion() {
        let mut views = [0.0, 0.3, -1.95].map(|x| StubView {
            pos: (x, 0.0, 0.0),
            pose: 110,
            guard: false,
            air: false,
            visible: true,
            alive: true,
        });
        let mut sys = CharacterContactSystem::new();
        for fps in [30.0, 60.0, 120.0] {
            sys.begin_frame(vec![0, 1]);
            let (dx, dz, _) = sys.resolve_xz(0, (0.0, 0.0), (3.0 / fps, 0.0), &views);
            assert!(dx.abs() < 1e-6 && dz.abs() < 1e-6);
            sys.finish_frame(1.0 / fps);
            assert_eq!(sys.contact_velocities[&1], (CHARACTER_PUSH_SPEED, 0.0));
            sys.begin_frame(vec![0, 1]);
            assert_eq!(
                sys.resolve_xz(0, (0.0, 0.0), (0.0, 0.0), &views),
                (0.0, 0.0, 0)
            );
            let (_, tangent, _) = sys.resolve_xz(0, (0.0, 0.0), (0.0, 3.0 / fps), &views);
            assert!((tangent - 3.0 / fps).abs() < 1e-6);
        }

        sys.begin_frame(vec![0, 1, 2]);
        let (dx, dz, contacts) = sys.resolve_xz(0, (0.0, 0.0), (-0.2, 0.0), &views);
        assert!(dx >= -0.03001 && dx < 0.0 && dz.abs() < 1e-6 && contacts > 0);
        views[1].pos.0 = 0.0;
        sys.begin_frame(vec![0, 1]);
        let (dx, dz, _) = sys.resolve_xz(0, (0.0, 0.0), (0.1, 0.0), &views);
        assert!((dx - 0.1).abs() < 1e-6 && dz.abs() < 1e-6);
    }

    #[test]
    fn small_local_and_remote_steps_still_detect_contact() {
        let views = [0.0, CHARACTER_CONTACT_RADIUS * 2.0 + 0.0005].map(|x| StubView {
            pos: (x, 0.0, 0.0),
            pose: 0,
            guard: false,
            air: false,
            visible: true,
            alive: true,
        });
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);
        let (dx, _, contacts) = sys.resolve_xz(0, (0.0, 0.0), (0.001, 0.0), &views);
        assert!(dx <= 0.00051 && contacts == 1);
        sys.begin_frame(vec![0, 1]);
        assert_eq!(sys.observe_segment(0, (0.0, 0.0), (0.001, 0.0), &views), 1);
    }

    #[test]
    fn touching_controllers_do_not_alternate_between_penetration_and_pop_out() {
        for offset in [0.0, 0.000001, -0.000001] {
            let mut views = [0.0, CHARACTER_CONTACT_RADIUS * 2.0 + offset].map(|x| StubView {
                pos: (x, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            });
            let mut sys = CharacterContactSystem::new();
            for _ in 0..30 {
                sys.begin_frame(vec![0, 1]);
                let (dx, dz, _) = sys.resolve_xz(0, (views[0].pos.0, 0.0), (0.3, 0.0), &views);
                assert!(dx.abs() < 0.00001 && dz.abs() < 0.00001,
                    "contact skin must block immediately, without next-frame pop: {offset} {dx} {dz}");
                views[0].pos.0 += dx;
                sys.finish_frame(1.0 / 60.0);
                assert!(sys.contact_velocities.contains_key(&1));
            }
        }
    }

    #[test]
    fn constants_match_source() {
        assert_eq!(CHARACTER_CONTACT_RADIUS, 8.0 * MODEL_SCALE);
        assert_eq!(CHARACTER_PUSH_SPEED, 20.0 * MODEL_SCALE);
        assert_eq!(CHARACTER_GROUND_CONTACT_DISTANCE, 4.0 * MODEL_SCALE);
        assert_eq!(IGNORE_CHARACTER_COLLISION_POSES.len(), 73);
    }

    #[test]
    fn ignore_table_covers_dodge() {
        assert!(ignores_character_collision(164, false));
        assert!(ignores_character_collision(176, false));

        assert!(ignores_character_collision(501, false));

        assert!(ignores_character_collision(151, false));
        assert!(ignores_character_collision(562, false));

        assert!(!ignores_character_collision(0, false));
        assert!(!ignores_character_collision(1000, false));

        assert!(ignores_character_collision(0, true));
    }

    #[test]
    fn near_ground_gate() {
        assert!((CHARACTER_GROUND_CONTACT_DISTANCE - 0.48).abs() < 1e-5);

        assert!(near_ground(0.2, false, 0.0));

        assert!(!near_ground(1.0, false, 0.0));

        assert!(!near_ground(0.2, true, 0.0));

        assert!(near_ground(-CONTACT_EPSILON, false, 0.0));

        assert!(!near_ground(-0.0001, false, 0.0));
    }

    #[test]
    fn resolve_blocked_by_target() {
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);

        let views = [
            StubView {
                pos: (0.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
            StubView {
                pos: (4.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
        ];
        let (dx, _, contacts) = sys.resolve_xz(0, (0.0, 0.0), (3.0, 0.0), &views);

        assert!(dx < 3.0);
        assert!(contacts >= 1);

        assert!(sys.pending.contains_key(&1));
    }

    #[test]
    fn z_axis_contact_is_independent_from_world_height() {
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);
        let views = [
            StubView {
                pos: (0.0, 10.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
            StubView {
                pos: (0.0, 10.0, 4.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
        ];

        let (dx, dz, contacts) = sys.resolve_xz(0, (0.0, 0.0), (0.0, 3.0), &views);
        assert!(dx.abs() < CONTACT_EPSILON);
        assert!(dz < 3.0, "+Z movement must hit the target: {dz}");
        assert!(contacts > 0);

        assert!(!sys.pending.contains_key(&1));
    }

    #[test]
    fn diagonal_contact_preserves_tangent_edge_slide() {
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);
        let views = [
            StubView {
                pos: (0.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
            StubView {
                pos: (4.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
        ];
        let (dx, dz, contacts) = sys.resolve_xz(0, (0.0, 0.0), (4.0, 2.0), &views);
        assert!(contacts > 0);
        assert!(dx < 4.0);
        assert!(dz > 0.5, "tangent component must survive: {dz}");
    }

    #[test]
    fn guard_target_not_pushed() {
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);
        let views = [
            StubView {
                pos: (0.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
            StubView {
                pos: (4.0, 0.0, 0.0),
                pose: 0,
                guard: true,
                air: false,
                visible: true,
                alive: true,
            },
        ];
        let (_, _, _) = sys.resolve_xz(0, (0.0, 0.0), (3.0, 0.0), &views);

        assert!(!sys.pending.contains_key(&1));
    }

    #[test]
    fn remote_target_blocks_without_being_pushed() {
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);
        let views = [
            StubView {
                pos: (0.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
            StubView {
                pos: (4.0, 0.0, 0.0),
                pose: 999,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
        ];
        let (dx, _, contacts) = sys.resolve_xz(0, (0.0, 0.0), (3.0, 0.0), &views);
        assert!(dx < 3.0);
        assert!(contacts > 0);
        assert!(!sys.pending.contains_key(&1));
    }

    #[test]
    fn dodge_phases_through() {
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);
        let views = [
            StubView {
                pos: (0.0, 0.0, 0.0),
                pose: 164,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
            StubView {
                pos: (4.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
        ];

        let (dx, _, contacts) = sys.resolve_xz(0, (0.0, 0.0), (3.0, 0.0), &views);
        assert!((dx - 3.0).abs() < 1e-5);
        assert_eq!(contacts, 0);
    }

    #[test]
    fn hidden_or_dead_movers_cannot_block_or_push() {
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);
        let views = [
            StubView {
                pos: (0.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: false,
            },
            StubView {
                pos: (4.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
        ];
        let (dx, _, contacts) = sys.resolve_xz(0, (0.0, 0.0), (3.0, 0.0), &views);
        assert!((dx - 3.0).abs() < CONTACT_EPSILON);
        assert_eq!(contacts, 0);
        assert_eq!(sys.observe_segment(0, (0.0, 0.0), (3.0, 0.0), &views), 0);
        assert!(sys.pending.is_empty());
    }

    #[test]
    fn finish_frame_clamps_components() {
        let mut sys = CharacterContactSystem::new();
        sys.pending.insert(5, (9999.0, -9999.0));
        sys.finish_frame(1.0 / 30.0);
        let (vx, vz) = sys.contact_velocities.get(&5).unwrap();
        assert!((*vx - CHARACTER_CONTACT_MAX_SPEED).abs() < 1e-5);
        assert!((*vz + CHARACTER_CONTACT_MAX_SPEED).abs() < 1e-5);
    }

    #[test]
    fn observed_remote_segment_pushes_owned_target() {
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);
        let views = [
            StubView {
                pos: (3.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
            StubView {
                pos: (2.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
        ];

        assert_eq!(sys.observe_segment(0, (0.0, 0.0), (3.0, 0.0), &views), 1);
        sys.finish_frame(1.0 / 30.0);
        let velocity = sys.contact_velocities.get(&1).copied().unwrap();
        assert!(velocity.0 > 0.0);
        assert!(velocity.1.abs() < CONTACT_EPSILON);
    }

    #[test]
    fn contacts_created_while_finishing_are_deferred() {
        let mut sys = CharacterContactSystem::new();
        let views = [
            StubView {
                pos: (0.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
            StubView {
                pos: (4.0, 0.0, 0.0),
                pose: 0,
                guard: false,
                air: false,
                visible: true,
                alive: true,
            },
        ];
        sys.begin_frame(vec![0, 1]);
        sys.pending.insert(0, (CHARACTER_PUSH_SPEED, 0.0));
        sys.finish_frame(1.0 / 30.0);
        let _ = sys.resolve_xz(0, (0.0, 0.0), (3.0, 0.0), &views);
        assert!(!sys.contact_velocities.contains_key(&1));

        sys.end_frame();
        sys.begin_frame(vec![0, 1]);

        let _ = sys.resolve_xz(0, (0.0, 0.0), (3.0, 0.0), &views);
        sys.finish_frame(1.0 / 30.0);
        assert_eq!(
            sys.contact_velocities.get(&1),
            Some(&(CHARACTER_PUSH_SPEED, 0.0))
        );
    }

    #[test]
    fn deferred_source_dedup_survives_entity_query_reordering() {
        let mut views = [0.0, 4.0].map(|x| StubView {
            pos: (x, 0.0, 0.0),
            pose: 0,
            guard: false,
            air: false,
            visible: true,
            alive: true,
        });
        let mut sys = CharacterContactSystem::new();
        sys.begin_frame(vec![0, 1]);
        sys.pending.insert(0, (CHARACTER_PUSH_SPEED, 0.0));
        sys.finish_frame(1.0 / 30.0);
        sys.resolve_xz(0, (0.0, 0.0), (3.0, 0.0), &views);
        sys.end_frame();
        views.swap(0, 1);
        sys.remap_deferred_targets(|old| Some(1 - old));
        sys.begin_frame(vec![0, 1]);
        sys.resolve_xz(1, (0.0, 0.0), (3.0, 0.0), &views);
        sys.finish_frame(1.0 / 30.0);
        assert_eq!(
            sys.contact_velocities.get(&0),
            Some(&(CHARACTER_PUSH_SPEED, 0.0))
        );
        assert!(!sys.contact_velocities.contains_key(&1));
    }
}
