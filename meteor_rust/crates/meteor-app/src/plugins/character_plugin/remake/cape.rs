use super::cloth::{topology::Topology, Capsule, Simulation};
use super::{FighterModel, GmdlMaterial, GmdlMaterialParams, GmdlMode};
use crate::app_state::{AppState, LoadedStageInfo, Settings};
use crate::components::fighter::{Concealment, Health};
use crate::plugins::{
    camera_plugin::GameplayCamera,
    character_plugin::{FighterBone, FighterModelBones, FighterPoseSampled},
    graphics_pack::{
        config::{CharacterCloth, Clothing},
        wind_at, GraphicsPacks, SceneGraphics,
    },
    stage_material::FrameUniforms,
    FrameClock,
};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

const LIMBS: [(&str, &str, f32); 9] = [
    ("bau_Neck", "bau_Head", 0.15),
    ("bau_L_UpperArm", "bau_L_Forearm", 0.14),
    ("bau_R_UpperArm", "bau_R_Forearm", 0.14),
    ("bau_L_Forearm", "bau_L_Hand", 0.12),
    ("bau_R_Forearm", "bau_R_Hand", 0.12),
    ("bad_L_Thigh", "bad_L_Calf", 0.18),
    ("bad_R_Thigh", "bad_R_Calf", 0.18),
    ("bad_L_Calf", "bad_L_Foot", 0.14),
    ("bad_R_Calf", "bad_R_Foot", 0.14),
];

#[derive(Component)]
struct Cape {
    model: Entity,
    anchors: [Entity; 4],
    limbs: Vec<(Entity, Entity, f32)>,
    profile: CharacterCloth,
    topology: Topology,
    simulation: Simulation,
    targets: Vec<Vec3>,
    capsules: Vec<Capsule>,
    previous_pose: Option<(Vec3, Vec3)>,
    tangents: bool,
}

pub(super) fn install(app: &mut App) {
    app.add_systems(Update, attach.after(super::super::spawn_fighter_model))
        .add_systems(
            PostUpdate,
            update
                .after(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
        );
}

fn grid(profile: &CharacterCloth) -> (Topology, Mesh) {
    let [cols, rows] = profile.grid;
    let mut points = Vec::with_capacity(cols * rows);
    let mut freedom = Vec::with_capacity(cols * rows);
    let mut uv = Vec::with_capacity(cols * rows);
    let mut colors = Vec::with_capacity(cols * rows);
    let mut triangles = Vec::new();
    for y in 0..rows {
        let v = y as f32 / (rows - 1) as f32;
        for x in 0..cols {
            let u = x as f32 / (cols - 1) as f32;
            let scallop = (u * std::f32::consts::TAU * 2.5).cos();
            points.push(Vec3::new(
                (u - 0.5) * profile.width * (0.83 + v * 0.25),
                -profile.length * v + 0.05 * (u - 0.5).abs(),
                v * 0.19 + scallop * (0.012 + v * 0.035),
            ));
            freedom.push(v.sqrt());
            uv.push([u, v]);
            let tint = 0.77 + 0.18 * scallop.abs();
            colors.push([tint, tint, tint, 1.0]);
            if y + 1 < rows && x + 1 < cols {
                let a = y * cols + x;
                let b = a + 1;
                let c = a + cols;
                let d = c + 1;
                if (x + y) % 2 == 0 {
                    triangles.extend([[a, c, d], [a, d, b]]);
                } else {
                    triangles.extend([[a, c, b], [b, c, d]]);
                }
            }
        }
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        points.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U16(
        triangles.iter().flatten().map(|&i| i as u16).collect(),
    ));
    mesh.compute_smooth_normals();
    let mut topology = Topology {
        area: vec![0.; points.len()],
        points,
        freedom,
        triangles,
        skin_vertices: Vec::new(),
        edges: Vec::new(),
        bends: Vec::new(),
        bindings: Vec::new(),
        normal_vertices: Vec::new(),
        normal_triangles: Vec::new(),
    };
    topology.constraints();
    (topology, mesh)
}

fn attach(
    mut commands: Commands,
    graphics: Option<Res<GraphicsPacks>>,
    loaded: Option<Res<LoadedStageInfo>>,
    models: Query<(Entity, &FighterModel, &FighterModelBones), Added<FighterModel>>,
    bones: Query<&FighterBone>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    let (Some(graphics), Some(loaded)) = (graphics, loaded) else {
        return;
    };
    if models.is_empty() || graphics.packs.is_empty() {
        return;
    }
    let mut scene = SceneGraphics::new(Some(&graphics), &loaded.stage_id);
    let mut shared_materials = std::collections::HashMap::new();
    for (entity, model, members) in &models {
        let Some((index, profile)) = graphics
            .packs
            .iter()
            .enumerate()
            .flat_map(|(i, pack)| {
                pack.config
                    .scenes
                    .iter()
                    .filter(|scene| {
                        scene.stage == "*" || scene.stage.eq_ignore_ascii_case(&loaded.stage_id)
                    })
                    .flat_map(move |scene| {
                        scene.character_cloth.iter().map(move |cloth| (i, cloth))
                    })
            })
            .filter(|(_, cloth)| cloth.character.eq_ignore_ascii_case(&model.character))
            .last()
        else {
            continue;
        };
        if !profile.enabled {
            continue;
        }
        let bone = |name: &str| {
            members
                .0
                .iter()
                .copied()
                .find(|entity| bones.get(*entity).is_ok_and(|bone| bone.name == name))
        };
        let [Some(neck), Some(pelvis), Some(left), Some(right)] =
            profile.bones.each_ref().map(|name| bone(name))
        else {
            warn!(
                "{}: character cloth has a missing anchor bone",
                model.character
            );
            continue;
        };
        let (topology, mut mesh) = grid(profile);
        let tangents = graphics.packs[index].config.materials[&profile.material]
            .normal
            .is_some()
            && mesh.generate_tangents().is_ok();
        let material = shared_materials
            .entry((index, profile.material.as_str()))
            .or_insert_with(|| {
                let mut material = GmdlMaterial::new_scene(
                    GmdlMaterialParams {
                        opacity: 1.,
                        alpha_cutoff: 0.,
                        mode: GmdlMode::Opaque.as_u32(),
                        has_texture: 0,
                        uv_offset: Vec2::ZERO,
                    },
                    None,
                    true,
                    GmdlMode::Opaque,
                );
                scene.apply_pbr(
                    index,
                    &profile.material,
                    "character cloth",
                    &mut material,
                    &mut mesh,
                    &mut images,
                );
                materials.add(material)
            })
            .clone();
        let limbs = LIMBS
            .iter()
            .filter_map(|&(a, b, radius)| Some((bone(a)?, bone(b)?, radius)))
            .collect();
        commands.spawn((
            Cape {
                model: entity,
                anchors: [neck, pelvis, left, right],
                limbs,
                profile: profile.clone(),
                topology,
                simulation: Simulation::default(),
                targets: Vec::new(),
                capsules: Vec::new(),
                previous_pose: None,
                tangents,
            },
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::Hidden,
            bevy::camera::visibility::NoFrustumCulling,
            bevy::light::NotShadowCaster,
            ChildOf(entity),
        ));
    }
}

fn body_frame([neck, pelvis, left, right]: [Vec3; 4]) -> Option<(Vec3, Vec3, Vec3, Vec3)> {
    let side = (left - right).try_normalize()?;
    let vertical = neck - pelvis;
    let up = (vertical - side * vertical.dot(side)).try_normalize()?;
    let back = up.cross(side).try_normalize()?;
    Some((left.midpoint(right).lerp(neck, 0.28), side, up, back))
}

fn clear_body(
    points: &[Vec3],
    triangles: &[[usize; 3]],
    capsules: &[Capsule],
    margin: f32,
) -> bool {
    triangles.iter().all(|&indices| {
        let [a, b, c] = indices.map(|i| points[i]);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        let center = (a + b + c) / 3.;
        let extent = center
            .distance(a)
            .max(center.distance(b))
            .max(center.distance(c));
        capsules.iter().all(|capsule| {
            let radius = capsule.radius + margin;
            let da = normal.dot(capsule.a - a);
            let db = normal.dot(capsule.b - a);
            if da.min(db) >= radius || da.max(db) <= -radius {
                return true;
            }
            let axis = capsule.b - capsule.a;
            let t =
                ((center - capsule.a).dot(axis) / axis.length_squared().max(1e-10)).clamp(0., 1.);
            center.distance_squared(capsule.a + axis * t) >= (radius + extent).powi(2)
        })
    })
}

#[allow(clippy::too_many_arguments)]
fn update(
    graphics: Option<Res<GraphicsPacks>>,
    loaded: Option<Res<LoadedStageInfo>>,
    settings: Option<Res<Settings>>,
    state: Option<Res<State<AppState>>>,
    time: Option<Res<Time>>,
    clock: Option<Res<FrameClock>>,
    frame: Option<Res<FrameUniforms>>,
    camera: Query<&GlobalTransform, With<GameplayCamera>>,
    models: Query<(&FighterModel, &GlobalTransform), With<FighterPoseSampled>>,
    actors: Query<(Option<&Health>, Option<&Concealment>)>,
    bones: Query<&GlobalTransform, With<FighterBone>>,
    mut capes: Query<(Entity, &mut Cape, &Mesh3d, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut nearest: Local<Vec<(f32, Entity)>>,
) {
    if capes.is_empty() {
        return;
    }
    let stage = loaded
        .as_ref()
        .map_or("", |loaded| loaded.stage_id.as_str());
    let fallback = Clothing::default();
    let stage_config = graphics
        .as_ref()
        .and_then(|packs| {
            packs
                .scenes(stage)
                .filter_map(|(_, scene)| scene.clothing.as_ref())
                .last()
        })
        .unwrap_or(&fallback);
    let wind = graphics.as_ref().and_then(|packs| packs.wind(stage));
    let eye = camera.iter().next().map(GlobalTransform::translation);
    let playing = state
        .as_ref()
        .is_none_or(|state| *state.get() == AppState::Playing);
    let dt = if playing {
        crate::plugins::simulation_dt(clock.as_deref(), time.map_or(0., |t| t.delta_secs()))
    } else {
        0.
    };
    let enabled = stage_config.enabled
        && settings
            .as_ref()
            .is_none_or(|s| s.character_motion && s.texture_quality < 2);
    nearest.clear();
    for (id, mut cape, _, mut visibility) in &mut capes {
        let distance = models.get(cape.model).ok().and_then(|(model, transform)| {
            let (health, hidden) = actors.get(model.fighter).ok()?;
            (health.is_none_or(|h| h.current > 0.) && hidden.is_none_or(|c| c.remaining <= 0.))
                .then(|| eye.map_or(0., |eye| eye.distance_squared(transform.translation())))
        });
        let config = cape.profile.physics.as_ref().unwrap_or(stage_config);
        if let Some(distance) = distance.filter(|d| {
            enabled
                && config.enabled
                && *d <= config.max_distance.min(stage_config.max_distance).powi(2)
        }) {
            nearest.push((distance, id));
        } else {
            cape.previous_pose = None;
            visibility.set_if_neq(Visibility::Hidden);
        }
    }
    nearest.sort_unstable_by(|(a, e), (b, f)| {
        a.total_cmp(b).then_with(|| e.to_bits().cmp(&f.to_bits()))
    });
    for (rank, &(_, id)) in nearest.iter().enumerate() {
        let Ok((_, mut cape, mesh, mut visibility)) = capes.get_mut(id) else {
            continue;
        };
        let cape = &mut *cape;
        let config = cape.profile.physics.as_ref().unwrap_or(stage_config);
        if rank >= stage_config.max_characters.min(config.max_characters) {
            cape.previous_pose = None;
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        let point = |id| bones.get(id).ok().map(GlobalTransform::translation);
        let [Some(neck), Some(pelvis), Some(left), Some(right)] = cape.anchors.map(point) else {
            continue;
        };
        let Some((at, side, up, back)) = body_frame([neck, pelvis, left, right]) else {
            cape.previous_pose = None;
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        if dt == 0.0
            && cape
                .previous_pose
                .is_some_and(|(old, previous_back)| old == at && previous_back == back)
        {
            continue;
        }
        let Ok((_, model)) = models.get(cape.model) else {
            continue;
        };
        cape.targets.clear();
        cape.targets.extend(
            cape.topology
                .points
                .iter()
                .map(|p| at + side * p.x + up * p.y + back * (cape.profile.back_offset + p.z)),
        );
        cape.capsules.clear();
        cape.capsules.push(Capsule {
            a: pelvis,
            b: neck,
            radius: 0.24,
        });
        cape.capsules.push(Capsule {
            a: left,
            b: right,
            radius: 0.16,
        });
        cape.capsules
            .extend(cape.limbs.iter().filter_map(|&(a, b, radius)| {
                Some(Capsule {
                    a: point(a)?,
                    b: point(b)?,
                    radius,
                })
            }));
        let reset = cape.previous_pose.is_none_or(|(old, previous_back)| {
            at.distance_squared(old) > 4. || back.dot(previous_back) < 0.35
        }) || !dt.is_finite()
            || !(0.0..=0.2).contains(&dt);
        cape.previous_pose = Some((at, back));
        if reset {
            cape.simulation.reset(&cape.targets);
        } else if dt > 0. {
            let velocity = wind.map_or(Vec3::ZERO, |wind| {
                let v = wind_at(wind, at, frame.as_ref().map_or(0.0, |f| f.seconds))
                    * wind.response.map_or(1.0, |r| r.cloth);
                Vec3::new(v.x, 0., v.y)
            });
            cape.simulation.advance(
                &cape.topology,
                &cape.targets,
                &cape.capsules,
                dt,
                velocity,
                1.,
                config,
            );
        }
        let margin = config.thickness + cape.profile.collision_margin;
        let mesh_is_clear =
            |points: &[Vec3]| clear_body(points, &cape.topology.triangles, &cape.capsules, margin);
        if !mesh_is_clear(&cape.simulation.positions) {
            cape.simulation.reset(&cape.targets);
            if !mesh_is_clear(&cape.simulation.positions) {
                visibility.set_if_neq(Visibility::Hidden);
                continue;
            }
        }
        let Some(mut mesh) = meshes.get_mut(&mesh.0) else {
            continue;
        };
        let inverse = model.affine().inverse();
        let Some(bevy::mesh::VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
        else {
            continue;
        };
        for (p, &world) in positions.iter_mut().zip(&cape.simulation.positions) {
            *p = inverse.transform_point3(world).to_array();
        }
        mesh.compute_smooth_normals();
        if cape.tangents {
            let _ = mesh.generate_tangents();
        }
        visibility.set_if_neq(Visibility::Inherited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cape_rejects_a_triangle_that_crosses_an_arm_between_free_vertices() {
        let triangle = [Vec3::new(-1., -1., 0.), Vec3::new(1., -1., 0.), Vec3::Y];
        let arm = Capsule {
            a: Vec3::NEG_Z,
            b: Vec3::Z,
            radius: 0.12,
        };
        assert!(!clear_body(&triangle, &[[0, 1, 2]], &[arm], 0.02));
        let translated = triangle.map(|p| p + Vec3::Z * 2.0);
        assert!(clear_body(&translated, &[[0, 1, 2]], &[arm], 0.02));
    }

    #[cfg(feature = "original-assets")]

    #[test]
    fn stock_p0_cape_attaches_and_keeps_the_render_surface_clear_when_turning() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let Ok(bnc) = std::fs::read_to_string(path.join("../../../origin_res/pmodel/P0.bnc"))
        else {
            return;
        };
        let skeleton =
            meteor_anim::skeleton::Skeleton::from_bone_data(&meteor_asset::bnc::parse_bones(&bnc));
        let packs = crate::plugins::graphics_pack::load_with_reader(
            br#"{"schema_version":1,"enabled":true,"packs":["graphics-packs/starter/pack.json"]}"#,
            |name| {
                std::fs::read(path.join("../../config").join(name))
                    .map_err(|error| error.to_string())
            },
        )
        .unwrap();
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .insert_resource(Time::<()>::default())
            .insert_resource(packs)
            .insert_resource(LoadedStageInfo {
                stage_id: "sn06".into(),
                ..default()
            });
        install(&mut app);
        let actor = app.world_mut().spawn_empty().id();
        let rotation = super::super::super::fighter_model_rotation(0.0);
        let fighter = app
            .world_mut()
            .spawn((
                FighterModel {
                    fighter: actor,
                    character: "p0".into(),
                },
                FighterPoseSampled,
                Transform::from_rotation(rotation).with_scale(Vec3::splat(0.12)),
            ))
            .id();
        for (index, node) in skeleton
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| !node.is_dummy)
        {
            app.world_mut().spawn((
                FighterBone {
                    model: fighter,
                    name: node.name.clone(),
                    skin_index: index,
                    node_index: index,
                },
                Transform::from_matrix(Mat4::from_cols_array(&node.world_bind.m)),
                ChildOf(fighter),
            ));
        }
        let mut visible_frames = 0;
        for step in 0..48 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f64(1.0 / 60.0));
            if step == 20 {
                let mut pose = app.world_mut().get_mut::<Transform>(fighter).unwrap();
                pose.rotation = Quat::from_rotation_y(std::f32::consts::PI) * rotation;
                pose.translation += Vec3::X * 3.0;
            }
            app.update();
            let world = app.world_mut();
            let (cape, visibility, material) = world
                .query::<(&Cape, &Visibility, &MeshMaterial3d<GmdlMaterial>)>()
                .single(world)
                .unwrap();
            assert_eq!(cape.model, fighter);
            let material = world
                .resource::<Assets<GmdlMaterial>>()
                .get(&material.0)
                .unwrap();
            assert!(material.pbr.y > 0.50 && material.pbr_tint.x > material.pbr_tint.y * 5.0);
            if *visibility != Visibility::Hidden {
                visible_frames += 1;
                assert!(clear_body(
                    &cape.simulation.positions,
                    &cape.topology.triangles,
                    &cape.capsules,
                    cape.profile.collision_margin,
                ));
            }
        }
        assert!(visible_frames >= 45, "{visible_frames} frames with cape");
    }
}
