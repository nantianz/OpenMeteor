pub mod visibility;
pub mod window;

use bevy_camera::{
    primitives::Frustum, CameraMainTextureUsages, ClearColor, ClearColorConfig, CompositingSpace,
    Exposure, MainPassResolutionOverride, NormalizedRenderTarget,
};
use bevy_diagnostic::FrameCount;
pub use visibility::*;
pub use window::*;

use crate::{
    camera::{ExtractedCamera, MipBias, NormalizedRenderTargetExt as _, TemporalJitter},
    extract_component::ExtractComponentPlugin,
    occlusion_culling::OcclusionCulling,
    render_asset::RenderAssets,
    render_phase::ViewRangefinder3d,
    render_resource::{DynamicUniformBuffer, ShaderType, Texture, TextureView},
    renderer::{RenderDevice, RenderQueue},
    sync_world::MainEntity,
    texture::{
        CachedTexture, ColorAttachment, DepthAttachment, GpuImage, ManualTextureViews,
        OutputColorAttachment, TextureCache,
    },
    GpuResourceAppExt, Render, RenderApp, RenderSystems,
};
use alloc::sync::{Arc, Weak};
use bevy_app::{App, Plugin};
use bevy_color::{LinearRgba, Oklaba, Srgba};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{prelude::*, VariantDefaults};
use bevy_image::ToExtents;
use bevy_math::{mat3, vec2, vec3, Mat3, Mat4, UVec4, Vec2, Vec3, Vec4, Vec4Swizzles};
use bevy_platform::collections::{hash_map::Entry, HashMap};
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use bevy_render_macros::ExtractComponent;
use bevy_shader::load_shader_library;
use bevy_transform::components::GlobalTransform;
use core::{
    ops::Range,
    sync::atomic::{AtomicUsize, Ordering},
};
use wgpu::{
    BufferUsages, Color as WgpuColor, RenderPassColorAttachment, RenderPassDepthStencilAttachment,
    StoreOp, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
};

static RGB_TO_LMS: Mat3 = mat3(
    vec3(0.311692, 0.0905138, 0.00764433),
    vec3(0.652085, 0.901341, 0.0486554),
    vec3(0.0362225, 0.00814478, 0.943700),
);

static LMS_TO_RGB: Mat3 = mat3(
    vec3(4.06305, -0.40791, -0.0118812),
    vec3(-2.93241, 1.40437, -0.0486532),
    vec3(-0.130646, 0.00353630, 1.0605344),
);

static D65_XY: Vec2 = vec2(0.31272, 0.32903);

static D65_LMS: Vec3 = vec3(0.975538, 1.01648, 1.08475);

pub const COLOR_TARGET_FORMAT_MASK_BITS: u32 = 0b11111;

#[inline]
pub fn texture_format_to_code(format: TextureFormat) -> Option<u8> {
    Some(match format {
        TextureFormat::R8Unorm => 0,
        TextureFormat::R8Snorm => 1,
        TextureFormat::Rg8Unorm => 2,
        TextureFormat::Rg8Snorm => 3,
        TextureFormat::Rgba8Unorm => 4,
        TextureFormat::Rgba8UnormSrgb => 5,
        TextureFormat::Rgba8Snorm => 6,
        TextureFormat::Bgra8Unorm => 7,
        TextureFormat::Bgra8UnormSrgb => 8,
        TextureFormat::R16Float => 11,
        TextureFormat::R16Unorm => 9,
        TextureFormat::R16Snorm => 10,
        TextureFormat::Rg16Float => 12,
        TextureFormat::Rg16Unorm => 13,
        TextureFormat::Rg16Snorm => 14,
        TextureFormat::Rgba16Float => 15,
        TextureFormat::Rgba16Unorm => 16,
        TextureFormat::Rgba16Snorm => 17,
        TextureFormat::R32Float => 18,
        TextureFormat::Rg32Float => 19,
        TextureFormat::Rgba32Float => 20,
        TextureFormat::Rgb10a2Unorm => 21,
        TextureFormat::Rg11b10Ufloat => 22,
        _ => return None,
    })
}

#[inline]
pub fn texture_format_from_code(code: u8) -> Option<TextureFormat> {
    Some(match code {
        0 => TextureFormat::R8Unorm,
        1 => TextureFormat::R8Snorm,
        2 => TextureFormat::Rg8Unorm,
        3 => TextureFormat::Rg8Snorm,
        4 => TextureFormat::Rgba8Unorm,
        5 => TextureFormat::Rgba8UnormSrgb,
        6 => TextureFormat::Rgba8Snorm,
        7 => TextureFormat::Bgra8Unorm,
        8 => TextureFormat::Bgra8UnormSrgb,
        11 => TextureFormat::R16Float,
        9 => TextureFormat::R16Unorm,
        10 => TextureFormat::R16Snorm,
        12 => TextureFormat::Rg16Float,
        13 => TextureFormat::Rg16Unorm,
        14 => TextureFormat::Rg16Snorm,
        15 => TextureFormat::Rgba16Float,
        16 => TextureFormat::Rgba16Unorm,
        17 => TextureFormat::Rgba16Snorm,
        18 => TextureFormat::R32Float,
        19 => TextureFormat::Rg32Float,
        20 => TextureFormat::Rgba32Float,
        21 => TextureFormat::Rgb10a2Unorm,
        22 => TextureFormat::Rg11b10Ufloat,
        _ => return None,
    })
}

pub struct ViewPlugin;

impl Plugin for ViewPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "view.wgsl");

        app

            .add_plugins((
                ExtractComponentPlugin::<Msaa>::default(),
                ExtractComponentPlugin::<OcclusionCulling>::default(),
                RenderVisibilityRangePlugin,
            ));

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(
                Render,
                (

                    clear_view_attachments
                        .in_set(RenderSystems::PrepareViews)
                        .before(create_surfaces),
                    cleanup_view_targets_for_resize
                        .in_set(RenderSystems::PrepareViews)
                        .before(create_surfaces),
                    prepare_view_attachments
                        .in_set(RenderSystems::PrepareViews)
                        .before(prepare_view_targets)
                        .after(prepare_windows),
                    prepare_view_targets
                        .in_set(RenderSystems::PrepareViews)
                        .after(prepare_windows)
                        .after(crate::render_asset::prepare_assets::<GpuImage>)
                        .ambiguous_with(crate::camera::sort_cameras),
                    prepare_view_uniforms.in_set(RenderSystems::PrepareResources),
                    collect_visible_cpu_culled_entities.in_set(RenderSystems::PrepareAssets),
                ),
            );
        }
    }

    fn finish(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_gpu_resource::<ViewUniforms>()
                .init_gpu_resource::<ViewTargetAttachments>();
        }
    }
}

#[derive(
    Component,
    Default,
    Clone,
    Copy,
    ExtractComponent,
    Reflect,
    PartialEq,
    PartialOrd,
    VariantDefaults,
    Eq,
    Hash,
    Debug,
)]
#[reflect(Component, Default, PartialEq, Hash, Debug)]
pub enum Msaa {
    Off = 1,
    Sample2 = 2,
    #[default]
    Sample4 = 4,
    Sample8 = 8,
}

impl Msaa {
    #[inline]
    pub fn samples(&self) -> u32 {
        *self as u32
    }

    pub fn from_samples(samples: u32) -> Self {
        match samples {
            1 => Msaa::Off,
            2 => Msaa::Sample2,
            4 => Msaa::Sample4,
            8 => Msaa::Sample8,
            _ => panic!("Unsupported MSAA sample count: {samples}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RetainedViewEntity {

    pub main_entity: MainEntity,

    pub auxiliary_entity: MainEntity,

    pub subview_index: u32,
}

impl RetainedViewEntity {

    pub fn new(
        main_entity: MainEntity,
        auxiliary_entity: Option<MainEntity>,
        subview_index: u32,
    ) -> Self {
        Self {
            main_entity,
            auxiliary_entity: auxiliary_entity.unwrap_or(Entity::PLACEHOLDER.into()),
            subview_index,
        }
    }
}

#[derive(Component)]
pub struct ExtractedView {

    pub retained_view_entity: RetainedViewEntity,

    pub clip_from_view: Mat4,
    pub world_from_view: GlobalTransform,

    pub clip_from_world: Option<Mat4>,

    pub target_format: TextureFormat,

    pub viewport: UVec4,
    pub color_grading: ColorGrading,

    pub invert_culling: bool,
}

impl ExtractedView {

    pub fn rangefinder3d(&self) -> ViewRangefinder3d {
        ViewRangefinder3d::from_world_from_view(&self.world_from_view.affine())
    }
}

#[derive(Component, Reflect, Debug, Default, Clone)]
#[reflect(Component, Default, Debug, Clone)]
pub struct ColorGrading {

    pub global: ColorGradingGlobal,

    pub shadows: ColorGradingSection,

    pub midtones: ColorGradingSection,

    pub highlights: ColorGradingSection,
}

#[derive(Clone, Debug, Reflect)]
#[reflect(Default, Clone)]
pub struct ColorGradingGlobal {

    pub exposure: f32,

    pub temperature: f32,

    pub tint: f32,

    pub hue: f32,

    pub post_saturation: f32,

    pub midtones_range: Range<f32>,
}

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct ColorGradingUniform {
    pub balance: Mat3,
    pub saturation: Vec3,
    pub contrast: Vec3,
    pub gamma: Vec3,
    pub gain: Vec3,
    pub lift: Vec3,
    pub midtone_range: Vec2,
    pub exposure: f32,
    pub hue: f32,
    pub post_saturation: f32,
}

#[derive(Reflect, Debug, Copy, Clone, PartialEq)]
#[reflect(Clone, PartialEq)]
pub struct ColorGradingSection {

    pub saturation: f32,

    pub contrast: f32,

    pub gamma: f32,

    pub gain: f32,

    pub lift: f32,
}

impl Default for ColorGradingGlobal {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            temperature: 0.0,
            tint: 0.0,
            hue: 0.0,
            post_saturation: 1.0,
            midtones_range: 0.2..0.7,
        }
    }
}

impl Default for ColorGradingSection {
    fn default() -> Self {
        Self {
            saturation: 1.0,
            contrast: 1.0,
            gamma: 1.0,
            gain: 1.0,
            lift: 0.0,
        }
    }
}

impl ColorGrading {

    pub fn with_identical_sections(
        global: ColorGradingGlobal,
        section: ColorGradingSection,
    ) -> ColorGrading {
        ColorGrading {
            global,
            highlights: section,
            midtones: section,
            shadows: section,
        }
    }

    pub fn all_sections(&self) -> impl Iterator<Item = &ColorGradingSection> {
        [&self.shadows, &self.midtones, &self.highlights].into_iter()
    }

    pub fn all_sections_mut(&mut self) -> impl Iterator<Item = &mut ColorGradingSection> {
        [&mut self.shadows, &mut self.midtones, &mut self.highlights].into_iter()
    }
}

#[derive(Default, Resource, Debug)]
pub struct RenderShadowLodOrigin(pub Vec3);

#[derive(Clone, ShaderType)]
pub struct ViewUniform {
    pub clip_from_world: Mat4,
    pub unjittered_clip_from_world: Mat4,
    pub world_from_clip: Mat4,
    pub world_from_view: Mat4,
    pub view_from_world: Mat4,

    pub clip_from_view: Mat4,
    pub view_from_clip: Mat4,
    pub world_position: Vec3,
    pub exposure: f32,

    pub viewport: Vec4,
    pub main_pass_viewport: Vec4,

    pub frustum: [Vec4; 6],

    pub lod_view_world_position: Vec3,
    pub color_grading: ColorGradingUniform,
    pub mip_bias: f32,
    pub frame_count: u32,
}

#[derive(Resource)]
pub struct ViewUniforms {
    pub uniforms: DynamicUniformBuffer<ViewUniform>,
}

impl FromWorld for ViewUniforms {
    fn from_world(world: &mut World) -> Self {
        let mut uniforms = DynamicUniformBuffer::default();
        uniforms.set_label(Some("view_uniforms_buffer"));

        let render_device = world.resource::<RenderDevice>();
        if render_device.limits().max_storage_buffers_per_shader_stage > 0 {
            uniforms.add_usages(BufferUsages::STORAGE);
        }

        Self { uniforms }
    }
}

#[derive(Component)]
pub struct ViewUniformOffset {
    pub offset: u32,
}

#[derive(Component, Clone)]
pub struct ViewTarget {
    main_textures: MainTargetTextures,
    main_texture_format: TextureFormat,

    main_texture: Arc<AtomicUsize>,

    out_texture: Option<OutputColorAttachment>,

    pub compositing_space: Option<CompositingSpace>,
}

#[derive(Resource, Default, Deref, DerefMut)]
pub struct ViewTargetAttachments(HashMap<NormalizedRenderTarget, OutputColorAttachment>);

pub struct PostProcessWrite<'a> {
    pub source: &'a TextureView,
    pub source_texture: &'a Texture,
    pub destination: &'a TextureView,
    pub destination_texture: &'a Texture,
}

impl From<ColorGrading> for ColorGradingUniform {
    fn from(component: ColorGrading) -> Self {

        let white_point_xy = D65_XY + vec2(-component.global.temperature, component.global.tint);

        let white_point_lms = vec3(0.701634, 1.15856, -0.904175)
            + (vec3(-0.051461, 0.045854, 0.953127)
                + vec3(0.452749, -0.296122, -0.955206) * white_point_xy.x)
                / white_point_xy.y;

        let white_point_adjustment = Mat3::from_diagonal(D65_LMS / white_point_lms);

        let balance = LMS_TO_RGB * white_point_adjustment * RGB_TO_LMS;

        Self {
            balance,
            saturation: vec3(
                component.shadows.saturation,
                component.midtones.saturation,
                component.highlights.saturation,
            ),
            contrast: vec3(
                component.shadows.contrast,
                component.midtones.contrast,
                component.highlights.contrast,
            ),
            gamma: vec3(
                component.shadows.gamma,
                component.midtones.gamma,
                component.highlights.gamma,
            ),
            gain: vec3(
                component.shadows.gain,
                component.midtones.gain,
                component.highlights.gain,
            ),
            lift: vec3(
                component.shadows.lift,
                component.midtones.lift,
                component.highlights.lift,
            ),
            midtone_range: vec2(
                component.global.midtones_range.start,
                component.global.midtones_range.end,
            ),
            exposure: component.global.exposure,
            hue: component.global.hue,
            post_saturation: component.global.post_saturation,
        }
    }
}

#[derive(Component, Default)]
pub struct NoIndirectDrawing;

impl ViewTarget {
    #[deprecated(
        note = "Use ExtractedView::target_format where possible. Bevy does not encourage a default HDR TextureFormat anymore. If you really need this, use TextureFormat::Rgba16Float"
    )]
    pub const TEXTURE_FORMAT_HDR: TextureFormat = TextureFormat::Rgba16Float;

    pub fn get_color_attachment(&self) -> RenderPassColorAttachment<'_> {
        if self.main_texture.load(Ordering::SeqCst) == 0 {
            self.main_textures.a.get_attachment()
        } else {
            self.main_textures.b.get_attachment()
        }
    }

    pub fn get_unsampled_color_attachment(&self) -> RenderPassColorAttachment<'_> {
        if self.main_texture.load(Ordering::SeqCst) == 0 {
            self.main_textures.a.get_unsampled_attachment()
        } else {
            self.main_textures.b.get_unsampled_attachment()
        }
    }

    pub fn main_texture(&self) -> &Texture {
        if self.main_texture.load(Ordering::SeqCst) == 0 {
            &self.main_textures.a.texture.texture
        } else {
            &self.main_textures.b.texture.texture
        }
    }

    pub fn main_texture_other(&self) -> &Texture {
        if self.main_texture.load(Ordering::SeqCst) == 0 {
            &self.main_textures.b.texture.texture
        } else {
            &self.main_textures.a.texture.texture
        }
    }

    pub fn main_texture_view(&self) -> &TextureView {
        if self.main_texture.load(Ordering::SeqCst) == 0 {
            &self.main_textures.a.texture.default_view
        } else {
            &self.main_textures.b.texture.default_view
        }
    }

    pub fn main_texture_other_view(&self) -> &TextureView {
        if self.main_texture.load(Ordering::SeqCst) == 0 {
            &self.main_textures.b.texture.default_view
        } else {
            &self.main_textures.a.texture.default_view
        }
    }

    pub fn sampled_main_texture(&self) -> Option<&Texture> {
        self.main_textures
            .a
            .resolve_target
            .as_ref()
            .map(|sampled| &sampled.texture)
    }

    pub fn sampled_main_texture_view(&self) -> Option<&TextureView> {
        self.main_textures
            .a
            .resolve_target
            .as_ref()
            .map(|sampled| &sampled.default_view)
    }

    #[inline]
    pub fn main_texture_format(&self) -> TextureFormat {
        self.main_texture_format
    }

    #[inline]
    pub fn out_texture(&self) -> Option<&TextureView> {
        self.out_texture.as_ref().map(|t| &t.view)
    }

    pub fn out_texture_color_attachment(
        &self,
        clear_color: Option<LinearRgba>,
    ) -> Option<RenderPassColorAttachment<'_>> {
        self.out_texture
            .as_ref()
            .map(|t| t.get_attachment(clear_color))
    }

    pub fn needs_present(&self) -> bool {
        self.out_texture
            .as_ref()
            .is_some_and(OutputColorAttachment::needs_present)
    }

    #[inline]
    pub fn out_texture_view_format(&self) -> Option<TextureFormat> {
        self.out_texture.as_ref().map(|t| t.view_format)
    }

    pub fn post_process_write(&self) -> PostProcessWrite<'_> {
        let old_is_a_main_texture = self.main_texture.fetch_xor(1, Ordering::SeqCst);

        if old_is_a_main_texture == 0 {
            self.main_textures.b.mark_as_cleared();
            PostProcessWrite {
                source: &self.main_textures.a.texture.default_view,
                source_texture: &self.main_textures.a.texture.texture,
                destination: &self.main_textures.b.texture.default_view,
                destination_texture: &self.main_textures.b.texture.texture,
            }
        } else {
            self.main_textures.a.mark_as_cleared();
            PostProcessWrite {
                source: &self.main_textures.b.texture.default_view,
                source_texture: &self.main_textures.b.texture.texture,
                destination: &self.main_textures.a.texture.default_view,
                destination_texture: &self.main_textures.a.texture.texture,
            }
        }
    }
}

#[derive(Component)]
pub struct ViewDepthTexture {
    pub texture: Texture,
    attachment: DepthAttachment,
}

impl ViewDepthTexture {
    pub fn new(texture: CachedTexture, clear_value: Option<f32>) -> Self {
        Self {
            texture: texture.texture,
            attachment: DepthAttachment::new(texture.default_view, clear_value),
        }
    }

    pub fn get_attachment(&self, store: StoreOp) -> RenderPassDepthStencilAttachment<'_> {
        self.attachment.get_attachment(store)
    }

    pub fn view(&self) -> &TextureView {
        &self.attachment.view
    }
}

pub fn prepare_view_uniforms(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    mut view_uniforms: ResMut<ViewUniforms>,
    views: Query<(
        Entity,
        Option<&ExtractedCamera>,
        &ExtractedView,
        Option<&Frustum>,
        Option<&TemporalJitter>,
        Option<&MipBias>,
        Option<&MainPassResolutionOverride>,
    )>,
    frame_count: Res<FrameCount>,
    shadow_lod_origin: Option<Res<RenderShadowLodOrigin>>,
) {
    let view_iter = views.iter();
    let view_count = view_iter.len();
    let Some(mut writer) =
        view_uniforms
            .uniforms
            .get_writer(view_count, &render_device, &render_queue)
    else {
        return;
    };
    for (
        entity,
        extracted_camera,
        extracted_view,
        frustum,
        temporal_jitter,
        mip_bias,
        resolution_override,
    ) in &views
    {
        let viewport = extracted_view.viewport.as_vec4();
        let mut main_pass_viewport = viewport;
        if let Some(resolution_override) = resolution_override {
            main_pass_viewport.z = resolution_override.0.x as f32;
            main_pass_viewport.w = resolution_override.0.y as f32;
        }

        let unjittered_projection = extracted_view.clip_from_view;
        let mut clip_from_view = unjittered_projection;

        if let Some(temporal_jitter) = temporal_jitter {
            temporal_jitter.jitter_projection(&mut clip_from_view, main_pass_viewport.zw());
        }

        let view_from_clip = clip_from_view.inverse();
        let world_from_view = extracted_view.world_from_view.to_matrix();
        let view_from_world = world_from_view.inverse();

        let clip_from_world = if temporal_jitter.is_some() {
            clip_from_view * view_from_world
        } else {
            extracted_view
                .clip_from_world
                .unwrap_or_else(|| clip_from_view * view_from_world)
        };

        let frustum = frustum
            .map(|frustum| frustum.half_spaces.map(|h| h.normal_d()))
            .unwrap_or([Vec4::ZERO; 6]);

        let lod_view_world_position = match (&extracted_camera, &shadow_lod_origin) {
            (Some(_), _) | (None, None) => {

                extracted_view.world_from_view.translation()
            }
            (None, Some(shadow_lod_origin))
                if extracted_view.retained_view_entity.auxiliary_entity
                    == MainEntity::from(Entity::PLACEHOLDER) =>
            {

                shadow_lod_origin.0
            }
            (None, Some(shadow_lod_origin)) => {

                match views.get(
                    extracted_view
                        .retained_view_entity
                        .auxiliary_entity
                        .entity(),
                ) {
                    Ok((_, _, camera_view, _, _, _, _)) => {
                        camera_view.world_from_view.translation()
                    }
                    Err(_) => shadow_lod_origin.0,
                }
            }
        };

        let view_uniforms = ViewUniformOffset {
            offset: writer.write(&ViewUniform {
                clip_from_world,
                unjittered_clip_from_world: unjittered_projection * view_from_world,
                world_from_clip: world_from_view * view_from_clip,
                world_from_view,
                view_from_world,
                clip_from_view,
                view_from_clip,
                world_position: extracted_view.world_from_view.translation(),
                exposure: extracted_camera
                    .map(|c| c.exposure)
                    .unwrap_or_else(|| Exposure::default().exposure()),
                viewport,
                main_pass_viewport,
                frustum,
                lod_view_world_position,
                color_grading: extracted_view.color_grading.clone().into(),
                mip_bias: mip_bias.unwrap_or(&MipBias(0.0)).0,
                frame_count: frame_count.0,
            }),
        };

        commands.entity(entity).insert(view_uniforms);
    }
}

#[derive(Clone)]
struct MainTargetTextures {
    a: ColorAttachment,
    b: ColorAttachment,

    main_texture: Arc<AtomicUsize>,
}

pub fn prepare_view_attachments(
    windows: Res<ExtractedWindows>,
    images: Res<RenderAssets<GpuImage>>,
    manual_texture_views: Res<ManualTextureViews>,
    cameras: Query<&ExtractedCamera>,
    mut view_target_attachments: ResMut<ViewTargetAttachments>,
) {
    for camera in cameras.iter() {
        let Some(target) = &camera.target else {
            continue;
        };

        if matches!(camera.output_mode, bevy_camera::CameraOutputMode::Skip) {
            continue;
        }

        match view_target_attachments.entry(target.clone()) {
            Entry::Occupied(_) => {}
            Entry::Vacant(entry) => {
                let Some(attachment) = target
                    .get_texture_view(&windows, &images, &manual_texture_views)
                    .cloned()
                    .zip(target.get_texture_view_format(&windows, &images, &manual_texture_views))
                    .map(|(view, format)| OutputColorAttachment::new(view.clone(), format))
                else {
                    continue;
                };
                entry.insert(attachment);
            }
        };
    }
}

pub fn clear_view_attachments(mut view_target_attachments: ResMut<ViewTargetAttachments>) {
    view_target_attachments.clear();
}

pub fn cleanup_view_targets_for_resize(
    mut commands: Commands,
    windows: Res<ExtractedWindows>,
    cameras: Query<(Entity, &ExtractedCamera), With<ViewTarget>>,
) {
    for (entity, camera) in &cameras {
        if let Some(NormalizedRenderTarget::Window(window_ref)) = &camera.target
            && let Some(window) = windows.get(&window_ref.entity())
            && (window.size_changed || window.present_mode_changed)
        {
            commands.entity(entity).remove::<ViewTarget>();
        }
    }
}

type MainTextureKey = (
    Option<NormalizedRenderTarget>,
    TextureUsages,
    TextureFormat,
    Msaa,
);

pub fn prepare_view_targets(
    mut commands: Commands,
    clear_color_global: Res<ClearColor>,
    render_device: Res<RenderDevice>,
    mut texture_cache: ResMut<TextureCache>,
    cameras: Query<(
        Entity,
        &ExtractedCamera,
        &ExtractedView,
        &CameraMainTextureUsages,
        &Msaa,
    )>,
    view_target_attachments: Res<ViewTargetAttachments>,
    mut main_texture_atomics: Local<HashMap<MainTextureKey, Weak<AtomicUsize>>>,
) {
    main_texture_atomics.retain(|_, weak| weak.strong_count() > 0);

    let mut textures = <HashMap<_, _>>::default();
    for (entity, camera, view, texture_usage, msaa) in cameras.iter() {
        let Some(target_size) = camera.physical_target_size else {

            commands.entity(entity).try_remove::<ViewTarget>();
            continue;
        };

        let out_attachment = camera
            .target
            .as_ref()
            .and_then(|target| view_target_attachments.get(target));

        if out_attachment.is_none() && !matches!(camera.clear_color, ClearColorConfig::None) {
            commands.entity(entity).try_remove::<ViewTarget>();
            continue;
        }

        let main_texture_format = view.target_format;

        let clear_color = match camera.clear_color {
            ClearColorConfig::Custom(color) => Some(color),
            ClearColorConfig::None => None,
            _ => Some(clear_color_global.0),
        };

        let converted_clear_color: Option<WgpuColor> =
            clear_color.map(|color| match camera.compositing_space {

                Some(CompositingSpace::Oklab) => Oklaba::from(color).into(),
                Some(CompositingSpace::Srgb) => Srgba::from(color).into(),
                Some(CompositingSpace::Linear) | None => LinearRgba::from(color).into(),
            });

        let key: MainTextureKey = (
            camera.target.clone(),
            texture_usage.0,
            main_texture_format,
            *msaa,
        );
        let (a, b, sampled, main_texture) = textures.entry(key.clone()).or_insert_with(|| {
            let descriptor = TextureDescriptor {
                label: None,
                size: target_size.to_extents(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: TextureDimension::D2,
                format: main_texture_format,
                usage: texture_usage.0,
                view_formats: match main_texture_format {
                    TextureFormat::Bgra8Unorm => &[TextureFormat::Bgra8UnormSrgb],
                    TextureFormat::Rgba8Unorm => &[TextureFormat::Rgba8UnormSrgb],
                    _ => &[],
                },
            };
            let a = texture_cache.get(
                &render_device,
                TextureDescriptor {
                    label: Some("main_texture_a"),
                    ..descriptor
                },
            );
            let b = texture_cache.get(
                &render_device,
                TextureDescriptor {
                    label: Some("main_texture_b"),
                    ..descriptor
                },
            );
            let sampled = if msaa.samples() > 1 {
                let sampled = texture_cache.get(
                    &render_device,
                    TextureDescriptor {
                        label: Some("main_texture_sampled"),
                        size: target_size.to_extents(),
                        mip_level_count: 1,
                        sample_count: msaa.samples(),
                        dimension: TextureDimension::D2,
                        format: main_texture_format,
                        usage: TextureUsages::RENDER_ATTACHMENT,
                        view_formats: descriptor.view_formats,
                    },
                );
                Some(sampled)
            } else {
                None
            };

            let main_texture = match main_texture_atomics.entry(key) {
                Entry::Occupied(e) => e
                    .get()
                    .upgrade()
                    .expect("dead weaks were pruned at top of system"),
                Entry::Vacant(e) => {
                    let arc = Arc::new(AtomicUsize::new(0));
                    e.insert(Arc::downgrade(&arc));
                    arc
                }
            };
            (a, b, sampled, main_texture)
        });

        let main_textures = MainTargetTextures {
            a: ColorAttachment::new(a.clone(), sampled.clone(), None, converted_clear_color),
            b: ColorAttachment::new(b.clone(), sampled.clone(), None, converted_clear_color),
            main_texture: main_texture.clone(),
        };

        commands.entity(entity).insert(ViewTarget {
            main_texture: main_textures.main_texture.clone(),
            main_textures,
            main_texture_format,
            out_texture: out_attachment.cloned(),
            compositing_space: camera.compositing_space,
        });
    }
}
