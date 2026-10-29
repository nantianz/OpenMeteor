use super::*;
use meteor_core::{Quat, Vec3 as Point};
use meteor_scene::scene_script_runtime::{PstValue, SceneItemInstance, SceneScriptRuntime};

#[derive(Debug, Clone)]
pub(super) struct PendingTransfer {
    pub serial: u32,
    pub message: Message,
    pub resend_at: f32,
}

struct Portal {
    name: String,
    min: [f32; 3],
    max: [f32; 3],
}

impl Portal {
    fn contains(&self, p: RoleVec3) -> bool {
        p.x >= self.min[0] - 0.5
            && p.x <= self.max[0] + 0.5
            && p.y >= self.min[1] - 0.5
            && p.y <= self.max[1] + 1.8
            && p.z >= self.min[2] - 0.5
            && p.z <= self.max[2] + 0.5
    }
}

struct Actor {
    id: String,
    team: i64,
    deaths: u32,
    alive: bool,
}

pub(super) struct SceneTransfers {
    epoch: (u32, u32),
    runtime: SceneScriptRuntime,
    portals: Vec<Portal>,
    destinations: HashMap<String, ([f32; 3], f32)>,
    actors: Vec<Option<Actor>>,
    cooldowns: HashMap<(usize, usize), f32>,
    next_scan: f32,
    serial: u32,
}

impl std::fmt::Debug for SceneTransfers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneTransfers")
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

fn rotate(q: Quat, p: Point) -> Point {
    let q = q.normalize();
    let v = Point::new(q.x, q.y, q.z);
    let t = v.cross(&p) * 2.0;
    p + t * q.w + v.cross(&t)
}

fn world(p: Point) -> [f32; 3] {
    let c = meteor_scene::stage_loader::ARENA_CENTER;
    let s = meteor_core::constants::MODEL_SCALE;
    [(p.x - c[0]) * s, (p.z - c[2]) * s, -(p.y - c[1]) * s]
}

pub(super) fn validate(root: Option<&Path>, room: &Room) -> Result<(), String> {
    let root = root.ok_or("SN22 transfer authority requires installed stage resources")?;
    load(root, room).map(|_| ())
}

fn load(root: &Path, room: &Room) -> Result<SceneTransfers, String> {
    let read =
        |path: &str| environment::read_resource(root, path).map_err(|e| format!("{path}: {e}"));
    let text = |path: &str| -> Result<String, String> {
        Ok(encoding_rs::GB18030.decode(&read(path)?).0.into_owned())
    };
    let desc = meteor_asset::des::parse_stage_description(&text("sn22/sn22.des")?);
    let mut source = Vec::new();
    for name in ["Def.h", "box.pst", "desk.pst", "chair.pst", "jug.pst"] {
        source.push(text(&format!("PScriptLib/{name}"))?);
    }
    source.push(text("sn22/sn22.pst")?);
    let instances = desc
        .dummies
        .iter()
        .enumerate()
        .map(|(index, d)| SceneItemInstance {
            index,
            name: d.name.clone(),
            props: [
                ("active".into(), PstValue::Num(1.0)),
                ("state".into(), PstValue::Num(3.0)),
            ]
            .into_iter()
            .collect(),
        })
        .collect();
    let mut runtime = SceneScriptRuntime::load(&source.join("\n"), instances, 250_000)?;
    runtime
        .host_mut()
        .set_game_mode(i64::from(mode_rule_id(&room.mode).unwrap_or(1)));
    runtime.start()?;
    let model =
        meteor_asset::gmdl::decode_gmdl(&read("cmodel/tp001.gmb")?).map_err(|e| e.to_string())?;
    let vertices = model
        .objects
        .iter()
        .flat_map(|o| o.positions.chunks_exact(3))
        .map(|v| Point::new(v[0], v[1], v[2]))
        .collect::<Vec<_>>();
    if vertices.is_empty() {
        return Err("tp001 has no trigger geometry".into());
    }
    let mut portals = Vec::new();
    let mut destinations = HashMap::new();
    for d in &desc.dummies {
        let forward = rotate(d.quaternion, Point::new(0.0, -1.0, 0.0));
        destinations.insert(
            d.name.clone(),
            (world(d.position), forward.x.atan2(-forward.y)),
        );
        if d.model.as_deref() != Some("tp001") || !runtime.has_item_callback(&d.name, "OnTouch") {
            continue;
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &vertices {
            let p = world(d.position + rotate(d.quaternion, *p * d.scale));
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        portals.push(Portal {
            name: d.name.clone(),
            min,
            max,
        });
    }
    Ok(SceneTransfers {
        epoch: (room.match_epoch, room.round_epoch),
        runtime,
        portals,
        destinations,
        actors: (0..16).map(|_| None).collect(),
        cooldowns: HashMap::new(),
        next_scan: 0.0,
        serial: 0,
    })
}

pub(super) fn update(
    states: &mut HashMap<String, SceneTransfers>,
    root: Option<&Path>,
    room: &mut Room,
) -> Vec<Message> {
    if room.stage != "sn22"
        || !room
            .round
            .as_ref()
            .is_some_and(|r| r.phase == RoundPhase::Active)
    {
        return Vec::new();
    }
    let epoch = (room.match_epoch, room.round_epoch);
    if states.get(&room.id).is_none_or(|s| s.epoch != epoch) {
        let Some(root) = root else { return Vec::new() };
        match load(root, room) {
            Ok(state) => {
                states.insert(room.id.clone(), state);
            }
            Err(error) => {
                log::error!("SN22 transfer resources: {error}");
                return Vec::new();
            }
        }
    }
    let state = states.get_mut(&room.id).expect("loaded transfer runtime");
    let mut events = Vec::new();

    for (slot, entry) in state.actors.iter_mut().enumerate() {
        let Some(actor) = entry else { continue };
        let current = room
            .combatants
            .get(&actor.id)
            .filter(|_| room.members.contains_key(&actor.id));
        let event = match current {
            None if actor.alive => Some(201),
            Some(c) if actor.alive && (!c.alive() || c.deaths != actor.deaths) => Some(202),
            _ => None,
        };
        if let Some(event) = event {
            state
                .runtime
                .host_mut()
                .set_character_team(slot as i64, actor.team);
            if let Err(e) = state.runtime.call(
                "Scene_OnCharacterEvent",
                &[PstValue::Num(slot as f64), PstValue::Num(event as f64)],
            ) {
                log::error!("SN22 character event: {e}");
            }
            room.pending_transfers.remove(&actor.id);
        }
        if let Some(c) = current {
            actor.alive = c.alive();
            actor.deaths = c.deaths;
            actor.team = i64::from(c.team.unwrap_or(0));
        } else {
            *entry = None;
            state.cooldowns.retain(|(_, s), _| *s != slot);
        }
    }
    for id in &room.order {
        if state.actors.iter().flatten().any(|a| &a.id == id) {
            continue;
        }
        let Some(c) = room.combatants.get(id) else {
            continue;
        };
        if let Some(entry) = state.actors.iter_mut().find(|a| a.is_none()) {
            *entry = Some(Actor {
                id: id.clone(),
                team: i64::from(c.team.unwrap_or(0)),
                deaths: c.deaths,
                alive: c.alive(),
            });
        }
    }
    for pending in room.pending_transfers.values_mut() {
        if room.elapsed >= pending.resend_at {
            events.push(pending.message.clone());
            pending.resend_at = room.elapsed + 1.0;
        }
    }
    if room.elapsed < state.next_scan {
        return events;
    }
    state.next_scan = room.elapsed + 0.12;
    state.cooldowns.retain(|_, expires| *expires > room.elapsed);
    for (slot, actor) in state
        .actors
        .iter()
        .enumerate()
        .filter_map(|(s, a)| a.as_ref().map(|a| (s, a)))
    {
        if !actor.alive
            || room.pending_transfers.contains_key(&actor.id)
            || room.pending_joiners.contains(&actor.id)
            || !room.members.get(&actor.id).is_some_and(|m| m.connected)
        {
            continue;
        }
        let Some(role) = room.role_states.get(&actor.id) else {
            continue;
        };
        let position = role.position;
        state
            .runtime
            .host_mut()
            .set_character_team(slot as i64, actor.team);
        for (index, portal) in state.portals.iter().enumerate() {
            if !portal.contains(position) || state.cooldowns.contains_key(&(index, slot)) {
                continue;
            }
            state.cooldowns.insert((index, slot), room.elapsed + 0.6);
            if let Err(e) =
                state
                    .runtime
                    .call_item(&portal.name, "OnTouch", &[PstValue::Num(slot as f64)])
            {
                log::error!("SN22 OnTouch: {e}");
                continue;
            }
            let mut transferred = false;
            for request in state.runtime.host_mut().take_transfer_requests() {
                if request.character_id != slot as i64 {
                    continue;
                }
                let Some(&(position, yaw)) = state.destinations.get(&request.destination) else {
                    continue;
                };
                let p = RoleVec3 {
                    x: position[0],
                    y: position[1],
                    z: position[2],
                };
                if let Some(role) = room.role_states.get_mut(&actor.id) {
                    role.position = p;
                    role.velocity = RoleVec3::ZERO;
                    role.forward = RoleVec3 {
                        x: yaw.sin(),
                        y: 0.0,
                        z: yaw.cos(),
                    };
                }
                if let Some(c) = room.combatants.get_mut(&actor.id) {
                    c.position = p;
                    c.yaw = yaw;
                    c.reset_geometry();
                }
                state.serial = state.serial.wrapping_add(1).max(1);
                let message = Message::SceneTransfer {
                    room_id: room.id.clone(),
                    match_epoch: room.match_epoch,
                    round_epoch: room.round_epoch,
                    serial: state.serial,
                    tick: room.authority_tick,
                    player_id: actor.id.clone(),
                    position,
                    yaw,
                };
                room.pending_transfers.insert(
                    actor.id.clone(),
                    PendingTransfer {
                        serial: state.serial,
                        message: message.clone(),
                        resend_at: room.elapsed + 1.0,
                    },
                );
                events.push(message);
                transferred = true;
            }
            if transferred {
                break;
            }
        }
    }
    events
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_state::encode_request1_state;

    fn setup(mode: &str, count: u64) -> RoomServer {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let mut s = RoomServer::new();
        s.origin_res = Some(root);
        for conn in 1..=count {
            s.connect(conn);
            s.handle(
                conn,
                Message::Hello {
                    name: format!("player{conn}"),
                    resume_token: None,
                    character_id: 0,
                    weapon: 6,
                },
            );
            if conn == 1 {
                s.handle(
                    conn,
                    Message::RoomCreate {
                        mode: mode.into(),
                        stage: "sn22".into(),
                        name: String::new(),
                        character_id: 0,
                        weapon: 6,
                        room_name: String::new(),
                        password: String::new(),
                        rules: Default::default(),
                    },
                );
            } else {
                s.handle(
                    conn,
                    Message::RoomJoin {
                        room_id: "room1".into(),
                        mode: None,
                        stage: None,
                        name: format!("player{conn}"),
                        character_id: 0,
                        weapon: 6,
                        password: String::new(),
                    },
                );
                s.handle(conn, Message::RoomReady { ready: true });
            }
        }
        let start = s.handle(1, Message::RoomStart);
        assert!(
            start
                .to_room
                .iter()
                .any(|m| matches!(m, Message::MatchStart { .. })),
            "{start:?}"
        );
        s.advance_authority(0.02, 0.0);
        assert!(s.scene_transfers.contains_key("room1"));
        s
    }

    fn portal(s: &RoomServer, name: &str) -> RoleVec3 {
        let p = s.scene_transfers["room1"]
            .portals
            .iter()
            .find(|p| p.name == name)
            .unwrap();
        RoleVec3 {
            x: (p.min[0] + p.max[0]) * 0.5,
            y: (p.min[1] + p.max[1]) * 0.5,
            z: (p.min[2] + p.max[2]) * 0.5,
        }
    }

    fn upload(s: &mut RoomServer, conn: u64, seq: u32, p: RoleVec3) {
        let room = &s.rooms["room1"];
        let mut state = decode_request1_state(&[0; 20]).unwrap();
        state.position = p;
        s.handle_v4_input(
            conn,
            V4Input {
                seq,
                match_epoch: Some(room.match_epoch),
                round_epoch: Some(room.round_epoch),
                role_state: encode_request1_state(state).to_vec(),
                ..Default::default()
            },
        );
    }

    fn tick(s: &mut RoomServer) -> Vec<(u64, Message)> {
        (0..8)
            .flat_map(|_| s.advance_authority(0.02, 0.0).events)
            .collect()
    }

    fn ack(s: &mut RoomServer, conn: u64) {
        let room = &s.rooms["room1"];
        let serial = room.pending_transfers[&format!("p{conn}")].serial;
        s.handle(
            conn,
            Message::SceneTransferAck {
                room_id: "room1".into(),
                match_epoch: room.match_epoch,
                round_epoch: room.round_epoch,
                serial,
            },
        );
    }

    #[test]
    fn sn22_guest_transfer_updates_authority_and_blocks_inflight_inputs_until_applied() {
        let mut s = setup("leader", 2);
        let source = portal(&s, "D_tpEC01");
        upload(&mut s, 2, 1, source);
        let events = tick(&mut s);
        let room = &s.rooms["room1"];
        let position = room.role_states["p2"].position;
        let expected = s.scene_transfers["room1"].destinations["D_tpED01"].0;
        assert_eq!([position.x, position.y, position.z], expected);
        assert_eq!(room.combatants["p2"].position, position);
        let packet = room.pending_transfers["p2"].message.clone();
        for conn in [1, 2] {
            assert!(events.iter().any(|(id, m)| *id == conn && m == &packet));
        }
        assert_eq!(
            Message::from_v4_binary(&packet.to_v4_binary().unwrap()).unwrap(),
            packet
        );
        assert_eq!(Message::from_json(&packet.to_json()).unwrap(), packet);
        let denied = s.handle(1, packet);
        assert!(matches!(&denied.to_sender[0], Message::Error { code, .. } if code == "FORBIDDEN"));
        upload(&mut s, 2, 2, source);
        assert_eq!(s.rooms["room1"].role_states["p2"].position, position);
        let serial = s.rooms["room1"].pending_transfers["p2"].serial;
        for (conn, epoch) in [(1, 1), (2, 99)] {
            s.handle(
                conn,
                Message::SceneTransferAck {
                    room_id: "room1".into(),
                    match_epoch: 1,
                    round_epoch: epoch,
                    serial,
                },
            );
            assert!(s.rooms["room1"].pending_transfers.contains_key("p2"));
        }
        ack(&mut s, 2);
        assert!(!s.rooms["room1"].pending_transfers.contains_key("p2"));
        let moved = RoleVec3 {
            x: position.x + 1.0,
            ..position
        };
        upload(&mut s, 2, 3, moved);
        assert!((s.rooms["room1"].role_states["p2"].position.x - moved.x).abs() < 0.02);
    }

    #[test]
    fn sn22_original_two_slots_return_gate_and_death_exit_release() {
        let mut s = setup("leader", 3);
        for conn in [1, 2, 3] {
            let p = portal(&s, "D_tpEC01");
            upload(&mut s, conn, 1, p);
        }
        tick(&mut s);
        assert_eq!(s.rooms["room1"].pending_transfers.len(), 2);
        assert!(!s.rooms["room1"].pending_transfers.contains_key("p3"));
        ack(&mut s, 1);
        ack(&mut s, 2);
        let exit = portal(&s, "D_tpEC17");
        upload(&mut s, 1, 2, exit);
        tick(&mut s);
        assert!(
            !s.rooms["room1"].pending_transfers.contains_key("p1"),
            "two occupants cannot leave"
        );
        let c = s
            .rooms
            .get_mut("room1")
            .unwrap()
            .combatants
            .get_mut("p2")
            .unwrap();
        c.health = 0.0;
        c.deaths += 1;
        tick(&mut s);
        s.handle(2, Message::RoomLeave);

        upload(&mut s, 3, 2, RoleVec3::ZERO);
        for _ in 0..5 {
            tick(&mut s);
        }
        assert!(
            s.rooms["room1"].pending_transfers.contains_key("p1"),
            "one survivor may return; death+exit releases once"
        );
        assert_eq!(
            s.scene_transfers["room1"].actors[0].as_ref().unwrap().id,
            "p1"
        );
        assert_eq!(
            s.scene_transfers["room1"].actors[2].as_ref().unwrap().id,
            "p3",
            "leave never renumbers another role"
        );
    }

    #[test]
    fn sn22_team_callback_rules_and_round_reset() {
        let mut s = setup("deathmatch", 3);

        s.scene_transfers
            .get_mut("room1")
            .unwrap()
            .portals
            .retain(|p| !p.name.starts_with("D_tpEC"));
        for (id, team) in [("p1", 1), ("p2", 2), ("p3", 1)] {
            s.rooms
                .get_mut("room1")
                .unwrap()
                .combatants
                .get_mut(id)
                .unwrap()
                .team = Some(team);
        }
        let red = portal(&s, "D_tpAC01");
        upload(&mut s, 2, 1, red);
        tick(&mut s);
        assert!(
            s.rooms["room1"].pending_transfers.is_empty(),
            "wrong team rejected by PST: {:?}",
            s.rooms["room1"].pending_transfers
        );
        upload(&mut s, 1, 1, red);
        upload(&mut s, 3, 1, red);
        tick(&mut s);
        assert!(s.rooms["room1"].pending_transfers.contains_key("p1"));
        assert!(
            !s.rooms["room1"].pending_transfers.contains_key("p3"),
            "one slot per team"
        );
        let blue = portal(&s, "D_tpBC01");
        upload(&mut s, 2, 2, blue);
        tick(&mut s);
        assert!(s.rooms["room1"].pending_transfers.contains_key("p2"));
        ack(&mut s, 1);
        ack(&mut s, 2);
        let exit = portal(&s, "D_tpAC09");
        upload(&mut s, 1, 2, exit);
        tick(&mut s);
        assert!(!s.rooms["room1"].pending_transfers.contains_key("p1"));
        RoomServer::start_round(s.rooms.get_mut("room1").unwrap(), 2);
        upload(&mut s, 1, 3, RoleVec3::ZERO);
        upload(&mut s, 2, 3, RoleVec3::ZERO);
        assert!(
            s.rooms["room1"].role_states.get("p3").is_none(),
            "old-round position is not a contact"
        );
        upload(&mut s, 3, 2, red);
        tick(&mut s);
        assert!(
            s.rooms["room1"].pending_transfers.contains_key("p3"),
            "new round starts with vacant slots"
        );
        assert_eq!(s.scene_transfers["room1"].epoch, (1, 2));
    }

    #[test]
    fn sn22_host_departure_preserves_guest_slot_and_releases_occupancy_once() {
        let mut s = setup("leader", 3);
        let entrance = portal(&s, "D_tpEC01");
        upload(&mut s, 1, 1, entrance);
        upload(&mut s, 2, 1, entrance);
        tick(&mut s);
        ack(&mut s, 1);
        ack(&mut s, 2);
        s.handle(1, Message::RoomLeave);
        tick(&mut s);
        assert!(s.scene_transfers["room1"].actors[0].is_none());
        assert_eq!(
            s.scene_transfers["room1"].actors[1].as_ref().unwrap().id,
            "p2"
        );
        let exit = portal(&s, "D_tpEC17");
        upload(&mut s, 2, 2, exit);
        tick(&mut s);
        assert!(s.rooms["room1"].pending_transfers.contains_key("p2"));
        let serial = s.rooms["room1"].pending_transfers["p2"].serial;
        s.handle(
            2,
            Message::SceneTransferAck {
                room_id: "another-room".into(),
                match_epoch: 1,
                round_epoch: 1,
                serial,
            },
        );
        assert!(s.rooms["room1"].pending_transfers.contains_key("p2"));
        let retry = (0..8).flat_map(|_| tick(&mut s)).collect::<Vec<_>>();
        assert!(retry.iter().any(|(id, m)| *id == 2
            && matches!(m, Message::SceneTransfer { serial: n, .. } if *n == serial)));
        ack(&mut s, 2);
        assert!(s.rooms["room1"].pending_transfers.is_empty());
    }
}
