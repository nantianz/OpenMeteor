use super::network_plugin::{NetState, NetStatus, RemoteFighter};
use super::stage_plugin::LocalFighter;
use crate::components::fighter::{
    CombatState, DeathRespawn, Fighter, FighterStatusEffects, Health, PoseRuntime, Qi, SmState,
    WornEffect,
};
use bevy::prelude::*;

pub(crate) fn apply_self_attack_effect(
    active: Option<Res<super::fighter_frame::ActiveFighter>>,
    resources: Res<super::combat_plugin::CombatResourceData>,
    assets: Option<Res<super::character_plugin::CharacterAssetsSet>>,
    net: Option<Res<NetState>>,
    tx: Option<Res<super::network_plugin::NetTx>>,
    mut sequence: Option<ResMut<super::network_plugin::NetInputSequence>>,
    mut fighters: Query<
        (
            Entity,
            &Fighter,
            &PoseRuntime,
            &Health,
            &mut FighterStatusEffects,
            Option<&LocalFighter>,
        ),
        Without<RemoteFighter>,
    >,
    mut committed: Local<std::collections::HashMap<Entity, (u16, u32)>>,
) {
    committed.retain(|entity, _| fighters.contains(*entity));
    for (entity, fighter, pose, health, mut status, local) in &mut fighters {
        if !super::fighter_frame::accepts(active.as_deref(), entity) {
            continue;
        }
        let instance = (pose.pose_id, pose.pose_instance_id);
        if health.current <= 0.0 || resources.0.pose_self_status_effect(pose.pose_id).is_none() {
            committed.remove(&entity);
            continue;
        }
        if committed.get(&entity) == Some(&instance) {
            continue;
        }
        let Some(effect) = resources
            .0
            .pose_self_status_effect(pose.pose_id)
            .filter(|effect| effect.attack_bonus > 0)
        else {
            continue;
        };
        let Some(definition) = assets
            .as_ref()
            .and_then(|assets| assets.0.get(&format!("p{}", fighter.character_id)))
            .and_then(|character| character.poses.get(&pose.pose_id))
        else {
            continue;
        };
        if pose.frame < definition.action_entry_frame as f32 {
            continue;
        }
        committed.insert(entity, instance);
        if net
            .as_ref()
            .is_some_and(|net| net.status == NetStatus::InRoom)
        {
            #[cfg(not(target_arch = "wasm32"))]
            if net.as_ref().is_some_and(|net| net.is_stock_udp()) {
                if let Some(tx) = &tx {
                    tx.send_stock_self_effect(local.is_none().then_some(entity), effect.id);
                }
            }
            if let Some(net) = net
                .as_ref()
                .filter(|net| !net.is_stock_udp() && net.round_active)
            {
                if local.is_some() {
                    if let (Some(tx), Some(sequence), Some(match_epoch), Some(round_epoch)) = (
                        tx.as_ref(),
                        sequence.as_mut(),
                        net.match_epoch,
                        net.round_epoch,
                    ) {
                        tx.send_control(meteor_network::protocol::Message::RoleSelfEffect {
                            seq: sequence.next(),
                            room_id: net.room_id.clone(),
                            match_epoch,
                            round_epoch,
                            pose_id: pose.pose_id,
                            pose_instance: pose.pose_instance_id,
                        });
                    }
                }
            }
        } else {
            status.apply(effect);
        }
    }
}

pub(crate) fn clear_status_effects(
    mut fighters: Query<(&mut FighterStatusEffects, &mut WornEffect)>,
    mut net: ResMut<NetState>,
) {
    for (mut status, mut worn) in &mut fighters {
        status.clear(&mut worn);
    }
    net.pending_status_effects.clear();
    net.pending_status_snapshots.clear();
}

pub(crate) fn tick_status_effects(
    time: Res<Time>,
    clock: Option<Res<super::FrameClock>>,
    mut net: Option<ResMut<NetState>>,
    resources: Option<Res<super::combat_plugin::CombatResourceData>>,
    buffs: Option<Res<super::item_plugin::ItemBuffs>>,
    mut player_performance: Option<ResMut<super::stage_plugin::PlayerPerformance>>,
    mut fighters: Query<
        (
            Entity,
            &mut FighterStatusEffects,
            &mut Health,
            &mut Qi,
            &mut WornEffect,
            Option<&mut DeathRespawn>,
            Option<&mut SmState>,
            Option<&mut CombatState>,
            Option<&mut PoseRuntime>,
            Option<&LocalFighter>,
            Option<&RemoteFighter>,
            Option<&mut crate::components::fighter::Concealment>,
            Option<&mut super::stage_plugin::NpcPerformance>,
        ),
        With<Fighter>,
    >,
) {
    let dt = super::simulation_dt(clock.as_deref(), time.delta_secs());
    let online = net
        .as_ref()
        .is_some_and(|net| net.status == NetStatus::InRoom);
    let pending = net
        .as_mut()
        .map(|net| std::mem::take(&mut net.pending_status_effects))
        .unwrap_or_default();
    let mut snapshots = net
        .as_mut()
        .map(|net| std::mem::take(&mut net.pending_status_snapshots))
        .unwrap_or_default();
    let mut consumed = vec![false; pending.len()];
    for (
        entity,
        mut status,
        mut health,
        mut qi,
        mut worn,
        death,
        sm,
        combat,
        pose,
        local,
        remote,
        mut concealment,
        npc_performance,
    ) in &mut fighters
    {
        let mut script_items = npc_performance
            .map(|mut performance| std::mem::take(&mut performance.use_items))
            .unwrap_or_default();
        if local.is_some() {
            if let Some(performance) = player_performance.as_deref_mut() {
                script_items.extend(std::mem::take(&mut performance.use_items));
            }
        }

        if !online && health.current > 0.0 {
            for id in script_items {
                if let Some(effect) = resources
                    .as_ref()
                    .and_then(|resources| resources.0.status_effect(id))
                {
                    status.apply(effect);
                } else {
                    warn!("PST use: missing category-2 resource {id}");
                }
            }
        }
        let actor_id = net.as_ref().and_then(|net| {
            net.stock_npc_actors.get(&entity).copied().or_else(|| {
                let id = if local.is_some() {
                    Some(net.player_id.as_str())
                } else {
                    remote.map(|remote| remote.player_id.as_str())
                }?;
                id.strip_prefix("stock-")?.parse::<u32>().ok()
            })
        });
        let player_id = if local.is_some() {
            net.as_ref().map(|net| net.player_id.as_str())
        } else {
            remote.map(|remote| remote.player_id.as_str())
        };
        if health.current <= 0.0 || death.as_ref().is_some_and(|death| death.active) {
            for (index, (target, event)) in pending.iter().enumerate() {
                if target.map_or(actor_id == Some(event.actor_id), |target| target == entity) {
                    consumed[index] = true;
                }
            }
            if let Some(id) = player_id {
                snapshots.remove(id);
            }
            if !online {
                health.max = (health.max - status.applied_max_health_bonus as f32).max(1.0);
                health.current = health.current.min(health.max);
            }
            if status.owns_concealment {
                if let Some(concealment) = concealment.as_deref_mut() {
                    concealment.remaining = 0.0;
                }
            }
            status.clear(&mut worn);
            continue;
        }
        if let Some(snapshot) = player_id.and_then(|id| snapshots.remove(id)) {
            status.effects.active.clear();
            for active in snapshot {
                let definition = active
                    .model
                    .strip_prefix("status:")
                    .and_then(|id| id.parse().ok())
                    .and_then(|id| resources.as_ref()?.0.status_effect(id));
                if let Some(mut definition) = definition {
                    definition.duration = active.remaining.max(0.0);
                    if definition.duration > 0.0 {
                        status.apply(definition);
                    }
                }
            }
        }
        for (index, (target, event)) in pending.iter().enumerate() {
            if target.map_or(actor_id == Some(event.actor_id), |target| target == entity)
                && event.category == 2
            {
                consumed[index] = true;
                if let Some(effect) = resources
                    .as_ref()
                    .and_then(|resources| resources.0.status_effect(event.resource_id))
                {
                    status.apply(effect);
                }
            }
        }
        status.clock += f64::from(dt);
        let now = status.clock;
        let (periodic, damaging_tick) = status.effects.tick_with_damage(now);
        status.pending_periodic_hit |= !online && damaging_tick;
        let instant = std::mem::take(&mut status.pending);
        if !online {
            let maximum_bonus = status.effects.max_health_bonus();
            health.max =
                (health.max + (maximum_bonus - status.applied_max_health_bonus) as f32).max(1.0);
            status.applied_max_health_bonus = maximum_bonus;
            if let Some(concealment) = concealment.as_deref_mut() {
                let remaining = status.effects.concealment_remaining(now);
                if remaining > 0.0 || status.owns_concealment {
                    concealment.remaining = remaining;
                    status.owns_concealment = remaining > 0.0;
                }
            }
            health.current =
                (health.current + (instant.health + periodic.health) as f32).clamp(0.0, health.max);
            qi.current = (qi.current + (instant.anger + periodic.anger) as f32).clamp(0.0, 100.0);
        }
        for (id, name) in [
            (673, "MaxLifUP"),
            (675, "PowUP"),
            (709, "AttrSLOW"),
            (710, "AttrPOIS"),
            (719, "PowUPMax"),
            (739, "PowUPSlf"),
        ] {
            let remaining = status
                .effects
                .active
                .iter()
                .filter(|effect| effect.definition.worn_effect == Some(id))
                .map(|effect| (effect.expires_at - now) as f32)
                .fold(0.0_f32, f32::max);
            if remaining > 0.0 {
                worn.refresh(name, remaining);
                if !status.worn.contains(&name) {
                    status.worn.push(name);
                }
            } else if status.worn.contains(&name) {
                status.worn.retain(|owned| *owned != name);
                let item_poison = if local.is_some() && id == 710 {
                    buffs.as_ref().map_or(0.0, |buffs| buffs.poison_total)
                } else {
                    0.0
                };
                if item_poison > 0.0 {
                    worn.refresh(name, item_poison);
                } else {
                    worn.effects.retain(|effect| effect.name != name);
                }
            }
        }
        if health.current <= 0.0 {
            if status.owns_concealment {
                if let Some(concealment) = concealment.as_deref_mut() {
                    concealment.remaining = 0.0;
                }
            }
            if !online {
                health.max = (health.max - status.applied_max_health_bonus as f32).max(1.0);
            }
            status.clear(&mut worn);
            if let Some(mut death) = death {
                death.active = true;
                death.timer = meteor_combat::death::RESPAWN_DELAY_SECONDS;
            }
            if let Some(mut sm) = sm {
                sm.current = meteor_core::FighterState::Dead;
            }
            if let Some(mut combat) = combat {
                combat.state = meteor_core::FighterState::Dead;
            }
            if let Some(mut pose) = pose {
                pose.pose_id = 0;
                pose.frame = 0.0;
            }
        }
    }
    if let Some(net) = net.as_mut() {
        net.pending_status_effects.extend(
            pending
                .into_iter()
                .enumerate()
                .filter_map(|(index, event)| {
                    (!consumed[index]
                        && event.1.category == 2
                        && event.0.is_none_or(|entity| fighters.contains(entity)))
                    .then_some(event)
                })
                .take(128),
        );
        net.pending_status_snapshots.extend(snapshots);
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fist_self_buff_waits_for_action_then_survives_pose_and_weapon_changes() {
        let (mut app, player, _) = setup();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let character =
            super::super::character_plugin::load_character_asset_for_test(&[root], "p0")
                .expect("original p0 assets");
        let marker = character.poses[&489].action_entry_frame;
        let mut pose = PoseRuntime::idle();
        character.apply_pose(&mut pose, 489, true);
        pose.frame = marker as f32 - 1.0;
        app.world_mut().entity_mut(player).insert(pose);
        app.insert_resource(super::super::character_plugin::CharacterAssetsSet(
            std::collections::HashMap::from([("p0".into(), character)]),
        ));
        app.add_systems(Update, apply_self_attack_effect.before(tick_status_effects));
        step(&mut app, 1);
        assert_eq!(
            app.world()
                .get::<FighterStatusEffects>(player)
                .unwrap()
                .effects
                .attack_bonus(),
            0
        );
        app.world_mut()
            .get_mut::<PoseRuntime>(player)
            .unwrap()
            .frame = marker as f32;
        step(&mut app, 20);
        let status = app.world().get::<FighterStatusEffects>(player).unwrap();
        assert_eq!(status.effects.active.len(), 1);
        assert_eq!(status.effects.attack_bonus(), 40);
        assert!((status.effects.active[0].expires_at - status.clock - 89.0).abs() < 0.001);
        app.world_mut()
            .get_mut::<PoseRuntime>(player)
            .unwrap()
            .pose_id = 0;
        app.world_mut().get_mut::<Fighter>(player).unwrap().weapon = 6;
        step(&mut app, 1778);
        assert_eq!(
            app.world()
                .get::<FighterStatusEffects>(player)
                .unwrap()
                .effects
                .attack_bonus(),
            40
        );
        assert!(app
            .world()
            .get::<WornEffect>(player)
            .unwrap()
            .effects
            .iter()
            .any(|effect| effect.name == "PowUPSlf"));
        step(&mut app, 3);
        assert_eq!(
            app.world()
                .get::<FighterStatusEffects>(player)
                .unwrap()
                .effects
                .attack_bonus(),
            0
        );
        assert!(!app
            .world()
            .get::<WornEffect>(player)
            .unwrap()
            .effects
            .iter()
            .any(|effect| effect.name == "PowUPSlf"));
    }

    #[test]
    fn pst_use_applies_real_resources_to_the_addressed_actor_once() {
        let (mut app, player, npc) = setup();
        app.world_mut()
            .entity_mut(npc)
            .insert(super::super::stage_plugin::NpcPerformance {
                use_items: vec![4, 20, 15],
                ..Default::default()
            });
        app.insert_resource(super::super::stage_plugin::PlayerPerformance {
            use_items: vec![14],
            ..Default::default()
        });
        step(&mut app, 1);
        let npc_health = app.world().get::<Health>(npc).unwrap();
        assert_eq!(npc_health.max, 7999.0);
        assert_eq!(npc_health.current, 6000.0);
        assert_eq!(
            app.world()
                .get::<FighterStatusEffects>(npc)
                .unwrap()
                .effects
                .attack_bonus(),
            25
        );
        assert_eq!(app.world().get::<Health>(player).unwrap().current, 990.0);

        step(&mut app, 1);
        assert_eq!(app.world().get::<Health>(npc).unwrap().current, 6000.0);
        assert_eq!(app.world().get::<Health>(npc).unwrap().max, 7999.0);
        app.world_mut()
            .get_mut::<super::super::stage_plugin::NpcPerformance>(npc)
            .unwrap()
            .use_items
            .push(20);
        step(&mut app, 1);
        assert_eq!(app.world().get::<Health>(npc).unwrap().max, 7999.0);
        app.world_mut().get_mut::<Health>(npc).unwrap().current = 0.0;
        step(&mut app, 1);
        assert_eq!(app.world().get::<Health>(npc).unwrap().max, 1000.0);
    }

    #[test]
    fn pst_use_concealment_expires_and_network_cannot_mint_script_effects() {
        let (mut app, player, npc) = setup();
        app.world_mut()
            .entity_mut(npc)
            .insert(super::super::stage_plugin::NpcPerformance {
                use_items: vec![7],
                ..Default::default()
            });
        step(&mut app, 1);
        assert!(
            app.world()
                .get::<crate::components::fighter::Concealment>(npc)
                .unwrap()
                .remaining
                > 59.0
        );
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::Concealment>(player)
                .unwrap()
                .remaining,
            0.0
        );
        step(&mut app, 1200);
        assert_eq!(
            app.world()
                .get::<crate::components::fighter::Concealment>(npc)
                .unwrap()
                .remaining,
            0.0
        );
        app.world_mut().resource_mut::<NetState>().status = NetStatus::InRoom;
        app.world_mut()
            .get_mut::<super::super::stage_plugin::NpcPerformance>(npc)
            .unwrap()
            .use_items
            .push(4);
        step(&mut app, 1);
        assert_eq!(
            app.world()
                .get::<FighterStatusEffects>(npc)
                .unwrap()
                .effects
                .attack_bonus(),
            0
        );
    }

    fn setup() -> (App, Entity, Entity) {
        let mut app = App::new();
        let table = meteor_asset::res::parse_meteor_res(include_bytes!(
            "../../../../../origin_res/Meteor.res"
        ))
        .unwrap()
        .combat_table();
        app.init_resource::<Time>()
            .init_resource::<NetState>()
            .insert_resource(super::super::combat_plugin::CombatResourceData(table))
            .add_systems(Update, tick_status_effects);
        let mut spawn = |slot| {
            app.world_mut()
                .spawn((
                    Fighter {
                        character_id: 0,
                        player_slot: slot,
                        weapon: 1,
                        weapon_slots: [1, 2],
                        weapon_slot: 0,
                        weapon_slot_variants: [0, 0],
                        weapon_variant: 0,
                    },
                    Health {
                        current: 1000.0,
                        max: 1000.0,
                    },
                    Qi::default(),
                    DeathRespawn::default(),
                ))
                .id()
        };
        let player = spawn(0);
        let npc = spawn(1);
        app.world_mut().entity_mut(player).insert(LocalFighter);
        (app, player, npc)
    }

    fn step(app: &mut App, frames: usize) {
        for _ in 0..frames {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(50));
            app.update();
        }
    }

    #[test]
    fn web_self_buff_snapshot_applies_to_caster_and_remote_with_remaining_time() {
        let (mut app, player, remote) = setup();
        app.world_mut().entity_mut(remote).insert(RemoteFighter {
            player_id: "p2".into(),
        });
        {
            let mut net = app.world_mut().resource_mut::<NetState>();
            net.status = NetStatus::InRoom;
            net.player_id = "p1".into();
            for id in ["p1", "p2"] {
                net.pending_status_snapshots.insert(
                    id.into(),
                    vec![meteor_network::protocol::ItemEffectWire {
                        model: "status:34".into(),
                        remaining: 0.2,
                    }],
                );
            }
        }
        step(&mut app, 1);
        for entity in [player, remote] {
            assert_eq!(
                app.world()
                    .get::<FighterStatusEffects>(entity)
                    .unwrap()
                    .effects
                    .attack_bonus(),
                40
            );
            assert!(app
                .world()
                .get::<WornEffect>(entity)
                .unwrap()
                .effects
                .iter()
                .any(|e| e.name == "PowUPSlf"));
        }
        step(&mut app, 5);
        for entity in [player, remote] {
            assert_eq!(
                app.world()
                    .get::<FighterStatusEffects>(entity)
                    .unwrap()
                    .effects
                    .attack_bonus(),
                0
            );
            assert!(!app
                .world()
                .get::<WornEffect>(entity)
                .unwrap()
                .effects
                .iter()
                .any(|e| e.name == "PowUPSlf"));
        }
    }

    #[test]
    fn status_poison_slow_visual_expiry_and_death_are_per_fighter() {
        let (mut app, player, npc) = setup();
        let table = app
            .world()
            .resource::<super::super::combat_plugin::CombatResourceData>()
            .0
            .clone();
        app.world_mut()
            .get_mut::<FighterStatusEffects>(player)
            .unwrap()
            .apply(table.pose_status_effect(210).unwrap());
        app.world_mut()
            .get_mut::<FighterStatusEffects>(npc)
            .unwrap()
            .apply(table.pose_status_effect(224).unwrap());
        step(&mut app, 1);
        assert_eq!(app.world().get::<Health>(player).unwrap().current, 990.0);
        assert_eq!(app.world().get::<Health>(npc).unwrap().current, 1000.0);
        assert!(app
            .world()
            .get::<WornEffect>(player)
            .unwrap()
            .effects
            .iter()
            .any(|effect| effect.name == "AttrPOIS"));
        assert!(app
            .world()
            .get::<WornEffect>(npc)
            .unwrap()
            .effects
            .iter()
            .any(|effect| effect.name == "AttrSLOW"));
        assert!(
            (app.world()
                .get::<FighterStatusEffects>(npc)
                .unwrap()
                .effects
                .speed_factor(1.0)
                - 0.3)
                .abs()
                < 1e-6
        );
        step(&mut app, 300);
        assert_eq!(app.world().get::<Health>(player).unwrap().current, 950.0);
        assert!(app
            .world()
            .get::<WornEffect>(player)
            .unwrap()
            .effects
            .is_empty());
        step(&mut app, 900);
        assert_eq!(
            app.world()
                .get::<FighterStatusEffects>(npc)
                .unwrap()
                .effects
                .speed_factor(1.0),
            1.0
        );
        app.world_mut().get_mut::<Health>(npc).unwrap().current = 5.0;
        app.world_mut()
            .get_mut::<FighterStatusEffects>(npc)
            .unwrap()
            .apply(table.pose_status_effect(204).unwrap());
        step(&mut app, 1);
        assert!(app.world().get::<DeathRespawn>(npc).unwrap().active);
        step(&mut app, 1);
        assert!(app
            .world()
            .get::<WornEffect>(npc)
            .unwrap()
            .effects
            .is_empty());
    }

    #[test]
    fn stock_event61_updates_target_visual_and_speed_without_predicting_hp() {
        let (mut app, player, npc) = setup();
        let mut net = app.world_mut().resource_mut::<NetState>();
        net.status = NetStatus::InRoom;
        net.player_id = "stock-1".into();
        net.pending_status_effects.push((
            None,
            meteor_network::stock_protocol::StockPropertyEffectEvent {
                actor_id: 1,
                category: 2,
                resource_id: 12,
            },
        ));
        net.pending_status_effects.push((
            Some(npc),
            meteor_network::stock_protocol::StockPropertyEffectEvent {
                actor_id: 2,
                category: 2,
                resource_id: 13,
            },
        ));
        drop(net);
        step(&mut app, 100);
        assert_eq!(app.world().get::<Health>(player).unwrap().current, 1000.0);
        assert_eq!(
            app.world()
                .get::<FighterStatusEffects>(player)
                .unwrap()
                .effects
                .active
                .len(),
            1
        );
        assert!(
            (app.world()
                .get::<FighterStatusEffects>(npc)
                .unwrap()
                .effects
                .speed_factor(1.0)
                - 0.3)
                .abs()
                < 1e-6
        );
    }
}
