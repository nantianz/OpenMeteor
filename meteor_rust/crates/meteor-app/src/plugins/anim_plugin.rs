use crate::components::fighter::Fighter;
use bevy::prelude::*;

pub struct AnimPlugin;

#[derive(Resource, Debug, Default)]
pub struct AnimationClock {
    pub frame: u64,
}

impl Plugin for AnimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimationClock>().add_systems(
            Update,
            advance_animation_clock.run_if(not(in_state(crate::app_state::AppState::Paused))),
        );
    }
}

fn advance_animation_clock(mut clock: ResMut<AnimationClock>, _fighters: Query<(), With<Fighter>>) {
    clock.frame = clock.frame.saturating_add(1);
}
