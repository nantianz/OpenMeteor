use meteor_network::role_state::{role_state_from_runtime, RoleStateRuntime, RoleVec3};
use meteor_network::stock_game::{StockGameClient, StockGameEvent, StockRoleSendPacer};
use meteor_network::stock_session::StockClientTransport;
use meteor_network::stock_udp::{stock_integrity_checksum, StockConnectRecord};
use std::io::Write;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let target = args.first().map(String::as_str).unwrap_or("127.0.0.1:5230");
    let seconds: u64 = args.get(1).map(String::as_str).unwrap_or("30").parse()?;
    let mut trace = std::fs::File::create(
        args.get(2)
            .map(String::as_str)
            .unwrap_or("stock-motion.csv"),
    )?;
    writeln!(trace, "time_ms,kind,actor,x,y,z,vx,vy,vz,word0,word1")?;
    let checksum = stock_integrity_checksum(|path| {
        std::fs::read(format!("../origin_res/{}", path.replace('\\', "/"))).ok()
    });
    let make_client = || -> Result<StockGameClient, Box<dyn std::error::Error>> {
        let transport =
            StockClientTransport::bind("0.0.0.0:0", target, StockConnectRecord::new("", checksum))?;
        Ok(StockGameClient::from_origin_res(
            transport,
            "../origin_res",
            0,
            0,
            6,
        )?)
    };
    let mut clients = [make_client()?, make_client()?];
    let start = Instant::now();
    let mut pacer = StockRoleSendPacer::default();
    let mut anchor = None;
    let mut motion_start = None;
    let mut sent = 0;
    let mut received = 0;
    let mut gaps = Vec::new();
    let mut delays = Vec::new();
    let mut samples = Vec::new();
    let mut last_receive = None;
    let mut max_position_error = 0.0_f32;
    let mut max_velocity_error = 0.0_f32;
    let mut max_loop_gap = 0_u64;
    let mut last_loop = 0;
    while start.elapsed() < Duration::from_secs(seconds) {
        let now = start.elapsed().as_millis() as u64;
        max_loop_gap = max_loop_gap.max(now - last_loop);
        last_loop = now;
        let sender = clients[0].actor_id();
        for (index, client) in clients.iter_mut().enumerate() {
            for event in client.tick(now)? {
                match event {
                    StockGameEvent::RoleStates(records) => {
                        for record in records {
                            let s = record.state;
                            if record.actor_id == 1 {
                                anchor.get_or_insert(s.position);
                            }
                            let kind = if index == 1 && Some(u32::from(record.actor_id)) == sender {
                                "relay"
                            } else {
                                "host"
                            };
                            if index == 1 {
                                writeln!(
                                    trace,
                                    "{now},{kind},{},{},{},{},{},{},{},{},{}",
                                    record.actor_id,
                                    s.position.x,
                                    s.position.y,
                                    s.position.z,
                                    s.velocity.x,
                                    s.velocity.y,
                                    s.velocity.z,
                                    s.state_words.state_word0,
                                    s.state_words.state_word1
                                )?;
                            }
                            if kind == "relay" {
                                received += 1;
                                if let Some(last) = last_receive {
                                    gaps.push(now - last);
                                }
                                last_receive = Some(now);
                                if let Some((sent_at, expected)) = samples.iter().rev().find(
                                    |(_, old): &&(u64, meteor_network::role_state::RoleState)| {
                                        old.state_words.pose_sequence == s.state_words.pose_sequence
                                    },
                                ) {
                                    delays.push(now - sent_at);
                                    for (actual, expected) in [
                                        (s.position.x, expected.position.x),
                                        (s.position.y, expected.position.y),
                                        (s.position.z, expected.position.z),
                                    ] {
                                        max_position_error =
                                            max_position_error.max((actual - expected).abs());
                                    }
                                    for (actual, expected) in [
                                        (s.velocity.x, expected.velocity.x),
                                        (s.velocity.y, expected.velocity.y),
                                        (s.velocity.z, expected.velocity.z),
                                    ] {
                                        max_velocity_error =
                                            max_velocity_error.max((actual - expected).abs());
                                    }
                                }
                            }
                        }
                    }
                    StockGameEvent::Vitals(_) => {}
                    StockGameEvent::RawApplication { opcode, .. } => {
                        println!("{now} client={index} raw={opcode}")
                    }
                    event => println!("{now} client={index} {event:?}"),
                }
            }
        }
        if clients.iter().all(|client| client.actor_id().is_some()) {
            if let Some(anchor) = anchor {
                let begun = *motion_start.get_or_insert(now);
                let t = (now - begun) as f32 / 1000.0;
                let state = role_state_from_runtime(RoleStateRuntime {
                    position: RoleVec3 {
                        x: anchor.x + 5.0 + t.sin(),
                        y: anchor.y,
                        z: anchor.z + 5.0 + t.cos(),
                    },
                    forward: RoleVec3 {
                        x: t.cos(),
                        y: 0.0,
                        z: -t.sin(),
                    },
                    velocity: RoleVec3 {
                        x: t.cos(),
                        y: 0.0,
                        z: -t.sin(),
                    },
                    pose_id: 0,
                    grounded: false,
                    guard_route: false,
                    playback_arg: 1,
                    pose_sequence: sent + 1,
                });
                pacer.submit(state);
                if let Some(state) =
                    pacer.take_due(start.elapsed().as_micros() as u64, clients[0].role_rate())
                {
                    clients[0].send_role_state(state, now)?;
                    sent += 1;
                    samples.push((now, state));
                    writeln!(
                        trace,
                        "{now},send,{},{},{},{},{},{},{},{},{}",
                        sender.unwrap_or(0),
                        state.position.x,
                        state.position.y,
                        state.position.z,
                        state.velocity.x,
                        state.velocity.y,
                        state.velocity.z,
                        state.state_words.state_word0,
                        state.state_words.state_word1
                    )?;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    for client in &mut clients {
        client.transport_mut().disconnect(0)?;
    }
    gaps.sort_unstable();
    delays.sort_unstable();
    for (name, values) in [("receive_gap_ms", &gaps), ("relay_age_ms", &delays)] {
        if !values.is_empty() {
            println!(
                "{name}: n={} p50={} p95={} max={} over200={}",
                values.len(),
                values[values.len() / 2],
                values[values.len() * 95 / 100],
                values.last().unwrap(),
                values.iter().filter(|v| **v > 200).count()
            );
        }
    }
    println!("sent={sent} received={received} max_position_error={max_position_error} max_velocity_error={max_velocity_error} max_loop_gap_ms={max_loop_gap}");
    if received == 0 {
        return Err("no motion relayed".into());
    }
    Ok(())
}
