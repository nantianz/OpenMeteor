use std::collections::{BTreeSet, VecDeque};

pub const TICK_RATE: u32 = 60;

pub const MAX_FRAME_DT: f32 = 0.25;

pub const HOST_EVENT_ID_BASE: u64 = 1_000_000;

pub const MAX_INPUT_SEQUENCE_JUMP: u32 = 10_000;

pub const MAX_DISCRETE_EVENT_HISTORY: usize = 256;

pub const SERVER_OWNED_TYPES: &[&str] = &[
    "match.start",
    "room.state",
    "room.playerLeft",
    "match.reset",
    "room.joined",
];

pub fn is_server_owned(message_type: &str) -> bool {
    SERVER_OWNED_TYPES.contains(&message_type)
}

pub fn clamp_frame_dt(clock_dt: f32) -> f32 {
    clock_dt.clamp(0.0, MAX_FRAME_DT)
}

pub fn host_session_id(player_id: &str) -> String {
    format!("host_{player_id}")
}

#[derive(Debug, Clone)]
pub struct PeerPlayer {
    pub player_id: String,
    pub name: String,
    pub character: Option<String>,
    pub weapon: Option<String>,
    pub team: i32,
    pub battle_pending: bool,
    pub connected: bool,
    pub disconnected_at: Option<f64>,
    pub ready: bool,
    pub health: f32,
}

impl PeerPlayer {
    pub fn new(player_id: impl Into<String>) -> Self {
        Self {
            player_id: player_id.into(),
            name: String::new(),
            character: None,
            weapon: None,
            team: 0,
            battle_pending: true,
            connected: true,
            disconnected_at: None,
            ready: false,
            health: 1000.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemberSnapshot {
    pub id: String,
    pub name: String,
    pub character: Option<String>,
    pub weapon: Option<String>,
    pub team: i32,
    pub battle_pending: bool,
    pub connected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomState {
    Lobby,
    Running,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputStamp {
    pub sequence: u32,

    pub carries_role_state: bool,

    pub carries_role_rate_request: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputAdmission {
    pub input_advanced: bool,
    pub role_state_advanced: bool,
    pub role_rate_advanced: bool,
    pub discrete_allowed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct InputAuthority {
    pub last_input_sequence: u32,
    pub last_role_state_sequence: u32,
    pub last_role_rate_sequence: u32,
    accepted_discrete: BTreeSet<(u32, u16)>,
    accepted_discrete_order: VecDeque<(u32, u16)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputReject {
    SequenceJump,
}

impl InputAuthority {
    pub fn accept(&mut self, input: InputStamp) -> Result<InputAdmission, InputReject> {
        let input_advanced = input.sequence > self.last_input_sequence;
        let role_state_advanced =
            input.carries_role_state && input.sequence > self.last_role_state_sequence;
        let role_rate_advanced =
            input.carries_role_rate_request && input.sequence > self.last_role_rate_sequence;
        if !input_advanced && !role_state_advanced && !role_rate_advanced {
            return Ok(InputAdmission {
                input_advanced: false,
                role_state_advanced: false,
                role_rate_advanced: false,
                discrete_allowed: true,
            });
        }
        if input_advanced
            && input.sequence.saturating_sub(self.last_input_sequence) > MAX_INPUT_SEQUENCE_JUMP
        {
            return Err(InputReject::SequenceJump);
        }
        if input_advanced {
            self.last_input_sequence = input.sequence;
        }
        if role_state_advanced {
            self.last_role_state_sequence = input.sequence;
        }
        if role_rate_advanced {
            self.last_role_rate_sequence = input.sequence;
        }
        Ok(InputAdmission {
            input_advanced,
            role_state_advanced,
            role_rate_advanced,
            discrete_allowed: true,
        })
    }

    pub fn accept_discrete(&mut self, sequence: u32, index: u16) -> bool {
        let key = (sequence, index);
        if !self.accepted_discrete.insert(key) {
            return false;
        }
        self.accepted_discrete_order.push_back(key);
        while self.accepted_discrete_order.len() > MAX_DISCRETE_EVENT_HISTORY {
            if let Some(expired) = self.accepted_discrete_order.pop_front() {
                self.accepted_discrete.remove(&expired);
            }
        }
        true
    }
}

#[derive(Debug, Clone)]
pub struct PredictionJournal<State, Input> {
    capacity: usize,
    pending: VecDeque<(u32, Input)>,
    _state: std::marker::PhantomData<State>,
}

impl<State, Input> PredictionJournal<State, Input> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            pending: VecDeque::new(),
            _state: std::marker::PhantomData,
        }
    }

    pub fn record(&mut self, sequence: u32, input: Input) {
        self.pending.push_back((sequence, input));
        while self.pending.len() > self.capacity {
            self.pending.pop_front();
        }
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn reconcile<F>(
        &mut self,
        acknowledged_sequence: u32,
        mut state: State,
        mut step: F,
    ) -> State
    where
        F: FnMut(&mut State, &Input),
    {
        while self
            .pending
            .front()
            .is_some_and(|(sequence, _)| *sequence <= acknowledged_sequence)
        {
            self.pending.pop_front();
        }
        for (_, input) in &self.pending {
            step(&mut state, input);
        }
        state
    }
}

#[derive(Debug, Clone)]
pub struct HostAuthority {
    pub host_player_id: String,
    pub room_state: RoomState,
    pub stopped: bool,
    pub next_event_id: u64,
    pub players: Vec<PeerPlayer>,
}

impl HostAuthority {
    pub fn new(host_player_id: impl Into<String>) -> Self {
        Self {
            host_player_id: host_player_id.into(),
            room_state: RoomState::Lobby,
            stopped: false,
            next_event_id: HOST_EVENT_ID_BASE,
            players: Vec::new(),
        }
    }

    pub fn sync_players(&mut self, members: &[MemberSnapshot]) {
        if self.stopped {
            return;
        }

        self.players.retain(|p| {
            p.player_id == self.host_player_id || members.iter().any(|m| m.id == p.player_id)
        });
        for member in members {
            let Some(player) = self.players.iter_mut().find(|p| p.player_id == member.id) else {
                let mut p = PeerPlayer::new(member.id.clone());
                p.name = member.name.clone();
                p.character = member.character.clone();
                p.weapon = member.weapon.clone();
                p.team = member.team;
                p.battle_pending = member.battle_pending;
                p.connected = member.connected;

                if self.room_state == RoomState::Running && !p.battle_pending {
                    p.ready = true;
                }
                self.players.push(p);
                continue;
            };
            let was_pending = player.battle_pending;
            if player.battle_pending
                && member.character.is_some()
                && player.character != member.character
            {
                player.character = member.character.clone();
            }
            if player.battle_pending && member.weapon.is_some() && player.weapon != member.weapon {
                player.weapon = member.weapon.clone();
            }
            if member.team == 1 || member.team == 2 {
                player.team = member.team;
            }
            player.battle_pending = member.battle_pending;
            player.connected = member.connected;
            player.disconnected_at = if player.connected {
                None
            } else {
                Some(player.disconnected_at.unwrap_or(0.0))
            };
            if self.room_state == RoomState::Running && !player.battle_pending {
                player.ready = true;
            }
            if self.room_state == RoomState::Running && was_pending && !player.battle_pending {
                if player.health <= 0.0 {
                    player.health = 1000.0;
                }
            }
        }
    }

    pub fn should_forward(message_type: &str) -> bool {
        !is_server_owned(message_type)
    }

    pub fn mark_peer_disconnected(&mut self, player_id: &str) {
        if self.stopped {
            return;
        }
        let session = host_session_id(player_id);
        if let Some(player) = self.players.iter_mut().find(|p| p.player_id == player_id) {
            let _ = session;
            player.connected = false;
            player.disconnected_at = Some(now_millis());
        }
    }

    pub fn mark_peer_reconnected(&mut self, player_id: &str) {
        if self.stopped {
            return;
        }
        if let Some(player) = self.players.iter_mut().find(|p| p.player_id == player_id) {
            player.connected = true;
            player.disconnected_at = None;
        }
    }

    pub fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
    }

    pub fn start_match(&mut self) {
        if self.stopped {
            return;
        }
        self.room_state = RoomState::Running;
        self.next_event_id = HOST_EVENT_ID_BASE;
        for player in &mut self.players {
            if !player.battle_pending {
                player.ready = true;
            }
        }
    }

    pub fn end_match(&mut self) {
        self.room_state = RoomState::Ended;
        self.stop();
    }
}

fn now_millis() -> f64 {
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, team: i32, pending: bool) -> MemberSnapshot {
        MemberSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            character: None,
            weapon: None,
            team,
            battle_pending: pending,
            connected: true,
        }
    }

    #[test]
    fn tick_rate_and_frame_clamp() {
        assert_eq!(TICK_RATE, 60);
        assert_eq!(MAX_FRAME_DT, 0.25);

        assert!((clamp_frame_dt(0.016) - 0.016).abs() < 1e-5);

        assert_eq!(clamp_frame_dt(1.0), 0.25);

        assert_eq!(clamp_frame_dt(-0.1), 0.0);
    }

    #[test]
    fn server_owned_types() {
        assert!(is_server_owned("match.start"));
        assert!(is_server_owned("room.state"));
        assert!(is_server_owned("room.playerLeft"));
        assert!(is_server_owned("match.reset"));
        assert!(is_server_owned("room.joined"));
        assert!(!is_server_owned("input"));
        assert!(!is_server_owned("match.end"));

        assert!(!HostAuthority::should_forward("match.start"));
        assert!(HostAuthority::should_forward("input"));
    }

    #[test]
    fn session_id_prefix() {
        assert_eq!(host_session_id("p1"), "host_p1");
    }

    #[test]
    fn sync_players_adds_and_removes() {
        let mut host = HostAuthority::new("host1");
        let members = vec![member("host1", 1, false), member("p2", 2, false)];
        host.sync_players(&members);
        assert_eq!(host.players.len(), 2);
        assert!(host.players.iter().any(|p| p.player_id == "p2"));

        let members = vec![member("host1", 1, false)];
        host.sync_players(&members);
        assert_eq!(host.players.len(), 1);
    }

    #[test]
    fn sync_players_pending_weapon_update() {
        let mut host = HostAuthority::new("host1");
        let mut m = member("p2", 2, true);
        m.weapon = Some("sword".into());
        host.sync_players(&[m]);

        let mut m = member("p2", 2, true);
        m.weapon = Some("blade".into());
        host.sync_players(&[m]);
        let p2 = host.players.iter().find(|p| p.player_id == "p2").unwrap();
        assert_eq!(p2.weapon.as_deref(), Some("blade"));
    }

    #[test]
    fn running_join_ready_and_spawn() {
        let mut host = HostAuthority::new("host1");
        host.start_match();

        let m = member("p2", 2, false);
        host.sync_players(&[m]);
        let p2 = host.players.iter().find(|p| p.player_id == "p2").unwrap();
        assert!(p2.ready);
        assert!(p2.health > 0.0);
    }

    #[test]
    fn disconnect_and_reconnect() {
        let mut host = HostAuthority::new("host1");
        let members = vec![member("host1", 1, false), member("p2", 2, false)];
        host.sync_players(&members);
        host.mark_peer_disconnected("p2");
        let p2 = host.players.iter().find(|p| p.player_id == "p2").unwrap();
        assert!(!p2.connected);
        assert!(p2.disconnected_at.is_some());
        host.mark_peer_reconnected("p2");
        let p2 = host.players.iter().find(|p| p.player_id == "p2").unwrap();
        assert!(p2.connected);
        assert!(p2.disconnected_at.is_none());
    }

    #[test]
    fn stop_blocks_sync() {
        let mut host = HostAuthority::new("host1");
        host.stop();
        host.sync_players(&[member("p2", 2, false)]);
        assert!(host.players.is_empty());
        host.mark_peer_disconnected("p2");
    }

    #[test]
    fn event_id_space_offset() {
        let mut host = HostAuthority::new("host1");
        host.start_match();
        assert_eq!(host.next_event_id, HOST_EVENT_ID_BASE);
    }

    #[test]
    fn input_watermarks_keep_role_state_independent() {
        let mut authority = InputAuthority::default();
        let first = authority
            .accept(InputStamp {
                sequence: 8,
                carries_role_state: true,
                carries_role_rate_request: true,
            })
            .unwrap();
        assert_eq!(
            first,
            InputAdmission {
                input_advanced: true,
                role_state_advanced: true,
                role_rate_advanced: true,
                discrete_allowed: true,
            }
        );

        authority
            .accept(InputStamp {
                sequence: 10,
                carries_role_state: false,
                carries_role_rate_request: false,
            })
            .unwrap();
        let delayed_state = authority
            .accept(InputStamp {
                sequence: 9,
                carries_role_state: true,
                carries_role_rate_request: false,
            })
            .unwrap();
        assert!(!delayed_state.input_advanced);
        assert!(delayed_state.role_state_advanced);
        assert_eq!(authority.last_input_sequence, 10);
        assert_eq!(authority.last_role_state_sequence, 9);
        let delayed_rate = authority
            .accept(InputStamp {
                sequence: 9,
                carries_role_state: false,
                carries_role_rate_request: true,
            })
            .unwrap();
        assert!(!delayed_rate.input_advanced);
        assert!(!delayed_rate.role_state_advanced);
        assert!(delayed_rate.role_rate_advanced);
        assert_eq!(authority.last_role_rate_sequence, 9);
    }

    #[test]
    fn stale_envelopes_still_admit_one_discrete_claim() {
        let mut authority = InputAuthority::default();
        authority
            .accept(InputStamp {
                sequence: 3,
                carries_role_state: false,
                carries_role_rate_request: false,
            })
            .unwrap();
        let stale = authority
            .accept(InputStamp {
                sequence: 2,
                carries_role_state: false,
                carries_role_rate_request: false,
            })
            .unwrap();
        assert!(stale.discrete_allowed);
        assert!(authority.accept_discrete(2, 0));
        assert!(!authority.accept_discrete(2, 0));
        assert!(authority.accept_discrete(2, 1));
    }

    #[test]
    fn input_sequence_jump_is_rejected_without_advancing_state() {
        let mut authority = InputAuthority::default();
        authority
            .accept(InputStamp {
                sequence: 4,
                carries_role_state: false,
                carries_role_rate_request: false,
            })
            .unwrap();
        assert_eq!(
            authority.accept(InputStamp {
                sequence: 4 + MAX_INPUT_SEQUENCE_JUMP + 1,
                carries_role_state: false,
                carries_role_rate_request: false,
            }),
            Err(InputReject::SequenceJump)
        );
        assert_eq!(authority.last_input_sequence, 4);
    }

    #[test]
    fn prediction_reconcile_drops_acknowledged_inputs_then_replays_tail() {
        let mut journal = PredictionJournal::<i32, i32>::new(3);
        journal.record(1, 2);
        journal.record(2, 3);
        journal.record(3, 5);
        let reconciled = journal.reconcile(1, 10, |state, input| *state += input);
        assert_eq!(reconciled, 18);
        assert_eq!(journal.pending_len(), 2);

        journal.record(4, 7);
        journal.record(5, 11);
        assert_eq!(journal.pending_len(), 3);
        let reconciled = journal.reconcile(3, 20, |state, input| *state += input);
        assert_eq!(reconciled, 38);
        assert_eq!(journal.pending_len(), 2);
    }
}
