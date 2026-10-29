use bevy::prelude::*;
use std::sync::{Arc, Mutex};

pub const PRIMARY_WS: &str = match option_env!("METEOR_PRIMARY_WS") {
    Some(address) => address,
    None => "ws://127.0.0.1:9001",
};
pub const SECONDARY_WS: &str = match option_env!("METEOR_SECONDARY_WS") {
    Some(address) => address,
    None => "ws://localhost:9001",
};
const ENDPOINTS: [&str; 2] = [PRIMARY_WS, SECONDARY_WS];
const PROBE_TIMEOUT_MS: u64 = 3000;
const SELECTION_TIMEOUT: f64 = 3.25;

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Measurement {
    connect_ms: f64,
    rtt_ms: f64,
}
impl Measurement {
    fn valid(&self) -> bool {
        [self.connect_ms, self.rtt_ms]
            .into_iter()
            .all(|v| v.is_finite() && v >= 0.0 && v <= PROBE_TIMEOUT_MS as f64)
    }
}
type ProbeResults = Arc<Mutex<[Option<Result<Measurement, String>>; 2]>>;

#[derive(Resource)]
pub(crate) struct ServerRoute {
    selected: String,
    finished: bool,
    started_at: f64,
    results: ProbeResults,
}
impl ServerRoute {
    fn new(fallback: String) -> Self {
        Self {
            selected: fallback,
            finished: false,
            started_at: 0.0,
            results: Arc::new(Mutex::new([None, None])),
        }
    }
    pub fn pending(&self) -> bool {
        !self.finished
    }
    fn settle(&mut self, now: f64) {
        if self.finished {
            return;
        }
        let Ok(results) = self.results.lock() else {
            return;
        };
        if results.iter().any(Option::is_none) && now - self.started_at < SELECTION_TIMEOUT {
            return;
        }

        if let Some((index, measurement)) = results
            .iter()
            .enumerate()
            .filter_map(|(i, result)| {
                result
                    .as_ref()?
                    .as_ref()
                    .ok()
                    .filter(|m| m.valid())
                    .map(|m| (i, m))
            })
            .min_by(|(_, a), (_, b)| {
                a.rtt_ms
                    .total_cmp(&b.rtt_ms)
                    .then(a.connect_ms.total_cmp(&b.connect_ms))
            })
        {
            self.selected = ENDPOINTS[index].into();
            info!(
                "Public server probe selected {}: median RTT {:.1}ms, connect {:.1}ms",
                self.selected, measurement.rtt_ms, measurement.connect_ms
            );
        }
        self.finished = true;
    }
}
pub(crate) fn address(route: Option<&ServerRoute>) -> String {
    route
        .map(|r| r.selected.clone())
        .unwrap_or_else(super::network_plugin::default_web_address)
}
pub(crate) fn install(app: &mut App) {
    app.insert_resource(ServerRoute::new(
        super::network_plugin::default_web_address(),
    ))
    .add_systems(Startup, begin_probes)
    .add_systems(Update, poll.before(crate::plugins::UiTouchActionSet));
}
fn begin_probes(time: Res<Time<Real>>, mut route: ResMut<ServerRoute>) {
    route.started_at = time.elapsed_secs_f64();
    #[cfg(not(target_arch = "wasm32"))]
    let overridden = std::env::var("METEOR_WS").is_ok_and(|v| !v.trim().is_empty());
    #[cfg(target_arch = "wasm32")]
    let overridden = false;
    if overridden || route.selected.starts_with("ws://") {
        route.finished = true;
        return;
    }
    launch_probes(route.results.clone());
}

#[cfg(not(target_arch = "wasm32"))]
async fn probe_native(url: &str) -> Result<Measurement, String> {
    use futures_util::{SinkExt, StreamExt};
    use std::time::{Duration, Instant};
    use tokio_tungstenite::tungstenite::Message;
    tokio::time::timeout(Duration::from_millis(PROBE_TIMEOUT_MS), async {
        let started = Instant::now();
        let (mut socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| e.to_string())?;
        let connect_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut samples = Vec::with_capacity(3);
        for sequence in 1..=3 {
            let sent = Instant::now();
            socket
                .send(Message::Text(
                    serde_json::json!({"type":"ping","clientTime":sequence})
                        .to_string()
                        .into(),
                ))
                .await
                .map_err(|e| e.to_string())?;
            loop {
                let frame = socket
                    .next()
                    .await
                    .ok_or("probe closed")?
                    .map_err(|e| e.to_string())?;
                if let Message::Text(text) = frame {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                    if value["type"] == "pong"
                        && value["clientTime"].as_f64() == Some(sequence as f64)
                    {
                        samples.push(sent.elapsed().as_secs_f64() * 1000.0);
                        break;
                    }
                } else if frame.is_close() {
                    return Err("probe closed".into());
                }
            }
        }

        let _ = socket.close(None).await;
        samples.sort_by(f64::total_cmp);
        Ok(Measurement {
            connect_ms,
            rtt_ms: samples[1],
        })
    })
    .await
    .map_err(|_| "probe timed out".to_string())?
}

#[cfg(not(target_arch = "wasm32"))]
fn launch_probes(results: ProbeResults) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                if let Ok(mut slots) = results.lock() {
                    *slots = [Some(Err(error.to_string())), Some(Err(error.to_string()))];
                }
                return;
            }
        };
        runtime.block_on(async {
            let probe = |index: usize| {
                let results = results.clone();
                async move {
                    let result = probe_native(ENDPOINTS[index]).await;
                    if let Ok(mut slots) = results.lock() {
                        slots[index] = Some(result);
                    }
                }
            };
            tokio::join!(probe(0), probe(1));
        });
    });
}

#[cfg(target_arch = "wasm32")]
fn launch_probes(results: ProbeResults) {
    #[wasm_bindgen::prelude::wasm_bindgen(inline_js = "
export function meteorProbeServer(url, timeoutMs) {
    return new Promise((resolve, reject) => {
        const started = performance.now();
        let socket, finished = false, connectMs = 0, sent = 0, sequence = 0;
        const samples = [];
        const finish = (result, error) => {
            if (finished) return;
            finished = true;
            clearTimeout(timer);
            if (socket) {
                socket.onopen = socket.onmessage = socket.onclose = null;
                socket.onerror = () => {};
                try { socket.close(); } catch {}
            }
            if (error) reject(new Error(error));
            else resolve(JSON.stringify(result));
        };
        const timer = setTimeout(() => finish(null, 'probe timed out'), timeoutMs);
        const ping = () => {
            sent = performance.now();
            try { socket.send(JSON.stringify({type: 'ping', clientTime: ++sequence})); }
            catch { finish(null, 'probe send failed'); }
        };
        try { socket = new WebSocket(url); }
        catch { finish(null, 'probe connect failed'); return; }
        socket.onopen = () => { connectMs = performance.now() - started; ping(); };
        socket.onmessage = event => {
            let message;
            try { message = JSON.parse(event.data); } catch { return; }
            if (message.type !== 'pong' || message.clientTime !== sequence) return;
            samples.push(performance.now() - sent);
            if (samples.length === 3) {
                samples.sort((a, b) => a - b);
                finish({connectMs, rttMs: samples[1]});
            } else ping();
        };
        socket.onerror = () => finish(null, 'probe network error');
        socket.onclose = () => finish(null, 'probe closed');
    });
}")]
    extern "C" {
        #[wasm_bindgen::prelude::wasm_bindgen(js_name = meteorProbeServer)]
        fn probe_server(url: &str, timeout_ms: u32) -> js_sys::Promise;
    }
    for (index, endpoint) in ENDPOINTS.iter().enumerate() {
        let results = results.clone();
        let promise = probe_server(endpoint, PROBE_TIMEOUT_MS as u32);
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(value) => value
                    .as_string()
                    .ok_or_else(|| "invalid probe result".to_string())
                    .and_then(|body| {
                        serde_json::from_str::<Measurement>(&body).map_err(|e| e.to_string())
                    }),
                Err(_) => Err("server probe failed".into()),
            };
            if let Ok(mut slots) = results.lock() {
                slots[index] = Some(outcome);
            }
        });
    }
}
fn poll(time: Res<Time<Real>>, mut route: ResMut<ServerRoute>) {
    let pending = route.pending();
    route.settle(time.elapsed_secs_f64());
    if pending && !route.pending() {
        info!("Selected public multiplayer endpoint: {}", route.selected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ok(rtt: f64) -> Option<Result<Measurement, String>> {
        Some(Ok(Measurement {
            connect_ms: 50.0,
            rtt_ms: rtt,
        }))
    }
    fn route(results: [Option<Result<Measurement, String>>; 2]) -> ServerRoute {
        let route = ServerRoute::new("wss://fallback.example/ws".into());
        *route.results.lock().unwrap() = results;
        route
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_probe_uses_real_socket_three_pings_and_closes() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}/ws", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                for sequence in 1..=3 {
                    let frame = socket.next().await.unwrap().unwrap();
                    let message: serde_json::Value =
                        serde_json::from_str(frame.to_text().unwrap()).unwrap();
                    assert_eq!(message["type"], "ping");
                    assert_eq!(message["clientTime"], sequence);
                    socket
                        .send(Message::Text(
                            serde_json::json!({"type":"pong","clientTime":sequence})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap();
                }
                assert!(socket.next().await.unwrap().unwrap().is_close());
            });
            assert!(probe_native(&url).await.unwrap().valid());
            server.await.unwrap();
        });
    }
    #[test]
    fn connection_time_breaks_equal_rtt_ties() {
        let mut r = route([
            ok(40.0),
            Some(Ok(Measurement {
                connect_ms: 10.0,
                rtt_ms: 40.0,
            })),
        ]);
        r.settle(0.5);
        assert_eq!(r.selected, SECONDARY_WS);
    }
    #[test]
    fn chooses_fastest_rtt_not_first_connection() {
        let mut r = route([ok(180.0), ok(40.0)]);
        r.settle(0.5);
        assert_eq!(r.selected, SECONDARY_WS);
        let mut r = route([ok(30.0), ok(100.0)]);
        r.settle(0.5);
        assert_eq!(r.selected, PRIMARY_WS);
    }
    #[test]
    fn waits_for_both_then_uses_available_at_deadline() {
        let mut r = route([ok(30.0), None]);
        r.settle(1.0);
        assert!(r.pending());
        r.settle(SELECTION_TIMEOUT);
        assert_eq!(r.selected, PRIMARY_WS);
        assert!(!r.pending());
    }
    #[test]
    fn unavailable_or_invalid_measurements_fall_back() {
        for results in [
            [None, None],
            [Some(Err("failed".into())), ok(f64::NAN)],
            [ok(-1.0), ok(f64::INFINITY)],
        ] {
            let mut r = route(results);
            r.settle(SELECTION_TIMEOUT);
            assert_eq!(r.selected, "wss://fallback.example/ws");
        }
    }
    #[test]
    fn one_failure_does_not_discard_other_and_selection_never_changes() {
        let mut r = route([Some(Err("TLS failed".into())), ok(100.0)]);
        r.settle(0.5);
        assert_eq!(r.selected, SECONDARY_WS);
        *r.results.lock().unwrap() = [ok(1.0), ok(100.0)];
        r.settle(1.0);
        assert_eq!(r.selected, SECONDARY_WS);
    }
}
