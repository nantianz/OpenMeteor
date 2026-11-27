mod particles;
mod props;
#[cfg(test)]
mod tests;

use super::{
    config::Campfire,
    fx::FxMaterial,
    scene_actors::{live_actors, SceneActors, BODY_HEIGHT},
    wind_at, GraphicsPacks,
};
use crate::{
    app_state::{AppState, LoadedStageInfo},
    plugins::{
        camera_plugin::GameplayCamera, render_plugin::PlayingSceneEntity,
        stage_collider::StageMeshCollider, FrameClock,
    },
};
use bevy::prelude::*;
use meteor_physics::collision::MeshCollider;

const BRAZIER: Vec3 = Vec3::new(1.45, 0.0, 0.30);

pub(in crate::plugins) struct Placement<'a> {
    profile: &'a Campfire,
    position: Vec3,
    rotation: Quat,
}

impl Placement<'_> {
    pub(super) fn clears_grass(&self, root: Vec3) -> bool {
        let center = self.position + self.rotation * (BRAZIER * self.profile.scale * 0.7);
        (root.y - center.y).abs() < 1.0
            && root.xz().distance_squared(center.xz()) < (2.28 * self.profile.scale).powi(2)
    }
}

pub(in crate::plugins) fn placements<'a>(
    packs: Option<&'a GraphicsPacks>,
    stage: &'a str,
    terrain: Option<&StageMeshCollider>,
) -> Vec<Placement<'a>> {
    let Some((profiles, terrain)) = packs
        .and_then(|packs| {
            packs
                .scenes(stage)
                .filter_map(|(_, s)| s.campfires.as_deref())
                .last()
        })
        .zip(terrain)
    else {
        return Vec::new();
    };
    profiles
        .iter()
        .filter_map(|profile| {
            let [x, y, z] = profile.position;
            let rotation = Quat::from_rotation_y(profile.yaw_degrees.to_radians());
            let offset = rotation * (BRAZIER * profile.scale);
            let ground = |x, z| {
                terrain
                    .sample_ground_hit(x, z, y, y + 0.8)
                    .filter(|hit| (hit.y - y).abs() < 3.0 && hit.ny >= 0.8)
            };
            let base = ground(x, z)?;
            let hearth = ground(x + offset.x, z + offset.z)?;

            if (base.y - hearth.y).abs() > 0.40 * profile.scale {
                return None;
            }
            Some(Placement {
                profile,
                position: Vec3::new(x, base.y.max(hearth.y) - 0.025, z),
                rotation,
            })
        })
        .collect()
}

#[derive(Component)]
pub(super) struct CampfireLight {
    elapsed: f32,
    phase: f32,
}

#[derive(Component)]
struct CampfireDraft {
    hearth: Vec3,
    scale: f32,
    flow: Vec2,
}

impl CampfireDraft {
    fn update(&mut self, origin: &GlobalTransform, actors: &SceneActors, dt: f32) {
        let hearth = origin.transform_point(self.hearth);
        let radius = 2.8 * self.scale;
        let incoming = live_actors(actors)
            .filter(|actor| {
                let height = hearth.y - actor.feet.y;
                (-0.55 * self.scale..=BODY_HEIGHT).contains(&height)
            })
            .fold(Vec2::ZERO, |sum, actor| {
                let distance = actor.feet.xz().distance(hearth.xz()) / radius;
                let influence = (1.0 - distance).max(0.0).powi(2);
                sum + actor.velocity.clamp_length_max(18.0) * (0.20 * influence)
            });
        self.flow = self
            .flow
            .lerp(incoming.clamp_length_max(2.6), 1.0 - (-11.0 * dt).exp());
    }
}

fn flicker(t: f32) -> f32 {
    1.0 + 0.13 * (t * 7.1).sin() + 0.075 * (t * 13.9).sin() + 0.055 * (t * 25.3).sin()
}

pub(in crate::plugins) fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    sprites: &mut Assets<FxMaterial>,
    placements: &[Placement],
) {
    if placements.is_empty() {
        return;
    }
    let kit = props::Kit::new(meshes, materials);
    let layers = particles::materials(sprites);
    for (i, fire) in placements.iter().enumerate() {
        let scale = fire.profile.scale;
        let phase = i as f32 * 2.31;
        let geometry = Transform::from_rotation(fire.rotation).with_scale(Vec3::splat(scale));
        let hearth = geometry.transform_point(BRAZIER + Vec3::Y * 0.76);
        let root = commands
            .spawn((
                Name::new("Banner campfire"),
                PlayingSceneEntity,
                CampfireDraft {
                    hearth,
                    scale,
                    flow: Vec2::ZERO,
                },
                Transform::from_translation(fire.position),
                Visibility::default(),
            ))
            .id();
        commands.entity(root).with_children(|root| {
            root.spawn((geometry, Visibility::default()))
                .with_children(|props| {
                    kit.spawn(props);
                    props::spawn_banner(
                        props,
                        meshes,
                        images,
                        materials,
                        fire.profile.banner_color,
                    );
                });
            particles::spawn(root, meshes, &layers, hearth, scale, i as u32 + 1);
            root.spawn((
                CampfireLight {
                    elapsed: 0.0,
                    phase,
                },
                PointLight {
                    color: Color::linear_rgb(1.0, 0.36, 0.094),
                    intensity: 140_000.0 * flicker(phase),
                    range: 14.5 * scale,
                    radius: 0.6 * scale,
                    ..default()
                },
                Transform::from_translation(hearth + Vec3::Y * 1.42 * scale),
                Visibility::Visible,
            ));
        });
    }
}

pub(super) fn install(app: &mut App) {
    use bevy::{camera::visibility::VisibilitySystems, transform::TransformSystems};

    app.add_systems(
        PostUpdate,
        animate
            .after(TransformSystems::Propagate)
            .before(VisibilitySystems::VisibilityPropagate)
            .before(super::runtime::update_frame)
            .run_if(in_state(AppState::Playing))
            .run_if(crate::plugins::simulation_frame),
    );
}

fn animate(
    time: Res<Time>,
    clock: Option<Res<FrameClock>>,
    packs: Res<GraphicsPacks>,
    stage: Option<Res<LoadedStageInfo>>,
    camera: Query<&GlobalTransform, With<GameplayCamera>>,
    actors: SceneActors,
    mut meshes: ResMut<Assets<Mesh>>,
    mut drafts: Query<(&mut CampfireDraft, &GlobalTransform)>,
    mut banners: Query<(&mut props::Banner, &GlobalTransform, &Mesh3d)>,
    mut particles: Query<(
        &mut particles::Plume,
        &GlobalTransform,
        &Mesh3d,
        &ChildOf,
        &mut Visibility,
    )>,
    mut lights: Query<(&mut CampfireLight, &mut PointLight, &ChildOf)>,
) {
    let dt = crate::plugins::simulation_dt(clock.as_deref(), time.delta_secs());
    let Some(stage) = stage else { return };
    let fallback = super::config::Wind::default();
    let wind = packs.wind(&stage.stage_id).unwrap_or(&fallback);
    for (mut draft, origin) in &mut drafts {
        draft.update(origin, &actors, dt);
    }
    let wake = |parent: &ChildOf| {
        drafts
            .get(parent.parent())
            .map_or(Vec2::ZERO, |(draft, _)| draft.flow)
    };
    for (mut light, mut point, parent) in &mut lights {
        light.elapsed += dt;
        let flare = 1.0 + 0.07 * wake(parent).length();
        point.intensity = 140_000.0 * flicker(light.elapsed + light.phase) * flare;
    }
    for (mut banner, transform, mesh) in &mut banners {
        let Some(mut mesh) = meshes.get_mut(&mesh.0) else {
            continue;
        };
        let cloth_wind = wind.response.map_or(1.0, |response| response.cloth);
        let flow = wind_at(wind, transform.translation(), f64::from(banner.elapsed)) * cloth_wind;
        let local = transform.rotation().inverse() * Vec3::new(flow.x, 0.0, flow.y);
        banner.advance(dt, local, transform, &actors, &mut mesh);
    }
    let Ok(camera) = camera.single() else { return };
    for (mut plume, transform, mesh, parent, mut visibility) in &mut particles {
        plume.elapsed += dt;
        if transform
            .translation()
            .distance_squared(camera.translation())
            > 130.0_f32.powi(2)
        {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        if let Some(mut mesh) = meshes.get_mut(&mesh.0) {
            let flow = wind_at(wind, transform.translation(), f64::from(plume.elapsed));
            plume.sprites(camera, flow, wake(parent)).write(&mut mesh);
            visibility.set_if_neq(Visibility::Visible);
        }
    }
}
