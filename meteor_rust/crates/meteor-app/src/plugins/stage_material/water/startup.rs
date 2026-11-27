use super::bake_cache::{Prepared, PREPARED};
use crate::plugins::ui_theme as theme;
use bevy::prelude::*;
use std::sync::{mpsc, Arc, Mutex};

#[derive(Resource, Default)]
pub(crate) struct WaterStartup {
    pub ready: bool,
    progress: Arc<Mutex<(f32, String)>>,
    receiver: Option<Mutex<mpsc::Receiver<Result<Vec<Prepared>, String>>>>,
}
#[derive(Component)]
struct BakeRoot;
#[derive(Component)]
struct BakeCaption;
#[derive(Component)]
struct BakeBar;
pub(crate) fn install(app: &mut App) {
    app.init_resource::<WaterStartup>()
        .add_systems(Startup, begin)
        .add_systems(Update, poll);
}
fn begin(
    mut commands: Commands,
    mut state: ResMut<WaterStartup>,
    font: Res<crate::plugins::launcher_ui::UiFont>,
    graphics: Option<Res<crate::plugins::graphics_pack::GraphicsPacks>>,
) {
    let Some((graphics, water)) = graphics.as_deref().and_then(|packs| {
        packs
            .water()
            .filter(|water| water.enabled())
            .map(|water| (packs, water.clone()))
    }) else {
        state.ready = true;
        return;
    };
    let graphics = graphics.catalog_snapshot();
    *state.progress.lock().unwrap() = (0.0, "正在检查水面烘焙缓存".into());
    let progress = state.progress.clone();
    let (tx, rx) = mpsc::channel();
    state.receiver = Some(Mutex::new(rx));
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            super::bake_cache::prepare(
                &water.config,
                &graphics,
                &super::bake_cache::directory(),
                |value, caption| {
                    *progress.lock().unwrap() = (value, caption);
                },
            )
        })
        .unwrap_or_else(|_| Err("水面烘焙任务异常终止".into()));
        let _ = tx.send(result);
    });
    commands
        .spawn((
            BakeRoot,
            theme::CinematicBackdrop,
            GlobalZIndex(3000),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                flex_direction: FlexDirection::Column,
                row_gap: px(18),
                padding: UiRect::all(px(32)),
                ..default()
            },
            BackgroundColor(theme::BACKGROUND),
        ))
        .with_children(|root| {
            root.spawn((
                BakeCaption,
                Text::new("正在准备水面"),
                TextFont {
                    font: font.0.clone().into(),
                    font_size: 18.0.into(),
                    ..default()
                },
                TextColor(theme::TEXT),
            ));
            root.spawn((
                Node {
                    width: percent(70),
                    max_width: px(460),
                    height: px(6),
                    ..default()
                },
                BackgroundColor(theme::LINE),
            ))
            .with_children(|track| {
                track.spawn((
                    BakeBar,
                    Node {
                        width: percent(0),
                        height: percent(100),
                        ..default()
                    },
                    BackgroundColor(theme::METAL),
                ));
            });
            root.spawn((
                Text::new("首次启动或资源变化时自动烘焙，完成后将复用缓存"),
                TextFont {
                    font: font.0.clone().into(),
                    font_size: 13.0.into(),
                    ..default()
                },
                TextColor(theme::MUTED),
            ));
        });
}
fn poll(
    mut commands: Commands,
    mut state: ResMut<WaterStartup>,
    roots: Query<Entity, With<BakeRoot>>,
    mut captions: Query<&mut Text, With<BakeCaption>>,
    mut bars: Query<&mut Node, With<BakeBar>>,
) {
    if state.ready {
        return;
    }
    let result = state
        .receiver
        .as_ref()
        .and_then(|r| match r.lock().unwrap().try_recv() {
            Ok(value) => Some(value),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("水面烘焙任务已中断".into())),
            Err(mpsc::TryRecvError::Empty) => None,
        });
    if let Some(result) = result {
        state.receiver = None;
        match result {
            Ok(surfaces) => {
                let _ = PREPARED.set(surfaces);
                state.ready = true;
                for e in &roots {
                    commands.entity(e).despawn();
                }
                return;
            }
            Err(error) => {
                error!("{error}");
                state.progress.lock().unwrap().1 =
                    format!("{error}\n请检查配置、资源和缓存目录后重新启动");
            }
        }
    }
    let (value, caption) = state.progress.lock().unwrap().clone();
    for mut text in &mut captions {
        text.0 = format!("{caption}  {:.0}%", value * 100.0);
    }
    for mut node in &mut bars {
        node.width = percent(value * 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_worker_keeps_startup_blocked_and_displays_the_reason() {
        let mut app = App::new();
        let (tx, rx) = mpsc::channel();
        let state = WaterStartup {
            receiver: Some(Mutex::new(rx)),
            ..default()
        };
        *state.progress.lock().unwrap() = (0.4, "正在自动烘焙水面".into());
        app.insert_resource(state).add_systems(Update, poll);
        let text = app.world_mut().spawn((BakeCaption, Text::new(""))).id();
        let bar = app.world_mut().spawn((BakeBar, Node::default())).id();
        app.update();
        assert_eq!(app.world().get::<Node>(bar).unwrap().width, percent(40));
        assert!(app.world().get::<Text>(text).unwrap().0.contains("40%"));
        tx.send(Err("测试资源缺失".into())).unwrap();
        app.update();
        assert!(!app.world().resource::<WaterStartup>().ready);
        assert!(app
            .world()
            .get::<Text>(text)
            .unwrap()
            .0
            .contains("测试资源缺失"));
        assert!(app.world().resource::<WaterStartup>().receiver.is_none());
    }
}
