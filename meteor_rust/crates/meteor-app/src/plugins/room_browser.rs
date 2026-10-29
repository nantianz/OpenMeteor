use super::*;
use crate::app_state::{MenuScreen, MenuStockDiscoveryText, MenuStockRefreshButton};
use crate::plugins::launcher_ui::{spawn_network_room_button, LauncherNetworkRoomList, UiFont};

#[derive(Component, Clone)]
pub(crate) enum RoomTarget {
    #[cfg(not(target_arch = "wasm32"))]
    Lan(StockDiscoveredRoom),
    Web(RoomSummary),
}
impl RoomTarget {
    pub(crate) fn presentation(&self) -> (&'static str, String, String, String) {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Lan(room) => {
                let mode = room.info.rule().and_then(|rule| {
                    meteor_network::modes::MODE_RULES
                        .iter()
                        .find(|(_, value)| *value == rule)
                        .map(|(id, _)| *id)
                });
                (
                    "局域网",
                    room.info.server_name.clone(),
                    format!(
                        "{}\n{} · {} · 版本 {}.{:02}",
                        room_metadata(
                            mode,
                            room.info.resolved_stage_id().as_deref(),
                            Some([room.info.field_0e as usize, room.info.field_0f as usize])
                        ),
                        room.endpoint,
                        if room.info.fields_0_to_6[6] != 0 {
                            "有密码"
                        } else {
                            "无密码"
                        },
                        room.info.fields_0_to_6[0],
                        room.info.fields_0_to_6[1]
                    ),
                    room.info
                        .max_players()
                        .map(|max| format!("{} / {max}", room.info.fields_0_to_6[3]))
                        .unwrap_or_else(|| format!("{} 人", room.info.fields_0_to_6[3])),
                )
            }
            Self::Web(room) => {
                let detail = format!(
                    "{}\n{} · {}",
                    room_metadata(
                        room.mode.as_deref(),
                        room.stage_id.as_deref(),
                        room.team_counts
                    ),
                    room.owner_name.as_deref().unwrap_or("房主未知"),
                    web_room_unavailable(room).unwrap_or(if room.state == "running" {
                        "对战中 · 可加入"
                    } else {
                        "准备中 · 可加入"
                    })
                );
                (
                    "Web",
                    room.name.clone(),
                    detail,
                    format!("{} / {}", room.player_count, room.max_players),
                )
            }
        }
    }

    fn key(&self) -> String {
        match self {
            #[cfg(not(target_arch = "wasm32"))]
            Self::Lan(room) => format!("lan:{}", room.endpoint),
            Self::Web(room) => format!("web:{}", room.room_id),
        }
    }
    fn label(&self) -> String {
        let (source, title, detail, count) = self.presentation();
        format!("[{source}] {title} · {count}\n{detail}")
    }
}

fn room_metadata(mode: Option<&str>, stage: Option<&str>, teams: Option<[usize; 2]>) -> String {
    let stage = stage
        .filter(|id| !id.is_empty())
        .map(|id| {
            meteor_scene::stage_registry::official_stages()
                .into_iter()
                .find(|stage| stage.id.eq_ignore_ascii_case(id))
                .map(|stage| stage.name)
                .unwrap_or_else(|| id.to_owned())
        })
        .unwrap_or_else(|| "地图未提供".into());
    let mode_label = mode
        .map(meteor_network::modes::mode_label)
        .unwrap_or("模式未提供");
    let teams = if mode.is_some_and(meteor_network::modes::is_team_mode) {
        teams
            .map(|[a, b]| format!(" · 流星 {a} 人 / 蝴蝶 {b} 人"))
            .unwrap_or_else(|| " · 阵营人数未提供".into())
    } else {
        String::new()
    };
    format!("{mode_label} · {stage}{teams}")
}
pub(super) fn web_room_unavailable(room: &RoomSummary) -> Option<&'static str> {
    if room.has_password {
        Some("需要密码（暂不支持加入）")
    } else if room.player_count >= room.max_players {
        Some("已满员")
    } else if !matches!(room.state.as_str(), "lobby" | "running") {
        Some("结算中 / 暂不可加入")
    } else {
        None
    }
}

#[derive(Resource, Default)]
pub(super) struct RoomBrowser {
    net: NetState,
    tx: NetTx,
    rx: NetRx,
    #[cfg(not(target_arch = "wasm32"))]
    native: NativeNetThread,
    pub(super) rooms: Vec<RoomSummary>,
    active: bool,
    last_attempt: Option<f64>,
    last_request: Option<f64>,
    updated_at: Option<f64>,
    waiting_since: Option<f64>,
    rendered: Vec<(String, String)>,
}
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct BrowserTransport(WasmNetTransport);
impl RoomBrowser {
    fn stop(&mut self) {
        self.tx.disconnect();
        self.tx.clear();
        self.rx.0 = None;
        #[cfg(not(target_arch = "wasm32"))]
        self.native.shutdown();
        self.net.transport_connected = false;
        self.net.status = NetStatus::Disconnected;
        self.rooms.clear();
        self.waiting_since = None;
    }
    fn fail(&mut self, reason: String) {
        self.stop();
        self.net.error = Some(reason);
    }
    fn request_rooms(&mut self, now: f64) {
        self.tx.send_control(Message::RoomList { rooms: None });
        self.last_request = Some(now);
        self.waiting_since.get_or_insert(now);
    }
    fn message(&mut self, message: Message, now: f64) {
        match message {
            Message::Welcome { .. } => {
                self.tx.activate();
                self.net.transport_connected = true;
                self.net.error = None;
                self.request_rooms(now);
            }
            Message::RoomList { rooms: Some(rooms) } => {
                self.rooms = rooms;
                self.updated_at = Some(now);
                self.waiting_since = None;
                self.net.error = None;
            }
            Message::Error { text, .. } => self.fail(text),
            _ => {}
        }
    }
}
impl Drop for RoomBrowser {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<RoomBrowser>()
        .add_systems(Update, poll.after(crate::plugins::UiTouchActionSet));
    #[cfg(not(target_arch = "wasm32"))]
    app.add_systems(
        Update,
        render
            .after(poll)
            .after(menu_stock_connection_buttons)
            .after(menu_stock_lan_discovery),
    );
    #[cfg(target_arch = "wasm32")]
    app.init_non_send::<BrowserTransport>()
        .add_systems(Update, render.after(poll));
}

#[allow(clippy::too_many_arguments)]
fn poll(
    route: Option<Res<crate::plugins::server_route::ServerRoute>>,
    current: Res<State<AppState>>,
    screen: Res<MenuScreen>,
    net: Res<NetState>,
    request: Res<LaunchRequest>,
    time: Res<Time>,
    refresh: Query<&Interaction, (Changed<Interaction>, With<MenuStockRefreshButton>)>,
    mut browser: ResMut<RoomBrowser>,
    #[cfg(target_arch = "wasm32")] mut transport: NonSendMut<BrowserTransport>,
) {
    let active = *current.get() == AppState::Menu
        && *screen == MenuScreen::Network
        && net.status == NetStatus::Disconnected;
    if !active {
        if browser.active {
            browser.stop();
            browser.active = false;
            browser.rendered.clear();
            #[cfg(target_arch = "wasm32")]
            transport.0.teardown(true);
        }
        return;
    }
    let now = time.elapsed_secs_f64();
    let refresh = !browser.active || refresh.iter().any(|i| *i == Interaction::Pressed);
    browser.active = true;
    let browser = &mut *browser;
    let mut received = Vec::new();
    if let Some(rx) = browser.rx.0.as_ref() {
        let rx = rx.lock().unwrap();
        loop {
            match rx.try_recv() {
                Ok(message) => received.push(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    received.push(NetInbound::Closed {
                        code: 1006,
                        reason: "大厅连接已断开".into(),
                    });
                    break;
                }
            }
        }
    }
    for message in received {
        match message {
            NetInbound::Control(message) => {
                let failed = matches!(message, Message::Error { .. });
                browser.message(message, now);
                if failed {
                    break;
                }
            }
            NetInbound::Closed { reason, .. } => {
                browser.fail(reason);
                break;
            }
            _ => {}
        }
    }
    if browser
        .waiting_since
        .is_some_and(|start| now - start >= 10.0)
    {
        browser.fail("获取 Web 房间超时，将自动重试".into());
    }
    if !route.as_ref().is_some_and(|route| route.pending())
        && browser.net.status == NetStatus::Disconnected
        && (refresh || browser.last_attempt.is_none_or(|last| now - last >= 5.0))
    {
        browser.last_attempt = Some(now);
        browser.net = NetState {
            address: crate::plugins::server_route::address(route.as_deref()),
            session_token: String::new(),
            room_id: String::new(),
            match_epoch: None,
            round_epoch: None,
            ..default()
        };
        #[cfg(not(target_arch = "wasm32"))]
        let result = begin_network_connection(
            &mut browser.net,
            &mut browser.tx,
            &mut browser.rx,
            &mut browser.native,
            &request,
        );
        #[cfg(target_arch = "wasm32")]
        let result = begin_network_connection(
            &mut browser.net,
            &mut browser.tx,
            &mut browser.rx,
            &request,
            &mut transport.0,
        );
        match result {
            Ok(()) => {
                browser.net.status = NetStatus::Connecting;
                browser.waiting_since = Some(now);
            }
            Err(error) => browser.fail(error),
        }
    } else if browser.net.transport_connected
        && (refresh || browser.last_request.is_none_or(|last| now - last >= 5.0))
    {
        browser.request_rooms(now);
    }
    #[cfg(target_arch = "wasm32")]
    flush_wasm_transport(&mut transport.0);
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn render(
    route: Option<Res<crate::plugins::server_route::ServerRoute>>,
    mut commands: Commands,
    screen: Res<MenuScreen>,
    time: Res<Time>,
    net: Res<NetState>,
    mut browser: ResMut<RoomBrowser>,
    #[cfg(not(target_arch = "wasm32"))] lan: Res<StockLanDiscoveryState>,
    font: Res<UiFont>,
    lists: Query<(Entity, Option<&Children>), With<LauncherNetworkRoomList>>,
    mut texts: Query<
        (&mut Text, Option<&MenuStockDiscoveryText>),
        Or<(With<MenuStockDiscoveryText>, With<MenuNetStatusText>)>,
    >,
) {
    if *screen != MenuScreen::Network {
        return;
    }
    let mut entries = Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    entries.extend(lan.rooms.iter().cloned().map(RoomTarget::Lan));
    entries.extend(browser.rooms.iter().cloned().map(RoomTarget::Web));
    entries.sort_by_key(RoomTarget::key);
    let signature: Vec<_> = entries.iter().map(|e| (e.key(), e.label())).collect();
    for (entity, children) in &lists {
        if signature != browser.rendered || children.is_none() {
            commands
                .entity(entity)
                .despawn_children()
                .with_children(|parent| {
                    if entries.is_empty() {
                        parent.spawn((
                            Text::new("暂无房间，可刷新重试或创建房间"),
                            TextFont {
                                font: font.0.clone().into(),
                                font_size: 12.0.into(),
                                ..default()
                            },
                            TextColor(crate::plugins::ui_theme::MUTED),
                        ));
                    }
                    for entry in &entries {
                        spawn_network_room_button(parent, &font.0, &entry.label(), entry.clone());
                    }
                });
        }
    }
    browser.rendered = signature;
    #[cfg(not(target_arch = "wasm32"))]
    let lan_status = lan
        .error
        .clone()
        .unwrap_or_else(|| format!("局域网：{} 个房间 · 自动搜索中", lan.rooms.len()));
    #[cfg(target_arch = "wasm32")]
    let lan_status = "浏览器不支持局域网广播发现".to_owned();
    let web_status = if let Some(error) = &browser.net.error {
        format!("Web：{error}（可刷新重试）")
    } else if !browser.active {
        "Web：房间搜索已暂停".into()
    } else if browser.net.transport_connected {
        format!(
            "Web：已连接 · {} 个房间{}",
            browser.rooms.len(),
            if browser.waiting_since.is_some() {
                " · 刷新中"
            } else {
                ""
            }
        )
    } else {
        "Web：连接大厅中…".into()
    };
    let updated = browser.updated_at.map_or_else(
        || "尚未完成".into(),
        |at| format!("{:.0} 秒前", (time.elapsed_secs_f64() - at).max(0.0)),
    );
    let action = net.error.clone().unwrap_or_else(|| match net.status {
        NetStatus::Disconnected => "点击房间加入；刷新同时更新局域网和 Web 列表".into(),
        NetStatus::Connecting => format!("正在加入：{}", net.address),
        NetStatus::InRoom => format!("已加入：{} · {} 人", net.room_id, net.members.len()),
    });
    for (mut text, summary) in &mut texts {
        let value = if summary.is_some() {
            format!("共 {} 个房间 · Web 更新：{updated}", entries.len())
        } else {
            format!(
                "{lan_status}  ·  {web_status}\n{}  ·  {action}",
                route.as_ref().filter(|r| r.pending()).map_or_else(
                    || crate::plugins::server_route::address(route.as_deref()),
                    |_| "正在选择联机服务器…".into(),
                )
            )
        };
        if text.0 != value {
            text.0 = value;
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn install_capture_fixture(app: &mut App) {
    let mut browser = RoomBrowser::default();
    browser.active = true;
    browser.net.transport_connected = true;
    browser.updated_at = Some(0.0);
    browser.rooms = ["江湖夜雨", "一线天 · 切磋"]
        .into_iter()
        .enumerate()
        .map(|(i, name)| RoomSummary {
            room_id: format!("web-{i}"),
            name: name.into(),
            mode: Some("deathmatch".into()),
            stage_id: Some("sn01".into()),
            configured: true,
            state: "lobby".into(),
            has_password: false,
            player_count: i + 2,
            max_players: 16,
            team_counts: None,
            owner_name: Some("江湖故人".into()),
        })
        .collect();
    let mut lan = StockLanDiscoveryState::default();
    lan.rooms.push(StockDiscoveredRoom {
        endpoint: "192.168.1.8:5230".parse().unwrap(),
        info: meteor_network::stock_udp::StockDiscoveryInfo::default_room(
            5230,
            "朋友的房间",
            "192.168.1.8",
        ),
        last_seen_ms: 0,
    });
    app.insert_resource(browser)
        .insert_resource(lan)
        .add_systems(Update, render);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn room(id: usize) -> RoomSummary {
        RoomSummary {
            room_id: format!("room-{id}"),
            name: format!("房间 {id}"),
            mode: Some("deathmatch".into()),
            stage_id: Some("sn01".into()),
            configured: true,
            state: "lobby".into(),
            has_password: false,
            player_count: 1,
            max_players: 16,
            team_counts: None,
            owner_name: Some("房主".into()),
        }
    }
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn original_lan_summary_uses_map_number_capacity_and_password() {
        let mut info = meteor_network::stock_udp::StockDiscoveryInfo::default_room(
            5230,
            "original",
            "127.0.0.1",
        );
        info.fields_0_to_6 = [9, 7, 6, 1, 16, 5, 1];
        info.field_0e = 1;
        let target = RoomTarget::Lan(StockDiscoveredRoom {
            endpoint: "127.0.0.1:5230".parse().unwrap(),
            info,
            last_seen_ms: 0,
        });
        let (_, _, detail, count) = target.presentation();
        assert!(detail.contains("四方阵"));
        assert!(detail.contains("流星 1 人 / 蝴蝶 0 人"));
        assert!(detail.contains("有密码"));
        assert!(detail.contains("9.07"));
        assert_eq!(count, "1 / 16");
    }

    #[test]
    fn room_metadata_displays_mode_map_and_teams_and_refreshes_on_team_change() {
        let mut entry = room(1);
        entry.team_counts = Some([2, 3]);
        let before = RoomTarget::Web(entry.clone()).label();
        assert!(before.contains("死斗"));
        assert!(before.contains("流星 2 人 / 蝴蝶 3 人"));
        assert!(!before.contains("sn01"));
        entry.team_counts = Some([3, 2]);
        assert_ne!(before, RoomTarget::Web(entry.clone()).label());
        entry.mode = Some("leader".into());
        assert!(!RoomTarget::Web(entry).label().contains("蝴蝶"));
        assert!(room_metadata(Some("deathmatch"), None, None).contains("阵营人数未提供"));
        assert!(room_metadata(None, None, None).contains("地图未提供"));
        assert!(room_metadata(None, Some("custom-map"), None).contains("custom-map"));
    }

    #[test]
    fn room_browser_refresh_never_joins_or_creates_even_when_empty() {
        let (sender, receiver) = mpsc::channel();
        let mut browser = RoomBrowser::default();
        browser.tx.begin_connect(sender);
        browser.message(
            Message::Welcome {
                player_id: "browser".into(),
                room_id: String::new(),
                members: vec![],
                client_id: "browser".into(),
                session_token: "unused".into(),
                resumed: false,
            },
            1.0,
        );
        assert!(matches!(
            receiver.try_recv(),
            Ok(NetOutbound::Control(Message::RoomList { rooms: None }))
        ));
        for rooms in [vec![], vec![room(1), room(2)]] {
            browser.message(Message::RoomList { rooms: Some(rooms) }, 2.0);
            assert!(
                receiver.try_recv().is_err(),
                "list response must not send RoomJoin"
            );
        }
        browser.request_rooms(6.0);
        assert!(matches!(
            receiver.try_recv(),
            Ok(NetOutbound::Control(Message::RoomList { rooms: None }))
        ));
        assert!(browser.net.session_token.is_empty());
    }
    #[test]
    fn room_browser_selection_joins_exactly_once_and_failure_allows_retry() {
        let (sender, receiver) = mpsc::channel();
        let mut tx = NetTx::default();
        tx.begin_connect(sender);
        let mut request = LaunchRequest::default();
        let mut net = NetState {
            status: NetStatus::Connecting,
            pending_web_join: Some(web_room_join(Some(&room(7)), &request)),
            ..default()
        };
        let mut end = MatchEnd::default();
        let mut next = NextState::default();
        let mut sequence = NetInputSequence::default();
        let mut pending = PendingNetActions::default();
        for message in [
            Message::Welcome {
                player_id: "player".into(),
                room_id: String::new(),
                members: vec![],
                client_id: "player".into(),
                session_token: "token".into(),
                resumed: false,
            },
            Message::RoomList {
                rooms: Some(vec![room(1)]),
            },
        ] {
            handle_net_message(
                &mut net,
                &mut request,
                &mut end,
                &mut next,
                message,
                &mut tx,
                &mut sequence,
                &mut pending,
                AppState::Menu,
            );
        }
        assert!(
            matches!(receiver.try_recv(), Ok(NetOutbound::Control(Message::RoomJoin { room_id, .. })) if room_id == "room-7")
        );
        assert!(receiver.try_recv().is_err());
        handle_net_message(
            &mut net,
            &mut request,
            &mut end,
            &mut next,
            Message::Error {
                code: "room_not_found".into(),
                text: "房间已离线".into(),
            },
            &mut tx,
            &mut sequence,
            &mut pending,
            AppState::Menu,
        );
        assert_eq!(net.status, NetStatus::Disconnected);
        assert!(net.pending_web_join.is_none());
        assert!(net.session_token.is_empty());
        assert_eq!(net.error.as_deref(), Some("房间已离线"));
    }

    #[test]
    fn room_browser_failure_clears_stale_rooms_and_closes_transport() {
        let (sender, receiver) = mpsc::channel();
        let mut browser = RoomBrowser::default();
        browser.tx.begin_connect(sender);
        browser.tx.activate();
        browser.rooms.push(room(1));
        browser.net.transport_connected = true;
        browser.fail("offline".into());
        assert!(browser.rooms.is_empty());
        assert!(!browser.net.transport_connected);
        assert_eq!(browser.net.error.as_deref(), Some("offline"));
        assert!(matches!(receiver.try_recv(), Ok(NetOutbound::Quit)));
    }
    #[test]
    fn room_browser_allows_running_but_blocks_locked_full_and_ended_rooms() {
        let mut entry = room(1);
        assert!(web_room_unavailable(&entry).is_none());
        entry.has_password = true;
        assert!(web_room_unavailable(&entry).unwrap().contains("密码"));
        entry.has_password = false;
        entry.player_count = 16;
        assert!(web_room_unavailable(&entry).unwrap().contains("满员"));
        entry.player_count = 1;
        entry.state = "running".into();
        assert!(web_room_unavailable(&entry).is_none());
        entry.state = "ended".into();
        assert!(web_room_unavailable(&entry).is_some());
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn room_browser_renders_both_sources_without_limit_and_survives_web_failure() {
        let mut app = App::new();
        let mut browser = RoomBrowser::default();
        browser.rooms = (0..12).map(room).collect();
        let lan_room = StockDiscoveredRoom {
            endpoint: "127.0.0.1:5230".parse().unwrap(),
            info: meteor_network::stock_udp::StockDiscoveryInfo::default_room(
                5230,
                "LAN",
                "127.0.0.1",
            ),
            last_seen_ms: 0,
        };
        let mut lan = StockLanDiscoveryState::default();
        lan.rooms.push(lan_room);
        app.insert_resource(MenuScreen::Network)
            .insert_resource(browser)
            .insert_resource(lan)
            .init_resource::<Time>()
            .init_resource::<NetState>()
            .insert_resource(UiFont(Handle::default()))
            .add_systems(Update, render);
        let list = app
            .world_mut()
            .spawn((Node::default(), LauncherNetworkRoomList))
            .id();
        app.world_mut().spawn((Text::default(), MenuNetStatusText));
        app.world_mut()
            .spawn((Text::default(), MenuStockDiscoveryText));
        app.update();
        assert_eq!(app.world().get::<Children>(list).unwrap().len(), 13);
        let mut targets = app.world_mut().query::<&RoomTarget>();
        assert_eq!(
            targets
                .iter(app.world())
                .filter(|t| matches!(t, RoomTarget::Web(_)))
                .count(),
            12
        );
        app.world_mut()
            .resource_mut::<RoomBrowser>()
            .fail("offline".into());
        app.update();
        assert_eq!(app.world().get::<Children>(list).unwrap().len(), 1);
        let mut targets = app.world_mut().query::<&RoomTarget>();
        assert!(matches!(
            targets.single(app.world()).unwrap(),
            RoomTarget::Lan(_)
        ));
    }
}
