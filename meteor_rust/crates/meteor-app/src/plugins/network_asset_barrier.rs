use super::*;
use crate::plugins::character_plugin::{CommonAmb, PreparedNetworkModels};

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub room_id: String,
    pub attempt_id: String,
    pub players: Vec<RoomMemberWire>,
}
#[derive(Clone, PartialEq, Eq)]
struct Key {
    session: String,
    room: String,
    attempt: String,
}
struct Active {
    key: Key,
    receiver: std::sync::Mutex<Receiver<Result<PreparedNetworkModels, String>>>,
}
#[derive(Resource, Default)]
struct Preparation {
    active: Option<Active>,
    completed: std::collections::VecDeque<Key>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Preparation>()
        .add_systems(Update, tick.after(net_poll));
}

#[allow(clippy::too_many_arguments)]
fn tick(
    mut commands: Commands,
    mut net: ResMut<NetState>,
    tx: Res<NetTx>,
    mut preparation: ResMut<Preparation>,
    mut characters: Option<ResMut<CharacterAssetsSet>>,
    #[cfg(target_os = "android")] mut packs: Option<
        ResMut<crate::android_assets::AndroidAssetLoader>,
    >,
    #[cfg(target_arch = "wasm32")] packs: Option<Res<crate::wasm_assets::WasmAssetLoader>>,
) {
    if net.is_stock_udp() || net.status == NetStatus::Disconnected {
        preparation.active = None;
        preparation.completed.clear();
        net.asset_prepare = None;
        return;
    }
    let Some(request) = net.asset_prepare.clone() else {
        preparation.active = None;
        return;
    };
    let key = Key {
        session: net.session_token.clone(),
        room: request.room_id.clone(),
        attempt: request.attempt_id.clone(),
    };
    if net.room_id != request.room_id {
        preparation.active = None;
        net.asset_prepare = None;
        return;
    }

    if !net.transport_connected {
        return;
    }
    if preparation.completed.contains(&key) {
        net.asset_prepare = None;
        return;
    }
    if preparation
        .active
        .as_ref()
        .is_none_or(|active| active.key != key)
    {
        let (sender, receiver) = mpsc::channel();
        preparation.active = Some(Active {
            key: key.clone(),
            receiver: std::sync::Mutex::new(receiver),
        });
        #[cfg(target_os = "android")]
        {
            let ids: Vec<_> = request.players.iter().map(|p| p.character_id).collect();
            let result = packs
                .as_deref_mut()
                .ok_or_else(|| "Android 资源加载器未初始化".to_owned())
                .and_then(|loader| crate::android_assets::load_network_models(&ids, loader));
            if let Err(error) = result {
                let _ = sender.send(Err(error));
                return;
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || {
            let result = crate::plugins::character_plugin::prepare_network_models(&request.players);
            let _ = sender.send(result);
        });
        #[cfg(target_arch = "wasm32")]
        {
            let packs = packs.map(|loader| loader.clone());
            wasm_bindgen_futures::spawn_local(async move {
                let ids: Vec<_> = request.players.iter().map(|p| p.character_id).collect();
                let result = match packs {
                    Some(loader) => crate::wasm_assets::load_network_models(&ids, loader).await,
                    None => Err("浏览器资源加载器未初始化".into()),
                }
                .and_then(|_| {
                    crate::plugins::character_plugin::prepare_network_models(&request.players)
                });
                let _ = sender.send(result);
            });
        }
        return;
    }
    let result = {
        let active = preparation.active.as_ref().unwrap();
        let receiver = active.receiver.lock().unwrap();
        match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("模型加载任务意外终止".into()),
        }
    };
    let ok = result.is_ok();
    match result {
        Ok(loaded) => {
            if let Some(characters) = characters.as_deref_mut() {
                characters.0.extend(loaded.characters);
            } else {
                commands.insert_resource(CharacterAssetsSet(loaded.characters));
            }
            commands.insert_resource(CommonAmb(loaded.common));
        }
        Err(error) => {
            warn!("Web asset barrier {} failed: {error}", key.attempt);
            let error = format!("人物/兵器资源加载失败：{error}");
            net.asset_error = Some(error.clone());
            net.error = Some(error);
        }
    }
    tx.send_control(Message::MatchAssetReady {
        attempt_id: key.attempt.clone(),
        ok,
    });
    preparation.completed.push_back(key);
    if preparation.completed.len() > 32 {
        preparation.completed.pop_front();
    }
    preparation.active = None;
    net.asset_prepare = None;
}

#[cfg(all(test, not(target_arch = "wasm32"), not(target_os = "android")))]
mod tests {
    use super::*;
    fn fixture() -> (
        App,
        Sender<Result<PreparedNetworkModels, String>>,
        Receiver<NetOutbound>,
        Request,
    ) {
        let request = Request {
            room_id: "room".into(),
            attempt_id: "asset-1".into(),
            players: vec![RoomMemberWire {
                player_id: "p1".into(),
                name: "player".into(),
                character_id: 0,
                weapon: 6,
                team: None,
                ready: true,
                connected: true,
            }],
        };
        let (sender, receiver) = mpsc::channel();
        let (outbound, messages) = mpsc::channel();
        let mut tx = NetTx::default();
        tx.begin_connect(outbound);
        tx.activate();
        let mut app = App::new();
        app.insert_resource(NetState {
            address: "ws://test/ws".into(),
            status: NetStatus::InRoom,
            transport_connected: true,
            room_id: request.room_id.clone(),
            player_id: "p1".into(),
            session_token: "session".into(),
            asset_prepare: Some(request.clone()),
            ..default()
        })
        .insert_resource(tx)
        .insert_resource(Preparation {
            active: Some(Active {
                key: Key {
                    session: "session".into(),
                    room: request.room_id.clone(),
                    attempt: request.attempt_id.clone(),
                },
                receiver: std::sync::Mutex::new(receiver),
            }),
            completed: default(),
        })
        .add_systems(Update, tick);
        (app, sender, messages, request)
    }
    #[test]
    #[ignore = "requires the local Web server via METEOR_TEST_WEB_URL"]
    fn asset_barrier_real_web_server_reaches_match_start() {
        let url = std::env::var("METEOR_TEST_WEB_URL").expect("local Web server URL");
        assert!(url.starts_with("ws://127.0.0.1:"));
        for transport in ["server", "p2p"] {
            let request = LaunchRequest {
                stage_id: "sn01".into(),
                mode: "leader".into(),
                ..default()
            };
            let mut net = NetState {
                address: url.clone(),
                status: NetStatus::Connecting,
                pending_web_join: Some(Message::RoomCreate {
                    mode: "leader".into(),
                    stage: "sn01".into(),
                    name: "Rust barrier test".into(),
                    character_id: 0,
                    weapon: 6,
                    room_name: "asset barrier integration".into(),
                    password: String::new(),
                    rules: meteor_network::protocol::RoomRulesWire {
                        transport: Some(transport.into()),
                        ..default()
                    },
                }),
                ..default()
            };
            let mut tx = NetTx::default();
            let mut rx = NetRx::default();
            let mut worker = NativeNetThread::default();
            begin_network_connection(&mut net, &mut tx, &mut rx, &mut worker, &request).unwrap();
            struct Guard(NativeNetThread, Sender<NetOutbound>);
            impl Drop for Guard {
                fn drop(&mut self) {
                    let _ = self.1.send(NetOutbound::Quit);
                    self.0.shutdown();
                }
            }
            let _guard = Guard(worker, tx.connecting.as_ref().unwrap().clone());
            let mut app = App::new();
            app.insert_resource(net)
                .insert_resource(tx)
                .insert_resource(rx)
                .insert_resource(request)
                .insert_resource(State::new(AppState::NetRoom))
                .init_resource::<NextState<AppState>>()
                .init_resource::<Time>()
                .init_resource::<NetInputSequence>()
                .init_resource::<PendingNetActions>()
                .init_resource::<MatchEnd>()
                .init_resource::<PendingStockAnger>()
                .init_resource::<Preparation>()
                .add_systems(Update, (net_poll, tick).chain());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            let mut configured = false;
            let mut start_sent = false;
            let mut reached_start = false;
            while std::time::Instant::now() < deadline {
                app.update();
                let net = app.world().resource::<NetState>();
                assert!(net.error.is_none(), "{transport}: {:?}", net.error);
                if !configured && net.status == NetStatus::InRoom {
                    app.world()
                        .resource::<NetTx>()
                        .send_control(Message::RoomSettings {
                            mode: "leader".into(),
                            stage_id: "sn01".into(),
                            rules: meteor_network::protocol::RoomRulesWire {
                                transport: Some(transport.into()),
                                ..default()
                            },
                        });
                    app.world()
                        .resource::<NetTx>()
                        .send_control(Message::RoomLatency {
                            room_id: String::new(),
                            player_id: String::new(),
                            latency: meteor_network::protocol::RoomLatencyWire {
                                server_rtt: Some(38.0),
                                host_rtt: None,
                                p2p_state: "waiting".into(),
                            },
                        });
                    configured = true;
                }
                let net = app.world().resource::<NetState>();
                if !start_sent
                    && net.status == NetStatus::InRoom
                    && net.lobby.configured == Some(true)
                    && net.room_latency.contains_key(&net.player_id)
                {
                    assert!(net.room_start_blocker().is_none());
                    app.world()
                        .resource::<NetTx>()
                        .send_control(Message::RoomStart);
                    start_sent = true;
                }
                if matches!(
                    app.world().resource::<NextState<AppState>>(),
                    NextState::Pending(AppState::Loading)
                ) {
                    reached_start = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(
                reached_start,
                "{transport}: server must pass the resource barrier and send match.start"
            );
            assert_eq!(app.world().resource::<Preparation>().completed.len(), 1);
            assert!(app
                .world()
                .resource::<CharacterAssetsSet>()
                .0
                .contains_key("p0"));
            assert!(app.world().resource::<NetState>().round_active);

            app.world_mut()
                .insert_resource(State::new(AppState::Playing));
            app.world()
                .resource::<NetTx>()
                .send_control(Message::RoomLatency {
                    room_id: String::new(),
                    player_id: String::new(),
                    latency: meteor_network::protocol::RoomLatencyWire {
                        server_rtt: Some(55.0),
                        host_rtt: Some(12.0),
                        p2p_state: "direct".into(),
                    },
                });
            let mut latency_received = false;
            while std::time::Instant::now() < deadline {
                app.update();
                let net = app.world().resource::<NetState>();
                assert!(net.error.is_none(), "{transport}: {:?}", net.error);
                if net
                    .room_latency
                    .get(&net.player_id)
                    .is_some_and(|latency| latency.server_rtt == Some(55.0))
                {
                    latency_received = true;
                    assert!(net.round_active);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(
                latency_received,
                "running room latency broadcast must decode and update"
            );
        }
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn asset_barrier_acknowledges_only_after_models_complete_and_deduplicates() {
        let (mut app, worker, outbound, request) = fixture();
        app.update();
        assert!(outbound.try_recv().is_err());
        let loaded =
            crate::plugins::character_plugin::prepare_network_models(&request.players).unwrap();
        assert!(worker.send(Ok(loaded)).is_ok());
        app.update();
        assert!(
            matches!(outbound.try_recv(), Ok(NetOutbound::Control(Message::MatchAssetReady { attempt_id, ok: true })) if attempt_id == "asset-1")
        );
        assert!(app
            .world()
            .resource::<CharacterAssetsSet>()
            .0
            .contains_key("p0"));
        assert_eq!(app.world().resource::<NetState>().status, NetStatus::InRoom);
        assert!(!app.world().resource::<NetState>().round_active);
        app.world_mut().resource_mut::<NetState>().asset_prepare = Some(request);
        app.update();
        assert!(outbound.try_recv().is_err());
    }
    #[test]
    fn asset_barrier_reports_failure_without_starting_a_game() {
        let (mut app, worker, outbound, _) = fixture();
        assert!(worker.send(Err("missing model".into())).is_ok());
        app.update();
        assert!(matches!(
            outbound.try_recv(),
            Ok(NetOutbound::Control(Message::MatchAssetReady {
                ok: false,
                ..
            }))
        ));
        assert!(app
            .world()
            .resource::<NetState>()
            .error
            .as_deref()
            .unwrap()
            .contains("missing model"));
        assert!(!app.world().resource::<NetState>().round_active);
    }
    #[test]
    fn asset_barrier_drops_stale_cancelled_and_udp_completions() {
        for scenario in 0..3 {
            let (mut app, worker, outbound, _) = fixture();
            {
                let mut net = app.world_mut().resource_mut::<NetState>();
                match scenario {
                    0 => net.room_id = "another-room".into(),
                    1 => net.asset_prepare = None,
                    _ => net.address = "udp://127.0.0.1:5230".into(),
                }
            }
            assert!(worker.send(Err("stale".into())).is_ok());
            app.update();
            assert!(outbound.try_recv().is_err());
        }
    }
    #[test]
    fn asset_barrier_waits_for_reconnection_before_acknowledging() {
        let (mut app, worker, outbound, _) = fixture();
        app.world_mut()
            .resource_mut::<NetState>()
            .transport_connected = false;
        assert!(worker.send(Err("failed while offline".into())).is_ok());
        app.update();
        assert!(outbound.try_recv().is_err());
        app.world_mut()
            .resource_mut::<NetState>()
            .transport_connected = true;
        app.update();
        assert!(matches!(
            outbound.try_recv(),
            Ok(NetOutbound::Control(Message::MatchAssetReady {
                ok: false,
                ..
            }))
        ));
    }
}
