#import bevy_pbr::{mesh_functions, mesh_view_bindings::{globals, view}}

struct GrassUniform {
    root_color: vec4<f32>,
    tip_color: vec4<f32>,
    blade: vec4<f32>,
    distances: vec4<f32>,
    contact: vec4<f32>,
    wind: vec4<f32>,
    wind_environment: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> field: GrassUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var roots: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var bends: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var bend_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> wind_clock: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<uniform> grass_light: vec4<f32>;

@vertex
fn vertex(
    @location(0) blade: vec3<f32>,
    @location(1) leaf: vec4<f32>,
    @builtin(instance_index) instance: u32,
) -> VertexOutput {
    let source = textureLoad(roots, vec2<i32>(i32(blade.x), i32(mesh_functions::get_tag(instance))), 0);
    var out: VertexOutput;
    out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    out.color = vec3<f32>(0.0);
    if (source.w == 0.0) {
        return out;
    }
    let root = source.xyz;
    let pattern = u32(source.w) - 1u;
    let seed = (f32(pattern & 255u) + 0.5) / 256.0;
    let species = (pattern >> 8u) & 3u;
    let slope = vec2<f32>(f32((pattern >> 10u) & 127u), f32((pattern >> 17u) & 127u))
        * (3.04 / 127.0) - 1.52;
    let distance_to_camera = distance(root, view.world_position);
    let distant = leaf.w < 0.0;
    var fade_at = field.distances.w;
    if (blade.x >= 144.0 || leaf.z >= 4.0) {
        fade_at = field.distances.x;
    } else if (blade.x >= 96.0 || leaf.z >= 2.0) {
        fade_at = field.distances.y;
    } else if (blade.x >= 80.0 || leaf.z >= 1.0) {
        fade_at = field.distances.z;
    }
    let fade = 1.0 - smoothstep(fade_at - max(fade_at * 0.075, 2.0), fade_at, distance_to_camera);
    if (fade <= 0.0) {
        return out;
    }

    let angle = seed * 6.2831853;
    let base = vec2<f32>(cos(angle), sin(angle));
    let facing = vec2<f32>(base.x * leaf.x - base.y * leaf.y, base.y * leaf.x + base.x * leaf.y);
    let side = vec2<f32>(-facing.y, facing.x);
    let t = blade.y;
    let individual = fract(seed * 13.79 + leaf.z * 0.618034);
    let growth = 0.76 + 0.24 * sin(dot(root.xz, vec2<f32>(0.44, -0.27)));
    var height = mix(field.blade.x, field.blade.y, individual)
        * (0.72 + growth * 0.45) * fade;
    var width = mix(field.blade.z, field.blade.w, fract(seed * 7.17 + leaf.z * 0.377)) * fade;
    var lean = 0.18 + 0.36 * individual;
    var taper = (0.90 + 0.10 * t) * (1.0 - t * t);
    var rise = t;
    let drying = smoothstep(0.55, 0.96, seed);
    var tip = field.tip_color.rgb * mix(vec3<f32>(0.83, 0.97, 1.04), vec3<f32>(1.37, 1.00, 0.68), drying);
    var bottom = field.root_color.rgb * (0.80 + 0.26 * seed);
    let plume = species == 1u && leaf.z == 0.0;
    if (distant) {
        let across = normalize(view.world_position.xz - root.xz + vec2<f32>(0.0001, 0.0));
        let compensate = mix(1.9, 3.0, smoothstep(field.distances.y, field.distances.w, distance_to_camera));
        let lateral = vec2<f32>(-across.y, across.x) * blade.z * width * compensate;
        let offset = lateral + facing * (height * t * lean);
        let silhouette = root + vec3<f32>(offset.x, height * t + dot(slope, lateral), offset.y);
        out.position = view.clip_from_world * vec4<f32>(silhouette, 1.0);
        out.color = mix(bottom, tip, 0.44 + t * 0.56) * grass_light.rgb;
        return out;
    }
    if (plume) {
        height *= 1.34;
        lean *= 0.72;
        let head = smoothstep(0.62, 0.83, t) * (1.0 - smoothstep(0.86, 1.0, t));
        taper = 0.13 * (1.0 - t) + 0.63 * head;
        tip = mix(tip, mix(vec3<f32>(0.23, 0.16, 0.089), vec3<f32>(0.44, 0.35, 0.21), seed),
            smoothstep(0.65, 0.84, t));
    } else if (species == 2u) {
        height *= 0.58;
        width *= 1.9;
        lean *= 1.7;
        rise = t * (1.44 - 0.71 * t);
        taper = mix(0.45, 1.15, sin(t * 3.14159265)) * (1.0 - t);
        tip *= vec3<f32>(0.72, 0.91, 0.85);
        bottom *= vec3<f32>(0.79, 1.02, 0.99);
    }
    var wind = vec2<f32>(0.0);
    if (t > 0.0) {
        if (field.wind_environment.y > 0.5) {
            let phase = (wind_clock.x + wind_clock.y) * field.wind.w
                - dot(root.xz, field.wind_environment.zw) * field.wind_environment.x;
            let gust = 0.7 * sin(phase) + 0.3 * sin(phase * 1.91);
            wind = field.wind.xy * (1.0 + field.wind.z * gust);
            wind /= max(1.0, length(wind) / 0.55);
        } else {
            let swell = dot(root.xz, vec2<f32>(0.26, 0.17)) - globals.time * (2.0 + field.wind.w);
            wind = field.wind.xy * field.wind.z * (0.53 + 0.47 * sin(swell + seed * 0.4));
        }
    }
    var bend = vec2<f32>(0.0);
    var pressure = 0.0;
    if (t > 0.0 && max(abs(root.x - view.world_position.x), abs(root.z - view.world_position.z)) < field.contact.w) {
        let sample_pos = root.xz / (field.contact.z * 64.0);
        let sample = textureSampleLevel(bends, bend_sampler, sample_pos, 0.0);
        let feet = field.contact.x + clamp(sample.a / max(sample.b, 0.005), 0.0, 1.0) * field.contact.y;
        let floor_match = 1.0 - smoothstep(0.35, 0.85, abs(feet - root.y));
        bend = (sample.rg * 255.0 - 128.0) / 127.0 * floor_match;
        pressure = sample.b * floor_match;
    }
    let clump = facing * (field.root_color.w * leaf.w);
    let curved = (facing * lean + wind + bend * 1.13) * (t * t * height);
    let lateral = side * (blade.z * width * 0.5 * taper);
    let y = height * rise * (1.0 - pressure * 0.76 * t) + dot(slope, clump + lateral) + width * 0.08 * t * blade.z;
    let offset = clump + curved + lateral;
    let displaced = root + vec3<f32>(offset.x, y, offset.y);
    out.position = view.clip_from_world * vec4<f32>(displaced, 1.0);
    let light = 0.61 + 0.25 * growth + 0.13 * abs(dot(facing, vec2<f32>(0.78, 0.63)));
    out.color = mix(bottom, tip, t * (1.34 - 0.34 * t)) * light * grass_light.rgb;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
