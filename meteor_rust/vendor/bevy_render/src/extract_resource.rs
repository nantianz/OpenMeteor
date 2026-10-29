use core::marker::PhantomData;

use bevy_app::{App, Plugin};
use bevy_ecs::{component::Mutable, prelude::*};
pub use bevy_render_macros::ExtractResource;
use bevy_utils::once;

use crate::{Extract, ExtractSchedule, RenderApp};

pub trait ExtractResource<F = ()>: Resource {
    type Source: Resource;

    fn extract_resource(source: &Self::Source) -> Self;
}

pub struct ExtractResourcePlugin<R: ExtractResource<F>, F = ()>(PhantomData<(R, F)>);

impl<R: ExtractResource<F>, F> Default for ExtractResourcePlugin<R, F> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<R: ExtractResource<F, Mutability = Mutable>, F: 'static + Send + Sync> Plugin
    for ExtractResourcePlugin<R, F>
{
    fn build(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(ExtractSchedule, extract_resource::<R, F>);
        } else {
            once!(bevy_log::error!(
                "Render app did not exist when trying to add `extract_resource` for <{}>.",
                core::any::type_name::<R>()
            ));
        }
    }
}

pub fn extract_resource<R: ExtractResource<F, Mutability = Mutable>, F>(
    mut commands: Commands,
    main_resource: Extract<Option<Res<R::Source>>>,
    target_resource: Option<ResMut<R>>,
) {
    if let Some(main_resource) = main_resource.as_ref() {
        if let Some(mut target_resource) = target_resource {
            if main_resource.is_changed() {
                *target_resource = R::extract_resource(main_resource);
            }
        } else {
            #[cfg(debug_assertions)]
            if !main_resource.is_added() {
                once!(bevy_log::warn!(
                    "Removing resource {} from render world not expected, adding using `Commands`.
                This may decrease performance",
                    core::any::type_name::<R>()
                ));
            }

            commands.insert_resource(R::extract_resource(main_resource));
        }
    }
}
