use futures_util::{SinkExt, StreamExt};
use meteor_network::host_authority::TICK_RATE;
use meteor_network::protocol::{Message, V4Payload};
use meteor_network::server::{Dispatch, RoomServer, WireFormat};
use meteor_network::wire_v4::Input as V4Input;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message as WsMessage;

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

enum IncomingMessage {
    Control(Message),
    Input(V4Input),
}

type ServerState = Arc<Mutex<RoomServer>>;

type Outbox = Arc<Mutex<HashMap<u64, UnboundedSender<WsMessage>>>>;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let port = std::env::var("PORT").unwrap_or_else(|_| "9001".into());
    let address = format!("127.0.0.1:{port}");
    let root = meteor_asset::pack_repository::default_pack_directory().unwrap_or_else(|error| {
        log::error!("{error}");
        std::process::exit(1)
    });
    let check_resources = std::env::args().any(|arg| arg == "--check-resources");
    if let Err(error) = meteor_asset::runtime_assets::initialize_native(&root) {
        log::error!("{error}");
        std::process::exit(1);
    }
    let server = RoomServer::from_mpk().unwrap_or_else(|error| {
        log::error!("required MPK combat resources: {error}");
        std::process::exit(1)
    });
    log::info!(
        "loaded authoritative combat catalog from MPK {}",
        root.display()
    );
    if check_resources {
        let repository =
            meteor_asset::pack_repository::PackRepository::open(&root).expect("MPK manifest");
        repository.verify_all().unwrap_or_else(|error| {
            log::error!("{error}");
            std::process::exit(1)
        });
        return;
    }
    let listener = TcpListener::bind(&address)
        .await
        .unwrap_or_else(|e| panic!("bind {address}: {e}"));
    log::info!("meteor-server listening on ws://{address}");
    let state: ServerState = Arc::new(Mutex::new(server));
    let outbox: Outbox = Arc::new(Mutex::new(HashMap::new()));
    start_authority_clock(state.clone(), outbox.clone());
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            continue;
        };
        let state = state.clone();
        let outbox = outbox.clone();
        tokio::spawn(async move {
            let conn_id = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
            log::info!("connection {conn_id} from {peer}");
            if let Err(error) =
                handle_connection(conn_id, stream, state.clone(), outbox.clone()).await
            {
                log::warn!("connection {conn_id} ended: {error}");
            }

            let targets = {
                let state = state.lock().unwrap();
                state.room_connections(conn_id)
            };
            let left = {
                let mut state = state.lock().unwrap();
                state.disconnect(conn_id)
            };
            for target in targets {
                for message in &left {
                    send_to(&outbox, &state, target, message);
                }
            }
            outbox.lock().unwrap().remove(&conn_id);
            log::info!("connection {conn_id} closed");
        });
    }
}

fn start_authority_clock(state: ServerState, outbox: Outbox) {
    tokio::spawn(async move {
        let tick_duration = Duration::from_secs_f64(1.0 / f64::from(TICK_RATE));
        let mut ticker = tokio::time::interval(tick_duration);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let server_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                * 1_000.0;
            let authority = state
                .lock()
                .unwrap()
                .advance_authority(1.0 / TICK_RATE as f32, server_time);
            for (conn_id, message) in authority.controls {
                send_to(&outbox, &state, conn_id, &message);
            }
            for (conn_id, message) in authority.events {
                send_to(&outbox, &state, conn_id, &message);
            }
            for (conn_id, snapshot) in authority.role_snapshots {
                send_v4_to(&outbox, &state, conn_id, &V4Payload::RoleSnapshot(snapshot));
            }
            for (conn_id, snapshot) in authority.snapshots {
                send_v4_to(&outbox, &state, conn_id, &V4Payload::Snapshot(snapshot));
            }
        }
    });
}

async fn handle_connection(
    conn_id: u64,
    stream: TcpStream,
    state: ServerState,
    outbox: Outbox,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut writer, mut reader) = ws.split();
    {
        let mut state = state.lock().unwrap();
        state.connect(conn_id);
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();
    outbox.lock().unwrap().insert(conn_id, tx.clone());
    let mut writer_task = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(Duration::from_secs(1));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let frame = tokio::select! {
                frame = rx.recv() => match frame {
                    Some(frame) => frame,
                    None => break,
                },
                _ = heartbeat.tick() => WsMessage::Ping(Vec::new().into()),
            };
            if writer.send(frame).await.is_err() {
                break;
            }
        }
    });
    let result = async {
        loop {

            let frame = tokio::select! {
                _ = &mut writer_task => break,
                frame = tokio::time::timeout(Duration::from_secs(5), reader.next()) => frame?,
            };
            let Some(frame) = frame else { break; };
            let frame = frame?;
            if frame.is_close() {
                break;
            }
            if frame.is_ping() {
                send_to(
                    &outbox,
                    &state,
                    conn_id,
                    &Message::Pong {
                        client_time: None,
                        server_time: None,
                    },
                );
                continue;
            }
            let incoming = match frame {
                WsMessage::Text(text) => Message::from_json(&text).map(IncomingMessage::Control),
                WsMessage::Binary(bytes) => {
                    let payload = Message::from_v4_payload(&bytes).ok();
                    if payload.is_some() {
                        state
                            .lock()
                            .unwrap()
                            .set_wire_format(conn_id, WireFormat::V4);
                    }
                    match payload {
                        Some(V4Payload::Control(message)) => Some(IncomingMessage::Control(message)),
                        Some(V4Payload::Input(input)) => Some(IncomingMessage::Input(input)),
                        Some(_) => {

                            send_to(
                                &outbox,
                                &state,
                                conn_id,
                                &Message::Error {
                                    code: "v4_unsupported".into(),
                                    text: "v4 snapshot and relay payloads are not available on the server authority path".into(),
                                },
                            );
                            continue;
                        }
                        None => None,
                    }
                }
                _ => continue,
            };
            let Some(incoming) = incoming else {
                send_to(
                    &outbox,
                    &state,
                    conn_id,
                    &Message::Error {
                        code: "bad_message".into(),
                        text: "unparseable protocol frame".into(),
                    },
                );
                continue;
            };
            let (dispatch, others, room) = {
                let mut state = state.lock().unwrap();
                let dispatch = match incoming {
                    IncomingMessage::Control(message) => {
                        log::info!("conn {conn_id} received: {}", message.type_name());
                        state.handle(conn_id, message)
                    }
                    IncomingMessage::Input(input) => {
                        log::debug!("conn {conn_id} received v4 input seq {}", input.seq);
                        state.handle_v4_input(conn_id, input)
                    }
                };
                log::debug!(
                    "conn {conn_id} handled: {}",
                    dispatch.to_sender.len()
                        + dispatch.to_others.len()
                        + dispatch.to_room.len()
                        + dispatch.to_conn.len()
                );

                let others = state.room_connections(conn_id);
                let room = state.room_all_connections(conn_id);
                (dispatch, others, room)
            };
            route(&outbox, &state, conn_id, &dispatch, &others, &room);
        }
        Ok(())
    }
    .await;

    writer_task.abort();
    drop(tx);
    outbox.lock().unwrap().remove(&conn_id);
    result
}

fn route(
    outbox: &Outbox,
    state: &ServerState,
    conn_id: u64,
    dispatch: &Dispatch,
    others: &[u64],
    room: &[u64],
) {
    for (target_conn, code, reason) in &dispatch.close_conn {
        if let Some(tx) = outbox.lock().unwrap().get(target_conn) {
            let frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: (*code).into(),
                reason: reason.clone().into(),
            };
            let _ = tx.send(WsMessage::Close(Some(frame)));
        }
    }
    for message in &dispatch.to_sender {
        send_to(outbox, state, conn_id, message);
    }
    for target in others {
        for message in &dispatch.to_others {
            send_to(outbox, state, *target, message);
        }
    }
    for target in room {
        for message in &dispatch.to_room {
            send_to(outbox, state, *target, message);
        }
    }
    for (target_conn, message) in &dispatch.to_conn {
        send_to(outbox, state, *target_conn, message);
    }
}

fn send_to(outbox: &Outbox, state: &ServerState, conn_id: u64, message: &Message) {
    let frame = match state.lock().unwrap().wire_format(conn_id) {
        WireFormat::Json => WsMessage::Text(message.to_json().into()),
        WireFormat::V4 => match message.to_v4_binary() {
            Ok(bytes) => WsMessage::Binary(bytes.into()),
            Err(error) => {
                log::warn!(
                    "v4 connection {conn_id}: cannot encode {}: {error}",
                    message.type_name()
                );
                let fallback = Message::Error {
                    code: "v4_unsupported".into(),
                    text: format!(
                        "{} is not available on the v4 authority path",
                        message.type_name()
                    ),
                };
                let bytes = fallback
                    .to_v4_binary()
                    .expect("error is always a protocol-v4 control message");
                WsMessage::Binary(bytes.into())
            }
        },
    };
    if let Some(tx) = outbox.lock().unwrap().get(&conn_id) {
        let _ = tx.send(frame);
    }
}

fn send_v4_to(outbox: &Outbox, state: &ServerState, conn_id: u64, payload: &V4Payload) {
    if state.lock().unwrap().wire_format(conn_id) != WireFormat::V4 {
        return;
    }
    let bytes = match payload.to_v4_binary() {
        Ok(bytes) => bytes,
        Err(error) => {
            log::warn!("v4 connection {conn_id}: cannot encode authority payload: {error}");
            return;
        }
    };
    if let Some(tx) = outbox.lock().unwrap().get(&conn_id) {
        let _ = tx.send(WsMessage::Binary(bytes.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broken_websockets_return_promptly_and_release_output_queue() {
        for silent in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let state = Arc::new(Mutex::new(RoomServer::new()));
            let outbox = Outbox::default();
            let server_state = state.clone();
            let server_outbox = outbox.clone();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                handle_connection(1, stream, server_state, server_outbox).await
            });
            let (client, _) = tokio_tungstenite::connect_async(format!("ws://{address}"))
                .await
                .unwrap();
            if silent {
                let result = tokio::time::timeout(Duration::from_secs(7), server)
                    .await
                    .expect("silent connection must time out")
                    .unwrap();
                assert!(result.is_err());
                drop(client);
            } else {
                drop(client);
                let _ = tokio::time::timeout(Duration::from_secs(2), server)
                    .await
                    .expect("reset connection must end immediately")
                    .unwrap();
            }
            assert!(outbox.lock().unwrap().is_empty());
        }
    }
}
