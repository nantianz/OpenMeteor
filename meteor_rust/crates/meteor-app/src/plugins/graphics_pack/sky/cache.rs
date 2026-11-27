use super::*;
use crate::plugins::{camera_plugin::GameplayCamera, stage_material::FrameUniforms};
use bevy::render::{
    render_asset::RenderAssets,
    render_resource::{binding_types::*, *},
    renderer::{RenderContext, RenderGraph, RenderGraphSystems, RenderQueue},
    texture::GpuImage,
    Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
};
use bevy::shader::ShaderDefVal;
use std::time::{Duration, Instant};

const WIDTH: u32 = 3072;
const HEIGHT: u32 = 1536;
const ROWS_PER_FRAME: u32 = 64;
const CLOUD_TILE_SIZE: u32 = 16;
const FORMAT: TextureFormat = TextureFormat::Rgba16Float;
const MAX_BLEND_SECONDS: f32 = 0.055;

const MAX_REPROJECT_SHIFT: f32 = 1.0;
const MAX_SHIFT_PER_CLOUD_DISTANCE: f32 = 0.015;
#[derive(Clone, Copy, Default, ShaderType)]
pub(crate) struct CacheView {
    origin_time: Vec4,
    previous_origin_time: Vec4,
    state: UVec4,
    drift: Vec4,
    previous_drift: Vec4,
    horizon: Vec4,
    zenith: Vec4,
    layer: Vec4,
}

#[derive(Clone, PartialEq)]
struct Profile {
    material: AssetId<CloudSkyMaterial>,
    sky: SkyUniform,
    noise: Handle<Image>,
    weather: Handle<Image>,
    cycling: bool,
}

impl Profile {
    fn preserves_panorama(&self, next: &Self) -> bool {
        self.cycling
            && next.cycling
            && self.material == next.material
            && self.noise == next.noise
            && self.weather == next.weather
            && self.sky.layer == next.sky.layer
            && self.sky.motion == next.sky.motion
            && self.sky.weather == next.sky.weather
            && self.sky.wind == next.sky.wind
            && self.sky.sun.w == next.sky.sun.w
    }
}

#[derive(Resource, Default)]
struct Source {
    profile: Option<Profile>,
    eye_time: Vec4,
}

struct Bake {
    eye_time: Vec4,
    slot: usize,
    row: u32,
    end_row: u32,
    travel: Vec2,
    bindings: BindGroup,
}

fn cloud_rows(layer: Vec4, eye_y: f32) -> std::ops::Range<u32> {
    let horizon = HEIGHT / 3;
    if eye_y < layer.x {
        (horizon - CLOUD_TILE_SIZE)..HEIGHT
    } else if eye_y > layer.x + layer.y {
        0..(horizon + CLOUD_TILE_SIZE)
    } else {
        0..HEIGHT
    }
}
fn can_reproject(anchor: Vec3, eye: Vec3, wind_delta: Vec2, layer: Vec4) -> bool {
    let displacement = eye - anchor - Vec3::new(wind_delta.x, 0.0, wind_delta.y);
    if displacement == Vec3::ZERO {
        return true;
    }
    let below = layer.x - anchor.y.max(eye.y);
    let above = anchor.y.min(eye.y) - (layer.x + layer.y);
    let gap = below.max(above).max(0.0);
    let limit = (gap * MAX_SHIFT_PER_CLOUD_DISTANCE).min(MAX_REPROJECT_SHIFT);
    displacement.length_squared() <= limit * limit
}

struct Panorama {
    views: [TextureView; 2],
    reflection_view: TextureView,
    sampler: Sampler,
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
    sky: UniformBuffer<SkyUniform>,
    stripe: UniformBuffer<CacheView>,
    view: UniformBuffer<CacheView>,
    profile: Option<Profile>,
    bake: Option<Bake>,
    published: Option<Instant>,
}

#[derive(Resource, Default)]
pub struct CloudCache(Option<Panorama>);

#[cfg(test)]
pub(super) fn ready(app: &App) -> bool {
    app.sub_app(RenderApp)
        .world()
        .resource::<CloudCache>()
        .0
        .as_ref()
        .is_some_and(|panorama| panorama.view.get().state.x != 0)
}

impl CloudCache {
    pub(crate) fn reflection_bindings(&self) -> Option<(&TextureView, &Sampler, &Buffer)> {
        let gpu = self.0.as_ref()?;
        Some((&gpu.reflection_view, &gpu.sampler, gpu.view.buffer()?))
    }

    pub(super) fn attach(&self, group: &mut UnpreparedBindGroup) -> Result<(), AsBindGroupError> {
        let gpu = self.0.as_ref().ok_or(AsBindGroupError::RetryNextUpdate)?;
        group.bindings.extend([
            (
                4,
                OwnedBindingResource::TextureView(TextureViewDimension::D2, gpu.views[0].clone()),
            ),
            (
                5,
                OwnedBindingResource::TextureView(TextureViewDimension::D2, gpu.views[1].clone()),
            ),
            (
                6,
                OwnedBindingResource::Sampler(SamplerBindingType::Filtering, gpu.sampler.clone()),
            ),
            (
                7,
                OwnedBindingResource::Buffer(gpu.view.buffer().unwrap().clone()),
            ),
        ]);
        Ok(())
    }

    pub(super) fn layout(entries: &mut Vec<BindGroupLayoutEntry>) {
        let float = TextureSampleType::Float { filterable: true };
        entries.extend([
            texture_2d(float).build(4, ShaderStages::FRAGMENT),
            texture_2d(float).build(5, ShaderStages::FRAGMENT),
            sampler(SamplerBindingType::Filtering).build(6, ShaderStages::FRAGMENT),
            uniform_buffer::<CacheView>(false).build(7, ShaderStages::FRAGMENT),
        ]);
    }
}

pub(super) fn install(app: &mut App) {
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<CloudCache>()
            .init_resource::<Source>()
            .add_systems(ExtractSchedule, extract)
            .add_systems(
                Render,
                initialize
                    .after(RenderSystems::ExtractCommands)
                    .before(RenderSystems::PrepareAssets),
            )
            .add_systems(RenderGraph, update.in_set(RenderGraphSystems::Begin));
    }
}

fn extract(
    mut commands: Commands,
    materials: Extract<Res<Assets<CloudSkyMaterial>>>,
    domes: Extract<Query<(&MeshMaterial3d<CloudSkyMaterial>, Has<SkyCycle>), With<CloudSky>>>,
    cameras: Extract<Query<(&GlobalTransform, &Camera, Has<GameplayCamera>), With<Camera3d>>>,
    frame: Extract<Option<Res<FrameUniforms>>>,
) {
    let profile = domes.iter().find_map(|(handle, cycling)| {
        let material = materials.get(&handle.0)?;
        Some(Profile {
            material: handle.0.id(),
            sky: material.0.params.clone(),
            noise: material.0.noise.clone(),
            weather: material.0.weather.clone(),
            cycling,
        })
    });
    let eye = cameras
        .iter()
        .filter(|(_, camera, _)| camera.is_active)
        .min_by_key(|(_, camera, gameplay)| (!*gameplay, camera.order))
        .map_or(Vec3::ZERO, |(transform, _, _)| transform.translation());
    commands.insert_resource(Source {
        profile,
        eye_time: eye.extend(frame.as_ref().map_or(0.0, |frame| frame.seconds as f32)),
    });
}

fn initialize(
    source: Res<Source>,
    mut cache: ResMut<CloudCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    pipelines: Res<PipelineCache>,
) {
    if source.profile.is_none() || cache.0.is_some() {
        return;
    }
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("cloud panorama"),
        size: Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 2,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format: FORMAT,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
        view_formats: &[],
    });
    let views = std::array::from_fn(|slot| {
        texture.create_view(&TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2),
            base_array_layer: slot as u32,
            array_layer_count: Some(1),
            ..default()
        })
    });
    let reflection_view = texture.create_view(&TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..default()
    });
    let sampler = device.create_sampler(&SamplerDescriptor {
        address_mode_u: AddressMode::Repeat,
        min_filter: FilterMode::Linear,
        mag_filter: FilterMode::Linear,
        ..default()
    });
    let layout = BindGroupLayoutDescriptor::new(
        "cloud panorama compute",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<SkyUniform>(false),
                texture_3d(TextureSampleType::Float { filterable: true }),
                binding_types::sampler(SamplerBindingType::Filtering),
                uniform_buffer::<CacheView>(false),
                texture_storage_2d(FORMAT, StorageTextureAccess::WriteOnly),
                texture_2d(TextureSampleType::Float { filterable: true }),
            ),
        ),
    );
    let pipeline = pipelines.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("cloud panorama compute".into()),
        layout: vec![layout.clone()],
        shader: SHADER,
        shader_defs: vec![ShaderDefVal::Bool("CLOUD_CACHE_GENERATE".into(), true)],
        entry_point: Some("bake_clouds".into()),
        ..default()
    });
    let mut view = UniformBuffer::<CacheView>::default();
    view.write_buffer(&device, &queue);
    cache.0 = Some(Panorama {
        views,
        reflection_view,
        sampler,
        layout,
        pipeline,
        view,
        sky: UniformBuffer::default(),
        stripe: UniformBuffer::default(),
        profile: None,
        bake: None,
        published: None,
    });
}

fn update(
    source: Res<Source>,
    mut cache: ResMut<CloudCache>,
    images: Res<RenderAssets<GpuImage>>,
    pipelines: Res<PipelineCache>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    mut context: RenderContext,
) {
    let Some(gpu) = &mut cache.0 else { return };
    if gpu.profile != source.profile {
        let preserve = gpu
            .profile
            .as_ref()
            .zip(source.profile.as_ref())
            .is_some_and(|(current, next)| current.preserves_panorama(next));
        gpu.profile.clone_from(&source.profile);
        if !preserve {
            gpu.bake = None;
            gpu.published = None;
            gpu.view.set(CacheView::default());
            gpu.view.write_buffer(&device, &queue);
        }
    }
    let Some(profile) = gpu
        .profile
        .as_ref()
        .filter(|p| p.sky.layer.z > 0.0 && p.sky.layer.w > 0.0)
    else {
        return;
    };
    let Some(pipeline) = pipelines.get_compute_pipeline(gpu.pipeline) else {
        return;
    };
    let travel = super::super::wind_travel_for_flow(profile.sky.wind, f64::from(source.eye_time.w))
        .as_vec2();
    let mut current = *gpu.view.get();
    if current.drift.xy() != travel {
        current.drift.x = travel.x;
        current.drift.y = travel.y;
        gpu.view.set(current);
        gpu.view.write_buffer(&device, &queue);
    }
    let interval = Duration::from_secs_f32(1.0 / profile.sky.motion.w);
    if gpu.bake.is_none() {
        let mut previous = *gpu.view.get();
        if let Some(published) = gpu.published.filter(|_| previous.state.w != 0) {
            let duration = interval.as_secs_f32().min(MAX_BLEND_SECONDS);
            let progress = (published.elapsed().as_secs_f32() / duration).min(1.0);
            let blend = (255.0 * progress).round() as u32;
            if blend != previous.state.z {
                previous.state.z = blend;
                previous.state.w = u32::from(blend != 255);
                gpu.view.set(previous);
                gpu.view.write_buffer(&device, &queue);
            }
        }
        if gpu
            .published
            .is_some_and(|published| published.elapsed() < interval)
            || (previous.state.x != 0
                && gpu.sky.get() == &profile.sky
                && can_reproject(
                    previous.origin_time.truncate(),
                    source.eye_time.truncate(),
                    travel - previous.drift.zw(),
                    profile.sky.layer,
                ))
        {
            return;
        }
        let (Some(noise), Some(weather)) =
            (images.get(&profile.noise), images.get(&profile.weather))
        else {
            return;
        };
        let slot = 1 - previous.state.y as usize;
        gpu.sky.set(profile.sky.clone());
        gpu.sky.write_buffer(&device, &queue);
        if gpu.stripe.buffer().is_none() {
            gpu.stripe.write_buffer(&device, &queue);
        }
        let bindings = device.create_bind_group(
            "cloud panorama compute",
            &pipelines.get_bind_group_layout(&gpu.layout),
            &BindGroupEntries::sequential((
                gpu.sky.binding().unwrap(),
                &noise.texture_view,
                &noise.sampler,
                gpu.stripe.binding().unwrap(),
                &gpu.views[slot],
                &weather.texture_view,
            )),
        );
        let rows = cloud_rows(profile.sky.layer, source.eye_time.y);
        gpu.bake = Some(Bake {
            eye_time: source.eye_time,
            slot,
            row: rows.start,
            end_row: rows.end,
            travel,
            bindings,
        });
    }
    let bake = gpu.bake.as_mut().unwrap();
    let rows = ROWS_PER_FRAME.min(bake.end_row - bake.row);
    gpu.stripe.set(CacheView {
        origin_time: bake.eye_time,
        state: UVec4::new(bake.row, rows, 0, 0),
        drift: bake.travel.extend(0.0).extend(0.0),
        ..default()
    });
    gpu.stripe.write_buffer(&device, &queue);
    {
        let mut pass = context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor {
                label: Some("cloud panorama stripe"),
                ..default()
            });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bake.bindings, &[]);
        pass.dispatch_workgroups(
            WIDTH.div_ceil(CLOUD_TILE_SIZE),
            rows.div_ceil(CLOUD_TILE_SIZE),
            1,
        );
    }
    bake.row += rows;
    if bake.row == bake.end_row {
        let previous = *gpu.view.get();
        gpu.view.set(CacheView {
            origin_time: bake.eye_time,
            previous_origin_time: previous.origin_time,
            state: UVec4::new(1, bake.slot as u32, 0, previous.state.x),
            drift: travel.extend(bake.travel.x).extend(bake.travel.y),
            previous_drift: previous.drift.zw().extend(0.0).extend(0.0),
            horizon: gpu.sky.get().horizon,
            zenith: gpu.sky.get().zenith,
            layer: gpu.sky.get().layer,
        });
        gpu.view.write_buffer(&device, &queue);
        gpu.published = Some(Instant::now());
        gpu.bake = None;
    }
}
#[cfg(test)]
#[test]
fn reuses_cloud_panorama_for_small_viewer_and_wind_motion_outside_the_layer() {
    let layer = Vec4::new(100.0, 30.0, 1.0, 1.0);
    let below = Vec3::new(4.0, 10.0, 2.0);
    assert!(can_reproject(below, below, Vec2::new(0.4, 0.0), layer));
    assert!(can_reproject(
        below,
        below + Vec3::X,
        Vec2::new(0.8, 0.0),
        layer
    ));
    assert!(!can_reproject(
        below,
        below + Vec3::X * 2.0,
        Vec2::ZERO,
        layer
    ));
    let near = below.with_y(99.0);
    let inside = below.with_y(110.0);
    let above = below.with_y(180.0);
    assert!(!can_reproject(near, near, Vec2::new(0.4, 0.0), layer));
    assert!(can_reproject(inside, inside, Vec2::ZERO, layer));
    assert!(!can_reproject(inside, inside, Vec2::new(0.4, 0.0), layer));
    assert!(!can_reproject(near, inside, Vec2::ZERO, layer));
    assert!(can_reproject(above, above, Vec2::new(0.4, 0.0), layer));
}
