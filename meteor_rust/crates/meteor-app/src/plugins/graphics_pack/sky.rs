use super::config::{Environment, Sky};
use super::GraphicsPacks;
use crate::plugins::stage_material::FrameUniforms;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Face, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::{Shader, ShaderRef};
use meteor_asset::gmdl::GmdlMesh;
use std::collections::HashSet;

const SHADER: Handle<Shader> = bevy::asset::uuid_handle!("1c5ee9c3-0985-436a-b829-287a3b623efb");

#[derive(Clone, ShaderType)]
struct SkyUniform {
    horizon: Vec4,
    zenith: Vec4,
    cloud_color: Vec4,
    layer: Vec4,
    motion: Vec4,
    sun: Vec4,
    sun_color: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct CloudSkyMaterial {
    #[uniform(0)]
    params: SkyUniform,
}

impl Material for CloudSkyMaterial {
    fn vertex_shader() -> ShaderRef {
        ShaderRef::Handle(SHADER)
    }

    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(SHADER)
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = Some(Face::Front);
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

#[derive(Component)]
pub(crate) struct CloudSky;

pub(crate) fn install(app: &mut App) {
    app.add_plugins(MaterialPlugin::<CloudSkyMaterial>::default())
        .add_systems(PostUpdate, update);
    bevy::asset::load_internal_asset!(app, SHADER, "cloud_sky.wgsl", Shader::from_wgsl);
}

pub(crate) fn scene<'a>(
    packs: &'a GraphicsPacks,
    stage: &'a str,
) -> Option<(&'a Sky, Environment)> {
    let profile = packs
        .scenes(stage)
        .filter_map(|(_, scene)| scene.sky.as_ref())
        .last()?;
    if !profile.enabled {
        return None;
    }
    let lighting = packs
        .scenes(stage)
        .filter_map(|(_, scene)| scene.environment.as_ref())
        .last()
        .cloned()
        .unwrap_or_default();
    Some((profile, lighting))
}

pub(crate) fn replacements(
    profile: &Sky,
    gmdl: &GmdlMesh,
    hidden: &HashSet<&str>,
) -> Result<HashSet<(usize, i32)>, String> {
    let mut batches = HashSet::new();
    if !profile.replace_skybox {
        return Ok(batches);
    }
    for target in &profile.skybox {
        let mut matching_object = None;
        for (index, object) in gmdl.objects.iter().enumerate() {
            if !object.name.eq_ignore_ascii_case(&target.object)
                || hidden.contains(object.name.as_str())
                || object.indices.is_empty()
                || object.positions.is_empty()
            {
                continue;
            }
            let mut matched = false;
            for &material in &object.face_mat {
                let texture = gmdl
                    .materials
                    .get(material as usize)
                    .and_then(|mat| gmdl.resolve_texture_name(mat));
                if texture.is_some_and(|name| name.eq_ignore_ascii_case(&target.texture)) {
                    batches.insert((index, material));
                    matched = true;
                }
            }
            if matched && matching_object.replace(index).is_some() {
                return Err(format!("ambiguous skybox object {}", target.object));
            }
        }
        if matching_object.is_none() {
            return Err(format!(
                "missing skybox batch {} / {}",
                target.object, target.texture
            ));
        }
    }
    Ok(batches)
}

pub(crate) fn material(sky: &Sky, light: &Environment) -> CloudSkyMaterial {
    CloudSkyMaterial {
        params: SkyUniform {
            horizon: Vec3::from_array(sky.horizon).extend(0.0),
            zenith: Vec3::from_array(sky.zenith).extend(0.0),
            cloud_color: Vec3::from_array(sky.cloud_color).extend(0.0),
            layer: Vec4::new(sky.base_height, sky.thickness, sky.density, sky.coverage),
            motion: Vec4::new(sky.wind[0], sky.wind[1], sky.scale, 0.0),
            sun: Vec3::from_array(light.sun_direction)
                .normalize_or_zero()
                .extend(sky.steps as f32),
            sun_color: Vec3::from_array(light.sun_color).extend(light.sun_intensity),
        },
    }
}

fn update(
    domes: Query<&MeshMaterial3d<CloudSkyMaterial>, With<CloudSky>>,
    frame: Option<Res<FrameUniforms>>,
    mut materials: ResMut<Assets<CloudSkyMaterial>>,
) {
    let seconds = frame.map_or(0.0, |frame| frame.seconds as f32);
    for handle in &domes {
        if materials
            .get(&handle.0)
            .is_some_and(|material| material.params.motion.w != seconds)
        {
            if let Some(mut material) = materials.get_mut(&handle.0) {
                material.params.motion.w = seconds;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sn13_selectors_require_every_authored_layer() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../origin_res/sn13/sn13.gmb");
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => panic!("{error}"),
        };
        let mesh = meteor_asset::gmdl::decode_gmdl(&bytes).unwrap();
        let packs = super::super::tests::starter();
        let (sky, _) = scene(&packs, "sn13").unwrap();
        let batches = replacements(sky, &mesh, &HashSet::new()).unwrap();
        assert!(batches.len() >= 5);
        assert!(batches.iter().all(|(object, _)| matches!(
            mesh.objects[*object].name.as_str(),
            "Sphere01" | "Sphere02" | "Sphere03"
        )));
        assert!(replacements(sky, &mesh, &HashSet::from(["Sphere02"])).is_err());
        let mut kept = sky.clone();
        kept.replace_skybox = false;
        assert!(replacements(&kept, &mesh, &HashSet::new())
            .unwrap()
            .is_empty());
    }
}
