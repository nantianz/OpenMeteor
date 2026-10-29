use crate::{
    sync_world::{despawn_temporary_render_entities, entity_sync_system, SyncWorldPlugin},
    Render, RenderApp, RenderSystems,
};
use bevy_app::{App, Plugin, SubApp};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    resource::Resource,
    schedule::{IntoScheduleConfigs, Schedule, ScheduleBuildSettings, ScheduleLabel, Schedules},
    world::{Mut, World},
};
use bevy_utils::default;

pub struct ExtractPlugin {

    pub pre_extract: fn(&mut World, &mut World),
}

impl Default for ExtractPlugin {
    fn default() -> Self {
        Self {
            pre_extract: |_, _| {},
        }
    }
}

impl Plugin for ExtractPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SyncWorldPlugin);
        app.init_resource::<ScratchMainWorld>();

        let mut render_app = SubApp::new();

        let mut extract_schedule = Schedule::new(ExtractSchedule);

        extract_schedule.set_build_settings(ScheduleBuildSettings {
            auto_insert_apply_deferred: false,
            ..default()
        });
        extract_schedule.set_apply_final_deferred(false);

        render_app
            .add_schedule(Render::base_schedule())
            .add_schedule(extract_schedule)
            .allow_ambiguous_resource::<MainWorld>()
            .add_systems(
                Render,
                (

                    apply_extract_commands.in_set(RenderSystems::ExtractCommands),
                    despawn_temporary_render_entities.in_set(RenderSystems::PostCleanup),
                ),
            );

        let pre_extract = self.pre_extract;
        render_app.set_extract(move |main_world, render_world| {
            pre_extract(main_world, render_world);

            {
                #[cfg(feature = "trace")]
                let _stage_span = bevy_log::info_span!("entity_sync").entered();
                entity_sync_system(main_world, render_world);
            }

            extract(main_world, render_world);
        });

        app.insert_sub_app(RenderApp, render_app);
    }
}

#[derive(ScheduleLabel, PartialEq, Eq, Debug, Clone, Hash, Default)]
pub struct ExtractSchedule;

fn apply_extract_commands(render_world: &mut World) {
    render_world.resource_scope(|render_world, mut schedules: Mut<Schedules>| {
        schedules
            .get_mut(ExtractSchedule)
            .unwrap()
            .apply_deferred(render_world);
    });
}

#[derive(Resource, Default, Deref, DerefMut)]
pub struct MainWorld(World);

#[derive(Resource, Default)]
struct ScratchMainWorld(World);

pub fn extract(main_world: &mut World, render_world: &mut World) {

    let scratch_world = main_world.remove_resource::<ScratchMainWorld>().unwrap();
    let inserted_world = core::mem::replace(main_world, scratch_world.0);
    render_world.insert_resource(MainWorld(inserted_world));
    render_world.run_schedule(ExtractSchedule);

    let inserted_world = render_world.remove_resource::<MainWorld>().unwrap();
    let scratch_world = core::mem::replace(main_world, inserted_world.0);
    main_world.insert_resource(ScratchMainWorld(scratch_world));
}

#[cfg(test)]
mod test {
    use bevy_app::{App, Startup};
    use bevy_ecs::{prelude::*, schedule::ScheduleLabel};

    use crate::{
        extract_component::{ExtractComponent, ExtractComponentPlugin},
        extract_plugin::ExtractPlugin,
        sync_component::SyncComponent,
        sync_world::MainEntity,
        Render, RenderApp,
    };

    #[derive(Component, Clone, Debug)]
    struct RenderComponent;

    #[derive(Component, Clone, Debug)]
    struct RenderComponentExtra;

    #[derive(Component, Clone, Debug, ExtractComponent)]
    struct RenderComponentSeparate;

    #[derive(Component, Clone, Debug)]
    struct RenderComponentNoExtract;

    impl SyncComponent for RenderComponent {
        type Target = (RenderComponent, RenderComponentExtra);
    }

    impl ExtractComponent for RenderComponent {
        type QueryData = &'static Self;
        type QueryFilter = ();
        type Out = (RenderComponent, RenderComponentExtra);

        fn extract_component(
            _item: bevy_ecs::query::QueryItem<'_, '_, Self::QueryData>,
        ) -> Option<Self::Out> {
            Some((RenderComponent, RenderComponentExtra))
        }
    }

    #[test]
    fn extraction_works() {
        let mut app = App::new();

        app.add_plugins(ExtractPlugin::default());
        app.add_plugins(ExtractComponentPlugin::<RenderComponent>::default());
        app.add_plugins(ExtractComponentPlugin::<RenderComponentSeparate>::default());
        app.add_systems(Startup, |mut commands: Commands| {
            commands.spawn((RenderComponent, RenderComponentSeparate));
        });

        let render_app = app.get_sub_app_mut(RenderApp).unwrap();

        render_app.update_schedule = Some(Render.intern());

        render_app.world_mut().add_observer(
            |event: On<Add, (RenderComponent, RenderComponentExtra)>, mut commands: Commands| {

                commands
                    .entity(event.entity)
                    .insert(RenderComponentNoExtract);
            },
        );

        app.update();

        {
            let render_app = app.get_sub_app_mut(RenderApp).unwrap();
            render_app
                .world_mut()
                .run_system_cached(
                    |entity: Single<(
                        &MainEntity,
                        Option<&RenderComponent>,
                        Option<&RenderComponentExtra>,
                        Option<&RenderComponentSeparate>,
                        Option<&RenderComponentNoExtract>,
                    )>| {
                        assert!(entity.1.is_some());
                        assert!(entity.2.is_some());
                        assert!(entity.3.is_some());
                        assert!(entity.4.is_some());
                    },
                )
                .unwrap();
        }

        app.world_mut()
            .run_system_cached(
                |mut commands: Commands, query: Query<Entity, With<RenderComponent>>| {
                    for entity in query {
                        commands.entity(entity).remove::<RenderComponent>();
                    }
                },
            )
            .unwrap();

        app.update();

        {
            let render_app = app.get_sub_app_mut(RenderApp).unwrap();
            render_app
                .world_mut()
                .run_system_cached(
                    |entity: Single<(
                        &MainEntity,
                        Option<&RenderComponent>,
                        Option<&RenderComponentExtra>,
                        Option<&RenderComponentSeparate>,
                        Option<&RenderComponentNoExtract>,
                    )>| {
                        assert!(entity.1.is_none());
                        assert!(entity.2.is_none());
                        assert!(entity.3.is_some());
                        assert!(entity.4.is_some());
                    },
                )
                .unwrap();
        }
    }
}
