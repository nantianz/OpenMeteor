use bevy_material::labels::DrawFunctionId;

use crate::render_phase::{PhaseItem, TrackedRenderPass};
use bevy_app::{App, SubApp};
use bevy_ecs::{
    entity::Entity,
    query::{QueryEntityError, QueryState, ROQueryItem, ReadOnlyQueryData},
    resource::Resource,
    system::{ReadOnlySystemParam, SystemParam, SystemParamItem, SystemState},
    world::World,
};
use bevy_utils::TypeIdMap;
use core::{any::TypeId, fmt::Debug};
use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use thiserror::Error;
use variadics_please::all_tuples;

pub trait Draw<P: PhaseItem>: Send + Sync + 'static {

    #[expect(
        unused_variables,
        reason = "The parameters here are intentionally unused by the default implementation; however, putting underscores here will result in the underscores being copied by rust-analyzer's tab completion."
    )]
    fn prepare(&mut self, world: &'_ World) {}

    fn draw<'w>(
        &mut self,
        world: &'w World,
        pass: &mut TrackedRenderPass<'w>,
        view: Entity,
        item: &P,
    ) -> Result<(), DrawError>;
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum DrawError {
    #[error("Failed to execute render command {0:?}")]
    RenderCommandFailure(&'static str),
    #[error("Failed to get execute view query")]
    InvalidViewQuery,
    #[error("View entity not found")]
    ViewEntityNotFound,
}

pub struct DrawFunctionsInternal<P: PhaseItem> {
    pub draw_functions: Vec<Box<dyn Draw<P>>>,
    pub indices: TypeIdMap<DrawFunctionId>,
}

impl<P: PhaseItem> DrawFunctionsInternal<P> {

    pub fn prepare(&mut self, world: &World) {
        for function in &mut self.draw_functions {
            function.prepare(world);
        }
    }

    pub fn add<T: Draw<P>>(&mut self, draw_function: T) -> DrawFunctionId {
        self.add_with::<T, T>(draw_function)
    }

    pub fn add_with<T: 'static, D: Draw<P>>(&mut self, draw_function: D) -> DrawFunctionId {
        let id = DrawFunctionId(self.draw_functions.len().try_into().unwrap());
        self.draw_functions.push(Box::new(draw_function));
        self.indices.insert(TypeId::of::<T>(), id);
        id
    }

    pub fn get_mut(&mut self, id: DrawFunctionId) -> Option<&mut dyn Draw<P>> {
        self.draw_functions.get_mut(id.0 as usize).map(|f| &mut **f)
    }

    pub fn get_id<T: 'static>(&self) -> Option<DrawFunctionId> {
        self.indices.get(&TypeId::of::<T>()).copied()
    }

    pub fn id<T: 'static>(&self) -> DrawFunctionId {
        self.get_id::<T>().unwrap_or_else(|| {
            panic!(
                "Draw function {} not found for {}",
                core::any::type_name::<T>(),
                core::any::type_name::<P>()
            )
        })
    }
}

#[derive(Resource)]
pub struct DrawFunctions<P: PhaseItem> {
    internal: RwLock<DrawFunctionsInternal<P>>,
}

impl<P: PhaseItem> Default for DrawFunctions<P> {
    fn default() -> Self {
        Self {
            internal: RwLock::new(DrawFunctionsInternal {
                draw_functions: Vec::new(),
                indices: Default::default(),
            }),
        }
    }
}

impl<P: PhaseItem> DrawFunctions<P> {

    pub fn read(&self) -> RwLockReadGuard<'_, DrawFunctionsInternal<P>> {
        self.internal.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, DrawFunctionsInternal<P>> {
        self.internal
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

pub trait RenderCommand<P: PhaseItem> {

    type Param: SystemParam + 'static;

    type ViewQuery: ReadOnlyQueryData;

    type ItemQuery: ReadOnlyQueryData;

    fn render<'w>(
        item: &P,
        view: ROQueryItem<'w, '_, Self::ViewQuery>,
        entity: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
        param: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult;
}

#[derive(Debug)]
pub enum RenderCommandResult {
    Success,
    Skip,
    Failure(&'static str),
}

macro_rules! render_command_tuple_impl {
    ($(#[$meta:meta])* $(($name: ident, $view: ident, $entity: ident)),*) => {
        $(#[$meta])*
        impl<P: PhaseItem, $($name: RenderCommand<P>),*> RenderCommand<P> for ($($name,)*) {
            type Param = ($($name::Param,)*);
            type ViewQuery = ($($name::ViewQuery,)*);
            type ItemQuery = ($($name::ItemQuery,)*);

            #[expect(
                clippy::allow_attributes,
                reason = "We are in a macro; as such, `non_snake_case` may not always lint."
            )]
            #[allow(
                non_snake_case,
                reason = "Parameter and variable names are provided by the macro invocation, not by us."
            )]
            fn render<'w>(
                _item: &P,
                ($($view,)*): ROQueryItem<'w, '_, Self::ViewQuery>,
                maybe_entities: Option<ROQueryItem<'w, '_, Self::ItemQuery>>,
                ($($name,)*): SystemParamItem<'w, '_, Self::Param>,
                _pass: &mut TrackedRenderPass<'w>,
            ) -> RenderCommandResult {
                match maybe_entities {
                    None => {
                        $(
                            match $name::render(_item, $view, None, $name, _pass) {
                                RenderCommandResult::Skip => return RenderCommandResult::Skip,
                                RenderCommandResult::Failure(reason) => return RenderCommandResult::Failure(reason),
                                _ => {},
                            }
                        )*
                    }
                    Some(($($entity,)*)) => {
                        $(
                            match $name::render(_item, $view, Some($entity), $name, _pass) {
                                RenderCommandResult::Skip => return RenderCommandResult::Skip,
                                RenderCommandResult::Failure(reason) => return RenderCommandResult::Failure(reason),
                                _ => {},
                            }
                        )*
                    }
                }
                RenderCommandResult::Success
            }
        }
    };
}

all_tuples!(
    #[doc(fake_variadic)]
    render_command_tuple_impl,
    0,
    15,
    C,
    V,
    E
);

pub struct RenderCommandState<P: PhaseItem + 'static, C: RenderCommand<P>> {
    state: SystemState<C::Param>,
    view: QueryState<C::ViewQuery>,
    entity: QueryState<C::ItemQuery>,
}

impl<P: PhaseItem, C: RenderCommand<P>> RenderCommandState<P, C> {

    pub fn new(world: &mut World) -> Self {
        Self {
            state: SystemState::new(world),
            view: world.query(),
            entity: world.query(),
        }
    }
}

impl<P: PhaseItem, C: RenderCommand<P> + Send + Sync + 'static> Draw<P> for RenderCommandState<P, C>
where
    C::Param: ReadOnlySystemParam,
{

    fn prepare(&mut self, world: &'_ World) {
        self.view.update_archetypes(world);
        self.entity.update_archetypes(world);
    }

    fn draw<'w>(
        &mut self,
        world: &'w World,
        pass: &mut TrackedRenderPass<'w>,
        view: Entity,
        item: &P,
    ) -> Result<(), DrawError> {
        let param = self.state.get(world).unwrap();
        let view = match self.view.get_manual(world, view) {
            Ok(view) => view,
            Err(err) => match err {
                QueryEntityError::NotSpawned(_) => return Err(DrawError::ViewEntityNotFound),
                QueryEntityError::QueryDoesNotMatch(_, _)
                | QueryEntityError::AliasedMutability(_) => {
                    return Err(DrawError::InvalidViewQuery)
                }
            },
        };

        let entity = self.entity.get_manual(world, item.entity()).ok();
        match C::render(item, view, entity, param, pass) {
            RenderCommandResult::Success | RenderCommandResult::Skip => Ok(()),
            RenderCommandResult::Failure(reason) => Err(DrawError::RenderCommandFailure(reason)),
        }
    }
}

pub trait AddRenderCommand {

    fn add_render_command<P: PhaseItem, C: RenderCommand<P> + Send + Sync + 'static>(
        &mut self,
    ) -> &mut Self
    where
        C::Param: ReadOnlySystemParam;
}

impl AddRenderCommand for SubApp {
    fn add_render_command<P: PhaseItem, C: RenderCommand<P> + Send + Sync + 'static>(
        &mut self,
    ) -> &mut Self
    where
        C::Param: ReadOnlySystemParam,
    {
        let draw_function = RenderCommandState::<P, C>::new(self.world_mut());
        let draw_functions = self
            .world()
            .get_resource::<DrawFunctions<P>>()
            .unwrap_or_else(|| {
                panic!(
                    "DrawFunctions<{}> must be added to the world as a resource \
                     before adding render commands to it",
                    core::any::type_name::<P>(),
                );
            });
        draw_functions.write().add_with::<C, _>(draw_function);
        self
    }
}

impl AddRenderCommand for App {
    fn add_render_command<P: PhaseItem, C: RenderCommand<P> + Send + Sync + 'static>(
        &mut self,
    ) -> &mut Self
    where
        C::Param: ReadOnlySystemParam,
    {
        SubApp::add_render_command::<P, C>(self.main_mut());
        self
    }
}
