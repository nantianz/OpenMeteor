use super::*;
use crate::{
    app_state::{AppState, LoadedStageInfo},
    components::fighter::Fighter,
    plugins::{stage_collider::StageCollider, stage_material::FrameUniforms},
};
use bevy::render::render_resource::ShaderType;
use meteor_physics::collision::MeshCollider;

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct GraphicsFrame {
    pub sky: Vec4,
    pub ground: Vec4,
    pub sun: Vec4,
    pub sun_color: Vec4,

    pub environment: Vec4,
    pub fog_bounds: [Vec4; 8],
    pub fog_colors: [Vec4; 8],
    pub contacts: [Vec4; 16],
    pub contact_normals: [Vec4; 16],

    pub counts: Vec4,

    pub water_wind: Vec4,
    pub water_wind_cross: Vec4,
}

impl GraphicsFrame {
    pub(crate) fn grass_light(&self) -> Vec4 {
        if self.counts.w <= 0.5 {
            return Vec4::ONE;
        }
        let diffuse = self.sky.truncate() * 1.45;
        let sun = self.sun_color.truncate() * (self.sun.w * self.sun.y.max(0.0) * 0.45);
        ((diffuse + sun) * self.environment.z)
            .clamp(Vec3::ZERO, Vec3::splat(1.4))
            .extend(1.0)
    }
}
pub(super) fn update_frame(
    packs: Res<GraphicsPacks>,
    settings: Option<Res<crate::app_state::Settings>>,
    loaded: Option<Res<LoadedStageInfo>>,
    state: Option<Res<State<AppState>>>,
    collider: Option<Res<StageCollider>>,
    fighters: Query<
        (
            Entity,
            &GlobalTransform,
            Option<&crate::components::fighter::Concealment>,
            Option<&crate::components::fighter::Health>,
        ),
        With<Fighter>,
    >,
    camera: Query<&GlobalTransform, With<crate::plugins::camera_plugin::GameplayCamera>>,
    grass: Query<(), With<super::grass::GrassField>>,
    sky: Query<&super::sky::CloudSky>,
    mut frame: Option<ResMut<FrameUniforms>>,
) {
    let Some(frame) = frame.as_deref_mut() else {
        return;
    };
    frame.graphics = GraphicsFrame::default();
    if state.is_none_or(|s| !matches!(s.get(), AppState::Playing | AppState::Paused)) {
        return;
    }
    let Some(loaded) = loaded else {
        return;
    };
    if let Some((wind, response)) = packs
        .wind(&loaded.stage_id)
        .and_then(|wind| wind.response.map(|response| (wind, response.water)))
    {
        let travel = -wind_travel(wind, frame.seconds, response) * 0.035;
        let gain = (wind_at(wind, Vec3::ZERO, frame.seconds).length() * response).min(1.0);
        let cross = bevy::math::DVec2::new(
            travel.x * 0.54 - travel.y * 0.84,
            travel.x * 0.84 + travel.y * 0.54,
        ) * 1.47;
        let wrap = |phase: bevy::math::DVec2| {
            Vec2::new(
                phase.x.rem_euclid(1.0) as f32,
                phase.y.rem_euclid(1.0) as f32,
            )
        };
        frame.graphics.water_wind = wrap(travel).extend(gain).extend(1.0);
        frame.graphics.water_wind_cross = wrap(cross).extend(0.0).extend(0.0);
    }
    frame.graphics.counts.z =
        f32::from(!grass.is_empty() && settings.as_ref().is_none_or(|s| s.grass_enhance));
    let mut environment = None;
    let mut fog = None;
    let mut contact = None;
    for (_, s) in packs.scenes(&loaded.stage_id) {
        if s.environment.is_some() {
            environment = s.environment.as_ref();
        }
        if s.fog.is_some() {
            fog = s.fog.as_ref();
        }
        if s.contact_shadows.is_some() {
            contact = s.contact_shadows.as_ref();
        }
    }
    if let Some(e) = environment {
        frame.graphics.sky = Vec3::from_array(e.sky).extend(0.);
        frame.graphics.ground = Vec3::from_array(e.ground).extend(0.);
        frame.graphics.sun = Vec3::from_array(e.sun_direction)
            .normalize()
            .extend(e.sun_intensity);
        frame.graphics.sun_color = Vec3::from_array(e.sun_color).extend(0.);
        frame.graphics.environment = Vec4::new(
            e.intensity,
            e.rotation_degrees.to_radians(),
            e.exposure,
            e.specular_mips.len().saturating_sub(1) as f32,
        );
    } else if !packs.packs.is_empty() {
        let e = config::Environment::default();
        frame.graphics.sky = Vec3::from_array(e.sky).extend(0.);
        frame.graphics.ground = Vec3::from_array(e.ground).extend(0.);
        frame.graphics.sun = Vec3::from_array(e.sun_direction)
            .normalize()
            .extend(e.sun_intensity);
        frame.graphics.sun_color = Vec3::from_array(e.sun_color).extend(0.);
        frame.graphics.environment = Vec4::new(e.intensity, 0., e.exposure, 0.);
    }
    if let Some(light) = sky.iter().find_map(|sky| sky.0) {
        frame.graphics.sky = light.sky.extend(0.0);
        frame.graphics.ground = light.ground.extend(0.0);
        frame.graphics.sun = light.sun;
        frame.graphics.sun_color = light.sun_color.extend(0.0);
        frame.graphics.counts.w = 1.0;
    }
    if let Some(fog) = fog.filter(|_| settings.as_ref().is_none_or(|s| s.volumetric_fog)) {
        frame.graphics.counts.x = fog.len() as f32;
        for (i, f) in fog.iter().enumerate() {
            frame.graphics.fog_bounds[i] = Vec3::from_array(f.center).extend(f.radius);
            frame.graphics.fog_colors[i] = Vec3::from_array(f.color).extend(f.density);
        }
    }
    if let Some(c) = contact {
        let eye = camera
            .iter()
            .next()
            .map_or(Vec3::ZERO, GlobalTransform::translation);
        let mut actors: Vec<_> = fighters
            .iter()
            .filter(|(_, _, c, h)| {
                c.is_none_or(|c| c.remaining <= 0.0) && h.is_none_or(|h| h.current > 0.0)
            })
            .map(|(id, t, _, _)| (id, t.translation()))
            .filter(|(_, p)| p.is_finite() && p.distance(eye) <= c.max_distance)
            .collect();
        actors.sort_by(|(a, p), (b, q)| {
            p.distance_squared(eye)
                .total_cmp(&q.distance_squared(eye))
                .then(a.to_bits().cmp(&b.to_bits()))
        });
        if let Some(mesh) = collider.as_ref().and_then(|c| c.mesh.as_ref()) {
            let mut count = 0;
            for (_, p) in actors {
                let Some(hit) = mesh.sample_ground_hit(p.x, p.z, p.y - c.max_height, p.y + 0.1)
                else {
                    continue;
                };
                let height = p.y - hit.y;
                if !(0.0..=c.max_height).contains(&height) || hit.ny < 0.42 {
                    continue;
                }
                let strength = c.strength * (1.0 - height / c.max_height);
                frame.graphics.contacts[count] =
                    Vec3::new(p.x, hit.y, p.z).extend(c.radius * (1.0 + height * 0.25));
                frame.graphics.contact_normals[count] =
                    Vec3::new(hit.nx, hit.ny, hit.nz).extend(strength);
                count += 1;
                if count == 16 {
                    break;
                }
            }
            frame.graphics.counts.y = count as f32;
        }
    }
}
