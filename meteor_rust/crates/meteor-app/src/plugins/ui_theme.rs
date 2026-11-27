use crate::app_state::{MenuRoot, NetRoomRoot, RoomRoot, SettingsRoot};
use crate::plugins::ui_components::{UiDialogSurface, UiNoticePanel, UiPanel};
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::{Shader, ShaderRef};
use bevy::text::{FontSize, LetterSpacing};
use bevy::ui::{widget::TextShadow, FocusPolicy};

pub const BACKGROUND: Color = Color::srgb(0.045, 0.066, 0.088);
pub const SURFACE: Color = Color::srgba(0.105, 0.143, 0.181, 0.91);
pub const CARD: Color = Color::srgba(0.139, 0.181, 0.224, 0.92);
pub const HOVER: Color = Color::srgb(0.213, 0.256, 0.303);
pub const SELECTED: Color = Color::srgba(0.373, 0.167, 0.157, 0.92);
pub const ACCENT: Color = Color::srgb(0.77, 0.32, 0.24);
pub const ACCENT_TEXT: Color = Color::srgb(1.0, 0.72, 0.61);
pub const PRIMARY: Color = Color::srgb(0.49, 0.20, 0.17);
pub const PRIMARY_HOVER: Color = Color::srgb(0.62, 0.24, 0.18);
pub const PRIMARY_PRESSED: Color = Color::srgb(0.34, 0.13, 0.12);
pub const CRIMSON_BRIGHT: Color = Color::srgb(1.0, 0.39, 0.29);
pub const TEXT: Color = Color::srgb(0.97, 0.93, 0.86);
pub const MUTED: Color = Color::srgb(0.72, 0.77, 0.82);
pub const METAL: Color = Color::srgb(0.65, 0.73, 0.79);
pub const LINE: Color = Color::srgba(0.59, 0.68, 0.75, 0.38);
pub const SCRIM: Color = Color::srgba(0.029, 0.046, 0.065, 0.84);
pub const HUD_SURFACE: Color = Color::srgba(0.063, 0.094, 0.126, 0.88);
pub const HEALTH: Color = Color::srgb(0.78, 0.16, 0.15);
pub const ANGER: Color = Color::srgb(0.82, 0.64, 0.28);

#[derive(Component)]
pub(crate) struct CinematicBackdrop;

#[derive(Component)]
pub(super) struct BackdropOpacity(pub f32);

#[derive(Component)]
pub(super) struct BackdropIntroTime(pub f32);
#[derive(Component)]
pub(super) struct BackdropShade {
    entity: Entity,
    opacity: f32,
    narrow: bool,
}
#[derive(Component)]
pub(crate) struct OrnateFrame;

#[derive(Resource)]
pub struct CinematicArt {
    pub image: Handle<Image>,
    pub size: Vec2,
}

#[derive(Component, Clone, Copy)]
#[require(BackgroundGradient)]
pub struct ActionStyle {
    pub primary: bool,
}
impl ActionStyle {
    pub const PRIMARY: Self = Self { primary: true };
    pub const SECONDARY: Self = Self { primary: false };
}

#[derive(Component)]
pub struct CompactAction;

const BACKDROP_ASSET: &str = "ui/mountain-pass-background.jpg";
const BACKDROP_BLUR_RADIUS: f32 = 0.0055;
const TITLE_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("f4aa72cc-64d8-49a8-8c7e-ec01958cfbb8");
const INK_STROKE_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("92c1e6e1-7840-4d9d-9f4e-77d8102a5b11");
const FRAME_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("5d47e129-9b34-4412-b89f-19e7c2d08f33");

const BACKDROP_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("b84e19c4-7f26-4d5e-a312-90c5d87e42a1");
#[derive(ShaderType, Clone, Copy, Debug)]
pub struct BackdropUniform {
    pub uv_rect: Vec4,
    pub params: Vec4,
    pub sweep: Vec2,
}
#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct BackdropMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub image: Handle<Image>,
    #[uniform(2)]
    pub backdrop: BackdropUniform,
}
impl UiMaterial for BackdropMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(BACKDROP_SHADER)
    }
}
#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct CalligraphyMaterial {
    #[texture(0)]
    #[sampler(1)]
    pub(crate) image: Handle<Image>,
}
impl UiMaterial for CalligraphyMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(TITLE_SHADER)
    }
}

#[derive(ShaderType, Clone, Copy, Debug)]
pub struct InkStrokeUniform {
    pub color: Vec4,
    pub params: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct InkStrokeMaterial {
    #[uniform(0)]
    pub stroke: InkStrokeUniform,
}
impl UiMaterial for InkStrokeMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(INK_STROKE_SHADER)
    }
}

#[derive(ShaderType, Clone, Copy, Debug)]
pub struct FrameUniform {
    pub color: Vec4,
    pub params: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct FrameMaterial {
    #[uniform(0)]
    pub frame: FrameUniform,
}
impl UiMaterial for FrameMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(FRAME_SHADER)
    }
}

#[derive(Resource)]
struct CalligraphyArt(Handle<CalligraphyMaterial>);

#[derive(Resource)]
struct SharedDecorMaterials {
    panel_frame: Handle<FrameMaterial>,
    dialog_frame: Handle<FrameMaterial>,
}

#[derive(Component)]
pub(crate) struct CalligraphyTitle;

#[derive(Component)]
struct HasInkStroke;

#[derive(Component)]
struct InkStrokeUnderlay {
    button: Entity,
    material: Handle<InkStrokeMaterial>,
    primary: bool,
    seed: f32,
    progress: f32,
}

#[derive(Component)]
struct HasOrnateFrame;

pub(crate) fn title_node(compact: bool) -> impl Bundle {
    (
        Node {
            width: Val::Px(if compact { 240.0 } else { 400.0 }),
            max_width: Val::Percent(100.0),
            aspect_ratio: Some(3.0),
            flex_shrink: 0.0,
            ..default()
        },
        CalligraphyTitle,
    )
}

fn load_title(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<CalligraphyMaterial>>,
) {
    let image = Image::from_buffer(
        &crate::resource_io::required("ui/cinematic-title-reborn-matte.png"),
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .expect("embedded calligraphy title must decode");
    let material = materials.add(CalligraphyMaterial {
        image: images.add(image),
    });
    commands.insert_resource(CalligraphyArt(material));
}

fn init_decor_materials(mut commands: Commands, mut frames: ResMut<Assets<FrameMaterial>>) {
    let panel_frame = frames.add(FrameMaterial {
        frame: FrameUniform {
            color: METAL.with_alpha(0.37).to_linear().to_vec4(),
            params: Vec4::new(18.0, 0.85, 0.0, 0.0),
        },
    });
    let dialog_frame = frames.add(FrameMaterial {
        frame: FrameUniform {
            color: METAL.with_alpha(0.52).to_linear().to_vec4(),
            params: Vec4::new(24.0, 1.0, 1.0, 0.0),
        },
    });
    commands.insert_resource(SharedDecorMaterials {
        panel_frame,
        dialog_frame,
    });
}

fn attach_title(
    mut commands: Commands,
    art: Res<CalligraphyArt>,
    titles: Query<
        Entity,
        (
            With<CalligraphyTitle>,
            Without<MaterialNode<CalligraphyMaterial>>,
        ),
    >,
) {
    for title in &titles {
        commands.entity(title).insert(MaterialNode(art.0.clone()));
    }
}

pub struct CinematicThemePlugin;
impl Plugin for CinematicThemePlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<Assets<Shader>>() {
            app.init_asset::<Shader>();
        }
        app.add_plugins((
            UiMaterialPlugin::<CalligraphyMaterial>::default(),
            UiMaterialPlugin::<InkStrokeMaterial>::default(),
            UiMaterialPlugin::<FrameMaterial>::default(),
            UiMaterialPlugin::<BackdropMaterial>::default(),
        ));
        bevy::asset::load_internal_asset!(
            app,
            TITLE_SHADER,
            "ui_calligraphy.wgsl",
            Shader::from_wgsl
        );
        bevy::asset::load_internal_asset!(
            app,
            INK_STROKE_SHADER,
            "ui_ink_stroke.wgsl",
            Shader::from_wgsl
        );
        bevy::asset::load_internal_asset!(app, FRAME_SHADER, "ui_frame.wgsl", Shader::from_wgsl);
        bevy::asset::load_internal_asset!(
            app,
            BACKDROP_SHADER,
            "ui_backdrop.wgsl",
            Shader::from_wgsl
        );
        app.add_systems(PreStartup, (load_art, load_title, init_decor_materials))
            .add_systems(
                Update,
                (
                    background_art,
                    attach_title,
                    attach_panel_frames,
                    attach_ink_strokes,
                    animate_ink_strokes,
                    polish_typography,
                    action_feedback,
                    compact_actions,
                    super::launcher_ui::hero_nav_feedback,
                )
                    .after(crate::plugins::UiTouchActionSet)
                    .after(crate::plugins::launcher_ui::adapt_launcher_touch_targets)
                    .after(crate::plugins::launcher_ui::adapt_launcher_layout),
            );
    }
}

fn read_backdrop_bytes() -> Option<Vec<u8>> {
    if let Ok(bytes) = crate::resource_io::read(BACKDROP_ASSET) {
        return Some(bytes);
    }
    #[cfg(test)]
    {
        let fallback = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/ui/mountain-pass-background.jpg");
        if let Ok(bytes) = std::fs::read(fallback) {
            return Some(bytes);
        }
    }
    None
}
fn fallback_backdrop_image() -> Image {
    let srgba = BACKGROUND.to_srgba();
    let pixel = [
        (srgba.red * 255.0).round() as u8,
        (srgba.green * 255.0).round() as u8,
        (srgba.blue * 255.0).round() as u8,
        255,
    ];
    let mut image = Image::new_fill(
        bevy::render::render_resource::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        &pixel,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}
pub(super) fn load_art(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let image = match read_backdrop_bytes() {
        Some(bytes) => match Image::from_buffer(
            &bytes,
            ImageType::Extension("jpg"),
            CompressedImageFormats::NONE,
            true,
            ImageSampler::linear(),
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        ) {
            Ok(image) => image,
            Err(error) => {
                warn!("mountain pass backdrop failed to decode, using solid fallback: {error}");
                fallback_backdrop_image()
            }
        },
        None => {
            warn!("external backdrop asset `{BACKDROP_ASSET}` not found, using solid fallback");
            fallback_backdrop_image()
        }
    };
    let size = image.size().as_vec2();
    commands.insert_resource(CinematicArt {
        image: images.add(image),
        size,
    });
}

pub(crate) fn cover_rect(image: Vec2, viewport: Vec2) -> Rect {
    let scale = (viewport.x / image.x).max(viewport.y / image.y).max(0.001);
    Rect::from_center_size(image * 0.5, viewport / scale)
}

fn cover_uv_rect(image: Vec2, viewport: Vec2) -> Vec4 {
    let rect = cover_rect(image, viewport);
    let inv = Vec2::new(1.0 / image.x.max(1.0), 1.0 / image.y.max(1.0));
    Vec4::new(
        rect.min.x * inv.x,
        rect.min.y * inv.y,
        rect.width() * inv.x,
        rect.height() * inv.y,
    )
}
pub(super) fn background_art(
    mut commands: Commands,
    art: Option<Res<CinematicArt>>,
    time: Option<Res<Time<Real>>>,
    windows: Query<&Window>,
    mut materials: ResMut<Assets<BackdropMaterial>>,
    mut shades: Query<&mut BackgroundGradient>,
    mut roots: Query<
        (
            Entity,
            Option<&MaterialNode<BackdropMaterial>>,
            Option<&BackdropOpacity>,
            Option<&BackdropIntroTime>,
            Option<&ComputedNode>,
            Option<&mut BackdropShade>,
        ),
        Or<(
            With<MenuRoot>,
            With<NetRoomRoot>,
            With<RoomRoot>,
            With<SettingsRoot>,
            With<CinematicBackdrop>,
        )>,
    >,
) {
    let Some(art) = art else {
        return;
    };
    let elapsed = time.map_or(0.0, |t| t.elapsed_secs());
    let size = windows.single().map_or(Vec2::new(1280.0, 720.0), |w| {
        Vec2::new(w.width(), w.height())
    });
    let aspect = (art.size.x / art.size.y.max(1.0)).max(0.25);
    for (entity, material_node, opacity, intro, computed, shade) in &mut roots {
        let viewport = computed
            .map(ComputedNode::size)
            .filter(|size| size.cmpgt(Vec2::ZERO).all())
            .unwrap_or(size);
        let opacity = opacity.map_or(1.0, |fade| fade.0);
        let sweep = match intro {
            Some(intro) => Vec2::new(0.27 + intro.0 * 0.52, 1.25),
            None => Vec2::new((elapsed * 0.14).fract() * 1.64 - 0.23, 1.0),
        };
        let uniform = BackdropUniform {
            uv_rect: cover_uv_rect(art.size, viewport),
            params: Vec4::new(elapsed, opacity, BACKDROP_BLUR_RADIUS, aspect),
            sweep,
        };
        if let Some(material_node) = material_node {
            if let Some(mut material) = materials.get_mut(&material_node.0) {
                if material.image != art.image {
                    material.image = art.image.clone();
                }
                material.backdrop = uniform;
            }
        } else {
            let handle = materials.add(BackdropMaterial {
                image: art.image.clone(),
                backdrop: uniform,
            });
            commands.entity(entity).insert(MaterialNode(handle));
        }
        let narrow = viewport.x < viewport.y * 1.15;
        if let Some(mut shade) = shade {
            if shade.opacity != opacity || shade.narrow != narrow {
                if let Ok(mut gradient) = shades.get_mut(shade.entity) {
                    *gradient = backdrop_gradient(opacity, narrow);
                }
                shade.opacity = opacity;
                shade.narrow = narrow;
            }
        } else {
            let layer = commands
                .spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: percent(100),
                        height: percent(100),
                        ..default()
                    },
                    backdrop_gradient(opacity, narrow),
                    FocusPolicy::Pass,
                    ZIndex(-1),
                ))
                .id();
            commands
                .entity(entity)
                .insert(BackdropShade {
                    entity: layer,
                    opacity,
                    narrow,
                })
                .add_child(layer);
        }
    }
}
fn backdrop_gradient(opacity: f32, narrow: bool) -> BackgroundGradient {
    BackgroundGradient(vec![
        LinearGradient::to_right(vec![
            ColorStop::percent(BACKGROUND.with_alpha(0.89 * opacity), 0),
            ColorStop::percent(BACKGROUND.with_alpha(0.80 * opacity), 27),
            ColorStop::percent(BACKGROUND.with_alpha(0.38 * opacity), 53),
            ColorStop::percent(
                BACKGROUND.with_alpha(if narrow { 0.40 } else { 0.05 } * opacity),
                100,
            ),
        ])
        .into(),
        LinearGradient::to_bottom(vec![
            ColorStop::percent(BACKGROUND.with_alpha(0.0), 78),
            ColorStop::percent(BACKGROUND.with_alpha(0.68 * opacity), 100),
        ])
        .into(),
    ])
}
pub(super) fn heading_rule(width: Val) -> impl Bundle {
    (
        Node {
            width,
            max_width: percent(100),
            height: px(2),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundGradient(vec![LinearGradient::to_right(vec![
            ColorStop::percent(ACCENT.with_alpha(0.18), 0),
            ColorStop::percent(CRIMSON_BRIGHT, 44),
            ColorStop::percent(ACCENT.with_alpha(0.22), 73),
            ColorStop::percent(ACCENT.with_alpha(0.0), 100),
        ])
        .into()]),
    )
}

fn attach_panel_frames(
    mut commands: Commands,
    decor: Option<Res<SharedDecorMaterials>>,
    panels: Query<
        (Entity, Option<&UiDialogSurface>, Option<&OrnateFrame>),
        (
            Or<(
                With<UiPanel>,
                With<UiDialogSurface>,
                With<UiNoticePanel>,
                With<OrnateFrame>,
            )>,
            Without<HasOrnateFrame>,
        ),
    >,
) {
    let Some(decor) = decor else {
        return;
    };
    for (entity, is_dialog, is_ornate) in &panels {
        let handle = if is_dialog.is_some() || is_ornate.is_some() {
            decor.dialog_frame.clone()
        } else {
            decor.panel_frame.clone()
        };
        let frame = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    top: Val::Px(0.0),
                    bottom: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                MaterialNode(handle),
                FocusPolicy::Pass,
                ZIndex(-1),
            ))
            .id();
        commands
            .entity(entity)
            .insert(HasOrnateFrame)
            .add_child(frame);
    }
}

fn attach_ink_strokes(
    mut commands: Commands,
    mut materials: Option<ResMut<Assets<InkStrokeMaterial>>>,
    buttons: Query<
        (
            Entity,
            Option<&ActionStyle>,
            Option<&super::launcher_ui::HeroNavButton>,
        ),
        (
            Or<(
                With<ActionStyle>,
                With<super::launcher_ui::HeroNavButton>,
                With<super::ui_plugin::ResponsiveMenuButton>,
            )>,
            Without<HasInkStroke>,
        ),
    >,
) {
    let Some(ref mut materials) = materials else {
        return;
    };
    for (entity, action_style, hero_nav) in &buttons {
        let primary = hero_nav.is_some() || action_style.is_some_and(|s| s.primary);
        let seed = (entity.index().index() as f32 * 1.618_034) % 37.0;
        let base_color = if primary {
            PRIMARY.with_alpha(0.14).to_linear().to_vec4()
        } else {
            METAL.with_alpha(0.10).to_linear().to_vec4()
        };
        let material = materials.add(InkStrokeMaterial {
            stroke: InkStrokeUniform {
                color: base_color,
                params: Vec4::new(0.0, seed, 0.0, 0.0),
            },
        });
        let underlay = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    right: Val::Px(0.0),
                    top: Val::Px(2.0),
                    bottom: Val::Px(2.0),
                    ..default()
                },
                MaterialNode(material.clone()),
                FocusPolicy::Pass,
                ZIndex(-1),
                InkStrokeUnderlay {
                    button: entity,
                    material,
                    primary,
                    seed,
                    progress: 0.0,
                },
            ))
            .id();
        commands
            .entity(entity)
            .insert(HasInkStroke)
            .add_child(underlay);
    }
}

fn animate_ink_strokes(
    time: Option<Res<Time>>,
    mut materials: Option<ResMut<Assets<InkStrokeMaterial>>>,
    buttons: Query<&Interaction>,
    mut underlays: Query<&mut InkStrokeUnderlay>,
) {
    let Some(ref mut materials) = materials else {
        return;
    };
    let (dt, elapsed) = time.map_or((0.033, 0.0), |t| {
        (t.delta_secs().clamp(0.001, 0.1), t.elapsed_secs())
    });
    for mut underlay in &mut underlays {
        let interaction = buttons
            .get(underlay.button)
            .copied()
            .unwrap_or(Interaction::None);
        let target = if interaction != Interaction::None {
            1.0
        } else {
            0.0
        };
        let speed = if target > underlay.progress { 8.5 } else { 5.5 };
        underlay.progress += (target - underlay.progress).clamp(-dt * speed, dt * speed);
        if (underlay.progress - target).abs() < 0.01 {
            underlay.progress = target;
        }
        if let Some(mut mat) = materials.get_mut(&underlay.material) {
            mat.stroke.params = Vec4::new(underlay.progress, underlay.seed, elapsed, 0.0);
            mat.stroke.color = match (underlay.primary, interaction) {
                (true, Interaction::Pressed) => PRIMARY.with_alpha(0.20).to_linear().to_vec4(),
                (true, _) => PRIMARY.with_alpha(0.14).to_linear().to_vec4(),
                (false, Interaction::Pressed) => METAL.with_alpha(0.16).to_linear().to_vec4(),
                (false, _) => METAL.with_alpha(0.10).to_linear().to_vec4(),
            };
        }
    }
}

fn polish_typography(
    mut commands: Commands,
    mut texts: Query<
        (
            Entity,
            &TextFont,
            Option<&mut LetterSpacing>,
            Option<&TextShadow>,
        ),
        Added<TextFont>,
    >,
) {
    for (entity, font, spacing, shadow) in &mut texts {
        let shadow_color = SCRIM.with_alpha(0.90);
        let px = match font.font_size {
            FontSize::Px(px) => px,
            _ => continue,
        };
        if px >= 22.0 {
            if let Some(mut spacing) = spacing {
                if matches!(*spacing, LetterSpacing::Px(v) if v == 0.0) {
                    *spacing = LetterSpacing::Px(2.0);
                }
            }
            if shadow.is_none() {
                commands.entity(entity).insert(TextShadow {
                    offset: Vec2::new(1.5, 2.0),
                    color: shadow_color,
                });
            }
        } else if px >= 16.0 {
            if let Some(mut spacing) = spacing {
                if matches!(*spacing, LetterSpacing::Px(v) if v == 0.0) {
                    *spacing = LetterSpacing::Px(0.8);
                }
            }
            if shadow.is_none() {
                commands.entity(entity).insert(TextShadow {
                    offset: Vec2::new(1.0, 1.5),
                    color: shadow_color,
                });
            }
        }
    }
}

pub(crate) fn hero_gradient(selected: bool, pressed: bool) -> BackgroundGradient {
    if !selected && !pressed {
        return BackgroundGradient::default();
    }
    BackgroundGradient(vec![LinearGradient::new(
        LinearGradient::TO_RIGHT,
        vec![
            ColorStop::auto(SELECTED.with_alpha(if pressed { 1.0 } else { 0.90 })),
            ColorStop::auto(SELECTED.with_alpha(0.10)),
        ],
    )
    .into()])
}

pub(crate) fn action_gradient(color: Color, primary: bool) -> BackgroundGradient {
    let top = if primary {
        color.mix(&PRIMARY_HOVER, 0.16)
    } else {
        SURFACE
    };
    let bottom = if primary {
        color.mix(&PRIMARY_PRESSED, 0.20)
    } else {
        color.mix(&BACKGROUND, 0.22)
    };
    BackgroundGradient(vec![LinearGradient::new(
        LinearGradient::TO_BOTTOM,
        vec![ColorStop::auto(top), ColorStop::auto(bottom)],
    )
    .into()])
}

fn action_feedback(
    mut buttons: Query<(
        &ActionStyle,
        &Interaction,
        &mut BackgroundColor,
        &mut BorderColor,
        &mut BackgroundGradient,
        &mut Node,
    )>,
) {
    for (style, interaction, mut bg, mut border, mut gradient, mut node) in &mut buttons {
        let active = *interaction != Interaction::None;
        bg.0 = Color::NONE;
        *gradient = hero_gradient(active, *interaction == Interaction::Pressed);
        node.border = UiRect::bottom(Val::Px(1.0));
        node.border.left = Val::Px(2.0);
        node.border_radius = BorderRadius::ZERO;
        *border = BorderColor::all(if style.primary {
            ACCENT.with_alpha(0.52)
        } else {
            LINE
        });
        border.left = if active { CRIMSON_BRIGHT } else { Color::NONE };
        if active {
            border.bottom = ACCENT.with_alpha(0.75);
        }
    }
}

pub(crate) fn compact_actions(
    windows: Query<&Window>,
    scale: Option<Res<super::ui_scale_plugin::EffectiveUiScale>>,
    mut buttons: Query<
        (&mut Node, Option<&super::launcher_ui::NetworkRailAction>),
        With<CompactAction>,
    >,
) {
    let mobile = windows.single().is_ok_and(|w| {
        super::launcher_ui::responsive_ui_layout(w.width(), w.height(), cfg!(target_os = "android"))
            .mobile
    });
    let unit = scale.as_ref().map_or(1.0, |scale| scale.0.max(0.05));
    let height = if mobile { 44.0 / unit } else { 40.0 };
    for (mut node, rail) in &mut buttons {
        if rail.is_some() {
            node.width = Val::Px(if mobile { 120.0 / unit } else { 148.0 });
        }
        node.height = Val::Px(height);
        node.min_height = Val::Px(height);
        node.max_height = Val::Px(height);
        node.max_width = Val::Px(if mobile { 240.0 / unit } else { 200.0 });
        node.flex_grow = 0.0;
        node.flex_shrink = 0.0;
        node.align_self = AlignSelf::FlexStart;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_actions_clear_highlight_after_pointer_leaves() {
        let mut app = App::new();
        app.add_systems(Update, action_feedback);
        for style in [ActionStyle::PRIMARY, ActionStyle::SECONDARY] {
            let button = app
                .world_mut()
                .spawn((
                    Button,
                    style,
                    BackgroundColor(PRIMARY),
                    BorderColor::all(ACCENT),
                ))
                .id();
            for state in [
                Interaction::None,
                Interaction::Hovered,
                Interaction::Pressed,
                Interaction::Hovered,
                Interaction::None,
            ] {
                *app.world_mut().get_mut::<Interaction>(button).unwrap() = state;
                app.update();
                let active = state != Interaction::None;
                assert_eq!(
                    app.world().get::<BackgroundColor>(button).unwrap().0,
                    Color::NONE
                );
                assert_eq!(
                    app.world()
                        .get::<BackgroundGradient>(button)
                        .unwrap()
                        .0
                        .is_empty(),
                    !active
                );
                assert_eq!(
                    app.world().get::<BorderColor>(button).unwrap().left,
                    if active { CRIMSON_BRIGHT } else { Color::NONE }
                );
                assert_eq!(
                    app.world().get::<Node>(button).unwrap().border.top,
                    Val::Px(0.0)
                );
            }
        }
    }

    #[test]
    fn cinematic_art_loads_external_jpeg_and_crops_without_distortion() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .add_systems(Update, load_art);
        app.update();
        let art = app.world().resource::<CinematicArt>();
        assert_eq!(art.size, Vec2::new(768.0, 432.0));
        for view in [
            Vec2::new(2560.0, 1080.0),
            Vec2::new(393.0, 852.0),
            Vec2::new(899.0, 432.0),
        ] {
            let rect = cover_rect(art.size, view);
            assert!((rect.width() / rect.height() - view.x / view.y).abs() < 0.001);
            assert!(rect.min.cmpge(Vec2::splat(-0.01)).all());
            assert!(rect.max.cmple(art.size + Vec2::splat(0.01)).all());
        }
    }

    #[test]
    fn startup_and_room_use_the_same_backdrop_through_the_intro_fade() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<BackdropMaterial>>()
            .add_systems(Startup, load_art)
            .add_systems(Update, background_art);
        let startup = app
            .world_mut()
            .spawn((
                Node::default(),
                CinematicBackdrop,
                BackdropOpacity(1.0),
                BackdropIntroTime(0.0),
            ))
            .id();
        let room = app.world_mut().spawn((Node::default(), NetRoomRoot)).id();
        app.update();

        let backdrop = |world: &World, entity| {
            let handle = &world
                .get::<MaterialNode<BackdropMaterial>>(entity)
                .unwrap()
                .0;
            world
                .resource::<Assets<BackdropMaterial>>()
                .get(handle)
                .unwrap()
                .clone()
        };
        let startup_mat = backdrop(app.world(), startup);
        let room_mat = backdrop(app.world(), room);
        assert_eq!(startup_mat.image, room_mat.image);
        assert_eq!(startup_mat.backdrop.params.y, 1.0);
        assert_eq!(room_mat.backdrop.params.y, 1.0);
        assert!(startup_mat.backdrop.params.z > 0.0);
        assert!(startup_mat.backdrop.sweep.x < 0.4);
        assert!(startup_mat.backdrop.sweep.y > room_mat.backdrop.sweep.y);
        app.world_mut()
            .get_mut::<BackdropIntroTime>(startup)
            .unwrap()
            .0 = 1.2;
        app.update();
        assert!((0.8..1.0).contains(&backdrop(app.world(), startup).backdrop.sweep.x));
        app.world_mut()
            .get_mut::<BackdropIntroTime>(startup)
            .unwrap()
            .0 = 2.2;

        app.world_mut()
            .get_mut::<BackdropOpacity>(startup)
            .unwrap()
            .0 = 0.5;
        app.update();
        assert_eq!(backdrop(app.world(), startup).backdrop.params.y, 0.5);
        assert_eq!(backdrop(app.world(), room).backdrop.params.y, 1.0);
        assert_eq!(
            backdrop(app.world(), room).backdrop.sweep,
            room_mat.backdrop.sweep
        );
    }

    #[test]
    fn backdrop_shader_parses_and_validates_with_naga() {
        let source = include_str!("ui_backdrop.wgsl").replace(
            "#import bevy_ui::ui_vertex_output::UiVertexOutput",
            "struct UiVertexOutput { @location(0) uv: vec2<f32>, @location(1) size: vec2<f32>, };",
        );
        let module = naga::front::wgsl::parse_str(&source).expect("backdrop WGSL must parse");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("backdrop WGSL must validate");
    }
    #[test]
    fn ink_strokes_and_frames_attach_and_animate_cleanly() {
        let mut app = App::new();
        app.init_resource::<Assets<InkStrokeMaterial>>()
            .init_resource::<Assets<FrameMaterial>>()
            .add_systems(Startup, init_decor_materials)
            .add_systems(
                Update,
                (
                    attach_panel_frames,
                    attach_ink_strokes,
                    animate_ink_strokes,
                    polish_typography,
                )
                    .chain(),
            );
        let panel = app.world_mut().spawn((Node::default(), OrnateFrame)).id();
        let button = app
            .world_mut()
            .spawn((Button, ActionStyle::PRIMARY, Interaction::None))
            .id();
        let heading = app
            .world_mut()
            .spawn((
                Text::new("江湖舆图"),
                TextFont {
                    font_size: 26.0.into(),
                    ..default()
                },
            ))
            .id();

        app.update();
        assert!(app.world().get::<HasOrnateFrame>(panel).is_some());
        assert!(app.world().get::<HasInkStroke>(button).is_some());
        assert!(app.world().get::<TextShadow>(heading).is_some());

        *app.world_mut().get_mut::<Interaction>(button).unwrap() = Interaction::Hovered;
        for _ in 0..8 {
            app.update();
        }
        let underlay = app.world().get::<Children>(button).unwrap()[0];
        let progress = app
            .world()
            .get::<InkStrokeUnderlay>(underlay)
            .unwrap()
            .progress;
        assert!(progress > 0.5, "hovered button must sweep ink stroke in");
    }
}
