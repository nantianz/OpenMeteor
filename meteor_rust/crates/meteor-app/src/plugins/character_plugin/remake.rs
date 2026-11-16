use super::*;
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    version: u32,
    bind_nodes: Vec<BindNode>,
    controls: Vec<Control>,
}

#[derive(Deserialize)]
struct BindNode {
    name: String,
    parent: Option<String>,
    pivot: [f32; 3],
    rotation: [f32; 4],
}

#[derive(Clone, Deserialize)]
struct Control {
    name: String,
    anchor: usize,
    pivot: [f32; 3],
    kind: String,
    side: f32,
    limit: f32,
}

pub(crate) struct VisualAssets {
    skins: [meteor_asset::skc::SkinMesh; 2],
    controls: Vec<Control>,
    textures: [Vec<u8>; 3],
    mesh_handles: [OnceLock<Handle<Mesh>>; 2],
    texture_handles: [OnceLock<Handle<bevy::image::Image>>; 3],
    inverse_handle: OnceLock<Handle<SkinnedMeshInverseBindposes>>,
}

fn lod() -> usize {
    usize::from(cfg!(any(target_arch = "wasm32", target_os = "android")))
}

impl VisualAssets {
    pub(super) fn skin(&self) -> &meteor_asset::skc::SkinMesh {
        &self.skins[lod()]
    }

    pub(super) fn mesh(&self, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        self.mesh_handles[lod()]
            .get_or_init(|| meshes.add(build_group_mesh(self.skin(), &self.skin().indices)))
            .clone()
    }

    pub(super) fn material(
        &self,
        color: u8,
        images: &mut Assets<bevy::image::Image>,
        materials: &mut Assets<GmdlMaterial>,
        quality: u8,
        handles: &mut crate::plugins::pak_textures::QualityTextureHandles,
    ) -> Handle<GmdlMaterial> {
        let index = usize::from(color.min(2));
        let texture = self.texture_handles[index]
            .get_or_init(|| {
                let handle = crate::plugins::pak_textures::image_handle_with_quality(
                    images,
                    &self.textures[index],
                    quality,
                )
                .expect("validated P0 remake texture");
                crate::plugins::pak_textures::register_quality_texture(handles, handle.clone());
                handle
            })
            .clone();
        materials.add(GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.0,
                mode: 0,
                has_texture: 1,
                uv_offset: Vec2::ZERO,
            },
            Some(texture),
            false,
            GmdlMode::Opaque,
        ))
    }

    pub(super) fn append_joints(
        &self,
        commands: &mut Commands,
        model: Entity,
        skeleton: &Skeleton,
        joints: &mut Vec<Entity>,
        inverse: &mut Vec<bevy::math::Mat4>,
    ) {
        let nodes = self
            .controls
            .iter()
            .map(|control| {
                let source = joints[control.anchor];
                let joint = commands
                    .spawn((SecondaryJoint, GlobalTransform::IDENTITY, ChildOf(model)))
                    .id();
                inverse.push(bevy::math::Mat4::from_cols_array(
                    &skeleton.skin_world_bind_inverse[control.anchor].m,
                ));
                joints.push(joint);
                DisplayNode {
                    source,
                    joint,
                    control: control.clone(),
                    angle: Vec2::ZERO,
                    target: Vec2::ZERO,
                    angular_velocity: Vec2::ZERO,
                    pivot: None,
                    pivot_velocity: Vec3::ZERO,
                    gravity_axis: Mat4::from_cols_array(
                        &skeleton.skin_world_bind_inverse[control.anchor].m,
                    )
                    .transform_vector3(Vec3::NEG_Z),
                }
            })
            .collect();
        commands.entity(model).insert(SecondaryMotion {
            nodes,
            elapsed: 0.0,
            accumulated: 0.0,
            velocity: Vec3::ZERO,
            anchor: None,
            sampled_position: None,
            facing: 0.0,
        });
    }

    pub(super) fn inverse(
        &self,
        values: Vec<bevy::math::Mat4>,
        assets: &mut Assets<SkinnedMeshInverseBindposes>,
    ) -> Handle<SkinnedMeshInverseBindposes> {
        self.inverse_handle
            .get_or_init(|| assets.add(SkinnedMeshInverseBindposes::from(values)))
            .clone()
    }
}

pub(super) fn load(
    roots: &[PathBuf],
    character: &str,
    skeleton: &Skeleton,
) -> Option<VisualAssets> {
    if character != "p0" {
        return None;
    }
    if std::env::var("METEOR_STOCK_P0").is_ok_and(|value| value == "1") {
        return None;
    }
    let metadata = read_first(roots, "pmodel/p0-remake-metadata.json")?;
    let result = prepared(skeleton, metadata, |name| {
        read_first(roots, &format!("pmodel/p0-remake-{name}"))
    });
    match result {
        Ok(assets) => {
            info!(
                "P0 remake ready: {} display vertices, {} triangles, 30 stock + 8 cosmetic joints",
                assets.skin().vertex_count,
                assets.skin().triangle_count
            );
            Some(assets)
        }
        Err(error) => {
            warn!("P0 remake rejected; displaying stock P0: {error}");
            None
        }
    }
}

fn prepared(
    skeleton: &Skeleton,
    metadata: Vec<u8>,
    mut read: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<VisualAssets, String> {
    let meta: Metadata = serde_json::from_slice(&metadata).map_err(|e| e.to_string())?;
    if meta.version != 1
        || meta.bind_nodes.len() != skeleton.nodes.len()
        || meta.controls.len() != 8
        || skeleton.skin_bone_count() != 30
    {
        return Err("unsupported visual rig".into());
    }

    for (bind, node) in meta.bind_nodes.iter().zip(&skeleton.nodes) {
        let parent = node.parent.map(|i| skeleton.nodes[i].name.as_str());
        if bind.name != node.name
            || bind.parent.as_deref() != parent
            || bind.pivot
                != [
                    node.local_translation.x,
                    node.local_translation.y,
                    node.local_translation.z,
                ]
            || bind.rotation
                != [
                    node.local_rotation.x,
                    node.local_rotation.y,
                    node.local_rotation.z,
                    node.local_rotation.w,
                ]
        {
            return Err(format!("stock bind mismatch: {}", node.name));
        }
    }
    for control in &meta.controls {
        if ![2, 11, 13].contains(&control.anchor)
            || !control.pivot.iter().all(|x| x.is_finite())
            || !control.limit.is_finite()
            || control.limit <= 0.0
            || control.limit > 0.2
            || ![-1.0, 1.0].contains(&control.side)
            || !["ribbon", "hair", "jacket"].contains(&control.kind.as_str())
        {
            return Err(format!("invalid display control {}", control.name));
        }
    }
    let mut skins = Vec::new();
    for (name, budget) in [("high.skc", 48_000), ("low.skc", 8_000)] {
        let bytes = read(name).ok_or_else(|| format!("missing {name}"))?;
        let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        let skin = meteor_asset::skc::parse_skin(text);
        if skin.vertex_count == 0
            || skin.triangle_count > budget
            || skin.groups.len() != 1
            || skin.positions.len() != skin.vertex_count * 3
            || skin.uvs.len() != skin.vertex_count * 2
            || skin.skin_weights.len() != skin.vertex_count * 4
            || skin.skin_indices.len() != skin.vertex_count * 4
            || skin
                .indices
                .iter()
                .any(|&i| i as usize >= skin.vertex_count)
            || skin.skin_indices.iter().any(|&i| i >= 38)
            || skin.skin_weights.iter().any(|&v| v < 0.0)
            || skin
                .skin_weights
                .chunks_exact(4)
                .any(|v| (v.iter().sum::<f32>() - 1.0).abs() > 1e-5)
            || skin
                .positions
                .iter()
                .chain(&skin.uvs)
                .chain(&skin.skin_weights)
                .any(|v| !v.is_finite())
        {
            return Err(format!("invalid display mesh {name}"));
        }
        skins.push(skin);
    }
    let mut textures = Vec::new();
    for color in ["neutral", "red", "blue"] {
        let prefix = if lod() == 1 { "low-" } else { "" };
        let name = format!("{prefix}{color}.jpg");
        let bytes = read(&name).ok_or_else(|| format!("missing {name}"))?;

        if meteor_asset::texture::decode_image(&bytes).is_err() {
            return Err(format!("invalid texture {name}"));
        }
        textures.push(bytes);
    }
    Ok(VisualAssets {
        skins: skins.try_into().map_err(|_| "LOD count")?,
        controls: meta.controls,
        textures: textures.try_into().map_err(|_| "texture count")?,
        mesh_handles: std::array::from_fn(|_| OnceLock::new()),
        texture_handles: std::array::from_fn(|_| OnceLock::new()),
        inverse_handle: OnceLock::new(),
    })
}

#[derive(Component)]
struct SecondaryJoint;

#[derive(Component)]
struct DisplaySkin(Entity);

#[derive(Component)]
pub(super) struct Appearance {
    pub(super) enabled: bool,
    pub(super) source_joints: Vec<Entity>,
    pub(super) color_index: u8,
    pub(super) stock: Option<StockAppearance>,
}

#[derive(Clone)]
pub(super) struct StockDraw {
    pub(super) mesh: Handle<Mesh>,
    pub(super) material: Handle<GmdlMaterial>,
    pub(super) alpha_cutoff: f32,
}

pub(super) struct StockAppearance {
    pub(super) draws: Vec<StockDraw>,
    pub(super) inverse: Handle<SkinnedMeshInverseBindposes>,
}

fn switch_appearance(
    mut commands: Commands,
    settings: Option<Res<crate::app_state::Settings>>,
    assets: Option<Res<CharacterAssetsSet>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut images: ResMut<Assets<bevy::image::Image>>,
    mut inverse_binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut quality: ResMut<crate::plugins::pak_textures::QualityTextureHandles>,
    mut models: Query<(
        Entity,
        &FighterModel,
        &mut Appearance,
        Option<&SecondaryMotion>,
        Option<&crate::plugins::rebalance_plugin::BalanceAfterimage>,
    )>,
    skins: Query<(Entity, &ChildOf, &FighterSkin)>,
) {
    let (Some(settings), Some(assets)) = (settings, assets) else {
        return;
    };
    for (entity, model, mut appearance, motion, ghost) in &mut models {
        let Some(character) = assets.0.get(&model.character) else {
            continue;
        };
        let visual = character
            .visual
            .as_ref()
            .filter(|_| settings.new_character_models);
        if appearance.enabled == visual.is_some() {
            continue;
        }
        let mut concealed = false;
        for (skin, parent, state) in &skins {
            if parent.parent() == entity {
                concealed |= state.concealed;
                commands.entity(skin).despawn();
            }
        }
        if let Some(motion) = motion {
            for node in &motion.nodes {
                commands.entity(node.joint).despawn();
            }
        }
        commands
            .entity(entity)
            .remove::<SecondaryMotion>()
            .remove::<DisplaySkin>();
        if let Some(ghost) = ghost {
            for part in ghost.ghost_meshes.iter().chain(&ghost.ghost_joints) {
                commands.entity(*part).despawn();
            }
            commands
                .entity(entity)
                .remove::<crate::plugins::rebalance_plugin::BalanceAfterimage>();
        }
        let skeleton = &character.skeleton;
        let mut joints = appearance.source_joints.clone();
        let mut inverse: Vec<_> = skeleton
            .skin_world_bind_inverse
            .iter()
            .map(|matrix| Mat4::from_cols_array(&matrix.m))
            .collect();
        let inverse = if let Some(visual) = visual {
            visual.append_joints(&mut commands, entity, skeleton, &mut joints, &mut inverse);
            visual.inverse(inverse, &mut inverse_binds)
        } else if let Some(stock) = &appearance.stock {
            stock.inverse.clone()
        } else {
            inverse_binds.add(SkinnedMeshInverseBindposes::from(inverse))
        };
        super::hit_sway::rebind_render_joints(&mut commands, entity, skeleton, &joints);
        let draws = super::spawn_body_skins(
            &mut commands,
            entity,
            model.fighter,
            character,
            visual,
            appearance.color_index,
            &joints,
            inverse.clone(),
            &mut meshes,
            &mut materials,
            &mut images,
            settings.texture_quality,
            &mut quality,
            concealed,
            if visual.is_none() {
                appearance
                    .stock
                    .as_ref()
                    .map(|stock| stock.draws.as_slice())
            } else {
                None
            },
        );
        if visual.is_none() && appearance.stock.is_none() {
            appearance.stock = Some(StockAppearance { draws, inverse });
        }
        appearance.enabled = visual.is_some();
    }
}

pub(super) fn link_skin(commands: &mut Commands, model: Entity, skin: Entity) {
    commands.entity(model).insert(DisplaySkin(skin));
}

struct DisplayNode {
    source: Entity,
    joint: Entity,
    control: Control,
    angle: Vec2,
    target: Vec2,
    angular_velocity: Vec2,
    pivot: Option<Vec3>,
    pivot_velocity: Vec3,
    gravity_axis: Vec3,
}

#[derive(Component)]
struct SecondaryMotion {
    nodes: Vec<DisplayNode>,
    elapsed: f32,
    accumulated: f32,
    velocity: Vec3,
    anchor: Option<Vec3>,
    sampled_position: Option<Vec3>,
    facing: f32,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<GmdlMaterial>>()
        .init_resource::<Assets<bevy::image::Image>>()
        .init_resource::<Assets<SkinnedMeshInverseBindposes>>()
        .init_resource::<crate::plugins::pak_textures::QualityTextureHandles>()
        .add_systems(Update, switch_appearance.after(super::spawn_fighter_model));
    app.add_systems(
        PostUpdate,
        update
            .after(bevy::transform::TransformSystems::Propagate)
            .after(super::hit_sway::HitSwaySet)
            .before(crate::plugins::rebalance_plugin::sync_afterimage),
    );
}

fn update(
    time: Option<Res<Time>>,
    clock: Option<Res<crate::plugins::FrameClock>>,
    state: Option<Res<State<crate::app_state::AppState>>>,
    settings: Option<Res<crate::app_state::Settings>>,
    actors: Query<(
        &crate::components::fighter::MotionState,
        Option<&crate::components::fighter::Health>,
    )>,
    cameras: Query<
        &GlobalTransform,
        (
            With<crate::plugins::camera_plugin::GameplayCamera>,
            Without<SecondaryJoint>,
        ),
    >,
    mut models: Query<(Entity, &FighterModel, &DisplaySkin, &mut SecondaryMotion)>,
    skins: Query<&SkinnedMesh, With<FighterSkin>>,
    sources: Query<&GlobalTransform, Without<SecondaryJoint>>,
    mut joints: Query<&mut GlobalTransform, With<SecondaryJoint>>,
) {
    let dt = if state
        .as_ref()
        .is_none_or(|s| *s.get() == crate::app_state::AppState::Playing)
    {
        crate::plugins::simulation_dt(clock.as_deref(), time.map_or(0.0, |time| time.delta_secs()))
    } else {
        0.0
    };
    let eye = cameras.iter().next().map(GlobalTransform::translation);
    let enabled = settings
        .as_ref()
        .is_none_or(|s| s.character_motion && s.texture_quality < 2);
    for (entity, model, display, mut motion) in &mut models {
        let Ok(world) = sources.get(entity) else {
            continue;
        };
        let position = world.translation();
        let reset = motion
            .anchor
            .is_none_or(|last| last.distance(position) > 8.0);
        motion.anchor = Some(position);
        let (velocity, facing, alive) =
            actors
                .get(model.fighter)
                .map_or((Vec3::ZERO, 0.0, true), |(v, h)| {
                    let facing = if v.facing.is_finite() { v.facing } else { 0.0 };
                    let velocity = if v.velocity.is_finite() {
                        v.velocity
                    } else {
                        Vec3::ZERO
                    };
                    (velocity, facing, h.is_none_or(|h| h.current > 0.0))
                });
        let distance = eye.map_or(0.0, |eye| eye.distance(position));
        let gain = if enabled && alive {
            (1.0 - (distance - 30.0) / 25.0).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if reset || !dt.is_finite() || dt > 0.25 || gain == 0.0 {
            motion.velocity = velocity;
            motion.facing = facing;
            motion.accumulated = 0.0;
            motion.sampled_position = Some(position);
            for node in &mut motion.nodes {
                node.angle = Vec2::ZERO;
                node.target = Vec2::ZERO;
                node.angular_velocity = Vec2::ZERO;
                node.pivot = None;
                node.pivot_velocity = Vec3::ZERO;
            }
        } else if dt > 0.0 {
            motion.elapsed = (motion.elapsed + dt) % (std::f32::consts::TAU * 100.0);
            motion.accumulated += dt;
            if motion.accumulated >= 1.0 / 30.0 {
                let step = motion.accumulated.min(0.1);
                motion.accumulated = 0.0;
                let acceleration = ((velocity - motion.velocity) / step).clamp_length_max(55.0);
                motion.velocity = velocity;
                let turn = (facing - motion.facing + std::f32::consts::PI)
                    .rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                motion.facing = facing;
                let turn_rate = (turn / step).clamp(-12.0, 12.0);
                let root_velocity = motion
                    .sampled_position
                    .map_or(Vec3::ZERO, |last| (position - last) / step);
                motion.sampled_position = Some(position);
                let phase = motion.elapsed;

                for (i, node) in motion.nodes.iter_mut().enumerate() {
                    let anchor = skins
                        .get(display.0)
                        .ok()
                        .and_then(|skin| skin.joints.get(node.control.anchor))
                        .copied()
                        .unwrap_or(node.source);
                    let Ok(source) = sources.get(anchor) else {
                        continue;
                    };
                    let pivot = source.transform_point(Vec3::from_array(node.control.pivot));
                    let part_velocity = node.pivot.map_or(Vec3::ZERO, |last| {
                        ((pivot - last) / step - root_velocity).clamp_length_max(12.0)
                    });
                    let part_acceleration =
                        ((part_velocity - node.pivot_velocity) / step).clamp_length_max(40.0);
                    node.pivot = Some(pivot);
                    node.pivot_velocity = part_velocity;
                    let drag = local_response(facing, velocity + part_velocity) / 9.0;
                    let inertia = local_response(facing, acceleration + part_acceleration) / 55.0;
                    let hanging = source
                        .affine()
                        .transform_vector3(node.gravity_axis)
                        .normalize_or_zero();
                    let gravity = world.rotation().inverse() * (hanging.cross(Vec3::NEG_Y));
                    let response =
                        -drag - inertia - Vec2::new(0.0, turn_rate * 0.1 * node.control.side)
                            + Vec2::new(gravity.x, gravity.y) * 0.3;
                    let breeze = Vec2::new(
                        (phase * 2.2 + i as f32 * 0.7).sin(),
                        (phase * 1.7 + node.control.side).sin(),
                    ) * 0.025;
                    node.target =
                        (response + breeze).clamp_length_max(1.0) * node.control.limit * gain;
                }
            }
            for node in &mut motion.nodes {
                advance_spring(node, dt);
            }
        }
        for node in &motion.nodes {
            let anchor = skins
                .get(display.0)
                .ok()
                .and_then(|skin| skin.joints.get(node.control.anchor))
                .copied()
                .unwrap_or(node.source);
            let (Ok(source), Ok(mut joint)) = (sources.get(anchor), joints.get_mut(node.joint))
            else {
                continue;
            };
            if node.angle == Vec2::ZERO {
                *joint = *source;
            } else {
                let pivot = source.transform_point(Vec3::from_array(node.control.pivot));
                let axis = world.rotation() * Vec3::new(node.angle.x, node.angle.y, 0.0);
                *joint = GlobalTransform::from(
                    Mat4::from_translation(pivot)
                        * Mat4::from_quat(Quat::from_scaled_axis(axis))
                        * Mat4::from_translation(-pivot)
                        * source.to_matrix(),
                );
            }
        }
    }
}

fn local_response(facing: f32, velocity: Vec3) -> Vec2 {
    let (sin, cos) = facing.sin_cos();
    Vec2::new(
        velocity.z * cos + velocity.x * sin,
        velocity.x * cos - velocity.z * sin,
    )
}

fn advance_spring(node: &mut DisplayNode, dt: f32) {
    let (frequency, damping): (f32, f32) = match node.control.kind.as_str() {
        "ribbon" if node.control.name.ends_with("_tip") => (8.0, 0.48),
        "ribbon" => (11.0, 0.7),
        "hair" => (14.0, 0.65),
        _ => (20.0, 0.9),
    };

    let decay = frequency * damping;
    let oscillation = frequency * (1.0 - damping * damping).sqrt();
    let (sin, cos) = (oscillation * dt).sin_cos();
    let attenuation = (-decay * dt).exp();
    let offset = node.angle - node.target;
    let speed = node.angular_velocity;
    node.angle =
        node.target + attenuation * (offset * cos + (speed + decay * offset) * (sin / oscillation));
    node.angular_velocity = attenuation
        * (speed * cos - (decay * speed + frequency * frequency * offset) * (sin / oscillation));
    if node.angle.length_squared() > node.control.limit * node.control.limit {
        node.angle = node.angle.clamp_length_max(node.control.limit);
        let normal = node.angle.normalize_or_zero();
        node.angular_velocity -= normal * node.angular_velocity.dot(normal).max(0.0);
    }
    node.angular_velocity = node
        .angular_velocity
        .clamp_length_max(frequency * node.control.limit * 2.0);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod pack_tests;

#[cfg(test)]
mod uv_tests;
