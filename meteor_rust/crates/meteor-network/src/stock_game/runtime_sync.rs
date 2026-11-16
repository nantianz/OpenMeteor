use super::*;
use crate::runtime_sync::{ProjectileTracker, RuntimePacket, PROJECTILES, SCENE_HITS};
use std::sync::Arc;

pub(super) struct HostState {
    pub owner: Option<SocketAddr>,
    pub catalog: Option<Arc<crate::combat_authority::AttackCatalog>>,
    pub peers: HashSet<SocketAddr>,
    pub generation: u32,
    pub reset_pending: bool,
    pub hit_serials: HashMap<SocketAddr, u32>,
    pub projectiles: ProjectileTracker,
    pub pending_hits: Vec<RuntimePacket>,
}
impl Default for HostState {
    fn default() -> Self {
        Self {
            owner: None,
            catalog: None,
            peers: Default::default(),
            generation: 1,
            reset_pending: false,
            hit_serials: Default::default(),
            projectiles: Default::default(),
            pending_hits: Vec::new(),
        }
    }
}
impl StockGameServer {
    pub fn enable_runtime_sync(
        &mut self,
        owner: SocketAddr,
        catalog: Arc<crate::combat_authority::AttackCatalog>,
    ) {
        let owner = if owner.ip().is_unspecified() {
            SocketAddr::new(
                if owner.is_ipv4() {
                    std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
                } else {
                    std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
                },
                owner.port(),
            )
        } else {
            owner
        };
        self.runtime.owner = Some(owner);
        self.runtime.catalog = Some(catalog);
    }
    fn send_runtime(
        &mut self,
        peer: SocketAddr,
        packet: RuntimePacket,
        now: u64,
    ) -> Result<(), StockGameError> {
        self.transport.send_application(
            peer,
            crate::runtime_sync::EVENT,
            packet.encode(),
            STOCK_DISCRETE_RELIABLE_FLAGS,
            now,
        )?;
        Ok(())
    }
    pub(super) fn runtime_role_ready(
        &mut self,
        peer: SocketAddr,
        now: u64,
    ) -> Result<(), StockGameError> {
        if !self.runtime.peers.contains(&peer) {
            return Ok(());
        }
        let Some(actor_id) = self.roles.get(&peer).map(|r| r.actor_id) else {
            return Ok(());
        };
        if let Some(owner) = self
            .runtime
            .owner
            .filter(|owner| self.transport.peer_transport_id(*owner).is_some())
        {
            let actors = if peer == owner {
                self.roles
                    .iter()
                    .filter(|(peer, _)| self.runtime.peers.contains(peer))
                    .map(|(_, role)| role.actor_id)
                    .collect::<Vec<_>>()
            } else {
                vec![actor_id]
            };
            for actor_id in actors {
                self.send_runtime(owner, RuntimePacket::Peer { actor_id }, now)?;
            }
        }
        self.send_runtime(
            peer,
            RuntimePacket::Welcome {
                generation: self.runtime.generation,
                features: SCENE_HITS | PROJECTILES,
            },
            now,
        )?;
        if Some(peer) == self.runtime.owner {
            for hit in std::mem::take(&mut self.runtime.pending_hits) {
                self.send_runtime(peer, hit, now)?;
            }
        }
        let retained = self
            .runtime
            .projectiles
            .active()
            .filter(|s| s.player_id != format!("stock-{actor_id}"))
            .cloned()
            .collect::<Vec<_>>();
        for state in retained {
            self.send_runtime(
                peer,
                RuntimePacket::Projectile {
                    generation: self.runtime.generation,
                    state,
                },
                now,
            )?;
        }
        Ok(())
    }
    pub(super) fn runtime_reset(&mut self, now: u64) -> Result<(), StockGameError> {
        self.runtime.reset_pending = false;
        self.runtime.generation = self.runtime.generation.wrapping_add(1).max(1);
        self.runtime.hit_serials.clear();
        self.runtime.projectiles.clear();
        self.runtime.pending_hits.clear();
        for peer in self.runtime.peers.clone() {
            self.runtime_role_ready(peer, now)?;
        }
        Ok(())
    }
    pub(super) fn handle_runtime_request(
        &mut self,
        peer: SocketAddr,
        payload: &[u8],
        now: u64,
    ) -> Result<(), StockGameError> {
        if self.runtime.owner.is_none() || self.runtime.catalog.is_none() {
            return Ok(());
        }
        let Some(packet) = RuntimePacket::decode(payload) else {
            return Ok(());
        };
        if packet == RuntimePacket::Hello {
            self.runtime.peers.insert(peer);
            return self.runtime_role_ready(peer, now);
        }
        if self.round_ended || !self.runtime.peers.contains(&peer) {
            return Ok(());
        }
        let Some(role) = self.roles.get(&peer).cloned() else {
            return Ok(());
        };
        match packet {
            RuntimePacket::SceneHit {
                generation,
                serial,
                reference,
                pose_id,
                frame,
                ..
            } => {
                if role.health <= 0.0
                    || generation != self.runtime.generation
                    || serial == 0
                    || Some(peer) == self.runtime.owner
                    || reference.is_empty()
                    || reference.len() > 128
                    || self
                        .runtime
                        .hit_serials
                        .get(&peer)
                        .is_some_and(|previous| !crate::runtime_sync::newer(serial, *previous))
                {
                    return Ok(());
                }
                let catalog = self.runtime.catalog.as_ref().unwrap();
                if catalog
                    .active_attack_index(role.hero as u8, pose_id, frame)
                    .is_none()
                {
                    return Ok(());
                }
                let (weapon, variant) = role
                    .weapon_item_id
                    .and_then(stock_weapon_from_item_id)
                    .unwrap_or((6, 0));
                let model = format!(
                    "w{}_{}",
                    meteor_combat::weapon_strategy::weapon_to_strategy(weapon),
                    variant
                );
                let power = self
                    .combat_resources
                    .weapon_stats_for_model(&model)
                    .or_else(|| self.combat_resources.weapon_stats(weapon));
                let damage = meteor_combat::hit_resolve::sum_attack_power(
                    weapon,
                    pose_id,
                    u32::from(pose_id),
                    power.map(|s| s.attack),
                    role.item_attack_bonus(),
                    &|pose| self.combat_resources.pose_attack_percent(pose),
                );
                self.runtime.hit_serials.insert(peer, serial);
                let hit = RuntimePacket::SceneHit {
                    generation,
                    serial,
                    actor_id: role.actor_id,
                    reference,
                    pose_id,
                    frame,
                    damage,
                };
                let owner = self.runtime.owner.unwrap();
                if self.roles.contains_key(&owner) {
                    self.send_runtime(owner, hit, now)?;
                } else if self.runtime.pending_hits.len() < 512 {
                    self.runtime.pending_hits.push(hit);
                }
            }
            RuntimePacket::Projectile {
                generation,
                mut state,
            } => {
                if generation != self.runtime.generation || !state.valid() {
                    return Ok(());
                }

                let owner = if Some(peer) == self.runtime.owner {
                    state
                        .player_id
                        .strip_prefix("stock-")
                        .and_then(|id| id.parse::<u32>().ok())
                        .unwrap_or(role.actor_id)
                } else {
                    role.actor_id
                };
                let Some(actor) = self.roles.values().find(|r| r.actor_id == owner) else {
                    return Ok(());
                };
                state.player_id = format!("stock-{owner}");
                let weapon = actor
                    .weapon_item_id
                    .and_then(stock_weapon_from_item_id)
                    .unwrap_or((6, 0));
                if state.active && !self.runtime.projectiles.known(&state.player_id, state.id) {
                    if actor.health <= 0.0
                        || (state.weapon, state.variant) != weapon
                        || !crate::runtime_sync::valid_projectile_birth(
                            self.runtime.catalog.as_ref().unwrap(),
                            actor.hero as u8,
                            &state,
                        )
                    {
                        return Ok(());
                    }
                }
                if !self.runtime.projectiles.accept(state.clone(), now) {
                    return Ok(());
                }
                for target in self
                    .runtime
                    .peers
                    .clone()
                    .into_iter()
                    .filter(|target| *target != peer)
                {
                    self.send_runtime(
                        target,
                        RuntimePacket::Projectile {
                            generation,
                            state: state.clone(),
                        },
                        now,
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn update_runtime(&mut self, now: u64) -> Result<(), StockGameError> {
        if self.runtime.reset_pending {
            self.runtime_reset(now)?;
        }

        let alive = self
            .roles
            .values()
            .map(|r| format!("stock-{}", r.actor_id))
            .collect::<HashSet<_>>();
        let gone = self
            .runtime
            .projectiles
            .active()
            .filter(|s| !alive.contains(&s.player_id))
            .map(|s| s.player_id.clone())
            .collect::<HashSet<_>>();
        let mut stopped = self.runtime.projectiles.expire(now);
        for player in gone {
            stopped.extend(self.runtime.projectiles.retire_owner(&player, now));
        }
        for state in stopped {
            for target in self.runtime.peers.clone() {
                self.send_runtime(
                    target,
                    RuntimePacket::Projectile {
                        generation: self.runtime.generation,
                        state: state.clone(),
                    },
                    now,
                )?;
            }
        }
        Ok(())
    }
}
