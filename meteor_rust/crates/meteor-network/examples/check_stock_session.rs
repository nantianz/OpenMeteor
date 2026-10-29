use meteor_network::stock_game::{StockGameClient, StockGameEvent};
use meteor_network::stock_session::{StockClientState, StockClientTransport};
use meteor_network::stock_udp::{stock_integrity_checksum, StockConnectRecord};
use std::io::Write;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:5230".into());
    let seconds: u64 = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "45".into())
        .parse()?;
    let mut trace = std::env::args()
        .nth(3)
        .map(std::fs::File::create)
        .transpose()?;
    if let Some(trace) = trace.as_mut() {
        writeln!(trace, "time_ms,actor,x,y,z,vx,vy,vz,word0,word1")?;
    }
    let checksum = stock_integrity_checksum(|path| {
        std::fs::read(format!("../origin_res/{}", path.replace('\\', "/"))).ok()
    });
    let transport =
        StockClientTransport::bind("0.0.0.0:0", &target, StockConnectRecord::new("", checksum))?;
    let mut client = StockGameClient::from_origin_res(transport, "../origin_res", 0, 0, 6)?;
    let started = Instant::now();
    let mut vitals = 0;
    let mut states = 0;
    let mut last_report = 0;
    while started.elapsed() < Duration::from_secs(seconds) {
        let now = started.elapsed().as_millis() as u64;
        for event in client.tick(now)? {
            match &event {
                StockGameEvent::Vitals(_) => vitals += 1,
                StockGameEvent::RoleStates(records) => {
                    states += records.len();
                    if let Some(trace) = trace.as_mut() {
                        for record in records {
                            let state = record.state;
                            writeln!(
                                trace,
                                "{now},{},{},{},{},{},{},{},{},{}",
                                record.actor_id,
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
                    if states <= 8 {
                        println!("[{now} ms] {event:?}");
                    }
                }
                StockGameEvent::RawApplication {
                    opcode, payload, ..
                } => {
                    println!(
                        "[{now} ms] raw opcode={opcode} len={} hex={}",
                        payload.len(),
                        payload
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    );
                }
                _ => println!("[{now} ms] {event:?}"),
            }
        }
        if client.transport().state() == StockClientState::Disconnected {
            return Err("production client disconnected".into());
        }
        if now / 5000 > last_report {
            last_report = now / 5000;
            println!(
                "[{now} ms] state={:?} actor={:?} vitals={vitals} states={states} rtt={}",
                client.transport().state(),
                client.actor_id(),
                client.transport().round_trip_ms()
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let entered = client.actor_id().is_some();
    client.transport_mut().disconnect(0)?;
    if !entered || vitals == 0 {
        return Err("no entered role/vitals".into());
    }
    println!(
        "PASS {seconds}s actor={:?} vitals={vitals} states={states}",
        client.actor_id()
    );
    Ok(())
}
