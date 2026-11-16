use bevy::prelude::*;

use crate::app_state::AppState;

pub(crate) fn scene_session_active(
    state: Option<Res<State<AppState>>>,
    net: Option<Res<super::super::network_plugin::NetState>>,
) -> bool {
    state.is_none_or(|state| {
        *state.get() == AppState::Playing
            || (matches!(state.get(), AppState::Paused | AppState::Settings)
                && net.is_some_and(|net| {
                    net.status == super::super::network_plugin::NetStatus::InRoom
                }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .init_state::<AppState>();
        app
    }

    fn transition(app: &mut App, state: AppState) {
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(state);
        app.update();
    }

    #[test]
    fn scene_simulation_continues_only_for_network_room_menus() {
        use super::super::super::network_plugin::{NetState, NetStatus};
        #[derive(Resource, Default)]
        struct Ticks(usize);
        let mut app = app();
        app.init_resource::<NetState>()
            .init_resource::<Ticks>()
            .add_systems(
                Update,
                (|mut ticks: ResMut<Ticks>| ticks.0 += 1).run_if(scene_session_active),
            );
        for online in [false, true] {
            app.world_mut().resource_mut::<NetState>().status = if online {
                NetStatus::InRoom
            } else {
                NetStatus::Disconnected
            };
            for state in [
                AppState::Playing,
                AppState::Paused,
                AppState::Settings,
                AppState::Menu,
                AppState::Result,
            ] {
                let before = app.world().resource::<Ticks>().0;
                transition(&mut app, state);
                let running = state == AppState::Playing
                    || (online && matches!(state, AppState::Paused | AppState::Settings));
                assert_eq!(
                    app.world().resource::<Ticks>().0 - before,
                    usize::from(running)
                );
            }
        }
    }
}
