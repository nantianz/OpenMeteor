use super::*;
use serde::Deserialize;
use std::sync::{Arc, OnceLock};
mod cloth;
mod cloth_runtime;
use cloth_runtime::update;

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
    cloth: Arc<cloth::topology::Topology>,
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
        let base = self.mesh_handles[lod()]
            .get_or_init(|| meshes.add(build_group_mesh(self.skin(), &self.skin().indices)))
            .clone();

        meshes.add(meshes.get(&base).expect("prepared display mesh").clone())
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
                }
            })
            .collect();
        commands.entity(model).insert(SecondaryMotion {
            nodes,
            cloth: cloth::Simulation::default(),
            anchor: None,
            deformed: false,
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
    let cloth = Arc::new(cloth::topology::Topology::prepare(
        &skins[1],
        &skins[lod()],
    )?);
    Ok(VisualAssets {
        skins: skins.try_into().map_err(|_| "LOD count")?,
        controls: meta.controls,
        cloth,
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
}

#[derive(Component)]
struct SecondaryMotion {
    nodes: Vec<DisplayNode>,
    cloth: cloth::Simulation,
    anchor: Option<Vec3>,
    deformed: bool,
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod pack_tests;

#[cfg(test)]
mod uv_tests;
