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
}

struct CloudCacheView {
    origin_time: vec4<f32>,
    previous_origin_time: vec4<f32>,
    state: vec4<u32>,
    drift: vec4<f32>,
    previous_drift: vec4<f32>,
}

#ifdef CLOUD_CACHE_GENERATE
@group(0) @binding(0) var<uniform> sky: SkyUniform;
@group(0) @binding(1) var density_field: texture_3d<f32>;
@group(0) @binding(2) var density_sampler: sampler;
@group(0) @binding(3) var<uniform> bake: CloudCacheView;
@group(0) @binding(4) var output_image: texture_storage_2d<rgba16float, write>;
#else
struct SkyVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) direction: vec3<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var density_field: texture_3d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var density_sampler: sampler;
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

fn cloud_body(h: f32, weather: f32, noise: vec4<f32>) -> f32 {
    return noise.r + 0.49 * (weather - 0.5) + 0.055 * (1.0 - h)
        + 0.16 * (noise.g - 0.5) + 0.20 * (noise.a - 0.49);
}

fn extinction(h: f32, top: f32, body: f32, threshold: f32) -> f32 {
    let profile = smoothstep(0.02, 0.17, h) * (1.0 - smoothstep(top * 0.59, top, h));
    return smoothstep(threshold, threshold + 0.24, body) * profile * sky.layer.z;
}

fn view_density(lattice: vec3<f32>, h: f32, weather: f32, top: f32, detail: f32, threshold: f32) -> f32 {
    if (h <= 0.02 || h >= min(top, 1.0)) { return 0.0; }
    var body = cloud_body(h, weather, field(lattice));
    if (detail > 0.4 && body > threshold - 0.15 && body < threshold + 0.38) {
        let fine = field(lattice * 2.48 + vec3<f32>(0.06, 0.21, 0.37));
        let boundary = smoothstep(threshold - 0.15, threshold - 0.04, body)
            * (1.0 - smoothstep(threshold + 0.23, threshold + 0.38, body));
        let gap = 1.0 - fine.a;
        let erosion = 0.32 * gap * gap * (0.60 + 0.40 * fine.g);
        body -= smoothstep(0.4, 0.9, detail) * boundary * erosion;
    }
    return extinction(h, top, body, threshold);
}

fn light_density(lattice: vec3<f32>, h: f32, weather: f32, top: f32, threshold: f32) -> f32 {
    if (h <= 0.02 || h >= min(top, 1.0)) { return 0.0; }
    return extinction(h, top, cloud_body(h, weather, field(lattice)), threshold);
}

fn phase(cosine: f32, g: f32) -> f32 {
    let base = 1.0 + g * g - 2.0 * g * cosine;
    return (1.0 - g * g) / (base * sqrt(base));
}

fn ray_offset(position: vec2<f32>) -> f32 {
    let pixel = vec2<u32>(position);
    var key = pixel.x * 2246822519u ^ pixel.y * 3266489917u;
    key = (key ^ (key >> 15u)) * 668265263u;
    let random = f32((key ^ (key >> 13u)) & 0xffffu) / 65536.0;
    return 0.15 + 0.70 * random;
}

fn integrate_volume(ray: vec3<f32>, origin: vec3<f32>, travel: vec2<f32>, offset: f32) -> vec4<f32> {
    let sun_dot = clamp(dot(ray, sky.sun.xyz), -1.0, 1.0);
    let sunlight = sky.sun_color.xyz * min(sky.sun_color.w, 8.0) * max(sky.sun.y, 0.0);
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
    let distance_haze = smoothstep(230.0, 2100.0, near);
    let length = far - near;

    let quality = clamp(length * sky.motion.z * 0.26, 0.5, 1.0)
        * mix(1.0, 0.42, smoothstep(160.0, 750.0, near));
    let steps = clamp(ceil(sky.sun.w * quality / 4.0) * 4.0, 8.0, sky.sun.w);
    let stride = length / steps;
    let light_step = sky.layer.y * 0.12;
    let forward_scatter = min(0.125 * phase(sun_dot, 0.62) + 0.11 * phase(sun_dot, -0.23), 0.72);
    let sky_fill = mix(sky.horizon.xyz, sky.zenith.xyz, 2.0 / 2.48);
    let detail = 1.0 - smoothstep(260.0, 800.0, near);
    let scale = sky.motion.z / 8.0;
    let lattice_bias = vec3<f32>(0.31, 0.26, 0.13);
    let first = origin + ray * (near + offset * stride);
    let lattice_start = (first - vec3<f32>(travel.x, 0.0, travel.y))
        * scale + lattice_bias;
    let lattice_stride = ray * (stride * scale);
    let height_start = cloud_height(first.y);
    let height_stride = ray.y * stride / sky.layer.y;
    let light_stride = sky.sun.xyz * (light_step * scale);
    let light_height = sky.sun.y * 0.12;
    let threshold = 0.83 - 0.55 * sky.layer.w;
    var added = vec3<f32>(0.0);
    var transmission = 1.0;
    for (var i = 0u; i < u32(steps); i += 1u) {
        let lattice = lattice_start + f32(i) * lattice_stride;
        let h = height_start + f32(i) * height_stride;
        let weather_uv = (lattice.xz - lattice_bias.xz) * 0.62;
        let weather = field(vec3<f32>(weather_uv.x + 0.13, 0.41, weather_uv.y)).b;
        let top = mix(0.48, 1.08, smoothstep(0.24, 0.77, weather));
        let density = view_density(lattice, h, weather, top, detail, threshold);
        if (density <= 0.0001) { continue; }
        let optical_depth = light_step * (
            light_density(lattice + light_stride * 0.65, h + light_height * 0.65, weather, top, threshold)
            + 2.3 * light_density(lattice + light_stride * 2.3, h + light_height * 2.3, weather, top, threshold));
        let direct = exp(-optical_depth) * (0.21 + forward_scatter);
        let height = smoothstep(0.10, 0.80, h);
        let diffuse = (0.13 + 0.23 * height) * exp(-0.22 * optical_depth);
        let ambient = sky_fill * (0.40 + 0.66 * height) + vec3<f32>(0.09);
        let radiance = sky.cloud_color.rgb * (ambient + sunlight * (direct + diffuse));
        let through_step = exp(-density * stride);
        added += transmission * (1.0 - through_step) * radiance;
        transmission *= through_step;
        if (transmission < 0.015) { break; }
    }
    return vec4<f32>(added, 1.0 - transmission) * (1.0 - distance_haze);
}

const PI: f32 = 3.14159265359;

#ifdef CLOUD_CACHE_GENERATE
@compute @workgroup_size(8, 8)
fn bake_clouds(@builtin(global_invocation_id) id: vec3<u32>) {
    let point = vec2<u32>(id.x, id.y + bake.state.x);
    let size = textureDimensions(output_image);
    if (any(point >= size) || id.y >= bake.state.y) { return; }
    let uv = (vec2<f32>(point) + 0.5) / vec2<f32>(size);
    let azimuth = (uv.x - 0.5) * (2.0 * PI);
    let elevation = (uv.y - 0.5) * PI;
    let ray = vec3<f32>(cos(elevation) * cos(azimuth), sin(elevation), cos(elevation) * sin(azimuth));
    let volume = integrate_volume(ray, bake.origin_time.xyz, bake.drift.xy,
        ray_offset(vec2<f32>(point)));
    textureStore(output_image, vec2<i32>(point), volume);
}
#else
fn cached_sample(slot: u32, anchor: vec4<f32>, ray: vec3<f32>, origin: vec3<f32>, drift: vec2<f32>) -> vec4<f32> {
    let center = sky.layer.x + sky.layer.y * 0.5;
    var relative = ray;
    if (abs(ray.y) > 0.002) {
        let distance = (center - origin.y) / ray.y;
        if (distance > 0.0 && distance < 2200.0 && (center - anchor.y) * ray.y > 0.0) {
            let wind = vec3<f32>(drift.x, 0.0, drift.y);
            let from_anchor = origin + ray * distance - wind - anchor.xyz;
            if (dot(from_anchor, from_anchor) > 0.001) { relative = normalize(from_anchor); }
        }
    }
    let uv = vec2<f32>(
        atan2(relative.z, relative.x) / (2.0 * PI) + 0.5,
        asin(clamp(relative.y, -1.0, 1.0)) / PI + 0.5);
    if (slot == 0u) { return textureSampleLevel(cloud_a, cloud_sampler, uv, 0.0); }
    return textureSampleLevel(cloud_b, cloud_sampler, uv, 0.0);
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
    let color = mix(sky.horizon.xyz, sky.zenith.xyz, pow(max(ray.y, 0.0), 0.48))
        + sunlight * (0.10 * pow(max(sun_dot, 0.0), 14.0)
        + 1.5 * smoothstep(0.999983, 0.999993, sun_dot));
    var volume = vec4<f32>(0.0);
    if (sky.layer.z > 0.0 && sky.layer.w > 0.0) {
        if (cache.state.x != 0u) {
            volume = cache_volume(ray, view.world_position);
        } else {
            volume = integrate_volume(ray, view.world_position, cache.drift.xy, ray_offset(input.position.xy));
        }
    }
    return vec4<f32>(color * (1.0 - volume.a) + volume.rgb, 1.0);
}
#endif
