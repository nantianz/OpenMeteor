use core::mem;

use crate::{
    batching::gpu_preprocessing::{GpuPreprocessingMode, GpuPreprocessingSupport},
    extract_component::{ExtractComponent, ExtractComponentPlugin},
    extract_resource::{extract_resource, ExtractResource, ExtractResourcePlugin},
    render_asset::RenderAssets,
    render_resource::TextureView,
    sync_component::SyncComponent,
    sync_world::{MainEntity, MainEntityHashSet, RenderEntity, SyncToRenderWorld},
    texture::{GpuImage, ManualTextureViews},
    view::{
        ColorGrading, ExtractedView, ExtractedWindows, Msaa, NoIndirectDrawing,
        RenderExtractedVisibleEntities, RenderVisibleEntities, RenderVisibleEntitiesClass,
        RetainedViewEntity, ViewUniformOffset, VisibilityExtractionSystemParam,
    },
    Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
};

use bevy_app::{App, Plugin, PostStartup, PostUpdate};
use bevy_asset::{AssetEvent, AssetEventSystems, AssetId, Assets};
use bevy_camera::{
    primitives::Frustum,
    visibility::{self, RenderLayers, VisibleEntities},
    Camera, Camera2d, Camera3d, CameraMainTextureUsages, CameraOutputMode, CameraUpdateSystems,
    ClearColor, ClearColorConfig, CompositingSpace, Exposure, Hdr, ManualTextureViewHandle,
    MsaaWriteback, NormalizedRenderTarget, Projection, RenderTarget, RenderTargetInfo, Viewport,
};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    change_detection::DetectChanges,
    component::Component,
    entity::{ContainsEntity, Entity, EntityHashMap, EntityHashSet},
    error::BevyError,
    lifecycle::HookContext,
    message::MessageReader,
    prelude::With,
    query::{Has, QueryItem},
    reflect::ReflectComponent,
    resource::Resource,
    schedule::{InternedScheduleLabel, IntoScheduleConfigs, ScheduleLabel, SystemSet},
    system::{Commands, Query, Res, ResMut},
    world::DeferredWorld,
};
use bevy_image::Image;
use bevy_log::warn;
use bevy_log::warn_once;
use bevy_math::{uvec2, vec2, Mat4, URect, UVec2, UVec4, Vec2};
use bevy_platform::collections::{HashMap, HashSet};
use bevy_reflect::prelude::*;
use bevy_transform::components::GlobalTransform;
use bevy_window::{PrimaryWindow, Window, WindowCreated, WindowResized, WindowScaleFactorChanged};
use itertools::Either;
use wgpu::TextureFormat;

#[derive(Resource, Default, Deref, DerefMut)]
pub struct CameraMainPassTextureFormats(pub EntityHashMap<TextureFormat>);

#[derive(Default)]
pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<Camera, Msaa>()
            .register_required_components::<Camera, SyncToRenderWorld>()
            .register_required_components::<Camera3d, ColorGrading>()
            .register_required_components::<Camera3d, Exposure>()
            .add_plugins((
                ExtractResourcePlugin::<ClearColor>::default(),
                ExtractComponentPlugin::<CameraMainTextureUsages>::default(),
            ))
            .add_systems(PostStartup, camera_system.in_set(CameraUpdateSystems))
            .add_systems(
                PostUpdate,
                camera_system
                    .in_set(CameraUpdateSystems)
                    .before(AssetEventSystems)
                    .before(visibility::update_frusta),
            );
        app.world_mut()
            .register_component_hooks::<Camera>()
            .on_add(warn_on_no_render_graph);

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<CameraMainPassTextureFormats>()
                .init_resource::<SortedCameras>()
                .init_resource::<DirtySpecializations>()
                .init_resource::<DirtyWireframeSpecializations>()
                .allow_ambiguous_resource::<DirtySpecializations>()
                .allow_ambiguous_resource::<DirtyWireframeSpecializations>()
                .configure_sets(
                    ExtractSchedule,
                    (
                        DirtySpecializationSystems::Clear
                            .before(DirtySpecializationSystems::CheckForChanges),
                        DirtySpecializationSystems::CheckForChanges
                            .before(DirtySpecializationSystems::CheckForRemovals),
                    ),
                )
                .add_systems(
                    ExtractSchedule,
                    (
                        extract_cameras.after(extract_resource::<ManualTextureViews, ()>),
                        clear_dirty_specializations.in_set(DirtySpecializationSystems::Clear),
                        clear_dirty_wireframe_specializations
                            .in_set(DirtySpecializationSystems::Clear),
                        expire_specializations_for_views.in_set(RenderSystems::Cleanup),
                        expire_wireframe_specializations_for_views.in_set(RenderSystems::Cleanup),
                    ),
                )
                .add_systems(Render, sort_cameras.in_set(RenderSystems::CreateViews));
        }
    }
}

fn warn_on_no_render_graph(world: DeferredWorld, HookContext { entity, caller, .. }: HookContext) {
    if !world.entity(entity).contains::<CameraRenderGraph>() {
        warn!("{}Entity {entity} has a `Camera` component, but it doesn't have a render graph configured. Usually, adding a `Camera2d` or `Camera3d` component will work.
        However, you may instead need to enable `bevy_core_pipeline`, or may want to manually add a `CameraRenderGraph` component to create a custom render graph.", caller.map(|location|format!("{location}: ")).unwrap_or_default());
    }
}

impl ExtractResource for ClearColor {
    type Source = Self;

    fn extract_resource(source: &Self::Source) -> Self {
        source.clone()
    }
}

impl SyncComponent for CameraMainTextureUsages {
    type Target = Self;
}

impl ExtractComponent for CameraMainTextureUsages {
    type QueryData = &'static Self;
    type QueryFilter = ();
    type Out = Self;

    fn extract_component(item: QueryItem<Self::QueryData>) -> Option<Self::Out> {
        Some(*item)
    }
}

impl SyncComponent for Camera2d {
    type Target = Self;
}

impl ExtractComponent for Camera2d {
    type QueryData = &'static Self;
    type QueryFilter = With<Camera>;
    type Out = Self;

    fn extract_component(item: QueryItem<Self::QueryData>) -> Option<Self::Out> {
        Some(item.clone())
    }
}

impl SyncComponent for Camera3d {
    type Target = Self;
}

impl ExtractComponent for Camera3d {
    type QueryData = &'static Self;
    type QueryFilter = With<Camera>;
    type Out = Self;

    fn extract_component(item: QueryItem<Self::QueryData>) -> Option<Self::Out> {
        Some(item.clone())
    }
}

#[derive(Component, Debug, Deref, DerefMut, Reflect, Clone)]
#[reflect(opaque)]
#[reflect(Component, Debug, Clone)]
pub struct CameraRenderGraph(pub InternedScheduleLabel);

impl CameraRenderGraph {

    #[inline]
    pub fn new<T: ScheduleLabel>(schedule: T) -> Self {
        Self(schedule.intern())
    }

    #[inline]
    pub fn set<T: ScheduleLabel>(&mut self, schedule: T) {
        self.0 = schedule.intern();
    }
}

pub trait NormalizedRenderTargetExt {
    fn get_texture_view<'a>(
        &self,
        windows: &'a ExtractedWindows,
        images: &'a RenderAssets<GpuImage>,
        manual_texture_views: &'a ManualTextureViews,
    ) -> Option<&'a TextureView>;

    fn get_texture_view_format<'a>(
        &self,
        windows: &'a ExtractedWindows,
        images: &'a RenderAssets<GpuImage>,
        manual_texture_views: &'a ManualTextureViews,
    ) -> Option<TextureFormat>;

    fn get_render_target_info<'a>(
        &self,
        resolutions: impl IntoIterator<Item = (Entity, &'a Window)>,
        images: &Assets<Image>,
        manual_texture_views: &ManualTextureViews,
    ) -> Result<RenderTargetInfo, MissingRenderTargetInfoError>;

    fn is_changed(
        &self,
        changed_window_ids: &EntityHashSet,
        changed_image_handles: &HashSet<&AssetId<Image>>,
    ) -> bool;
}

impl NormalizedRenderTargetExt for NormalizedRenderTarget {
    fn get_texture_view<'a>(
        &self,
        windows: &'a ExtractedWindows,
        images: &'a RenderAssets<GpuImage>,
        manual_texture_views: &'a ManualTextureViews,
    ) -> Option<&'a TextureView> {
        match self {
            NormalizedRenderTarget::Window(window_ref) => windows
                .get(&window_ref.entity())
                .and_then(|window| window.swap_chain_texture_view.as_ref()),
            NormalizedRenderTarget::Image(image_target) => images
                .get(&image_target.handle)
                .map(|image| &image.texture_view),
            NormalizedRenderTarget::TextureView(id) => {
                manual_texture_views.get(id).map(|tex| &tex.texture_view)
            }
            NormalizedRenderTarget::None { .. } => None,
        }
    }

    fn get_texture_view_format<'a>(
        &self,
        windows: &'a ExtractedWindows,
        images: &'a RenderAssets<GpuImage>,
        manual_texture_views: &'a ManualTextureViews,
    ) -> Option<TextureFormat> {
        match self {
            NormalizedRenderTarget::Window(window_ref) => windows
                .get(&window_ref.entity())
                .and_then(|window| window.swap_chain_texture_view_format),
            NormalizedRenderTarget::Image(image_target) => {
                images.get(&image_target.handle).map(GpuImage::view_format)
            }
            NormalizedRenderTarget::TextureView(id) => {
                manual_texture_views.get(id).map(|tex| tex.view_format)
            }
            NormalizedRenderTarget::None { .. } => None,
        }
    }

    fn get_render_target_info<'a>(
        &self,
        resolutions: impl IntoIterator<Item = (Entity, &'a Window)>,
        images: &Assets<Image>,
        manual_texture_views: &ManualTextureViews,
    ) -> Result<RenderTargetInfo, MissingRenderTargetInfoError> {
        match self {
            NormalizedRenderTarget::Window(window_ref) => resolutions
                .into_iter()
                .find(|(entity, _)| *entity == window_ref.entity())
                .map(|(_, window)| RenderTargetInfo {
                    physical_size: window.physical_size(),
                    scale_factor: window.resolution.scale_factor(),
                })
                .ok_or(MissingRenderTargetInfoError::Window {
                    window: window_ref.entity(),
                }),
            NormalizedRenderTarget::Image(image_target) => images
                .get(&image_target.handle)
                .map(|image| RenderTargetInfo {
                    physical_size: image.size(),
                    scale_factor: image_target.scale_factor,
                })
                .ok_or(MissingRenderTargetInfoError::Image {
                    image: image_target.handle.id(),
                }),
            NormalizedRenderTarget::TextureView(id) => manual_texture_views
                .get(id)
                .map(|tex| RenderTargetInfo {
                    physical_size: tex.size,
                    scale_factor: 1.0,
                })
                .ok_or(MissingRenderTargetInfoError::TextureView { texture_view: *id }),
            NormalizedRenderTarget::None { width, height } => Ok(RenderTargetInfo {
                physical_size: uvec2(*width, *height),
                scale_factor: 1.0,
            }),
        }
    }

    fn is_changed(
        &self,
        changed_window_ids: &EntityHashSet,
        changed_image_handles: &HashSet<&AssetId<Image>>,
    ) -> bool {
        match self {
            NormalizedRenderTarget::Window(window_ref) => {
                changed_window_ids.contains(&window_ref.entity())
            }
            NormalizedRenderTarget::Image(image_target) => {
                changed_image_handles.contains(&image_target.handle.id())
            }
            NormalizedRenderTarget::TextureView(_) => true,
            NormalizedRenderTarget::None { .. } => false,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MissingRenderTargetInfoError {
    #[error("RenderTarget::Window missing ({window:?}): Make sure the provided entity has a Window component.")]
    Window { window: Entity },
    #[error("RenderTarget::Image missing ({image:?}): Make sure the Image's usages include RenderAssetUsages::MAIN_WORLD.")]
    Image { image: AssetId<Image> },
    #[error("RenderTarget::TextureView missing ({texture_view:?}): make sure the texture view handle was not removed.")]
    TextureView {
        texture_view: ManualTextureViewHandle,
    },
}

pub fn camera_system(
    mut window_resized_reader: MessageReader<WindowResized>,
    mut window_created_reader: MessageReader<WindowCreated>,
    mut window_scale_factor_changed_reader: MessageReader<WindowScaleFactorChanged>,
    mut image_asset_event_reader: MessageReader<AssetEvent<Image>>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    windows: Query<(Entity, &Window)>,
    images: Res<Assets<Image>>,
    manual_texture_views: Res<ManualTextureViews>,
    mut cameras: Query<(&mut Camera, &RenderTarget, &mut Projection)>,
) -> Result<(), BevyError> {
    let primary_window = primary_window.iter().next();

    let mut changed_window_ids = EntityHashSet::default();
    changed_window_ids.extend(window_created_reader.read().map(|event| event.window));
    changed_window_ids.extend(window_resized_reader.read().map(|event| event.window));
    let scale_factor_changed_window_ids: EntityHashSet = window_scale_factor_changed_reader
        .read()
        .map(|event| event.window)
        .collect();
    changed_window_ids.extend(scale_factor_changed_window_ids.clone());

    let changed_image_handles: HashSet<&AssetId<Image>> = image_asset_event_reader
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } | AssetEvent::Added { id } => Some(id),
            _ => None,
        })
        .collect();

    for (mut camera, render_target, mut camera_projection) in &mut cameras {
        let mut viewport_size = camera
            .viewport
            .as_ref()
            .map(|viewport| viewport.physical_size);

        if let Some(normalized_target) = render_target.normalize(primary_window)
            && (normalized_target.is_changed(&changed_window_ids, &changed_image_handles)
                || camera.is_added()
                || camera_projection.is_changed()
                || camera.computed.old_viewport_size != viewport_size
                || camera.computed.old_sub_camera_view != camera.sub_camera_view)
        {
            let new_computed_target_info = normalized_target.get_render_target_info(
                windows,
                &images,
                &manual_texture_views,
            )?;

            if normalized_target.is_changed(&scale_factor_changed_window_ids, &HashSet::default())
                && let Some(old_scale_factor) = camera
                    .computed
                    .target_info
                    .as_ref()
                    .map(|info| info.scale_factor)
            {
                let resize_factor = new_computed_target_info.scale_factor / old_scale_factor;
                if let Some(ref mut viewport) = camera.viewport {
                    let resize = |vec: UVec2| (vec.as_vec2() * resize_factor).as_uvec2();
                    viewport.physical_position = resize(viewport.physical_position);
                    viewport.physical_size = resize(viewport.physical_size);
                    viewport_size = Some(viewport.physical_size);
                }
            }

            if let Some(viewport) = &mut camera.viewport {
                viewport.clamp_to_size(new_computed_target_info.physical_size);
            }
            camera.computed.target_info = Some(new_computed_target_info);
            if let Some(size) = camera.logical_viewport_size()
                && size.x != 0.0
                && size.y != 0.0
            {
                camera_projection.update(size.x, size.y);
                camera.computed.clip_from_view = match &camera.sub_camera_view {
                    Some(sub_view) => camera_projection.get_clip_from_view_for_sub(sub_view),
                    None => camera_projection.get_clip_from_view(),
                }
            }
        }

        if camera.computed.old_viewport_size != viewport_size {
            camera.computed.old_viewport_size = viewport_size;
        }

        if camera.computed.old_sub_camera_view != camera.sub_camera_view {
            camera.computed.old_sub_camera_view = camera.sub_camera_view;
        }
    }
    Ok(())
}

#[derive(Component, Debug)]
#[require(RenderVisibleEntities)]
pub struct ExtractedCamera {
    pub target: Option<NormalizedRenderTarget>,
    pub physical_viewport_size: Option<UVec2>,
    pub physical_target_size: Option<UVec2>,
    pub viewport: Option<Viewport>,
    pub schedule: InternedScheduleLabel,
    pub order: isize,
    pub output_mode: CameraOutputMode,
    pub msaa_writeback: MsaaWriteback,
    pub clear_color: ClearColorConfig,
    pub sorted_camera_index_for_target: usize,
    pub exposure: f32,
    pub hdr: bool,

    pub compositing_space: Option<CompositingSpace>,
}

pub fn extract_cameras(
    mut commands: Commands,
    mut main_pass_formats: ResMut<CameraMainPassTextureFormats>,
    query: Extract<
        Query<(
            Entity,
            RenderEntity,
            &Camera,
            &RenderTarget,
            &CameraRenderGraph,
            &GlobalTransform,
            &VisibleEntities,
            &Frustum,
            (
                Has<Hdr>,
                Option<&CompositingSpace>,
                Option<&ColorGrading>,
                Option<&Exposure>,
                Option<&TemporalJitter>,
                Option<&MipBias>,
                Option<&RenderLayers>,
                Option<&Projection>,
                Has<NoIndirectDrawing>,
            ),
        )>,
    >,
    primary_window: Extract<Query<Entity, With<PrimaryWindow>>>,
    extracted_windows: Res<ExtractedWindows>,
    manual_texture_views: Res<ManualTextureViews>,
    images: Res<RenderAssets<GpuImage>>,
    mut existing_render_visible_entities_cpu_culling: Query<
        &mut RenderExtractedVisibleEntities,
        With<RenderVisibleEntities>,
    >,
    gpu_preprocessing_support: Res<GpuPreprocessingSupport>,
    visibility_extraction_system_param: VisibilityExtractionSystemParam,
) {
    main_pass_formats.clear();
    let primary_window = primary_window.iter().next();
    type ExtractedCameraComponents = (
        ExtractedCamera,
        ExtractedView,
        RenderVisibleEntities,
        TemporalJitter,
        MipBias,
        RenderLayers,
        Projection,
        NoIndirectDrawing,
        ViewUniformOffset,
    );

    for (
        main_entity,
        render_entity,
        camera,
        render_target,
        camera_render_graph,
        transform,
        visible_entities,
        frustum,
        (
            hdr,
            compositing_space,
            color_grading,
            exposure,
            temporal_jitter,
            mip_bias,
            render_layers,
            projection,
            no_indirect_drawing,
        ),
    ) in query.iter()
    {
        if !camera.is_active {
            commands
                .entity(render_entity)
                .remove::<ExtractedCameraComponents>();
            continue;
        }

        let color_grading = color_grading.unwrap_or(&ColorGrading::default()).clone();

        if let (
            Some(URect {
                min: viewport_origin,
                ..
            }),
            Some(viewport_size),
            Some(target_size),
        ) = (
            camera.physical_viewport_rect(),
            camera.physical_viewport_size(),
            camera.physical_target_size(),
        ) {
            if target_size.x == 0 || target_size.y == 0 {
                commands
                    .entity(render_entity)
                    .remove::<ExtractedCameraComponents>();
                continue;
            }

            let mut render_visible_entities_cpu_culling =
                match existing_render_visible_entities_cpu_culling.get_mut(render_entity) {
                    Ok(ref mut existing_render_visible_entities_cpu_culling) => {
                        mem::take(&mut **existing_render_visible_entities_cpu_culling)
                    }
                    Err(_) => RenderExtractedVisibleEntities::default(),
                };

            for (visibility_class, visible_mesh_entities) in visible_entities.entities.iter() {
                let render_view_visible_entities = render_visible_entities_cpu_culling
                    .classes
                    .entry(*visibility_class)
                    .or_default();
                render_view_visible_entities.entities.clear();
                for main_entity in visible_mesh_entities {
                    let render_entity =
                        match visibility_extraction_system_param.mapper.get(*main_entity) {
                            Ok(render_entity) => render_entity.entity(),
                            Err(_) => Entity::PLACEHOLDER,
                        };
                    render_view_visible_entities
                        .entities
                        .push((render_entity, MainEntity::from(*main_entity)));
                }
            }

            let target = render_target.normalize(primary_window);
            let output_texture_format = target
                .as_ref()
                .and_then(|target| {
                    target
                        .get_texture_view_format(&extracted_windows, &images, &manual_texture_views)
                        .map(|format| normalize_bgra8(target, format))
                })
                .unwrap_or(TextureFormat::Rgba8UnormSrgb);
            let target_format = if hdr {
                TextureFormat::Rgba16Float
            } else if compositing_space.is_some_and(|s| *s == CompositingSpace::Srgb) {
                TextureFormat::Rgba8Unorm
            } else {
                output_texture_format
            };
            main_pass_formats.insert(render_entity, target_format);

            let mut commands = commands.entity(render_entity);
            commands.insert((
                ExtractedCamera {
                    target,
                    viewport: camera.viewport.clone(),
                    physical_viewport_size: Some(viewport_size),
                    physical_target_size: Some(target_size),
                    schedule: camera_render_graph.0,
                    order: camera.order,
                    output_mode: camera.output_mode,
                    msaa_writeback: camera.msaa_writeback,
                    clear_color: camera.clear_color,

                    sorted_camera_index_for_target: 0,
                    exposure: exposure
                        .map(Exposure::exposure)
                        .unwrap_or_else(|| Exposure::default().exposure()),
                    hdr,
                    compositing_space: compositing_space.copied(),
                },
                ExtractedView {
                    retained_view_entity: RetainedViewEntity::new(main_entity.into(), None, 0),
                    clip_from_view: camera.clip_from_view(),
                    world_from_view: *transform,
                    clip_from_world: None,
                    target_format,
                    viewport: UVec4::new(
                        viewport_origin.x,
                        viewport_origin.y,
                        viewport_size.x,
                        viewport_size.y,
                    ),
                    color_grading,
                    invert_culling: camera.invert_culling,
                },
                render_visible_entities_cpu_culling,
                *frustum,
            ));

            if let Some(temporal_jitter) = temporal_jitter {
                commands.insert(temporal_jitter.clone());
            } else {
                commands.remove::<TemporalJitter>();
            }

            if let Some(mip_bias) = mip_bias {
                commands.insert(mip_bias.clone());
            } else {
                commands.remove::<MipBias>();
            }

            if let Some(render_layers) = render_layers {
                commands.insert(render_layers.clone());
            } else {
                commands.remove::<RenderLayers>();
            }

            if let Some(projection) = projection {
                commands.insert(projection.clone());
            } else {
                commands.remove::<Projection>();
            }

            if no_indirect_drawing
                || !matches!(
                    gpu_preprocessing_support.max_supported_mode,
                    GpuPreprocessingMode::Culling
                )
            {
                commands.insert(NoIndirectDrawing);
            } else {
                commands.remove::<NoIndirectDrawing>();
            }
        };
    }
}

fn normalize_bgra8(target: &NormalizedRenderTarget, format: TextureFormat) -> TextureFormat {
    if matches!(target, NormalizedRenderTarget::Window(_))
        && format == TextureFormat::Bgra8UnormSrgb
    {
        return TextureFormat::Rgba8UnormSrgb;
    }
    format
}

#[derive(Resource, Default)]
pub struct SortedCameras(pub Vec<SortedCamera>);

pub struct SortedCamera {
    pub entity: Entity,
    pub order: isize,
    pub target: Option<NormalizedRenderTarget>,
    pub hdr: bool,
    pub output_mode: CameraOutputMode,
}

pub fn sort_cameras(
    mut sorted_cameras: ResMut<SortedCameras>,
    mut cameras: Query<(Entity, &mut ExtractedCamera)>,
) {
    sorted_cameras.0.clear();
    for (entity, camera) in cameras.iter() {
        sorted_cameras.0.push(SortedCamera {
            entity,
            order: camera.order,
            target: camera.target.clone(),
            hdr: camera.hdr,
            output_mode: camera.output_mode,
        });
    }

    sorted_cameras
        .0
        .sort_by(|c1, c2| (c1.order, &c1.target).cmp(&(c2.order, &c2.target)));
    let mut previous_order_target = None;
    let mut ambiguities = <HashSet<_>>::default();
    let mut target_counts = <HashMap<_, _>>::default();
    for sorted_camera in &mut sorted_cameras.0 {
        let new_order_target = (sorted_camera.order, sorted_camera.target.clone());
        if let Some(previous_order_target) = previous_order_target
            && previous_order_target == new_order_target
        {
            ambiguities.insert(new_order_target.clone());
        }
        if let Some(target) = &sorted_camera.target {
            let count = target_counts
                .entry((target.clone(), sorted_camera.hdr))
                .or_insert(0usize);
            let (_, mut camera) = cameras.get_mut(sorted_camera.entity).unwrap();
            camera.sorted_camera_index_for_target = *count;
            *count += 1;
        }
        previous_order_target = Some(new_order_target);
    }

    if !ambiguities.is_empty() {
        warn_once!(
            "Camera order ambiguities detected for active cameras with the following priorities: {:?}. \
            To fix this, ensure there is exactly one Camera entity spawned with a given order for a given RenderTarget. \
            Ambiguities should be resolved because either (1) multiple active cameras were spawned accidentally, which will \
            result in rendering multiple instances of the scene or (2) for cases where multiple active cameras is intentional, \
            ambiguities could result in unpredictable render results.",
            ambiguities
        );
    }
}

#[derive(Component, Clone, Default, Reflect)]
#[reflect(Default, Component, Clone)]
pub struct TemporalJitter {

    pub offset: Vec2,
}

impl TemporalJitter {
    pub fn jitter_projection(&self, clip_from_view: &mut Mat4, view_size: Vec2) {

        let mut jitter = (self.offset * vec2(2.0, -2.0)) / view_size;

        if clip_from_view.w_axis.w == 1.0 {
            jitter *= vec2(clip_from_view.x_axis.x, clip_from_view.y_axis.y) * 0.5;
        }

        clip_from_view.z_axis.x += jitter.x;
        clip_from_view.z_axis.y += jitter.y;
    }
}

#[derive(Component, Reflect, Clone)]
#[reflect(Default, Component)]
pub struct MipBias(pub f32);

impl Default for MipBias {
    fn default() -> Self {
        Self(-1.0)
    }
}

#[derive(Clone, Resource, Default)]
pub struct DirtySpecializations {

    pub changed_renderables: MainEntityHashSet,

    pub removed_renderables: MainEntityHashSet,

    pub views: HashSet<RetainedViewEntity>,
}

impl DirtySpecializations {

    pub fn must_wipe_specializations_for_view(&self, view: RetainedViewEntity) -> bool {
        self.views.contains(&view)
    }

    fn entity_pair_from_visible_main_entity<'a>(
        &'a self,
        render_visible_mesh_entities: &'a RenderVisibleEntitiesClass,
        main_entity: &'a MainEntity,
    ) -> Option<(&'a Entity, &'a MainEntity)> {

        if let Ok(index) = render_visible_mesh_entities
            .entities_cpu_culling
            .binary_search_by_key(main_entity, |(_, main_entity)| *main_entity)
        {
            let (key, value) = &render_visible_mesh_entities.entities_cpu_culling[index];
            return Some((key, value));
        }

        if let Some(entity) = render_visible_mesh_entities
            .entities_gpu_culling
            .get(main_entity)
        {
            return Some((entity, main_entity));
        }

        None
    }

    pub fn iter_to_despecialize<'a>(&'a self) -> impl Iterator<Item = &'a MainEntity> {

        self.changed_renderables
            .iter()
            .chain(self.removed_renderables.iter())
    }

    pub fn iter_to_specialize<'a>(
        &'a self,
        view: RetainedViewEntity,
        render_view_visible_mesh_entities: &'a RenderVisibleEntitiesClass,
        last_frame_view_pending_queues: &'a HashSet<(Entity, MainEntity)>,
    ) -> impl Iterator<Item = (&'a Entity, &'a MainEntity)> {
        (if self.must_wipe_specializations_for_view(view) {
            Either::Left(render_view_visible_mesh_entities.iter_visible())
        } else {
            Either::Right(
                render_view_visible_mesh_entities
                    .added_entities()
                    .iter()
                    .map(|(entity, main_entity)| (entity, main_entity))
                    .chain(self.changed_renderables.iter().filter_map(|main_entity| {
                        self.entity_pair_from_visible_main_entity(
                            render_view_visible_mesh_entities,
                            main_entity,
                        )
                    })),
            )
        })
        .chain(last_frame_view_pending_queues.iter().filter_map(
            |(entity, main_entity)| {
                if render_view_visible_mesh_entities.entity_pair_is_visible(*entity, *main_entity) {
                    Some((entity, main_entity))
                } else {
                    None
                }
            },
        ))
    }

    pub fn iter_to_dequeue<'a>(
        &'a self,
        view: RetainedViewEntity,
        render_visible_mesh_entities: &'a RenderVisibleEntitiesClass,
    ) -> impl Iterator<Item = &'a MainEntity> {
        render_visible_mesh_entities
            .removed_entities
            .iter()
            .map(|(_, main_entity)| main_entity)
            .chain(if self.must_wipe_specializations_for_view(view) {

                Either::Left(
                    render_visible_mesh_entities
                        .iter_visible()
                        .map(|(_, main_entity)| main_entity),
                )
            } else {

                Either::Right(
                    self.changed_renderables
                        .iter()
                        .chain(self.removed_renderables.iter()),
                )
            })
    }

    pub fn iter_to_queue<'a>(
        &'a self,
        view: RetainedViewEntity,
        render_visible_mesh_entities: &'a RenderVisibleEntitiesClass,
        last_frame_view_pending_queues: &'a HashSet<(Entity, MainEntity)>,
    ) -> impl Iterator<Item = (&'a Entity, &'a MainEntity)> {
        (if self.must_wipe_specializations_for_view(view) {
            Either::Left(render_visible_mesh_entities.iter_visible())
        } else {
            Either::Right(
                render_visible_mesh_entities
                    .added_entities()
                    .iter()
                    .map(|(entity, main_entity)| (entity, main_entity))
                    .chain(self.changed_renderables.iter().filter_map(|main_entity| {

                        if render_visible_mesh_entities
                            .added_entities()
                            .binary_search_by_key(main_entity, |(_, main_entity)| *main_entity)
                            .is_err()
                        {
                            self.entity_pair_from_visible_main_entity(
                                render_visible_mesh_entities,
                                main_entity,
                            )
                        } else {
                            None
                        }
                    })),
            )
        })
        .chain(last_frame_view_pending_queues.iter().filter_map(
            |(entity, main_entity)| {
                if render_visible_mesh_entities.entity_pair_is_visible(*entity, *main_entity) {
                    Some((entity, main_entity))
                } else {
                    None
                }
            },
        ))
    }
}

#[derive(Clone, Resource, Default, Deref, DerefMut)]
pub struct DirtyWireframeSpecializations(pub DirtySpecializations);

pub fn clear_dirty_specializations(mut dirty_specializations: ResMut<DirtySpecializations>) {
    dirty_specializations.changed_renderables.clear();
    dirty_specializations.removed_renderables.clear();
    dirty_specializations.views.clear();
}

pub fn clear_dirty_wireframe_specializations(
    mut dirty_wireframe_specializations: ResMut<DirtyWireframeSpecializations>,
) {
    dirty_wireframe_specializations.changed_renderables.clear();
    dirty_wireframe_specializations.removed_renderables.clear();
    dirty_wireframe_specializations.views.clear();
}

pub fn expire_specializations_for_views(
    views: Query<&ExtractedView>,
    mut dirty_specializations: ResMut<DirtySpecializations>,
) {
    let all_live_retained_view_entities: HashSet<_> =
        views.iter().map(|view| view.retained_view_entity).collect();
    dirty_specializations.views.retain(|retained_view_entity| {
        all_live_retained_view_entities.contains(retained_view_entity)
    });
}

pub fn expire_wireframe_specializations_for_views(
    views: Query<&ExtractedView>,
    mut dirty_wireframe_specializations: ResMut<DirtyWireframeSpecializations>,
) {
    let all_live_retained_view_entities: HashSet<_> =
        views.iter().map(|view| view.retained_view_entity).collect();
    dirty_wireframe_specializations
        .views
        .retain(|retained_view_entity| {
            all_live_retained_view_entities.contains(retained_view_entity)
        });
}

#[derive(SystemSet, Clone, PartialEq, Eq, Debug, Hash)]
pub enum DirtySpecializationSystems {

    Clear,

    CheckForChanges,

    CheckForRemovals,
}

#[derive(Default, Deref, DerefMut)]
pub struct PendingQueues(pub HashMap<RetainedViewEntity, ViewPendingQueues>);

#[derive(Default)]
pub struct ViewPendingQueues {

    pub current_frame: HashSet<(Entity, MainEntity)>,

    pub prev_frame: HashSet<(Entity, MainEntity)>,
}

impl PendingQueues {

    pub fn prepare_for_new_frame(
        &mut self,
        retained_view_entity: RetainedViewEntity,
    ) -> &mut ViewPendingQueues {
        let view_pending_queues = self.entry(retained_view_entity).or_default();
        mem::swap(
            &mut view_pending_queues.current_frame,
            &mut view_pending_queues.prev_frame,
        );
        view_pending_queues.current_frame.clear();
        view_pending_queues
    }

    pub fn expire_stale_views(&mut self, all_views: &HashSet<RetainedViewEntity>) {
        self.retain(|retained_view_entity, _| all_views.contains(retained_view_entity));
    }
}
