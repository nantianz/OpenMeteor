use meteor_network::stock_udp::{
    stock_integrity_checksum, StockApplicationPacket, StockConnectRecord, StockControlPacket,
    StockDatagram, StockReliability, STOCK_MAX_DATAGRAM_BYTES, STOCK_PACKET_TYPE_APPLICATION,
};
use std::env;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn outgoing(bytes: &[u8], destination: SocketAddr, elapsed_ms: u64, note: &str) {
    println!(
        "[{elapsed_ms:>7} ms] SEND -> {destination} {note} hex={}",
        hex(bytes)
    );
}

fn send_reliable_application(
    socket: &UdpSocket,
    reliability: &mut StockReliability,
    destination: SocketAddr,
    opcode: u16,
    payload: Vec<u8>,
    now_ms: u64,
    note: &str,
) {
    let packet = StockApplicationPacket::new(0, opcode, payload, 1).expect("packet builds");
    let packet = reliability.prepare_outbound(packet, now_ms);
    let bytes = packet.encode().expect("packet encodes");
    outgoing(&bytes, destination, now_ms, note);
    socket.send_to(&bytes, destination).expect("send");
}

fn describe_incoming(bytes: &[u8], peer: SocketAddr, elapsed_ms: u64, cookie: Option<u32>) {
    println!("[{elapsed_ms:>7} ms] RECV {peer} len={}", bytes.len());
    println!("           hex={}", hex(bytes));
    let Ok(datagram) = StockDatagram::decode(bytes) else {
        println!("           (10-byte header decode failed)");
        return;
    };
    println!(
        "           type=0x{:02x} flags=0x{:02x} seq={} ack={} chan={} body_len={}",
        datagram.packet_type,
        datagram.flags,
        datagram.sequence,
        datagram.ack,
        datagram.logical_channel,
        datagram.body.len()
    );
    if datagram.packet_type == STOCK_PACKET_TYPE_APPLICATION {
        if let Ok(StockApplicationPacket {
            opcode, payload, ..
        }) = StockApplicationPacket::decode(bytes)
        {
            println!("           app opcode={opcode} payload={}", hex(&payload));
        }
        return;
    }

    match StockControlPacket::decode(bytes, None) {
        Ok(control) => println!("           control={control:?}"),
        Err(without_key) => match (cookie, StockControlPacket::decode(bytes, cookie)) {
            (Some(_), Ok(control)) => {
                println!("           control(xor)={control:?}");
            }
            _ => println!("           control decode failed: {without_key}"),
        },
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let target = args
        .first()
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:5230".into());

    let identity = args.get(1).cloned().unwrap_or_else(|| {
        let checksum = stock_integrity_checksum(|path| {
            std::fs::read(format!("../origin_res/{}", path.replace('\\', "/"))).ok()
        });
        println!("integrity checksum = {checksum}");
        format!(
            "hex:{}",
            hex(&StockConnectRecord::new("", checksum)
                .encode()
                .expect("record encodes"))
        )
    });
    let identity_bytes: Vec<u8> = if let Some(hex_text) = identity.strip_prefix("hex:") {
        (0..hex_text.len())
            .step_by(2)
            .map(|offset| {
                u8::from_str_radix(hex_text.get(offset..offset + 2).expect("偶数长度 hex"), 16)
                    .expect("合法 hex 字节")
            })
            .collect()
    } else {
        identity.clone().into_bytes()
    };
    let destination: SocketAddr = target
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
        .unwrap_or_else(|| panic!("目标地址无法解析: {target}"));
    println!(
        "probe target={destination} identity={identity:?} wire={}",
        hex(&identity_bytes)
    );

    let socket = UdpSocket::bind("0.0.0.0:0").expect("bind ephemeral");
    socket.set_nonblocking(true).expect("nonblocking");
    println!("local={}", socket.local_addr().expect("local addr"));

    let start = Instant::now();

    let mut reliability = StockReliability::default();
    let discover = StockControlPacket::Discover {
        sent_at: start.elapsed().as_millis() as u32,
        query: "serverlist".into(),
    }
    .encode(0)
    .expect("discover encodes");
    let discover = reliability.prepare_control(&discover).expect("decorated");
    outgoing(&discover, destination, 0, "discover(serverlist)");
    socket
        .send_to(&discover, destination)
        .expect("discover sends");

    let mut cookie: Option<u32> = None;
    let mut transport_id: Option<u32> = None;
    let logical_channel: u16 = 0;
    let mut connected = false;
    let mut last_connect_ms: Option<u64> = None;
    let mut last_ping_ms: Option<u64> = None;

    let hero: u32 = std::env::var("PROBE_HERO")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let camp: u32 = std::env::var("PROBE_CAMP")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let weapon_item_id: u32 = std::env::var("PROBE_WEAPON")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(6);
    let pos_checksum = std::fs::read(format!("../origin_res/pmodel/P{hero}.POS"))
        .map(|bytes| meteor_network::stock_protocol::stock_resource_checksum(&bytes))
        .unwrap_or(0);
    println!(
        "entry: hero={hero} camp={camp} weapon_item={weapon_item_id} pos_checksum={pos_checksum:#x}"
    );
    let mut entry_sent = false;
    let mut own_event9_seen = false;
    let mut weapon_sent = false;

    let deadline = start + Duration::from_secs(45);
    let mut phase = 1;
    while Instant::now() < deadline {
        let now_ms = start.elapsed().as_millis() as u64;
        if phase == 1 && now_ms >= 3_000 {
            println!("[{now_ms:>7} ms] phase1(discover) 结束，进入 phase2(connect)");
            phase = 2;
        }

        if phase == 2 && !connected {
            if last_connect_ms.is_none_or(|last| now_ms - last >= 250) {
                let cookie_value = cookie.unwrap_or(0);
                let mut body = cookie_value.to_le_bytes().to_vec();
                if cookie_value != 0 {
                    body.extend_from_slice(&identity_bytes);
                    body.push(0);
                    let key = cookie_value.to_le_bytes();
                    for (index, byte) in body.iter_mut().enumerate() {
                        *byte ^= key[index & 3];
                    }
                }
                let datagram = StockDatagram {
                    packet_type: 2,
                    flags: if cookie_value != 0 { 0x10 } else { 0 },
                    sequence: 0,
                    ack: 0,
                    logical_channel: 0,
                    body,
                };
                let bytes = datagram.encode().expect("connect encodes");
                let bytes = reliability.prepare_control(&bytes).expect("decorated");
                outgoing(
                    &bytes,
                    destination,
                    now_ms,
                    &format!("connect(cookie={cookie_value:#x})"),
                );
                socket.send_to(&bytes, destination).expect("connect sends");
                last_connect_ms = Some(now_ms);
            }
        } else if connected && last_ping_ms.is_none_or(|last| now_ms - last >= 1_000) {
            let ping = StockControlPacket::Ping {
                client_time: now_ms as u32,
                latency: 0,
            };
            let bytes = ping.encode(logical_channel).expect("ping encodes");
            let bytes = reliability.prepare_control(&bytes).expect("decorated");
            outgoing(&bytes, destination, now_ms, "ping");
            socket.send_to(&bytes, destination).expect("ping sends");
            last_ping_ms = Some(now_ms);
        }

        if connected && !entry_sent {
            let mut payload = hero.to_le_bytes().to_vec();
            payload.extend_from_slice(&camp.to_le_bytes());
            payload.extend_from_slice(&pos_checksum.to_le_bytes());
            send_reliable_application(
                &socket,
                &mut reliability,
                destination,
                4,
                payload,
                now_ms,
                "request4(entry)",
            );
            entry_sent = true;
        }
        if connected && own_event9_seen && !weapon_sent {
            send_reliable_application(
                &socket,
                &mut reliability,
                destination,
                6,
                weapon_item_id.to_le_bytes().to_vec(),
                now_ms,
                "request6(weapon)",
            );
            weapon_sent = true;
        }

        let mut buffer = [0u8; STOCK_MAX_DATAGRAM_BYTES];
        match socket.recv_from(&mut buffer) {
            Ok((length, peer)) => {
                let bytes = &buffer[..length];
                describe_incoming(bytes, peer, now_ms, cookie);

                let Ok(datagram) = StockDatagram::decode(bytes) else {
                    continue;
                };
                if peer != destination {
                    println!("           (来源端口与目标不同，生产 transport 会静默丢弃此包)");
                }
                if !reliability.accept_datagram(&datagram) {
                    println!("           (reliability 门拒绝：重复/乱序)");
                    continue;
                }
                if datagram.packet_type == STOCK_PACKET_TYPE_APPLICATION {
                    if !connected {
                        println!("           (未连接时收到 application，生产会丢弃)");
                    }

                    if let Ok(packet) = StockApplicationPacket::decode(bytes) {
                        if packet.opcode == 9 && packet.payload.len() >= 8 {
                            let actor =
                                u32::from_le_bytes(packet.payload[0..4].try_into().expect("u32"));
                            if Some(actor) == transport_id {
                                own_event9_seen = true;
                                println!("           -> 自身 event 9 到达，准备 request 6");
                            }
                        }
                    }
                    continue;
                }
                match StockControlPacket::decode(bytes, None) {
                    Ok(StockControlPacket::Challenge { cookie: value })
                        if !connected && cookie.is_none() =>
                    {
                        cookie = Some(value);
                        reliability = StockReliability::default();
                        last_connect_ms = None;
                        println!(
                            "           -> 记录 challenge cookie={value:#010x}，立即回加密 connect"
                        );
                    }
                    Ok(StockControlPacket::Connected { transport_id: id }) if !connected => {
                        transport_id = Some(id);
                        connected = true;
                        println!("           -> CONNECTED transport_id={id}");
                    }
                    Ok(StockControlPacket::Pong { .. }) => {}
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("recv failed: {error}"),
        }

        std::thread::sleep(Duration::from_millis(10));
    }

    println!("结束：connected={connected} cookie={cookie:?} transport_id={transport_id:?}");
}
