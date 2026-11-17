use super::*;
use crate::plugins::graphics_pack::config::Clothing;

pub(super) fn update(
    time: Option<Res<Time>>,
    clock: Option<Res<crate::plugins::FrameClock>>,
    state: Option<Res<State<crate::app_state::AppState>>>,
    settings: Option<Res<crate::app_state::Settings>>,
    graphics: Option<Res<crate::plugins::graphics_pack::GraphicsPacks>>,
    graphics_frame: Option<Res<crate::plugins::stage_material::FrameUniforms>>,
    loaded: Option<Res<crate::app_state::LoadedStageInfo>>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut meshes: ResMut<Assets<Mesh>>,
    actors: Query<Option<&crate::components::fighter::Health>>,
    cameras: Query<
        &GlobalTransform,
        (
            With<crate::plugins::camera_plugin::GameplayCamera>,
            Without<SecondaryJoint>,
        ),
    >,
    mut models: Query<(Entity, &FighterModel, &DisplaySkin, &mut SecondaryMotion)>,
    skins: Query<(&SkinnedMesh, &Mesh3d), With<FighterSkin>>,
    sources: Query<&GlobalTransform, Without<SecondaryJoint>>,
    mut joints: Query<&mut GlobalTransform, With<SecondaryJoint>>,
) {
    let Some(assets) = assets else {
        return;
    };
    let dt = if state
        .as_ref()
        .is_none_or(|s| *s.get() == crate::app_state::AppState::Playing)
    {
        crate::plugins::simulation_dt(clock.as_deref(), time.map_or(0., |t| t.delta_secs()))
    } else {
        0.
    };
    let stage = loaded.as_ref().map_or("", |s| s.stage_id.as_str());
    let fallback = Clothing::default();
    let config = graphics
        .as_ref()
        .and_then(|p| {
            p.scenes(stage)
                .filter_map(|(_, s)| s.clothing.as_ref())
                .last()
        })
        .unwrap_or(&fallback);
    let wind = graphics
        .as_ref()
        .and_then(|p| p.scenes(stage).filter_map(|(_, s)| s.wind.as_ref()).last());
    let eye = cameras.iter().next().map(GlobalTransform::translation);
    let enabled = settings
        .as_ref()
        .is_none_or(|s| s.character_motion && s.texture_quality < 2)
        && config.enabled;
    let mut nearest: Vec<_> = models
        .iter()
        .filter_map(|(entity, model, _, _)| {
            if actors
                .get(model.fighter)
                .ok()
                .flatten()
                .is_some_and(|h| h.current <= 0.)
            {
                return None;
            }
            let position = sources.get(entity).ok()?.translation();
            Some((eye.map_or(0., |p| p.distance_squared(position)), entity))
        })
        .collect();
    nearest.sort_by(|(a, e), (b, f)| a.total_cmp(b).then_with(|| e.to_bits().cmp(&f.to_bits())));
    nearest.truncate(config.max_characters);
    for (entity, model, display, mut motion) in &mut models {
        let (Ok(world), Ok((skin, mesh))) = (sources.get(entity), skins.get(display.0)) else {
            continue;
        };
        let Some(character) = assets.0.get(&model.character) else {
            continue;
        };
        let Some(visual) = &character.visual else {
            continue;
        };
        let mut matrices = Vec::with_capacity(38);
        let mut joint_world = Vec::with_capacity(30);
        for (i, &e) in skin.joints.iter().take(30).enumerate() {
            let Ok(source) = sources.get(e) else {
                break;
            };
            joint_world.push(source.translation());
            matrices.push(
                source.to_matrix()
                    * Mat4::from_cols_array(&character.skeleton.skin_world_bind_inverse[i].m),
            );
        }
        if matrices.len() != 30 {
            continue;
        }

        for node in &motion.nodes {
            let anchor = skin
                .joints
                .get(node.control.anchor)
                .copied()
                .unwrap_or(node.source);
            if let (Ok(source), Ok(mut joint)) = (sources.get(anchor), joints.get_mut(node.joint)) {
                *joint = *source;
            }
            matrices.push(matrices[node.control.anchor]);
        }
        let data = &visual.cloth;
        let anchors: Vec<_> = data
            .skin_vertices
            .iter()
            .zip(&data.points)
            .map(|(i, p)| blended(&visual.skins[1], *i, &matrices).transform_point3(*p))
            .collect();
        let position = world.translation();
        let reset = motion
            .anchor
            .is_none_or(|last| last.distance(position) > 8.);
        motion.anchor = Some(position);
        let active = enabled
            && nearest.iter().any(|(_, e)| *e == entity)
            && actors
                .get(model.fighter)
                .ok()
                .flatten()
                .is_none_or(|h| h.current > 0.)
            && eye.is_none_or(|p| p.distance(position) < config.max_distance);
        let reset = reset || !dt.is_finite() || dt > 0.2 || dt < 0. || !active;
        if reset {
            motion.cloth.reset(&anchors);
        } else if dt > 0. {
            let velocity = wind.map_or(Vec3::ZERO, |w| {
                let v = crate::plugins::graphics_pack::wind_at(
                    w,
                    position,
                    graphics_frame.as_ref().map_or(0., |f| f.seconds),
                );
                Vec3::new(v.x, 0., v.y)
            });
            let scale = world.to_scale_rotation_translation().0.abs().max_element();

            let capsules: Vec<_> = [
                (2, 11, 1.7),
                (11, 12, 1.7),
                (3, 4, 1.0),
                (4, 5, 0.7),
                (7, 8, 1.0),
                (8, 9, 0.7),
            ]
            .map(|(a, b, r)| cloth::Capsule {
                a: joint_world[a],
                b: joint_world[b],
                radius: r * scale,
            })
            .into_iter()
            .collect();
            motion
                .cloth
                .advance(data, &anchors, &capsules, dt, velocity, scale, config);
        }
        if !reset && dt == 0. {
            continue;
        }
        if reset && !motion.deformed {
            continue;
        }
        let rest_normals = if reset {
            visual.mesh_handles[lod()]
                .get()
                .and_then(|h| meshes.get(h))
                .and_then(|m| m.attribute(Mesh::ATTRIBUTE_NORMAL))
                .and_then(|v| v.as_float3())
                .map(|v| v.to_vec())
        } else {
            None
        };
        let Some(mut mesh) = meshes.get_mut(&mesh.0) else {
            continue;
        };
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        for binding in &data.bindings {
            let i = binding.vertex;
            let rest = Vec3::from_slice(&visual.skin().positions[i * 3..i * 3 + 3]);
            let delta = binding
                .particles
                .iter()
                .enumerate()
                .map(|(k, j)| {
                    if data.freedom[*j] == 0. {
                        Vec3::ZERO
                    } else {
                        (motion.cloth.positions[*j] - anchors[*j]) * binding.weights[k]
                    }
                })
                .sum::<Vec3>();
            let matrix = blended(visual.skin(), i, &matrices);

            let p = if reset || matrix.determinant().abs() < 1e-10 {
                rest
            } else {
                rest + matrix
                    .inverse()
                    .transform_vector3(delta * binding.influence)
            };
            positions[i] = if p.is_finite() {
                p.to_array()
            } else {
                rest.to_array()
            };
        }

        let mut normals = vec![Vec3::ZERO; positions.len()];
        for &[a, b, c] in &data.normal_triangles {
            let p = Vec3::from_array(positions[a]);
            let q = Vec3::from_array(positions[b]);
            let r = Vec3::from_array(positions[c]);
            let n = (q - p).cross(r - p).normalize_or_zero();
            for (i, u, v) in [(a, q - p, r - p), (b, p - q, r - q), (c, p - r, q - r)] {
                if u.length_squared() * v.length_squared() > f32::EPSILON {
                    normals[i] += n * u.angle_between(v);
                }
            }
        }
        if let Some(VertexAttributeValues::Float32x3(values)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL)
        {
            for &i in &data.normal_vertices {
                values[i] = rest_normals
                    .as_ref()
                    .map_or_else(|| normals[i].normalize_or_zero().to_array(), |n| n[i]);
            }
        }
        motion.deformed = !reset;
    }
}
fn blended(skin: &meteor_asset::skc::SkinMesh, i: usize, matrices: &[Mat4]) -> Mat4 {
    (0..4)
        .map(|j| matrices[skin.skin_indices[i * 4 + j] as usize] * skin.skin_weights[i * 4 + j])
        .fold(Mat4::ZERO, |a, b| a + b)
}
