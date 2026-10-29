use bevy::prelude::*;

use crate::app_state::{Keymap, LaunchRequest};
use crate::components::fighter::{Health, LockOnController, MotionState};
use crate::plugins::network_plugin::{NetState, NetStatus, RemoteFighter};
use crate::plugins::stage_plugin::{LocalFighter, MissionNpc};

#[derive(Resource, Default)]
pub(super) struct SpectatorState {
    pub active: bool,
    pub target: Option<Entity>,
    name: String,
}

pub(super) fn reset_spectator(mut state: ResMut<SpectatorState>) {
    *state = SpectatorState::default();
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn update_spectator(
    mut state: ResMut<SpectatorState>,
    net: Option<Res<NetState>>,
    request: Option<Res<LaunchRequest>>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<Keymap>,
    debug: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    local: Query<&Health, With<LocalFighter>>,
    candidates: Query<
        (Entity, &Health, Option<&RemoteFighter>, Option<&MissionNpc>),
        (Without<LocalFighter>, With<Transform>, With<MotionState>),
    >,
    mut lock: ResMut<LockOnController>,
) {
    let eligible = net
        .as_deref()
        .is_some_and(|net| net.status == NetStatus::InRoom && !net.battle_pending)
        && request.as_deref().is_some_and(|request| {
            matches!(
                meteor_network::modes::mode_rule_id(&request.mode),
                Some(4 | 5)
            )
        })
        && local.single().is_ok_and(|health| health.current <= 0.0);
    if !eligible {
        *state = SpectatorState::default();
        return;
    }
    let net = net.as_deref().unwrap();
    let team = net.player_team(&net.player_id);
    let mut alive = Vec::new();
    for (entity, health, remote, npc) in &candidates {
        if health.current <= 0.0 {
            continue;
        }
        let (id, actor_team, name) = if let Some(remote) = remote {
            let member = net
                .members
                .iter()
                .find(|member| member.player_id == remote.player_id);
            if member.is_some_and(|member| !member.connected) {
                continue;
            }
            (
                remote.player_id.clone(),
                net.player_team(&remote.player_id),
                member
                    .map(|member| member.name.clone())
                    .or_else(|| net.remote_actor_names.get(&remote.player_id).cloned())
                    .unwrap_or_else(|| remote.player_id.clone()),
            )
        } else if let Some(npc) = npc.filter(|npc| npc.in_scene_roster) {
            (format!("npc-{}", npc.id), Some(npc.team), npc.name.clone())
        } else {
            continue;
        };
        if team.is_some() && actor_team == team {
            alive.push((id, entity, name));
        }
    }
    alive.sort_by(|a, b| a.0.cmp(&b.0));
    let current = alive
        .iter()
        .position(|(_, entity, _)| Some(*entity) == state.target);
    let input = state.active && !debug.is_some_and(|debug| debug.captures_input());
    let next = input && keymap.attack.just_pressed(&keys, &mouse);
    let previous = input && keymap.guard.just_pressed(&keys, &mouse);
    let index = current.map_or(0, |index| {
        if next {
            (index + 1) % alive.len()
        } else if previous {
            (index + alive.len() - 1) % alive.len()
        } else {
            index
        }
    });
    state.active = true;
    state.target = alive.get(index).map(|(_, entity, _)| *entity);
    state.name = alive
        .get(index)
        .map(|(_, _, name)| name.clone())
        .unwrap_or_default();

    lock.system.clear();
    lock.lock_entity = None;
    lock.track_entity = None;
}

#[derive(Component)]
pub(super) struct SpectatorHint;

pub(super) fn update_hint(
    mut commands: Commands,
    state: Res<SpectatorState>,
    keymap: Res<Keymap>,
    font: Option<Res<crate::plugins::launcher_ui::UiFont>>,
    mut hints: Query<(Entity, &mut Text), With<SpectatorHint>>,
) {
    if !state.active {
        for (entity, _) in &hints {
            commands.entity(entity).despawn();
        }
        return;
    }
    let label = if state.target.is_some() {
        format!(
            "观战：{}  ·  {} / {} 切换队友  ·  移动视角自由环绕",
            state.name,
            keymap.attack.label(),
            keymap.guard.label()
        )
    } else {
        "没有存活队友，等待下一回合".into()
    };
    if let Ok((_, mut text)) = hints.single_mut() {
        if text.0 != label {
            text.0 = label;
        }
    } else {
        commands.spawn((
            SpectatorHint,
            Text::new(label),
            TextFont {
                font: font.map(|font| font.0.clone()).unwrap_or_default().into(),
                font_size: 18.0.into(),
                ..default()
            },
            TextColor(Color::WHITE),
            TextLayout::justify(Justify::Center),
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Percent(22.0),
                left: Val::Px(0.0),
                width: Val::Percent(100.0),
                ..default()
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        apply_follow_camera_input, follow_fighter_camera, CameraMode, CameraRecentreRequest,
        GameplayCamera, ThirdPersonState, LOOK_HEIGHT,
    };
    use super::*;
    use crate::components::fighter::SmState;

    #[test]
    fn spectator_follows_only_living_teammates_cycles_orbits_and_returns_on_revive() {
        for mode in ["assassin", "deathmatch"] {
            let mut time: Time = Time::default();
            time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
            let mut app = App::new();
            app.insert_resource(time)
                .init_resource::<SpectatorState>()
                .init_resource::<ButtonInput<KeyCode>>()
                .init_resource::<ButtonInput<MouseButton>>()
                .init_resource::<Keymap>()
                .init_resource::<crate::app_state::Settings>()
                .init_resource::<LockOnController>()
                .init_resource::<CameraMode>()
                .init_resource::<CameraRecentreRequest>()
                .init_resource::<crate::plugins::stage_collider::StageCollider>()
                .init_resource::<Messages<bevy::input::mouse::MouseMotion>>()
                .init_resource::<Messages<bevy::input::mouse::MouseWheel>>()
                .insert_resource(LaunchRequest {
                    mode: mode.into(),
                    ..default()
                })
                .insert_resource(NetState {
                    status: NetStatus::InRoom,
                    player_id: "local".into(),
                    remote_actor_teams: [
                        ("local".into(), 1),
                        ("a".into(), 1),
                        ("b".into(), 1),
                        ("enemy".into(), 2),
                        ("dead".into(), 1),
                    ]
                    .into_iter()
                    .collect(),
                    ..default()
                })
                .add_systems(
                    Update,
                    (
                        update_spectator,
                        apply_follow_camera_input,
                        follow_fighter_camera,
                    )
                        .chain(),
                );
            let local = app
                .world_mut()
                .spawn((
                    LocalFighter,
                    Health {
                        current: 0.0,
                        max: 100.0,
                    },
                    Transform::default(),
                    MotionState {
                        movement_mode: meteor_core::MovementMode::Idle,
                        velocity: Vec3::ZERO,
                        facing: 0.0,
                    },
                    SmState::default(),
                ))
                .id();
            let mut spawn = |id: &str, x, hp| {
                app.world_mut()
                    .spawn((
                        RemoteFighter {
                            player_id: id.into(),
                        },
                        Health {
                            current: hp,
                            max: 100.0,
                        },
                        Transform::from_xyz(x, 0.0, 0.0),
                        MotionState {
                            movement_mode: meteor_core::MovementMode::Idle,
                            velocity: Vec3::ZERO,
                            facing: 0.0,
                        },
                    ))
                    .id()
            };
            let a = spawn("a", 50.0, 100.0);
            let b = spawn("b", 100.0, 100.0);
            spawn("enemy", -50.0, 100.0);
            spawn("dead", -100.0, 0.0);
            let camera = app
                .world_mut()
                .spawn((
                    GameplayCamera,
                    Transform::default(),
                    Projection::Perspective(PerspectiveProjection::default()),
                    ThirdPersonState::default(),
                ))
                .id();
            app.world_mut()
                .resource_mut::<LockOnController>()
                .lock_entity = Some(b);
            app.update();
            assert_eq!(app.world().resource::<SpectatorState>().target, Some(a));
            assert_eq!(
                app.world().get::<ThirdPersonState>(camera).unwrap().target,
                Vec3::new(50.0, LOOK_HEIGHT, 0.0)
            );
            assert!(app
                .world()
                .resource::<LockOnController>()
                .lock_entity
                .is_none());

            for (button, expected) in [
                (MouseButton::Left, b),
                (MouseButton::Right, a),
                (MouseButton::Right, b),
                (MouseButton::Left, a),
            ] {
                app.world_mut()
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .reset_all();
                app.world_mut()
                    .resource_mut::<ButtonInput<MouseButton>>()
                    .press(button);
                app.update();
                assert_eq!(
                    app.world().resource::<SpectatorState>().target,
                    Some(expected)
                );
            }

            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .reset_all();
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .press(MouseButton::Left);
            app.world_mut()
                .resource_mut::<ButtonInput<MouseButton>>()
                .clear();
            app.world_mut()
                .resource_mut::<Messages<bevy::input::mouse::MouseMotion>>()
                .write(bevy::input::mouse::MouseMotion {
                    delta: Vec2::new(100.0, 10.0),
                });
            app.update();
            assert_ne!(
                app.world().get::<ThirdPersonState>(camera).unwrap().yaw,
                0.0
            );
            for entity in [local, a, b] {
                assert_eq!(app.world().get::<MotionState>(entity).unwrap().facing, 0.0);
            }
            app.world_mut().get_mut::<Health>(a).unwrap().current = 0.0;
            app.update();
            assert_eq!(app.world().resource::<SpectatorState>().target, Some(b));
            app.world_mut().despawn(b);
            let eye = app.world().get::<Transform>(camera).unwrap().translation;
            app.update();
            assert!(app.world().resource::<SpectatorState>().active);
            assert_eq!(app.world().resource::<SpectatorState>().target, None);
            assert_eq!(
                app.world().get::<Transform>(camera).unwrap().translation,
                eye
            );
            app.world_mut().get_mut::<Health>(local).unwrap().current = 100.0;
            app.update();
            assert!(!app.world().resource::<SpectatorState>().active);
            assert_eq!(
                app.world()
                    .get::<ThirdPersonState>(camera)
                    .unwrap()
                    .target
                    .x,
                0.0
            );
            app.world_mut().resource_mut::<LaunchRequest>().mode = "escort".into();
            app.world_mut().get_mut::<Health>(local).unwrap().current = 0.0;
            app.update();
            assert!(!app.world().resource::<SpectatorState>().active);
        }
    }
}
