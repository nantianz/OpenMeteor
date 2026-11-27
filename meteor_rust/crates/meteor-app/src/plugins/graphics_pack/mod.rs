pub(super) mod campfires;
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
mod scene_actors;
pub(crate) mod sky;
pub(super) mod snow;
mod surfaces;
pub(super) mod waterfalls;
mod weapons;
use crate::app_state::AppState;
use bevy::picking::Pickable;
use bevy::prelude::*;
#[cfg(test)]
pub(crate) use loader::load_with_reader;
pub(crate) use loader::GraphicsPacks;
pub(crate) use runtime::GraphicsFrame;
pub(crate) use surfaces::SceneGraphics;

pub fn install(app: &mut App) {
    sky::install(app);
    grass::install(app);
    snow::install(app);
    color_pipeline::install(app);
    effects::install(app);
    fx::install(app);
    campfires::install(app);
    app.init_resource::<GraphicsPacks>()
        .add_systems(Update, show_error)
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
pub(super) const FAILURE_NOTICE: &str = "图形增强包加载失败，已关闭。";
#[derive(Component)]
struct PackFailureNotice {
    expires_at: f64,
}
fn show_error(
    mut commands: Commands,
    packs: Res<GraphicsPacks>,
    font: Res<crate::plugins::launcher_ui::UiFont>,
    state: Res<State<AppState>>,
    time: Res<Time<Real>>,
    notices: Query<(Entity, &PackFailureNotice)>,
    mut shown: Local<bool>,
) {
    let now = time.elapsed_secs_f64();
    for (entity, notice) in &notices {
        if now >= notice.expires_at {
            commands.entity(entity).despawn();
        }
    }
    if *shown || packs.error.is_none() || !matches!(state.get(), AppState::Menu | AppState::Playing)
    {
        return;
    }
    *shown = true;
    commands
        .spawn((
            PackFailureNotice {
                expires_at: now + 7.0,
            },
            GlobalZIndex(4000),
            Pickable::IGNORE,
            bevy::ui::FocusPolicy::Pass,
            Node {
                position_type: PositionType::Absolute,
                top: px(24),
                left: px(16),
                right: px(16),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                super::launcher_ui::launcher_text(
                    FAILURE_NOTICE,
                    14.0,
                    super::ui_theme::ACCENT_TEXT,
                    &font.0,
                ),
                Pickable::IGNORE,
                bevy::ui::FocusPolicy::Pass,
                TextLayout::justify(Justify::Center),
                Node {
                    padding: UiRect::axes(px(16), px(8)),
                    ..default()
                },
                BackgroundColor(super::ui_theme::SURFACE),
            ));
        });
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
