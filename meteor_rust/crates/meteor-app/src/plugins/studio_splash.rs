use crate::app_state::AppState;
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, TextureFormat};
use bevy::shader::{Shader, ShaderRef};
pub(crate) fn development_notice() -> String {
    format!(
        "早期开发版本，不代表最终成品 · {}",
        env!("METEOR_BUILD_VERSION")
    )
}

pub struct StudioSplashPlugin;
impl Plugin for StudioSplashPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(UiMaterialPlugin::<LogoMaterial>::default());
        bevy::asset::load_internal_asset!(
            app,
            LOGO_SHADER,
            "ui_studio_logo.wgsl",
            Shader::from_wgsl
        );
        app.add_systems(OnEnter(AppState::StudioSplash), setup)
            .add_systems(Update, animate);
    }
}
#[derive(Component)]
struct SplashRoot;
#[derive(Component)]
struct SplashLogo;
#[derive(Component)]
struct SplashPiece(usize);
const ASSEMBLE_END: f32 = 1.0;

const PIECES: [(f32, f32, f32); 6] = [
    (-65.0, -40.0, -28.0),
    (10.0, -65.0, 20.0),
    (65.0, -35.0, 32.0),
    (-60.0, 45.0, -22.0),
    (-10.0, 65.0, 25.0),
    (65.0, 40.0, -30.0),
];

const PIECE_SHAPES: [&[(f32, f32)]; 6] = [
    &[(0., 0.), (0.47, 0.), (0.47, 0.28), (0.42, 0.35), (0., 0.42)],
    &[
        (0.47, 0.),
        (1., 0.),
        (1., 0.44),
        (0.60, 0.44),
        (0.47, 0.41),
        (0.42, 0.35),
        (0.47, 0.28),
    ],
    &[
        (0., 0.42),
        (0.42, 0.35),
        (0.47, 0.41),
        (0.60, 0.44),
        (0.58, 0.60),
        (0.49, 0.57),
        (0.36, 0.43),
        (0., 0.52),
    ],
    &[
        (0., 0.52),
        (0.36, 0.43),
        (0.49, 0.57),
        (0.38, 0.75),
        (0., 1.),
    ],
    &[
        (0.38, 0.75),
        (0.49, 0.57),
        (0.58, 0.60),
        (0.66, 0.72),
        (0.60, 1.),
        (0., 1.),
    ],
    &[
        (0.60, 0.44),
        (1., 0.44),
        (1., 1.),
        (0.60, 1.),
        (0.66, 0.72),
        (0.58, 0.60),
    ],
];
fn inside_shape(x: f32, y: f32, polygon: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let mut previous = polygon[polygon.len() - 1];
    for &(px, py) in polygon {
        let (qx, qy) = previous;
        if (py > y) != (qy > y) && x < (qx - px) * (y - py) / (qy - py) + px {
            inside = !inside;
        }
        previous = (px, py);
    }
    inside
}
fn shaped_pieces(image: &Image) -> Vec<Image> {
    let width = image.width() as usize;
    let height = image.height() as usize;
    let mut pieces: Vec<Image> = (0..6)
        .map(|_| {
            let mut piece = image.clone();
            for pixel in piece.data.as_mut().unwrap().chunks_exact_mut(4) {
                pixel[3] = 0;
            }
            piece
        })
        .collect();
    for (index, pixel) in image.data.as_ref().unwrap().chunks_exact(4).enumerate() {
        if pixel[3] == 0 {
            continue;
        }
        let x = ((index % width) as f32 + 0.5) / width as f32;
        let y = ((index / width) as f32 + 0.5) / height as f32;
        let piece = PIECE_SHAPES
            .iter()
            .position(|shape| inside_shape(x, y, shape))
            .expect("shape partition covers every ink pixel");
        pieces[piece].data.as_mut().unwrap()[index * 4 + 3] = pixel[3];
    }
    pieces
}
const LOGO_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("d8360a36-69b9-40f5-9a58-668275023b5a");
#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct LogoMaterial {
    #[uniform(0)]
    params: Vec4,
    #[texture(1)]
    #[sampler(2)]
    image: Handle<Image>,
}
impl UiMaterial for LogoMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(LOGO_SHADER)
    }
}
#[derive(Component)]
struct SplashGlint;
#[derive(Component)]
struct SplashNotice;
#[derive(Resource, Default)]
struct IntroClock(f32);
const DISSOLVE_START: f32 = 1.4;
const DISSOLVE_DURATION: f32 = 0.55;
const MENU_START: f32 = DISSOLVE_START + DISSOLVE_DURATION;
const REVEAL_START: f32 = 2.0;
const DURATION: f32 = 2.4;
fn envelope(t: f32) -> f32 {
    (t / 0.45).clamp(0.0, 1.0) * ((MENU_START - t) / 0.15).clamp(0.0, 1.0)
}
fn logo_image() -> Image {
    let mut image = Image::from_buffer(
        &crate::resource_io::required("ui/studio-logo.png"),
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::RENDER_WORLD,
    )
    .expect("launcher logo must decode")
    .convert(TextureFormat::Rgba8UnormSrgb)
    .expect("logo converts to RGBA");

    if let Some(data) = image.data.as_mut() {
        for pixel in data.chunks_exact_mut(4) {
            let ink = 255 - pixel[0].min(pixel[1]).min(pixel[2]);

            let ink = (f32::from(ink) * 1.1 - 12.75).clamp(0.0, 255.0).round() as u16;
            let alpha = (ink * u16::from(pixel[3]) / 255) as u8;
            pixel.copy_from_slice(&[255, 255, 255, alpha]);
        }
    }
    image
}
fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<LogoMaterial>>,
    font: Res<super::launcher_ui::UiFont>,
    mut next: ResMut<NextState<AppState>>,
) {
    #[cfg(target_arch = "wasm32")]
    if web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("boot-loading"))
        .is_some()
    {
        next.set(AppState::Menu);
        return;
    }
    let _ = &mut next;
    commands.insert_resource(IntroClock::default());
    let image = logo_image();
    let fragments: Vec<_> = shaped_pieces(&image)
        .into_iter()
        .map(|piece| images.add(piece))
        .collect();
    let logo = images.add(image);
    commands
        .spawn((
            SplashRoot,
            GlobalZIndex(2000),
            bevy::ui::FocusPolicy::Block,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
        ))
        .with_children(|root| {
            root.spawn((
                SplashNotice,
                Text::new(development_notice()),
                TextFont {
                    font: font.0.clone().into(),
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(Color::srgb(0.65, 0.68, 0.70)),
                TextLayout::justify(Justify::Center),
                Node {
                    position_type: PositionType::Absolute,
                    bottom: percent(8),
                    max_width: percent(90),
                    ..default()
                },
            ));
            root.spawn(Node {
                width: Val::VMin(42.0),
                height: Val::VMin(42.0),
                max_width: px(280),
                max_height: px(280),
                ..default()
            })
            .with_children(|group| {
                group.spawn((
                    SplashLogo,
                    MaterialNode(materials.add(LogoMaterial {
                        image: logo.clone(),
                        params: Vec4::ZERO,
                    })),
                    Node {
                        width: percent(100),
                        height: percent(100),
                        position_type: PositionType::Absolute,
                        ..default()
                    },
                ));
                for index in 0..6 {
                    group.spawn((
                        SplashPiece(index),
                        ImageNode {
                            image: fragments[index].clone(),
                            color: Color::NONE,
                            ..default()
                        },
                        Node {
                            position_type: PositionType::Absolute,
                            width: percent(100),
                            height: percent(100),
                            ..default()
                        },
                    ));
                }
            });
            root.spawn((
                Node {
                    width: px(120),
                    height: px(2),
                    margin: UiRect::top(px(20)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(Color::NONE),
            ))
            .with_children(|track| {
                track.spawn((
                    SplashGlint,
                    Node {
                        position_type: PositionType::Absolute,
                        width: px(32),
                        height: px(2),
                        ..default()
                    },
                    BackgroundColor(Color::WHITE),
                ));
            });
        });
}
fn animate(
    mut commands: Commands,
    time: Res<Time<Real>>,
    state: Res<State<AppState>>,
    clock: Option<ResMut<IntroClock>>,
    mut next: ResMut<NextState<AppState>>,
    mut logos: Query<
        (&MaterialNode<LogoMaterial>, &mut UiTransform),
        (With<SplashLogo>, Without<SplashPiece>),
    >,
    mut pieces: Query<(&SplashPiece, &mut ImageNode, &mut UiTransform)>,
    mut materials: ResMut<Assets<LogoMaterial>>,
    mut glints: Query<(&mut Node, &mut BackgroundColor), (With<SplashGlint>, Without<SplashRoot>)>,
    mut roots: Query<(Entity, &mut BackgroundColor), With<SplashRoot>>,
    mut texts: Query<&mut TextColor, With<SplashNotice>>,
) {
    let Some(mut clock) = clock else {
        return;
    };

    clock.0 += time.delta_secs().min(0.05);
    let t = clock.0;
    let alpha = envelope(t);
    let progress = ((t - DISSOLVE_START) / DISSOLVE_DURATION).clamp(0.0, 1.0);

    let ease = progress.powi(3);
    for (handle, mut transform) in &mut logos {
        transform.scale = Vec2::splat(1.0 + 0.85 * ease);
        if let Some(mut material) = materials.get_mut(&handle.0) {
            material.params = Vec4::new(
                if t < ASSEMBLE_END { 0.0 } else { 1.0 - ease },
                0.4 * ease / (1.0 + 0.85 * ease),
                0.0,
                0.0,
            );
        }
    }
    for (piece, mut image, mut transform) in &mut pieces {
        let (x, y, angle) = PIECES[piece.0];
        let p = ((t - piece.0 as f32 * 0.03) / 0.85).clamp(0.0, 1.0);
        let remaining = (1.0 - p).powi(5);
        transform.translation = Val2::percent(x * remaining, y * remaining);
        transform.rotation = Rot2::degrees(angle * remaining);
        transform.scale = Vec2::splat(1.0 - 0.18 * remaining);
        image.color = Color::srgba(
            1.0,
            1.0,
            1.0,
            if t < ASSEMBLE_END {
                (t / 0.08).min(1.0)
            } else {
                0.0
            },
        );
    }
    for mut color in &mut texts {
        color
            .0
            .set_alpha(((DISSOLVE_START + 0.18 - t) / 0.18).clamp(0.0, 1.0));
    }
    for (mut node, mut color) in &mut glints {
        node.left = px(-32.0 + (t / 1.4).fract() * 184.0);
        color.0 = Color::srgba(0.85, 0.80, 0.69, alpha);
    }
    if t >= MENU_START && *state.get() == AppState::StudioSplash {
        next.set(AppState::Menu);
    }
    for (entity, mut color) in &mut roots {
        let reveal = ((t - REVEAL_START) / (DURATION - REVEAL_START)).clamp(0.0, 1.0);
        color
            .0
            .set_alpha(1.0 - reveal * reveal * (3.0 - 2.0 * reveal));
        if t >= DURATION || !matches!(state.get(), AppState::StudioSplash | AppState::Menu) {
            commands.entity(entity).despawn();
            commands.remove_resource::<IntroClock>();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logo_fragment_validates_with_ui_vertex_layout() {
        let source = include_str!("ui_studio_logo.wgsl").replace(
            "#import bevy_ui::ui_vertex_output::UiVertexOutput",
            "struct UiVertexOutput {
                @location(0) uv: vec2<f32>,
                @location(1) border_widths: vec4<f32>,
                @location(2) border_radius: vec4<f32>,
                @location(3) @interpolate(flat) size: vec2<f32>,
                @builtin(position) position: vec4<f32>,
            };",
        );
        let module = naga::front::wgsl::parse_str(&source).expect("logo shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("logo shader validates with the Bevy UI inputs");
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn shaped_fragments_reassemble_without_missing_or_duplicate_ink() {
        let image = logo_image();
        let pieces = shaped_pieces(&image);
        let mut counts = [0usize; 6];
        for (index, original) in image.data.as_ref().unwrap().chunks_exact(4).enumerate() {
            let mut sum = 0u16;
            for (part, piece) in pieces.iter().enumerate() {
                let alpha = piece.data.as_ref().unwrap()[index * 4 + 3];
                sum += u16::from(alpha);
                if alpha > 0 {
                    counts[part] += 1;
                }
            }
            assert_eq!(sum, u16::from(original[3]));
        }
        assert!(counts.iter().all(|count| *count > 100));
    }
    #[cfg(feature = "original-assets")]
    #[test]
    fn embedded_logo_has_white_ink_and_transparent_background() {
        let image = logo_image();
        let width = image.width() as usize;
        let height = image.height() as usize;
        let data = image.data.unwrap();
        assert!(data.chunks_exact(4).all(|p| p[..3] == [255, 255, 255]));
        assert!(data.chunks_exact(4).any(|p| p[3] == 0));
        assert!(data.chunks_exact(4).any(|p| p[3] > 250));
        assert!(data.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255));

        for (index, pixel) in data.chunks_exact(4).enumerate() {
            let (x, y) = (index % width, index / width);
            if x < width / 10 || x >= width * 9 / 10 || y < height / 10 || y >= height * 9 / 10 {
                assert_eq!(pixel[3], 0, "visible background at ({x}, {y})");
            }
        }
    }
    #[cfg(feature = "original-assets")]
    #[test]
    fn intro_enters_menu_cleans_up_and_does_not_replay() {
        use bevy::state::app::StatesPlugin;
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            StatesPlugin,
        ))
        .init_asset::<Shader>()
        .insert_resource(Assets::<Image>::default())
        .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f32(1.0 / 60.0),
        ))
        .insert_resource(Assets::<Font>::default())
        .add_plugins(super::super::launcher_ui::UiFontPlugin)
        .insert_state(AppState::StudioSplash)
        .add_plugins(StudioSplashPlugin);

        let font = app
            .world()
            .resource::<super::super::launcher_ui::UiFont>()
            .0
            .clone();
        assert!(app.world().resource::<Assets<Font>>().get(&font).is_some());
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::StudioSplash
        );
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<SplashRoot>>()
                .iter(app.world())
                .count(),
            1
        );
        for _ in 0..30 {
            app.update();
        }
        let mut pieces = app
            .world_mut()
            .query::<(&SplashPiece, &ImageNode, &UiTransform)>();
        assert_eq!(pieces.iter(app.world()).count(), 6);
        for (_, image, transform) in pieces.iter(app.world()) {
            assert!(image.color.alpha() > 0.0);
            assert_ne!(transform.rotation, Rot2::IDENTITY);
        }
        for material in app.world().resource::<Assets<LogoMaterial>>().iter() {
            assert_eq!(
                material.1.params.x, 0.0,
                "whole logo stays hidden during assembly"
            );
        }
        for _ in 0..40 {
            app.update();
        }
        for (_, image, _) in pieces.iter(app.world()) {
            assert_eq!(image.color.alpha(), 0.0);
        }
        for material in app.world().resource::<Assets<LogoMaterial>>().iter() {
            assert_eq!(
                material.1.params.x, 1.0,
                "assembled logo holds before dissolving"
            );
            assert_eq!(material.1.params.y, 0.0);
        }
        for _ in 0..62 {
            app.update();
        }

        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Menu
        );
        assert!(app.world().contains_resource::<IntroClock>());
        for handle in app
            .world_mut()
            .query_filtered::<&MaterialNode<LogoMaterial>, With<SplashLogo>>()
            .iter(app.world())
        {
            let material = app
                .world()
                .resource::<Assets<LogoMaterial>>()
                .get(&handle.0)
                .unwrap();
            assert_eq!(material.params.x, 0.0);
            assert!((material.params.y - 0.4 / 1.85).abs() < 0.0001);
        }
        let alpha = app
            .world_mut()
            .query_filtered::<&BackgroundColor, With<SplashRoot>>()
            .single(app.world())
            .unwrap()
            .0
            .alpha();
        assert!(alpha > 0.0 && alpha < 1.0);
        for _ in 0..80 {
            app.update();
        }
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Menu
        );
        assert!(!app.world().contains_resource::<IntroClock>());
        assert_eq!(
            app.world_mut()
                .query_filtered::<Entity, With<SplashRoot>>()
                .iter(app.world())
                .count(),
            0
        );
        for state in [AppState::Loading, AppState::Menu] {
            app.world_mut()
                .resource_mut::<NextState<AppState>>()
                .set(state);
            app.update();
        }
        assert!(!app.world().contains_resource::<IntroClock>());
    }
    #[test]
    fn intro_fades_in_and_out() {
        assert_eq!(envelope(0.0), 0.0);
        assert_eq!(envelope(1.0), 1.0);
        assert_eq!(envelope(DURATION), 0.0);
    }
}
