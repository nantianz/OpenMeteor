use core::marker::PhantomData;

use bevy_app::{App, Plugin};
use bevy_ecs::{
    bundle::{Bundle, NoBundleEffect},
    component::Component,
};

use crate::sync_world::{EntityRecord, PendingSyncEntity, SyncToRenderWorld};

pub struct SyncComponentPlugin<C, F = ()>(PhantomData<(C, F)>);

impl<C: SyncComponent<F>, F> Default for SyncComponentPlugin<C, F> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

pub trait SyncComponent<F = ()>: Component {

    type Target: Bundle<Effect: NoBundleEffect>;

}

impl<C: SyncComponent<F>, F: Send + Sync + 'static> Plugin for SyncComponentPlugin<C, F> {
    fn build(&self, app: &mut App) {
        app.register_required_components::<C, SyncToRenderWorld>();

        app.world_mut()
            .register_component_hooks::<C>()
            .on_remove(|mut world, context| {
                let mut pending = world.resource_mut::<PendingSyncEntity>();
                pending.push(EntityRecord::ComponentRemoved(
                    context.entity,
                    |mut entity| {
                        entity.remove::<C::Target>();
                    },
                ));
            });
    }
}
