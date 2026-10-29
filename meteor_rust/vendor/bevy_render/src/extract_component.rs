use crate::{
    sync_component::{SyncComponent, SyncComponentPlugin},
    sync_world::RenderEntity,
    Extract, ExtractSchedule, RenderApp,
};
use bevy_app::{App, Plugin};
use bevy_camera::visibility::ViewVisibility;
use bevy_ecs::{
    bundle::NoBundleEffect,
    prelude::*,
    query::{QueryFilter, QueryItem, ReadOnlyQueryData},
};
use core::marker::PhantomData;

pub use crate::uniform::{ComponentUniforms, DynamicUniformIndex, UniformComponentPlugin};

pub use bevy_render_macros::ExtractComponent;

pub trait ExtractComponent<F = ()>: SyncComponent<F> {

    type QueryData: ReadOnlyQueryData;

    type QueryFilter: QueryFilter;

    type Out: Bundle<Effect: NoBundleEffect>;

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out>;
}

pub struct ExtractComponentPlugin<C, F = ()> {
    only_extract_visible: bool,
    marker: PhantomData<fn() -> (C, F)>,
}

impl<C, F> Default for ExtractComponentPlugin<C, F> {
    fn default() -> Self {
        Self {
            only_extract_visible: false,
            marker: PhantomData,
        }
    }
}

impl<C, F> ExtractComponentPlugin<C, F> {
    pub fn extract_visible() -> Self {
        Self {
            only_extract_visible: true,
            marker: PhantomData,
        }
    }
}

impl<C: ExtractComponent<F>, F: 'static + Send + Sync> Plugin for ExtractComponentPlugin<C, F> {
    fn build(&self, app: &mut App) {
        app.add_plugins(SyncComponentPlugin::<C, F>::default());

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            if self.only_extract_visible {
                render_app.add_systems(ExtractSchedule, extract_visible_components::<C, F>);
            } else {
                render_app.add_systems(ExtractSchedule, extract_components::<C, F>);
            }
        }
    }
}

fn extract_components<C: ExtractComponent<F>, F>(
    mut commands: Commands,
    mut previous_len: Local<usize>,
    query: Extract<Query<(RenderEntity, C::QueryData), C::QueryFilter>>,
) {
    let mut values = Vec::with_capacity(*previous_len);
    for (entity, query_item) in &query {
        if let Some(component) = C::extract_component(query_item) {
            values.push((entity, component));
        } else {
            commands.entity(entity).remove::<C::Target>();
        }
    }
    *previous_len = values.len();
    commands.try_insert_batch(values);
}

fn extract_visible_components<C: ExtractComponent<F>, F>(
    mut commands: Commands,
    mut previous_len: Local<usize>,
    query: Extract<Query<(RenderEntity, &ViewVisibility, C::QueryData), C::QueryFilter>>,
) {
    let mut values = Vec::with_capacity(*previous_len);
    for (entity, view_visibility, query_item) in &query {
        if view_visibility.get() {
            if let Some(component) = C::extract_component(query_item) {
                values.push((entity, component));
            } else {
                commands.entity(entity).remove::<C::Target>();
            }
        }
    }
    *previous_len = values.len();
    commands.try_insert_batch(values);
}
