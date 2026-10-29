use super::*;
use bevy::ecs::system::SystemParam;

#[derive(Debug, Default, PartialEq, Eq)]
enum ConnectionPhase {
    #[default]
    Idle,
    Connecting,
    Cancelling,
    Failed(String),
}

#[derive(Resource, Default)]
pub(crate) struct StockConnectionDialog {
    phase: ConnectionPhase,
    address: String,
    selection: Option<LaunchRequest>,
    started_at: Option<Instant>,
}

impl StockConnectionDialog {
    pub(crate) fn is_open(&self) -> bool {
        self.phase != ConnectionPhase::Idle
    }

    pub(super) fn fail(&mut self, error: String) {
        self.phase = ConnectionPhase::Failed(error);
    }
}

#[derive(Component)]
pub(crate) struct StockConnectionDialogRoot;

#[derive(Component)]
pub(crate) enum StockConnectionText {
    Title,
    Detail,
    Cancel,
}

#[derive(Component, Clone, Copy)]
pub(crate) enum StockConnectionAction {
    Cancel,
    Retry,
}

pub(super) fn start(
    dialog: &mut StockConnectionDialog,
    net: &mut NetState,
    tx: &mut NetTx,
    rx: &mut NetRx,
    worker: &mut NativeNetThread,
    request: &LaunchRequest,
) {
    dialog.phase = ConnectionPhase::Connecting;
    dialog.address.clone_from(&net.address);
    dialog.selection = Some(request.clone());
    dialog.started_at = Some(Instant::now());
    net.error = None;
    net.status = NetStatus::Connecting;
    net.reconnect_attempt = 0;
    net.reconnect_at = None;
    if let Err(error) = begin_network_connection(net, tx, rx, worker, request) {
        net.status = NetStatus::Disconnected;
        dialog.fail(error);
    }
}

#[derive(SystemParam)]
pub(super) struct ConnectionSession<'w> {
    net: ResMut<'w, NetState>,
    tx: ResMut<'w, NetTx>,
    rx: ResMut<'w, NetRx>,
    worker: ResMut<'w, NativeNetThread>,
    sequence: ResMut<'w, NetInputSequence>,
    pending: ResMut<'w, PendingNetActions>,
    chat: ResMut<'w, NetChatInput>,
    anger: ResMut<'w, PendingStockAnger>,
    next_state: ResMut<'w, NextState<AppState>>,
}

impl ConnectionSession<'_> {
    fn stop(&mut self) {
        self.rx.0 = None;
        self.tx.disconnect();
        self.net.error = None;
        reset_network_session(
            &mut self.net,
            &mut self.tx,
            &mut self.rx,
            &mut self.sequence,
            &mut self.pending,
            &mut self.chat,
        );
        self.net.intentional_disconnect = true;
        *self.anger = PendingStockAnger::default();
        *self.next_state = NextState::Unchanged;
    }
}

pub(super) fn input(
    mut dialog: ResMut<StockConnectionDialog>,
    mut session: ConnectionSession,
    mut request: ResMut<LaunchRequest>,
    mut preparation: ResMut<PendingStockConnection>,
    keys: Res<ButtonInput<KeyCode>>,
    actions: Query<(&Interaction, &StockConnectionAction), Changed<Interaction>>,
    mut background: Query<&mut Interaction, Without<StockConnectionAction>>,
) {
    if !dialog.is_open() {
        return;
    }
    for mut interaction in &mut background {
        if *interaction != Interaction::None {
            *interaction = Interaction::None;
        }
    }
    let cancel = keys.just_pressed(KeyCode::Escape)
        || actions.iter().any(|(interaction, action)| {
            *interaction == Interaction::Pressed && matches!(action, StockConnectionAction::Cancel)
        });
    if cancel && dialog.phase != ConnectionPhase::Cancelling {
        session.stop();
        dialog.phase = ConnectionPhase::Cancelling;
        return;
    }
    if matches!(dialog.phase, ConnectionPhase::Failed(_))
        && session.worker.handle.is_none()
        && actions.iter().any(|(interaction, action)| {
            *interaction == Interaction::Pressed && matches!(action, StockConnectionAction::Retry)
        })
    {
        let Some(selection) = dialog.selection.clone() else {
            return;
        };
        if matches!(&dialog.phase, ConnectionPhase::Failed(error) if error.contains("密码错误"))
        {
            session.stop();
            let mut selection = selection;
            selection.stock_password.clear();
            *request = selection.clone();
            preparation.0 = Some((dialog.address.clone(), selection));
            *dialog = StockConnectionDialog::default();
            return;
        }
        *request = selection;
        session.net.address.clone_from(&dialog.address);
        start(
            &mut dialog,
            &mut session.net,
            &mut session.tx,
            &mut session.rx,
            &mut session.worker,
            &request,
        );
    }
}

#[allow(clippy::type_complexity)]
pub(super) fn refresh(
    mut dialog: ResMut<StockConnectionDialog>,
    mut session: ConnectionSession,
    mut roots: Query<
        &mut Node,
        (
            With<StockConnectionDialogRoot>,
            Without<StockConnectionAction>,
        ),
    >,
    mut actions: Query<(&StockConnectionAction, &mut Node)>,
    mut texts: Query<(&StockConnectionText, &mut Text)>,
) {
    session.worker.reap_finished();
    if dialog.phase == ConnectionPhase::Connecting {
        if let Some(error) = session.net.error.clone() {
            session.stop();
            dialog.fail(error);
        } else if session.net.status != NetStatus::InRoom
            && dialog
                .started_at
                .is_some_and(|time| time.elapsed().as_secs() >= 30)
        {
            session.stop();
            dialog.fail("连接超时，请检查房主是否在线及双方网络后重试。".into());
        } else if session.net.status == NetStatus::Disconnected {
            session.stop();
            dialog.fail("连接已断开，请重试。".into());
        }
    }
    if dialog.phase == ConnectionPhase::Cancelling && session.worker.handle.is_none() {
        *dialog = StockConnectionDialog::default();
    }
    for mut node in &mut roots {
        node.display = if dialog.is_open() {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (action, mut node) in &mut actions {
        node.display = match action {
            StockConnectionAction::Retry
                if matches!(dialog.phase, ConnectionPhase::Failed(_))
                    && dialog.selection.is_some()
                    && session.worker.handle.is_none() =>
            {
                Display::Flex
            }
            StockConnectionAction::Cancel if dialog.phase != ConnectionPhase::Cancelling => {
                Display::Flex
            }
            _ => Display::None,
        };
    }
    let (title, detail) = match &dialog.phase {
        ConnectionPhase::Idle => ("", String::new()),
        ConnectionPhase::Connecting => {
            let status = if session.net.status == NetStatus::InRoom {
                "连接成功，正在进入游戏…"
            } else if session.net.transport_connected {
                "已连接，正在等待入场确认…"
            } else if dialog.address.starts_with("udp-host://") {
                "正在创建局域网房间…"
            } else {
                "正在联系房主…"
            };
            let endpoint = dialog
                .address
                .strip_prefix("udp-host://")
                .or_else(|| dialog.address.strip_prefix("udp://"))
                .unwrap_or(&dialog.address);
            let detail = if dialog.address.starts_with("udp-host://") {
                status.into()
            } else {
                format!("{status}\n{endpoint}")
            };
            ("局域网连接", detail)
        }
        ConnectionPhase::Cancelling => ("正在取消连接", "正在关闭连接，请稍候…".into()),
        ConnectionPhase::Failed(error) => ("连接失败", error.clone()),
    };
    for (kind, mut text) in &mut texts {
        text.0 = match kind {
            StockConnectionText::Title => title.into(),
            StockConnectionText::Detail => detail.clone(),
            StockConnectionText::Cancel if matches!(dialog.phase, ConnectionPhase::Failed(_)) => {
                "返回".into()
            }
            StockConnectionText::Cancel => "取消连接".into(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::state::app::AppExtStates;
    use meteor_network::stock_protocol::StockRoleEnteredEvent;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<AppState>()
            .init_resource::<NetState>()
            .init_resource::<NetTx>()
            .init_resource::<NetRx>()
            .init_resource::<NativeNetThread>()
            .init_resource::<NetInputSequence>()
            .init_resource::<PendingNetActions>()
            .init_resource::<PendingStockAnger>()
            .init_resource::<NetChatInput>()
            .init_resource::<LaunchRequest>()
            .init_resource::<MatchEnd>()
            .init_resource::<Time>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<StockConnectionDialog>()
            .init_resource::<PendingStockConnection>()
            .add_systems(Update, (input, net_poll, refresh).chain());
        app.world_mut().spawn((
            StockConnectionDialogRoot,
            Node {
                display: Display::None,
                ..default()
            },
        ));
        for kind in [
            StockConnectionText::Title,
            StockConnectionText::Detail,
            StockConnectionText::Cancel,
        ] {
            app.world_mut().spawn((kind, Text::default()));
        }
        app
    }

    fn pending_attempt(app: &mut App) -> (Sender<NetInbound>, Receiver<NetOutbound>) {
        let (out, commands) = mpsc::channel();
        let (events, inbound) = mpsc::channel();
        app.world_mut().resource_mut::<NetTx>().begin_connect(out);
        app.world_mut().resource_mut::<NetRx>().0 = Some(std::sync::Mutex::new(inbound));
        let mut net = app.world_mut().resource_mut::<NetState>();
        net.address = "udp://127.0.0.1:5230".into();
        net.status = NetStatus::Connecting;
        *app.world_mut().resource_mut::<StockConnectionDialog>() = StockConnectionDialog {
            phase: ConnectionPhase::Connecting,
            address: "udp://127.0.0.1:5230".into(),
            selection: Some(LaunchRequest {
                stock_rule: 1,
                stock_camp: 0,
                stage_id: "sn04".into(),
                ..default()
            }),
            started_at: Some(Instant::now()),
        };
        (events, commands)
    }

    fn enqueue_entry(events: &Sender<NetInbound>) {
        events
            .send(NetInbound::StockGame(StockGameEvent::TransportConnected {
                peer: "127.0.0.1:5230".parse().unwrap(),
                transport_id: 7,
            }))
            .unwrap();
        events
            .send(NetInbound::StockGame(StockGameEvent::RoleEntered(
                StockRoleEnteredEvent {
                    actor_id: 7,
                    hero: 0,
                    hero_checksum: 0,
                    camp: 0,
                },
            )))
            .unwrap();
    }

    #[test]
    fn lost_stock_host_exits_every_battle_screen_without_reconnecting() {
        for screen in [
            AppState::NetRoom,
            AppState::Loading,
            AppState::Playing,
            AppState::Paused,
            AppState::Settings,
            AppState::Result,
        ] {
            for worker_dropped in [false, true] {
                let mut app = test_app();
                let (events, _commands) = pending_attempt(&mut app);
                app.insert_resource(State::new(screen));
                *app.world_mut().resource_mut::<StockConnectionDialog>() =
                    StockConnectionDialog::default();
                {
                    let mut net = app.world_mut().resource_mut::<NetState>();
                    net.status = NetStatus::InRoom;
                    net.reconnect_at = Some(0.0);
                    net.session_token = "stale".into();
                }
                if !worker_dropped {
                    events
                        .send(NetInbound::Closed {
                            code: 1006,
                            reason: "原版 UDP 房主已断开".into(),
                        })
                        .unwrap();
                }
                drop(events);
                app.update();
                let net = app.world().resource::<NetState>();
                assert_eq!(net.status, NetStatus::Disconnected);
                assert!(net.reconnect_at.is_none());
                assert!(net.session_token.is_empty());
                assert!(
                    matches!(
                        app.world().resource::<NextState<AppState>>(),
                        NextState::Pending(AppState::Menu)
                    ),
                    "{screen:?}"
                );
                assert!(app.world().resource::<NetRx>().0.is_none());
            }
        }
    }

    #[test]
    fn password_retry_returns_to_preparation_and_clears_old_secret() {
        let mut app = test_app();
        let _channels = pending_attempt(&mut app);
        {
            let mut dialog = app.world_mut().resource_mut::<StockConnectionDialog>();
            dialog.phase = ConnectionPhase::Failed("房间密码错误，请重试并重新输入密码".into());
            dialog.selection.as_mut().unwrap().stock_password = "wrong".into();
        }
        app.world_mut()
            .spawn((StockConnectionAction::Retry, Interaction::Pressed));
        app.update();
        let pending = app.world().resource::<PendingStockConnection>();
        let (address, selection) = pending.0.as_ref().unwrap();
        assert_eq!(address, "udp://127.0.0.1:5230");
        assert!(selection.stock_password.is_empty());
        assert!(app
            .world()
            .resource::<LaunchRequest>()
            .stock_password
            .is_empty());
        assert!(!app.world().resource::<StockConnectionDialog>().is_open());
    }

    #[test]
    fn stock_dialog_cancel_discards_queued_success_and_blocks_background_actions() {
        let mut app = test_app();
        let (events, commands) = pending_attempt(&mut app);
        enqueue_entry(&events);
        let background = app
            .world_mut()
            .spawn((MenuNetButton, Interaction::Pressed))
            .id();
        app.world_mut()
            .spawn((StockConnectionAction::Cancel, Interaction::Pressed));
        app.update();
        assert!(matches!(commands.try_recv(), Ok(NetOutbound::Quit)));
        assert!(app.world().resource::<NetRx>().0.is_none());
        assert_eq!(
            app.world().get::<Interaction>(background),
            Some(&Interaction::None)
        );
        let net = app.world().resource::<NetState>();
        assert_eq!(net.status, NetStatus::Disconnected);
        assert!(net.room_id.is_empty() && net.reconnect_at.is_none());
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Unchanged
        ));
        assert!(!app.world().resource::<StockConnectionDialog>().is_open());
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Menu
        );
    }

    #[test]
    fn stock_dialog_keeps_failure_in_modal_without_automatic_reconnect() {
        let mut app = test_app();
        let (events, _commands) = pending_attempt(&mut app);
        events
            .send(NetInbound::Control(Message::Error {
                code: "connect".into(),
                text: "房间已关闭".into(),
            }))
            .unwrap();
        drop(events);
        app.update();
        assert_eq!(
            app.world().resource::<StockConnectionDialog>().phase,
            ConnectionPhase::Failed("房间已关闭".into())
        );
        assert!(app.world().resource::<NetState>().reconnect_at.is_none());
        assert!(app.world().resource::<NetState>().error.is_none());
        let root = app
            .world_mut()
            .query_filtered::<&Node, With<StockConnectionDialogRoot>>()
            .single(app.world())
            .unwrap();
        assert_eq!(root.display, Display::Flex);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(!app.world().resource::<StockConnectionDialog>().is_open());
    }

    #[test]
    fn stock_dialog_shows_handshake_progress_then_enters_only_after_local_role() {
        let mut app = test_app();
        let (events, _commands) = pending_attempt(&mut app);
        events
            .send(NetInbound::StockGame(StockGameEvent::TransportConnected {
                peer: "127.0.0.1:5230".parse().unwrap(),
                transport_id: 7,
            }))
            .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<NetState>().status,
            NetStatus::Connecting
        );
        let detail = app
            .world_mut()
            .query::<(&StockConnectionText, &Text)>()
            .iter(app.world())
            .find(|(kind, _)| matches!(kind, StockConnectionText::Detail))
            .unwrap()
            .1;
        assert!(detail.0.contains("等待入场确认"));
        enqueue_entry(&events);
        app.update();
        assert_eq!(app.world().resource::<NetState>().status, NetStatus::InRoom);
        assert!(app.world().resource::<StockConnectionDialog>().is_open());
        assert!(matches!(
            app.world().resource::<NextState<AppState>>(),
            NextState::Pending(AppState::Loading)
        ));
    }

    #[test]
    fn stock_dialog_timeout_disconnects_instead_of_waiting_forever() {
        let mut app = test_app();
        let (_events, commands) = pending_attempt(&mut app);
        app.world_mut()
            .resource_mut::<StockConnectionDialog>()
            .started_at = Some(Instant::now() - std::time::Duration::from_secs(31));
        app.update();
        assert!(
            matches!(&app.world().resource::<StockConnectionDialog>().phase, ConnectionPhase::Failed(error) if error.contains("超时"))
        );
        assert!(matches!(commands.try_recv(), Ok(NetOutbound::Quit)));
        assert!(app.world().resource::<NetRx>().0.is_none());
    }

    #[test]
    fn stock_dialog_cancel_waits_for_port_release_and_retry_reuses_selection() {
        let mut app = test_app();
        let (_events, commands) = pending_attempt(&mut app);
        let listener = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let (release, wait) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            assert!(matches!(
                commands.recv_timeout(std::time::Duration::from_secs(3)),
                Ok(NetOutbound::Quit)
            ));
            let _ = wait.recv_timeout(std::time::Duration::from_secs(3));
            drop(listener);
        });
        app.world_mut()
            .resource_mut::<NativeNetThread>()
            .install("udp-host://127.0.0.1:0", worker);
        let cancel = app
            .world_mut()
            .spawn((StockConnectionAction::Cancel, Interaction::Pressed))
            .id();
        app.update();
        assert_eq!(
            app.world().resource::<StockConnectionDialog>().phase,
            ConnectionPhase::Cancelling
        );
        assert!(std::net::UdpSocket::bind(endpoint).is_err());
        release.send(()).unwrap();
        let handle = app
            .world_mut()
            .resource_mut::<NativeNetThread>()
            .handle
            .take()
            .unwrap();
        handle.join().unwrap();
        app.update();
        assert!(!app.world().resource::<StockConnectionDialog>().is_open());
        assert!(std::net::UdpSocket::bind(endpoint).is_ok());
        *app.world_mut().get_mut::<Interaction>(cancel).unwrap() = Interaction::None;

        let (_events, _commands) = pending_attempt(&mut app);
        {
            let mut dialog = app.world_mut().resource_mut::<StockConnectionDialog>();
            dialog.address = "udp-host://127.0.0.1:0".into();
            dialog.selection.as_mut().unwrap().weapon = 8;
            dialog.fail("模拟首次连接失败".into());
        }
        app.world_mut().resource_mut::<LaunchRequest>().weapon = 1;
        app.world_mut()
            .spawn((StockConnectionAction::Retry, Interaction::Pressed));
        app.update();
        assert_eq!(app.world().resource::<LaunchRequest>().weapon, 8);
        assert_eq!(
            app.world().resource::<StockConnectionDialog>().phase,
            ConnectionPhase::Connecting
        );
        assert!(app.world().resource::<NativeNetThread>().handle.is_some());
        app.world().resource::<NetTx>().disconnect();
        app.world_mut().resource_mut::<NativeNetThread>().shutdown();
        assert!(app.world().resource::<NativeNetThread>().handle.is_none());
    }
}
