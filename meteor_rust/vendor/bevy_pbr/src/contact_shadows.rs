

use bevy_app::{App, Plugin};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::Entity,
    query::{QueryItem, With},
    reflect::ReflectComponent,
    resource::Resource,
    schedule::IntoScheduleConfigs,
    system::{Commands, Query, Res, ResMut},
};
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use bevy_render::{
    extract_component::{ExtractComponent, ExtractComponentPlugin},
    render_resource::{DynamicUniformBuffer, ShaderType},
    renderer::{RenderDevice, RenderQueue},
    sync_component::SyncComponent,
    view::ExtractedView,
    GpuResourceAppExt, Render, RenderApp, RenderSystems,
};

pub struct ContactShadowsPlugin;

#[derive(Clone, Copy, Component, Reflect)]
#[reflect(Component, Default, Clone)]
#[require(bevy_core_pipeline::prepass::DepthPrepass)]
pub struct ContactShadows {

    pub linear_steps: u32,

    pub thickness: f32,

    pub length: f32,
}

impl Default for ContactShadows {
    fn default() -> Self {
        Self {
            linear_steps: 16,
            thickness: 0.1,
            length: 0.3,
        }
    }
}

#[derive(Clone, Copy, ShaderType, Default)]
pub struct ContactShadowsUniform {
    pub linear_steps: u32,
    pub thickness: f32,
    pub length: f32,
    #[cfg(feature = "webgl")]
    pub _padding: f32,
}

impl From<ContactShadows> for ContactShadowsUniform {
    fn from(settings: ContactShadows) -> Self {
        Self {
            linear_steps: settings.linear_steps,
            thickness: settings.thickness,
            length: settings.length,
            #[cfg(feature = "webgl")]
            _padding: 0.0,
        }
    }
}

impl SyncComponent for ContactShadows {
    type Target = (Self, ViewContactShadowsUniformOffset);
}

impl ExtractComponent for ContactShadows {
    type QueryData = &'static ContactShadows;
    type QueryFilter = ();
    type Out = Self;

    fn extract_component(settings: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        Some(*settings)
    }
}

#[derive(Resource, Default)]
pub struct ContactShadowsBuffer(pub DynamicUniformBuffer<ContactShadowsUniform>);

impl Plugin for ContactShadowsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<ContactShadows>()
            .add_plugins(ExtractComponentPlugin::<ContactShadows>::default());

        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };

        render_app
            .init_gpu_resource::<ContactShadowsBuffer>()
            .add_systems(
                Render,
                prepare_contact_shadows_settings.in_set(RenderSystems::PrepareResources),
            );
    }
}

fn prepare_contact_shadows_settings(
    mut commands: Commands,
    views: Query<(Entity, &ContactShadows), With<ExtractedView>>,
    mut contact_shadows_buffer: ResMut<ContactShadowsBuffer>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    let Some(mut writer) =
        contact_shadows_buffer
            .0
            .get_writer(views.iter().len(), &render_device, &render_queue)
    else {
        return;
    };
    for (entity, settings) in &views {
        let uniform = ContactShadowsUniform::from(*settings);
        let offset = writer.write(&uniform);
        commands
            .entity(entity)
            .insert(ViewContactShadowsUniformOffset(offset));
    }
}

#[derive(Component, Default, Deref, DerefMut)]
pub struct ViewContactShadowsUniformOffset(pub u32);
