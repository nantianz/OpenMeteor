pub(crate) mod config;
mod loader;
mod runtime;
mod surfaces;
use bevy::prelude::*;
#[cfg(test)]
pub(crate) use loader::load_with_reader;
pub(crate) use loader::GraphicsPacks;
pub(crate) use runtime::GraphicsFrame;
pub(crate) use surfaces::SceneGraphics;

pub fn install(app: &mut App) {
    app.init_resource::<GraphicsPacks>()
        .add_systems(Startup, show_error)
        .add_systems(
            PostUpdate,
            (runtime::update_frame, surfaces::update_wind)
                .after(bevy::transform::TransformSystems::Propagate),
        );
}
fn show_error(
    mut commands: Commands,
    packs: Res<GraphicsPacks>,
    font: Res<crate::plugins::launcher_ui::UiFont>,
) {
    if let Some(error) = &packs.error {
        commands.spawn((
            GlobalZIndex(4000),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100), height: percent(100),
                padding: UiRect::all(px(40)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgb(0.04, 0.04, 0.05)),
        )).with_children(|root| {
            root.spawn((
                Text::new(format!("图形增强包加载失败\n{error}\n\n修正配置，或将 graphics-enhancement.json 的 enabled 改为 false 后重启。")),
                TextFont { font: font.0.clone().into(), font_size: 18.0.into(), ..default() },
                TextColor(Color::srgb(0.95, 0.8, 0.6)),
            ));
        });
    }
}
pub fn wind_at(wind: &config::Wind, position: Vec3, seconds: f64) -> Vec2 {
    let direction = Vec2::from_array(wind.direction).normalize_or_zero();
    let phase = seconds * wind.frequency as f64
        + f64::from((position.x + position.z * 0.73) * wind.spatial_scale);
    let wave = phase.rem_euclid(std::f64::consts::TAU).sin() * 0.7
        + (phase * 1.91).rem_euclid(std::f64::consts::TAU).sin() * 0.3;
    direction * wind.strength * (1.0 + wind.gust * wave as f32)
}
#[cfg(test)]
mod tests;
