use super::*;
use crate::components::fighter::WeaponCollisionGeometry;
use bevy::transform::helper::TransformHelper;

#[derive(Component, Default)]
pub(crate) struct CollisionSampleState {
    pub pending: bool,
}

#[derive(Component, Clone, Copy)]
pub(crate) struct SampledCollisionTransform(pub GlobalTransform);

pub(crate) fn capture_collision_transforms(
    mut commands: Commands,
    mut models: Query<(Entity, &FighterModel, &mut CollisionSampleState)>,
    skeletons: FighterSkeletonQuery,
    mut nodes: Query<(&GlobalTransform, Option<&mut SampledCollisionTransform>)>,
    weapons: Query<(Entity, &WeaponCollisionGeometry)>,
    transforms: TransformHelper,
) {
    for (model_entity, model, mut state) in &mut models {
        if !state.pending {
            continue;
        }
        for entity in model_bones(model_entity, &skeletons)
            .iter()
            .copied()
            .chain(model_dummies(model_entity, &skeletons).iter().copied())
            .chain(
                weapons
                    .iter()
                    .filter(|(_, part)| part.fighter == model.fighter)
                    .map(|(entity, _)| entity),
            )
        {
            if let Ok((propagated, sample)) = nodes.get_mut(entity) {
                let matrix = transforms
                    .compute_global_transform(entity)
                    .unwrap_or(*propagated);
                if let Some(mut sample) = sample {
                    sample.0 = matrix;
                } else {
                    commands
                        .entity(entity)
                        .insert(SampledCollisionTransform(matrix));
                }
            }
        }
        state.pending = false;
    }
}

#[cfg(test)]
pub(crate) fn refresh_collision_fixture(world: &mut World, fighter: Entity) {
    use bevy::ecs::system::RunSystemOnce;

    let mut models = world.query::<(&FighterModel, &mut CollisionSampleState)>();
    for (model, mut state) in models.iter_mut(world) {
        if model.fighter == fighter {
            state.pending = true;
        }
    }
    world.run_system_once(capture_collision_transforms).unwrap();
}
