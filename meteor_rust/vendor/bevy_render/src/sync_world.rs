use bevy_app::Plugin;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::{ContainsEntity, Entity, EntityEquivalent, EntityHash},
    lifecycle::{Add, Remove},
    observer::On,
    query::With,
    reflect::ReflectComponent,
    resource::Resource,
    system::{Local, Query, ResMut, SystemState},
    world::{EntityWorldMut, Mut, World},
};
use bevy_platform::collections::{HashMap, HashSet};
use bevy_reflect::{std_traits::ReflectDefault, Reflect};

#[derive(Default)]
pub struct SyncWorldPlugin;

impl Plugin for SyncWorldPlugin {
    fn build(&self, app: &mut bevy_app::App) {
        app.init_resource::<PendingSyncEntity>();
        app.add_observer(
            |add: On<Add, SyncToRenderWorld>, mut pending: ResMut<PendingSyncEntity>| {
                pending.push(EntityRecord::Added(add.entity));
            },
        );
        app.add_observer(
            |remove: On<Remove, SyncToRenderWorld>,
             mut pending: ResMut<PendingSyncEntity>,
             query: Query<&RenderEntity>| {
                if let Ok(e) = query.get(remove.entity) {
                    pending.push(EntityRecord::Removed(*e));
                };
            },
        );
    }
}

#[derive(Component, Copy, Clone, Debug, Default, Reflect)]
#[reflect(Component, Default, Clone)]
#[component(storage = "SparseSet")]
pub struct SyncToRenderWorld;

#[derive(Component, Deref, Copy, Clone, Debug, Eq, Hash, PartialEq, Reflect)]
#[component(clone_behavior = Ignore)]
#[reflect(Component, Clone)]
pub struct RenderEntity(Entity);
impl RenderEntity {
    #[inline]
    pub fn id(&self) -> Entity {
        self.0
    }
}

impl From<Entity> for RenderEntity {
    fn from(entity: Entity) -> Self {
        RenderEntity(entity)
    }
}

impl ContainsEntity for RenderEntity {
    fn entity(&self) -> Entity {
        self.id()
    }
}

unsafe impl EntityEquivalent for RenderEntity {}

#[derive(Component, Deref, Copy, Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, Reflect)]
#[reflect(Component, Clone)]
pub struct MainEntity(Entity);
impl MainEntity {
    #[inline]
    pub fn id(&self) -> Entity {
        self.0
    }
}

impl From<Entity> for MainEntity {
    fn from(entity: Entity) -> Self {
        MainEntity(entity)
    }
}

impl ContainsEntity for MainEntity {
    fn entity(&self) -> Entity {
        self.id()
    }
}

unsafe impl EntityEquivalent for MainEntity {}

pub type MainEntityHashMap<V> = HashMap<MainEntity, V, EntityHash>;

pub type MainEntityHashSet = HashSet<MainEntity, EntityHash>;

#[derive(Component, Copy, Clone, Debug, Default, Reflect)]
#[reflect(Component, Default, Clone)]
pub struct TemporaryRenderEntity;

#[derive(Debug)]
pub(crate) enum EntityRecord {

    Added(Entity),

    Removed(RenderEntity),

    ComponentRemoved(Entity, fn(EntityWorldMut<'_>)),
}

#[derive(Resource, Default, Deref, DerefMut)]
pub(crate) struct PendingSyncEntity {
    records: Vec<EntityRecord>,
}

pub(crate) fn entity_sync_system(main_world: &mut World, render_world: &mut World) {
    main_world.resource_scope(|world, mut pending: Mut<PendingSyncEntity>| {

        for record in pending.drain(..) {
            match record {
                EntityRecord::Added(e) => {
                    if let Ok(mut main_entity) = world.get_entity_mut(e) {
                        match main_entity.entry::<RenderEntity>() {
                            bevy_ecs::world::ComponentEntry::Occupied(_) => {
                                panic!("Attempting to synchronize an entity that has already been synchronized!");
                            }
                            bevy_ecs::world::ComponentEntry::Vacant(entry) => {
                                let id = render_world.spawn(MainEntity(e)).id();

                                entry.insert(RenderEntity(id));
                            }
                        };
                    }
                }
                EntityRecord::Removed(render_entity) => {
                    if let Ok(ec) = render_world.get_entity_mut(render_entity.id()) {
                        ec.despawn();
                    };
                }
                EntityRecord::ComponentRemoved(main_entity, removal_function) => {
                    let Some(render_entity) = world.get::<RenderEntity>(main_entity) else {
                        continue;
                    };
                    if let Ok(render_world_entity) = render_world.get_entity_mut(render_entity.id()) {
                        removal_function(render_world_entity);
                    }
                },
            }
        }
    });
}

pub(crate) fn despawn_temporary_render_entities(
    world: &mut World,
    state: &mut SystemState<Query<Entity, With<TemporaryRenderEntity>>>,
    mut local: Local<Vec<Entity>>,
) {
    let query = state.get(world).unwrap();

    local.extend(query.iter());

    local.sort_unstable_by_key(|e| e.index());
    for e in local.drain(..).rev() {
        world.despawn(e);
    }
}

mod render_entities_world_query_impls {
    use super::{MainEntity, RenderEntity};

    use bevy_ecs::{
        archetype::Archetype,
        change_detection::Tick,
        component::{ComponentId, Components},
        entity::Entity,
        query::{
            ArchetypeQueryData, FilteredAccess, IterQueryData, QueryData, ReadOnlyQueryData,
            ReleaseStateQueryData, SingleEntityQueryData, WorldQuery,
        },
        storage::{Table, TableRow},
        world::{unsafe_world_cell::UnsafeWorldCell, World},
    };

    unsafe impl WorldQuery for RenderEntity {
        type Fetch<'w> = <&'static RenderEntity as WorldQuery>::Fetch<'w>;
        type State = <&'static RenderEntity as WorldQuery>::State;

        fn shrink_fetch<'wlong: 'wshort, 'wshort>(
            fetch: Self::Fetch<'wlong>,
        ) -> Self::Fetch<'wshort> {
            fetch
        }

        #[inline]
        unsafe fn init_fetch<'w, 's>(
            world: UnsafeWorldCell<'w>,
            component_id: &'s ComponentId,
            last_run: Tick,
            this_run: Tick,
        ) -> Self::Fetch<'w> {

            unsafe {
                <&RenderEntity as WorldQuery>::init_fetch(world, component_id, last_run, this_run)
            }
        }

        const IS_DENSE: bool = <&'static RenderEntity as WorldQuery>::IS_DENSE;

        #[inline]
        unsafe fn set_archetype<'w, 's>(
            fetch: &mut Self::Fetch<'w>,
            component_id: &'s ComponentId,
            archetype: &'w Archetype,
            table: &'w Table,
        ) {

            unsafe {
                <&RenderEntity as WorldQuery>::set_archetype(fetch, component_id, archetype, table);
            }
        }

        #[inline]
        unsafe fn set_table<'w, 's>(
            fetch: &mut Self::Fetch<'w>,
            &component_id: &'s ComponentId,
            table: &'w Table,
        ) {

            unsafe { <&RenderEntity as WorldQuery>::set_table(fetch, &component_id, table) }
        }

        fn update_component_access(&component_id: &ComponentId, access: &mut FilteredAccess) {
            <&RenderEntity as WorldQuery>::update_component_access(&component_id, access);
        }

        fn init_state(world: &mut World) -> ComponentId {
            <&RenderEntity as WorldQuery>::init_state(world)
        }

        fn get_state(components: &Components) -> Option<Self::State> {
            <&RenderEntity as WorldQuery>::get_state(components)
        }

        fn matches_component_set(
            &state: &ComponentId,
            set_contains_id: &impl Fn(ComponentId) -> bool,
        ) -> bool {
            <&RenderEntity as WorldQuery>::matches_component_set(&state, set_contains_id)
        }
    }

    unsafe impl QueryData for RenderEntity {
        const IS_READ_ONLY: bool = true;
        const IS_ARCHETYPAL: bool = <&MainEntity as QueryData>::IS_ARCHETYPAL;
        type ReadOnly = RenderEntity;
        type Item<'w, 's> = Entity;

        fn shrink<'wlong: 'wshort, 'wshort, 's>(
            item: Self::Item<'wlong, 's>,
        ) -> Self::Item<'wshort, 's> {
            item
        }

        #[inline(always)]
        unsafe fn fetch<'w, 's>(
            state: &'s Self::State,
            fetch: &mut Self::Fetch<'w>,
            entity: Entity,
            table_row: TableRow,
        ) -> Option<Self::Item<'w, 's>> {

            let component =
                unsafe { <&RenderEntity as QueryData>::fetch(state, fetch, entity, table_row) };
            component.map(RenderEntity::id)
        }

        fn iter_access(
            state: &Self::State,
        ) -> impl Iterator<Item = bevy_ecs::query::EcsAccessType<'_>> {
            <&RenderEntity as QueryData>::iter_access(state)
        }
    }

    unsafe impl IterQueryData for RenderEntity {}

    unsafe impl ReadOnlyQueryData for RenderEntity {}

    unsafe impl SingleEntityQueryData for RenderEntity {}

    impl ArchetypeQueryData for RenderEntity {}

    impl ReleaseStateQueryData for RenderEntity {
        fn release_state<'w>(item: Self::Item<'w, '_>) -> Self::Item<'w, 'static> {
            item
        }
    }

    unsafe impl WorldQuery for MainEntity {
        type Fetch<'w> = <&'static MainEntity as WorldQuery>::Fetch<'w>;
        type State = <&'static MainEntity as WorldQuery>::State;

        fn shrink_fetch<'wlong: 'wshort, 'wshort>(
            fetch: Self::Fetch<'wlong>,
        ) -> Self::Fetch<'wshort> {
            fetch
        }

        #[inline]
        unsafe fn init_fetch<'w, 's>(
            world: UnsafeWorldCell<'w>,
            component_id: &'s ComponentId,
            last_run: Tick,
            this_run: Tick,
        ) -> Self::Fetch<'w> {

            unsafe {
                <&MainEntity as WorldQuery>::init_fetch(world, component_id, last_run, this_run)
            }
        }

        const IS_DENSE: bool = <&'static MainEntity as WorldQuery>::IS_DENSE;

        #[inline]
        unsafe fn set_archetype<'w, 's>(
            fetch: &mut Self::Fetch<'w>,
            component_id: &ComponentId,
            archetype: &'w Archetype,
            table: &'w Table,
        ) {

            unsafe {
                <&MainEntity as WorldQuery>::set_archetype(fetch, component_id, archetype, table);
            }
        }

        #[inline]
        unsafe fn set_table<'w, 's>(
            fetch: &mut Self::Fetch<'w>,
            &component_id: &'s ComponentId,
            table: &'w Table,
        ) {

            unsafe { <&MainEntity as WorldQuery>::set_table(fetch, &component_id, table) }
        }

        fn update_component_access(&component_id: &ComponentId, access: &mut FilteredAccess) {
            <&MainEntity as WorldQuery>::update_component_access(&component_id, access);
        }

        fn init_state(world: &mut World) -> ComponentId {
            <&MainEntity as WorldQuery>::init_state(world)
        }

        fn get_state(components: &Components) -> Option<Self::State> {
            <&MainEntity as WorldQuery>::get_state(components)
        }

        fn matches_component_set(
            &state: &ComponentId,
            set_contains_id: &impl Fn(ComponentId) -> bool,
        ) -> bool {
            <&MainEntity as WorldQuery>::matches_component_set(&state, set_contains_id)
        }
    }

    unsafe impl QueryData for MainEntity {
        const IS_READ_ONLY: bool = true;
        const IS_ARCHETYPAL: bool = <&MainEntity as QueryData>::IS_ARCHETYPAL;
        type ReadOnly = MainEntity;
        type Item<'w, 's> = Entity;

        fn shrink<'wlong: 'wshort, 'wshort, 's>(
            item: Self::Item<'wlong, 's>,
        ) -> Self::Item<'wshort, 's> {
            item
        }

        #[inline(always)]
        unsafe fn fetch<'w, 's>(
            state: &'s Self::State,
            fetch: &mut Self::Fetch<'w>,
            entity: Entity,
            table_row: TableRow,
        ) -> Option<Self::Item<'w, 's>> {

            let component =
                unsafe { <&MainEntity as QueryData>::fetch(state, fetch, entity, table_row) };
            component.map(MainEntity::id)
        }

        fn iter_access(
            state: &Self::State,
        ) -> impl Iterator<Item = bevy_ecs::query::EcsAccessType<'_>> {
            <&MainEntity as QueryData>::iter_access(state)
        }
    }

    unsafe impl IterQueryData for MainEntity {}

    unsafe impl ReadOnlyQueryData for MainEntity {}

    unsafe impl SingleEntityQueryData for MainEntity {}

    impl ArchetypeQueryData for MainEntity {}

    impl ReleaseStateQueryData for MainEntity {
        fn release_state<'w>(item: Self::Item<'w, '_>) -> Self::Item<'w, 'static> {
            item
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy_ecs::{
        component::Component,
        entity::Entity,
        lifecycle::{Add, Remove},
        observer::On,
        query::With,
        system::{Query, ResMut},
        world::World,
    };

    use super::{
        entity_sync_system, EntityRecord, MainEntity, PendingSyncEntity, RenderEntity,
        SyncToRenderWorld,
    };

    #[derive(Component)]
    struct RenderDataComponent;

    #[test]
    fn sync_world() {
        let mut main_world = World::new();
        let mut render_world = World::new();
        main_world.init_resource::<PendingSyncEntity>();

        main_world.add_observer(
            |add: On<Add, SyncToRenderWorld>, mut pending: ResMut<PendingSyncEntity>| {
                pending.push(EntityRecord::Added(add.entity));
            },
        );
        main_world.add_observer(
            |remove: On<Remove, SyncToRenderWorld>,
             mut pending: ResMut<PendingSyncEntity>,
             query: Query<&RenderEntity>| {
                if let Ok(e) = query.get(remove.entity) {
                    pending.push(EntityRecord::Removed(*e));
                };
            },
        );

        for _ in 0..99 {
            main_world.spawn_empty();
        }

        let main_entity = main_world
            .spawn(RenderDataComponent)

            .insert(SyncToRenderWorld)
            .id();

        entity_sync_system(&mut main_world, &mut render_world);

        let mut q = render_world.query_filtered::<Entity, With<MainEntity>>();

        assert!(q.iter(&render_world).count() == 1);

        let render_entity = q.single(&render_world).unwrap();
        let render_entity_component = main_world.get::<RenderEntity>(main_entity).unwrap();

        assert!(render_entity_component.id() == render_entity);

        let main_entity_component = render_world
            .get::<MainEntity>(render_entity_component.id())
            .unwrap();

        assert!(main_entity_component.id() == main_entity);

        main_world.despawn(main_entity);

        entity_sync_system(&mut main_world, &mut render_world);

        assert!(q.iter(&render_world).count() == 0);
    }
}
