use std::collections::VecDeque;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use meteor_network::p2p::INPUT_BACKLOG_LIMIT;
use meteor_network::protocol::{Message, V4Payload};
use meteor_network::wire_v4::{
    Input as V4Input, RelayFromHost as V4RelayFromHost, RelayHost as V4RelayHost,
};
use serde_json::{json, Value as JsonValue};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{
    register_default_interceptors, MediaEngine, PeerConnection, PeerConnectionBuilder,
    PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceCandidateInit, RTCIceServer,
    RTCPeerConnectionIceEvent, RTCPeerConnectionState, RTCSessionDescription, Registry,
};
use webrtc::runtime::{default_runtime, Runtime};

const CONTROL_CHANNEL_LABEL: &str = "game";
const STATE_CHANNEL_LABEL: &str = "state";
const MAX_PENDING_CANDIDATES: usize = 32;

enum TransportEvent {
    IceCandidate {
        player_id: String,
        generation: String,
        candidate: JsonValue,
    },
    ChannelDiscovered {
        player_id: String,
        generation: String,
        label: String,
        channel: Arc<dyn DataChannel>,
    },
    ChannelOpen {
        player_id: String,
        generation: String,
        label: String,
    },
    ChannelDown {
        player_id: String,
        generation: String,
        label: Option<String>,
    },
    ChannelMessage {
        player_id: String,
        generation: String,
        label: String,
        bytes: Vec<u8>,
    },
}

pub(crate) enum NativeP2pOutput {
    WebSocketControl(Box<Message>),
    InboundFrame(Vec<u8>),
}

struct PeerEntry {
    player_id: String,
    generation: String,
    pc: Arc<dyn PeerConnection>,
    control: Option<Arc<dyn DataChannel>>,
    state: Option<Arc<dyn DataChannel>>,
    control_open: bool,
    state_open: bool,
    remote_description_set: bool,
    pending_candidates: VecDeque<RTCIceCandidateInit>,
}

impl PeerEntry {
    async fn close(&self) {
        if let Some(channel) = &self.state {
            let _ = channel.close().await;
        }
        if let Some(channel) = &self.control {
            let _ = channel.close().await;
        }
        let _ = self.pc.close().await;
    }
}

struct NativePeerHandler {
    player_id: String,
    generation: String,
    runtime: Arc<dyn Runtime>,
    events: tokio::sync::mpsc::UnboundedSender<TransportEvent>,
}

#[async_trait]
impl PeerConnectionEventHandler for NativePeerHandler {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let Ok(candidate) = event.candidate.to_json() else {
            return;
        };
        let Ok(candidate) = serde_json::to_value(candidate) else {
            return;
        };
        let _ = self.events.send(TransportEvent::IceCandidate {
            player_id: self.player_id.clone(),
            generation: self.generation.clone(),
            candidate,
        });
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if matches!(
            state,
            RTCPeerConnectionState::Disconnected
                | RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
        ) {
            let _ = self.events.send(TransportEvent::ChannelDown {
                player_id: self.player_id.clone(),
                generation: self.generation.clone(),
                label: None,
            });
        }
    }

    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        spawn_channel_pump(
            self.runtime.clone(),
            self.events.clone(),
            self.player_id.clone(),
            self.generation.clone(),
            channel,
        );
    }
}

fn spawn_channel_pump(
    runtime: Arc<dyn Runtime>,
    events: tokio::sync::mpsc::UnboundedSender<TransportEvent>,
    player_id: String,
    generation: String,
    channel: Arc<dyn DataChannel>,
) {
    runtime.spawn(Box::pin(async move {
        let Ok(label) = channel.label().await else {
            return;
        };
        if label != CONTROL_CHANNEL_LABEL && label != STATE_CHANNEL_LABEL {
            let _ = channel.close().await;
            return;
        }
        let _ = events.send(TransportEvent::ChannelDiscovered {
            player_id: player_id.clone(),
            generation: generation.clone(),
            label: label.clone(),
            channel: channel.clone(),
        });
        while let Some(event) = channel.poll().await {
            match event {
                DataChannelEvent::OnOpen => {
                    let _ = events.send(TransportEvent::ChannelOpen {
                        player_id: player_id.clone(),
                        generation: generation.clone(),
                        label: label.clone(),
                    });
                }
                DataChannelEvent::OnMessage(message) => {
                    let _ = events.send(TransportEvent::ChannelMessage {
                        player_id: player_id.clone(),
                        generation: generation.clone(),
                        label: label.clone(),
                        bytes: message.data.to_vec(),
                    });
                }
                DataChannelEvent::OnClosing
                | DataChannelEvent::OnClose
                | DataChannelEvent::OnError => {
                    let _ = events.send(TransportEvent::ChannelDown {
                        player_id: player_id.clone(),
                        generation: generation.clone(),
                        label: Some(label.clone()),
                    });
                    break;
                }
                _ => {}
            }
        }
    }));
}

pub(crate) struct NativeWebP2p {
    player_id: String,
    prepare_attempt: Option<String>,
    host_id: String,
    active_room_id: String,
    match_epoch: Option<u32>,
    round_epoch: Option<u32>,
    active_peer: bool,
    report_sent: bool,
    entry: Option<PeerEntry>,
    queued_candidates: VecDeque<(String, RTCIceCandidateInit)>,
    udp_bind_address: String,
    event_tx: tokio::sync::mpsc::UnboundedSender<TransportEvent>,
    event_rx: tokio::sync::mpsc::UnboundedReceiver<TransportEvent>,
}

impl NativeWebP2p {
    pub(crate) fn new() -> Self {
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            player_id: String::new(),
            prepare_attempt: None,
            host_id: String::new(),
            active_room_id: String::new(),
            match_epoch: None,
            round_epoch: None,
            active_peer: false,
            report_sent: false,
            entry: None,
            queued_candidates: VecDeque::new(),
            udp_bind_address: "0.0.0.0:0".into(),
            event_tx,
            event_rx,
        }
    }

    pub(crate) fn set_player_id(&mut self, player_id: String) {
        if !player_id.is_empty() {
            self.player_id = player_id;
        }
    }

    pub(crate) async fn handle_control(
        &mut self,
        message: &Message,
    ) -> Result<Vec<NativeP2pOutput>, String> {
        match message {
            Message::MatchPrepare {
                room_id,
                match_epoch,
                round_epoch,
                attempt_id,
                host_id,
                config,
                ..
            } => {
                self.teardown().await;
                if *round_epoch != 0
                    || config.match_epoch != *match_epoch
                    || config.round_epoch != *round_epoch
                {
                    return Ok(Vec::new());
                }
                self.active_room_id = room_id.clone();
                self.match_epoch = Some(*match_epoch);
                self.round_epoch = Some(*round_epoch);
                self.prepare_attempt = Some(attempt_id.clone());
                self.host_id = host_id.clone();
                self.report_sent = false;
                if host_id == &self.player_id {
                    self.report_sent = true;
                    return Ok(vec![NativeP2pOutput::WebSocketControl(Box::new(
                        Message::RtcReport {
                            attempt_id: attempt_id.clone(),
                            ok: false,
                        },
                    ))]);
                }
            }
            Message::RtcSignal {
                from_id, payload, ..
            } => {
                if from_id == &self.host_id && !from_id.is_empty() {
                    return self.handle_signal(from_id, payload).await;
                }
            }
            Message::MatchStart {
                room_id,
                match_epoch,
                round_epoch,
                authority,
                host_id,
                ..
            } => {
                if authority == "p2p"
                    && host_id != &self.player_id
                    && self.entry.as_ref().is_some_and(|entry| entry.control_open)
                {
                    self.active_peer = true;
                    self.active_room_id = room_id.clone();
                    self.host_id = host_id.clone();
                    self.match_epoch = *match_epoch;
                    self.round_epoch = *round_epoch;
                } else if authority != "p2p" {
                    self.teardown().await;
                }
                self.prepare_attempt = None;
            }
            Message::RoundStart {
                match_epoch,
                round_epoch,
                ..
            } if self.active_peer && *match_epoch == self.match_epoch => {
                self.round_epoch = *round_epoch;
            }
            Message::MatchReset | Message::RoomLeft => self.teardown().await,
            Message::MatchEnd { result, .. } if result.as_deref() == Some("aborted") => {
                self.teardown().await;
            }
            _ => {}
        }
        Ok(Vec::new())
    }

    async fn handle_signal(
        &mut self,
        from_id: &str,
        payload: &JsonValue,
    ) -> Result<Vec<NativeP2pOutput>, String> {
        let generation = payload
            .get("generation")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string();
        if generation.len() > 64
            || generation
                .chars()
                .any(|ch| !(ch.is_ascii_alphanumeric() || "._:-".contains(ch)))
        {
            return Ok(Vec::new());
        }
        if let Some(sdp) = payload.get("sdp") {
            if sdp.get("type").and_then(JsonValue::as_str) != Some("offer") {
                return Ok(Vec::new());
            }
            if let Some(entry) = self.entry.take() {
                entry.close().await;
            }
            let offer = serde_json::from_value::<RTCSessionDescription>(sdp.clone())
                .map_err(|error| format!("invalid WebRTC offer: {error}"))?;
            let pc = self.build_peer(from_id, &generation).await?;
            pc.set_remote_description(offer)
                .await
                .map_err(|error| format!("set remote offer: {error}"))?;
            let mut entry = PeerEntry {
                player_id: from_id.to_string(),
                generation: generation.clone(),
                pc,
                control: None,
                state: None,
                control_open: false,
                state_open: false,
                remote_description_set: true,
                pending_candidates: VecDeque::new(),
            };
            self.flush_queued_candidates(&mut entry).await;
            let answer = entry
                .pc
                .create_answer(None)
                .await
                .map_err(|error| format!("create WebRTC answer: {error}"))?;
            entry
                .pc
                .set_local_description(answer)
                .await
                .map_err(|error| format!("set local answer: {error}"))?;
            let local = entry
                .pc
                .local_description()
                .await
                .ok_or_else(|| "WebRTC answer has no local description".to_string())?;
            let sdp = serde_json::to_value(local)
                .map_err(|error| format!("serialize WebRTC answer: {error}"))?;
            self.entry = Some(entry);
            return Ok(vec![NativeP2pOutput::WebSocketControl(Box::new(
                Message::RtcSignal {
                    room_id: String::new(),
                    target_id: from_id.to_string(),
                    from_id: String::new(),
                    payload: json!({ "generation": generation, "sdp": sdp }),
                },
            ))]);
        }
        if let Some(candidate) = payload.get("candidate") {
            let candidate = serde_json::from_value::<RTCIceCandidateInit>(candidate.clone())
                .map_err(|error| format!("invalid ICE candidate: {error}"))?;
            let current = self.entry.as_ref().is_some_and(|entry| {
                entry.player_id == from_id
                    && entry.generation == generation
                    && entry.remote_description_set
            });
            if current {
                if let Some(entry) = self.entry.as_mut() {
                    let _ = entry.pc.add_ice_candidate(candidate).await;
                }
            } else {
                self.queued_candidates.push_back((generation, candidate));
                while self.queued_candidates.len() > MAX_PENDING_CANDIDATES {
                    self.queued_candidates.pop_front();
                }
            }
        }
        Ok(Vec::new())
    }

    async fn build_peer(
        &self,
        player_id: &str,
        generation: &str,
    ) -> Result<Arc<dyn PeerConnection>, String> {
        let runtime = default_runtime()
            .ok_or_else(|| "webrtc crate has no enabled async runtime".to_string())?;
        let mut media = MediaEngine::default();
        media
            .register_default_codecs()
            .map_err(|error| format!("register WebRTC codecs: {error}"))?;
        let registry = register_default_interceptors(Registry::new(), &mut media)
            .map_err(|error| format!("register WebRTC interceptors: {error}"))?;
        let configuration = RTCConfigurationBuilder::new()
            .with_ice_servers(default_ice_servers())
            .build();
        let handler = Arc::new(NativePeerHandler {
            player_id: player_id.to_string(),
            generation: generation.to_string(),
            runtime: runtime.clone(),
            events: self.event_tx.clone(),
        });
        let pc = PeerConnectionBuilder::new()
            .with_configuration(configuration)
            .with_media_engine(media)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_runtime(runtime)
            .with_udp_addrs(vec![self.udp_bind_address.clone()])
            .with_dedicated_reactor_thread(true)
            .with_data_channel_send_buffer_limit(INPUT_BACKLOG_LIMIT)
            .build()
            .await
            .map_err(|error| format!("build WebRTC peer: {error}"))?;
        Ok(Arc::new(pc) as Arc<dyn PeerConnection>)
    }

    async fn flush_queued_candidates(&mut self, entry: &mut PeerEntry) {
        let mut keep = VecDeque::new();
        while let Some((generation, candidate)) = self.queued_candidates.pop_front() {
            if generation == entry.generation {
                let _ = entry.pc.add_ice_candidate(candidate).await;
            } else {
                keep.push_back((generation, candidate));
            }
        }
        self.queued_candidates = keep;
        while let Some(candidate) = entry.pending_candidates.pop_front() {
            let _ = entry.pc.add_ice_candidate(candidate).await;
        }
    }

    pub(crate) async fn drain_outputs(&mut self) -> Vec<NativeP2pOutput> {
        let mut outputs = Vec::new();
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                TransportEvent::IceCandidate {
                    player_id,
                    generation,
                    candidate,
                } if self.entry_is_current(&player_id, &generation) => {
                    outputs.push(NativeP2pOutput::WebSocketControl(Box::new(
                        Message::RtcSignal {
                            room_id: String::new(),
                            target_id: player_id,
                            from_id: String::new(),
                            payload: json!({ "generation": generation, "candidate": candidate }),
                        },
                    )));
                }
                TransportEvent::ChannelDiscovered {
                    player_id,
                    generation,
                    label,
                    channel,
                } if self.entry_is_current(&player_id, &generation) => {
                    if let Some(entry) = self.entry.as_mut() {
                        match label.as_str() {
                            CONTROL_CHANNEL_LABEL => entry.control = Some(channel),
                            STATE_CHANNEL_LABEL => entry.state = Some(channel),
                            _ => {}
                        }
                    }
                }
                TransportEvent::ChannelOpen {
                    player_id,
                    generation,
                    label,
                } if self.entry_is_current(&player_id, &generation) => {
                    if let Some(entry) = self.entry.as_mut() {
                        match label.as_str() {
                            CONTROL_CHANNEL_LABEL => entry.control_open = true,
                            STATE_CHANNEL_LABEL => entry.state_open = true,
                            _ => {}
                        }
                    }
                    if label == CONTROL_CHANNEL_LABEL && !self.report_sent {
                        if let Some(attempt_id) = self.prepare_attempt.clone() {
                            self.report_sent = true;
                            outputs.push(NativeP2pOutput::WebSocketControl(Box::new(
                                Message::RtcReport {
                                    attempt_id,
                                    ok: true,
                                },
                            )));
                        }
                    }
                }
                TransportEvent::ChannelDown {
                    player_id,
                    generation,
                    label,
                } if self.entry_is_current(&player_id, &generation) => {
                    if let Some(entry) = self.entry.as_mut() {
                        match label.as_deref() {
                            Some(STATE_CHANNEL_LABEL) => entry.state_open = false,
                            _ => {
                                entry.control_open = false;
                            }
                        }
                    }
                }
                TransportEvent::ChannelMessage {
                    player_id,
                    generation,
                    label,
                    bytes,
                } if self.entry_is_current(&player_id, &generation) => {
                    if label == CONTROL_CHANNEL_LABEL {
                        if let Ok(V4Payload::Control(Message::RtcPing { id, sent_at })) =
                            Message::from_v4_payload(&bytes)
                        {
                            if let Some(channel) =
                                self.entry.as_ref().and_then(|entry| entry.control.as_ref())
                            {
                                if let Ok(reply) =
                                    V4Payload::Control(Message::RtcPong { id, sent_at })
                                        .to_v4_binary()
                                {
                                    let _ =
                                        channel.try_send(BytesMut::from(reply.as_slice())).await;
                                }
                            }
                            continue;
                        }
                    }
                    self.observe_authority_frame(&bytes);
                    outputs.push(NativeP2pOutput::InboundFrame(bytes));
                }
                _ => {}
            }
        }
        outputs
    }

    fn entry_is_current(&self, player_id: &str, generation: &str) -> bool {
        self.entry
            .as_ref()
            .is_some_and(|entry| entry.player_id == player_id && entry.generation == generation)
    }

    pub(crate) fn accept_relay_from_host(&mut self, relay: &V4RelayFromHost) -> bool {
        if !self.active_peer
            || relay.room_id != self.active_room_id
            || relay.match_epoch != self.match_epoch
        {
            return false;
        }
        let same_round = relay.round_epoch == self.round_epoch;
        let next_round = self.round_epoch.is_some_and(|round| {
            relay.round_epoch == Some(round.saturating_add(1))
                && matches!(
                    Message::from_v4_payload(&relay.payload),
                    Ok(V4Payload::Control(Message::RoundStart { .. }))
                )
        });
        if !same_round && !next_round {
            return false;
        }
        self.observe_authority_frame(&relay.payload);
        true
    }

    fn observe_authority_frame(&mut self, bytes: &[u8]) {
        if let Ok(V4Payload::Control(Message::RoundStart {
            match_epoch,
            round_epoch,
            ..
        })) = Message::from_v4_payload(bytes)
        {
            if match_epoch == self.match_epoch {
                self.round_epoch = round_epoch;
            }
        }
    }

    pub(crate) async fn send_input(&self, input: V4Input) -> Result<Option<Vec<u8>>, String> {
        if !self.active_peer
            || input.match_epoch != self.match_epoch
            || input.round_epoch != self.round_epoch
        {
            return Err("P2P peer is not active for this input epoch".into());
        }
        let Some(entry) = self.entry.as_ref() else {
            return relay_input(input).map(Some);
        };
        let has_state = !input.role_state.is_empty() || !input.combat.is_empty();
        let has_reliable = !input.actions.is_empty() || input.role_rate_request.is_some();
        if entry.state_open {
            if has_state {
                let mut state_input = input.clone();
                if has_reliable {
                    state_input.actions.clear();
                    state_input.role_rate_request = None;
                }
                if let Some(state) = entry.state.as_ref() {
                    let outstanding = state.outstanding_bytes().await.unwrap_or_default();
                    if outstanding <= INPUT_BACKLOG_LIMIT {
                        let bytes = V4Payload::Input(state_input)
                            .to_v4_binary()
                            .map_err(|error| error.to_string())?;
                        if state
                            .try_send(BytesMut::from(bytes.as_slice()))
                            .await
                            .is_err()
                        {
                            let full = V4Payload::Input(input.clone())
                                .to_v4_binary()
                                .map_err(|error| error.to_string())?;
                            if entry.control_open {
                                if let Some(control) = entry.control.as_ref() {
                                    if control
                                        .try_send(BytesMut::from(full.as_slice()))
                                        .await
                                        .is_ok()
                                    {
                                        return Ok(None);
                                    }
                                }
                            }
                            return relay_input(input).map(Some);
                        }
                    }
                }
            }
            if has_reliable {
                let mut reliable = input.clone();
                reliable.role_state.clear();
                reliable.combat.clear();
                reliable.visual = None;
                let bytes = V4Payload::Input(reliable.clone())
                    .to_v4_binary()
                    .map_err(|error| error.to_string())?;
                if entry.control_open {
                    if let Some(control) = entry.control.as_ref() {
                        if control
                            .try_send(BytesMut::from(bytes.as_slice()))
                            .await
                            .is_ok()
                        {
                            return Ok(None);
                        }
                    }
                }
                return relay_input(reliable).map(Some);
            }
            return Ok(None);
        }
        let bytes = V4Payload::Input(input.clone())
            .to_v4_binary()
            .map_err(|error| error.to_string())?;
        if entry.control_open {
            if let Some(control) = entry.control.as_ref() {
                if control
                    .try_send(BytesMut::from(bytes.as_slice()))
                    .await
                    .is_ok()
                {
                    return Ok(None);
                }
            }
        }
        relay_input(input).map(Some)
    }

    pub(crate) fn is_active_peer(&self) -> bool {
        self.active_peer
    }

    #[cfg(test)]
    fn use_test_loopback(&mut self) {
        self.udp_bind_address = "127.0.0.1:0".into();
    }

    pub(crate) async fn teardown(&mut self) {
        if let Some(entry) = self.entry.take() {
            entry.close().await;
        }
        self.prepare_attempt = None;
        self.host_id.clear();
        self.active_room_id.clear();
        self.match_epoch = None;
        self.round_epoch = None;
        self.active_peer = false;
        self.report_sent = false;
        self.queued_candidates.clear();
        while self.event_rx.try_recv().is_ok() {}
    }
}

fn relay_input(input: V4Input) -> Result<Vec<u8>, String> {
    let match_epoch = input.match_epoch;
    let round_epoch = input.round_epoch;
    let payload = V4Payload::Input(input)
        .to_v4_binary()
        .map_err(|error| error.to_string())?;
    V4Payload::RelayHost(V4RelayHost {
        payload,
        match_epoch,
        round_epoch,
    })
    .to_v4_binary()
    .map_err(|error| error.to_string())
}

fn ice_servers(turn_urls: &str, username: &str, credential: &str) -> Vec<RTCIceServer> {
    let mut servers = vec![RTCIceServer {
        urls: vec![
            "stun:stun.l.google.com:19302".into(),
            "stun:stun1.l.google.com:19302".into(),
            "stun:stun.cloudflare.com:3478".into(),
        ],
        ..Default::default()
    }];
    let urls: Vec<String> = turn_urls
        .split(',')
        .map(str::trim)
        .filter(|url| url.starts_with("turn:") || url.starts_with("turns:"))
        .map(str::to_owned)
        .collect();
    if !urls.is_empty() {
        servers.push(RTCIceServer {
            urls,
            username: username.to_owned(),
            credential: credential.to_owned(),
        });
    }
    servers
}

fn default_ice_servers() -> Vec<RTCIceServer> {
    ice_servers(
        &std::env::var("METEOR_TURN_URLS").unwrap_or_default(),
        &std::env::var("METEOR_TURN_USERNAME").unwrap_or_default(),
        &std::env::var("METEOR_TURN_PASSWORD").unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use meteor_network::protocol::{P2pMatchConfig, RoomRulesWire};
    use std::time::Duration;
    use webrtc::data_channel::RTCDataChannelInit;

    struct OffererHandler {
        candidates: tokio::sync::mpsc::UnboundedSender<JsonValue>,
    }

    #[async_trait]
    impl PeerConnectionEventHandler for OffererHandler {
        async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
            let Ok(candidate) = event.candidate.to_json() else {
                return;
            };
            if let Ok(candidate) = serde_json::to_value(candidate) {
                let _ = self.candidates.send(candidate);
            }
        }
    }

    async fn build_loopback_offerer(
        candidates: tokio::sync::mpsc::UnboundedSender<JsonValue>,
    ) -> Arc<dyn PeerConnection> {
        let runtime = default_runtime().expect("tokio WebRTC runtime");
        let mut media = MediaEngine::default();
        media.register_default_codecs().unwrap();
        let registry = register_default_interceptors(Registry::new(), &mut media).unwrap();
        let pc = PeerConnectionBuilder::new()
            .with_configuration(RTCConfigurationBuilder::new().build())
            .with_media_engine(media)
            .with_interceptor_registry(registry)
            .with_handler(Arc::new(OffererHandler { candidates }))
            .with_runtime(runtime)
            .with_udp_addrs(vec!["127.0.0.1:0".to_string()])
            .with_dedicated_reactor_thread(true)
            .build()
            .await
            .expect("loopback offerer builds");
        Arc::new(pc) as Arc<dyn PeerConnection>
    }

    fn pump_test_channel(channel: Arc<dyn DataChannel>) {
        tokio::spawn(async move {
            while let Some(event) = channel.poll().await {
                if matches!(event, DataChannelEvent::OnClose | DataChannelEvent::OnError) {
                    break;
                }
            }
        });
    }

    fn sample_input() -> V4Input {
        V4Input {
            seq: 7,
            role_state: vec![1; 20],
            role_rate_request: Some(16),
            r#move: None,
            yaw: 0.0,
            run: false,
            guard: false,
            actions: vec!["cure".into()],
            combat: Vec::new(),
            visual: None,
            match_epoch: Some(3),
            round_epoch: Some(1),
        }
    }

    #[test]
    fn relay_envelope_preserves_epochs_and_nested_input() {
        let bytes = relay_input(sample_input()).expect("relay encodes");
        let V4Payload::RelayHost(relay) = Message::from_v4_payload(&bytes).expect("relay decodes")
        else {
            panic!("expected relay.host");
        };
        assert_eq!(relay.match_epoch, Some(3));
        assert_eq!(relay.round_epoch, Some(1));
        let V4Payload::Input(input) =
            Message::from_v4_payload(&relay.payload).expect("nested input decodes")
        else {
            panic!("expected nested input");
        };
        assert_eq!(input.seq, 7);
        assert_eq!(input.role_state, vec![1; 20]);
        assert_eq!(input.actions, vec!["cure"]);
    }

    #[test]
    fn ice_configuration_defaults_to_public_stun_without_credentials() {
        let servers = ice_servers("", "", "");
        assert_eq!(servers.len(), 1);
        assert!(servers[0].urls.iter().all(|url| url.starts_with("stun:")));
        assert!(servers[0].username.is_empty());
        assert!(servers[0].credential.is_empty());
    }

    #[test]
    fn ice_configuration_accepts_explicit_turn_settings() {
        let servers = ice_servers(
            "turn:relay.example.invalid:3478, turns:relay.example.invalid:5349",
            "example-user",
            "example-password",
        );
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[1].urls.len(), 2);
        assert_eq!(servers[1].username, "example-user");
        assert_eq!(servers[1].credential, "example-password");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_peer_completes_real_game_and_state_datachannel_handshake() {
        let (candidate_tx, mut candidate_rx) = tokio::sync::mpsc::unbounded_channel();
        let offerer = build_loopback_offerer(candidate_tx).await;
        let game = offerer
            .create_data_channel(CONTROL_CHANNEL_LABEL, Some(RTCDataChannelInit::default()))
            .await
            .unwrap();
        let state = offerer
            .create_data_channel(
                STATE_CHANNEL_LABEL,
                Some(RTCDataChannelInit {
                    ordered: false,
                    max_retransmits: Some(0),
                    ..Default::default()
                }),
            )
            .await
            .unwrap();
        pump_test_channel(game);
        pump_test_channel(state);

        let mut peer = NativeWebP2p::new();
        peer.use_test_loopback();
        peer.set_player_id("rust".into());
        peer.handle_control(&Message::MatchPrepare {
            room_id: "LOOP01".into(),
            match_epoch: 1,
            round_epoch: 0,
            attempt_id: "rtc_loop".into(),
            host_id: "web".into(),
            timeout_ms: 8_000,
            config: P2pMatchConfig {
                match_epoch: 1,
                round_epoch: 0,
                mode: "leader".into(),
                stage_id: "sn01".into(),
                configured: true,
                room_name: "loopback".into(),
                rules: RoomRulesWire::web_defaults(),
                players: Vec::new(),
            },
        })
        .await
        .unwrap();

        let offer = offerer.create_offer(None).await.unwrap();
        offerer.set_local_description(offer).await.unwrap();
        let offer = offerer.local_description().await.unwrap();
        let response = peer
            .handle_control(&Message::RtcSignal {
                room_id: "LOOP01".into(),
                target_id: String::new(),
                from_id: "web".into(),
                payload: json!({
                    "generation": "loop-1",
                    "sdp": serde_json::to_value(offer).unwrap(),
                }),
            })
            .await
            .unwrap();
        let answer = response.into_iter().find_map(|output| {
            let NativeP2pOutput::WebSocketControl(message) = output else {
                return None;
            };
            let Message::RtcSignal { payload, .. } = *message else {
                return None;
            };
            payload.get("sdp").cloned()
        });
        let answer = serde_json::from_value::<RTCSessionDescription>(
            answer.expect("native peer emits SDP answer"),
        )
        .unwrap();
        offerer.set_remote_description(answer).await.unwrap();

        let reported = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                while let Ok(candidate) = candidate_rx.try_recv() {
                    let _ = peer
                        .handle_control(&Message::RtcSignal {
                            room_id: "LOOP01".into(),
                            target_id: String::new(),
                            from_id: "web".into(),
                            payload: json!({
                                "generation": "loop-1",
                                "candidate": candidate,
                            }),
                        })
                        .await;
                }
                for output in peer.drain_outputs().await {
                    if let NativeP2pOutput::WebSocketControl(message) = output {
                        match *message {
                            Message::RtcSignal { payload, .. } => {
                                if let Some(candidate) = payload.get("candidate") {
                                    let candidate = serde_json::from_value::<RTCIceCandidateInit>(
                                        candidate.clone(),
                                    )
                                    .unwrap();
                                    let _ = offerer.add_ice_candidate(candidate).await;
                                }
                            }
                            Message::RtcReport { attempt_id, ok }
                                if attempt_id == "rtc_loop" && ok =>
                            {
                                return true
                            }
                            _ => {}
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("loopback WebRTC handshake timed out");
        assert!(reported);
        peer.teardown().await;
        let _ = offerer.close().await;
    }
}
