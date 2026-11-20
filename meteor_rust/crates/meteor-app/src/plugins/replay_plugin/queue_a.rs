use super::super::network_plugin::{
    scene_ops_from_stock,
    stock_scene::{snapshot_ops, StockSceneObjectMap},
    AuthorityWeaponDrop, NetState, StockSceneItemGrounded, StockSceneItemPickup,
};
use meteor_network::protocol::{RoomMemberWire, SceneOp};
use meteor_network::stock_protocol as stock;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone)]
pub(super) enum RecordedEvent {
    PlayerName(u32, String),
    Chat(u32, String),
    Left(u32),
    Kill(stock::StockKillEvent),
    Death {
        actor: Option<u32>,
        announced: bool,
    },
    Respawn(Option<u32>),
    Timer {
        seconds: u32,
        ended: bool,
    },
    Score(stock::StockTeamScoreEvent),
    Grounded(Vec<stock::StockSceneItemRecord>),
    Pickup(stock::StockSceneTakenEvent),
    Removed(stock::StockSceneItemRemovedEvent),
    WeaponObject {
        actor: u32,
        object: stock::StockWeaponObjectRequest,
    },
    Carried {
        actor: u32,
        category: u32,
        item: u32,
        object: u32,
    },
    SceneState(stock::StockSceneStateEvent),
    SceneOps(stock::StockSceneOpsEvent),
    RoleRate(u32),
    Ready(u32),
    PlayerNotice {
        actor: u32,
        message: u32,
    },
    QuickChat {
        actor: u32,
        message: u32,
    },
    ClearTarget {
        actor: u32,
        target: u32,
    },
}

impl RecordedEvent {
    pub(super) fn decode(id: u32, payload: &[u8]) -> Option<Self> {
        let words = |count: usize| (payload.len() == count * 4).then_some(());
        let word = |index: usize| {
            let offset = index * 4;
            Some(u32::from_le_bytes(
                payload.get(offset..offset + 4)?.try_into().ok()?,
            ))
        };
        Some(match id {
            0 => {
                let (actor, name) = stock::decode_stock_member_joined(payload).ok()?;
                Self::PlayerName(actor, name)
            }
            6 => Self::Left(stock::StockRoleLeftEvent::decode(payload).ok()?.actor_id),
            21 => {
                let (&actor, bytes) = payload.split_first()?;
                let actor = u32::from(actor.checked_sub(1)?);
                let bytes = bytes.split(|byte| *byte == 0).next()?;
                let (message, _, _) = encoding_rs::GBK.decode(bytes);
                Self::Chat(actor, message.into_owned())
            }
            23 => Self::Kill(stock::StockKillEvent::decode(payload).ok()?),
            24 => {
                if !matches!(payload.len(), 0 | 4) {
                    return None;
                }
                Self::Death {
                    actor: None,
                    announced: false,
                }
            }
            26 => {
                words(2)?;
                Self::Timer {
                    seconds: word(0)?,
                    ended: word(1)? != 0,
                }
            }
            29 | 31 => Self::Grounded(stock::StockSceneItemRecord::decode_many(payload).ok()?),
            30 => {
                let event = stock::StockWeaponObjectEvent::decode(payload).ok()?;
                Self::WeaponObject {
                    actor: event.actor_id,
                    object: event.object,
                }
            }
            32 => Self::Pickup(stock::StockSceneTakenEvent::decode(payload).ok()?),
            34 => Self::Score(stock::StockTeamScoreEvent::decode(payload).ok()?),
            44 | 78 => {
                words(1)?;
                Self::Death {
                    actor: Some(word(0)?),
                    announced: id == 44,
                }
            }
            45 => {
                let item = stock::StockCarriedItemEvent::decode(payload).ok()?;
                Self::Carried {
                    actor: item.actor_id,
                    category: item.category,
                    item: item.item_id,
                    object: item.scene_reference,
                }
            }
            46 => Self::Respawn(Some(
                stock::StockRespawnEvent::decode(payload).ok()?.actor_id,
            )),
            48 => Self::SceneState(stock::StockSceneStateEvent::decode(payload).ok()?),
            52 => {
                words(1)?;
                Self::Ready(word(0)?)
            }
            56 => Self::SceneOps(stock::StockSceneOpsEvent::decode(payload).ok()?),
            63 | 79 => {
                words(1)?;
                Self::PlayerNotice {
                    actor: word(0)?,
                    message: if id == 63 { 14 } else { 23 },
                }
            }
            64 => Self::RoleRate(stock::StockRoleRate::decode(payload).ok()?.session_hz()?),
            67 if payload.is_empty() => Self::Respawn(None),
            70 => Self::Removed(stock::StockSceneItemRemovedEvent::decode(payload).ok()?),
            73 => {
                let event = stock::StockQuickChatEvent::decode(payload).ok()?;
                Self::QuickChat {
                    actor: event.actor_id,
                    message: event.chat.message_id()?,
                }
            }
            76 => {
                let event = stock::StockTargetClearEvent::decode(payload).ok()?;
                Self::ClearTarget {
                    actor: event.actor_id,
                    target: event.target_id,
                }
            }
            _ => return None,
        })
    }
}

pub(super) fn actor_key(actor: u32) -> String {
    format!("mrc:{actor}")
}

#[derive(Default)]
pub(super) struct MatchProjection {
    pub(super) dead: HashSet<u32>,
    pub(super) left: HashSet<u32>,
    pub(super) cleared_targets: Vec<(u32, u32)>,
    pub(super) local_life: Option<bool>,
    pub(super) weapon_grants: Vec<(u32, u32)>,
    ready: HashSet<u32>,
    scene_records: HashMap<u32, stock::StockSceneItemRecord>,
    clock: Option<(f32, bool)>,
    weapon_objects: HashMap<u32, VecDeque<String>>,
    quick_chat: Option<HashMap<u32, String>>,
}

impl MatchProjection {
    pub(super) fn clear_net(net: &mut NetState) {
        net.members
            .retain(|member| !member.player_id.starts_with("mrc:"));
        net.remote_actor_names
            .retain(|key, _| !key.starts_with("mrc:"));
        net.remote_kills.retain(|key, _| !key.starts_with("mrc:"));
        net.stock_deaths.retain(|key, _| !key.starts_with("mrc:"));
        if net.player_id.starts_with("mrc:") {
            net.player_id.clear();
            net.mode.clear();
        }
        net.chat_messages.clear();
        net.pending_system_notices.clear();
        net.pending_scene_ops.clear();
        net.pending_stock_scene_grounded.clear();
        net.pending_stock_scene_items.clear();
        net.pending_weapon_drops.clear();
        net.dynamic_weapon_drops.clear();
        net.round_remaining_seconds = None;
        net.round_active = false;
        net.stock_team_wins = [0; 2];
        net.role_uplink_hz = meteor_network::role_state::ROLE_UPLINK_HZ;
    }

    pub(super) fn update_timer(&self, elapsed: f32, net: &mut NetState) {
        net.round_remaining_seconds = self.clock.map(|(deadline, ended)| {
            if ended {
                0.0
            } else {
                (deadline - elapsed).max(0.0)
            }
        });
        if let Some((_, ended)) = self.clock {
            net.round_active = !ended;
        }
    }

    pub(super) fn member(
        &mut self,
        net: &mut NetState,
        actor: u32,
        character_id: u8,
        weapon: u8,
        camp: Option<u32>,
    ) {
        self.left.remove(&actor);
        let player_id = actor_key(actor);
        if let Some(member) = net
            .members
            .iter_mut()
            .find(|member| member.player_id == player_id)
        {
            member.character_id = character_id;
            member.weapon = weapon;
            if let Some(camp) = camp {
                member.team = Some(camp);
            }
        } else {
            net.members.push(RoomMemberWire {
                name: display_name(net, actor),
                player_id,
                character_id,
                weapon,
                team: camp,
                ready: self.ready.contains(&actor),
                connected: true,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply(
        &mut self,
        event: RecordedEvent,
        time: f32,
        local_actor: Option<u32>,
        seeking: bool,
        scene_frame: Option<stock::StockSceneFrame>,
        objects: Option<&StockSceneObjectMap>,
        net: &mut NetState,
    ) {
        match event {
            RecordedEvent::PlayerName(actor, name) => {
                let id = actor_key(actor);
                if let Some(member) = net.members.iter_mut().find(|member| member.player_id == id) {
                    member.name.clone_from(&name);
                }
                net.remote_actor_names.insert(id, name);
            }
            RecordedEvent::Chat(actor, text) => {
                queue_chat(net, actor, text, seeking);
            }
            RecordedEvent::Left(actor) => {
                self.left.insert(actor);
                self.dead.remove(&actor);
                self.ready.remove(&actor);
                if !seeking {
                    let name = display_name(net, actor);
                    super::super::system_notice::queue_net_notice(
                        net,
                        super::super::system_notice::format_leave(&name),
                    );
                }
                let id = actor_key(actor);
                net.members.retain(|member| member.player_id != id);
                net.remote_actor_names.remove(&id);
                net.remote_kills.remove(&id);
                net.stock_deaths.remove(&id);
            }
            RecordedEvent::Kill(event) => {
                *net.remote_kills
                    .entry(actor_key(event.attacker_actor_id))
                    .or_default() += 1;
                if self.dead.insert(event.target_actor_id) {
                    *net.stock_deaths
                        .entry(actor_key(event.target_actor_id))
                        .or_default() += 1;
                }
                if local_actor != Some(event.target_actor_id) && !seeking {
                    let attacker = display_name(net, event.attacker_actor_id);
                    let target = display_name(net, event.target_actor_id);
                    super::super::system_notice::queue_net_notice(
                        net,
                        format!("{attacker} 击败了 {target}"),
                    );
                }
            }
            RecordedEvent::Death { actor, announced } => {
                if let Some(actor) = actor.or(local_actor) {
                    if self.dead.insert(actor) {
                        *net.stock_deaths.entry(actor_key(actor)).or_default() += 1;
                    }
                    if local_actor == Some(actor) {
                        self.local_life = Some(false);
                    }
                    if announced && !seeking {
                        let name = display_name(net, actor);
                        super::super::system_notice::queue_net_notice(
                            net,
                            format!("服务器判定 {name} 死亡"),
                        );
                    }
                }
            }
            RecordedEvent::Respawn(actor) => {
                if let Some(actor) = actor.or(local_actor) {
                    if Some(actor) != local_actor || matches!(event, RecordedEvent::Respawn(None)) {
                        self.dead.remove(&actor);
                        if Some(actor) == local_actor {
                            self.local_life = Some(true);
                        }
                    }
                }
            }
            RecordedEvent::Timer { seconds, ended } => {
                self.clock = Some((time + seconds as f32, ended))
            }
            RecordedEvent::Score(score) => {
                net.stock_team_wins = [score.team1_wins, score.team2_wins]
            }
            RecordedEvent::Grounded(records) => {
                for record in records {
                    if record.category == 1 {
                        if let Some(throws) = self.weapon_objects.get_mut(&record.item_id) {
                            while let Some(reference) = throws.pop_front() {
                                if net.dynamic_weapon_drops.remove(&reference).is_some() {
                                    break;
                                }
                            }
                            if throws.is_empty() {
                                self.weapon_objects.remove(&record.item_id);
                            }
                        }
                    }
                    self.scene_records.insert(record.record_id, record);
                    net.pending_stock_scene_grounded
                        .push(StockSceneItemGrounded {
                            record_id: record.record_id,
                            category: record.category,
                            item_id: record.item_id,
                            position: scene_position(record.role_position(), scene_frame),
                            spawned: true,
                        });
                }
            }
            RecordedEvent::Pickup(event) => {
                if event.category == 1 {
                    self.weapon_grants.push((event.actor_id, event.item_id));
                }
                let position = self
                    .scene_records
                    .remove(&event.record_id)
                    .map(|record| scene_position(record.role_position(), scene_frame));
                net.pending_stock_scene_items.push(StockSceneItemPickup {
                    actor_id: event.actor_id,
                    record_id: event.record_id,
                    category: event.category,
                    item_id: event.item_id,
                    scene_reference: None,
                    position,
                });
            }
            RecordedEvent::Removed(event) => {
                let position = self
                    .scene_records
                    .remove(&event.record_id)
                    .map(|record| scene_position(record.role_position(), scene_frame));
                net.dynamic_weapon_drops
                    .remove(&format!("D_wpStock{}", event.record_id));
                net.pending_stock_scene_items.push(StockSceneItemPickup {
                    actor_id: 0,
                    record_id: event.record_id,
                    category: event.category,
                    item_id: event.item_id,
                    scene_reference: None,
                    position,
                });
            }
            RecordedEvent::WeaponObject { actor, object } => {
                if !seeking {
                    self.queue_weapon_object(time, actor, object, scene_frame, net);
                }
            }
            RecordedEvent::Carried {
                actor,
                category,
                item,
                object,
            } => {
                if let Some(reference) = objects.and_then(|map| map.script_index(object)) {
                    for attribute in ["active", "interactive"] {
                        net.pending_scene_ops.push(SceneOp {
                            kind: "set".into(),
                            reference: reference.into(),
                            field: "attribute".into(),
                            args: vec![attribute.into(), 0.into()],
                        });
                    }
                }
                if category == 1 {
                    self.weapon_grants.push((actor, item));
                }
            }
            RecordedEvent::SceneState(snapshot) => {
                for state in snapshot.objects {
                    if let Some(reference) =
                        objects.and_then(|map| map.script_index(state.reference))
                    {
                        net.pending_scene_ops.extend(snapshot_ops(state, reference));
                    }
                }
            }
            RecordedEvent::SceneOps(mut ops) => {
                ops.operations.retain_mut(|op| {
                    let Some(reference) = objects.and_then(|map| map.script_index(op.reference))
                    else {
                        return false;
                    };
                    op.reference = reference;
                    !seeking || op.kind != stock::STOCK_SCENE_OP_EFFECT
                });
                net.pending_scene_ops.extend(scene_ops_from_stock(&ops));
            }
            RecordedEvent::RoleRate(hz) => net.role_uplink_hz = hz,
            RecordedEvent::Ready(actor) => {
                self.ready.insert(actor);
                if let Some(member) = net
                    .members
                    .iter_mut()
                    .find(|member| member.player_id == actor_key(actor))
                {
                    member.ready = true;
                }
            }
            RecordedEvent::ClearTarget { actor, target } => {
                self.cleared_targets.push((actor, target))
            }
            RecordedEvent::PlayerNotice { actor, message } => {
                if !seeking {
                    let name = display_name(net, actor);
                    let text = match message {
                        14 => format!("服务器已将 {name} 移出对局"),
                        23 => format!("服务器限制 {name} 加入对局"),
                        _ => return,
                    };
                    super::super::system_notice::queue_net_notice(net, text);
                }
            }
            RecordedEvent::QuickChat { actor, message } => {
                let table = self
                    .quick_chat
                    .get_or_insert_with(super::super::system_notice::load_stock_quick_chat);
                let text = table
                    .get(&message)
                    .cloned()
                    .unwrap_or_else(|| format!("快捷发言 {message}"));
                queue_chat(net, actor, text, seeking);
            }
        }
    }

    fn queue_weapon_object(
        &mut self,
        time: f32,
        actor: u32,
        object: stock::StockWeaponObjectRequest,
        frame: Option<stock::StockSceneFrame>,
        net: &mut NetState,
    ) {
        let Some((weapon, variant)) = stock::stock_weapon_from_item_id(object.record_key) else {
            return;
        };
        let Some(model) = super::super::character_plugin::weapon_model_stem(weapon, variant) else {
            return;
        };
        let reference = format!("D_wpMrc{actor}_{}", time.to_bits());
        let position = stock::StockSceneItemRecord {
            record_id: 0,
            item_id: object.record_key,
            category: 1,
            position_tenths: object.position_tenths,
        };
        let mut position = scene_position(position.role_position(), frame);
        position[1] += 6.0;
        let [x, z, y] = object.forward_tenths;
        let direction = bevy::math::Vec3::new(x as f32, y as f32, -(z as f32)) * 0.1;
        let drop = AuthorityWeaponDrop {
            reference: reference.clone(),
            model: model.to_owned(),
            position,
            velocity: Some((direction * 12.0).to_array()),
            settled: false,
            pickup_ready: false,
        };
        let throws = self.weapon_objects.entry(object.record_key).or_default();
        throws.retain(|reference| net.dynamic_weapon_drops.contains_key(reference));
        throws.push_back(reference.clone());
        net.dynamic_weapon_drops.insert(reference, drop.clone());
        net.pending_weapon_drops.push(drop);
    }
}

fn scene_position(
    position: meteor_network::role_state::RoleVec3,
    frame: Option<stock::StockSceneFrame>,
) -> [f32; 3] {
    let position = frame.map_or(position, |frame| frame.position_to_scene(position));
    [position.x, position.y, position.z]
}

fn display_name(net: &NetState, actor: u32) -> String {
    net.remote_actor_names
        .get(&actor_key(actor))
        .cloned()
        .unwrap_or_else(|| format!("玩家 {actor}"))
}

fn queue_chat(net: &mut NetState, actor: u32, text: String, seeking: bool) {
    let name = display_name(net, actor);
    if net.chat_messages.len() == 20 {
        net.chat_messages.pop_front();
    }
    if !seeking {
        super::super::system_notice::queue_net_notice(net, format!("{name}  {text}"));
    }
    net.chat_messages.push_back((name, text));
}

#[cfg(all(test, feature = "original-assets"))]
mod tests {
    use super::*;

    fn demo() -> meteor_asset::mrc::MrcDocument {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../mrcs/佳佳VS竹子/佳佳VS竹子.mrc");
        let file = std::fs::read(path).expect("original recording");
        meteor_asset::mrc::decode_mrc(&file).expect("valid MRC")
    }

    #[test]
    fn routes_every_queue_a_message_in_the_original_demo() {
        let doc = demo();
        let mut runtime = super::super::MrcSessionRuntime::default();
        for entry in &doc.queue_a {
            super::super::route_mrc_session_entry(entry, &mut runtime);
        }
        assert_eq!(runtime.event_counts.len(), 34);
        assert_eq!(runtime.event_counts.values().sum::<u64>(), 25_879);
        assert!(
            runtime.unhandled_counts.is_empty(),
            "{:?}",
            runtime.unhandled_counts
        );
    }

    #[test]
    fn recorded_life_round_and_scene_state_follow_the_demo_time() {
        let doc = demo();
        let mut net = NetState::default();
        let mut state = MatchProjection::default();
        let mut cursor = doc.queue_a.iter().peekable();
        let mut step_to = |t, net: &mut NetState, state: &mut MatchProjection| {
            while let Some(entry) = cursor.next_if(|entry| entry.time <= t) {
                if let Some(event) = RecordedEvent::decode(entry.id, &entry.blob) {
                    state.apply(event, entry.time, Some(4), true, None, None, net);
                }
            }
            state.update_timer(t, net);
        };
        step_to(244.0, &mut net, &mut state);
        assert!(state.dead.contains(&3));
        assert_eq!(net.remote_kills.get(&actor_key(4)), Some(&1));
        assert_eq!(net.role_uplink_hz, 20);
        assert!(!net.chat_messages.is_empty());
        assert!(
            net.pending_system_notices.is_empty(),
            "seek must not replay old notices"
        );
        step_to(250.0, &mut net, &mut state);
        assert!(!state.dead.contains(&3));
        assert!(state.dead.contains(&4));
        assert_eq!(state.local_life, Some(false));
        assert!(!net.pending_stock_scene_grounded.is_empty());
        step_to(253.0, &mut net, &mut state);
        assert!(!state.dead.contains(&4));
        assert_eq!(state.local_life, Some(true));
    }
}
