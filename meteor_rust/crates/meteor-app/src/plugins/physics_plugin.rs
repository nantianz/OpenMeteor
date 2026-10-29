use crate::components::fighter::{Fighter, MotionState};
use crate::plugins::stage_plugin::LocalFighter;
use bevy::prelude::*;

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            integrate_motion
                .in_set(crate::plugins::FighterFrameSet::Movement)
                .run_if(crate::plugins::simulation_frame),
        );
    }
}

type RemoteMotionQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static MotionState),
    (
        With<Fighter>,
        Without<LocalFighter>,
        Without<crate::plugins::combat_plugin::NpcFighter>,
        Without<crate::plugins::network_plugin::RemoteFighter>,
    ),
>;

fn integrate_motion(
    mut query: RemoteMotionQuery<'_, '_>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    for (mut transform, motion) in &mut query {
        transform.translation += motion.velocity * dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fighter() -> Fighter {
        Fighter {
            character_id: 0,
            player_slot: 1,
            weapon: 6,
            weapon_slots: [6, 0],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        }
    }

    fn motion() -> MotionState {
        MotionState {
            movement_mode: meteor_core::MovementMode::Run,
            velocity: Vec3::new(10.0, 0.0, 0.0),
            facing: 0.0,
        }
    }

    #[test]
    fn generic_motion_integrates_but_npc_and_remote_keep_their_authoritative_transform() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .insert_resource(crate::plugins::FrameClock {
                sim_dt: 0.1,
                ..default()
            })
            .add_systems(Update, integrate_motion);
        let generic = app
            .world_mut()
            .spawn((fighter(), motion(), Transform::default()))
            .id();
        let npc = app
            .world_mut()
            .spawn((
                fighter(),
                motion(),
                Transform::default(),
                crate::plugins::combat_plugin::NpcFighter,
            ))
            .id();
        let remote = app
            .world_mut()
            .spawn((
                fighter(),
                motion(),
                Transform::default(),
                crate::plugins::network_plugin::RemoteFighter {
                    player_id: "remote".into(),
                },
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<Transform>(generic).unwrap().translation.x,
            1.0
        );
        assert_eq!(
            app.world().get::<Transform>(npc).unwrap().translation.x,
            0.0
        );
        assert_eq!(
            app.world().get::<Transform>(remote).unwrap().translation.x,
            0.0
        );
    }
}
