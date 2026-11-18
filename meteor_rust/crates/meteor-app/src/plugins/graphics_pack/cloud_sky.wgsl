#import bevy_pbr::mesh_view_bindings::view

struct SkyUniform {
    horizon: vec4<f32>,
    zenith: vec4<f32>,
    cloud_color: vec4<f32>,
    layer: vec4<f32>,
    motion: vec4<f32>,
    sun: vec4<f32>,
    sun_color: vec4<f32>,
}

struct SkyVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) direction: vec3<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyUniform;

@vertex
fn vertex(@location(0) position: vec3<f32>) -> SkyVertex {
    var out: SkyVertex;
    let clip = view.clip_from_world * vec4<f32>(view.world_position + position, 1.0);
    out.position = vec4<f32>(clip.xy, 0.0, clip.w);
    out.direction = position;
    return out;
}

fn hash_lattice(position: vec3<i32>) -> f32 {
    let cell = bitcast<vec3<u32>>(position);
    var seed = cell.x * 1597334677u ^ cell.y * 3812015801u ^ cell.z * 2798796415u;
    seed ^= seed >> 16u;
    seed *= 2246822519u;
    seed ^= seed >> 13u;
    return f32(seed & 0x00ffffffu) / 16777216.0;
}

fn noise(position: vec3<f32>) -> f32 {
    let i = vec3<i32>(floor(position));
    let f = fract(position);
    let s = f * f * (3.0 - 2.0 * f);
    let a = mix(hash_lattice(i), hash_lattice(i + vec3<i32>(1, 0, 0)), s.x);
    let b = mix(hash_lattice(i + vec3<i32>(0, 1, 0)), hash_lattice(i + vec3<i32>(1, 1, 0)), s.x);
    let c = mix(hash_lattice(i + vec3<i32>(0, 0, 1)), hash_lattice(i + vec3<i32>(1, 0, 1)), s.x);
    let d = mix(hash_lattice(i + vec3<i32>(0, 1, 1)), hash_lattice(i + vec3<i32>(1, 1, 1)), s.x);
    return mix(mix(a, b, s.y), mix(c, d, s.y), s.z);
}

fn cloud_density(world: vec3<f32>) -> f32 {
    let height = (world.y - sky.layer.x) / sky.layer.y;
    if (height <= 0.0 || height >= 1.0) {
        return 0.0;
    }
    let p = (world - vec3<f32>(sky.motion.x, 0.0, sky.motion.y) * sky.motion.w) * sky.motion.z;
    let structure = 0.62 * noise(p + vec3<f32>(23.7, 5.2, 19.1))
        + 0.27 * noise(p * 2.09) + 0.11 * noise(p * 4.32);
    let threshold = 1.0 - sky.layer.w;
    let shape = smoothstep(0.0, 0.24, height) * (1.0 - smoothstep(0.54, 1.0, height));
    return smoothstep(threshold, threshold + 0.24, structure) * shape * sky.layer.z;
}

fn phase(cosine: f32, g: f32) -> f32 {
    let base = 1.0 + g * g - 2.0 * g * cosine;
    return (1.0 - g * g) / (base * sqrt(base));
}

@fragment
fn fragment(input: SkyVertex) -> @location(0) vec4<f32> {
    let ray = normalize(input.direction);
    let sun_dot = clamp(dot(ray, sky.sun.xyz), -1.0, 1.0);
    var color = mix(sky.horizon.xyz, sky.zenith.xyz, pow(max(ray.y, 0.0), 0.48));
    let sunlight = sky.sun_color.xyz * min(sky.sun_color.w, 8.0) * max(sky.sun.y, 0.0);
    color += sunlight * (0.10 * pow(max(sun_dot, 0.0), 24.0)
        + 2.0 * smoothstep(0.9996, 0.9999, sun_dot));
    if (abs(ray.y) < 0.002 || sky.layer.z == 0.0 || sky.layer.w == 0.0) {
        return vec4<f32>(color, 1.0);
    }
    let origin = view.world_position;
    let a = (sky.layer.x - origin.y) / ray.y;
    let b = (sky.layer.x + sky.layer.y - origin.y) / ray.y;
    let near = max(min(a, b), 0.0);
    let far = min(max(a, b), 2200.0);
    if (far <= near) {
        return vec4<f32>(color, 1.0);
    }
    let steps = u32(sky.sun.w);
    let stride = (far - near) / f32(steps);
    let light_step = sky.layer.y * 0.19;
    let forward_scatter = 0.21 * phase(sun_dot, 0.55) + 0.09 * phase(sun_dot, -0.23);
    var added = vec3<f32>(0.0);
    var transmission = 1.0;
    for (var i = 0u; i < steps; i += 1u) {
        let point = origin + ray * (near + (f32(i) + 0.5) * stride);
        let extinction = cloud_density(point);
        if (extinction <= 0.0001) {
            continue;
        }
        let shadow = exp(-light_step * (
            cloud_density(point + sky.sun.xyz * light_step)
            + cloud_density(point + sky.sun.xyz * (light_step * 2.7)) * 1.7));
        let radiance = sky.cloud_color.xyz * (mix(sky.horizon.xyz, vec3<f32>(1.0), 0.35) * 0.68
            + sunlight * (0.18 + shadow * forward_scatter));
        let through_step = exp(-extinction * stride);
        added += transmission * (1.0 - through_step) * radiance;
        transmission *= through_step;
        if (transmission < 0.015) {
            break;
        }
    }
    let horizon_fade = 1.0 - smoothstep(800.0, 2200.0, near);
    return vec4<f32>(mix(color, color * transmission + added, horizon_fade), 1.0);
}
