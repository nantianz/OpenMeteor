#import bevy_pbr::{mesh_functions, mesh_view_bindings::{globals, view}}

struct GrassUniform {
    root_color: vec4<f32>,
    tip_color: vec4<f32>,
    blade: vec4<f32>,
    distances: vec4<f32>,
    contact: vec4<f32>,
    wind: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> field: GrassUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var roots: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var bends: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var bend_sampler: sampler;

@vertex
fn vertex(
    @location(0) blade: vec3<f32>,
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
    let seed = source.w;
    let distance_to_camera = distance(root, view.world_position);
    var fade_at = field.distances.z;
    if (blade.x >= 72.0) {
        fade_at = field.distances.x;
    } else if (blade.x >= 24.0) {
        fade_at = field.distances.y;
    }
    let fade = 1.0 - smoothstep(fade_at - 3.0, fade_at, distance_to_camera);
    if (fade <= 0.0) {
        return out;
    }

    let angle = fract(seed * 23.713) * 6.2831853;
    let facing = vec2<f32>(cos(angle), sin(angle));
    let side = vec2<f32>(-facing.y, facing.x);
    let t = blade.y;
    let height = mix(field.blade.x, field.blade.y, fract(seed * 13.79)) * fade;
    let width = mix(field.blade.z, field.blade.w, fract(seed * 47.221)) * fade;
    let swell = dot(root.xz, vec2<f32>(0.26, 0.17)) - globals.time * (2.0 + field.wind.w);
    let wind = field.wind.xy * field.wind.z * (0.65 + 0.35 * sin(swell + seed));
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
    let curved = (facing * (0.14 + 0.20 * seed) + wind + bend * 1.13) * (t * t * height);
    let lateral = side * (blade.z * width * 0.5 * (1.0 - t * t));
    let y = height * t * (1.0 - pressure * 0.76 * t) + width * 0.07 * t * blade.z;
    let displaced = root + vec3<f32>(curved.x + lateral.x, y, curved.y + lateral.y);
    out.position = view.clip_from_world * vec4<f32>(displaced, 1.0);
    let light = 0.79 + 0.18 * seed + 0.13 * abs(dot(facing, vec2<f32>(0.78, 0.63)));
    out.color = mix(field.root_color.rgb, field.tip_color.rgb, t * (2.0 - t)) * light;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
