use crate::app_state::{MenuRoot, NetRoomRoot, RoomRoot, SettingsRoot};
use crate::plugins::ui_components::{UiDialogSurface, UiNoticePanel, UiPanel};
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::{Shader, ShaderRef};
use bevy::text::{FontSize, LetterSpacing};
use bevy::ui::{widget::TextShadow, FocusPolicy};

pub const BACKGROUND: Color = Color::srgb(0.032, 0.030, 0.028);
pub const SURFACE: Color = Color::srgba(0.045, 0.042, 0.038, 0.94);
pub const CARD: Color = Color::srgba(0.078, 0.071, 0.064, 0.90);
pub const HOVER: Color = Color::srgb(0.17, 0.14, 0.11);
pub const SELECTED: Color = Color::srgba(0.26, 0.14, 0.11, 0.94);
pub const ACCENT: Color = Color::srgb(0.72, 0.57, 0.35);
pub const ACCENT_TEXT: Color = Color::srgb(0.93, 0.84, 0.65);
pub const PRIMARY: Color = Color::srgb(0.46, 0.13, 0.11);
pub const PRIMARY_HOVER: Color = Color::srgb(0.62, 0.17, 0.14);
pub const PRIMARY_PRESSED: Color = Color::srgb(0.31, 0.08, 0.07);
pub const CRIMSON: Color = Color::srgb(0.68, 0.15, 0.13);
pub const CRIMSON_BRIGHT: Color = Color::srgb(0.88, 0.24, 0.19);
pub const TEXT: Color = Color::srgb(0.95, 0.93, 0.88);
pub const MUTED: Color = Color::srgb(0.64, 0.62, 0.58);
pub const METAL: Color = Color::srgb(0.82, 0.72, 0.53);
pub const LINE: Color = Color::srgba(0.76, 0.66, 0.48, 0.26);
pub const SCRIM: Color = Color::srgba(0.015, 0.013, 0.011, 0.84);
pub const HUD_SURFACE: Color = Color::srgba(0.035, 0.033, 0.032, 0.86);
pub const HEALTH: Color = Color::srgb(0.78, 0.16, 0.15);
pub const ANGER: Color = Color::srgb(0.82, 0.64, 0.28);

#[derive(Component)]
pub(crate) struct CinematicBackdrop;

#[derive(Component)]
pub(super) struct BackdropOpacity(pub f32);

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

const TITLE_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("f4aa72cc-64d8-49a8-8c7e-ec01958cfbb8");
const INK_STROKE_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("92c1e6e1-7840-4d9d-9f4e-77d8102a5b11");
const ATMOSPHERE_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("c381a847-2a19-46bf-850c-63f1e5a10a22");
const FRAME_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("5d47e129-9b34-4412-b89f-19e7c2d08f33");

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
pub struct AtmosphereUniform {
    pub ember: Vec4,
    pub params: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct AtmosphereMaterial {
    #[uniform(0)]
    pub atmo: AtmosphereUniform,
}
impl UiMaterial for AtmosphereMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(ATMOSPHERE_SHADER)
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
    atmosphere: Handle<AtmosphereMaterial>,
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

#[derive(Component)]
struct HasAtmosphere;

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

fn init_decor_materials(
    mut commands: Commands,
    mut atmospheres: ResMut<Assets<AtmosphereMaterial>>,
    mut frames: ResMut<Assets<FrameMaterial>>,
) {
    let atmosphere = atmospheres.add(AtmosphereMaterial {
        atmo: AtmosphereUniform {
            ember: Vec4::new(0.92, 0.36, 0.16, 0.78),
            params: Vec4::new(0.0, 0.85, 1.0, 0.0),
        },
    });
    let panel_frame = frames.add(FrameMaterial {
        frame: FrameUniform {
            color: Vec4::new(0.82, 0.72, 0.53, 0.72),
            params: Vec4::new(18.0, 0.85, 0.0, 0.0),
        },
    });
    let dialog_frame = frames.add(FrameMaterial {
        frame: FrameUniform {
            color: Vec4::new(0.88, 0.77, 0.56, 0.88),
            params: Vec4::new(24.0, 1.0, 1.0, 0.0),
        },
    });
    commands.insert_resource(SharedDecorMaterials {
        atmosphere,
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
            UiMaterialPlugin::<AtmosphereMaterial>::default(),
            UiMaterialPlugin::<FrameMaterial>::default(),
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
        bevy::asset::load_internal_asset!(
            app,
            ATMOSPHERE_SHADER,
            "ui_atmosphere.wgsl",
            Shader::from_wgsl
        );
        bevy::asset::load_internal_asset!(app, FRAME_SHADER, "ui_frame.wgsl", Shader::from_wgsl);
        app.add_systems(PreStartup, (load_art, load_title, init_decor_materials))
            .add_systems(
                Update,
                (
                    background_art,
                    attach_atmosphere,
                    animate_atmosphere,
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

pub(super) fn load_art(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    match Image::from_buffer(
        include_bytes!("../../assets/ui/ink-wash-background.png"),
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    ) {
        Ok(image) => {
            let size = image.size().as_vec2();
            commands.insert_resource(CinematicArt {
                image: images.add(image),
                size,
            });
        }
        Err(error) => error!("ink wash background failed to decode: {error}"),
    }
}

pub(crate) fn cover_rect(image: Vec2, viewport: Vec2) -> Rect {
    let scale = (viewport.x / image.x).max(viewport.y / image.y).max(0.001);
    Rect::from_center_size(image * 0.5, viewport / scale)
}

pub(super) fn background_art(
    mut commands: Commands,
    art: Option<Res<CinematicArt>>,
    windows: Query<&Window>,
    mut roots: Query<
        (
            Entity,
            Option<&mut ImageNode>,
            Option<&BackdropOpacity>,
            Option<&ComputedNode>,
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
    let size = windows.single().map_or(Vec2::new(1280.0, 720.0), |w| {
        Vec2::new(w.width(), w.height())
    });
    for (entity, image, opacity, computed) in &mut roots {
        let viewport = computed
            .map(ComputedNode::size)
            .filter(|size| size.cmpgt(Vec2::ZERO).all())
            .unwrap_or(size);
        let rect = Some(cover_rect(art.size, viewport));
        let tint = Color::srgba(0.29, 0.26, 0.23, opacity.map_or(1.0, |fade| fade.0));
        if let Some(mut image) = image {
            if image.image != art.image {
                image.image = art.image.clone();
            }
            if image.rect != rect {
                image.rect = rect;
            }
            if image.color != tint {
                image.color = tint;
            }
        } else {
            commands.entity(entity).insert(ImageNode {
                image: art.image.clone(),
                rect,
                color: tint,
                ..default()
            });
        }
    }
}

fn attach_atmosphere(
    mut commands: Commands,
    decor: Option<Res<SharedDecorMaterials>>,
    roots: Query<
        Entity,
        (
            Or<(
                With<MenuRoot>,
                With<NetRoomRoot>,
                With<RoomRoot>,
                With<SettingsRoot>,
                With<CinematicBackdrop>,
            )>,
            Without<HasAtmosphere>,
            Without<BackdropOpacity>,
        ),
    >,
) {
    let Some(decor) = decor else {
        return;
    };
    for root in &roots {
        let overlay = commands
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
                MaterialNode(decor.atmosphere.clone()),
                FocusPolicy::Pass,
                ZIndex(-1),
            ))
            .id();
        commands
            .entity(root)
            .insert(HasAtmosphere)
            .add_child(overlay);
    }
}

fn animate_atmosphere(
    time: Option<Res<Time>>,
    decor: Option<Res<SharedDecorMaterials>>,
    materials: Option<ResMut<Assets<AtmosphereMaterial>>>,
) {
    let (Some(time), Some(decor), Some(mut materials)) = (time, decor, materials) else {
        return;
    };
    if let Some(mut mat) = materials.get_mut(&decor.atmosphere) {
        mat.atmo.params.x = time.elapsed_secs();
    };
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
            Vec4::new(0.68, 0.14, 0.12, 0.90)
        } else {
            Vec4::new(0.44, 0.34, 0.21, 0.72)
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
                (true, Interaction::Pressed) => Vec4::new(0.84, 0.20, 0.16, 0.96),
                (true, _) => Vec4::new(0.68, 0.14, 0.12, 0.88),
                (false, Interaction::Pressed) => Vec4::new(0.62, 0.48, 0.28, 0.85),
                (false, _) => Vec4::new(0.44, 0.34, 0.21, 0.72),
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
                    color: Color::srgba(0.0, 0.0, 0.0, 0.85),
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
                    color: Color::srgba(0.0, 0.0, 0.0, 0.78),
                });
            }
        }
    }
}

pub(crate) fn hero_gradient(selected: bool, pressed: bool) -> BackgroundGradient {
    if !selected && !pressed {
        return BackgroundGradient::default();
    }
    let start = if pressed {
        PRIMARY_PRESSED.with_alpha(0.94)
    } else if selected {
        PRIMARY_HOVER.with_alpha(0.86)
    } else {
        Color::srgba(0.045, 0.042, 0.038, 0.4)
    };
    let mid = if pressed {
        PRIMARY.with_alpha(0.68)
    } else if selected {
        PRIMARY.with_alpha(0.50)
    } else {
        Color::srgba(0.04, 0.038, 0.035, 0.2)
    };
    BackgroundGradient(vec![LinearGradient::new(
        LinearGradient::TO_RIGHT,
        vec![
            ColorStop::new(start, Val::Percent(0.0)),
            ColorStop::new(mid, Val::Percent(52.0)),
            ColorStop::new(ACCENT.with_alpha(0.18), Val::Percent(82.0)),
            ColorStop::new(Color::NONE, Val::Percent(100.0)),
        ],
    )
    .into()])
}

pub(crate) fn action_gradient(color: Color, primary: bool) -> BackgroundGradient {
    let top = if primary {
        color.mix(&CRIMSON_BRIGHT, 0.16)
    } else {
        Color::srgba(0.13, 0.12, 0.10, 0.65)
    };
    let bottom = if primary {
        color.mix(&BACKGROUND, 0.28)
    } else {
        Color::srgba(0.04, 0.038, 0.035, 0.86)
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
            METAL.with_alpha(0.45)
        } else {
            LINE
        });
        border.left = if active { ACCENT_TEXT } else { Color::NONE };
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
                    if active { ACCENT_TEXT } else { Color::NONE }
                );
                assert_eq!(
                    app.world().get::<Node>(button).unwrap().border.top,
                    Val::Px(0.0)
                );
            }
        }
    }

    #[test]
    fn cinematic_art_is_embedded_and_crops_without_distortion() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .add_systems(Update, load_art);
        app.update();
        let art = app.world().resource::<CinematicArt>();
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
            .add_systems(Startup, load_art)
            .add_systems(Update, background_art);
        let startup = app
            .world_mut()
            .spawn((Node::default(), CinematicBackdrop, BackdropOpacity(1.0)))
            .id();
        let room = app.world_mut().spawn((Node::default(), NetRoomRoot)).id();
        app.update();

        let image = |world: &World, entity| world.get::<ImageNode>(entity).unwrap().clone();
        let startup_image = image(app.world(), startup);
        let room_image = image(app.world(), room);
        assert_eq!(startup_image.image, room_image.image);
        assert_eq!(startup_image.color, room_image.color);

        app.world_mut()
            .get_mut::<BackdropOpacity>(startup)
            .unwrap()
            .0 = 0.5;
        app.update();
        assert_eq!(
            image(app.world(), startup).color,
            room_image.color.with_alpha(0.5)
        );
        assert_eq!(image(app.world(), room).color, room_image.color);
    }

    #[test]
    fn ink_strokes_frames_and_atmosphere_attach_and_animate_cleanly() {
        let mut app = App::new();
        app.init_resource::<Assets<InkStrokeMaterial>>()
            .init_resource::<Assets<AtmosphereMaterial>>()
            .init_resource::<Assets<FrameMaterial>>()
            .add_systems(Startup, init_decor_materials)
            .add_systems(
                Update,
                (
                    attach_atmosphere,
                    attach_panel_frames,
                    attach_ink_strokes,
                    animate_ink_strokes,
                    polish_typography,
                )
                    .chain(),
            );
        let root = app.world_mut().spawn((Node::default(), MenuRoot)).id();
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
        assert!(app.world().get::<HasAtmosphere>(root).is_some());
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
