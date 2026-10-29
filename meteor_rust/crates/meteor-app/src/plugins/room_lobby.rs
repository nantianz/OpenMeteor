use super::*;

#[derive(Component)]
pub(crate) struct ProfileArrow {
    pub character: bool,
    pub direction: i8,
}
#[derive(Component)]
pub(crate) struct ProfileText(pub bool);
#[derive(Component)]
pub(crate) struct ReadyButton;
#[derive(Component)]
pub(crate) struct LobbyHint;

const WEAPONS: [(u8, &str); 12] = [
    (6, "sword"),
    (5, "dagger"),
    (8, "blade"),
    (7, "spear"),
    (9, "hammer"),
    (4, "stinger"),
    (1, "dart"),
    (2, "wheel"),
    (3, "gun"),
    (10, "staffBlade"),
    (11, "fist"),
    (12, "ninjaBlade"),
];

impl NetState {
    pub(crate) fn selected_room_team(&self) -> Option<u32> {
        if self.battle_pending {
            self.late_join_team
                .or_else(|| self.local_room_member().and_then(|m| m.team))
        } else {
            self.local_room_member().and_then(|m| m.team)
        }
    }
    pub(crate) fn room_owner(&self) -> Option<&str> {
        self.lobby
            .owner_player_id
            .as_deref()
            .or_else(|| self.members.first().map(|m| m.player_id.as_str()))
    }
    pub(crate) fn is_room_owner(&self) -> bool {
        self.room_owner() == Some(self.player_id.as_str())
    }
    pub(crate) fn local_room_member(&self) -> Option<&RoomMemberWire> {
        self.members.iter().find(|m| m.player_id == self.player_id)
    }
    pub(crate) fn room_capacity(&self) -> usize {
        self.lobby.max_players.unwrap_or(16).clamp(1, 16)
    }
    pub(crate) fn can_select_room_team(&self) -> bool {
        self.can_edit_room_profile()
    }
    pub(crate) fn can_edit_room_profile(&self) -> bool {
        self.can_edit_room()
            || (!self.is_stock_udp()
                && self.status == NetStatus::InRoom
                && self.transport_connected
                && self.battle_pending
                && !self.round_active
                && self.lobby.state.as_deref() == Some("running")
                && !self.lobby.asset_loading.unwrap_or(false)
                && self.local_room_member().is_some_and(|m| m.connected))
    }
    pub(crate) fn can_edit_room(&self) -> bool {
        !self.is_stock_udp()
            && self.status == NetStatus::InRoom
            && self.transport_connected
            && !self.round_active
            && self
                .lobby
                .state
                .as_deref()
                .is_none_or(|state| state == "lobby")
            && !self.lobby.asset_loading.unwrap_or(false)
            && self.local_room_member().is_some_and(|m| m.connected)
    }
    pub(crate) fn room_start_blocker(&self) -> Option<&'static str> {
        if !self.can_edit_room() {
            return Some("正在连接或同步房间，请稍候");
        }
        if !self.is_room_owner() {
            return Some("等待房主开始游戏");
        }
        if !self
            .lobby
            .configured
            .unwrap_or(!self.mode.is_empty() && !self.stage.is_empty())
        {
            return Some("请房主先设置游戏模式和地图");
        }
        if self.members.iter().any(|m| !m.connected) {
            return Some("有玩家断线，等待重新连接");
        }
        if is_team_mode(&self.mode) && self.members.iter().any(|m| !matches!(m.team, Some(1 | 2))) {
            return Some("仍有玩家未选择阵营");
        }
        if self
            .members
            .iter()
            .any(|m| Some(m.player_id.as_str()) != self.room_owner() && !m.ready)
        {
            return Some("等待其他玩家准备");
        }
        None
    }
}

pub(super) fn profile_input(
    arrows: Query<(&Interaction, &ProfileArrow), Changed<Interaction>>,
    net: Res<NetState>,
    tx: Res<NetTx>,
    mut request: ResMut<LaunchRequest>,
) {
    if !net.can_edit_room_profile() {
        return;
    }
    for (interaction, arrow) in &arrows {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if arrow.character {
            request.character_id =
                (i32::from(request.character_id) + i32::from(arrow.direction)).rem_euclid(20) as u8;
        } else {
            let index = WEAPONS
                .iter()
                .position(|(id, _)| *id == request.weapon)
                .unwrap_or(0);
            request.weapon =
                WEAPONS[(index as i32 + i32::from(arrow.direction)).rem_euclid(12) as usize].0;
        }
        tx.send_control(Message::RoomProfile {
            character: format!("p{}", request.character_id),
            weapon: WEAPONS
                .iter()
                .find(|(id, _)| *id == request.weapon)
                .unwrap()
                .1
                .into(),
        });
    }
}

pub(super) fn ready_input(
    buttons: Query<&Interaction, (Changed<Interaction>, With<ReadyButton>)>,
    net: Res<NetState>,
    tx: Res<NetTx>,
) {
    if !net.can_select_room_team() || (net.is_room_owner() && !net.battle_pending) {
        return;
    }
    let Some(me) = net.local_room_member() else {
        return;
    };
    if is_team_mode(&net.mode) && !matches!(net.selected_room_team(), Some(1 | 2)) {
        return;
    }
    if buttons.iter().any(|i| *i == Interaction::Pressed) {
        if net.battle_pending {
            if let Some(team) = net.selected_room_team() {
                tx.send_control(Message::TeamSelect { team });
                return;
            }
        }
        tx.send_control(Message::RoomReady {
            ready: net.battle_pending || !me.ready,
        });
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn update(
    mut commands: Commands,
    net: Res<NetState>,
    request: Res<LaunchRequest>,
    mut buttons: Query<
        (
            Entity,
            &mut Node,
            &Children,
            Option<&ReadyButton>,
            Has<bevy::ui::InteractionDisabled>,
        ),
        Or<(
            With<ReadyButton>,
            With<crate::app_state::NetRoomStartButton>,
        )>,
    >,
    arrows: Query<(Entity, Has<bevy::ui::InteractionDisabled>), With<ProfileArrow>>,
    mut labels: Query<&mut Text, (Without<ProfileText>, Without<LobbyHint>)>,
    mut profiles: Query<(&ProfileText, &mut Text), Without<LobbyHint>>,
    mut hints: Query<&mut Text, (With<LobbyHint>, Without<ProfileText>)>,
) {
    for (entity, mut node, children, ready, disabled) in &mut buttons {
        let is_ready = ready.is_some();
        let enabled = if is_ready {
            net.can_select_room_team()
                && (!is_team_mode(&net.mode) || matches!(net.selected_room_team(), Some(1 | 2)))
        } else {
            net.room_start_blocker().is_none()
        };
        if enabled == disabled {
            if enabled {
                commands
                    .entity(entity)
                    .remove::<bevy::ui::InteractionDisabled>();
            } else {
                commands
                    .entity(entity)
                    .insert(bevy::ui::InteractionDisabled);
            }
        }
        node.display = if is_ready != (net.is_room_owner() && !net.battle_pending) {
            Display::Flex
        } else {
            Display::None
        };
        let label = if is_ready {
            if net.battle_pending {
                "开始"
            } else if net.local_room_member().is_some_and(|m| m.ready) {
                "取消准备"
            } else {
                "准备"
            }
        } else {
            "开始游戏"
        };
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child) {
                if text.0 != label {
                    text.0 = label.into();
                }
            }
        }
    }
    for (entity, disabled) in &arrows {
        if net.can_edit_room_profile() == disabled {
            if disabled {
                commands
                    .entity(entity)
                    .remove::<bevy::ui::InteractionDisabled>();
            } else {
                commands
                    .entity(entity)
                    .insert(bevy::ui::InteractionDisabled);
            }
        }
    }
    for (marker, mut text) in &mut profiles {
        let label = if marker.0 {
            format!(
                "人物：{}",
                crate::plugins::launcher_ui::character_name(request.character_id)
            )
        } else {
            format!(
                "兵器：{}",
                crate::plugins::ui_plugin::hud_weapon_name(request.weapon)
            )
        };
        if text.0 != label {
            text.0 = label;
        }
    }
    let hint = net.error.clone().unwrap_or_else(|| {
        if net.lobby.state.as_deref() == Some("running") && !net.round_active {
            if net.lobby.asset_loading.unwrap_or(false) {
                "正在等待所有玩家加载模型".into()
            } else if is_team_mode(&net.mode) && net.selected_room_team().is_none() {
                "请选择流星或蝴蝶阵营，再点击开始".into()
            } else {
                "点击开始，加载模型后进入当前战局".into()
            }
        } else if net.is_room_owner() {
            net.room_start_blocker()
                .unwrap_or("所有玩家已准备，可以开始游戏")
                .into()
        } else if !net.can_select_room_team() {
            "正在连接或同步房间，请稍候".into()
        } else if is_team_mode(&net.mode)
            && net
                .local_room_member()
                .is_some_and(|m| !matches!(m.team, Some(1 | 2)))
        {
            "请选择流星或蝴蝶阵营，再准备".into()
        } else if net.local_room_member().is_some_and(|m| m.ready) {
            "已准备，等待房主开始；修改整备会取消准备".into()
        } else {
            "选择人物、兵器和阵营后点击准备".into()
        }
    });
    for mut text in &mut hints {
        if text.0 != hint {
            text.0 = hint.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn member(id: &str, ready: bool) -> RoomMemberWire {
        RoomMemberWire {
            player_id: id.into(),
            name: id.into(),
            character_id: 3,
            weapon: 6,
            team: Some(1),
            ready,
            connected: true,
        }
    }
    fn room(player: &str) -> NetState {
        NetState {
            status: NetStatus::InRoom,
            transport_connected: true,
            player_id: player.into(),
            mode: "deathmatch".into(),
            stage: "sn01".into(),
            members: vec![member("guest", false), member("owner", false)],
            lobby: meteor_network::protocol::RoomLobbyWire {
                owner_player_id: Some("owner".into()),
                configured: Some(true),
                max_players: Some(16),
                state: Some("lobby".into()),
                asset_loading: Some(false),
            },
            ..default()
        }
    }
    #[test]
    fn late_join_profile_arrows_send_selection_and_lock_during_loading() {
        let (sender, receiver) = mpsc::channel();
        let mut tx = NetTx::default();
        tx.begin_connect(sender);
        tx.activate();
        let mut net = room("guest");
        net.battle_pending = true;
        net.lobby.state = Some("running".into());
        assert!(net.room_start_blocker().is_some());
        let mut app = App::new();
        app.insert_resource(net)
            .insert_resource(tx)
            .insert_resource(LaunchRequest {
                character_id: 3,
                weapon: 6,
                ..default()
            })
            .add_systems(Update, (profile_input, update).chain());
        let arrow = app
            .world_mut()
            .spawn((
                Interaction::None,
                ProfileArrow {
                    character: true,
                    direction: 1,
                },
                bevy::ui::InteractionDisabled,
            ))
            .id();
        app.update();
        assert!(app
            .world()
            .get::<bevy::ui::InteractionDisabled>(arrow)
            .is_none());
        *app.world_mut().get_mut::<Interaction>(arrow).unwrap() = Interaction::Pressed;
        app.update();
        assert!(
            matches!(receiver.try_recv(), Ok(NetOutbound::Control(Message::RoomProfile { character, weapon }))
            if character == "p4" && weapon == "sword")
        );
        *app.world_mut().get_mut::<Interaction>(arrow).unwrap() = Interaction::None;
        app.world_mut()
            .get_mut::<ProfileArrow>(arrow)
            .unwrap()
            .character = false;
        app.update();
        *app.world_mut().get_mut::<Interaction>(arrow).unwrap() = Interaction::Pressed;
        app.update();
        assert!(
            matches!(receiver.try_recv(), Ok(NetOutbound::Control(Message::RoomProfile { character, weapon }))
            if character == "p4" && weapon == "dagger")
        );
        app.world_mut()
            .resource_mut::<NetState>()
            .lobby
            .asset_loading = Some(true);
        *app.world_mut().get_mut::<Interaction>(arrow).unwrap() = Interaction::Pressed;
        app.update();
        assert!(app
            .world()
            .get::<bevy::ui::InteractionDisabled>(arrow)
            .is_some());
        assert!(receiver.try_recv().is_err());
        let mut net = app.world_mut().resource_mut::<NetState>();
        net.lobby.asset_loading = Some(false);
        net.battle_pending = false;
        assert!(!net.can_edit_room_profile());
        net.battle_pending = true;
        net.transport_connected = false;
        assert!(!net.can_edit_room_profile());
    }

    #[test]
    fn late_join_team_choice_waits_for_start_and_button_says_start() {
        let (sender, receiver) = mpsc::channel();
        let mut tx = NetTx::default();
        tx.begin_connect(sender);
        tx.activate();
        let mut net = room("guest");
        net.battle_pending = true;
        net.lobby.state = Some("running".into());
        net.members[0].team = None;
        let mut app = App::new();
        app.insert_resource(net)
            .insert_resource(tx)
            .init_resource::<LaunchRequest>()
            .add_systems(
                Update,
                (net_room_team_select_buttons, ready_input, update).chain(),
            );
        let label = app.world_mut().spawn(Text::new("准备")).id();
        let ready = app
            .world_mut()
            .spawn((Interaction::None, Node::default(), ReadyButton))
            .add_child(label)
            .id();
        app.world_mut().spawn((
            Interaction::Pressed,
            crate::app_state::NetRoomTeamSelectButton { team: 2 },
        ));
        app.update();
        assert_eq!(
            app.world().resource::<NetState>().selected_room_team(),
            Some(2)
        );
        assert!(
            receiver.try_recv().is_err(),
            "choosing a camp must not enter the match"
        );
        assert_eq!(app.world().get::<Text>(label).unwrap().0, "开始");
        *app.world_mut().get_mut::<Interaction>(ready).unwrap() = Interaction::Pressed;
        app.update();
        assert!(matches!(
            receiver.try_recv(),
            Ok(NetOutbound::Control(Message::TeamSelect { team: 2 }))
        ));
        assert!(
            receiver.try_recv().is_err(),
            "Web team entry uses only team.select"
        );
    }

    #[test]
    fn web_lobby_uses_server_owner_and_waits_for_ready_online_members() {
        let mut net = room("owner");
        assert!(net.is_room_owner());
        assert!(net.room_start_blocker().unwrap().contains("准备"));
        net.members[0].ready = true;
        assert!(net.room_start_blocker().is_none());
        net.members[0].connected = false;
        assert!(net.room_start_blocker().unwrap().contains("断线"));
        net.members[0].connected = true;
        net.members[0].team = None;
        assert!(net.room_start_blocker().unwrap().contains("阵营"));
        net.members[0].team = Some(2);
        net.lobby.asset_loading = Some(true);
        assert!(!net.can_edit_room());
        net.lobby.asset_loading = Some(false);
        net.lobby.configured = Some(false);
        assert!(net.room_start_blocker().unwrap().contains("模式"));
    }
    #[test]
    fn web_lobby_profile_ready_and_start_use_the_web_request_messages() {
        let (sender, receiver) = mpsc::channel();
        let mut tx = NetTx::default();
        tx.begin_connect(sender);
        tx.activate();
        let mut app = App::new();
        app.insert_resource(room("guest"))
            .insert_resource(tx)
            .insert_resource(LaunchRequest {
                character_id: 3,
                weapon: 6,
                ..default()
            })
            .add_systems(Update, (profile_input, ready_input, net_room_start_button));
        let arrow = app
            .world_mut()
            .spawn((
                Interaction::Pressed,
                ProfileArrow {
                    character: true,
                    direction: 1,
                },
            ))
            .id();
        app.update();
        assert!(
            matches!(receiver.try_recv(), Ok(NetOutbound::Control(Message::RoomProfile { character, weapon })) if character == "p4" && weapon == "sword")
        );
        assert_eq!(
            app.world().resource::<NetState>().members[0].character_id,
            3,
            "only room.state updates accepted profile"
        );
        *app.world_mut().get_mut::<Interaction>(arrow).unwrap() = Interaction::None;
        let ready = app
            .world_mut()
            .spawn((Interaction::Pressed, ReadyButton))
            .id();
        app.update();
        assert!(matches!(
            receiver.try_recv(),
            Ok(NetOutbound::Control(Message::RoomReady { ready: true }))
        ));
        *app.world_mut().get_mut::<Interaction>(ready).unwrap() = Interaction::None;
        app.update();
        app.world_mut().resource_mut::<NetState>().members[0].ready = true;
        *app.world_mut().get_mut::<Interaction>(ready).unwrap() = Interaction::Pressed;
        app.update();
        assert!(matches!(
            receiver.try_recv(),
            Ok(NetOutbound::Control(Message::RoomReady { ready: false }))
        ));
        *app.world_mut().get_mut::<Interaction>(ready).unwrap() = Interaction::None;
        let start = app
            .world_mut()
            .spawn((Interaction::Pressed, crate::app_state::NetRoomStartButton))
            .id();
        app.update();
        assert!(receiver.try_recv().is_err(), "guest cannot start");
        *app.world_mut().get_mut::<Interaction>(start).unwrap() = Interaction::None;
        app.update();
        app.world_mut().resource_mut::<NetState>().player_id = "owner".into();
        *app.world_mut().get_mut::<Interaction>(start).unwrap() = Interaction::Pressed;
        app.update();
        assert!(matches!(
            receiver.try_recv(),
            Ok(NetOutbound::Control(Message::RoomStart))
        ));
    }
    #[test]
    fn web_lobby_authoritative_profile_and_team_change_clear_ready_and_sync_selection() {
        let mut net = room("guest");
        net.members[0].ready = true;
        let message = Message::from_json(r#"{"type":"room.state","roomId":"r","mode":"deathmatch","stageId":"sn01","ownerPlayerId":"owner","configured":true,"maxPlayers":16,"state":"lobby","players":[{"id":"guest","character":"p8","weapon":"staffBlade","team":2,"ready":false,"connected":true},{"id":"owner","character":"p1","weapon":"sword","team":1,"ready":false,"connected":true}]}"#).unwrap();
        let mut request = LaunchRequest::default();
        handle_net_message(
            &mut net,
            &mut request,
            &mut MatchEnd::default(),
            &mut NextState::default(),
            message,
            &mut NetTx::default(),
            &mut NetInputSequence::default(),
            &mut PendingNetActions::default(),
            AppState::NetRoom,
        );
        assert_eq!((request.character_id, request.weapon), (8, 10));
        assert_eq!(net.local_room_member().unwrap().team, Some(2));
        assert!(!net.local_room_member().unwrap().ready);
        assert_eq!(net.room_capacity(), 16);
        assert!(!net.is_room_owner());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn web_lobby_actions_never_reach_stock_client_or_host_sessions() {
        for address in ["udp://127.0.0.1:5230", "udp-host://0.0.0.0:5230"] {
            for player in ["guest", "owner"] {
                let (sender, receiver) = mpsc::channel();
                let mut tx = NetTx::default();
                tx.begin_connect(sender);
                tx.activate();
                let mut net = room(player);
                net.address = address.into();
                net.members[0].ready = true;

                assert!(!net.can_edit_room());
                let mut app = App::new();
                app.insert_resource(net)
                    .insert_resource(tx)
                    .init_resource::<LaunchRequest>()
                    .add_systems(
                        Update,
                        (
                            profile_input,
                            ready_input,
                            net_room_start_button,
                            net_room_team_select_buttons,
                        ),
                    );
                app.world_mut().spawn((
                    Interaction::Pressed,
                    ProfileArrow {
                        character: true,
                        direction: 1,
                    },
                ));
                app.world_mut().spawn((Interaction::Pressed, ReadyButton));
                app.world_mut()
                    .spawn((Interaction::Pressed, crate::app_state::NetRoomStartButton));
                app.world_mut().spawn((
                    Interaction::Pressed,
                    crate::app_state::NetRoomTeamSelectButton { team: 2 },
                ));
                app.update();
                assert!(
                    receiver.try_recv().is_err(),
                    "Web commands leaked to {address} as {player}"
                );
            }
        }
    }

    #[test]
    fn room_latency_broadcast_updates_only_current_member_without_changing_game_state() {
        for state in [AppState::NetRoom, AppState::Playing] {
            let mut net = room("guest");
            net.address = "ws://test/ws".into();
            net.room_id = "room".into();
            net.round_active = state == AppState::Playing;
            let (sender, receiver) = mpsc::channel();
            let mut tx = NetTx::default();
            tx.begin_connect(sender);
            tx.activate();
            let mut request = LaunchRequest::default();
            let mut next = NextState::default();
            for (room_id, player_id, rtt) in [
                ("room", "guest", 20.0),
                ("room", "guest", 38.0),
                ("other-room", "guest", 99.0),
                ("room", "unknown", 99.0),
            ] {
                let message = Message::RoomLatency {
                    room_id: room_id.into(),
                    player_id: player_id.into(),
                    latency: meteor_network::protocol::RoomLatencyWire {
                        server_rtt: Some(rtt),
                        host_rtt: None,
                        p2p_state: "connecting".into(),
                    },
                };
                handle_net_message(
                    &mut net,
                    &mut request,
                    &mut MatchEnd::default(),
                    &mut next,
                    message,
                    &mut tx,
                    &mut NetInputSequence::default(),
                    &mut PendingNetActions::default(),
                    state,
                );
            }
            assert_eq!(net.room_latency.len(), 1);
            assert_eq!(net.room_latency["guest"].server_rtt, Some(38.0));
            assert!(net.error.is_none());
            assert_eq!(net.status, NetStatus::InRoom);
            assert_eq!(net.round_active, state == AppState::Playing);
            assert!(matches!(next, NextState::Unchanged));
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn web_lobby_all_twelve_weapon_labels_round_trip_to_original_ids() {
        for (id, label) in WEAPONS {
            assert_eq!(meteor_network::protocol::weapon_id_from_web_name(label), id);
        }
    }
    #[test]
    fn web_lobby_ui_queries_are_compatible_and_show_guest_ready_only() {
        let mut app = App::new();
        app.insert_resource(room("guest"))
            .init_resource::<LaunchRequest>()
            .add_systems(Update, update);
        let ready = app
            .world_mut()
            .spawn((Node::default(), ReadyButton))
            .with_children(|p| {
                p.spawn(Text::default());
            })
            .id();
        let start = app
            .world_mut()
            .spawn((Node::default(), crate::app_state::NetRoomStartButton))
            .with_children(|p| {
                p.spawn(Text::default());
            })
            .id();
        app.world_mut().spawn((Text::default(), ProfileText(true)));
        app.world_mut().spawn((Text::default(), ProfileText(false)));
        app.world_mut().spawn((Text::default(), LobbyHint));
        app.update();
        assert_eq!(
            app.world().get::<Node>(ready).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<Node>(start).unwrap().display,
            Display::None
        );
        app.world_mut().resource_mut::<NetState>().player_id = "owner".into();
        app.update();
        assert_eq!(
            app.world().get::<Node>(ready).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<Node>(start).unwrap().display,
            Display::Flex
        );
        assert!(app
            .world()
            .get::<bevy::ui::InteractionDisabled>(start)
            .is_some());
    }
}
