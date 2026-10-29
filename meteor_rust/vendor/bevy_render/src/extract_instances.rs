

use core::marker::PhantomData;

use bevy_app::{App, Plugin};
use bevy_camera::visibility::ViewVisibility;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    prelude::Entity,
    query::{QueryFilter, QueryItem, ReadOnlyQueryData},
    resource::Resource,
    system::{Query, ResMut},
};

use crate::sync_world::MainEntityHashMap;
use crate::{Extract, ExtractSchedule, RenderApp};

pub trait ExtractInstance: Send + Sync + Sized + 'static {

    type QueryData: ReadOnlyQueryData;

    type QueryFilter: QueryFilter;

    fn extract(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self>;
}

#[derive(Default)]
pub struct ExtractInstancesPlugin<EI>
where
    EI: ExtractInstance,
{
    only_extract_visible: bool,
    marker: PhantomData<fn() -> EI>,
}

#[derive(Resource, Deref, DerefMut)]
pub struct ExtractedInstances<EI>(MainEntityHashMap<EI>)
where
    EI: ExtractInstance;

impl<EI> Default for ExtractedInstances<EI>
where
    EI: ExtractInstance,
{
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<EI> ExtractInstancesPlugin<EI>
where
    EI: ExtractInstance,
{

    pub fn new() -> Self {
        Self {
            only_extract_visible: false,
            marker: PhantomData,
        }
    }

    pub fn extract_visible() -> Self {
        Self {
            only_extract_visible: true,
            marker: PhantomData,
        }
    }
}

impl<EI> Plugin for ExtractInstancesPlugin<EI>
where
    EI: ExtractInstance,
{
    fn build(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.init_resource::<ExtractedInstances<EI>>();
            if self.only_extract_visible {
                render_app.add_systems(ExtractSchedule, extract_visible::<EI>);
            } else {
                render_app.add_systems(ExtractSchedule, extract_all::<EI>);
            }
        }
    }
}

fn extract_all<EI>(
    mut extracted_instances: ResMut<ExtractedInstances<EI>>,
    query: Extract<Query<(Entity, EI::QueryData), EI::QueryFilter>>,
) where
    EI: ExtractInstance,
{
    extracted_instances.clear();
    for (entity, other) in &query {
        if let Some(extract_instance) = EI::extract(other) {
            extracted_instances.insert(entity.into(), extract_instance);
        }
    }
}

fn extract_visible<EI>(
    mut extracted_instances: ResMut<ExtractedInstances<EI>>,
    query: Extract<Query<(Entity, &ViewVisibility, EI::QueryData), EI::QueryFilter>>,
) where
    EI: ExtractInstance,
{
    extracted_instances.clear();
    for (entity, view_visibility, other) in &query {
        if view_visibility.get()
            && let Some(extract_instance) = EI::extract(other)
        {
            extracted_instances.insert(entity.into(), extract_instance);
        }
    }
}
