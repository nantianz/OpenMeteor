use crate::components::fighter::{Fighter, Health, MotionState};
use bevy::prelude::*;
use meteor_physics::{character_contact::CHARACTER_CONTACT_RADIUS, collision::BODY_BOX_Z_HIGH};

pub(super) const BODY_RADIUS: f32 = CHARACTER_CONTACT_RADIUS;
pub(super) const BODY_HEIGHT: f32 = BODY_BOX_Z_HIGH + BODY_RADIUS * 0.5;

pub(super) type SceneActors<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        Option<&'static MotionState>,
        Option<&'static Health>,
    ),
    With<Fighter>,
>;

pub(super) struct SceneActor {
    pub entity: Entity,
    pub feet: Vec3,
    pub velocity: Vec2,
}

pub(super) fn live_actors<'a>(actors: &'a SceneActors) -> impl Iterator<Item = SceneActor> + 'a {
    actors
        .iter()
        .filter_map(|(entity, transform, motion, health)| {
            let feet = transform.translation();
            if !feet.is_finite() || health.is_some_and(|health| health.current <= 0.0) {
                return None;
            }
            let velocity = motion
                .map(|motion| motion.velocity.xz())
                .filter(|velocity| velocity.is_finite())
                .unwrap_or_default();
            Some(SceneActor {
                entity,
                feet,
                velocity,
            })
        })
}

#[cfg(test)]
pub(super) fn test_actor(feet: Vec3, velocity: Vec3) -> impl Bundle {
    (
        Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 0],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        },
        MotionState {
            movement_mode: meteor_core::MovementMode::Run,
            velocity,
            facing: 0.0,
        },
        Transform::from_translation(feet),
        GlobalTransform::from_translation(feet),
    )
}
