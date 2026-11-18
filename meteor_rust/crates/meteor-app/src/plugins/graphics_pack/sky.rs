use super::config::{Environment, Sky};
use super::GraphicsPacks;
use bevy::ecs::system::SystemParamItem;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, AsBindGroupError, BindGroupLayout, BindGroupLayoutEntry, Face,
    RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError, UnpreparedBindGroup,
};
use bevy::render::renderer::RenderDevice;
use bevy::shader::{Shader, ShaderRef};
use meteor_asset::gmdl::GmdlMesh;
use std::collections::HashSet;

mod cache;
mod noise;

const SHADER: Handle<Shader> = bevy::asset::uuid_handle!("1c5ee9c3-0985-436a-b829-287a3b623efb");

#[derive(Clone, Default, PartialEq, ShaderType)]
struct SkyUniform {
    horizon: Vec4,
    zenith: Vec4,
    cloud_color: Vec4,
    layer: Vec4,
    motion: Vec4,
    wind: Vec4,
    sun: Vec4,
    sun_color: Vec4,
}

#[derive(AsBindGroup, Clone)]
pub(crate) struct SkyBindings {
    #[uniform(0)]
    params: SkyUniform,
    #[texture(1, dimension = "3d")]
    #[sampler(2)]
    noise: Handle<Image>,
}

#[derive(Asset, TypePath, Clone)]
pub(crate) struct CloudSkyMaterial(SkyBindings);

impl AsBindGroup for CloudSkyMaterial {
    type Data = ();
    type Param = (
        <SkyBindings as AsBindGroup>::Param,
        Res<'static, cache::CloudCache>,
    );

    fn label() -> &'static str {
        "cloud sky"
    }

    fn bind_group_data(&self) -> Self::Data {}

    fn unprepared_bind_group(
        &self,
        layout: &BindGroupLayout,
        device: &RenderDevice,
        (bindings, cache): &mut SystemParamItem<'_, '_, Self::Param>,
        force_no_bindless: bool,
    ) -> Result<UnpreparedBindGroup, AsBindGroupError> {
        let mut group =
            self.0
                .unprepared_bind_group(layout, device, bindings, force_no_bindless)?;
        cache.attach(&mut group)?;
        Ok(group)
    }

    fn bind_group_layout_entries(
        device: &RenderDevice,
        force_no_bindless: bool,
    ) -> Vec<BindGroupLayoutEntry> {
        let mut entries = SkyBindings::bind_group_layout_entries(device, force_no_bindless);
        cache::CloudCache::layout(&mut entries);
        entries
    }
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SkyLighting {
    pub sky: Vec3,
    pub ground: Vec3,
    pub sun: Vec4,
    pub sun_color: Vec3,
}

#[derive(Component)]
pub(crate) struct CloudSky(pub Option<SkyLighting>);

#[derive(Resource, Default)]
pub(crate) struct CloudNoise(Option<Handle<Image>>);

impl CloudNoise {
    pub(crate) fn get(&mut self, images: &mut Assets<Image>) -> Handle<Image> {
        self.0
            .get_or_insert_with(|| images.add(noise::image()))
            .clone()
    }
}

pub(crate) fn install(app: &mut App) {
    app.add_plugins(MaterialPlugin::<CloudSkyMaterial>::default())
        .init_resource::<CloudNoise>();
    bevy::asset::load_internal_asset!(app, SHADER, "cloud_sky.wgsl", Shader::from_wgsl);
    cache::install(app);
}

#[cfg(test)]
pub(crate) fn cache_ready(app: &App) -> bool {
    cache::ready(app)
}

pub(crate) fn scene(packs: &GraphicsPacks, stage: &str) -> Option<(Sky, Environment)> {
    let mut profile = packs
        .scenes(stage)
        .filter_map(|(_, scene)| scene.sky.as_ref())
        .last()?
        .clone();
    if !profile.enabled {
        return None;
    }
    let mut lighting = packs
        .scenes(stage)
        .filter_map(|(_, scene)| scene.environment.as_ref())
        .last()
        .cloned()
        .unwrap_or_default();
    if let Some(sun) = &profile.sun {
        lighting.sun_direction = sun.direction;
        lighting.sun_color = sun.color;
        lighting.sun_intensity = sun.intensity;
    }
    if let Some(minutes) = profile
        .fixed_time
        .as_deref()
        .and_then(super::config::sky_minutes)
    {
        sample_fixed_time(&mut profile, &mut lighting, minutes);
    }
    Some((profile, lighting))
}

fn sample_fixed_time(sky: &mut Sky, lighting: &mut Environment, minutes: u16) {
    let hour_angle = (f32::from(minutes) - 720.0) * (std::f32::consts::TAU / 1440.0);
    let (across, height) = hour_angle.sin_cos();
    let authored = Vec3::from_array(lighting.sun_direction);
    let meridian = Vec3::new(authored.x, 0.0, authored.z)
        .try_normalize()
        .unwrap_or(Vec3::NEG_Z);
    let horizontal = -Vec3::Y.cross(meridian) * across + meridian * (0.34 * height);
    let direction = (horizontal + Vec3::Y * (0.94 * height + 0.07)).normalize();
    let elevation = direction.y;
    let day = smoothstep(0.04, 0.61, elevation);
    let twilight = smoothstep(-0.26, 0.07, elevation);
    let mut horizon = Vec3::new(0.012, 0.022, 0.047);
    let mut zenith = Vec3::new(0.003, 0.008, 0.023);
    let dawn = minutes < 720;
    let warm = if dawn {
        Vec3::new(0.82, 0.44, 0.24)
    } else {
        Vec3::new(0.77, 0.25, 0.105)
    };
    horizon = horizon
        .lerp(warm, twilight)
        .lerp(Vec3::from_array(sky.horizon), day);
    zenith = zenith
        .lerp(Vec3::new(0.09, 0.135, 0.31), twilight)
        .lerp(Vec3::from_array(sky.zenith), day);
    let clouds = Vec3::new(0.19, 0.28, 0.49)
        .lerp(
            if dawn {
                Vec3::new(1.00, 0.82, 0.71)
            } else {
                Vec3::new(1.0, 0.62, 0.44)
            },
            twilight,
        )
        .lerp(Vec3::from_array(sky.cloud_color), day);
    sky.horizon = horizon.to_array();
    sky.zenith = zenith.to_array();
    sky.cloud_color = clouds.to_array();
    sky.ambient_strength *= 0.27 + 0.73 * twilight;
    lighting.sun_direction = direction.to_array();
    lighting.sun_color = warm
        .lerp(Vec3::from_array(lighting.sun_color), day)
        .to_array();
    let high_sun = smoothstep(0.60, 0.95, elevation);
    lighting.sun_intensity *= smoothstep(-0.02, 0.45, elevation) * (1.0 - 0.25 * high_sun);
}

fn smoothstep(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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

pub(crate) fn atmosphere_lighting(sky: &Sky, light: &Environment) -> SkyLighting {
    let dir = Vec3::from_array(light.sun_direction).normalize_or_zero();

    let clear = Vec3::from_array(sky.horizon).lerp(Vec3::from_array(sky.zenith), 2.0 / 2.48);
    let overcast = sky.coverage * (1.0 - (-sky.density * sky.thickness * 0.52).exp());
    let cloud_fill = Vec3::from_array(sky.cloud_color)
        * (clear * 0.53 + Vec3::splat(0.14 + 0.14 * dir.y.max(0.0)));
    let sky_fill = clear.lerp(cloud_fill, overcast) * sky.ambient_strength;
    SkyLighting {
        sky: sky_fill,
        ground: sky_fill * Vec3::new(1.0, 0.88, 0.74) * sky.ground_bounce,
        sun: dir.extend(light.sun_intensity * (1.0 - overcast * 0.65) * dir.y.max(0.0).sqrt()),
        sun_color: Vec3::from_array(light.sun_color),
    }
}

pub(crate) fn material(
    sky: &Sky,
    light: &Environment,
    noise: Handle<Image>,
    wind: Option<&super::config::Wind>,
) -> CloudSkyMaterial {
    let flow = wind
        .and_then(|wind| {
            wind.response
                .map(|response| super::wind_flow(wind, response.cloud))
        })
        .unwrap_or_else(|| Vec2::from_array(sky.wind).extend(0.0).extend(0.0));
    CloudSkyMaterial(SkyBindings {
        params: SkyUniform {
            horizon: Vec3::from_array(sky.horizon).extend(0.0),
            zenith: Vec3::from_array(sky.zenith).extend(0.0),
            cloud_color: Vec3::from_array(sky.cloud_color).extend(0.0),
            layer: Vec4::new(sky.base_height, sky.thickness, sky.density, sky.coverage),
            motion: Vec4::new(0.0, 0.0, sky.scale, sky.cloud_update_hz as f32),
            wind: flow,
            sun: Vec3::from_array(light.sun_direction)
                .normalize_or_zero()
                .extend(sky.steps as f32),
            sun_color: Vec3::from_array(light.sun_color).extend(light.sun_intensity),
        },
        noise,
    })
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
        let batches = replacements(&sky, &mesh, &HashSet::new()).unwrap();
        assert!(batches.len() >= 5);
        assert!(batches.iter().all(|(object, _)| matches!(
            mesh.objects[*object].name.as_str(),
            "Sphere01" | "Sphere02" | "Sphere03"
        )));
        assert!(replacements(&sky, &mesh, &HashSet::from(["Sphere02"])).is_err());
        let mut kept = sky.clone();
        kept.replace_skybox = false;
        assert!(replacements(&kept, &mesh, &HashSet::new())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn fixed_times_move_the_sun_and_follow_the_daylight_curve() {
        let mut packs = super::super::tests::starter();
        let (_, authored) = scene(&packs, "sn13").unwrap();
        let across = Vec3::Y
            .cross(Vec3::from_array(authored.sun_direction))
            .normalize();
        let mut lighting = Vec::new();
        for time in ["07:00", "12:00", "18:30", "23:30"] {
            packs.packs[0]
                .config
                .scenes
                .iter_mut()
                .find(|scene| scene.stage == "sn13")
                .unwrap()
                .sky
                .as_mut()
                .unwrap()
                .fixed_time = Some(time.to_owned());
            let (profile, environment) = scene(&packs, "sn13").unwrap();
            lighting.push(atmosphere_lighting(&profile, &environment));
        }
        let [dawn, noon, dusk, night] = lighting.as_slice() else {
            unreachable!()
        };
        assert!(dawn.sun.truncate().dot(across) > 0.8);
        assert!(noon.sun.y > 0.9);
        assert!(dusk.sun.truncate().dot(across) < -0.8);
        assert!(noon.sun.w > dawn.sun.w && dawn.sun.w > dusk.sun.w);
        assert_eq!(night.sun.w, 0.0);
        assert!(noon.sky.length() > dusk.sky.length() && dusk.sky.length() > night.sky.length());
        for invalid in ["7:00", "24:00", "05:60", "sunrise", "０７:００"] {
            assert!(super::super::config::sky_minutes(invalid).is_none());
        }
    }
}
