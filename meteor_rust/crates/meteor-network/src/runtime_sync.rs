use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const REQUEST: u16 = 0x7f09;
pub const EVENT: u16 = 0x7f0a;
pub const MAGIC: &[u8; 4] = b"MRS1";
pub const SCENE_HITS: u8 = 1;
pub const PROJECTILES: u8 = 2;
pub const PROJECTILE_TIMEOUT_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectileState {
    pub player_id: String,
    pub id: u32,
    pub sequence: u32,

    pub kind: u8,
    pub weapon: u8,
    pub variant: u8,
    pub pose_id: u16,
    pub pose_frame: f32,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub velocity: [f32; 3],
    pub age: f32,
    pub active: bool,
}
impl ProjectileState {
    pub fn valid(&self) -> bool {
        self.id != 0
            && matches!(self.kind, 1 | 2)
            && (1..=12).contains(&self.weapon)
            && self.player_id.len() <= 128
            && self.pose_frame.is_finite()
            && self.pose_frame >= 0.0
            && self.age.is_finite()
            && (0.0..=20.0).contains(&self.age)
            && self
                .position
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 8192.0)
            && self
                .velocity
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 2048.0)
            && self.rotation.iter().all(|v| v.is_finite())
            && (0.9..=1.1).contains(&self.rotation.iter().map(|v| v * v).sum::<f32>())
    }
}
pub fn valid_projectile_birth(
    catalog: &crate::combat_authority::AttackCatalog,
    character: u8,
    state: &ProjectileState,
) -> bool {
    use meteor_combat::ranged_weapons::{ranged_attack_profile, RangedKind};
    ranged_attack_profile(
        state.pose_id,
        state.weapon,
        Some(meteor_combat::weapon_strategy::weapon_to_strategy(
            state.weapon,
        )),
    )
    .is_some_and(|profile| {
        profile.entity
            && matches!(
                (state.kind, profile.kind),
                (1, RangedKind::Dart) | (2, RangedKind::Wheel)
            )
    }) && catalog
        .active_attack_index(character, state.pose_id, state.pose_frame)
        .is_some()
}

pub fn newer(sequence: u32, previous: u32) -> bool {
    let delta = sequence.wrapping_sub(previous);
    delta != 0 && delta < (1 << 31)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum RuntimePacket {
    Hello,
    Welcome {
        generation: u32,
        features: u8,
    },
    Peer {
        actor_id: u32,
    },
    SceneHit {
        generation: u32,
        serial: u32,
        actor_id: u32,
        reference: String,
        pose_id: u16,
        frame: f32,
        damage: i32,
    },
    Projectile {
        generation: u32,
        state: ProjectileState,
    },
}
impl RuntimePacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(serde_json::to_vec(self).expect("finite runtime records serialize"));
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > 1100 {
            return None;
        }
        serde_json::from_slice(bytes.strip_prefix(MAGIC)?).ok()
    }
}

#[derive(Debug, Clone)]
struct RetainedProjectile {
    state: ProjectileState,
    at: u64,
}
#[derive(Debug, Clone, Default)]
pub struct ProjectileTracker {
    records: BTreeMap<(String, u32), RetainedProjectile>,
}
impl ProjectileTracker {
    pub fn clear(&mut self) {
        self.records.clear();
    }
    pub fn forget_owner(&mut self, player: &str) {
        self.records.retain(|(owner, _), _| owner != player);
    }
    pub fn accept(&mut self, state: ProjectileState, now: u64) -> bool {
        if !state.valid() {
            return false;
        }
        let key = (state.player_id.clone(), state.id);
        if let Some(previous) = self.records.get(&key) {
            if !newer(state.sequence, previous.state.sequence)
                && !(state.sequence == previous.state.sequence
                    && previous.state.active
                    && !state.active)
                || !previous.state.active && state.active
                || (state.kind, state.weapon, state.variant)
                    != (
                        previous.state.kind,
                        previous.state.weapon,
                        previous.state.variant,
                    )
            {
                return false;
            }
        } else if !state.active
            || self.active().count() >= 256
            || self
                .active()
                .filter(|previous| previous.player_id == state.player_id)
                .count()
                >= 64
        {
            return false;
        }

        if self.records.len() >= 4096 && !self.records.contains_key(&key) {
            if let Some(oldest) = self
                .records
                .iter()
                .filter(|(_, r)| !r.state.active)
                .min_by_key(|(_, r)| r.at)
                .map(|(key, _)| key.clone())
            {
                self.records.remove(&oldest);
            }
        }
        self.records
            .insert(key, RetainedProjectile { state, at: now });
        true
    }
    pub fn active(&self) -> impl Iterator<Item = &ProjectileState> {
        self.records
            .values()
            .filter(|r| r.state.active)
            .map(|r| &r.state)
    }
    pub fn known(&self, player: &str, id: u32) -> bool {
        self.records.contains_key(&(player.to_owned(), id))
    }
    pub fn retire_owner(&mut self, player: &str, now: u64) -> Vec<ProjectileState> {
        self.retire(now, |r| r.state.player_id == player)
    }
    fn retire(
        &mut self,
        now: u64,
        condition: impl Fn(&RetainedProjectile) -> bool,
    ) -> Vec<ProjectileState> {
        let mut output = Vec::new();
        for record in self.records.values_mut() {
            if record.state.active && condition(record) {
                record.state.active = false;

                record.at = now;
                output.push(record.state.clone());
            }
        }
        output
    }
    pub fn expire(&mut self, now: u64) -> Vec<ProjectileState> {
        let output = self.retire(now, |r| now.saturating_sub(r.at) > PROJECTILE_TIMEOUT_MS);
        self.records
            .retain(|_, r| r.state.active || now.saturating_sub(r.at) <= 20_000);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn state() -> ProjectileState {
        ProjectileState {
            player_id: "p1".into(),
            id: 1,
            sequence: 1,
            kind: 1,
            weapon: 1,
            variant: 0,
            pose_id: 200,
            pose_frame: 3519.0,
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            velocity: [0.0, 0.0, 42.0],
            age: 0.0,
            active: true,
        }
    }
    #[test]
    fn runtime_codec_and_projectile_lifecycle_reject_invalid_stale_and_resurrection() {
        let packet = RuntimePacket::Projectile {
            generation: 7,
            state: state(),
        };
        assert_eq!(RuntimePacket::decode(&packet.encode()), Some(packet));
        assert_eq!(RuntimePacket::decode(b"MRS2{}"), None);
        let mut tracker = ProjectileTracker::default();
        assert!(tracker.accept(state(), 0));
        assert!(!tracker.accept(state(), 1));
        let mut update = state();
        update.sequence = 2;
        update.age = 0.1;
        assert!(tracker.accept(update.clone(), 100));
        assert_eq!(tracker.active().count(), 1);
        assert_eq!(tracker.expire(2101).len(), 1);
        update.sequence = 3;
        assert!(!tracker.accept(update, 2102));
        assert_eq!(tracker.active().count(), 0);
        tracker.clear();
        let mut invalid = state();
        invalid.position[0] = f32::NAN;
        assert!(!tracker.accept(invalid, 2200));
        assert!(tracker.accept(state(), 2200));
        assert_eq!(tracker.retire_owner("p1", 2201).len(), 1);
    }
    #[test]
    fn retired_history_does_not_block_rapid_fire_and_departure_resets_actor_namespace() {
        let mut tracker = ProjectileTracker::default();
        for id in 1..300 {
            let mut flight = state();
            flight.id = id;
            assert!(tracker.accept(flight.clone(), id as u64));
            flight.sequence = 2;
            flight.active = false;
            assert!(tracker.accept(flight, id as u64));
        }
        assert_eq!(tracker.active().count(), 0);
        tracker.forget_owner("p1");
        assert!(tracker.accept(state(), 400));
    }
}
