use std::collections::BTreeMap;

pub const DEFAULT_DISCONNECT_GRACE_MS: u64 = 4_000;

pub const INPUT_BACKLOG_LIMIT: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pRole {
    Host,
    Peer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportRoute {
    Control,
    State,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum P2pLinkState {
    Waiting,
    Connecting,
    Direct,
    Failed,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerLink {
    pub player_id: String,
    pub control_open: bool,
    pub state_open: bool,
    pub control_buffered: usize,
    pub state_buffered: usize,
    pub disconnected_at_ms: Option<u64>,
}

impl PeerLink {
    pub fn new(player_id: impl Into<String>) -> Self {
        Self {
            player_id: player_id.into(),
            control_open: false,
            state_open: false,
            control_buffered: 0,
            state_buffered: 0,
            disconnected_at_ms: None,
        }
    }

    pub fn direct_open(&self) -> bool {
        self.control_open || self.state_open
    }

    pub fn route_for(&self, message_type: &str) -> TransportRoute {
        if is_state_stream(message_type) && self.state_open {
            TransportRoute::State
        } else if self.control_open {
            TransportRoute::Control
        } else {
            TransportRoute::Relay
        }
    }

    pub fn drops_for_backpressure(&self, message_type: &str) -> bool {
        self.route_for(message_type) == TransportRoute::State
            && self.state_buffered > INPUT_BACKLOG_LIMIT
    }

    pub fn input_healthy(&self, relay_buffered: usize) -> bool {
        match self.route_for("input") {
            TransportRoute::State => self.state_buffered <= INPUT_BACKLOG_LIMIT,
            TransportRoute::Control => self.control_buffered <= INPUT_BACKLOG_LIMIT,
            TransportRoute::Relay => relay_buffered <= INPUT_BACKLOG_LIMIT,
        }
    }

    pub fn mark_disconnected(&mut self, now_ms: u64) {
        self.disconnected_at_ms.get_or_insert(now_ms);
    }

    pub fn mark_reconnected(&mut self) {
        self.disconnected_at_ms = None;
    }

    pub fn disconnect_grace_expired(&self, now_ms: u64, grace_ms: u64) -> bool {
        self.disconnected_at_ms
            .is_some_and(|at| now_ms.saturating_sub(at) >= grace_ms)
    }
}

#[derive(Debug, Clone)]
pub struct P2pCoordinator {
    pub role: P2pRole,
    pub active: bool,
    pub host_channel_lost: bool,
    pub links: BTreeMap<String, PeerLink>,
    pub link_state: P2pLinkState,
}

impl P2pCoordinator {
    pub fn new(role: P2pRole) -> Self {
        Self {
            role,
            active: false,
            host_channel_lost: false,
            links: BTreeMap::new(),
            link_state: P2pLinkState::Waiting,
        }
    }

    pub fn begin_connecting(&mut self, supported: bool) {
        self.link_state = if supported {
            P2pLinkState::Connecting
        } else {
            P2pLinkState::Unsupported
        };
    }

    pub fn start(&mut self) {
        self.active = true;
        if self.links.values().any(PeerLink::direct_open) {
            self.link_state = P2pLinkState::Direct;
        }
    }

    pub fn add_link(&mut self, player_id: impl Into<String>) -> &mut PeerLink {
        let player_id = player_id.into();
        self.links
            .entry(player_id.clone())
            .or_insert_with(|| PeerLink::new(player_id))
    }

    pub fn expire_disconnects(&mut self, now_ms: u64, grace_ms: u64) -> Vec<String> {
        let expired = self
            .links
            .values()
            .filter(|link| link.disconnect_grace_expired(now_ms, grace_ms))
            .map(|link| link.player_id.clone())
            .collect::<Vec<_>>();
        if expired.is_empty() {
            return expired;
        }
        if self.role == P2pRole::Peer {
            self.host_channel_lost = true;
        }
        self.link_state = P2pLinkState::Failed;
        for player_id in &expired {
            if let Some(link) = self.links.get_mut(player_id) {
                link.control_open = false;
                link.state_open = false;
                link.disconnected_at_ms = None;
            }
        }
        expired
    }

    pub fn route_for(&self, player_id: &str, message_type: &str) -> TransportRoute {
        if self.role == P2pRole::Peer && self.host_channel_lost {
            return TransportRoute::Relay;
        }
        self.links
            .get(player_id)
            .map(|link| link.route_for(message_type))
            .unwrap_or(TransportRoute::Relay)
    }

    pub fn teardown(&mut self) {
        self.active = false;
        self.host_channel_lost = false;
        self.links.clear();
        self.link_state = P2pLinkState::Waiting;
    }
}

pub fn is_state_stream(message_type: &str) -> bool {
    matches!(message_type, "input" | "snapshot" | "combat.hit")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_traffic_prefers_lossy_channel_and_reliable_uses_control() {
        let mut link = PeerLink::new("p2");
        link.control_open = true;
        link.state_open = true;
        assert_eq!(link.route_for("input"), TransportRoute::State);
        assert_eq!(link.route_for("snapshot"), TransportRoute::State);
        assert_eq!(link.route_for("combat.hit"), TransportRoute::State);
        assert_eq!(link.route_for("match.end"), TransportRoute::Control);
    }

    #[test]
    fn state_backpressure_drops_only_superseded_state() {
        let mut link = PeerLink::new("p2");
        link.control_open = true;
        link.state_open = true;
        link.state_buffered = INPUT_BACKLOG_LIMIT + 1;
        assert!(link.drops_for_backpressure("input"));
        assert!(!link.drops_for_backpressure("match.end"));
    }

    #[test]
    fn peer_falls_back_to_relay_only_after_disconnect_grace() {
        let mut coordinator = P2pCoordinator::new(P2pRole::Peer);
        let link = coordinator.add_link("host");
        link.control_open = true;
        link.state_open = true;
        coordinator.start();
        assert_eq!(
            coordinator.route_for("host", "input"),
            TransportRoute::State
        );

        coordinator
            .links
            .get_mut("host")
            .unwrap()
            .mark_disconnected(100);
        assert!(coordinator
            .expire_disconnects(
                100 + DEFAULT_DISCONNECT_GRACE_MS - 1,
                DEFAULT_DISCONNECT_GRACE_MS
            )
            .is_empty());
        assert!(!coordinator.host_channel_lost);

        assert_eq!(
            coordinator.expire_disconnects(
                100 + DEFAULT_DISCONNECT_GRACE_MS,
                DEFAULT_DISCONNECT_GRACE_MS
            ),
            vec!["host".to_string()]
        );
        assert!(coordinator.host_channel_lost);
        assert_eq!(
            coordinator.route_for("host", "input"),
            TransportRoute::Relay
        );
    }

    #[test]
    fn reconnect_cancels_disconnect_fallback() {
        let mut link = PeerLink::new("p2");
        link.mark_disconnected(10);
        link.mark_reconnected();
        assert!(!link.disconnect_grace_expired(
            10 + DEFAULT_DISCONNECT_GRACE_MS,
            DEFAULT_DISCONNECT_GRACE_MS
        ));
    }

    #[test]
    fn input_health_uses_direct_before_relay() {
        let mut link = PeerLink::new("host");
        link.control_open = true;
        link.state_open = true;
        link.state_buffered = INPUT_BACKLOG_LIMIT + 1;
        assert!(!link.input_healthy(0));
        link.state_open = false;
        link.control_buffered = 0;
        assert!(link.input_healthy(INPUT_BACKLOG_LIMIT + 1));
        link.control_open = false;
        assert!(!link.input_healthy(INPUT_BACKLOG_LIMIT + 1));
    }
}
