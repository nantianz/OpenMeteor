use crate::MainWorld;
use bevy_ecs::{
    change_detection::Tick,
    prelude::*,
    query::FilteredAccessSet,
    system::{
        ReadOnlySystemParam, SystemMeta, SystemParam, SystemParamItem, SystemParamValidationError,
        SystemState,
    },
    world::unsafe_world_cell::UnsafeWorldCell,
};
use core::ops::{Deref, DerefMut};

pub struct Extract<'w, 's, P>
where
    P: ReadOnlySystemParam + 'static,
{
    item: SystemParamItem<'w, 's, P>,
}

#[doc(hidden)]
pub struct ExtractState<P: SystemParam + 'static> {
    state: SystemState<P>,
    main_world_state: <Res<'static, MainWorld> as SystemParam>::State,
}

unsafe impl<P> ReadOnlySystemParam for Extract<'_, '_, P> where P: ReadOnlySystemParam {}

unsafe impl<P> SystemParam for Extract<'_, '_, P>
where
    P: ReadOnlySystemParam,
{
    type State = ExtractState<P>;
    type Item<'w, 's> = Extract<'w, 's, P>;

    fn init_state(world: &mut World) -> Self::State {
        let mut main_world = world.resource_mut::<MainWorld>();
        ExtractState {
            state: SystemState::new(&mut main_world),
            main_world_state: Res::<MainWorld>::init_state(world),
        }
    }

    fn init_access(
        state: &Self::State,
        system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        Res::<MainWorld>::init_access(
            &state.main_world_state,
            system_meta,
            component_access_set,
            world,
        );
    }

    #[inline]
    unsafe fn get_param<'w, 's>(
        state: &'s mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell<'w>,
        change_tick: Tick,
    ) -> Result<Self::Item<'w, 's>, SystemParamValidationError> {

        let main_world = unsafe {
            Res::<MainWorld>::get_param(
                &mut state.main_world_state,
                system_meta,
                world,
                change_tick,
            )?
        };
        let item = state.state.get(main_world.into_inner())?;
        Ok(Extract { item })
    }
}

impl<'w, 's, P> Deref for Extract<'w, 's, P>
where
    P: ReadOnlySystemParam,
{
    type Target = SystemParamItem<'w, 's, P>;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.item
    }
}

impl<'w, 's, P> DerefMut for Extract<'w, 's, P>
where
    P: ReadOnlySystemParam,
{
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.item
    }
}

impl<'a, 'w, 's, P> IntoIterator for &'a Extract<'w, 's, P>
where
    P: ReadOnlySystemParam,
    &'a SystemParamItem<'w, 's, P>: IntoIterator,
{
    type Item = <&'a SystemParamItem<'w, 's, P> as IntoIterator>::Item;
    type IntoIter = <&'a SystemParamItem<'w, 's, P> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (&self.item).into_iter()
    }
}
