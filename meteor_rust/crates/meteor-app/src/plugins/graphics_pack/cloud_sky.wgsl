#import meteor::cloud_panorama::{CloudCacheView, panorama_uv, PI, HORIZON_UV};

#ifndef CLOUD_CACHE_GENERATE
#import bevy_pbr::mesh_view_bindings::view
#endif

struct SkyUniform {
    horizon: vec4<f32>,
    zenith: vec4<f32>,
    cloud_color: vec4<f32>,
    layer: vec4<f32>,
    motion: vec4<f32>,
    wind: vec4<f32>,
    sun: vec4<f32>,
    sun_color: vec4<f32>,
    weather: vec4<f32>,
    celestial: vec4<f32>,
}

#ifdef CLOUD_CACHE_GENERATE
@group(0) @binding(0) var<uniform> sky: SkyUniform;
@group(0) @binding(1) var density_field: texture_3d<f32>;
@group(0) @binding(2) var density_sampler: sampler;
@group(0) @binding(3) var<uniform> bake: CloudCacheView;
@group(0) @binding(4) var output_image: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var weather_map: texture_2d<f32>;
#else
struct SkyVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) direction: vec3<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var density_field: texture_3d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var density_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var weather_map: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var cloud_a: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var cloud_b: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var cloud_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var<uniform> cache: CloudCacheView;

@vertex
fn vertex(@location(0) position: vec3<f32>) -> SkyVertex {
    var out: SkyVertex;
    let clip = view.clip_from_world * vec4<f32>(view.world_position + position, 1.0);
    out.position = vec4<f32>(clip.xy, 0.0, clip.w);
    out.direction = position;
    return out;
}
#endif

fn field(p: vec3<f32>) -> vec4<f32> {
    return textureSampleLevel(density_field, density_sampler, p, 0.0);
}

fn cloud_height(y: f32) -> f32 {
    return (y - sky.layer.x) / sky.layer.y;
}

struct CloudProfile {
    coverage: f32,
    base: f32,
    top: f32,
};

fn cloud_profile(position: vec2<f32>) -> CloudProfile {
    let map = textureSampleLevel(weather_map, density_sampler,
        (position + sky.weather.xy) * sky.weather.z, 0.0).rgb;
    return CloudProfile(map.r, 0.031 + 0.055 * map.b, 0.40 + 0.59 * map.g);
}

fn cloud_body(h: f32, weather: CloudProfile, noise: vec4<f32>) -> f32 {
    let crown = smoothstep(weather.top * 0.50, weather.top, h);
    return weather.coverage + 0.30 * (noise.r - 0.5) + 0.23 * (noise.a - 0.45)
        + 0.09 * (noise.g - 0.5) - 0.23 * crown;
}

fn extinction(h: f32, weather: CloudProfile, body: f32, threshold: f32, definition: f32) -> f32 {
    let base = smoothstep(weather.base, weather.base + 0.108, h);
    let crown = 1.0 - smoothstep(weather.top * 0.83, weather.top, h);
    let condensate = smoothstep(threshold - 0.012, threshold + mix(0.20, 0.087, definition), body);
    return condensate * base * crown * sky.layer.z;
}

fn view_density(lattice: vec3<f32>, h: f32, weather: CloudProfile, detail: f32, threshold: f32, definition: f32) -> f32 {
    if (h <= weather.base || h >= weather.top) { return 0.0; }
    var body = cloud_body(h, weather, field(lattice));
    if (detail > 0.12 && body > threshold - 0.10 && body < threshold + 0.29) {
        let fine = field(lattice * 2.37 + vec3<f32>(0.06, 0.21, 0.37));

        let edge = 1.0 - smoothstep(threshold + 0.105, threshold + 0.29, body);
        let gap = 1.0 - fine.a;
        let erosion = 0.11 * gap * gap + 0.043 * (1.0 - mix(fine.g, fine.b, 0.42));
        body -= erosion * edge * detail * mix(0.53, 1.18, definition);
    }
    return extinction(h, weather, body, threshold, definition);
}

fn light_density(lattice: vec3<f32>, h: f32, weather: CloudProfile, threshold: f32, definition: f32) -> f32 {
    if (h <= weather.base || h >= weather.top) { return 0.0; }
    return extinction(h, weather, cloud_body(h, weather, field(lattice)), threshold, definition);
}

fn phase(cosine: f32, g: f32) -> f32 {
    let base = 1.0 + g * g - 2.0 * g * cosine;
    return (1.0 - g * g) / (base * sqrt(base));
}

fn integrate_volume(ray: vec3<f32>, origin: vec3<f32>, travel: vec2<f32>, sample_phase: f32) -> vec4<f32> {
    let sun_dot = clamp(dot(ray, sky.sun.xyz), -1.0, 1.0);
    let sunlight = sky.sun_color.xyz * min(sky.sun_color.w, 8.0) * smoothstep(0.0, 0.62, sky.sun.y);
    if (abs(ray.y) < 0.002 || sky.layer.z == 0.0 || sky.layer.w == 0.0) {
        return vec4<f32>(0.0);
    }
    let a = (sky.layer.x - origin.y) / ray.y;
    let b = (sky.layer.x + sky.layer.y - origin.y) / ray.y;
    let near = max(min(a, b), 0.0);
    let far = min(max(a, b), 2200.0);
    if (far <= near || near >= 2100.0) {
        return vec4<f32>(0.0);
    }
    let distance_haze = smoothstep(410.0, 2100.0, near);
    let length = far - near;

    let quality = clamp(length * sky.motion.z * 0.26, 0.5, 1.0)
        * mix(1.0, 0.53, smoothstep(160.0, 790.0, near));
    let sample_count = clamp(sky.sun.w * quality, 8.0, sky.sun.w);
    let steps = u32(ceil(sample_count));
    let stride = length / sample_count;
    let light_step = sky.layer.y * 0.12;
    let forward_scatter = min(0.18 * phase(sun_dot, 0.65) + 0.095 * phase(sun_dot, -0.21), 0.78);
    let sky_fill = mix(sky.horizon.xyz, sky.zenith.xyz, 2.0 / 2.48);
    let aerial = mix(sky.horizon.xyz, sky.zenith.xyz, pow(max(ray.y, 0.0), 0.48));
    let detail = 1.0 - smoothstep(320.0, 900.0, near);
    let definition = sky.motion.x * (1.0 - smoothstep(450.0, 1050.0, near));
    let scale = sky.motion.z / 8.0;
    let lattice_bias = vec3<f32>(0.31, 0.26, 0.13);
    let first = origin + ray * near;
    let lattice_start = (first - vec3<f32>(travel.x, 0.0, travel.y))
        * scale + lattice_bias;
    let lattice_stride = ray * (stride * scale);
    let height_start = cloud_height(first.y);
    let height_stride = ray.y * stride / sky.layer.y;
    let light_stride = sky.sun.xyz * (light_step * scale);
    let light_height = sky.sun.y * 0.12;
    let threshold = mix(0.64, 0.22, sky.layer.w);
    let refine_density = smoothstep(0.11, 0.17, stride * sky.motion.z);
    var added = vec3<f32>(0.0);
    var transmission = 1.0;
    for (var i = 0u; i < steps; i += 1u) {
        let segment = min(sample_count - f32(i), 1.0);

        let shift = (fract(sample_phase + f32(i) * 0.61803399) - 0.5) * 0.42;
        let midpoint = f32(i) + (0.5 + shift) * segment;
        var lattice = lattice_start + midpoint * lattice_stride;
        var h = height_start + midpoint * height_stride;
        let weather = cloud_profile((lattice.xz - lattice_bias.xz) / scale);
        if (weather.coverage < threshold - 0.17) { continue; }
        var density: f32;
        if (refine_density > 0.0) {
            let offset = segment * 0.25 * refine_density;
            let front = view_density(lattice - offset * lattice_stride, h - offset * height_stride,
                weather, detail, threshold, definition);
            let back = view_density(lattice + offset * lattice_stride, h + offset * height_stride,
                weather, detail, threshold, definition);
            density = 0.5 * (front + back);

            let weighted = offset * (back - front) / max(front + back, 0.0001);
            lattice += weighted * lattice_stride;
            h += weighted * height_stride;
        } else {
            density = view_density(lattice, h, weather, detail, threshold, definition);
        }
        if (density <= 0.0001) { continue; }
        let optical_depth = light_step * (
            0.70 * light_density(lattice + light_stride * 0.35, h + light_height * 0.35, weather, threshold, definition)
            + 1.45 * light_density(lattice + light_stride * 1.4, h + light_height * 1.4, weather, threshold, definition)
            + 2.4 * light_density(lattice + light_stride * 3.35, h + light_height * 3.35, weather, threshold, definition));
        let direct = exp(-optical_depth) * (0.48 + forward_scatter);

        let scatter = 0.23 * exp(-0.45 * optical_depth) + 0.155 * exp(-0.11 * optical_depth);
        let height = smoothstep(weather.base, weather.top, h);
        let rim = 1.0 - clamp(density / sky.layer.z, 0.0, 1.0);
        let ambient = sky_fill * (0.31 + 0.48 * height) + vec3<f32>(0.066 + 0.045 * rim);
        let cloud_light = sky.cloud_color.rgb * (ambient + sunlight * (direct + scatter));
        let distance = near + midpoint * stride;
        let haze = 1.0 - exp(-0.00052 * distance);
        let radiance = mix(cloud_light, aerial, haze);
        let through_step = exp(-density * stride * segment);
        added += transmission * (1.0 - through_step) * radiance;
        transmission *= through_step;
        if (transmission < 0.015) { break; }
    }
    return vec4<f32>(added, 1.0 - transmission) * (1.0 - distance_haze);
}

#ifdef CLOUD_CACHE_GENERATE

const TILE_SIZE: u32 = 16u;
const HALO_SIZE: u32 = TILE_SIZE + 2u;
var<workgroup> tile: array<vec4<f32>, HALO_SIZE * HALO_SIZE>;

fn cache_jitter(point: vec2<u32>) -> f32 {
    var key = point.x * 2246822519u ^ point.y * 3266489917u;
    key = (key ^ (key >> 15u)) * 668265263u;
    return f32((key ^ (key >> 13u)) & 0xffffu) / 65536.0;
}

fn bake_ray(point: vec2<u32>, size: vec2<u32>) -> vec4<f32> {
    let uv = (vec2<f32>(point) + 0.5) / vec2<f32>(size);
    let azimuth = (uv.x - 0.5) * (2.0 * PI);
    let horizon_offset = uv.y - HORIZON_UV;
    let elevation = horizon_offset * (0.5 * PI)
        / select(HORIZON_UV, 1.0 - HORIZON_UV, horizon_offset >= 0.0);
    let ray = vec3<f32>(cos(elevation) * cos(azimuth), sin(elevation), cos(elevation) * sin(azimuth));
    return integrate_volume(ray, bake.origin_time.xyz, bake.drift.xy, cache_jitter(point));
}

@compute @workgroup_size(TILE_SIZE, TILE_SIZE)
fn bake_clouds(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>,
               @builtin(local_invocation_index) index: u32) {
    let size = textureDimensions(output_image);
    let start = TILE_SIZE * group.xy + vec2<u32>(0u, bake.state.x);
    for (var cell = index; cell < HALO_SIZE * HALO_SIZE; cell += TILE_SIZE * TILE_SIZE) {
        let p = vec2<i32>(start + vec2<u32>(cell % HALO_SIZE, cell / HALO_SIZE)) - vec2<i32>(1);
        let wrapped = vec2<u32>(u32((p.x + i32(size.x)) % i32(size.x)), u32(clamp(p.y, 0, i32(size.y) - 1)));
        tile[cell] = bake_ray(wrapped, size);
    }
    workgroupBarrier();
    let point = start + local.xy;
    if (any(point >= size) || point.y - bake.state.x >= bake.state.y) { return; }
    let center_index = (local.y + 1u) * HALO_SIZE + local.x + 1u;
    let center = tile[center_index];
    var filtered = vec4<f32>(0.0);
    var total = 0.0;
    for (var y = 0u; y < 3u; y += 1u) {
        for (var x = 0u; x < 3u; x += 1u) {
            let sample = tile[(local.y + y) * HALO_SIZE + local.x + x];
            let weight = select(1.0, 2.0, x == 1u) * select(1.0, 2.0, y == 1u)
                * (1.0 - 0.85 * smoothstep(0.10, 0.45, abs(sample.a - center.a)));
            filtered += sample * weight;
            total += weight;
        }
    }

    let across = abs(tile[center_index - 1u] - tile[center_index + 1u]);
    let vertical = abs(tile[center_index - HALO_SIZE] - tile[center_index + HALO_SIZE]);
    let contrast = max(across, vertical);
    let feature = max(contrast.a, max(contrast.r, max(contrast.g, contrast.b)));
    let strength = mix(0.83, 0.32, smoothstep(0.048, 0.21, feature));
    textureStore(output_image, vec2<i32>(point), mix(center, filtered / total, strength));
}
#else
fn cached_sample(slot: u32, anchor: vec4<f32>, ray: vec3<f32>, origin: vec3<f32>, drift: vec2<f32>) -> vec4<f32> {
    let uv = panorama_uv(anchor.xyz, ray, origin, drift, sky.layer.xy);
    if (uv.z == 0.0) { return vec4<f32>(0.0); }
    if (slot == 0u) { return textureSampleLevel(cloud_a, cloud_sampler, uv.xy, 0.0); }
    return textureSampleLevel(cloud_b, cloud_sampler, uv.xy, 0.0);
}

fn cache_volume(ray: vec3<f32>, origin: vec3<f32>) -> vec4<f32> {
    let current = cached_sample(cache.state.y, cache.origin_time, ray, origin, cache.drift.xy - cache.drift.zw);
    if (cache.state.w == 0u) { return current; }
    let previous = cached_sample(1u - cache.state.y, cache.previous_origin_time, ray, origin, cache.drift.xy - cache.previous_drift.xy);
    return mix(previous, current, f32(cache.state.z) / 255.0);
}

@fragment
fn fragment(input: SkyVertex) -> @location(0) vec4<f32> {
    let ray = normalize(input.direction);
    let sun_dot = clamp(dot(ray, sky.sun.xyz), -1.0, 1.0);
    let sunlight = sky.sun_color.xyz * min(sky.sun_color.w, 8.0) * max(sky.sun.y, 0.0);
    let sun_disc = sunlight * (0.10 * pow(max(sun_dot, 0.0), 14.0)
        + 1.5 * smoothstep(0.999983, 0.999993, sun_dot));

    let moon_visibility = sky.celestial.x * smoothstep(0.0, 0.08, sky.sun.y) * smoothstep(0.0, 0.03, sky.sun_color.w);
    let moon_disc = moon_visibility * vec3<f32>(0.68, 0.75, 0.88)
        * (0.50 * smoothstep(0.99986, 0.99991, sun_dot) + 0.08 * pow(max(sun_dot, 0.0), 500.0));
    let color = mix(sky.horizon.xyz, sky.zenith.xyz, pow(max(ray.y, 0.0), 0.48))
        + select(sun_disc, moon_disc, sky.celestial.x > 0.5);
    var volume = vec4<f32>(0.0);
    if (sky.layer.z > 0.0 && sky.layer.w > 0.0) {
        if (cache.state.x != 0u) {
            volume = cache_volume(ray, view.world_position);
        } else {
            volume = integrate_volume(ray, view.world_position, cache.drift.xy, 0.5);
        }
    }
    return vec4<f32>(color * (1.0 - volume.a) + volume.rgb, 1.0);
}
#endif
