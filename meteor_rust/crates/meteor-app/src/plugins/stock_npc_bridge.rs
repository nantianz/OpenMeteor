use super::*;
use crate::plugins::stage_plugin::MissionNpc;
use meteor_network::role_state::RoleState;
use meteor_network::stock_game::StockGameError;
use std::collections::HashMap;

pub(super) struct NpcRole {
    pub entity: Entity,
    pub hero: u32,
    pub weapon: u8,
    pub camp: u32,

    #[allow(dead_code)]
    pub name: String,
    pub state: RoleState,
}

#[derive(Default)]
pub(super) struct Bridge {
    clients: HashMap<Entity, StockGameClient>,
    anger_revisions: HashMap<Entity, u64>,
}

impl Bridge {
    pub(super) fn sync(
        &mut self,
        server: &StockGameServer,
        roster: Vec<NpcRole>,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.clients.retain(|entity, client| {
            if roster.iter().any(|role| role.entity == *entity) {
                true
            } else {
                let _ = client.transport_mut().disconnect(0);
                false
            }
        });
        let mut address = server.transport().local_addr()?;
        address.set_ip(if address.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
        for role in roster {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                self.clients.entry(role.entity)
            {
                let transport = StockClientTransport::bind(
                    if address.is_ipv4() {
                        "127.0.0.1:0"
                    } else {
                        "[::1]:0"
                    },
                    address,
                    meteor_network::stock_udp::StockConnectRecord::for_open_room(),
                )?;
                entry.insert(super::stock_asset_client(
                    transport,
                    role.hero,
                    role.camp,
                    role.weapon,
                )?);
            }
            let client = self
                .clients
                .get_mut(&role.entity)
                .expect("inserted NPC client");
            if client.actor_id().is_some() {
                let state = server
                    .scene_frame()
                    .map_or(role.state, |frame| frame.to_stock(role.state));
                client.send_role_state(state, now_ms)?;
            }
        }
        Ok(())
    }

    pub(super) fn send_self_effect(
        &mut self,
        entity: Entity,
        id: u32,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.clients
            .get_mut(&entity)
            .ok_or(StockGameError::RoleNotEntered)?
            .send_self_effect(id, now_ms)
    }

    pub(super) fn send_hit(
        &mut self,
        entity: Entity,
        hit: StockHitRequest,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.clients
            .get_mut(&entity)
            .ok_or(StockGameError::RoleNotEntered)?
            .send_hit(hit, now_ms)
    }

    pub(super) fn tick(
        &mut self,
        now_ms: u64,
        to_app: &Sender<NetInbound>,
    ) -> Result<(), StockGameError> {
        for (&entity, client) in &mut self.clients {
            for event in client.tick(now_ms)? {
                if let StockGameEvent::Vitals(vitals) = event {
                    let _ = to_app.send(NetInbound::StockVitals {
                        actor: Some(entity),
                        vitals,
                        spend_revision: self.anger_revisions.get(&entity).copied().unwrap_or(0),
                    });
                    continue;
                }

                if matches!(
                    event,
                    StockGameEvent::TransportConnected { .. }
                        | StockGameEvent::Vitals(_)
                        | StockGameEvent::LocalDeath
                        | StockGameEvent::LocalRespawn
                        | StockGameEvent::PeerDisconnected { .. }
                ) || matches!(&event, StockGameEvent::RoleEntered(role) if Some(role.actor_id) == client.actor_id())
                    || matches!(&event, StockGameEvent::PropertyEffect(event) if Some(event.actor_id) == client.actor_id())
                    || matches!(&event, StockGameEvent::HitReaction { event, .. } if Some(event.target_actor_id) == client.actor_id())
                {
                    let _ = to_app.send(NetInbound::StockNpcEvent(entity, event));
                }
            }
        }
        Ok(())
    }

    pub(super) fn send_anger_spend(
        &mut self,
        entity: Entity,
        key: (u32, u16),
        spend: StockAngerSpend,
        revision: u64,
        now_ms: u64,
    ) -> Result<(), StockGameError> {
        self.clients
            .get_mut(&entity)
            .ok_or(StockGameError::RoleNotEntered)?
            .send_anger_spend(key, spend, now_ms)?;
        self.anger_revisions.insert(entity, revision);
        Ok(())
    }
}

pub(super) fn apply_event(net: &mut NetState, entity: Entity, event: StockGameEvent) {
    match event {
        StockGameEvent::TransportConnected { transport_id, .. } => {
            net.stock_npc_actors.insert(entity, transport_id);
        }
        StockGameEvent::RoleEntered(role) => {
            net.stock_npc_actors.insert(entity, role.actor_id);
            net.stock_npc_ready.insert(entity);
        }
        StockGameEvent::Vitals(vitals) => {
            net.pending_npc_vitals.push((
                entity,
                [
                    f32::from(vitals.health),
                    f32::from(vitals.max_health),
                    f32::from(vitals.anger),
                ],
            ));
        }
        StockGameEvent::LocalRespawn => {
            if let Some(actor) = net.stock_npc_actors.get(&entity) {
                let player_id = format!("stock-{actor}");
                net.pending_spawn_relocations.insert(player_id.clone());
                net.pending_respawns.push(player_id);
            }
        }
        StockGameEvent::PropertyEffect(event) => {
            net.pending_status_effects.push((Some(entity), event));
        }
        StockGameEvent::LocalDeath => {
            net.pending_npc_hits.push((
                entity,
                AuthorityHit {
                    stock_guard_bucket: None,
                    damage: 0.0,
                    health: Some(0.0),
                    anger: None,
                    guarded: false,
                    attacker_id: None,
                    effect_strategy: None,
                    motion_scale: None,
                    reaction: None,
                    tick: net.last_role_snapshot_tick,
                    impact: false,
                    terminal: true,
                },
            ));
        }
        StockGameEvent::HitReaction { guarded, event } => {
            let reaction = if guarded {
                CombatReactionWire {
                    defense_move: event.motion_scale(),
                    defense_value: event.minimum_seconds(),
                    target_pose: Some((event.pose_id >> 16) as u16),
                    ..Default::default()
                }
            } else {
                CombatReactionWire {
                    target_move: event.motion_scale(),
                    target_value: event.minimum_seconds(),
                    target_pose: Some(event.pose_id as u16),
                    ..Default::default()
                }
            };
            net.pending_npc_hits.push((
                entity,
                AuthorityHit {
                    stock_guard_bucket: guarded.then(|| super::stock_guard_bucket(event.pose_id)),
                    damage: 0.0,
                    health: None,
                    anger: None,
                    guarded,
                    attacker_id: None,
                    effect_strategy: u8::try_from(event.context)
                        .ok()
                        .filter(|strategy| *strategy < 12),
                    motion_scale: Some(event.motion_scale()),
                    reaction: Some(reaction),
                    tick: net.last_role_snapshot_tick,
                    impact: true,
                    terminal: false,
                },
            ));
        }
        StockGameEvent::PeerDisconnected { reason, .. } => {
            net.stock_npc_ready.remove(&entity);
            net.error = Some(format!("NPC 已断开房间（reason={reason}）"));
        }
        _ => {}
    }
}

#[allow(clippy::type_complexity)]
pub(super) fn sync_npc_roles(
    time: Res<Time>,
    mut last: Local<f32>,
    mut net: ResMut<NetState>,
    tx: Res<NetTx>,
    mut npcs: Query<
        (
            Entity,
            &Fighter,
            &PoseRuntime,
            &Transform,
            &MotionState,
            &FighterAir,
            &SmState,
            &mut MissionNpc,
        ),
        With<crate::plugins::debug_plugin::DebugSpawnedNpc>,
    >,
) {
    if net.status != NetStatus::InRoom || !net.is_stock_host() || !net.round_active {
        return;
    }
    let now = time.elapsed_secs();
    if now - *last < 1.0 / net.role_uplink_hz.max(1) as f32 {
        return;
    }
    *last = now;
    let roster: Vec<_> = npcs
        .iter_mut()
        .map(
            |(entity, fighter, pose, transform, motion, air, sm, mut mission)| {
                if net.mode == "leader" {
                    mission.team = 0;
                }
                NpcRole {
                    entity,
                    hero: u32::from(fighter.character_id),
                    weapon: fighter.weapon,
                    camp: u32::from(mission.team),
                    name: mission.name.clone(),
                    state: role_state_from_runtime(local_request1_runtime(
                        pose,
                        transform,
                        motion,
                        air,
                        sm.current == meteor_core::FighterState::Guard,
                    )),
                }
            },
        )
        .collect();
    net.stock_npc_actors
        .retain(|entity, _| roster.iter().any(|role| role.entity == *entity));
    net.stock_npc_ready
        .retain(|entity| roster.iter().any(|role| role.entity == *entity));
    if let Some(sender) = &tx.active {
        let _ = sender.send(NetOutbound::StockNpcRoster(roster));
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    fn role(entity: Entity) -> NpcRole {
        NpcRole {
            entity,
            hero: 1,
            weapon: 6,
            camp: 0,
            name: "DebugNPC_1".into(),
            state: role_state_from_runtime(RoleStateRuntime {
                position: RoleVec3 {
                    x: 0.0,
                    y: 0.0,
                    z: 3.0,
                },
                forward: RoleVec3 {
                    x: 0.0,
                    y: 0.0,
                    z: -1.0,
                },
                velocity: RoleVec3::ZERO,
                pose_id: 250,
                grounded: true,
                guard_route: false,
                playback_arg: 0,
                pose_sequence: 1,
            }),
        }
    }

    fn pump(
        server: &mut StockGameServer,
        clients: &mut [&mut StockGameClient],
        bridge: &mut Bridge,
        tx: &Sender<NetInbound>,
        start: u64,
        count: u64,
    ) -> Vec<Vec<StockGameEvent>> {
        let mut result = vec![Vec::new(); clients.len()];
        for now in start..start + count {
            server.tick(now).unwrap();
            bridge.tick(now, tx).unwrap();
            for (index, client) in clients.iter_mut().enumerate() {
                result[index].extend(client.tick(now).unwrap());
            }
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        result
    }

    #[test]
    fn stock_npc_bridge_enters_hits_host_and_receives_damage_without_duplicates() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "NPC regression").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::bundled_revision(),
            1,
            stock_combat_resources(),
        );
        let mut host = StockGameClient::for_bundled_revision(
            StockClientTransport::bind(
                "127.0.0.1:0",
                address,
                meteor_network::stock_udp::StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        let mut observer = StockGameClient::for_bundled_revision(
            StockClientTransport::bind(
                "127.0.0.1:0",
                address,
                meteor_network::stock_udp::StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            0,
            0,
            6,
        )
        .unwrap();
        let npc = World::new().spawn_empty().id();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut bridge = Bridge::default();
        bridge.sync(&server, vec![role(npc)], 0).unwrap();
        pump(
            &mut server,
            &mut [&mut host, &mut observer],
            &mut bridge,
            &tx,
            0,
            150,
        );
        assert_eq!(server.ready_peer_count(), 3);
        let npc_id = bridge.clients[&npc].actor_id().unwrap();
        let host_id = host.actor_id().unwrap();
        assert_ne!(npc_id, host_id);
        let mut net = NetState::default();
        for message in rx.try_iter() {
            if let NetInbound::StockNpcEvent(entity, event) = message {
                apply_event(&mut net, entity, event);
            }
        }
        assert_eq!(net.stock_npc_actors[&npc], npc_id);
        assert!(net.stock_npc_ready.contains(&npc));
        bridge.sync(&server, vec![role(npc)], 200).unwrap();
        let states = pump(
            &mut server,
            &mut [&mut host, &mut observer],
            &mut bridge,
            &tx,
            200,
            80,
        );
        for events in states {
            assert!(events
                .iter()
                .any(|event| matches!(event, StockGameEvent::RoleStates(records)
                if records.iter().any(|record| u32::from(record.actor_id) == npc_id))));
        }
        let hit = StockHitRequest {
            target_actor_id: host_id,
            hit_word: 250 | (10 << 16),
            target_pose_word: 93,
            moves_word: 400,
            values_word: 50,
        };
        bridge.send_hit(npc, hit, 2_200).unwrap();
        bridge.send_hit(npc, hit, 2_200).unwrap();
        let events = pump(
            &mut server,
            &mut [&mut host, &mut observer],
            &mut bridge,
            &tx,
            2_200,
            150,
        );
        assert_eq!(events[0].iter().filter(|event| matches!(event,
            StockGameEvent::HitReaction { guarded: false, event } if event.target_actor_id == host_id && event.context == 10)).count(), 1);

        assert!(!events[1]
            .iter()
            .any(|event| matches!(event, StockGameEvent::HitReaction { .. })));
        let host_hp = events[0]
            .iter()
            .find_map(|event| match event {
                StockGameEvent::Vitals(v) if v.health < v.max_health => Some(v.health),
                _ => None,
            })
            .expect("NPC hit must lower the host's authoritative HP");
        assert!(host_hp > 0);
        let gain = ((2_000 - u32::from(host_hp)) * 14 / 100) as u16;
        assert!(events[0]
            .iter()
            .any(|event| matches!(event, StockGameEvent::Vitals(v) if v.anger == gain)));
        host.send_hit(
            StockHitRequest {
                target_actor_id: npc_id,
                ..hit
            },
            2_500,
        )
        .unwrap();
        let player_after_attack = pump(
            &mut server,
            &mut [&mut host, &mut observer],
            &mut bridge,
            &tx,
            2_500,
            150,
        );
        assert!(
            player_after_attack[0]
                .iter()
                .filter_map(|event| match event {
                    StockGameEvent::Vitals(v) => Some(v.anger),
                    _ => None,
                })
                .all(|anger| anger == gain),
            "landing a hit grants no attacker Qi"
        );
        let mut npc_hp = None;
        let mut npc_anger = 0;
        for message in rx.try_iter() {
            if let NetInbound::StockVitals {
                actor: Some(entity),
                vitals,
                ..
            } = message
            {
                if vitals.health < vitals.max_health {
                    npc_hp = Some(vitals.health);
                }
                npc_anger = vitals.anger;
                apply_event(&mut net, entity, StockGameEvent::Vitals(vitals));
                continue;
            }
            if let NetInbound::StockNpcEvent(entity, event) = message {
                if let StockGameEvent::Vitals(v) = &event {
                    if v.health < v.max_health {
                        npc_hp = Some(v.health);
                    }
                }
                apply_event(&mut net, entity, event);
            }
        }
        assert_eq!(
            npc_hp,
            Some(host_hp),
            "equal weapons use the same host damage formula in both directions"
        );
        assert_eq!(
            npc_anger, gain,
            "defender gains floor(14 * actual damage / 100)"
        );
        assert_eq!(
            net.pending_npc_hits
                .iter()
                .filter(|(_, hit)| hit.impact)
                .count(),
            1
        );
        assert_eq!(
            net.pending_npc_hits
                .iter()
                .find(|(_, hit)| hit.impact)
                .unwrap()
                .1
                .effect_strategy,
            Some(10)
        );

        assert!(gain > 0);
        let further_hits = 61_u64.div_ceil(u64::from(gain)) - 1;
        for index in 0..further_hits {
            let now = 2_800 + index * 250;
            host.send_hit(
                StockHitRequest {
                    target_actor_id: npc_id,
                    ..hit
                },
                now,
            )
            .unwrap();
            pump(
                &mut server,
                &mut [&mut host, &mut observer],
                &mut bridge,
                &tx,
                now,
                150,
            );
            for message in rx.try_iter() {
                if let NetInbound::StockVitals {
                    actor: Some(entity),
                    vitals,
                    ..
                } = message
                {
                    assert_eq!(entity, npc);
                    npc_anger = vitals.anger;
                }
            }
        }
        assert_eq!(
            u64::from(npc_anger),
            (u64::from(gain) * (further_hits + 1)).min(100)
        );
        assert!(npc_anger >= 60);
        let burst_at = 2_800 + further_hits * 250 + 200;
        eprintln!("NPC authority regression: hit damage={}, gain per hit={gain}, funded Qi={npc_anger}, after burst={}", 2_000 - u32::from(host_hp), npc_anger - 60);
        bridge
            .send_anger_spend(npc, (1, 85), StockAngerSpend::Burst, 1, burst_at)
            .unwrap();
        pump(
            &mut server,
            &mut [&mut host, &mut observer],
            &mut bridge,
            &tx,
            burst_at,
            3_100,
        );
        let mut samples = 0;
        for message in rx.try_iter() {
            if let NetInbound::StockVitals {
                actor: Some(entity),
                vitals,
                spend_revision,
            } = message
            {
                assert_eq!(entity, npc);
                assert_eq!(spend_revision, 1);
                assert_eq!(
                    vitals.anger,
                    npc_anger - 60,
                    "NPC burst stays debited across future vitals"
                );
                samples += 1;
            }
        }
        assert!(samples > 10);

        bridge.sync(&server, vec![], burst_at + 3_200).unwrap();
        let events = pump(
            &mut server,
            &mut [&mut host, &mut observer],
            &mut bridge,
            &tx,
            burst_at + 3_200,
            80,
        );
        assert_eq!(server.ready_peer_count(), 2);
        assert!(events[1].iter().any(
            |event| matches!(event, StockGameEvent::RoleLeft(left) if left.actor_id == npc_id)
        ));
    }

    #[test]
    fn stock_npc_status_effect_survives_private_bridge_filter() {
        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "NPC status").unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::bundled_revision(),
            1,
            stock_combat_resources(),
        );
        let mut host = StockGameClient::for_bundled_revision(
            StockClientTransport::bind(
                "127.0.0.1:0",
                address,
                meteor_network::stock_udp::StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            0,
            0,
            1,
        )
        .unwrap();
        let npc = World::new().spawn_empty().id();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut bridge = Bridge::default();
        bridge.sync(&server, vec![role(npc)], 0).unwrap();
        pump(&mut server, &mut [&mut host], &mut bridge, &tx, 0, 100);
        let npc_id = bridge.clients[&npc].actor_id().unwrap();
        let mut net = NetState::default();
        for message in rx.try_iter() {
            if let NetInbound::StockNpcEvent(entity, event) = message {
                apply_event(&mut net, entity, event);
            }
        }
        host.send_hit(
            StockHitRequest {
                target_actor_id: npc_id,
                hit_word: 204,
                target_pose_word: 110,
                moves_word: 100,
                values_word: 50,
            },
            2100,
        )
        .unwrap();
        pump(&mut server, &mut [&mut host], &mut bridge, &tx, 2100, 80);
        for message in rx.try_iter() {
            if let NetInbound::StockNpcEvent(entity, event) = message {
                apply_event(&mut net, entity, event);
            }
        }
        assert_eq!(
            net.pending_status_effects,
            vec![(
                Some(npc),
                meteor_network::stock_protocol::StockPropertyEffectEvent {
                    actor_id: npc_id,
                    category: 2,
                    resource_id: 12
                }
            )]
        );
    }

    fn spawn_actor(world: &mut World, local: bool) -> Entity {
        let mut actor = world.spawn((
            Fighter {
                character_id: 1,
                player_slot: if local { 0 } else { 1 },
                weapon: 6,
                weapon_slots: [6, 0],
                weapon_slot: 0,
                weapon_slot_variants: [0, 0],
                weapon_variant: 0,
            },
            Transform::default(),
            Health::default(),
            Qi::default(),
            DeathRespawn::default(),
            PoseRuntime::idle(),
            EntryBlend::default(),
            CombatState::default(),
            SmState::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
            FighterAir::default(),
            FighterDodge::default(),
            FighterAttackMotion::default(),
            TargetHitClock::default(),
            AiIntent::default(),
        ));
        if local {
            actor.insert(LocalFighter);
        }
        actor.id()
    }

    #[test]
    fn stock_npc_udp_death_respawn_reaches_ecs_and_resumes_role_uplink() {
        use bevy::ecs::system::RunSystemOnce;

        fn receive(app: &mut App, rx: &std::sync::mpsc::Receiver<NetInbound>) -> Vec<Entity> {
            let mut revived = Vec::new();
            for message in rx.try_iter() {
                let mut net = app.world_mut().resource_mut::<NetState>();
                match message {
                    NetInbound::StockNpcEvent(entity, event) => {
                        if matches!(event, StockGameEvent::LocalRespawn) {
                            revived.push(entity);
                        }
                        apply_event(&mut net, entity, event);
                    }
                    NetInbound::StockVitals {
                        actor: Some(entity),
                        vitals,
                        ..
                    } => {
                        apply_event(&mut net, entity, StockGameEvent::Vitals(vitals));
                    }
                    _ => {}
                }
            }
            app.world_mut()
                .run_system_once(net_apply_stock_vitals)
                .unwrap();
            app.world_mut().run_system_once(net_handle_hit).unwrap();
            app.world_mut()
                .run_system_once(net_apply_authority_respawns)
                .unwrap();
            revived
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p1").unwrap();
        let mut app = App::new();
        app.insert_resource(CharacterAssetsSet(HashMap::from([(
            "p1".into(),
            character,
        )])))
        .insert_resource(NetState {
            status: NetStatus::InRoom,
            address: "udp-host://127.0.0.1:0".into(),
            wire_format: NetWireFormat::V4,
            round_active: true,
            mode: "leader".into(),
            ..Default::default()
        })
        .init_resource::<LockOnController>()
        .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
        .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>();
        let player = spawn_actor(app.world_mut(), true);
        let npc = spawn_actor(app.world_mut(), false);
        let other_npc = spawn_actor(app.world_mut(), false);
        let corpse = Vec3::new(0.0, 0.0, 3.0);
        let spawn = Vec3::new(-25.0, 3.0, 40.0);
        app.world_mut()
            .get_mut::<Transform>(npc)
            .unwrap()
            .translation = corpse;
        app.world_mut()
            .get_mut::<DeathRespawn>(npc)
            .unwrap()
            .spawn_pos = spawn;

        let transport =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "NPC revival regression")
                .unwrap();
        let address = transport.local_addr().unwrap();
        let mut server = StockGameServer::with_rule_and_combat_resources(
            transport,
            StockHeroCatalog::bundled_revision(),
            1,
            stock_combat_resources(),
        );
        let mut host = StockGameClient::for_bundled_revision(
            StockClientTransport::bind(
                "127.0.0.1:0",
                address,
                meteor_network::stock_udp::StockConnectRecord::for_open_room(),
            )
            .unwrap(),
            1,
            0,
            6,
        )
        .unwrap();
        let mut bridge = Bridge::default();
        let (tx, rx) = std::sync::mpsc::channel();
        bridge
            .sync(&server, vec![role(npc), role(other_npc)], 0)
            .unwrap();
        pump(&mut server, &mut [&mut host], &mut bridge, &tx, 0, 150);
        receive(&mut app, &rx);
        let npc_id = bridge.clients[&npc].actor_id().unwrap();
        let host_id = host.actor_id().unwrap();
        app.world_mut().resource_mut::<NetState>().player_id = format!("stock-{host_id}");
        assert_eq!(
            app.world().resource::<NetState>().stock_npc_actors[&npc],
            npc_id
        );
        bridge
            .sync(&server, vec![role(npc), role(other_npc)], 200)
            .unwrap();
        host.send_role_state(role(player).state, 200).unwrap();
        pump(&mut server, &mut [&mut host], &mut bridge, &tx, 200, 100);
        receive(&mut app, &rx);

        let hit = StockHitRequest {
            target_actor_id: npc_id,
            hit_word: 250,
            target_pose_word: 93,
            moves_word: 400,
            values_word: 50,
        };
        let mut death_at = None;
        for index in 0..100 {
            let now = 2_200 + index * 250;
            host.send_hit(hit, now).unwrap();
            pump(&mut server, &mut [&mut host], &mut bridge, &tx, now, 30);
            assert!(receive(&mut app, &rx).is_empty());
            if app.world().get::<DeathRespawn>(npc).unwrap().active {
                death_at = Some(now);
                break;
            }
        }
        let death_at = death_at.expect("real stock hit requests must kill the NPC");
        assert_eq!(app.world().get::<Health>(npc).unwrap().current, 0.0);
        assert_eq!(app.world().get::<PoseRuntime>(npc).unwrap().pose_id, 117);
        assert_eq!(
            app.world().get::<SmState>(npc).unwrap().current,
            meteor_core::FighterState::Dead
        );
        pump(
            &mut server,
            &mut [&mut host],
            &mut bridge,
            &tx,
            death_at + 2_900,
            100,
        );
        assert!(receive(&mut app, &rx).is_empty());
        assert!(app.world().get::<DeathRespawn>(npc).unwrap().active);
        assert_eq!(
            app.world().get::<Transform>(npc).unwrap().translation,
            corpse
        );

        let events = pump(
            &mut server,
            &mut [&mut host],
            &mut bridge,
            &tx,
            death_at + 3_100,
            100,
        );
        assert!(events[0].iter().any(|event| matches!(event,
            StockGameEvent::Respawn(event) if event.actor_id == npc_id)));
        assert_eq!(
            receive(&mut app, &rx),
            [npc],
            "owner event 67 must survive the NPC bridge filter and reach its ECS entity"
        );
        assert!(!app.world().get::<DeathRespawn>(npc).unwrap().active);
        let hp = app.world().get::<Health>(npc).unwrap();
        assert_eq!(hp.current, hp.max);
        assert_eq!(app.world().get::<PoseRuntime>(npc).unwrap().pose_id, 0);
        assert!(
            crate::plugins::movement_plugin::ordinary_locomotion_allowed(
                app.world().get::<SmState>(npc).unwrap(),
                false
            )
        );
        assert_eq!(
            app.world().get::<Transform>(npc).unwrap().translation,
            spawn
        );
        for untouched in [player, other_npc] {
            assert!(!app.world().get::<DeathRespawn>(untouched).unwrap().active);
            assert_eq!(
                app.world().get::<Transform>(untouched).unwrap().translation,
                Vec3::ZERO
            );
            assert_eq!(
                app.world()
                    .get::<PoseRuntime>(untouched)
                    .unwrap()
                    .pose_instance_id,
                0
            );
        }

        let state = role_state_from_runtime(local_request1_runtime(
            app.world().get::<PoseRuntime>(npc).unwrap(),
            app.world().get::<Transform>(npc).unwrap(),
            app.world().get::<MotionState>(npc).unwrap(),
            app.world().get::<FighterAir>(npc).unwrap(),
            false,
        ));
        let mut revived_role = role(npc);
        revived_role.state = state;
        bridge
            .sync(
                &server,
                vec![revived_role, role(other_npc)],
                death_at + 3_250,
            )
            .unwrap();
        let events = pump(
            &mut server,
            &mut [&mut host],
            &mut bridge,
            &tx,
            death_at + 3_250,
            100,
        );
        assert!(
            events[0].iter().any(|event| matches!(event,
            StockGameEvent::RoleStates(records) if records.iter().any(|record| {
                let position = record.state.position;
                u32::from(record.actor_id) == npc_id
                    && (Vec3::new(position.x, position.y, position.z) - spawn).length() < 0.15
                    && record.state.state_words.pose_id == 0
            }))),
            "observer must receive the revived NPC's spawn position and idle pose"
        );
        assert!(
            receive(&mut app, &rx).is_empty(),
            "reliable retries cannot revive twice"
        );
    }

    #[test]
    fn stock_npc_private_vitals_reaction_and_respawn_only_change_the_owned_npc() {
        use bevy::ecs::system::RunSystemOnce;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            crate::plugins::character_plugin::load_character_asset_for_test(&[root], "p1").unwrap();
        let mut app = App::new();
        app.insert_resource(CharacterAssetsSet(HashMap::from([(
            "p1".into(),
            character,
        )])))
        .init_resource::<NetState>()
        .init_resource::<LockOnController>()
        .init_resource::<crate::plugins::camera_plugin::CameraBasis>()
        .add_message::<crate::plugins::impact_fx_plugin::CombatImpactEvent>();
        let player = spawn_actor(app.world_mut(), true);
        let npc = spawn_actor(app.world_mut(), false);
        app.world_mut().get_mut::<Fighter>(player).unwrap().weapon = 11;
        app.world_mut().get_mut::<Fighter>(npc).unwrap().weapon = 6;
        {
            let mut net = app.world_mut().resource_mut::<NetState>();
            net.status = NetStatus::InRoom;
            net.round_active = true;
            net.player_id = "stock-1".into();
            net.wire_format = NetWireFormat::V4;
            net.stock_npc_actors.insert(npc, 2);
            apply_event(
                &mut net,
                npc,
                StockGameEvent::Vitals(meteor_network::stock_protocol::StockVitalsEvent {
                    health: 900,
                    max_health: 1000,
                    anger: 14,
                }),
            );
            apply_event(
                &mut net,
                npc,
                StockGameEvent::HitReaction {
                    guarded: false,
                    event: meteor_network::stock_protocol::StockHitReactionEvent {
                        target_actor_id: 2,
                        pose_id: 93,
                        move_hundred: 400,
                        value_hundred: 50,
                        context: 10,
                    },
                },
            );
        }
        app.world_mut()
            .run_system_once(net_apply_stock_vitals)
            .unwrap();
        app.world_mut().run_system_once(net_handle_hit).unwrap();
        let impacts: Vec<_> = app
            .world_mut()
            .resource_mut::<Messages<crate::plugins::impact_fx_plugin::CombatImpactEvent>>()
            .drain()
            .collect();
        assert_eq!(impacts.len(), 1);
        assert_eq!(impacts[0].defender, npc);
        assert_eq!(
            impacts[0].effect_strategy,
            Some(10),
            "a sword NPC hit by fists must retain W10 even without an attacker ID"
        );
        assert_eq!(app.world().get::<Health>(npc).unwrap().current, 900.0);
        assert_eq!(app.world().get::<Qi>(npc).unwrap().current, 14.0);
        assert!(app
            .world()
            .get::<FighterHitReaction>(npc)
            .unwrap()
            .current
            .is_some());
        assert_eq!(app.world().get::<PoseRuntime>(npc).unwrap().pose_id, 93);
        assert_eq!(
            app.world().get::<Health>(player).unwrap().current,
            Health::default().current
        );
        assert!(app
            .world()
            .get::<FighterHitReaction>(player)
            .unwrap()
            .current
            .is_none());

        apply_event(
            &mut app.world_mut().resource_mut::<NetState>(),
            npc,
            StockGameEvent::LocalDeath,
        );
        app.world_mut().run_system_once(net_handle_hit).unwrap();
        assert!(app.world().get::<DeathRespawn>(npc).unwrap().active);
        let spawn = Vec3::new(-25.0, 3.0, 40.0);
        app.world_mut()
            .get_mut::<DeathRespawn>(npc)
            .unwrap()
            .spawn_pos = spawn;
        apply_event(
            &mut app.world_mut().resource_mut::<NetState>(),
            npc,
            StockGameEvent::LocalRespawn,
        );
        app.world_mut()
            .run_system_once(net_apply_authority_respawns)
            .unwrap();
        assert_eq!(app.world().get::<Health>(npc).unwrap().current, 1000.0);
        assert!(!app.world().get::<DeathRespawn>(npc).unwrap().active);
        assert_eq!(
            app.world().get::<Transform>(npc).unwrap().translation,
            spawn
        );
        assert_eq!(
            app.world().get::<Transform>(player).unwrap().translation,
            Vec3::ZERO
        );
        assert!(app
            .world()
            .get::<FighterHitReaction>(npc)
            .unwrap()
            .current
            .is_none());
        assert_eq!(
            app.world()
                .get::<TargetHitClock>(player)
                .unwrap()
                .spawn_elapsed,
            0.0
        );
    }

    #[test]
    fn stock_npc_observer_bootstraps_event2_but_host_does_not_duplicate_its_npc() {
        use bevy::ecs::system::RunSystemOnce;
        for owned in [false, true] {
            let mut app = App::new();
            let npc = app.world_mut().spawn_empty().id();
            let mut net = NetState {
                address: "udp-host://0.0.0.0:5230".into(),
                status: NetStatus::InRoom,
                pending_role_states: vec![("stock-2".into(), role(npc).state, 1)],
                ..Default::default()
            };
            net.remote_actor_profiles.insert("stock-2".into(), (1, 6));
            if owned {
                net.stock_npc_actors.insert(npc, 2);
            }
            app.insert_resource(net);
            app.world_mut().run_system_once(net_apply_snapshot).unwrap();
            app.world_mut().run_system_once(net_apply_snapshot).unwrap();
            let count = app
                .world_mut()
                .query::<&RemoteFighter>()
                .iter(app.world())
                .count();
            assert_eq!(
                count,
                usize::from(!owned),
                "one observer replica, no host duplicate"
            );
            assert!(app
                .world()
                .resource::<NetState>()
                .pending_role_states
                .is_empty());
        }
    }
}
