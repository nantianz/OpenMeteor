use super::config::{sky_minutes, Environment, Sky, SkyTime};
use super::GraphicsPacks;
use crate::plugins::{
    render_plugin::update_uv_scroll, simulation_play_now, stage_material::FrameUniforms, FrameClock,
};
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

pub(crate) mod cache;
mod clock;
mod noise;

const SHADER: Handle<Shader> = bevy::asset::uuid_handle!("1c5ee9c3-0985-436a-b829-287a3b623efb");
const PANORAMA_SHADER: Handle<Shader> =
    bevy::asset::uuid_handle!("b2a22c67-e87b-4946-9382-2e87b7d3a4b3");

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
    weather: Vec4,
    celestial: Vec4,
}

#[derive(AsBindGroup, Clone)]
pub(crate) struct SkyBindings {
    #[uniform(0)]
    params: SkyUniform,
    #[texture(1, dimension = "3d")]
    #[sampler(2)]
    noise: Handle<Image>,
    #[texture(3)]
    weather: Handle<Image>,
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
    pub night_factor: f32,
}

#[derive(Component)]
pub(crate) struct CloudSky(pub Option<SkyLighting>);

#[derive(Component)]
pub(crate) struct SkyCycle {
    base: (Sky, Environment),
    clock: SkyClock,
}

enum SkyClock {
    Cycle(clock::Cycle),
    System(clock::SystemClock),
}

#[derive(Clone)]
pub(crate) struct CloudTextures {
    noise: Handle<Image>,
    weather: Handle<Image>,
}

#[derive(Resource, Default)]
pub(crate) struct CloudNoise(Option<CloudTextures>);

impl CloudNoise {
    pub(crate) fn get(&mut self, images: &mut Assets<Image>) -> CloudTextures {
        self.0
            .get_or_insert_with(|| CloudTextures {
                noise: images.add(noise::image()),
                weather: images.add(noise::weather()),
            })
            .clone()
    }
}

pub(crate) fn install(app: &mut App) {
    app.add_plugins(MaterialPlugin::<CloudSkyMaterial>::default())
        .init_resource::<CloudNoise>()
        .add_systems(Update, update_cycle.after(update_uv_scroll));
    bevy::asset::load_internal_asset!(app, SHADER, "cloud_sky.wgsl", Shader::from_wgsl);
    register_panorama_shader(app);
    cache::install(app);
}

pub(crate) fn register_panorama_shader(app: &mut App) {
    if !app
        .world()
        .resource::<Assets<Shader>>()
        .contains(PANORAMA_SHADER.id())
    {
        bevy::asset::load_internal_asset!(
            app,
            PANORAMA_SHADER,
            "cloud_panorama.wgsl",
            Shader::from_wgsl
        );
    }
}

#[cfg(test)]
pub(crate) fn cache_ready(app: &App) -> bool {
    cache::ready(app)
}

pub(crate) fn scene(
    packs: &GraphicsPacks,
    stage: &str,
) -> Option<(Sky, Environment, Option<SkyCycle>)> {
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
    let (minutes, cycle) = match &profile.time {
        Some(SkyTime::Cycle {
            start_time,
            time_scale,
        }) => {
            let minutes = sky_minutes(start_time)?;
            let cycle = SkyCycle {
                base: (profile.clone(), lighting.clone()),
                clock: SkyClock::Cycle(clock::Cycle::new(
                    minutes,
                    *time_scale,
                    profile.cloud_update_hz,
                )),
            };
            (Some(f32::from(minutes)), Some(cycle))
        }
        Some(SkyTime::Fixed { time }) => (sky_minutes(time).map(f32::from), None),
        Some(SkyTime::System {}) => {
            let minutes = clock::local_minutes();
            if minutes.is_none() {
                warn!("{stage}: could not read the local time for the sky");
            }
            let cycle = SkyCycle {
                base: (profile.clone(), lighting.clone()),
                clock: SkyClock::System(clock::SystemClock::default()),
            };
            (minutes, Some(cycle))
        }
        None => (
            profile
                .fixed_time
                .as_deref()
                .and_then(sky_minutes)
                .map(f32::from),
            None,
        ),
    };
    if let Some(minutes) = minutes {
        sample_time(&mut profile, &mut lighting, minutes);
    }
    Some((profile, lighting, cycle))
}

pub(crate) fn update_cycle(
    frame_clock: Option<Res<FrameClock>>,
    frame: Option<Res<FrameUniforms>>,
    real_time: Option<Res<Time<Real>>>,
    mut domes: Query<(
        Option<&MeshMaterial3d<CloudSkyMaterial>>,
        &mut CloudSky,
        &mut SkyCycle,
    )>,
    mut materials: ResMut<Assets<CloudSkyMaterial>>,
) {
    let fallback = frame.as_ref().map_or(0.0, |frame| frame.seconds);
    let now = simulation_play_now(frame_clock.as_deref(), fallback);
    for (handle, mut sky, mut cycle) in &mut domes {
        let minutes = match &mut cycle.clock {
            SkyClock::Cycle(clock) => clock.sample(now),
            SkyClock::System(clock) => clock.sample(
                real_time
                    .as_ref()
                    .map_or(now, |time| time.elapsed_secs_f64()),
                clock::local_minutes,
            ),
        };
        let Some(minutes) = minutes else {
            continue;
        };
        let (mut profile, mut light) = cycle.base.clone();
        sample_time(&mut profile, &mut light, minutes);
        if let Some(mut material) = handle.and_then(|handle| materials.get_mut(&handle.0)) {
            material.0.params.set_lighting(&profile, &light);
        }
        if sky.0.is_some() {
            sky.0 = Some(atmosphere_lighting(&profile, &light));
        }
    }
}

fn sample_time(sky: &mut Sky, lighting: &mut Environment, minutes: f32) {
    let hour_angle = (minutes - 720.0) * (std::f32::consts::TAU / 1440.0);
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
    sky.night_factor = 1.0 - twilight;
    let mut horizon = Vec3::new(0.012, 0.022, 0.047);
    let mut zenith = Vec3::new(0.003, 0.008, 0.023);
    let dawn = minutes < 720.0;
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
    let high_sun = smoothstep(0.60, 0.95, elevation);
    let sun_intensity =
        lighting.sun_intensity * smoothstep(-0.02, 0.45, elevation) * (1.0 - 0.25 * high_sun);
    let moon_direction = -direction;
    let moon_fade = smoothstep(-0.02, 0.28, moon_direction.y);
    let moon_color = sky
        .moon
        .as_ref()
        .map_or([0.58, 0.68, 1.0], |moon| moon.color);
    let moon_intensity = sky.moon.as_ref().map_or(0.35, |moon| moon.intensity) * moon_fade;

    let moon_weight = smoothstep(-0.12, 0.12, -elevation);
    sky.moon_active = moon_weight >= 0.5;
    lighting.sun_direction = (if sky.moon_active {
        moon_direction
    } else {
        direction
    })
    .to_array();
    lighting.sun_color = warm
        .lerp(Vec3::from_array(lighting.sun_color), day)
        .lerp(Vec3::from_array(moon_color), moon_weight)
        .to_array();
    lighting.sun_intensity = if sky.moon_active {
        moon_intensity * (2.0 * moon_weight - 1.0)
    } else {
        sun_intensity * (1.0 - 2.0 * moon_weight)
    };
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
        night_factor: sky.night_factor,
    }
}

pub(crate) fn material(
    sky: &Sky,
    light: &Environment,
    textures: CloudTextures,
    wind: Option<&super::config::Wind>,
) -> CloudSkyMaterial {
    let flow = wind
        .and_then(|wind| {
            wind.response
                .map(|response| super::wind_flow(wind, response.cloud))
        })
        .unwrap_or_else(|| Vec2::from_array(sky.wind).extend(0.0).extend(0.0));
    let mut params = SkyUniform {
        layer: Vec4::new(sky.base_height, sky.thickness, sky.density, sky.coverage),
        motion: Vec4::new(
            sky.cloud_definition,
            0.0,
            sky.scale,
            sky.cloud_update_hz as f32,
        ),
        wind: flow,
        weather: Vec2::from_array(sky.weather_offset)
            .extend(sky.weather_scale)
            .extend(0.0),
        ..default()
    };
    params.set_lighting(sky, light);
    CloudSkyMaterial(SkyBindings {
        params,
        noise: textures.noise,
        weather: textures.weather,
    })
}

impl SkyUniform {
    fn set_lighting(&mut self, sky: &Sky, light: &Environment) {
        self.horizon = Vec3::from_array(sky.horizon).extend(0.0);
        self.zenith = Vec3::from_array(sky.zenith).extend(0.0);
        self.cloud_color = Vec3::from_array(sky.cloud_color).extend(0.0);
        self.sun = Vec3::from_array(light.sun_direction)
            .normalize_or_zero()
            .extend(sky.steps as f32);
        self.sun_color = Vec3::from_array(light.sun_color).extend(light.sun_intensity);
        self.celestial = Vec4::new(f32::from(sky.moon_active), 0., 0., 0.);
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
        let (sky, _, _) = scene(&packs, "sn13").unwrap();
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
        let (_, authored, _) = scene(&packs, "sn13").unwrap();
        let across = Vec3::Y
            .cross(Vec3::from_array(authored.sun_direction))
            .normalize();
        let mut lighting = Vec::new();
        for time in ["07:00", "12:00", "17:30", "23:30"] {
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
            let (profile, environment, _) = scene(&packs, "sn13").unwrap();
            lighting.push(atmosphere_lighting(&profile, &environment));
        }
        let [dawn, noon, dusk, night] = lighting.as_slice() else {
            unreachable!()
        };
        assert!(dawn.sun.truncate().dot(across) > 0.8);
        assert!(noon.sun.y > 0.9);
        assert!(dusk.sun.truncate().dot(across) < -0.8);
        assert!(noon.sun.w > dawn.sun.w && noon.sun.w > dusk.sun.w);
        assert!(night.sun.w > 0.0 && night.sun.w < dawn.sun.w);
        assert!(night.sun.y > 0.0 && night.sun_color.z > night.sun_color.x);
        assert!(noon.sky.length() > dusk.sky.length() && dusk.sky.length() > night.sky.length());
        for invalid in ["7:00", "24:00", "05:60", "sunrise", "０７:００"] {
            assert!(super::super::config::sky_minutes(invalid).is_none());
        }
    }

    #[test]
    fn realtime_sky_keeps_a_live_clock_and_moon_can_be_configured() {
        let mut packs = super::super::tests::starter();
        let sky = packs.packs[0]
            .config
            .scenes
            .iter_mut()
            .find(|scene| scene.stage == "sn13")
            .unwrap()
            .sky
            .as_mut()
            .unwrap();
        sky.time = Some(SkyTime::System {});
        sky.moon = Some(super::super::config::SkyMoon {
            color: [0.3, 0.5, 1.0],
            intensity: 0.25,
        });
        let (_, _, cycle) = scene(&packs, "sn13").unwrap();
        assert!(matches!(cycle.unwrap().clock, SkyClock::System(_)));
        let mut moon_sky = Sky::default();
        moon_sky.moon = Some(super::super::config::SkyMoon {
            color: [0.3, 0.5, 1.0],
            intensity: 0.25,
        });
        let mut moon_light = Environment::default();
        sample_time(&mut moon_sky, &mut moon_light, 0.0);
        assert!(moon_sky.moon_active && moon_light.sun_direction[1] > 0.0);
        assert_eq!(moon_light.sun_color, [0.3, 0.5, 1.0]);
        assert!(moon_light.sun_intensity > 0.20 && moon_light.sun_intensity <= 0.25);
    }

    #[test]
    fn clock_updates_lighting_without_a_cloud_rendering_material() {
        let mut app = App::new();
        app.init_resource::<Assets<CloudSkyMaterial>>()
            .init_resource::<FrameUniforms>()
            .add_systems(Update, update_cycle);
        let base = (Sky::default(), Environment::default());
        let (mut night, mut night_light) = base.clone();
        sample_time(&mut night, &mut night_light, 0.0);
        let entity = app
            .world_mut()
            .spawn((
                CloudSky(Some(atmosphere_lighting(&night, &night_light))),
                SkyCycle {
                    base,
                    clock: SkyClock::Cycle(clock::Cycle::new(0, 60.0, 15)),
                },
            ))
            .id();
        app.update();
        let night = app.world().get::<CloudSky>(entity).unwrap().0.unwrap();
        app.world_mut().resource_mut::<FrameUniforms>().seconds = 720.0;
        app.update();
        let noon = app.world().get::<CloudSky>(entity).unwrap().0.unwrap();
        assert!(night.sun.w > 0.0 && night.sun_color.z > night.sun_color.x);
        assert!(noon.sun.w > night.sun.w * 2.0 && noon.sky.length() > night.sky.length());
    }

    #[test]
    fn sun_moon_handoff_has_no_light_jump_or_invalid_direction() {
        for center in [342.0f32, 1098.0] {
            let mut previous: Option<(bool, f32)> = None;
            let mut switches = 0;
            for second in -600..=600 {
                let mut sky = Sky::default();
                let mut environment = Environment::default();
                sample_time(&mut sky, &mut environment, center + second as f32 / 60.0);
                let direction = Vec3::from_array(environment.sun_direction);
                assert!(direction.is_finite() && (direction.length() - 1.0).abs() < 0.001);
                assert!(environment.sun_intensity.is_finite() && environment.sun_intensity >= 0.0);
                if let Some((was_moon, prior)) = previous {
                    assert!((prior - environment.sun_intensity).abs() < 0.005);
                    if was_moon != sky.moon_active {
                        switches += 1;
                        assert!(
                            prior < 0.001 && environment.sun_intensity < 0.001,
                            "the source changes only after direct light has faded"
                        );
                    }
                }
                previous = Some((sky.moon_active, environment.sun_intensity));
            }
            assert_eq!(switches, 1, "each horizon transition changes source once");
        }
    }
}
