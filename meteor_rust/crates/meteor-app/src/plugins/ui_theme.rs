use crate::app_state::{MenuRoot, NetRoomRoot, RoomRoot, SettingsRoot};
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::{Shader, ShaderRef};

pub const BACKGROUND: Color = Color::srgb(0.055, 0.063, 0.078);
pub const SURFACE: Color = Color::srgba(0.082, 0.094, 0.114, 0.94);
pub const CARD: Color = Color::srgba(0.105, 0.118, 0.137, 0.88);
pub const HOVER: Color = Color::srgb(0.18, 0.19, 0.21);
pub const SELECTED: Color = Color::srgba(0.31, 0.10, 0.11, 0.94);
pub const ACCENT: Color = Color::srgb(0.70, 0.20, 0.19);
pub const ACCENT_TEXT: Color = Color::srgb(0.91, 0.49, 0.43);
pub const PRIMARY: Color = Color::srgb(0.48, 0.105, 0.11);
pub const PRIMARY_HOVER: Color = Color::srgb(0.62, 0.15, 0.15);
pub const PRIMARY_PRESSED: Color = Color::srgb(0.37, 0.065, 0.075);
pub const TEXT: Color = Color::srgb(0.94, 0.93, 0.90);
pub const MUTED: Color = Color::srgb(0.65, 0.68, 0.71);
pub const METAL: Color = Color::srgb(0.80, 0.74, 0.60);
pub const LINE: Color = Color::srgba(0.62, 0.65, 0.68, 0.28);
pub const SCRIM: Color = Color::srgba(0.02, 0.025, 0.035, 0.80);
pub const HUD_SURFACE: Color = Color::srgba(0.055, 0.063, 0.078, 0.84);
pub const HEALTH: Color = Color::srgb(0.73, 0.20, 0.21);
pub const ANGER: Color = Color::srgb(0.72, 0.58, 0.30);

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
#[derive(Resource)]
struct CalligraphyArt(Handle<CalligraphyMaterial>);
#[derive(Component)]
pub(crate) struct CalligraphyTitle;

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
        app.add_plugins(UiMaterialPlugin::<CalligraphyMaterial>::default());
        bevy::asset::load_internal_asset!(
            app,
            TITLE_SHADER,
            "ui_calligraphy.wgsl",
            Shader::from_wgsl
        );
        app.add_systems(PreStartup, (load_art, load_title))
            .add_systems(
                Update,
                (
                    background_art,
                    attach_title,
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

fn load_art(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    match Image::from_buffer(
        &crate::resource_io::required("ui/cinematic-fortress.png"),
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
        Err(error) => error!("cinematic UI background failed to decode: {error}"),
    }
}

pub(crate) fn cover_rect(image: Vec2, viewport: Vec2) -> Rect {
    let scale = (viewport.x / image.x).max(viewport.y / image.y).max(0.001);
    Rect::from_center_size(image * 0.5, viewport / scale)
}

fn background_art(
    mut commands: Commands,
    art: Option<Res<CinematicArt>>,
    windows: Query<&Window>,
    mut roots: Query<
        (Entity, Option<&mut ImageNode>),
        Or<(
            With<MenuRoot>,
            With<NetRoomRoot>,
            With<RoomRoot>,
            With<SettingsRoot>,
        )>,
    >,
) {
    let Some(art) = art else {
        return;
    };
    let size = windows.single().map_or(Vec2::new(1280.0, 720.0), |w| {
        Vec2::new(w.width(), w.height())
    });
    let rect = Some(cover_rect(art.size, size));
    for (entity, image) in &mut roots {
        if let Some(mut image) = image {
            if image.rect != rect {
                image.rect = rect;
            }
        } else {
            commands.entity(entity).insert(ImageNode {
                image: art.image.clone(),
                rect,
                ..default()
            });
        }
    }
}

pub(crate) fn hero_gradient(selected: bool, pressed: bool) -> BackgroundGradient {
    if !selected && !pressed {
        return BackgroundGradient::default();
    }
    let start = if pressed {
        PRIMARY_PRESSED
    } else if selected {
        PRIMARY_HOVER.with_alpha(0.85)
    } else {
        Color::srgba(0.045, 0.055, 0.07, 0.4)
    };
    BackgroundGradient(vec![LinearGradient::new(
        LinearGradient::TO_RIGHT,
        vec![
            ColorStop::new(start, Val::Percent(0.0)),
            ColorStop::new(
                if selected {
                    PRIMARY.with_alpha(0.45)
                } else {
                    Color::srgba(0.04, 0.05, 0.065, 0.2)
                },
                Val::Percent(55.0),
            ),
            ColorStop::new(Color::NONE, Val::Percent(100.0)),
        ],
    )
    .into()])
}

pub(crate) fn action_gradient(color: Color, primary: bool) -> BackgroundGradient {
    let top = if primary {
        color.mix(&ACCENT_TEXT, 0.08)
    } else {
        Color::srgba(0.16, 0.18, 0.20, 0.55)
    };
    let bottom = if primary {
        color.mix(&BACKGROUND, 0.18)
    } else {
        Color::srgba(0.04, 0.05, 0.065, 0.8)
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
            METAL.with_alpha(0.4)
        } else {
            LINE
        });
        border.left = if active { ACCENT_TEXT } else { Color::NONE };
        if active {
            border.bottom = ACCENT.with_alpha(0.65);
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

    #[cfg(feature = "original-assets")]
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
}
