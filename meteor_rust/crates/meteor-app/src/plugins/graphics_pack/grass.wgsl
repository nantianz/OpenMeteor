#import bevy_pbr::{mesh_functions, mesh_view_bindings::{globals, view, lights}, shadows::fetch_directional_shadow}

struct GrassUniform {
    root_color: vec4<f32>,
    tip_color: vec4<f32>,
    blade: vec4<f32>,
    distances: vec4<f32>,
    contact: vec4<f32>,
    wind: vec4<f32>,
    wind_environment: vec4<f32>,
    meadow: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) ribbon: vec4<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) canopy: vec3<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> field: GrassUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var roots: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var bends: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var bend_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> wind_clock: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<uniform> grass_light: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var<uniform> grass_light_direction: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var<uniform> scene_lighting: vec4<f32>;

fn coarse_segment(t: f32, leaf_id: u32, tier: u32) -> vec2<f32> {
    if (t <= 0.0 || t >= 1.0) {
        return vec2<f32>(t);
    }
    if (leaf_id == 0u) {
        if (tier == 0u && t < 0.67) { return vec2<f32>(0.0, 0.67); }
        if (tier == 1u && t > 0.67) { return vec2<f32>(0.67, 1.0); }
        if (tier == 2u) { return vec2<f32>(0.0, 1.0); }
    } else if ((tier == 0u && leaf_id >= 2u) || (tier == 1u && leaf_id == 1u)) {
        return vec2<f32>(0.0, 1.0);
    }
    return vec2<f32>(t);
}
fn ribbon_shape(t: f32, species: u32, plume: bool, painted: bool, pressure: f32) -> vec4<f32> {
    var taper = (0.90 + 0.10 * t) * (1.0 - t * t);
    var rise = t;
    if (painted) {
        taper = 1.0 - t;
    } else if (plume) {
        let head = smoothstep(0.62, 0.83, t) * (1.0 - smoothstep(0.86, 1.0, t));
        taper = 0.13 * (1.0 - t) + 0.63 * head;
    } else if (species == 2u) {
        rise = t * (1.44 - 0.71 * t);
        taper = mix(0.45, 1.15, sin(t * 3.14159265)) * (1.0 - t);
    }
    return vec4<f32>(taper, t * t, rise * (1.0 - pressure * 0.94 * t),
        select(t, 0.0, painted || t >= 1.0));
}
@vertex
fn vertex(
    @location(0) blade: vec3<f32>,
    @location(1) leaf: vec4<f32>,
    @builtin(instance_index) instance: u32,
) -> VertexOutput {
    let tag = mesh_functions::get_tag(instance);
    let lod = f32(tag >> 16u) / 256.0;
    let source = textureLoad(roots, vec2<i32>(i32(blade.x), i32(tag & 65535u)), 0);
    var out: VertexOutput;
    out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    out.color = vec3<f32>(0.0);
    out.ribbon = vec4<f32>(0.0);
    out.world_position = vec3<f32>(0.0);
    out.canopy = vec3<f32>(0.0);
    if (source.w == 0.0) {
        return out;
    }
    let root = source.xyz;
    let pattern = u32(source.w) - 1u;
    let seed = (f32(pattern & 255u) + 0.5) / 256.0;
    let species = (pattern >> 8u) & 3u;
    let slope = (vec2<f32>(f32((pattern >> 10u) & 31u), f32((pattern >> 15u) & 31u)) - 16.0)
        * (1.52 / 15.0);
    let edge = f32((pattern >> 20u) & 15u) / 15.0;
    let distance_to_camera = distance(root, view.world_position);
    var last_lod = 3u;
    if (blade.x >= 144.0 || leaf.z >= 4.0) {
        last_lod = 0u;
    } else if (blade.x >= 96.0 || leaf.z >= 2.0) {
        last_lod = 1u;
    } else if (leaf.z >= 1.0) {
        last_lod = 2u;
    }
    let fade_at = field.distances[last_lod];
    let fade_span = max(fade_at * 0.25, 3.0);
    let spatial = 1.0 - smoothstep(fade_at - fade_span, fade_at, distance_to_camera);
    let morph = 1.0 - smoothstep(f32(last_lod), f32(last_lod + 1u), lod);
    let fade = spatial * morph;
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
    let patch_height = sin(dot(root.xz, vec2<f32>(0.20, 0.112))
        + 0.8 * sin(dot(root.xz, vec2<f32>(-0.09, 0.24))));
    var height = mix(field.blade.x, field.blade.y, individual)
        * (0.72 + growth * 0.45) * (1.0 + patch_height * 0.26 * field.meadow.w)
        * fade * mix(0.25, 1.0, edge);
    var width = mix(field.blade.z, field.blade.w, fract(seed * 7.17 + leaf.z * 0.377))
        * fade * mix(0.56, 1.0, edge);

    width *= mix(1.0, 1.85, smoothstep(field.distances.y, field.distances.z, distance_to_camera));
    var lean = 0.18 + 0.36 * individual;
    let drying = smoothstep(0.55, 0.96, seed);
    var tip = field.tip_color.rgb * mix(vec3<f32>(0.83, 0.97, 1.04), vec3<f32>(1.37, 1.00, 0.68), drying);
    var bottom = field.root_color.rgb * (0.80 + 0.26 * seed);
    let plume = species == 1u && leaf.z == 0.0;
    let painted_plume = plume && field.tip_color.w > 0.5;
    if (plume) {
        height *= 1.34;
        lean *= 0.72;
        if (painted_plume) {

            width *= 3.25;
        } else {
            tip = mix(tip, mix(vec3<f32>(0.23, 0.16, 0.089), vec3<f32>(0.44, 0.35, 0.21), seed),
                smoothstep(0.65, 0.84, t));
        }
    } else if (species == 2u) {
        height *= 0.58;
        width *= 1.9;
        lean *= 1.7;
        tip *= vec3<f32>(0.72, 0.91, 0.85);
        bottom *= vec3<f32>(0.79, 1.02, 0.99);
    }
    var wind = vec2<f32>(0.0);
    if (t > 0.0) {
        let sway_damp = 1.0 - 0.65 * smoothstep(field.distances.x, field.distances.z, distance_to_camera);
        if (field.wind_environment.y > 0.5) {
            let phase = (wind_clock.x + wind_clock.y) * field.wind.w
                - dot(root.xz, field.wind_environment.zw) * field.wind_environment.x;
            let gust = 0.7 * sin(phase) + 0.3 * sin(phase * 1.91);
            let flexibility = 1.0 + field.meadow.w * (0.24 * patch_height + 0.16 * (2.0 * seed - 1.0));
            wind = field.wind.xy * ((1.0 + field.wind.z * gust * sway_damp) * flexibility);
            wind /= max(1.0, length(wind) / 0.55);
        } else {
            let swell = dot(root.xz, vec2<f32>(0.26, 0.17)) - globals.time * (2.0 + field.wind.w);
            wind = field.wind.xy * field.wind.z * (0.53 + 0.47 * sin(swell + seed * 0.4) * sway_damp);
        }
    }
    let directional = 0.13 * abs(dot(facing, grass_light_direction.xy));
    let light = 0.61 + 0.25 * growth + mix(0.065, directional, grass_light_direction.z);
    out.color = mix(bottom, tip, t * (1.34 - 0.34 * t)) * light * grass_light.rgb;
    out.canopy = mix(field.root_color.rgb, field.tip_color.rgb, 0.66) * light * grass_light.rgb;
    if (painted_plume) {
        out.color = light * grass_light.rgb * mix(0.82, 1.22, seed);
    }
    var bend = vec2<f32>(0.0);
    var pressure = 0.0;
    if (t > 0.0 && max(abs(root.x - view.world_position.x), abs(root.z - view.world_position.z)) < field.contact.w) {
        let sample_pos = root.xz / (field.contact.z * 64.0);
        let sample = textureSampleLevel(bends, bend_sampler, sample_pos, 0.0);
        let feet = field.contact.x + clamp(sample.a / max(sample.b, 0.005), 0.0, 1.0) * field.contact.y;
        let floor_match = 1.0 - smoothstep(0.35, 0.85, abs(feet - root.y));

        bend = (sample.rg * 255.0 - 128.0) / 127.0 * inverseSqrt(max(sample.b, 0.04)) * floor_match;
        pressure = sample.b * floor_match;
    }
    let clump = facing * (field.root_color.w * leaf.w * mix(0.45, 1.0, edge));

    var shape = ribbon_shape(t, species, plume, painted_plume, pressure);
    let lod_phase = fract(lod);
    if (lod_phase > 0.0) {
        let segment = coarse_segment(t, u32(leaf.z), u32(lod));
        if (segment.x < segment.y) {
            let a = ribbon_shape(segment.x, species, plume, painted_plume, pressure);
            let b = ribbon_shape(segment.y, species, plume, painted_plume, pressure);
            let coarse = mix(a, b, (t - segment.x) / (segment.y - segment.x));
            shape = mix(shape, coarse, smoothstep(0.0, 1.0, lod_phase));
        }
    }
    let curved = (facing * lean + wind * (1.0 - pressure) + bend * 1.65) * (shape.y * height);
    var raster_half_width = shape.x;
#ifdef MULTISAMPLED
    if (!painted_plume) {
        let to_eye = view.world_position - root;
        let orthographic = view.clip_from_view[3].w == 1.0;
        let depth = select(max(dot(view.world_from_view[2].xyz, to_eye), 0.1), 1.0, orthographic);
        let pixel = 2.0 * depth / max(abs(view.clip_from_view[1].y) * view.viewport.w, 1.0);
        let side_dot = select(dot(side, to_eye.xz) / max(distance_to_camera, 0.1),
            dot(side, view.world_from_view[2].xz), orthographic);
        let projected = sqrt(max(1.0 - side_dot * side_dot, 0.09));
        let pad = min(0.85 * pixel / max(width * projected, 0.001), 0.75);
        raster_half_width += pad * (1.0 - t);
    }
#endif
    let ribbon_x = blade.z * raster_half_width;
    out.ribbon = vec4<f32>(ribbon_x, shape.x, t, select(0.0, 1.0, painted_plume));
    let lateral = side * (ribbon_x * width * 0.5);
    let side_rise = width * 0.08 * shape.w * blade.z;
    let y = height * shape.z + dot(slope, clump + lateral) + side_rise;
    let offset = clump + curved + lateral;
    let displaced = root + vec3<f32>(offset.x, y, offset.y);
    out.world_position = displaced;
    out.position = view.clip_from_world * vec4<f32>(displaced, 1.0);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let height_step = fwidth(in.ribbon.z);
    let width_step = max(fwidth(in.ribbon.x), 0.001);
    var half_width = in.ribbon.y;
    var color = in.color;
    if (in.ribbon.w > 0.5) {
        let t = in.ribbon.z;
        let head = smoothstep(0.56, 0.70, t) * (1.0 - smoothstep(0.78, 0.99, t));
        let feather = mix(1.0, 0.86 + cos(t * 93.0) * 0.14,
            1.0 - smoothstep(0.004, 0.025, height_step));
        half_width = max(0.047 * (1.0 - t), min(0.30 * head * feather, (1.0 - t) * 0.94));
        let shaft = mix(field.root_color.rgb, field.tip_color.rgb, t);
        let bristle = field.meadow.rgb * mix(0.74, 1.15, smoothstep(0.62, 0.94, t));
        color *= mix(shaft, bristle, smoothstep(0.54, 0.74, t));
    }
#ifdef MULTISAMPLED
    let high = min(in.ribbon.x + 0.5 * width_step, half_width);
    let low = max(in.ribbon.x - 0.5 * width_step, -half_width);
    let raw_coverage = max(high - low, 0.0) / width_step;
    let coverage = smoothstep(0.08, 0.92, raw_coverage);
    if (coverage <= 0.0) { discard; }
#else
    let coverage = 1.0;
    if (abs(in.ribbon.x) > half_width) { discard; }
#endif
    var shadow = 1.0;
    if (scene_lighting.w > 0.0 && lights.n_directional_lights > 0u) {
        let dist = distance(in.world_position, view.world_position);
        let shadow_fade = 1.0 - smoothstep(field.distances.y * 0.85, field.distances.z, dist);
        if (shadow_fade > 0.0) {
        let world = vec4<f32>(in.world_position, 1.0);
        let visibility = fetch_directional_shadow(0u, world, vec3<f32>(0.0, 1.0, 0.0),
            (view.view_from_world * world).z, in.position.xy);
            shadow = mix(1.0, visibility, scene_lighting.w * 0.7 * shadow_fade);
    }
    }
    let resolved = smoothstep(0.55, 2.4, 2.0 * half_width / width_step);
    color = mix(in.canopy, color, mix(0.10, 1.0, resolved));
    return vec4<f32>(color * shadow, coverage);
}
