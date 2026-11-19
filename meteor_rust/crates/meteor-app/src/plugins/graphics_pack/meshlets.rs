use super::{
    config::{MeshletProp, MeshletReplacement, Placement},
    loader::LoadedPack,
    GraphicsPacks, SceneGraphics,
};
use crate::plugins::{
    render_plugin::classify_gmdl_material,
    stage_material::{GmdlMaterial, GmdlMaterialParams, GmdlMode},
};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues},
    prelude::*,
};
use meteor_asset::{
    des::StageObject,
    gmdl::{GmdlMesh, GmdlObject},
    mlmesh::{Model, Primitive, MAX_MESHLETS},
};
use sha2::{Digest, Sha256};
use std::{cell::OnceCell, collections::BTreeMap};

const MAX_GPU_BYTES: usize = 32 * 1024 * 1024;
const MAX_PROPS: usize = 32;
type Primitives = Vec<(Vec<Mesh>, GmdlMaterial)>;

pub(crate) fn supports_backend(backend: wgpu::Backend) -> bool {
    matches!(
        backend,
        wgpu::Backend::Dx12 | wgpu::Backend::Metal | wgpu::Backend::Vulkan
    )
}

struct Rule<'a> {
    pack: &'a LoadedPack,
    pack_index: usize,
    target: &'a MeshletReplacement,
}

#[derive(Default)]
pub(crate) struct SceneMeshlets<'a> {
    packs: Option<&'a GraphicsPacks>,
    rules: BTreeMap<String, Vec<Rule<'a>>>,
    props: Vec<(usize, &'a LoadedPack, &'a MeshletProp)>,
    stage: &'a str,
    source: &'a [u8],
    source_hash: OnceCell<String>,
    allocated_bytes: usize,
    allocated_meshlets: usize,
}

impl<'a> SceneMeshlets<'a> {
    pub(crate) fn new(
        packs: Option<&'a GraphicsPacks>,
        stage: &'a str,
        source: &'a [u8],
        backend: Option<wgpu::Backend>,
    ) -> Self {
        let mut result = Self {
            packs,
            stage,
            source,
            ..default()
        };
        let Some(packs) = packs else {
            return result;
        };
        let mut rules = BTreeMap::<String, Vec<Rule>>::new();
        let mut props = Vec::new();
        for (pack_index, pack) in packs.packs.iter().enumerate() {
            for scene in pack
                .config
                .scenes
                .iter()
                .filter(|scene| scene.stage.eq_ignore_ascii_case(stage))
            {
                props.extend(
                    scene
                        .meshlet_props
                        .iter()
                        .map(|prop| (pack_index, pack, prop)),
                );
                for target in &scene.meshlet_replacements {
                    rules
                        .entry(target.object.to_ascii_lowercase())
                        .or_default()
                        .push(Rule {
                            pack,
                            pack_index,
                            target,
                        });
                }
            }
        }
        if !rules.is_empty() || !props.is_empty() {
            if backend.is_some_and(supports_backend) && packs.has_loose_model_root() {
                result.rules = rules;
                result.props = props;
            } else {
                info!("graphics meshlets {stage}: using stock models on backend {backend:?} or without loose configuration");
            }
        }
        result
    }

    pub(crate) fn warn_unmatched(&self) {
        for object in self.rules.keys() {
            warn!(
                "graphics meshlets {} / {object}: target missing or not drawable",
                self.stage
            );
        }
    }

    pub(crate) fn replace(
        &mut self,
        gmdl: &GmdlMesh,
        object: &GmdlObject,
        scene_object: Option<&StageObject>,
        center: [f32; 3],
        scripted: impl FnOnce() -> bool,
        graphics: &mut SceneGraphics,
        images: &mut Assets<Image>,
    ) -> Option<Drawable> {
        let rules = self.rules.remove(&object.name.to_ascii_lowercase())?;
        match self.prepare(
            &rules,
            gmdl,
            object,
            scene_object,
            center,
            scripted,
            graphics,
            images,
        ) {
            Ok(value) => Some(value),
            Err(reason) => {
                warn!(
                    "graphics meshlets {} / {}: {reason}; using stock model",
                    self.stage, object.name
                );
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &mut self,
        rules: &[Rule<'_>],
        gmdl: &GmdlMesh,
        object: &GmdlObject,
        scene_object: Option<&StageObject>,
        center: [f32; 3],
        scripted: impl FnOnce() -> bool,
        graphics: &mut SceneGraphics,
        images: &mut Assets<Image>,
    ) -> Result<Drawable, String> {
        let [rule] = rules else {
            return Err("replacement rules conflict".into());
        };
        if gmdl
            .objects
            .iter()
            .filter(|other| other.name.eq_ignore_ascii_case(&object.name))
            .count()
            != 1
        {
            return Err("object name is ambiguous".into());
        }
        if !eligible(gmdl, object, scene_object, graphics) || scripted() {
            return Err("object is not an eligible static, opaque display mesh".into());
        }
        if !self.matches_source(rule.target.source_sha256.as_deref()) {
            return Err("stage GMB does not match source_sha256".into());
        }
        let anchor = source_anchor(object, center).ok_or("invalid original bounds")?;
        let transform = placed_at(anchor, &rule.target.placement);
        let primitives = self.prepare_model(
            (rule.pack_index, rule.pack),
            &rule.target.model,
            1,
            graphics,
            images,
        )?;
        info!(
            "graphics meshlets {} / {}: {} ({} meshlets)",
            self.stage,
            object.name,
            rule.target.model,
            primitives
                .iter()
                .map(|(meshes, _)| meshes.len())
                .sum::<usize>()
        );
        Ok(Drawable {
            transform,
            primitives,
        })
    }

    fn matches_source(&self, expected: Option<&str>) -> bool {
        expected.is_none_or(|expected| {
            self.source_hash
                .get_or_init(|| format!("{:x}", Sha256::digest(self.source)))
                .eq_ignore_ascii_case(expected)
        })
    }

    pub(crate) fn prepare_props(
        &mut self,
        graphics: &mut SceneGraphics,
        images: &mut Assets<Image>,
    ) -> Vec<PlacedProps> {
        let mut placed = Vec::new();
        let mut remaining = MAX_PROPS;
        for (index, pack, prop) in std::mem::take(&mut self.props) {
            if !self.matches_source(prop.source_sha256.as_deref())
                || prop.placements.len() > remaining
            {
                warn!(
                    "graphics props {} / {}: source mismatch or instance budget exceeded",
                    self.stage, prop.model
                );
                continue;
            }
            match self.prepare_model(
                (index, pack),
                &prop.model,
                prop.placements.len(),
                graphics,
                images,
            ) {
                Ok(primitives) => {
                    remaining -= prop.placements.len();
                    let transforms = prop
                        .placements
                        .iter()
                        .map(|p| placed_at(Vec3::ZERO, p))
                        .collect();
                    info!(
                        "graphics props {} / {}: placed {} instances",
                        self.stage,
                        prop.model,
                        prop.placements.len()
                    );
                    placed.push(PlacedProps {
                        transforms,
                        primitives,
                    });
                }
                Err(reason) => warn!("graphics props {} / {}: {reason}", self.stage, prop.model),
            }
        }
        placed
    }

    fn prepare_model(
        &mut self,
        (pack_index, pack): (usize, &LoadedPack),
        name: &str,
        instances: usize,
        graphics: &mut SceneGraphics,
        images: &mut Assets<Image>,
    ) -> Result<Primitives, String> {
        let definition = &pack.config.meshlet_models[name];
        let bytes = self
            .packs
            .unwrap()
            .read_meshlet_model(pack, &definition.file)?;
        let model =
            Model::decode(&bytes).map_err(|reason| format!("{}: {reason}", definition.file))?;
        let mut bytes = 0;
        let mut count = 0;
        for p in &model.primitives {
            if !definition.materials.contains_key(&p.slot) {
                return Err(format!("missing mapping for material slot {}", p.slot));
            }
            count += p.meshlets.len() * instances;
            bytes += p
                .meshlets
                .iter()
                .map(|m| 4096 + 64 * m.vertices.len() + 2 * m.indices.len())
                .sum::<usize>();
        }
        if self.allocated_bytes + bytes > MAX_GPU_BYTES
            || self.allocated_meshlets + count > MAX_MESHLETS
        {
            return Err("scene Meshlet GPU budget exceeded".into());
        }
        let mut primitives = Vec::with_capacity(model.primitives.len());
        for mut primitive in model.primitives {
            let material_id = &definition.materials[&primitive.slot];
            if pack.config.materials[material_id].normal.is_some() && primitive.tangents.is_none() {
                primitive.tangents = Some(generate_tangents(&primitive)?);
            }
            let mut meshes = split_meshes(&primitive);
            let mut material = GmdlMaterial::new_scene(
                GmdlMaterialParams {
                    opacity: 1.,
                    alpha_cutoff: 0.,
                    mode: GmdlMode::Opaque.as_u32(),
                    has_texture: 0,
                    uv_offset: Vec2::ZERO,
                },
                None,
                false,
                GmdlMode::Opaque,
            );
            graphics.apply_pbr(
                pack_index,
                material_id,
                name,
                &mut material,
                &mut meshes[0],
                images,
            );
            primitives.push((meshes, material));
        }
        self.allocated_bytes += bytes;
        self.allocated_meshlets += count;
        Ok(primitives)
    }
}

pub(crate) struct Drawable {
    pub(crate) transform: Transform,
    pub(crate) primitives: Primitives,
}

pub(crate) struct PlacedProps {
    pub(crate) transforms: Vec<Transform>,
    pub(crate) primitives: Primitives,
}

fn placed_at(anchor: Vec3, placement: &Placement) -> Transform {
    Transform::from_translation(anchor + Vec3::from_array(placement.offset))
        .with_rotation(Quat::from_rotation_y(placement.yaw_degrees.to_radians()))
        .with_scale(Vec3::splat(placement.scale))
}

fn eligible(
    gmdl: &GmdlMesh,
    object: &GmdlObject,
    desc: Option<&StageObject>,
    graphics: &SceneGraphics,
) -> bool {
    if object
        .name
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("d_"))
        || desc.is_some_and(|entry| {
            entry.is_dummy
                || entry.skip_render()
                || entry.model.is_some()
                || entry.billboard != 0
                || entry.texture_animation.enabled
                || entry.deform.is_some()
                || entry.effect.is_some()
                || entry.is_hell
                || entry.damage_name != 0
        })
    {
        return false;
    }
    let mut found = false;
    for index in &object.face_mat {
        let Some(material) = usize::try_from(*index)
            .ok()
            .and_then(|i| gmdl.materials.get(i))
        else {
            continue;
        };
        found = true;
        let (mode, opacity, cutoff) = classify_gmdl_material(material);
        let texture = gmdl.resolve_texture_name(material);
        if mode != GmdlMode::Opaque
            || opacity != 1.
            || cutoff != 0.
            || material.blend.eq_ignore_ascii_case("alphachannel")
            || texture.is_some_and(|name| name.to_ascii_lowercase().ends_with(".ifl"))
            || graphics.has_wind(&object.name, texture)
        {
            return false;
        }
    }
    found
}

pub(crate) fn source_anchor(object: &GmdlObject, center: [f32; 3]) -> Option<Vec3> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for v in object.positions.chunks_exact(3) {
        let point = Vec3::new(v[0] - center[0], v[2] - center[2], center[1] - v[1])
            * meteor_core::constants::MODEL_SCALE;
        if !point.is_finite() {
            return None;
        }
        min = min.min(point);
        max = max.max(point);
    }
    let midpoint = min.lerp(max, 0.5);
    midpoint
        .is_finite()
        .then_some(Vec3::new(midpoint.x, min.y, midpoint.z))
}

fn pick<const N: usize>(values: &[[f32; N]], remap: &[u32]) -> Vec<[f32; N]> {
    remap.iter().map(|&index| values[index as usize]).collect()
}

fn generate_tangents(primitive: &Primitive) -> Result<Vec<[f32; 4]>, String> {
    let indices = primitive
        .meshlets
        .iter()
        .flat_map(|cluster| {
            cluster
                .indices
                .iter()
                .map(|&i| cluster.vertices[usize::from(i)])
        })
        .collect();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, primitive.positions.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, primitive.normals.clone())
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, primitive.uvs.clone())
    .with_inserted_indices(Indices::U32(indices));
    mesh.generate_tangents()
        .map_err(|error| format!("cannot generate replacement tangent basis: {error}"))?;
    match mesh.remove_attribute(Mesh::ATTRIBUTE_TANGENT) {
        Some(VertexAttributeValues::Float32x4(tangents)) => Ok(tangents),
        _ => Err("missing replacement tangent basis".into()),
    }
}

pub(super) fn split_meshes(primitive: &Primitive) -> Vec<Mesh> {
    primitive
        .meshlets
        .iter()
        .map(|cluster| {
            let remap = &cluster.vertices;

            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pick(&primitive.positions, remap))
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, pick(&primitive.normals, remap))
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, pick(&primitive.uvs, remap))
            .with_inserted_indices(Indices::U16(
                cluster.indices.iter().map(|&i| u16::from(i)).collect(),
            ));
            if let Some(tangents) = &primitive.tangents {
                mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, pick(tangents, remap));
            }
            if let Some(colors) = &primitive.colors {
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, pick(colors, remap));
            }
            mesh
        })
        .collect()
}
