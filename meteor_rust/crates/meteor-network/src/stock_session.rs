use crate::stock_udp::{
    StockApplicationPacket, StockConnectRecord, StockControlPacket, StockDatagram,
    StockDiscoveryInfo, StockPacketError, StockReliability, STOCK_FLAG_XOR_BODY,
    STOCK_MAX_DATAGRAM_BYTES, STOCK_PACKET_TYPE_APPLICATION, STOCK_PACKET_TYPE_CONNECT,
};
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};

pub const STOCK_CONNECT_RETRY_MS: u64 = 250;
pub const STOCK_PING_INTERVAL_MS: u64 = 1_000;

pub const STOCK_LAN_DEFAULT_PORT: u16 = 5_230;
pub const STOCK_DISCOVERY_INTERVAL_MS: u64 = 1_000;
pub const STOCK_DISCOVERY_ROOM_TTL_MS: u64 = 3_500;

pub const STOCK_IDLE_TIMEOUT_MS: u64 = 10_000;

pub const STOCK_CONNECT_TIMEOUT_MS: u64 = STOCK_IDLE_TIMEOUT_MS * 2;

pub const STOCK_SERVER_TIMEOUT_SCAN_MS: u64 = 200;
pub const STOCK_TIMEOUT_REASON: u32 = 3;

pub const STOCK_REJECT_VERSION: u32 = 0xff01;
pub const STOCK_REJECT_CDKEY: u32 = 0xff02;
pub const STOCK_REJECT_FULL: u32 = 0xff03;
pub const STOCK_REJECT_PASSWORD: u32 = 0xff04;
pub const STOCK_REJECT_CHECKSUM: u32 = 0xff06;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockClientState {
    Connecting,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StockSessionEvent {
    Connected {
        peer: SocketAddr,
        transport_id: u32,
    },
    PeerConnected {
        peer: SocketAddr,
        transport_id: u32,
        identity: String,
    },
    Application {
        peer: SocketAddr,
        packet: StockApplicationPacket,
    },
    Disconnected {
        peer: SocketAddr,
        reason: u32,
    },
    DiscoveryReply {
        peer: SocketAddr,
        sent_at: u32,
        info: StockDiscoveryInfo,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockDiscoveredRoom {
    pub endpoint: SocketAddr,
    pub info: StockDiscoveryInfo,
    pub last_seen_ms: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum StockTransportError {
    #[error("stock UDP socket error: {0}")]
    Io(#[from] io::Error),
    #[error("stock UDP packet error: {0}")]
    Packet(#[from] StockPacketError),
    #[error("stock UDP client is not connected")]
    NotConnected,
    #[error("stock UDP peer is unknown: {0}")]
    UnknownPeer(SocketAddr),
    #[error("stock UDP address did not resolve")]
    AddressDidNotResolve,
}

pub struct StockDiscoveryClient {
    socket: UdpSocket,
    destination: SocketAddr,
    query: String,
    reliability: StockReliability,
    last_query_ms: Option<u64>,
    rooms: HashMap<SocketAddr, StockDiscoveredRoom>,
    metadata_queries: HashMap<SocketAddr, (u32, u8)>,
}

impl StockDiscoveryClient {
    pub fn bind_lan() -> Result<Self, StockTransportError> {
        Self::bind_with_destination(
            "0.0.0.0:0",
            SocketAddr::from(([255, 255, 255, 255], STOCK_LAN_DEFAULT_PORT)),
            "serverlist",
        )
    }

    pub fn bind_with_destination(
        local: impl ToSocketAddrs,
        destination: impl ToSocketAddrs,
        query: impl Into<String>,
    ) -> Result<Self, StockTransportError> {
        let socket = UdpSocket::bind(local)?;
        socket.set_broadcast(true)?;
        socket.set_nonblocking(true)?;
        let destination = destination
            .to_socket_addrs()?
            .next()
            .ok_or(StockTransportError::AddressDidNotResolve)?;
        Ok(Self {
            socket,
            destination,
            query: query.into(),
            reliability: StockReliability::default(),
            last_query_ms: None,
            rooms: HashMap::new(),
            metadata_queries: HashMap::new(),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, StockTransportError> {
        Ok(self.socket.local_addr()?)
    }

    pub fn refresh(&mut self) {
        self.last_query_ms = None;
    }

    pub fn rooms(&self) -> Vec<StockDiscoveredRoom> {
        let mut rooms = self.rooms.values().cloned().collect::<Vec<_>>();
        rooms.sort_by(|left, right| {
            left.info
                .server_name
                .cmp(&right.info.server_name)
                .then_with(|| left.endpoint.cmp(&right.endpoint))
        });
        rooms
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<Vec<StockDiscoveredRoom>, StockTransportError> {
        if self
            .last_query_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= STOCK_DISCOVERY_INTERVAL_MS)
        {
            for query in [
                self.query.clone(),
                format!("{}:meteor-v1", self.query),
                format!("{}:meteor-v2", self.query),
            ] {
                let packet = StockControlPacket::Discover {
                    sent_at: now_ms as u32,
                    query,
                }
                .encode(0)?;
                let packet = self.reliability.prepare_control(&packet)?;
                self.socket.send_to(&packet, self.destination)?;
            }
            self.last_query_ms = Some(now_ms);
        }

        let mut buffer = [0u8; STOCK_MAX_DATAGRAM_BYTES];
        loop {
            let (length, peer) = match self.socket.recv_from(&mut buffer) {
                Ok(result) => result,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,

                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(error) => return Err(error.into()),
            };
            let bytes = &buffer[..length];
            let Ok(StockControlPacket::DiscoverReply { info, sent_at }) =
                StockControlPacket::decode(bytes, None)
            else {
                continue;
            };
            if self.last_query_ms.map(|time| time as u32) != Some(sent_at) {
                continue;
            }
            let version = if info.starts_with(b"MRD2") {
                2
            } else {
                u8::from(info.starts_with(b"MRD1"))
            };
            let Ok(mut info) = StockDiscoveryInfo::decode_room_summary(&info) else {
                continue;
            };
            let endpoint = SocketAddr::new(peer.ip(), info.port);
            if self
                .metadata_queries
                .get(&endpoint)
                .is_some_and(|&(time, previous)| time == sent_at && previous > version)
            {
                if let Some(room) = self.rooms.get(&endpoint) {
                    info.stage_id = room.info.stage_id.clone();
                    info.server_name = room.info.server_name.clone();
                }
            } else {
                self.metadata_queries.insert(endpoint, (sent_at, version));
            }
            self.rooms.insert(
                endpoint,
                StockDiscoveredRoom {
                    endpoint,
                    info,
                    last_seen_ms: now_ms,
                },
            );
        }
        self.rooms.retain(|_, room| {
            now_ms.saturating_sub(room.last_seen_ms) <= STOCK_DISCOVERY_ROOM_TTL_MS
        });
        self.metadata_queries
            .retain(|endpoint, _| self.rooms.contains_key(endpoint));
        Ok(self.rooms())
    }
}

pub struct StockClientTransport {
    socket: UdpSocket,
    server: SocketAddr,

    connect_record: Vec<u8>,
    player_name: String,
    state: StockClientState,
    cookie: u32,
    transport_id: Option<u32>,
    logical_channel: u16,
    reliability: StockReliability,
    connect_started_ms: Option<u64>,
    last_received_ms: Option<u64>,
    last_connect_sent_ms: Option<u64>,
    last_ping_sent_ms: Option<u64>,
    round_trip_ms: u32,
    last_pong_ms: Option<u64>,
    resent_since_health_sample: bool,
}

impl StockClientTransport {
    pub fn bind(
        local: impl ToSocketAddrs,
        server: impl ToSocketAddrs,
        connect_record: StockConnectRecord,
    ) -> Result<Self, StockTransportError> {
        let socket = UdpSocket::bind(local)?;
        socket.set_nonblocking(true)?;
        let server = server
            .to_socket_addrs()?
            .next()
            .ok_or(StockTransportError::AddressDidNotResolve)?;

        let logical_channel = socket.local_addr()?.port();
        Ok(Self {
            socket,
            server,
            connect_record: connect_record.encode()?,
            player_name: connect_record.player_name,
            state: StockClientState::Connecting,
            cookie: 0,
            transport_id: None,
            logical_channel,
            reliability: StockReliability::default(),
            connect_started_ms: None,
            last_received_ms: None,
            last_connect_sent_ms: None,
            last_ping_sent_ms: None,
            round_trip_ms: 0,
            last_pong_ms: None,
            resent_since_health_sample: false,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, StockTransportError> {
        Ok(self.socket.local_addr()?)
    }

    pub fn server_addr(&self) -> SocketAddr {
        self.server
    }

    pub fn player_name(&self) -> &str {
        &self.player_name
    }

    pub fn state(&self) -> StockClientState {
        self.state
    }

    pub fn transport_id(&self) -> Option<u32> {
        self.transport_id
    }

    pub fn measured_round_trip_ms(&self, now_ms: u64) -> Option<u32> {
        self.last_pong_ms
            .filter(|at| now_ms.saturating_sub(*at) < 10_000)
            .map(|_| self.round_trip_ms)
    }

    pub fn round_trip_ms(&self) -> u32 {
        self.round_trip_ms
    }

    pub fn take_health_sample(&mut self) -> bool {
        let healthy = self.state == StockClientState::Connected && !self.resent_since_health_sample;
        self.resent_since_health_sample = false;
        healthy
    }

    pub fn set_logical_channel(&mut self, logical_channel: u16) {
        self.logical_channel = logical_channel;
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<Vec<StockSessionEvent>, StockTransportError> {
        let mut events = Vec::new();
        let timed_out = match self.state {
            StockClientState::Connecting => self
                .connect_started_ms
                .is_some_and(|started| now_ms.saturating_sub(started) > STOCK_CONNECT_TIMEOUT_MS),
            StockClientState::Connected => self
                .last_received_ms
                .is_some_and(|received| now_ms.saturating_sub(received) > STOCK_IDLE_TIMEOUT_MS),
            StockClientState::Disconnected => false,
        };
        if timed_out {
            self.state = StockClientState::Disconnected;
            events.push(StockSessionEvent::Disconnected {
                peer: self.server,
                reason: STOCK_TIMEOUT_REASON,
            });
            return Ok(events);
        }
        if self.state == StockClientState::Connecting
            && self
                .last_connect_sent_ms
                .is_none_or(|last| now_ms.saturating_sub(last) >= STOCK_CONNECT_RETRY_MS)
        {
            self.connect_started_ms.get_or_insert(now_ms);
            let connect = StockControlPacket::Connect {
                cookie: self.cookie,
                record: self.connect_record.clone(),
            };
            self.send_control(&connect, self.logical_channel)?;
            self.last_connect_sent_ms = Some(now_ms);
        } else if self.state == StockClientState::Connected
            && self
                .last_ping_sent_ms
                .is_none_or(|last| now_ms.saturating_sub(last) >= STOCK_PING_INTERVAL_MS)
        {
            let ping = StockControlPacket::Ping {
                client_time: now_ms as u32,
                latency: self.round_trip_ms,
            };
            self.send_control(&ping, self.logical_channel)?;
            self.last_ping_sent_ms = Some(now_ms);
        }

        self.flush_resends(now_ms)?;
        let mut buffer = [0u8; STOCK_MAX_DATAGRAM_BYTES];
        loop {
            let (length, peer) = match self.socket.recv_from(&mut buffer) {
                Ok(result) => result,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,

                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(error) => return Err(error.into()),
            };
            if peer != self.server {
                continue;
            }
            let bytes = &buffer[..length];
            let Ok(datagram) = StockDatagram::decode(bytes) else {
                continue;
            };

            self.last_received_ms = Some(now_ms);
            if !self.reliability.accept_datagram(&datagram) {
                continue;
            }
            if datagram.packet_type == STOCK_PACKET_TYPE_APPLICATION {
                if self.state != StockClientState::Connected {
                    continue;
                }
                let Ok(packet) = StockApplicationPacket::decode(bytes) else {
                    continue;
                };
                events.push(StockSessionEvent::Application { peer, packet });
                continue;
            }
            let Ok(control) = StockControlPacket::decode(bytes, None) else {
                continue;
            };
            match control {
                StockControlPacket::Challenge { cookie }
                    if self.state == StockClientState::Connecting =>
                {
                    self.cookie = cookie;
                    self.reliability = StockReliability::default();
                    self.last_connect_sent_ms = None;
                }
                StockControlPacket::Connected { transport_id }
                    if self.state == StockClientState::Connecting =>
                {
                    self.transport_id = Some(transport_id);
                    self.state = StockClientState::Connected;
                    events.push(StockSessionEvent::Connected { peer, transport_id });
                }
                StockControlPacket::Disconnect { reason } => {
                    self.state = StockClientState::Disconnected;
                    events.push(StockSessionEvent::Disconnected { peer, reason });
                }
                StockControlPacket::Pong {
                    client_time,
                    server_time: _,
                } if self.state == StockClientState::Connected => {
                    self.last_pong_ms = Some(now_ms);
                    let sample = (now_ms as u32).wrapping_sub(client_time);
                    self.round_trip_ms = if self.round_trip_ms == 0 {
                        sample
                    } else {
                        (self.round_trip_ms.saturating_add(sample.saturating_mul(2))) / 3
                    };
                }
                StockControlPacket::DiscoverReply { sent_at, info } => {
                    if let Ok(info) = StockDiscoveryInfo::decode(&info) {
                        events.push(StockSessionEvent::DiscoveryReply {
                            peer,
                            sent_at,
                            info,
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(events)
    }

    pub fn send_application(
        &mut self,
        opcode: u16,
        payload: impl Into<Vec<u8>>,
        transport_flags: u8,
        now_ms: u64,
    ) -> Result<(), StockTransportError> {
        self.send_application_tracked(opcode, payload, transport_flags, now_ms)
            .map(|_| ())
    }

    pub fn send_application_tracked(
        &mut self,
        opcode: u16,
        payload: impl Into<Vec<u8>>,
        transport_flags: u8,
        now_ms: u64,
    ) -> Result<u16, StockTransportError> {
        if self.state != StockClientState::Connected {
            return Err(StockTransportError::NotConnected);
        }
        let packet =
            StockApplicationPacket::new(self.logical_channel, opcode, payload, transport_flags)?;
        let packet = self.reliability.prepare_outbound(packet, now_ms);
        self.send(&packet.encode()?)?;
        Ok(packet.sequence)
    }

    pub fn disconnect(&mut self, reason: u32) -> Result<(), StockTransportError> {
        if self.state == StockClientState::Disconnected {
            return Ok(());
        }
        self.send_control(
            &StockControlPacket::Disconnect { reason },
            self.logical_channel,
        )?;
        self.state = StockClientState::Disconnected;
        Ok(())
    }

    fn flush_resends(&mut self, now_ms: u64) -> Result<(), StockTransportError> {
        let due = self.reliability.resend_due(now_ms);
        if due.is_empty() {
            return Ok(());
        }
        for packet in due {
            let packet = self.reliability.refresh_reliable(packet);
            self.send(&packet.encode()?)?;
        }
        self.reliability.mark_resend_sent(now_ms);
        self.resent_since_health_sample = true;
        Ok(())
    }

    fn send(&self, bytes: &[u8]) -> Result<(), StockTransportError> {
        self.socket.send_to(bytes, self.server)?;
        Ok(())
    }

    fn send_control(
        &mut self,
        packet: &StockControlPacket,
        logical_channel: u16,
    ) -> Result<(), StockTransportError> {
        let bytes = packet.encode(logical_channel)?;
        let bytes = self.reliability.prepare_control(&bytes)?;
        self.send(&bytes)
    }
}

#[derive(Debug)]
struct PendingChallenge {
    cookie: u32,
    created_at_ms: u64,
}

#[derive(Debug)]
struct StockServerPeer {
    reported_latency: Option<(u32, u64)>,
    player_name: String,
    cookie: u32,
    transport_id: u32,
    identity: String,
    logical_channel: u16,
    reliability: StockReliability,
    last_received_ms: u64,
}

pub struct StockServerTransport {
    socket: UdpSocket,
    discovery_query: String,
    discovery_info: StockDiscoveryInfo,

    room_password: String,
    pending: HashMap<SocketAddr, PendingChallenge>,
    peers: HashMap<SocketAddr, StockServerPeer>,
    next_transport_id: u32,
    last_timeout_scan_ms: u64,
}

impl StockServerTransport {
    pub fn peer_latencies(&self, now_ms: u64) -> Vec<(u32, u32)> {
        self.peers
            .values()
            .filter_map(|peer| {
                peer.reported_latency
                    .filter(|(_, at)| now_ms.saturating_sub(*at) < 10_000)
                    .map(|(ms, _)| (peer.transport_id, ms))
            })
            .take(32)
            .collect()
    }

    pub fn bind(
        local: impl ToSocketAddrs,
        discovery_query: impl Into<String>,
        server_name: impl Into<String>,
    ) -> Result<Self, StockTransportError> {
        let socket = UdpSocket::bind(local)?;
        socket.set_nonblocking(true)?;
        let discovery_info =
            StockDiscoveryInfo::default_room(socket.local_addr()?.port(), server_name, "");
        log::info!("[stock-lan] host listening on {}", socket.local_addr()?);
        Ok(Self {
            socket,
            discovery_query: discovery_query.into(),
            discovery_info,
            room_password: String::new(),
            pending: HashMap::new(),
            peers: HashMap::new(),
            next_transport_id: 1,
            last_timeout_scan_ms: 0,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, StockTransportError> {
        Ok(self.socket.local_addr()?)
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    pub fn set_discovery_stage(&mut self, stage: Option<String>) {
        self.discovery_info.fields_0_to_6[2] = stage
            .as_deref()
            .and_then(|id| id.strip_prefix("sn"))
            .and_then(|number| number.parse::<u8>().ok())
            .filter(|number| (1..=254).contains(number))
            .unwrap_or(0);
        self.discovery_info.stage_id = stage;
    }

    pub fn set_discovery_rule(&mut self, rule: u8) {
        self.discovery_info.fields_0_to_6[5] = rule;
    }

    pub fn set_discovery_camp_counts(&mut self, camp1: usize, camp2: usize) {
        self.discovery_info.field_0e = camp1.min(254) as u8;
        self.discovery_info.field_0f = camp2.min(254) as u8;
    }

    pub fn set_room_password(&mut self, password: impl Into<String>) {
        self.room_password = password.into();
    }

    pub fn peer_identity(&self, peer: SocketAddr) -> Option<&str> {
        self.peers.get(&peer).map(|record| record.identity.as_str())
    }

    pub fn peer_player_name(&self, peer: SocketAddr) -> Option<&str> {
        self.peers
            .get(&peer)
            .map(|record| record.player_name.as_str())
    }

    pub fn peer_addresses(&self) -> Vec<SocketAddr> {
        self.peers.keys().copied().collect()
    }

    pub fn peer_transport_id(&self, peer: SocketAddr) -> Option<u32> {
        self.peers.get(&peer).map(|record| record.transport_id)
    }

    pub fn set_peer_logical_channel(
        &mut self,
        peer: SocketAddr,
        logical_channel: u16,
    ) -> Result<(), StockTransportError> {
        let record = self
            .peers
            .get_mut(&peer)
            .ok_or(StockTransportError::UnknownPeer(peer))?;
        record.logical_channel = logical_channel;
        Ok(())
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<Vec<StockSessionEvent>, StockTransportError> {
        self.flush_resends(now_ms)?;
        let mut events = self.expire_silent_peers(now_ms);
        let mut buffer = [0u8; STOCK_MAX_DATAGRAM_BYTES];
        loop {
            let (length, peer) = match self.socket.recv_from(&mut buffer) {
                Ok(result) => result,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,

                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(error) => return Err(error.into()),
            };
            let bytes = &buffer[..length];
            let datagram = match StockDatagram::decode(bytes) {
                Ok(datagram) => datagram,
                Err(error) => {
                    if bytes.first() == Some(&crate::stock_udp::STOCK_PACKET_TYPE_DISCOVER) {
                        log::debug!(
                            "[stock-lan] invalid query header from {peer}: bytes={length}, {error}"
                        );
                    }
                    continue;
                }
            };
            if let Some(record) = self.peers.get_mut(&peer) {
                record.last_received_ms = now_ms;
            }
            if datagram.packet_type == STOCK_PACKET_TYPE_APPLICATION {
                let Some(record) = self.peers.get_mut(&peer) else {
                    continue;
                };
                if !record.reliability.accept_datagram(&datagram) {
                    continue;
                }
                let Ok(packet) = StockApplicationPacket::decode(bytes) else {
                    continue;
                };
                events.push(StockSessionEvent::Application { peer, packet });
                continue;
            }

            let xor_key = (datagram.packet_type == STOCK_PACKET_TYPE_CONNECT
                && datagram.flags & STOCK_FLAG_XOR_BODY != 0)
                .then(|| {
                    self.pending
                        .get(&peer)
                        .map(|pending| pending.cookie)
                        .or_else(|| self.peers.get(&peer).map(|record| record.cookie))
                })
                .flatten();
            let control = match StockControlPacket::decode(bytes, xor_key) {
                Ok(control) => control,
                Err(error) => {
                    if datagram.packet_type == STOCK_PACKET_TYPE_CONNECT {
                        log::debug!("[stock-lan] connect decode failed from {peer}: bytes={length}, flags={}, {error}", datagram.flags);
                    }
                    if datagram.packet_type == crate::stock_udp::STOCK_PACKET_TYPE_DISCOVER {
                        log::debug!(
                            "[stock-lan] invalid query body from {peer}: bytes={length}, {error}"
                        );
                    }
                    continue;
                }
            };
            match control {
                StockControlPacket::Connect { cookie: 0, .. } => {
                    let cookie = endpoint_cookie(peer, now_ms, self.next_transport_id);
                    self.pending.insert(
                        peer,
                        PendingChallenge {
                            cookie,
                            created_at_ms: now_ms,
                        },
                    );
                    self.send_untracked_control(
                        peer,
                        &StockControlPacket::Challenge { cookie },
                        self.local_addr()?.port(),
                    )?;
                    log::debug!("[stock-lan] challenge sent to {peer}");
                }
                StockControlPacket::Connect { cookie, record } => {
                    let parsed = StockConnectRecord::decode(&record).ok();
                    let identity = match &parsed {
                        Some(parsed) if parsed.compatibility != [9, 7] => {
                            log::debug!(
                                "[stock-lan] connect rejected from {peer}: version mismatch"
                            );
                            self.send_untracked_control(
                                peer,
                                &StockControlPacket::Disconnect {
                                    reason: STOCK_REJECT_VERSION,
                                },
                                0,
                            )?;
                            continue;
                        }
                        Some(parsed) if parsed.password != self.room_password => {
                            log::debug!(
                                "[stock-lan] connect rejected from {peer}: password mismatch"
                            );
                            self.send_untracked_control(
                                peer,
                                &StockControlPacket::Disconnect {
                                    reason: STOCK_REJECT_PASSWORD,
                                },
                                0,
                            )?;
                            continue;
                        }
                        Some(parsed) => parsed.cdkey.clone(),
                        None => {
                            log::debug!("[stock-lan] connect rejected from {peer}: invalid record");
                            self.send_untracked_control(
                                peer,
                                &StockControlPacket::Disconnect {
                                    reason: STOCK_REJECT_VERSION,
                                },
                                0,
                            )?;
                            continue;
                        }
                    };
                    let pending_valid = self
                        .pending
                        .get(&peer)
                        .is_some_and(|pending| pending.cookie == cookie);
                    let peer_valid = self
                        .peers
                        .get(&peer)
                        .is_some_and(|record| record.cookie == cookie);
                    if !pending_valid && !peer_valid {
                        log::debug!("[stock-lan] connect ignored from {peer}: challenge mismatch");
                        continue;
                    }
                    let (transport_id, inserted) = if let Some(record) = self.peers.get_mut(&peer) {
                        if !record.reliability.accept_datagram(&datagram) {
                            continue;
                        }
                        (record.transport_id, false)
                    } else {
                        let transport_id = self.allocate_transport_id();
                        let mut reliability = StockReliability::default();
                        if !reliability.accept_datagram(&datagram) {
                            continue;
                        }
                        self.peers.insert(
                            peer,
                            StockServerPeer {
                                reported_latency: None,
                                player_name: parsed.as_ref().unwrap().player_name.clone(),
                                cookie,
                                transport_id,
                                identity: identity.clone(),
                                logical_channel: 0,
                                reliability,
                                last_received_ms: now_ms,
                            },
                        );
                        (transport_id, true)
                    };
                    self.send_untracked_control(
                        peer,
                        &StockControlPacket::Connected { transport_id },
                        self.local_addr()?.port(),
                    )?;
                    log::debug!("[stock-lan] connect accepted: peer={peer}, transport_id={transport_id}, new={inserted}");
                    if inserted {
                        self.pending.remove(&peer);
                        events.push(StockSessionEvent::PeerConnected {
                            peer,
                            transport_id,
                            identity,
                        });
                    }
                }
                StockControlPacket::Disconnect { reason } => {
                    let accepted = self
                        .peers
                        .get_mut(&peer)
                        .is_some_and(|record| record.reliability.accept_datagram(&datagram));
                    if accepted && self.peers.remove(&peer).is_some() {
                        events.push(StockSessionEvent::Disconnected { peer, reason });
                    }
                }
                StockControlPacket::Ping {
                    client_time,
                    latency,
                } if self.peers.contains_key(&peer) => {
                    let accepted = self
                        .peers
                        .get_mut(&peer)
                        .is_some_and(|record| record.reliability.accept_datagram(&datagram));
                    if accepted {
                        if let Some(record) = self.peers.get_mut(&peer) {
                            record.reported_latency =
                                (latency <= 60_000).then_some((latency, now_ms));
                        }
                        self.send_peer_control(
                            peer,
                            &StockControlPacket::Pong {
                                client_time,
                                server_time: now_ms as u32,
                            },
                        )?;
                    }
                }
                StockControlPacket::Discover { sent_at, query }
                    if query.is_empty()
                        || query == self.discovery_query
                        || query == format!("{}:meteor-v1", self.discovery_query)
                        || query == format!("{}:meteor-v2", self.discovery_query) =>
                {
                    let extended = query == format!("{}:meteor-v1", self.discovery_query);
                    let mut info = self.discovery_info.clone();
                    info.fields_0_to_6[3] = self.peers.len().min(254) as u8;

                    if let Some(ip) = local_ipv4_toward(peer) {
                        info.server_address = ip.to_string();
                    }
                    self.send_untracked_control(
                        peer,
                        &StockControlPacket::DiscoverReply {
                            sent_at,
                            info: if query == format!("{}:meteor-v2", self.discovery_query) {
                                info.encode_unicode_room_summary()?
                            } else if extended {
                                info.encode_room_summary()?
                            } else {
                                info.encode()?
                            },
                        },
                        0,
                    )?;
                    log::debug!(
                        "[stock-lan] query replied: peer={peer}, sent_at={sent_at}, extended={}, map={}, rule={}, players={}",
                        extended,
                        info.fields_0_to_6[2],
                        info.fields_0_to_6[5],
                        info.fields_0_to_6[3],
                    );
                }
                StockControlPacket::Discover { query, .. } => {
                    log::debug!(
                        "[stock-lan] query ignored from {peer}: query_length={}",
                        query.len()
                    );
                }
                _ => {}
            }
        }
        Ok(events)
    }

    pub fn send_application(
        &mut self,
        peer: SocketAddr,
        opcode: u16,
        payload: impl Into<Vec<u8>>,
        transport_flags: u8,
        now_ms: u64,
    ) -> Result<(), StockTransportError> {
        let record = self
            .peers
            .get_mut(&peer)
            .ok_or(StockTransportError::UnknownPeer(peer))?;
        let packet =
            StockApplicationPacket::new(record.logical_channel, opcode, payload, transport_flags)?;
        let packet = record.reliability.prepare_outbound(packet, now_ms);
        let bytes = packet.encode()?;
        self.socket.send_to(&bytes, peer)?;
        Ok(())
    }

    pub fn disconnect_peer(
        &mut self,
        peer: SocketAddr,
        reason: u32,
    ) -> Result<(), StockTransportError> {
        self.send_peer_control(peer, &StockControlPacket::Disconnect { reason })?;
        self.peers.remove(&peer);
        Ok(())
    }

    fn allocate_transport_id(&mut self) -> u32 {
        let id = self.next_transport_id.max(1);
        self.next_transport_id = self.next_transport_id.wrapping_add(1).max(1);
        id
    }

    fn flush_resends(&mut self, now_ms: u64) -> Result<(), StockTransportError> {
        let peers = self.peer_addresses();
        for peer in peers {
            let sends = {
                let record = self
                    .peers
                    .get(&peer)
                    .expect("peer address came from the same map");
                record
                    .reliability
                    .resend_due(now_ms)
                    .into_iter()
                    .map(|packet| record.reliability.refresh_reliable(packet).encode())
                    .collect::<Result<Vec<_>, _>>()?
            };
            if sends.is_empty() {
                continue;
            }
            for bytes in sends {
                self.socket.send_to(&bytes, peer)?;
            }
            self.peers
                .get_mut(&peer)
                .expect("peer address came from the same map")
                .reliability
                .mark_resend_sent(now_ms);
        }
        Ok(())
    }

    fn expire_silent_peers(&mut self, now_ms: u64) -> Vec<StockSessionEvent> {
        if now_ms.saturating_sub(self.last_timeout_scan_ms) <= STOCK_SERVER_TIMEOUT_SCAN_MS {
            return Vec::new();
        }
        self.last_timeout_scan_ms = now_ms;
        self.pending.retain(|_, pending| {
            now_ms.saturating_sub(pending.created_at_ms) <= STOCK_CONNECT_TIMEOUT_MS
        });
        let expired = self
            .peers
            .iter()
            .filter_map(|(peer, record)| {
                (now_ms.saturating_sub(record.last_received_ms) > STOCK_IDLE_TIMEOUT_MS)
                    .then_some(*peer)
            })
            .collect::<Vec<_>>();
        expired
            .into_iter()
            .filter_map(|peer| {
                self.peers
                    .remove(&peer)
                    .map(|_| StockSessionEvent::Disconnected {
                        peer,
                        reason: STOCK_TIMEOUT_REASON,
                    })
            })
            .collect()
    }

    fn send_peer_control(
        &mut self,
        peer: SocketAddr,
        packet: &StockControlPacket,
    ) -> Result<(), StockTransportError> {
        let bytes = {
            let record = self
                .peers
                .get_mut(&peer)
                .ok_or(StockTransportError::UnknownPeer(peer))?;
            let bytes = packet.encode(record.logical_channel)?;
            record.reliability.prepare_control(&bytes)?
        };
        self.socket.send_to(&bytes, peer)?;
        Ok(())
    }

    fn send_untracked_control(
        &self,
        peer: SocketAddr,
        packet: &StockControlPacket,
        logical_channel: u16,
    ) -> Result<(), StockTransportError> {
        let bytes = packet.encode(logical_channel)?;
        self.socket.send_to(&bytes, peer)?;
        Ok(())
    }
}

fn local_ipv4_toward(peer: SocketAddr) -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(peer).ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(address) => Some(*address.ip()),
        SocketAddr::V6(_) => None,
    }
}

fn endpoint_cookie(peer: SocketAddr, now_ms: u64, sequence: u32) -> u32 {
    let ip = match peer.ip() {
        std::net::IpAddr::V4(ip) => u32::from_le_bytes(ip.octets()),
        std::net::IpAddr::V6(ip) => ip.segments().iter().fold(0u32, |hash, segment| {
            hash.rotate_left(5) ^ u32::from(*segment)
        }),
    };
    let cookie = ip
        .wrapping_mul(3)
        .wrapping_add(u32::from(peer.port()).wrapping_mul(2))
        .wrapping_add(now_ms as u32)
        .wrapping_add(sequence.rotate_left(13));
    cookie.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_server_replies_with_exact_me_discovery_record() {
        assert_legacy_discovery_reply(false);
    }

    #[test]
    fn native_server_replies_to_original_empty_lan_query_with_me_record() {
        assert_legacy_discovery_reply(true);
    }

    fn assert_legacy_discovery_reply(empty_query: bool) {
        let mut server = StockServerTransport::bind("127.0.0.1:0", "serverlist", "Rust 房间")
            .expect("server binds");
        server.set_discovery_camp_counts(2, 3);
        server.set_discovery_rule(5);
        let server_addr = server.local_addr().unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.set_nonblocking(true).unwrap();

        let discover = if empty_query {
            vec![7, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0x78, 0x56, 0x34, 0x12, 0]
        } else {
            StockControlPacket::Discover {
                sent_at: 0x1234_5678,
                query: "serverlist".into(),
            }
            .encode(0)
            .unwrap()
        };
        let mut reliability = StockReliability::default();
        let discover = reliability.prepare_control(&discover).unwrap();
        client.send_to(&discover, server_addr).unwrap();

        let mut buffer = [0u8; STOCK_MAX_DATAGRAM_BYTES];
        let reply = (0..500).find_map(|now_ms| {
            server.tick(now_ms).unwrap();
            match client.recv_from(&mut buffer) {
                Ok((length, peer)) if peer == server_addr => {
                    StockControlPacket::decode(&buffer[..length], None).ok()
                }
                Ok(_) => None,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_micros(50));
                    None
                }
                Err(error) => panic!("discovery receive failed: {error}"),
            }
        });
        let StockControlPacket::DiscoverReply { sent_at, info } =
            reply.expect("server replies to legacy LAN discovery")
        else {
            panic!("expected discover reply");
        };
        assert_eq!(sent_at, 0x1234_5678);
        assert!(
            info.starts_with(b"ME"),
            "legacy clients must never receive MRD1"
        );
        let info = StockDiscoveryInfo::decode(&info).unwrap();
        assert_eq!(info.port, server_addr.port());
        assert_eq!(info.server_name, "Rust 房间");
        assert_eq!(
            info.server_address, "127.0.0.1",
            "record +0x50 is the host address, not a stage name"
        );
        assert_eq!(info.fields_0_to_6[..2], [9, 7]);
        assert_eq!(info.fields_0_to_6[3], 0);
        assert_eq!((info.field_0e, info.field_0f), (2, 3));
        assert_eq!(info.rule(), Some(5));
    }

    #[test]
    fn native_room_browser_discovers_deduplicates_and_expires_stock_rooms() {
        let mut server = StockServerTransport::bind("127.0.0.1:0", "serverlist", "原版房间3️⃣")
            .expect("server binds");
        let server_addr = server.local_addr().unwrap();
        let mut browser =
            StockDiscoveryClient::bind_with_destination("127.0.0.1:0", server_addr, "serverlist")
                .expect("browser binds");

        server.set_discovery_stage(Some("sn03".into()));

        browser.tick(0).expect("first discovery query sends");
        let room = (1..500).find_map(|now_ms| {
            server.tick(now_ms).unwrap();
            let rooms = browser.tick(now_ms).unwrap();
            if let Some(room) = rooms.into_iter().find(|room| {
                room.info.server_name == "原版房间3️⃣"
                    && room.info.stage_id.as_deref() == Some("sn03")
            }) {
                Some(room)
            } else {
                std::thread::sleep(std::time::Duration::from_micros(50));
                None
            }
        });
        let room = room.expect("stock room is discovered");
        assert_eq!(room.endpoint, server_addr);
        assert_eq!(room.info.server_name, "原版房间3️⃣");
        assert_eq!(room.info.server_address, "127.0.0.1");
        assert_eq!(room.info.stage_id.as_deref(), Some("sn03"));
        assert_eq!(
            room.info.fields_0_to_6[2], 3,
            "legacy clients receive the map number too"
        );

        browser.refresh();
        browser.tick(1_000).expect("refresh query sends");
        let refreshed = (1_001..1_500).find_map(|now_ms| {
            server.tick(now_ms).unwrap();
            let rooms = browser.tick(now_ms).unwrap();
            rooms
                .first()
                .filter(|room| room.last_seen_ms >= 1_001)
                .cloned()
        });
        assert!(refreshed.is_some(), "repeat replies refresh the same room");
        assert_eq!(browser.rooms().len(), 1, "endpoint is the dedupe key");

        drop(server);
        let rooms = browser
            .tick(1_500 + STOCK_DISCOVERY_ROOM_TTL_MS)
            .expect("stale rooms can be pruned");
        assert!(rooms.is_empty());
    }

    #[test]
    fn browser_metadata_survives_reordering_but_not_a_new_legacy_host() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = server.local_addr().unwrap();
        let mut browser =
            StockDiscoveryClient::bind_with_destination("127.0.0.1:0", address, "serverlist")
                .unwrap();
        let mut info = StockDiscoveryInfo::default_room(address.port(), "房间3️⃣🦋", "127.0.0.1");
        info.stage_id = Some("sn04".into());
        browser.tick(0).unwrap();
        for payload in [
            info.encode_unicode_room_summary().unwrap(),
            info.encode_room_summary().unwrap(),
            info.encode().unwrap(),
        ] {
            let bytes = StockControlPacket::DiscoverReply {
                sent_at: 0,
                info: payload,
            }
            .encode(0)
            .unwrap();
            server
                .send_to(&bytes, browser.local_addr().unwrap())
                .unwrap();
        }
        for now in 1..500 {
            if !browser.tick(now).unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert_eq!(browser.rooms()[0].info.stage_id.as_deref(), Some("sn04"));
        assert_eq!(browser.rooms()[0].info.server_name, "房间3️⃣🦋");
        browser.refresh();
        browser.tick(1000).unwrap();
        for (sent_at, payload) in [
            (0, info.encode_room_summary().unwrap()),
            (1000, info.encode().unwrap()),
        ] {
            let bytes = StockControlPacket::DiscoverReply {
                sent_at,
                info: payload,
            }
            .encode(0)
            .unwrap();
            server
                .send_to(&bytes, browser.local_addr().unwrap())
                .unwrap();
        }
        for now in 1001..1500 {
            if browser.tick(now).unwrap()[0].last_seen_ms > 1000 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(50));
        }
        assert_eq!(browser.rooms()[0].info.stage_id, None);
        assert_eq!(browser.rooms()[0].info.server_name, "房间3???");
    }

    #[test]
    fn browser_discovers_recreated_room_on_the_same_port() {
        let mut server = StockServerTransport::bind("127.0.0.1:0", "serverlist", "first").unwrap();
        let address = server.local_addr().unwrap();
        let mut browser =
            StockDiscoveryClient::bind_with_destination("127.0.0.1:0", address, "serverlist")
                .unwrap();
        for generation in 0..3 {
            let name = format!("room-{generation}");
            drop(server);
            server = StockServerTransport::bind(address, "serverlist", &name).unwrap();
            browser.refresh();
            let found = (generation * 1_000..generation * 1_000 + 500).any(|now| {
                let rooms = browser.tick(now).unwrap();
                server.tick(now).unwrap();
                std::thread::sleep(std::time::Duration::from_micros(50));
                rooms
                    .iter()
                    .any(|room| room.endpoint == address && room.info.server_name == name)
            });
            assert!(found, "same browser must discover generation {generation}");
            assert_eq!(browser.rooms().len(), 1);
        }
    }

    #[test]
    fn native_udp_loopback_completes_stock_handshake_and_application_flow() {
        let mut server = StockServerTransport::bind("127.0.0.1:0", "serverlist", "Rust stock room")
            .expect("server binds");
        let server_addr = server.local_addr().unwrap();
        let mut client = StockClientTransport::bind(
            "127.0.0.1:0",
            server_addr,
            StockConnectRecord::for_open_room(),
        )
        .expect("client binds");

        assert_eq!(client.measured_round_trip_ms(0), None);
        let mut client_connected = false;
        let mut server_connected = false;
        let mut now = 0;
        for _ in 0..500 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            client_connected |= client
                .tick(now)
                .unwrap()
                .iter()
                .any(|event| matches!(event, StockSessionEvent::Connected { .. }));
            server_connected |= server
                .tick(now)
                .unwrap()
                .iter()
                .any(|event| matches!(event, StockSessionEvent::PeerConnected { .. }));
            now += 1;
        }
        assert!(client.measured_round_trip_ms(now).is_some());
        assert!(client.measured_round_trip_ms(now + 10_000).is_none());
        assert!(client_connected);
        assert!(server_connected);
        assert_eq!(client.state(), StockClientState::Connected);
        assert_eq!(server.peer_count(), 1);

        let client_addr = client.local_addr().unwrap();
        client.set_logical_channel(7);
        server.set_peer_logical_channel(client_addr, 7).unwrap();
        client
            .send_application(1, [1, 2, 3], 0, now)
            .expect("client application sends");
        let mut server_events = Vec::new();
        for offset in 0..500 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            server_events.extend(server.tick(now + offset).unwrap());
            if server_events
                .iter()
                .any(|event| matches!(event, StockSessionEvent::Application { .. }))
            {
                break;
            }
        }
        assert!(
            server_events.iter().any(|event| matches!(
                event,
                StockSessionEvent::Application { peer, packet }
                    if *peer == client_addr
                        && packet.logical_channel == 7
                        && packet.opcode == 1
                        && packet.payload == [1, 2, 3]
            )),
            "server events: {server_events:?}"
        );

        server
            .send_application(client_addr, 2, [4, 5], 1, now)
            .expect("server application sends");
        let mut client_events = Vec::new();
        for offset in 0..500 {
            std::thread::sleep(std::time::Duration::from_micros(50));
            client_events.extend(client.tick(now + offset).unwrap());
            if client_events
                .iter()
                .any(|event| matches!(event, StockSessionEvent::Application { .. }))
            {
                break;
            }
        }
        assert!(client_events.iter().any(|event| matches!(
            event,
            StockSessionEvent::Application { peer, packet }
                if *peer == server_addr
                    && packet.logical_channel == 7
                    && packet.opcode == 2
                    && packet.payload == [4, 5]
        )));
    }

    #[test]
    fn stock_host_handshake_replies_are_unsequenced_and_do_not_consume_data_sequence() {
        let mut server = StockServerTransport::bind("127.0.0.1:0", "serverlist", "room").unwrap();
        let address = server.local_addr().unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.set_nonblocking(true).unwrap();
        let mut receive = |packet: StockControlPacket, now: u64| {
            client.send_to(&packet.encode(0).unwrap(), address).unwrap();
            let mut buffer = [0; STOCK_MAX_DATAGRAM_BYTES];
            for offset in 0..500 {
                server.tick(now + offset).unwrap();
                match client.recv_from(&mut buffer) {
                    Ok((length, _)) => return buffer[..length].to_vec(),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_micros(50));
                    }
                    Err(error) => panic!("receive failed: {error}"),
                }
            }
            panic!("host did not reply");
        };
        let challenge = receive(
            StockControlPacket::Connect {
                cookie: 0,
                record: vec![],
            },
            0,
        );
        let StockControlPacket::Challenge { cookie } =
            StockControlPacket::decode(&challenge, None).unwrap()
        else {
            panic!("expected challenge");
        };
        let connect = StockControlPacket::Connect {
            cookie,
            record: StockConnectRecord::for_open_room().encode().unwrap(),
        };
        let accepted = receive(connect.clone(), 500);
        let repeated = receive(connect, 1000);
        let mut original_receiver = StockReliability::default();
        for bytes in [&challenge, &accepted, &repeated] {
            let datagram = StockDatagram::decode(bytes).unwrap();
            assert_eq!((datagram.sequence, datagram.ack), (0, 0));
            assert_eq!(datagram.logical_channel, address.port());
            assert!(original_receiver.accept_datagram(&datagram));
        }
        assert!(matches!(
            StockControlPacket::decode(&accepted, None).unwrap(),
            StockControlPacket::Connected { .. }
        ));
        assert_eq!(server.peer_count(), 1);
        server
            .send_application(client.local_addr().unwrap(), 1, [1], 0, 1500)
            .unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        client.set_nonblocking(false).unwrap();
        let mut buffer = [0; STOCK_MAX_DATAGRAM_BYTES];
        let (length, _) = client.recv_from(&mut buffer).unwrap();
        let first_data = StockDatagram::decode(&buffer[..length]).unwrap();
        assert_eq!(first_data.sequence, 1);
        assert!(original_receiver.accept_datagram(&first_data));
    }

    #[test]
    fn client_connect_timeout_uses_stock_strict_twenty_second_boundary() {
        let unused = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = unused.local_addr().unwrap();
        drop(unused);
        let mut client =
            StockClientTransport::bind("127.0.0.1:0", address, StockConnectRecord::for_open_room())
                .unwrap();

        assert!(client.tick(0).unwrap().is_empty());
        assert!(client.tick(STOCK_CONNECT_TIMEOUT_MS).unwrap().is_empty());
        assert_eq!(client.state(), StockClientState::Connecting);
        assert!(client
            .tick(STOCK_CONNECT_TIMEOUT_MS + 1)
            .unwrap()
            .iter()
            .any(|event| matches!(
                event,
                StockSessionEvent::Disconnected { peer, reason }
                    if *peer == address && *reason == STOCK_TIMEOUT_REASON
            )));
        assert_eq!(client.state(), StockClientState::Disconnected);
    }

    #[test]
    fn connected_client_times_out_strictly_after_ten_silent_seconds() {
        let mut server =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "timeout").unwrap();
        let server_addr = server.local_addr().unwrap();
        let mut client = StockClientTransport::bind(
            "127.0.0.1:0",
            server_addr,
            StockConnectRecord::for_open_room(),
        )
        .unwrap();
        let connected_at = (0..500)
            .find(|now_ms| {
                std::thread::sleep(std::time::Duration::from_micros(50));
                let _ = server.tick(*now_ms).unwrap();
                client
                    .tick(*now_ms)
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, StockSessionEvent::Connected { .. }))
            })
            .expect("client connects");

        assert!(client
            .tick(connected_at + STOCK_IDLE_TIMEOUT_MS)
            .unwrap()
            .is_empty());
        assert!(client
            .tick(connected_at + STOCK_IDLE_TIMEOUT_MS + 1)
            .unwrap()
            .iter()
            .any(|event| matches!(
                event,
                StockSessionEvent::Disconnected { peer, reason }
                    if *peer == server_addr && *reason == STOCK_TIMEOUT_REASON
            )));
    }

    #[test]
    fn hosted_transport_scans_and_removes_silent_peer_with_reason_three() {
        let mut server =
            StockServerTransport::bind("127.0.0.1:0", "serverlist", "timeout").unwrap();
        let server_addr = server.local_addr().unwrap();
        let mut client = StockClientTransport::bind(
            "127.0.0.1:0",
            server_addr,
            StockConnectRecord::for_open_room(),
        )
        .unwrap();
        let server_connected_at = (0..500)
            .find(|now_ms| {
                std::thread::sleep(std::time::Duration::from_micros(50));
                let _ = client.tick(*now_ms).unwrap();
                server
                    .tick(*now_ms)
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, StockSessionEvent::PeerConnected { .. }))
            })
            .expect("server accepts client");
        assert_eq!(server.peer_count(), 1);

        assert!(server
            .tick(server_connected_at + STOCK_IDLE_TIMEOUT_MS)
            .unwrap()
            .is_empty());
        let events = server
            .tick(server_connected_at + STOCK_IDLE_TIMEOUT_MS + STOCK_SERVER_TIMEOUT_SCAN_MS + 1)
            .unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            StockSessionEvent::Disconnected { reason, .. }
                if *reason == STOCK_TIMEOUT_REASON
        )));
        assert_eq!(server.peer_count(), 0);
    }
}
