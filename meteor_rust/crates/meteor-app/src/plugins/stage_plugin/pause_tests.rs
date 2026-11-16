use super::*;
use crate::plugins::network_plugin::{NetState, NetStatus};
use crate::plugins::{install_frame_clock, simulation_frame, FrameClock};
use std::time::Duration;

fn app() -> App {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res/Level");
    let source = crate::text_codec::decode_text(&std::fs::read(root.join("sn09_.pst")).unwrap());
    let definition = meteor_scene::mission_runtime::parse_mission_definition(&source);
    let mut mission = MissionScriptRuntime::new(&source, definition.values);
    mission.start().unwrap();
    mission.host_mut().register_char("冷燕", 1);
    mission.host_mut().register_char("屠城", 2);
    for id in 0..=2 {
        mission.host_mut().sync_fighter(id, 3000.0, 3000.0, 2.0);
    }
    let scene = SceneScriptRuntime::load(
        "int ticks = 0; Scene_OnIdle() { ticks = ticks + 1; SetSceneItem(0, \"ticks\", ticks); }",
        vec![SceneItemInstance {
            name: "clock_probe".into(),
            index: 0,
            props: Default::default(),
        }],
        10_000,
    )
    .unwrap();
    let mut app = App::new();
    app.add_plugins(bevy::state::app::StatesPlugin)
        .init_state::<AppState>()
        .init_resource::<Time>()
        .init_resource::<Time<Real>>()
        .insert_resource(crate::app_state::Settings {
            target_fps: 0,
            ..default()
        })
        .init_resource::<LaunchRequest>()
        .init_resource::<NetState>()
        .insert_resource(StageMissionRuntime::from_script(mission))
        .insert_resource(StageScriptRuntime::from_script(scene))
        .init_resource::<crate::plugins::stage_collider::StageCollider>()
        .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
        .add_systems(
            Update,
            (tick_mission_script, tick_stage_script).run_if(simulation_frame),
        );
    install_frame_clock(&mut app);
    transition(&mut app, AppState::Playing);
    app
}

fn step(app: &mut App, seconds: f64) {
    app.world_mut()
        .resource_mut::<Time<Real>>()
        .advance_by(Duration::from_secs_f64(seconds));
    app.update();
}

fn transition(app: &mut App, state: AppState) {
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(state);
    step(app, 0.05);
}

fn mission(app: &App) -> &MissionScriptRuntime {
    app.world()
        .resource::<StageMissionRuntime>()
        .script()
        .unwrap()
}

fn scene_ticks(app: &App) -> f64 {
    app.world()
        .resource::<StageScriptRuntime>()
        .script()
        .unwrap()
        .host()
        .instance(0)
        .unwrap()
        .get("ticks")
        .as_f64()
}

#[cfg(feature = "original-assets")]
#[test]
fn solo_pause_freezes_sn09_mission_and_scene_until_resume() {
    let mut app = app();
    for _ in 0..3900 {
        step(&mut app, 0.05);
    }
    assert!(mission(&app).elapsed() > 194.0);
    assert_eq!(mission(&app).host().game_over(), None);
    let elapsed = mission(&app).elapsed();
    let ticks = scene_ticks(&app);

    for state in [AppState::Paused, AppState::Settings, AppState::Paused] {
        transition(&mut app, state);
        for _ in 0..2000 {
            step(&mut app, 0.05);
        }
        assert_eq!(mission(&app).elapsed(), elapsed);
        assert_eq!(mission(&app).host().game_over(), None);
        assert_eq!(scene_ticks(&app), ticks);
        let clock = app.world().resource::<FrameClock>();
        assert_eq!(
            (clock.sim_dt, clock.wall_dt, clock.wall_step),
            (0.0, 0.0, 0.0)
        );
    }
    transition(&mut app, AppState::Playing);
    assert!((mission(&app).elapsed() - elapsed - 0.05).abs() < 1e-6);
    assert!((app.world().resource::<FrameClock>().wall_step - 0.05).abs() < 1e-6);
    assert_eq!(mission(&app).host().game_over(), None);
    for _ in 0..200 {
        step(&mut app, 0.05);
    }
    assert_eq!(mission(&app).host().game_over(), Some(1));
}

#[cfg(feature = "original-assets")]
#[test]
fn solo_pause_discards_capped_frame_debt_without_changing_target_fps() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<crate::app_state::Settings>()
        .target_fps = 30;
    step(&mut app, 0.008);
    let elapsed = mission(&app).elapsed();
    transition(&mut app, AppState::Paused);
    step(&mut app, 300.0);
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Playing);
    for _ in 0..4 {
        step(&mut app, 0.008);
    }
    assert_eq!(mission(&app).elapsed(), elapsed);
    step(&mut app, 0.008);
    let clock = app.world().resource::<FrameClock>();

    assert!((clock.sim_dt - 0.04).abs() < 1e-6);
    assert_eq!(
        app.world()
            .resource::<crate::app_state::Settings>()
            .target_fps,
        30
    );
    assert!((clock.wall_step - 0.04).abs() < 1e-6);
    assert!((mission(&app).elapsed() - elapsed - 0.04).abs() < 1e-6);

    for _ in 0..4 {
        step(&mut app, 0.008);
    }
    let clock = app.world().resource::<FrameClock>();
    assert!((clock.sim_dt - 0.032).abs() < 1e-6);
    assert!((clock.wall_step - 0.032).abs() < 1e-6);
    assert!((mission(&app).elapsed() - elapsed - 0.072).abs() < 1e-6);
}

#[cfg(feature = "original-assets")]
#[test]
fn network_pause_keeps_scene_authority_clock_running() {
    let mut app = app();
    let mut net = app.world_mut().resource_mut::<NetState>();
    net.status = NetStatus::InRoom;
    net.player_id = "host".into();
    net.members.push(meteor_network::protocol::RoomMemberWire {
        player_id: "host".into(),
        name: "host".into(),
        character_id: 0,
        weapon: 5,
        team: Some(1),
        ready: true,
        connected: true,
    });
    drop(net);
    for state in [AppState::Paused, AppState::Settings] {
        let before = scene_ticks(&app);
        transition(&mut app, state);
        for _ in 0..20 {
            step(&mut app, 0.05);
        }
        assert!(scene_ticks(&app) > before);
        assert!(app.world().resource::<FrameClock>().sim_dt > 0.0);
    }
}
