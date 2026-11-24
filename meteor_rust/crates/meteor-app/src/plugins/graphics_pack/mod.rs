pub(crate) mod cloth_binding;
pub(crate) mod cloth_obj;
mod color_pipeline;
pub(crate) mod config;
pub(crate) mod effects;
pub(crate) mod fx;
pub(crate) mod grass;
mod ground_cover;
mod loader;
pub(crate) mod meshlets;
mod runtime;
pub(crate) mod sky;
pub(super) mod snow;
mod surfaces;
pub(super) mod waterfalls;
mod weapons;
use crate::app_state::AppState;
use bevy::prelude::*;
#[cfg(test)]
pub(crate) use loader::load_with_reader;
pub(crate) use loader::{CatalogEntry, GraphicsPackCatalog, GraphicsPacks};
pub(crate) use runtime::GraphicsFrame;
pub(crate) use surfaces::SceneGraphics;

pub fn install(app: &mut App) {
    sky::install(app);
    grass::install(app);
    snow::install(app);
    color_pipeline::install(app);
    effects::install(app);
    fx::install(app);
    app.init_resource::<GraphicsPacks>()
        .add_systems(Startup, show_error)
        .add_systems(
            OnExit(AppState::Playing),
            release_stage_assets.run_if(super::ends_playing_session),
        )
        .add_systems(
            PostUpdate,
            (runtime::update_frame, surfaces::update_wind)
                .after(bevy::transform::TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            weapons::apply_weapon_materials.before(bevy::transform::TransformSystems::Propagate),
        );
}
fn release_stage_assets(mut packs: ResMut<GraphicsPacks>) {
    packs.packs.clear();
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
impl GraphicsPacks {
    pub(crate) fn wind<'a>(&'a self, stage: &'a str) -> Option<&'a config::Wind> {
        self.scenes(stage)
            .filter_map(|(_, scene)| scene.wind.as_ref())
            .last()
    }
}

pub(super) fn wind_flow(wind: &config::Wind, response: f32) -> Vec4 {
    let mean = Vec2::from_array(wind.direction).normalize_or_zero() * (wind.strength * response);
    Vec4::new(mean.x, mean.y, wind.gust, wind.frequency)
}

pub fn wind_at(wind: &config::Wind, position: Vec3, seconds: f64) -> Vec2 {
    let flow = wind_flow(wind, 1.0);
    let spatial = if wind.response.is_some() {
        -position
            .xz()
            .dot(Vec2::from_array(wind.direction).normalize_or_zero())
    } else {
        position.x + position.z * 0.73
    };
    let phase = seconds * wind.frequency as f64 + f64::from(spatial * wind.spatial_scale);
    let wave = phase.rem_euclid(std::f64::consts::TAU).sin() * 0.7
        + (phase * 1.91).rem_euclid(std::f64::consts::TAU).sin() * 0.3;
    flow.xy() * (1.0 + wind.gust * wave as f32)
}

fn wind_travel(wind: &config::Wind, seconds: f64, response: f32) -> bevy::math::DVec2 {
    wind_travel_for_flow(wind_flow(wind, response), seconds)
}

fn wind_travel_for_flow(flow: Vec4, seconds: f64) -> bevy::math::DVec2 {
    let rate = f64::from(flow.w);
    let integral = |frequency: f64| {
        if frequency == 0.0 {
            return 0.0;
        }
        2.0 * (0.5 * frequency * seconds).sin().powi(2) / frequency
    };
    let elapsed =
        seconds + f64::from(flow.z) * (0.7 * integral(rate) + 0.3 * integral(1.91 * rate));
    flow.xy().as_dvec2() * elapsed
}
#[cfg(test)]
mod tests;
