use super::cloth::{topology::Topology, Capsule, Simulation};
use super::{FighterModel, GmdlMaterial, GmdlMaterialParams, GmdlMode};
use crate::app_state::{AppState, LoadedStageInfo, Settings};
use crate::components::fighter::{Concealment, Health};
use crate::plugins::{
    camera_plugin::GameplayCamera,
    character_plugin::{FighterBone, FighterDummy, FighterModelBones, FighterPoseSampled},
    graphics_pack::{
        cloth_binding::{DetailMesh, Pinning},
        cloth_obj::ClothObj,
        config::{CharacterCloth, ClothNode, ClothPoint, Clothing},
        wind_at, GraphicsPacks, SceneGraphics,
    },
    stage_material::FrameUniforms,
    FrameClock,
};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use std::sync::Arc;

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
    rigid: Option<Entity>,
    attachment: Attachment,
    colliders: Vec<(AttachedPoint, AttachedPoint, f32)>,
    profile: CharacterCloth,
    topology: Topology,
    render_points: Option<Vec<usize>>,
    detail: Option<Arc<DetailMesh>>,
    detail_frames: Vec<Mat3>,
    render_world: Vec<Vec3>,
    groups: Vec<BoundPinGroup>,
    pin_weights: Vec<Vec<(usize, f32)>>,
    pin_frames: Vec<(Vec3, Mat3)>,
    simulation: Simulation,
    targets: Vec<Vec3>,
    capsules: Vec<Capsule>,
    previous_pose: Option<(Vec3, Mat3)>,
    tangents: bool,
}

#[derive(Component)]
struct CapeResolved;

#[derive(Component, Default)]
struct CapeSlots(Vec<String>);

#[derive(Clone, Copy)]
struct AttachedPoint {
    entity: Entity,
    offset: Vec3,
}

struct BoundPinGroup {
    anchor: AttachedPoint,
    rest_origin: Vec3,
    inverse_rest: Mat3,
}

impl AttachedPoint {
    fn at(entity: Entity) -> Self {
        Self {
            entity,
            offset: Vec3::ZERO,
        }
    }

    fn position(self, transforms: &Query<&GlobalTransform>) -> Option<Vec3> {
        let transform = transforms.get(self.entity).ok()?;
        Some(transform.translation() + transform.rotation() * self.offset)
    }
}

enum Attachment {
    Torso([Entity; 4]),
    Node(AttachedPoint),
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
    let mesh = cloth_mesh(
        points.iter().map(|p| p.to_array()).collect(),
        uv,
        colors,
        triangles.iter().flatten().map(|&i| i as u16).collect(),
    );
    (Topology::from_triangles(points, freedom, triangles), mesh)
}

fn cloth_mesh(
    points: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u16>,
) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, points)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
    .with_inserted_indices(Indices::U16(indices));
    mesh.compute_smooth_normals();
    mesh
}

fn obj_render_mesh(obj: &ClothObj) -> Mesh {
    cloth_mesh(
        obj.vertices
            .iter()
            .map(|(point, _)| obj.points[*point].to_array())
            .collect(),
        obj.vertices.iter().map(|(_, uv)| *uv).collect(),
        vec![[1.0; 4]; obj.vertices.len()],
        obj.indices.clone(),
    )
}

fn obj_mesh(sim: &ClothObj, obj: &ClothObj, freedom: Vec<f32>) -> (Topology, Mesh, Vec<usize>) {
    (
        Topology::from_triangles(sim.points.clone(), freedom, sim.triangles.clone()),
        obj_render_mesh(obj),
        obj.vertices.iter().map(|(point, _)| *point).collect(),
    )
}

fn rotation_matrix(degrees: [f32; 3]) -> Mat3 {
    let [x, y, z] = degrees.map(f32::to_radians);
    Mat3::from_quat(Quat::from_euler(EulerRot::XYZ, x, y, z))
}

fn attach(
    mut commands: Commands,
    graphics: Option<Res<GraphicsPacks>>,
    loaded: Option<Res<LoadedStageInfo>>,
    models: Query<(Entity, &FighterModel, Option<&CapeSlots>), Without<CapeResolved>>,
    children: Query<&Children>,
    nodes: Query<(Option<&FighterBone>, Option<&FighterDummy>, Option<&Name>)>,
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
    let mut material_for = |index: usize, id: &str, opacity: f32, mesh: &mut Mesh| {
        let definition = &graphics.packs[index].config.materials[id];
        let tangents = definition.normal.is_some() && mesh.generate_tangents().is_ok();
        let material = shared_materials
            .entry((index, id.to_owned(), opacity.to_bits()))
            .or_insert_with(|| {
                let mode = if opacity < 1.0 {
                    GmdlMode::Blend
                } else {
                    GmdlMode::Opaque
                };
                let mut material = GmdlMaterial::new_scene(
                    GmdlMaterialParams {
                        opacity,
                        alpha_cutoff: 0.,
                        mode: mode.as_u32(),
                        has_texture: 0,
                        uv_offset: Vec2::ZERO,
                    },
                    None,
                    true,
                    mode,
                );
                scene.apply_pbr(
                    index,
                    id,
                    "character cloth",
                    &mut material,
                    mesh,
                    &mut images,
                );
                materials.add(material)
            })
            .clone();
        (material, tangents)
    };
    for (entity, model, previous) in &models {
        let mut profiles = std::collections::BTreeMap::new();
        for (index, scene) in graphics.packs.iter().enumerate().flat_map(|(index, pack)| {
            pack.config
                .scenes
                .iter()
                .filter(|scene| {
                    scene.stage == "*" || scene.stage.eq_ignore_ascii_case(&loaded.stage_id)
                })
                .map(move |scene| (index, scene))
        }) {
            for cloth in scene
                .character_cloth
                .iter()
                .filter(|cloth| cloth.character.eq_ignore_ascii_case(&model.character))
            {
                if cloth.enabled {
                    profiles.insert(cloth.slot.as_str(), (index, cloth));
                } else if cloth.slot.is_empty() {
                    profiles.clear();
                } else {
                    profiles.remove(cloth.slot.as_str());
                }
            }
        }
        let mut resolved = previous.map_or_else(Vec::new, |slots| slots.0.clone());
        for (&slot, &(index, profile)) in &profiles {
            if resolved.iter().any(|ready| ready == slot) {
                continue;
            }
            let find = |kind: &ClothNode| {
                match kind {
                    ClothNode::Model {} => return Some(entity),
                    ClothNode::Actor {} => return Some(model.fighter),
                    _ => {}
                }
                let mut matches = children
                    .iter_descendants::<Children>(entity)
                    .filter(|&child| {
                        nodes
                            .get(child)
                            .is_ok_and(|(bone, dummy, node)| match kind {
                                ClothNode::Bone { name } => {
                                    bone.is_some_and(|bone| bone.name == *name)
                                }
                                ClothNode::Dummy { name } => {
                                    dummy.is_some_and(|dummy| dummy.name == *name)
                                }
                                ClothNode::Named { name } => {
                                    node.is_some_and(|node| node.as_str() == name)
                                }
                                _ => false,
                            })
                    });
                matches.next().filter(|_| matches.next().is_none())
            };
            let resolve = |point: &ClothPoint| {
                Some(AttachedPoint {
                    entity: find(&point.node)?,
                    offset: Vec3::from_array(point.offset),
                })
            };
            let bone = |name: &str| find(&ClothNode::Bone { name: name.into() });
            let attachment = if let Some(anchor) = &profile.anchor {
                let Some(point) = resolve(anchor) else {
                    continue;
                };
                Attachment::Node(point)
            } else {
                let [Some(neck), Some(pelvis), Some(left), Some(right)] =
                    profile.bones.each_ref().map(|name| bone(name))
                else {
                    continue;
                };
                Attachment::Torso([neck, pelvis, left, right])
            };
            let colliders = if let Some(authored) = &profile.colliders {
                let Some(resolved) = authored
                    .iter()
                    .map(|c| Some((resolve(&c.a)?, resolve(&c.b)?, c.radius)))
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                resolved
            } else if let Attachment::Torso([neck, pelvis, left, right]) = attachment {
                let mut resolved = vec![
                    (AttachedPoint::at(pelvis), AttachedPoint::at(neck), 0.24),
                    (AttachedPoint::at(left), AttachedPoint::at(right), 0.16),
                ];
                resolved.extend(LIMBS.iter().filter_map(|&(a, b, radius)| {
                    Some((
                        AttachedPoint::at(bone(a)?),
                        AttachedPoint::at(bone(b)?),
                        radius,
                    ))
                }));
                resolved
            } else {
                continue;
            };
            let pack = &graphics.packs[index];
            let mut groups = Vec::new();
            let mut pin_weights = Vec::new();
            let mut detail = None;
            let (topology, mut mesh, render_points) = if let Some(source) = &profile.mesh {
                let Some(sim) = pack.cloth_meshes.get(&source.id) else {
                    continue;
                };
                let Some(resolved_groups) = source
                    .pin_groups
                    .iter()
                    .map(|group| {
                        Some(BoundPinGroup {
                            anchor: resolve(&group.anchor)?,
                            rest_origin: Vec3::from_array(group.rest_origin),
                            inverse_rest: rotation_matrix(group.rest_rotation_degrees).transpose(),
                        })
                    })
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                groups = resolved_groups;
                let render = if let Some(id) = &source.render_id {
                    detail = pack
                        .cloth_details
                        .get(&(source.id.clone(), id.clone()))
                        .cloned();
                    if detail.is_none() {
                        continue;
                    }
                    &pack.cloth_meshes[id]
                } else {
                    sim
                };
                let Ok(pinning) = Pinning::prepare(sim, source) else {
                    resolved.push(slot.to_owned());
                    continue;
                };
                pin_weights = pinning.weights;
                let (topology, mesh, indices) = obj_mesh(sim, render, pinning.freedom);
                (topology, mesh, Some(indices))
            } else {
                let (topology, mesh) = grid(profile);
                (topology, mesh, None)
            };
            let (material, tangents) =
                material_for(index, &profile.material, profile.opacity, &mut mesh);
            let instance = commands.spawn_empty().id();
            let rigid = profile.rigid_mesh.as_ref().map(|rigid| {
                let mut mesh = obj_render_mesh(&pack.cloth_meshes[&rigid.id]);
                let (material, _) = material_for(index, &rigid.material, 1.0, &mut mesh);
                commands
                    .spawn((
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(material),
                        Transform::IDENTITY,
                        Visibility::Inherited,
                        ChildOf(instance),
                    ))
                    .id()
            });
            commands.entity(instance).insert((
                Cape {
                    model: entity,
                    rigid,
                    attachment,
                    colliders,
                    profile: profile.clone(),
                    topology,
                    render_points,
                    detail,
                    detail_frames: Vec::new(),
                    render_world: Vec::new(),
                    groups,
                    pin_frames: Vec::new(),
                    pin_weights,
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
            resolved.push(slot.to_owned());
        }
        if resolved.len() == profiles.len() {
            commands.entity(entity).insert(CapeResolved);
        } else if previous.map(|slots| slots.0.len()).unwrap_or_default() != resolved.len() {
            commands.entity(entity).insert(CapeSlots(resolved));
        }
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
    let mut body_min = Vec3::splat(f32::INFINITY);
    let mut body_max = Vec3::splat(f32::NEG_INFINITY);
    for capsule in capsules {
        let radius = Vec3::splat(capsule.radius + margin);
        body_min = body_min.min(capsule.a.min(capsule.b) - radius);
        body_max = body_max.max(capsule.a.max(capsule.b) + radius);
    }
    triangles.iter().all(|&indices| {
        let [a, b, c] = indices.map(|i| points[i]);
        if a.min(b).min(c).cmpgt(body_max).any() || a.max(b).max(c).cmplt(body_min).any() {
            return true;
        }
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

impl Cape {
    fn prepare_surface(&mut self, orientation: Mat3, margin: f32) -> bool {
        let points = &self.simulation.positions;
        let check = !self.profile.keep_visible_on_contact;
        if check && !clear_body(points, &self.topology.triangles, &self.capsules, margin) {
            return false;
        }
        self.detail.as_ref().is_none_or(|detail| {
            detail.deform(
                points,
                &self.topology.triangles,
                orientation,
                &mut self.detail_frames,
                &mut self.render_world,
            );
            !check
                || clear_body(
                    &self.render_world,
                    &detail.triangles,
                    &self.capsules,
                    margin,
                )
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    graphics: Option<Res<GraphicsPacks>>,
    loaded: Option<Res<LoadedStageInfo>>,
    settings: Option<Res<Settings>>,
    state: Option<Res<State<AppState>>>,
    time: Option<Res<Time>>,
    clock: Option<Res<FrameClock>>,
    frame: Option<Res<FrameUniforms>>,
    camera: Query<&GlobalTransform, With<GameplayCamera>>,
    models: Query<(
        &FighterModel,
        &GlobalTransform,
        Has<FighterModelBones>,
        Has<FighterPoseSampled>,
    )>,
    actors: Query<(Option<&Health>, Option<&Concealment>)>,
    transforms: Query<&GlobalTransform>,
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
        let distance =
            models
                .get(cape.model)
                .ok()
                .and_then(|(model, transform, has_bones, posed)| {
                    if has_bones && !posed {
                        return None;
                    }
                    let (health, hidden) = actors.get(model.fighter).ok()?;
                    (health.is_none_or(|h| h.current > 0.)
                        && hidden.is_none_or(|c| c.remaining <= 0.))
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
        let pose = match cape.attachment {
            Attachment::Torso(anchors) => {
                let points =
                    anchors.map(|id| transforms.get(id).ok().map(GlobalTransform::translation));
                if let [Some(neck), Some(pelvis), Some(left), Some(right)] = points {
                    body_frame([neck, pelvis, left, right])
                        .map(|(at, side, up, back)| (at, Mat3::from_cols(side, up, back)))
                } else {
                    None
                }
            }
            Attachment::Node(anchor) => anchor.position(&transforms).zip(
                transforms
                    .get(anchor.entity)
                    .ok()
                    .map(|node| Mat3::from_quat(node.rotation())),
            ),
        };
        let Some((at, basis)) = pose else {
            cape.previous_pose = None;
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        let Ok((_, model, _, _)) = models.get(cape.model) else {
            continue;
        };
        let orientation = basis * rotation_matrix(cape.profile.rotation_degrees);
        let origin = at + basis.z_axis * cape.profile.back_offset;
        let handedness = Mat3::from_diagonal(Vec3::new(1., 1., orientation.determinant().signum()));
        cape.pin_frames.clear();
        cape.pin_frames.push((origin, orientation));
        cape.pin_frames
            .extend(cape.groups.iter().filter_map(|group| {
                let node = transforms.get(group.anchor.entity).ok()?;
                let rotation = node.rotation();
                let matrix = Mat3::from_quat(rotation) * handedness * group.inverse_rest;
                let offset = node.translation() + rotation * group.anchor.offset;
                Some((offset - matrix * group.rest_origin, matrix))
            }));
        if cape.pin_frames.len() != cape.groups.len() + 1 {
            cape.previous_pose = None;
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        let targets = || {
            cape.topology.points.iter().enumerate().map(|(i, &point)| {
                cape.pin_weights.get(i).map_or_else(
                    || origin + orientation * point,
                    |weights| {
                        weights
                            .iter()
                            .map(|&(group, weight)| {
                                let (offset, matrix) = cape.pin_frames[group];
                                (offset + matrix * point) * weight
                            })
                            .sum()
                    },
                )
            })
        };
        let capsules = || {
            cape.colliders.iter().filter_map(|&(a, b, radius)| {
                Some(Capsule {
                    a: a.position(&transforms)?,
                    b: b.position(&transforms)?,
                    radius,
                })
            })
        };
        let targets_changed = dt == 0.0 && targets().ne(cape.targets.iter().copied());
        if dt == 0.0
            && !targets_changed
            && cape.previous_pose == Some((at, basis))
            && capsules()
                .map(|c| (c.a, c.b, c.radius))
                .eq(cape.capsules.iter().map(|c| (c.a, c.b, c.radius)))
        {
            continue;
        }
        if let Some(rigid) = cape.rigid {
            let world = bevy::math::Affine3A::from_mat3_translation(orientation, origin);
            let local = model.affine().inverse() * world;
            commands.entity(rigid).insert((
                Transform::from_matrix(Mat4::from(local)),
                GlobalTransform::from(world),
            ));
        }
        cape.targets.clear();
        cape.targets.extend(targets());
        cape.capsules.clear();
        cape.capsules.extend(capsules());
        if cape.capsules.len() != cape.colliders.len() {
            cape.previous_pose = None;
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        }
        let reset = cape.previous_pose.is_none_or(|(old, previous)| {
            at.distance_squared(old) > 4.
                || basis.z_axis.dot(previous.z_axis) < 0.35
                || (dt == 0.0 && (at != old || basis != previous))
        }) || targets_changed
            || (!cape.groups.is_empty()
                && cape
                    .targets
                    .iter()
                    .zip(&cape.simulation.anchors)
                    .zip(&cape.topology.freedom)
                    .any(|((target, prev), &freedom)| {
                        freedom == 0. && target.distance_squared(*prev) > 4.
                    }))
            || !dt.is_finite()
            || !(0.0..=0.2).contains(&dt);
        cape.previous_pose = Some((at, basis));
        if reset {
            cape.simulation.reset(&cape.targets);
        } else if dt > 0. {
            let velocity = wind.map_or(Vec3::ZERO, |wind| {
                let v = wind_at(wind, at, frame.as_ref().map_or(0.0, |f| f.seconds))
                    * wind.response.map_or(1.0, |r| r.cloth);
                Vec3::new(v.x, 0., v.y)
            });
            let dt = if cape.detail.is_some() || !cape.groups.is_empty() {
                dt.min(1. / 30.)
            } else {
                dt
            };
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
        if !cape.prepare_surface(orientation, margin) {
            if reset {
                visibility.set_if_neq(Visibility::Hidden);
                continue;
            }
            cape.simulation.reset(&cape.targets);
            if !cape.prepare_surface(orientation, margin) {
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
        let world_points = if cape.detail.is_some() {
            &cape.render_world
        } else {
            &cape.simulation.positions
        };
        for (i, p) in positions.iter_mut().enumerate() {
            let point = cape
                .render_points
                .as_ref()
                .map_or(i, |vertices| vertices[i]);
            *p = inverse.transform_point3(world_points[point]).to_array();
        }
        mesh.compute_smooth_normals();
        if cape.tangents {
            let _ = mesh.generate_tangents();
        }
        visibility.set_if_neq(Visibility::Inherited);
    }
}

#[cfg(all(
    test,
    feature = "original-assets",
    not(any(target_arch = "wasm32", target_os = "android"))
))]
#[path = "cape/visual_tests.rs"]
mod visual_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obj_cape_follows_a_late_named_node_on_a_model_without_bones() {
        let packs = crate::plugins::graphics_pack::load_with_reader(
            br#"{"schema_version":1,"enabled":true,"packs":["pack.json"]}"#,
            |name| match name {
                "pack.json" => Ok(br#"{
                    "schema_version":1,"id":"fixture",
                    "cloth_meshes":{"cape":"cape.obj"},
                    "materials":{"red":{"tint":[0.6,0.02,0.02],"metallic":0.65}},
                    "scenes":[{"stage":"*","character_cloth":[{
                        "character":"p0","material":"red","mesh":{"id":"cape","pin_vertices":[1,2]},
                        "anchor":{"node":{"kind":"named","name":"cape_socket"},"offset":[0,0,0.4]},
                        "back_offset":0,
                        "colliders":[{
                            "a":{"node":{"kind":"model"},"offset":[0,0.45,0]},
                            "b":{"node":{"kind":"model"},"offset":[0,1.35,0]},
                            "radius":0.20
                        }]
                    }]}]
                }"#
                .to_vec()),
                "cape.obj" => Ok(
                    b"v -0.3 0 0\nv 0.3 0 0\nv -0.3 -0.8 0\nv 0.3 -0.8 0\nf 1 2 3\nf 2 4 3\n"
                        .to_vec(),
                ),
                _ => Err(name.into()),
            },
        )
        .unwrap();
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .insert_resource(packs)
            .insert_resource(LoadedStageInfo::default());
        install(&mut app);
        let actor = app.world_mut().spawn(Transform::IDENTITY).id();
        let model = app
            .world_mut()
            .spawn((
                FighterModel {
                    fighter: actor,
                    character: "p0".into(),
                },
                Transform::IDENTITY,
                ChildOf(actor),
            ))
            .id();
        app.update();
        let world = app.world_mut();
        assert_eq!(world.query::<&Cape>().iter(world).count(), 0);
        let socket = world
            .spawn((
                Name::new("cape_socket"),
                Transform::from_xyz(0.0, 1.35, 0.0),
                ChildOf(model),
            ))
            .id();
        app.update();
        app.world_mut()
            .get_mut::<Transform>(socket)
            .unwrap()
            .translation
            .x += 0.4;
        app.update();
        let world = app.world_mut();
        let (cape, visibility) = world.query::<(&Cape, &Visibility)>().single(world).unwrap();
        assert_eq!(*visibility, Visibility::Inherited);
        assert_eq!(cape.topology.triangles.len(), 2);
        assert_eq!(cape.topology.freedom, [0., 0., 1., 1.]);
        assert!((cape.simulation.positions[0] - Vec3::new(0.1, 1.35, 0.4)).length() < 1e-5);
        assert_eq!(cape.capsules.len(), 1);
    }

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

    #[test]
    fn detail_cloth_follows_two_independent_named_pins_and_checks_the_visible_surface() {
        let packs = crate::plugins::graphics_pack::load_with_reader(
            br#"{"schema_version":1,"enabled":true,"packs":["pack.json"]}"#,
            |name| match name {
                "pack.json" => Ok(br#"{
                    "schema_version":1,"id":"fixture",
                    "cloth_meshes":{"coat":"coat.obj","detail":"detail.obj"},
                    "materials":{"cloth":{}},
                    "scenes":[{"stage":"*","character_cloth":[{
                        "character":"p0","material":"cloth","mesh":{
                            "id":"coat","render_id":"detail",
                            "pin_groups":[{
                                "anchor":{"node":{"kind":"named","name":"left"}},
                                "vertices":[1],"rest_origin":[-0.3,0,0]
                            },{
                                "anchor":{"node":{"kind":"named","name":"right"}},
                                "vertices":[2],"rest_origin":[0.3,0,0]
                            }]
                        },
                        "anchor":{"node":{"kind":"model"},"offset":[0,1.35,0.5]},
                        "back_offset":0,
                        "colliders":[{
                            "a":{"node":{"kind":"named","name":"body"},"offset":[0,0.45,0]},
                            "b":{"node":{"kind":"named","name":"body"},"offset":[0,1.35,0]},
                            "radius":0.20
                        }]
                    }]}]
                }"#.to_vec()),
                "coat.obj" => Ok(b"v -0.3 0 0\nv 0.3 0 0\nv -0.3 -0.8 0\nv 0.3 -0.8 0\nf 1 2 3\nf 2 4 3\n".to_vec()),
                "detail.obj" => Ok(b"v -0.3 0 0\nv 0.3 0 0\nv -0.3 -0.8 0\nv 0.3 -0.8 0\nv 0 -0.4 -0.1\nf 1 2 5\nf 1 5 3\nf 2 4 5\nf 3 5 4\n".to_vec()),
                _ => Err(name.into()),
            },
        ).unwrap();
        let mut app = App::new();
        app.add_plugins(bevy::transform::TransformPlugin)
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .insert_resource(packs)
            .insert_resource(LoadedStageInfo::default());
        install(&mut app);
        let world = app.world_mut();
        let actor = world.spawn(Transform::IDENTITY).id();
        let model = world
            .spawn((
                FighterModel {
                    fighter: actor,
                    character: "p0".into(),
                },
                Transform::IDENTITY,
                ChildOf(actor),
            ))
            .id();
        let [_, right, body] = [
            ("left", Vec3::new(-0.3, 1.35, 0.5)),
            ("right", Vec3::new(0.3, 1.35, 0.5)),
            ("body", Vec3::ZERO),
        ]
        .map(|(name, position)| {
            world
                .spawn((
                    Name::new(name),
                    Transform::from_translation(position),
                    ChildOf(model),
                ))
                .id()
        });
        app.update();
        let world = app.world_mut();
        let mut query = world.query::<(&Cape, &Visibility, &Mesh3d)>();
        let (cape, visibility, mesh) = query.single(world).unwrap();
        assert_eq!(*visibility, Visibility::Inherited);
        assert_eq!(cape.simulation.positions.len(), 4);
        assert_eq!(cape.render_world.len(), 5);
        assert!((cape.render_world[2] - Vec3::new(0., 0.95, 0.4)).length() < 1e-5);
        let mesh = mesh.0.clone();
        let vertex_count = world
            .resource::<Assets<Mesh>>()
            .get(&mesh)
            .unwrap()
            .count_vertices();
        assert_eq!(vertex_count, 5);

        world.get_mut::<Transform>(body).unwrap().translation.z = 0.24;
        app.update();
        let world = app.world_mut();
        let (cape, visibility, _) = query.single(world).unwrap();
        assert!(clear_body(
            &cape.simulation.positions,
            &cape.topology.triangles,
            &cape.capsules,
            0.024
        ));
        assert_eq!(
            *visibility,
            Visibility::Hidden,
            "only the high mesh intersects"
        );

        world.get_mut::<Transform>(body).unwrap().translation.z = 0.;
        world.get_mut::<Transform>(right).unwrap().translation.x += 0.25;
        app.update();
        let world = app.world_mut();
        let (cape, visibility, _) = query.single(world).unwrap();
        assert_eq!(*visibility, Visibility::Inherited);
        assert!((cape.render_world[0] - Vec3::new(-0.3, 1.35, 0.5)).length() < 1e-5);
        assert!((cape.render_world[1] - Vec3::new(0.55, 1.35, 0.5)).length() < 1e-5);
        assert!(cape.render_world[2].x > 0.08);
    }

    #[cfg(feature = "original-assets")]
    #[test]
    fn stock_p0_outfit_keeps_all_triangles_when_turning() {
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
            let (cape, visibility, material, mesh) = world
                .query::<(&Cape, &Visibility, &MeshMaterial3d<GmdlMaterial>, &Mesh3d)>()
                .single(world)
                .unwrap();
            assert_eq!(cape.model, fighter);
            let material = world
                .resource::<Assets<GmdlMaterial>>()
                .get(&material.0)
                .unwrap();
            assert_eq!(cape.groups.len(), 3);
            assert!(
                cape.detail.as_ref().unwrap().triangles.len() > cape.topology.triangles.len() * 5
            );
            assert!(material.texture.is_some());
            if *visibility != Visibility::Hidden {
                visible_frames += 1;
                assert!(cape.profile.keep_visible_on_contact);
                let mesh = world.resource::<Assets<Mesh>>().get(&mesh.0).unwrap();
                assert_eq!(
                    mesh.indices().unwrap().len(),
                    cape.detail.as_ref().unwrap().triangles.len() * 3
                );
            }
        }
        assert!(visible_frames >= 45, "{visible_frames} frames with outfit");
    }
}
