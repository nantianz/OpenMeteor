use core::{any::TypeId, mem};

use bevy_ecs::{
    component::Component,
    entity::Entity,
    prelude::ReflectComponent,
    query::{Changed, Or, With},
    system::{
        lifetimeless::{Read, SQuery},
        Local, Query, SystemParam,
    },
};
#[cfg(feature = "trace")]
use bevy_log::info_span;
use bevy_platform::collections::{HashMap, HashSet};
use bevy_reflect::{prelude::ReflectDefault, Reflect};
use bevy_utils::TypeIdMap;

use crate::{
    sync_world::{MainEntity, MainEntityHashMap, RenderEntity},
    view::RetainedViewEntity,
    Extract,
};

mod range;
use bevy_camera::visibility::*;
pub use range::*;

#[derive(Clone, Component, Default, Debug)]
pub struct RenderVisibleEntities {

    pub classes: TypeIdMap<RenderVisibleEntitiesClass>,
}

#[derive(Clone, Component, Default, Debug, Reflect)]
#[reflect(Component, Default, Debug, Clone)]
pub struct RenderShadowMapVisibleEntities {

    #[reflect(ignore, clone)]
    pub subviews: HashMap<RetainedViewEntity, RenderVisibleEntities>,
}

#[derive(Clone, Debug, Default, Reflect)]
#[reflect(Debug, Default, Clone)]
pub struct RenderVisibleEntitiesClass {

    #[reflect(ignore, clone)]
    pub entities_cpu_culling: Vec<(Entity, MainEntity)>,

    pub entities_gpu_culling: MainEntityHashMap<Entity>,

    pub added_entities: Vec<(Entity, MainEntity)>,

    pub removed_entities: Vec<(Entity, MainEntity)>,
}

#[derive(Component, Clone, Default, Debug)]
pub struct RenderExtractedVisibleEntities {

    pub classes: TypeIdMap<RenderExtractedVisibleEntitiesClass>,
}

#[derive(Component, Default)]
pub struct RenderExtractedShadowMapVisibleEntities {

    pub subviews: HashMap<RetainedViewEntity, RenderExtractedVisibleEntities>,
}

#[derive(Clone, Default, Debug)]
pub struct RenderExtractedVisibleEntitiesClass {

    pub entities: Vec<(Entity, MainEntity)>,
}

impl RenderVisibleEntities {

    pub fn get<QF>(&self) -> Option<&RenderVisibleEntitiesClass>
    where
        QF: 'static,
    {
        self.classes.get(&TypeId::of::<QF>())
    }
}

impl RenderVisibleEntitiesClass {

    pub fn prepare_for_new_frame(&mut self) {
        self.added_entities.clear();
        self.removed_entities.clear();
    }

    pub fn update_cpu_culled_entities(
        &mut self,
        visible_mesh_entities_cpu_culling: &[(Entity, MainEntity)],
    ) {
        #[cfg(feature = "trace")]
        let _update_from = info_span!("update_from", name = "update_from").entered();

        let old_entities_cpu_culling = mem::take(&mut self.entities_cpu_culling);

        let mut old_entity_cpu_culling_iter = old_entities_cpu_culling.iter().peekable();
        {
            #[cfg(feature = "trace")]
            let _old_entity_cpu_culling_span =
                info_span!("old_entity_cpu_culling", name = "old_entity_cpu_culling").entered();
            for (render_entity, visible_main_entity) in visible_mesh_entities_cpu_culling {

                while old_entity_cpu_culling_iter
                    .peek()
                    .is_some_and(|(_, main_entity)| *main_entity < *visible_main_entity)
                {
                    self.removed_entities
                        .push(*old_entity_cpu_culling_iter.next().unwrap());
                }

                self.entities_cpu_culling
                    .push((*render_entity, *visible_main_entity));

                if old_entity_cpu_culling_iter
                    .peek()
                    .is_some_and(|&&(_, main_entity)| main_entity == *visible_main_entity)
                {
                    old_entity_cpu_culling_iter.next();
                } else {
                    self.added_entities
                        .push((*render_entity, *visible_main_entity));
                }
            }
        }

        {
            #[cfg(feature = "trace")]
            let _old_entity_cpu_culling_removal_span = info_span!(
                "old_entity_cpu_culling_removal",
                name = "old_entity_cpu_culling_removal"
            )
            .entered();
            self.removed_entities
                .extend(old_entity_cpu_culling_iter.copied());
        }
    }

    pub fn add_entity(&mut self, pair: (Entity, MainEntity)) {
        self.added_entities.push(pair);
    }

    pub fn added_entities(&self) -> &[(Entity, MainEntity)] {
        &self.added_entities
    }

    pub fn entity_pair_is_visible(&self, entity: Entity, main_entity: MainEntity) -> bool {
        self.entities_cpu_culling
            .binary_search(&(entity, main_entity))
            .is_ok()
            || self
                .entities_gpu_culling
                .get(&main_entity)
                .is_some_and(|that_entity| *that_entity == entity)
    }

    pub fn iter_visible<'a>(&'a self) -> impl Iterator<Item = (&'a Entity, &'a MainEntity)> {
        self.entities_cpu_culling
            .iter()
            .map(|(entity, main_entity)| (entity, main_entity))
            .chain(
                self.entities_gpu_culling
                    .iter()
                    .map(|(main_entity, entity)| (entity, main_entity)),
            )
    }

    pub fn sort_added_entities(&mut self) {
        self.added_entities
            .sort_unstable_by_key(|(_, main_entity)| *main_entity);
    }
}

#[derive(SystemParam)]
pub struct VisibilityExtractionSystemParam<'w, 's> {

    pub mapper: Extract<'w, 's, SQuery<Read<RenderEntity>>>,
}

pub type VisibilityExtractionNoCpuCullingChangedQuery = SQuery<
    (Entity, Read<VisibilityClass>, Read<InheritedVisibility>),
    (
        Or<(Changed<NoCpuCulling>, Changed<InheritedVisibility>)>,
        With<NoCpuCulling>,
    ),
>;

pub fn collect_visible_cpu_culled_entities(
    mut cameras: Query<(
        &mut RenderVisibleEntities,
        Option<&mut RenderExtractedVisibleEntities>,
    )>,
    mut lights: Query<(
        &mut RenderShadowMapVisibleEntities,
        Option<&mut RenderExtractedShadowMapVisibleEntities>,
    )>,
    mut visibility_classes: Local<HashSet<TypeId>>,
) {

    for (mut render_visible_entities, mut maybe_render_visible_entities_cpu_culling) in
        cameras.iter_mut()
    {
        let mut maybe_render_subview_visible_entities_cpu_culling =
            maybe_render_visible_entities_cpu_culling.as_deref_mut();
        collect_visible_cpu_culled_entities_for_subview(
            &mut render_visible_entities,
            &mut maybe_render_subview_visible_entities_cpu_culling,
            &mut visibility_classes,
        );
    }

    for (
        mut render_shadow_map_visible_entities,
        mut maybe_render_shadow_map_visible_entities_cpu_culling,
    ) in lights.iter_mut()
    {
        for (subview, render_visible_entities) in
            render_shadow_map_visible_entities.subviews.iter_mut()
        {
            let mut maybe_render_subview_visible_entities_cpu_culling =
                maybe_render_shadow_map_visible_entities_cpu_culling
                    .as_mut()
                    .and_then(|render_subview_visible_entities_cpu_culling| {
                        render_subview_visible_entities_cpu_culling
                            .subviews
                            .get_mut(subview)
                    });
            collect_visible_cpu_culled_entities_for_subview(
                render_visible_entities,
                &mut maybe_render_subview_visible_entities_cpu_culling,
                &mut visibility_classes,
            );
        }
    }
}

fn collect_visible_cpu_culled_entities_for_subview(
    render_visible_entities: &mut RenderVisibleEntities,
    maybe_render_subview_visible_entities: &mut Option<&mut RenderExtractedVisibleEntities>,
    visibility_classes: &mut HashSet<TypeId>,
) {

    visibility_classes.clear();
    visibility_classes.extend(render_visible_entities.classes.keys().copied());
    if let Some(ref mut render_subview_visible_entities) = *maybe_render_subview_visible_entities {
        visibility_classes.extend(render_subview_visible_entities.classes.keys().copied());
    }

    for visibility_class in visibility_classes.iter() {
        let entities = render_visible_entities
            .classes
            .entry(*visibility_class)
            .or_default();

        entities.prepare_for_new_frame();

        let Some(ref mut render_subview_visible_entities) = *maybe_render_subview_visible_entities
        else {
            continue;
        };
        let Some(render_view_entities) = render_subview_visible_entities
            .classes
            .get_mut(visibility_class)
        else {
            continue;
        };

        render_view_entities
            .entities
            .sort_unstable_by_key(|(_, main_entity)| *main_entity);

        entities.update_cpu_culled_entities(&render_view_entities.entities);
    }
}
