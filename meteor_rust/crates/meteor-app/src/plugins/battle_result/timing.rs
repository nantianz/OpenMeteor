use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::app_state::{AppState, LaunchRequest};
use crate::components::fighter::MatchEnd;
use crate::plugins::network_plugin::{NetState, NetStatus};

#[derive(Serialize, Deserialize)]
struct StoredTimes {
    version: u32,
    best_ms: BTreeMap<String, u64>,
}

#[derive(Resource, Default)]
pub(crate) struct BestTimes {
    best_ms: BTreeMap<String, u64>,
    persistent: bool,
    pub save_failed: bool,
}

impl BestTimes {
    pub fn load() -> Self {
        match read_storage() {
            Ok(None) => Self {
                persistent: true,
                ..default()
            },
            Ok(Some(text)) => match Self::decode(&text) {
                Ok(mut records) => {
                    records.persistent = true;
                    records
                }
                Err(error) => {
                    warn!("clear records left untouched: {error}");
                    Self {
                        save_failed: true,
                        ..default()
                    }
                }
            },
            Err(error) => {
                warn!("read clear records: {error}");
                Self {
                    save_failed: true,
                    ..default()
                }
            }
        }
    }

    fn decode(text: &str) -> Result<Self, String> {
        let stored: StoredTimes = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if stored.version != 1 {
            return Err("unsupported clear record version".into());
        }
        Ok(Self {
            best_ms: stored
                .best_ms
                .into_iter()
                .filter(|(id, ms)| !id.is_empty() && *ms > 0)
                .collect(),
            ..default()
        })
    }

    fn encode(&self) -> String {
        serde_json::to_string_pretty(&StoredTimes {
            version: 1,
            best_ms: self.best_ms.clone(),
        })
        .expect("integer clear times serialize")
    }

    pub fn best(&self, stage: &str) -> Option<u64> {
        self.best_ms.get(stage).copied()
    }

    fn record(&mut self, stage: &str, ms: u64) -> bool {
        if stage.is_empty() || ms == 0 || self.best(stage).is_some_and(|best| best <= ms) {
            return false;
        }
        self.best_ms.insert(stage.into(), ms);
        if self.persistent {
            self.save_failed = match write_storage(&self.encode()) {
                Ok(()) => false,
                Err(error) => {
                    warn!("save clear record: {error}");
                    true
                }
            };
        }
        true
    }
}

#[derive(Resource, Default)]
pub(crate) struct SettlementTime {
    pub elapsed_ms: Option<u64>,
    pub new_best: bool,
    captured: bool,
    round: Option<(Option<u32>, u32)>,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CaptureSettlementTime;

pub(super) fn reset(mut snapshot: ResMut<SettlementTime>) {
    *snapshot = default();
}

pub(super) fn capture(
    state: Res<State<AppState>>,
    request: Res<LaunchRequest>,
    end: Res<MatchEnd>,
    net: Res<NetState>,
    mut records: ResMut<BestTimes>,
    mut snapshot: ResMut<SettlementTime>,
) {
    if !matches!(
        state.get(),
        AppState::Playing | AppState::Paused | AppState::Result
    ) {
        return;
    }
    let round = net
        .battle_result
        .as_ref()
        .map(|r| (net.round_epoch, r.round));
    if (!end.settled && round.is_none()) || snapshot.round != round {
        *snapshot = SettlementTime { round, ..default() };
    }
    if snapshot.captured || (!end.settled && round.is_none()) {
        return;
    }
    snapshot.captured = true;

    snapshot.elapsed_ms = milliseconds(end.elapsed);
    if request.mode == "single"
        && !request.spawn_room_npcs
        && net.status != NetStatus::InRoom
        && net.battle_result.is_none()
        && end.result == 4
    {
        if let Some(ms) = snapshot.elapsed_ms {
            snapshot.new_best = records.record(&request.stage_id, ms);
        }
    }
}

pub(crate) fn milliseconds(seconds: f32) -> Option<u64> {
    (seconds.is_finite() && seconds >= 0.0).then(|| (f64::from(seconds) * 1000.0).round() as u64)
}

pub(crate) fn format_time(ms: Option<u64>) -> String {
    ms.map_or_else(
        || "—".into(),
        |ms| format!("{}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn storage_path() -> std::path::PathBuf {
    #[cfg(target_os = "android")]
    if let Some(path) = bevy::android::ANDROID_APP
        .get()
        .and_then(|app| app.internal_data_path())
    {
        return path.join("clear-times.json");
    }
    std::path::PathBuf::from("clear-times.json")
}

fn read_storage() -> Result<Option<String>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        match std::fs::read_to_string(storage_path()) {
            Ok(text) => Ok(Some(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let storage = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .ok_or("local storage unavailable")?;
        storage
            .get_item("meteor.rust.clear-times.v1")
            .map_err(|e| format!("{e:?}"))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn write_file(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temporary, text)?;
    std::fs::rename(temporary, path)
}

fn write_storage(text: &str) -> Result<(), String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        write_file(&storage_path(), text).map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let storage = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .ok_or("local storage unavailable")?;
        storage
            .set_item("meteor.rust.clear-times.v1", text)
            .map_err(|e| format!("{e:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.insert_resource(State::new(AppState::Playing))
            .insert_resource(LaunchRequest {
                mode: "single".into(),
                stage_id: "sn01".into(),
                ..default()
            })
            .init_resource::<MatchEnd>()
            .init_resource::<NetState>()
            .init_resource::<BestTimes>()
            .init_resource::<SettlementTime>()
            .add_systems(Update, capture);
        app
    }

    fn settle(app: &mut App, bits: i32, elapsed: f32) {
        *app.world_mut().resource_mut::<MatchEnd>() = MatchEnd::default();
        app.update();
        *app.world_mut().resource_mut::<MatchEnd>() = MatchEnd {
            result: bits,
            elapsed,
            settled: true,
            ..default()
        };
        app.update();
    }

    #[test]
    fn solo_records_only_faster_wins_per_stage_and_freezes_at_decision() {
        let mut app = app();
        settle(&mut app, 4, 125.125);
        assert_eq!(
            app.world().resource::<BestTimes>().best("sn01"),
            Some(125125)
        );
        assert!(app.world().resource::<SettlementTime>().new_best);

        app.world_mut().resource_mut::<MatchEnd>().elapsed = 135.125;
        app.insert_resource(State::new(AppState::Result));
        app.update();
        assert_eq!(
            app.world().resource::<SettlementTime>().elapsed_ms,
            Some(125125)
        );
        for (bits, seconds) in [(2, 20.0), (1, 10.0), (6, 5.0), (4, 130.0), (4, 125.125)] {
            settle(&mut app, bits, seconds);
            assert_eq!(
                app.world().resource::<BestTimes>().best("sn01"),
                Some(125125)
            );
            assert!(!app.world().resource::<SettlementTime>().new_best);
        }
        settle(&mut app, 4, 90.25);
        assert_eq!(
            app.world().resource::<BestTimes>().best("sn01"),
            Some(90250)
        );
        app.world_mut().resource_mut::<LaunchRequest>().stage_id = "sn03".into();
        settle(&mut app, 4, 100.0);
        assert_eq!(
            app.world().resource::<BestTimes>().best("sn03"),
            Some(100000)
        );
        assert_eq!(
            app.world().resource::<BestTimes>().best("sn01"),
            Some(90250)
        );
    }

    #[test]
    fn network_round_time_freezes_and_restarts_without_writing_solo_record() {
        let mut app = app();
        app.world_mut().resource_mut::<NetState>().status = NetStatus::InRoom;
        app.world_mut().resource_mut::<MatchEnd>().elapsed = 60.5;
        app.world_mut().resource_mut::<NetState>().battle_result =
            Some(super::super::NetworkBattleResult {
                round: 1,
                ..default()
            });
        app.update();
        app.world_mut().resource_mut::<MatchEnd>().elapsed = 70.5;
        app.world_mut()
            .resource_mut::<NetState>()
            .battle_result
            .as_mut()
            .unwrap()
            .elapsed = 10.0;
        app.update();
        assert_eq!(
            app.world().resource::<SettlementTime>().elapsed_ms,
            Some(60500)
        );
        app.world_mut().resource_mut::<NetState>().battle_result = None;
        app.update();
        app.world_mut().resource_mut::<MatchEnd>().elapsed = 20.0;
        app.world_mut().resource_mut::<NetState>().battle_result =
            Some(super::super::NetworkBattleResult {
                round: 2,
                ..default()
            });
        app.update();
        assert_eq!(
            app.world().resource::<SettlementTime>().elapsed_ms,
            Some(20000)
        );
        assert!(app.world().resource::<BestTimes>().best_ms.is_empty());

        app.world_mut().resource_mut::<MatchEnd>().settled = true;
        app.world_mut().resource_mut::<MatchEnd>().elapsed = 30.0;
        app.update();
        assert_eq!(
            app.world().resource::<SettlementTime>().elapsed_ms,
            Some(20000)
        );
    }

    #[test]
    fn non_campaign_invalid_and_zero_times_never_create_records() {
        let mut app = app();
        for mode in ["leader", "deathmatch", "replay"] {
            app.world_mut().resource_mut::<LaunchRequest>().mode = mode.into();
            settle(&mut app, 4, 1.0);
        }
        app.world_mut().resource_mut::<LaunchRequest>().mode = "single".into();
        for elapsed in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            settle(&mut app, 4, elapsed);
        }
        app.world_mut()
            .resource_mut::<LaunchRequest>()
            .spawn_room_npcs = true;
        settle(&mut app, 4, 1.0);
        assert!(app.world().resource::<BestTimes>().best_ms.is_empty());
        assert_eq!(format_time(Some(3_661_234)), "61:01.234");
        assert_eq!(format_time(Some(0)), "0:00.000");
        assert_eq!(format_time(None), "—");
    }

    #[test]
    fn records_round_trip_and_reject_damaged_or_future_formats() {
        let mut records = BestTimes::default();
        records.record("sn01", 123456);
        records.record("sn03", 54321);
        let restored = BestTimes::decode(&records.encode()).unwrap();
        assert_eq!(restored.best_ms, records.best_ms);
        assert!(BestTimes::decode("broken").is_err());
        assert!(BestTimes::decode(r#"{"version":2,"best_ms":{}}"#).is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn disk_record_can_be_replaced_and_reloaded() {
        let path = std::env::temp_dir().join(format!(
            "meteor-clear-record-test-{}.json",
            std::process::id()
        ));
        let mut records = BestTimes::default();
        records.record("sn01", 123000);
        write_file(&path, &records.encode()).unwrap();
        records.record("sn01", 99000);
        write_file(&path, &records.encode()).unwrap();
        let restored = BestTimes::decode(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(restored.best("sn01"), Some(99000));
        std::fs::remove_file(path).unwrap();
    }
}
