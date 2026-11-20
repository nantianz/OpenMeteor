#import bevy_pbr::forward_io::VertexOutput;
#import bevy_pbr::mesh_view_bindings::{view, lights, view_transmission_texture, view_transmission_sampler};
#ifdef DEPTH_PREPASS
#import bevy_pbr::mesh_view_bindings::depth_prepass_texture;
#endif
#import bevy_pbr::mesh_bindings::mesh;
#import bevy_pbr::mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT;
#import bevy_pbr::shadows::fetch_directional_shadow;

#ifdef WATER_SKY_REFLECTION
#import meteor::cloud_panorama::{CloudCacheView, panorama_uv};
@group(#{MATERIAL_BIND_GROUP}) @binding(19) var water_clouds: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(20) var water_cloud_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(21) var<uniform> water_cloud_view: CloudCacheView;

fn water_cloud_sample(slot: u32, anchor: vec3<f32>, ray: vec3<f32>, origin: vec3<f32>, drift: vec2<f32>) -> vec4<f32> {
    let uv = panorama_uv(anchor, ray, origin, drift, water_cloud_view.layer.xy);
    if (uv.z == 0.0) { return vec4<f32>(0.0); }
    return textureSampleLevel(water_clouds, water_cloud_sampler, uv.xy, i32(slot), 0.0);
}

fn reflected_sky(origin: vec3<f32>, ray: vec3<f32>) -> vec3<f32> {
    let cache = water_cloud_view;
    let slot = cache.state.y;
    var cloud = water_cloud_sample(slot, cache.origin_time.xyz, ray, origin, cache.drift.xy - cache.drift.zw);
    if (cache.state.w != 0u) {
        let old = water_cloud_sample(1u - slot, cache.previous_origin_time.xyz, ray, origin,
            cache.drift.xy - cache.previous_drift.xy);
        cloud = mix(old, cloud, f32(cache.state.z) / 255.0);
    }
    let clear = mix(cache.horizon.rgb, cache.zenith.rgb, pow(max(ray.y, 0.0), 0.48));
    return clear * (1.0 - cloud.a) + cloud.rgb;
}
#endif

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> material: GmdlGpuParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_color_sampler: sampler;

@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> occlusion: CameraOcclusionParams;

@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> simulation_time: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<uniform> lighting: vec4<f32>;

@group(#{MATERIAL_BIND_GROUP}) @binding(6) var normal_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var normal_sampler: sampler;

@group(#{MATERIAL_BIND_GROUP}) @binding(8) var water_reflection: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(9) var water_reflection_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(10) var water_depth: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(11) var water_depth_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(12) var water_normal: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(13) var water_normal_sampler: sampler;

@group(#{MATERIAL_BIND_GROUP}) @binding(15) var surface_detail: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(16) var surface_detail_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(18) var surface_aux: texture_2d<f32>;

#ifdef WATER_SKY_REFLECTION
fn water_sky_horizon(depth_uv: vec2<f32>, ray: vec3<f32>) -> f32 {
    let dimensions = textureDimensions(surface_aux);
    let size = f32(dimensions.y);
    let sectors = dimensions.x / dimensions.y * 4u;
    let tile_width = 4.0 / f32(sectors);
    let cell = clamp(depth_uv, vec2<f32>(0.5 / size), vec2<f32>(1.0 - 0.5 / size));
    let sector = fract(atan2(ray.z, ray.x) / 6.28318530718 + 1.0) * f32(sectors);
    let first = u32(sector);
    let next = (first + 1u) % sectors;
    let a = textureSampleLevel(surface_aux, water_depth_sampler, vec2<f32>((cell.x + f32(first / 4u)) * tile_width, cell.y), 0.0);
    var b = a;
    if (first % 4u == 3u) {
        b = textureSampleLevel(surface_aux, water_depth_sampler, vec2<f32>((cell.x + f32(next / 4u)) * tile_width, cell.y), 0.0);
    }
    return mix(a[first % 4u], b[next % 4u], fract(sector));
}
#endif

fn water_disturbance(world: vec3<f32>) -> vec3<f32> {
    let bounds = material.water_dynamics_bounds;
    if (bounds.z <= 0.0 || material.water.x != 1.0) { return vec3<f32>(0.0); }
    let uv = (world.xz - bounds.xy) / bounds.zw;
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) { return vec3<f32>(0.0); }
    let data = textureSampleLevel(surface_detail, surface_detail_sampler, uv, 0.0);
    return vec3<f32>((data.rg * 255.0 - vec2<f32>(128.0)) / 64.0, data.b);
}

struct LiquidContacts {
    positions: array<vec4<f32>, 8>,
    styles: array<vec4<f32>, 8>,
    counts: vec4<f32>,
    bounds: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(14) var<uniform> liquid_contacts: LiquidContacts;

struct GmdlGpuParams {
    base: GmdlMaterialParams,
    scroll: vec4<f32>,
    detail: vec4<f32>,
    body_glow: vec4<f32>,
    body_style: vec4<f32>,
    water: vec4<f32>,
    water_bounds: vec4<f32>,
    first_person_clip: vec4<f32>,
    water_dynamics_bounds: vec4<f32>,
    pbr: vec4<f32>, pbr_tint: vec4<f32>, pbr_textures: vec4<f32>,
    water_optics: WaterOptics,
    grass_ground: vec4<f32>,
    grass_bounds: vec4<f32>,
    pbr_lighting: vec3<f32>,
    grass_underlay_scale: f32,
    water_sky_style: vec4<f32>,
    water_sky_ranges: vec4<f32>,
};

struct WaterOptics {
    absorption_ior: vec4<f32>,
    scattering_g: vec4<f32>,
    control: vec4<f32>,
};

struct CameraOcclusionParams {
    eye: vec4<f32>,
    player: vec4<f32>,
    watched: vec4<f32>,
    self_eye: vec4<f32>,
};

fn visibility_tunnel(world: vec3<f32>, body: vec4<f32>) -> f32 {
    let axis = body.xyz - occlusion.eye.xyz;
    let axis_length = length(axis);
    if (axis_length < 0.01 || body.w <= 0.0) { return 0.0; }
    let direction = axis / axis_length;
    let depth = dot(world - occlusion.eye.xyz, direction);
    if (depth <= 0.0 || depth >= axis_length - 0.25) { return 0.0; }
    let radius = mix(0.25, body.w, clamp(depth / axis_length, 0.0, 1.0));
    let radial = length(world - occlusion.eye.xyz - direction * depth);
    return (1.0 - smoothstep(radius * 0.65, radius, radial))
        * smoothstep(0.25, 0.65, axis_length - depth);
}

fn camera_fade(world: vec3<f32>, pixel: vec2<f32>) -> bool {
    let amount = occlusion.eye.w * max(visibility_tunnel(world, occlusion.player), visibility_tunnel(world, occlusion.watched));

    let bayer = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0,
                              3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let xy = vec2<u32>(pixel) % vec2<u32>(4u);
    return amount > (bayer[xy.y * 4u + xy.x] + 0.5) / 16.0;
}

struct GmdlMaterialParams {
    opacity: f32,
    alpha_cutoff: f32,
    mode: u32,
    has_texture: u32,

    uv_offset: vec2<f32>,
};

fn water_enhance_active(mode: u32) -> bool {

    return simulation_time.w > 0.0 && material.water.x > 0.0 && material.water.x < 3.5
        && (mode == 1u || (mode == 4u && material.water.x > 1.5 && material.water.x < 2.5));
}

fn liquid_contact_slope(world: vec3<f32>) -> vec2<f32> {
    var slope = vec2<f32>(0.0);
    if (material.water.x == 1.0 && material.water_dynamics_bounds.z > 0.0) { return slope; }
    if (liquid_contacts.counts.x <= 0.0 || any(world.xz < liquid_contacts.bounds.xy)
        || any(world.xz > liquid_contacts.bounds.zw)) { return slope; }

    for (var i = 0u; i < min(u32(liquid_contacts.counts.x), 8u); i += 1u) {
        let contact = liquid_contacts.positions[i];
        let style = liquid_contacts.styles[i];
        let speed = select(select(2.4, 1.35, style.y > 1.5), 0.75, style.y > 2.5);
        let radius = 0.15 + contact.w * speed;
        let delta = world.xz - contact.xz;
        if (abs(style.y - material.water.x) < 0.1 && abs(world.y - contact.y) < select(0.4, 1.2, style.y > 1.5 && style.y < 2.5)
            && all(abs(delta) < vec2<f32>(radius + 0.65))) {
            let distance = length(delta);
            let band = distance - radius;
            if (abs(band) < 0.65 && distance > 0.001) {
                let fade = max(1.0 - contact.w / 1.8, 0.0);
                let wave = sin(band * 9.6664) * (1.0 - smoothstep(0.12, 0.65, abs(band)));
                slope += delta / distance * wave * fade * fade * style.x * 0.35;
            }
        }
    }
    return slope;
}

fn water_surface_normal(in: VertexOutput) -> vec3<f32> {
    let clear = material.water.x < 1.5;
    let shared_wind = clear && graphics.water_wind.w > 0.5;
    let uv = in.world_position.xz * select(0.09, 0.14, clear);
    let direction = material.scroll.xy / max(length(material.scroll.xy), 0.001);
    let speed = select(select(1.0, 0.55, material.water.x > 1.5), 0.25, material.water.x > 2.5);
    let velocity = (direction * 0.017 + vec2<f32>(0.003, 0.001)) * speed;

    let phase = select(fract(velocity * simulation_time.x) + fract(velocity * simulation_time.y),
        graphics.water_wind.xy, shared_wind);
    let cross_velocity = vec2<f32>(-0.011, 0.007) * speed;
    let cross_phase = select(fract(cross_velocity * simulation_time.x) + fract(cross_velocity * simulation_time.y),
        graphics.water_wind_cross.xy, shared_wind);
    let a = textureSample(water_normal, water_normal_sampler, uv + phase).xy * 2.0 - 1.0;
    let cross_uv = select(vec2<f32>(-uv.y, uv.x) * 1.73,
        vec2<f32>(uv.x * 0.54 - uv.y * 0.84, uv.x * 0.84 + uv.y * 0.54) * 1.47, clear);
    let b = textureSample(water_normal, water_normal_sampler,
        cross_uv + cross_phase + vec2<f32>(0.37, 0.19)).xy * 2.0 - 1.0;
    let cross_slope = select(vec2<f32>(b.y, -b.x),
        vec2<f32>(0.54 * b.x + 0.84 * b.y, -0.84 * b.x + 0.54 * b.y), clear);
    let footprint = max(length(dpdx(uv)), length(dpdy(uv)));
    let fade = 1.0 - smoothstep(0.025, 0.20, footprint);
    let slope = ((a * select(0.65, 0.72, clear)
        + cross_slope * select(0.35, 0.28, clear))
        * select(1.0, 0.75, clear) * select(1.0, graphics.water_wind.z, shared_wind)
        + liquid_contact_slope(in.world_position.xyz)) * fade;
    let dynamic = water_disturbance(in.world_position.xyz);
    return normalize(normalize(in.world_normal) + vec3<f32>(slope.x - dynamic.x, 0.0, slope.y - dynamic.y));
}

fn water_reflection_uv(direction: vec3<f32>) -> vec2<f32> {
    let d = direction;
    let a = abs(d);
    var face: u32;
    var q: vec2<f32>;
    if (a.x >= a.y && a.x >= a.z) {
        face = select(1u, 0u, d.x >= 0.0);
        q = vec2<f32>(select(d.z, -d.z, d.x >= 0.0), -d.y) / a.x;
    } else if (a.y >= a.z) {
        face = select(3u, 2u, d.y >= 0.0);
        q = vec2<f32>(d.x, select(-d.z, d.z, d.y >= 0.0)) / a.y;
    } else {
        face = select(5u, 4u, d.z >= 0.0);
        q = vec2<f32>(select(-d.x, d.x, d.z >= 0.0), -d.y) / a.z;
    }
    let atlas_size = vec2<f32>(textureDimensions(water_reflection));
    let face_size = atlas_size.x / 3.0 - 2.0;
    let tile_size = face_size + 2.0;
    return (vec2<f32>(f32(face % 3u), f32(face / 3u)) * tile_size
        + vec2<f32>(1.0) + (q * 0.5 + 0.5) * face_size) / atlas_size;
}

#ifdef DEPTH_PREPASS
struct ScreenRaySample {
    uv: vec2<f32>,
    ndc: vec3<f32>,
    depth: f32,
    valid: bool,
};

fn screen_ray_pixel(uv: vec2<f32>) -> vec2<i32> {
    return vec2<i32>(floor(view.viewport.xy + uv * view.viewport.zw));
}

fn opaque_depth_range(pixel: vec2<i32>) -> vec2<f32> {
    let first = textureLoad(depth_prepass_texture, pixel, 0);
    var depths = vec2<f32>(first);
#ifdef MULTISAMPLED
    for (var sample = 1u; sample < textureNumSamples(depth_prepass_texture); sample += 1u) {
        let depth = textureLoad(depth_prepass_texture, pixel, i32(sample));
        depths = vec2<f32>(min(depths.x, depth), max(depths.y, depth));
    }
#endif
    return depths;
}

fn sample_screen_ray(world: vec3<f32>, ray: vec3<f32>, distance_along: f32) -> ScreenRaySample {
    let clip = view.clip_from_world * vec4<f32>(world + ray * distance_along, 1.0);
    if (clip.w <= 0.0) { return ScreenRaySample(vec2<f32>(0.0), vec3<f32>(0.0), 0.0, false); }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (ndc.z <= 0.0 || ndc.z >= 1.0
        || any(uv <= vec2<f32>(0.01)) || any(uv >= vec2<f32>(0.99))) {
        return ScreenRaySample(uv, ndc, 0.0, false);
    }
    return ScreenRaySample(uv, ndc,
        textureLoad(depth_prepass_texture, screen_ray_pixel(uv), 0), true);
}

fn water_refraction_color(world: vec3<f32>, pixel: vec2<i32>, depths: vec2<f32>,
    confidence: f32) -> vec4<f32> {

    if (depths.x <= 0.0) { return vec4<f32>(0.0); }
    let center = (vec2<f32>(pixel) + 0.5 - view.viewport.xy) / view.viewport.zw;
    let ndc = vec2<f32>(center.x * 2.0 - 1.0, 1.0 - center.y * 2.0);
    let position = view.world_from_clip * vec4<f32>(ndc, depths.y, 1.0);
    if (abs(position.w) < 0.0001 || position.y / position.w >= world.y - 0.025) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(textureLoad(view_transmission_texture, pixel, 0).rgb, confidence);
}
#endif

fn water_screen_reflection(world: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>) -> vec4<f32> {
#ifdef DEPTH_PREPASS
    if ((u32(material.detail.y) & 1u) == 0u) { return vec4<f32>(0.0); }
    let ray = normalize(reflect(-view_dir, normal));
    if (ray.y <= 0.02) { return vec4<f32>(0.0); }
    var distance_along = 0.15;
    var previous_distance = 0.0;
    var previous_front = true;
    for (var step = 0u; step < 8u; step += 1u) {
        let sample = sample_screen_ray(world, ray, distance_along);
        if (!sample.valid) { break; }
        let behind = sample.depth > 0.0 && sample.ndc.z <= sample.depth;
        if (behind && previous_front) {
            var near = previous_distance;
            var far = distance_along;
            var hit = sample;

            for (var refine = 0u; refine < 4u; refine += 1u) {
                let middle = (near + far) * 0.5;
                let candidate = sample_screen_ray(world, ray, middle);
                if (candidate.valid && candidate.depth > 0.0
                    && candidate.ndc.z <= candidate.depth) {
                    far = middle;
                    hit = candidate;
                } else {
                    near = middle;
                }
            }
            let scene_h = view.world_from_clip * vec4<f32>(hit.ndc.xy, hit.depth, 1.0);
            if (abs(scene_h.w) < 0.0001) { break; }
            let scene = scene_h.xyz / scene_h.w;
            let delta = scene - world;
            let along = dot(delta, ray);
            let sideways = length(delta - ray * along);
            if (scene.y > world.y + 0.06 && along > 0.05
                && sideways < max(0.18, along * 0.045)) {
                let edge = max(abs(hit.uv.x * 2.0 - 1.0), abs(hit.uv.y * 2.0 - 1.0));
                let confidence = (1.0 - smoothstep(0.80, 0.98, edge))
                    * (1.0 - smoothstep(8.0, 20.0, along));
                let rgb = textureLoad(view_transmission_texture, screen_ray_pixel(hit.uv), 0).rgb;
                return vec4<f32>(rgb, confidence);
            }
        }
        previous_front = !behind;
        previous_distance = distance_along;
        distance_along *= 1.85;
    }
#endif
    return vec4<f32>(0.0);
}

fn water_screen_refraction(world: vec3<f32>, frag_coord: vec3<f32>,
    ray: vec3<f32>, path: f32) -> vec4<f32> {
#ifdef DEPTH_PREPASS
    if ((u32(material.detail.y) & 2u) == 0u || ray.y >= -0.02 || path <= 0.06) { return vec4<f32>(0.0); }
    let source_pixel = vec2<i32>(frag_coord.xy);
    let source_depths = opaque_depth_range(source_pixel);

    if (source_depths.y > frag_coord.z + 0.000001) { return vec4<f32>(0.0); }
    var near = 0.04;
    let start = sample_screen_ray(world, ray, near);
    if (start.valid && (start.depth <= 0.0 || start.ndc.z > start.depth)) {
        for (var step = 1u; step <= 8u; step += 1u) {
            let t = f32(step) / 8.0;
            var far = mix(0.04, path * 1.2, t * t);
            var hit = sample_screen_ray(world, ray, far);
            if (!hit.valid) { break; }
            if (hit.depth <= 0.0 || hit.ndc.z > hit.depth) {
                near = far;
                continue;
            }
            for (var i = 0u; i < 3u; i += 1u) {
                let middle = (near + far) * 0.5;
                let sample = sample_screen_ray(world, ray, middle);
                if (sample.valid && sample.depth > 0.0 && sample.ndc.z <= sample.depth) {
                    hit = sample;
                    far = middle;
                } else {
                    near = middle;
                }
            }
            let position = view.world_from_clip * vec4<f32>(hit.ndc.xy, hit.depth, 1.0);
            if (abs(position.w) < 0.0001) { break; }
            let delta = position.xyz / position.w - world;
            let along = dot(delta, ray);
            if (delta.y >= -0.025 || along <= 0.0
                || length(delta - ray * along) > max(0.12, 0.08 * along)) { break; }
            let edge = max(abs(hit.uv.x * 2.0 - 1.0), abs(hit.uv.y * 2.0 - 1.0));
            let pixel = screen_ray_pixel(hit.uv);
            let color = water_refraction_color(world, pixel, opaque_depth_range(pixel),
                1.0 - smoothstep(0.8, 0.98, edge));
            if (color.a > 0.0) { return color; }
            break;
        }
    }
    return water_refraction_color(world, source_pixel, source_depths, 1.0);
#endif
    return vec4<f32>(0.0);
}

fn water_fresnel(cosine: f32, ior: f32) -> f32 {
    let ratio = (ior - 1.0) / (ior + 1.0);
    let f0 = ratio * ratio;
    return f0 + (1.0 - f0) * pow(1.0 - clamp(cosine, 0.0, 1.0), 5.0);
}

fn enhanced_water(
    world: vec3<f32>, frag_coord: vec3<f32>, surface_normal: vec3<f32>, normal: vec3<f32>, view_dir: vec3<f32>,
    light_dir: vec3<f32>, reflection: vec3<f32>, reflection_valid: f32, sky_weight: f32,
    depth: f32, coverage: f32, shade: f32, foam: f32,
) -> vec4<f32> {
    let optics = material.water_optics;
    let ior = optics.absorption_ior.w;
    let ripple_transmission = optics.control.y * (1.0 - material.water_sky_style.y * sky_weight);
    let ray = refract(-view_dir, normalize(mix(surface_normal, normal, ripple_transmission)), 1.0 / ior);
    let path = min(depth / max(abs(dot(ray, surface_normal)), 0.25), optics.control.z);
    let extinction = max(optics.absorption_ior.rgb + optics.scattering_g.rgb, vec3<f32>(0.0001));
    let transmission = exp(-extinction * path);
    let albedo = optics.scattering_g.rgb / extinction;
    let f = water_fresnel(abs(dot(normal, view_dir)), ior);
    let n_dot_l = dot(normal, light_dir);
    let light_entry = 1.0 - water_fresnel(abs(n_dot_l), ior);
    let cos_theta = dot(ray, light_dir);
    let g = optics.scattering_g.w;
    let phase = (1.0 - g * g) / pow(max(1.0 + g * g - 2.0 * g * cos_theta, 0.08), 1.5);
    var sun = select(vec3<f32>(1.0), graphics.sun_color.rgb, graphics.sun.w > 0.001);
    var ambient = vec3<f32>(0.12);
    if (graphics.counts.w > 0.5) {
        sun = graphics.sun_color.rgb * graphics.sun.w;
        ambient = graphics.sky.rgb * 0.32;
    }
    let angular = min(phase, 4.0);
    let surface_light = max(dot(surface_normal, light_dir), 0.0);
    let light_cos = sqrt(1.0 - (1.0 - surface_light * surface_light) / (ior * ior));
    let path_ratio = 1.0 + max(-dot(ray, surface_normal), 0.0) / light_cos;

    let direct_integral = (1.0 - exp(-extinction * path * path_ratio)) / path_ratio;
    let transmitted_light = max(n_dot_l, 0.0) * direct_integral * (0.12 + angular * 0.1);
    let thin_path = min(path, 0.4);
    let shallow_scatter = (1.0 - exp(-extinction * thin_path))
        * (0.10 * (1.0 - abs(n_dot_l)) + 0.18 * max(-n_dot_l, 0.0) * exp(-path));
    let volume = albedo * ((1.0 - transmission) * ambient
        + sun * light_entry * (transmitted_light + shallow_scatter * angular) * shade);
    let refracted = water_screen_refraction(world, frag_coord, ray, path);
    let seen = refracted.a;
    var reflected = f * reflection_valid;
#ifdef WATER_SKY_REFLECTION

    let grazing = 1.0 - clamp(view_dir.y, 0.0, 1.0);
    let sky_fresnel = max(f, water_fresnel(1.0 - grazing, ior));
    let sheen = material.water_sky_style.z * smoothstep(material.water_sky_ranges.x, material.water_sky_ranges.y, grazing);
    reflected = mix(f, mix(sky_fresnel, 1.0, sheen), sky_weight) * reflection_valid;
#endif
    let absorbed = 1.0 - dot(transmission, vec3<f32>(0.2126, 0.7152, 0.0722));
    let layer = reflected + (1.0 - reflected) * mix(absorbed, 1.0, seen);
    let half_sum = light_dir + view_dir;
    let half_dir = half_sum / max(length(half_sum), 0.0001);
    let rough = optics.control.x;
    let exponent = clamp(2.0 / (rough * rough) - 2.0, 2.0, 256.0);
    let sparkle = pow(max(dot(normal, half_dir), 0.0), exponent) * max(n_dot_l, 0.0) * f;
    let premultiplied = reflection * reflected + (1.0 - reflected)
        * (volume + refracted.rgb * transmission * seen) + sun * sparkle * layer * shade;
    let color = premultiplied / max(layer, 0.001);
    let foam_amount = min(foam, 0.92);
    return vec4<f32>(mix(color, vec3<f32>(0.48, 0.65, 0.61) * shade, foam_amount),
        mix(layer, 0.92, foam_amount) * coverage);
}

fn shade_water(in: VertexOutput, normal: vec3<f32>, color: vec4<f32>, shade: f32) -> vec4<f32> {
    if (material.base.mode == 4u) {

        let slope = clamp(dot(normal.xz, vec2<f32>(0.7, 0.3)) * 2.4, -0.35, 0.35);
        let rolling = 0.94 + slope * 0.60;
        let crest = smoothstep(0.12, 0.35, slope);
        return vec4<f32>(color.rgb * rolling * min(shade, 1.15)
            + vec3<f32>(0.008, 0.035, 0.008) * crest, color.a);
    }
    if (material.water.x > 2.5) {

        let heat = clamp(color.r - color.b, 0.0, 1.0);
        let pulse = 0.96 + 0.04 * sin(simulation_time.y * 0.8 + in.uv.x * 7.0 + in.uv.y * 5.0);
        let glow = color.rgb * mix(0.85, 1.8, smoothstep(0.03, 0.55, heat))
            + vec3<f32>(0.12, 0.025, 0.001) * heat;
        let relief = 0.94 + 0.12 * max(dot(normal, normalize(vec3<f32>(0.4, 0.8, 0.3))), 0.0);
        return vec4<f32>(glow * pulse * relief * clamp(shade, 0.85, 1.15), color.a);
    }
    let poison = material.water.x > 1.5;
    let depth_uv = (in.world_position.xz - material.water_bounds.xy) / material.water_bounds.zw;
    let bed = textureSampleLevel(water_depth, water_depth_sampler, depth_uv, 0.0);
    let valid_bed = smoothstep(0.75, 0.98, bed.g * bed.b);
    let depth = bed.r * material.water.y;
    let view_dir = normalize(view.world_position.xyz - in.world_position.xyz);
    let surface_normal = normalize(in.world_normal);
    let authored_alpha = material.base.opacity * material.water.z;
    let coverage = clamp(color.a / max(authored_alpha, 0.001), 0.0, 1.0);
    let dynamic = water_disturbance(in.world_position.xyz);
    let physical_normal = normalize(surface_normal - vec3<f32>(dynamic.x, 0.0, dynamic.y));
    let reflection_normal = normalize(mix(physical_normal, normal,
        select(0.12, 1.0, poison)));
    let reflection_ray = reflect(-view_dir, reflection_normal);
    let reflected = textureSampleLevel(water_reflection, water_reflection_sampler,
        water_reflection_uv(reflection_ray), 0.0);
    var probe = reflected;
    var sky_weight = 0.0;
#ifdef WATER_SKY_REFLECTION
    if (water_cloud_view.state.x != 0u && view_dir.y > 0.0) {

        let sky_normal = normalize(vec3<f32>(0.0, 1.0, 0.0)
            + vec3<f32>(physical_normal.x, 0.0, physical_normal.z) * material.water_sky_style.x);
        let sky_ray = reflect(-view_dir, sky_normal);
        if (sky_ray.y > 0.0) {
            let horizon = water_sky_horizon(depth_uv, sky_ray);
            sky_weight = smoothstep(horizon + material.water_sky_ranges.z, horizon + material.water_sky_ranges.w, sky_ray.y);
            if (sky_weight > 0.0) {
                let sky = reflected_sky(in.world_position.xyz, sky_ray);
                probe = mix(probe, vec4<f32>(sky, 1.0), sky_weight);
            }
        }
    }
#endif
    let screen_reflection = water_screen_reflection(in.world_position.xyz, reflection_normal, view_dir);
    let reflected_rgb = mix(probe.rgb, screen_reflection.rgb, screen_reflection.a);
    let reflection_valid = max(probe.a, screen_reflection.a);
    var light_dir = normalize(vec3<f32>(40.0, 80.0, 30.0));
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    if (lights.n_directional_lights > 0u) {
        light_dir = lights.directional_lights[0u].direction_to_light;
    }
#endif
    if (!poison) {
        if (material.water_optics.control.w < 0.5) { return color; }
        if (graphics.counts.w > 0.5 || graphics.sun.w > 0.001) { light_dir = graphics.sun.xyz; }
        let optical_depth = mix(material.water.w, depth, valid_bed);
        return enhanced_water(in.world_position.xyz, in.position.xyz, surface_normal, normal, view_dir,
            light_dir, reflected_rgb, reflection_valid, sky_weight * (1.0 - screen_reflection.a),
            optical_depth, coverage, shade, dynamic.z);
    }
    let air_cos = clamp(abs(dot(surface_normal, view_dir)), 0.0, 1.0);
    let water_cos = sqrt(1.0 - (1.0 - air_cos * air_cos) / (1.333 * 1.333));
    let absorption = 1.0 - exp(-depth * material.water.w / water_cos);
    let alpha = mix(0.58 * coverage, absorption * coverage, valid_bed);
    let authored_luma = clamp(dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
    let body = vec3<f32>(0.065, 0.27, 0.015) * shade * (0.97 + 0.06 * authored_luma);
    let ndotv = clamp(abs(dot(reflection_normal, view_dir)), 0.0, 1.0);
    let f = 0.0204 + 0.9796 * pow(1.0 - ndotv, 5.0);
    let reflection_weight = min(f * reflection_valid * 0.55 + screen_reflection.a * 0.12, 0.8);
    let half_sum = view_dir + light_dir;
    let half_v = half_sum / max(length(half_sum), 0.0001);
    let spec = pow(max(dot(normal, half_v), 0.0), 28.0) * 0.09 * min(shade, 1.0);
    let out_alpha = reflection_weight + alpha * (1.0 - reflection_weight);
    let rgb = (body * alpha * (1.0 - reflection_weight)
        + reflected_rgb * reflection_weight + vec3<f32>(spec) * out_alpha) / max(out_alpha, 0.001);
    let foam = dynamic.z * (0.55 + 0.45 * smoothstep(0.0, 0.3, length(dynamic.xy)));
    return vec4<f32>(mix(rgb, vec3<f32>(0.48, 0.65, 0.61) * shade, foam),
        mix(out_alpha, 0.92, foam));
}

fn shade_spray(in: VertexOutput, facing_normal: vec3<f32>, color: vec4<f32>, shade: f32) -> vec4<f32> {
    let q = in.uv * 2.0 - vec2<f32>(1.0);
    let r2 = dot(q, q);
    if (r2 >= 1.0) { discard; }
    let thickness = sqrt(max(1.0 - r2, 0.0));
    var normal = facing_normal;
#ifdef VERTEX_TANGENTS
    let tangent = normalize(in.world_tangent.xyz);
    let bitangent = cross(facing_normal, tangent);
    normal = normalize(tangent * q.x + bitangent * q.y + facing_normal * thickness);
#endif
    let eye = normalize(view.world_position - in.world_position.xyz);
    let fresnel = 0.0204 + 0.9796 * pow(1.0 - abs(dot(normal, eye)), 5.0);
    var light_dir = normalize(vec3<f32>(40.0, 80.0, 30.0));
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    if (lights.n_directional_lights > 0u) {
        light_dir = lights.directional_lights[0u].direction_to_light;
    }
#endif
    let half_v = normalize(eye + light_dir + vec3<f32>(0.00001));
    let highlight = pow(max(dot(normal, half_v), 0.0), 32.0);
    let edge = 1.0 - smoothstep(0.72, 1.0, r2);
    let alpha = color.a * edge * (0.16 * thickness + fresnel * 0.16 + highlight * 0.5);
    return vec4<f32>(color.rgb * (0.7 + 0.3 * shade) + vec3<f32>(highlight * 0.45), alpha);
}

fn scene_detail_normal(in: VertexOutput, uv: vec2<f32>) -> vec3<f32> {
    let normal = normalize(in.world_normal);
#ifdef VERTEX_TANGENTS

    let sample = textureSample(normal_texture, normal_sampler, uv).xyz * 2.0 - 1.0;
    if (material.detail.x > 0.0 && (simulation_time.z > 0.0 || material.pbr.x > 0.5)) {
        let tangent = in.world_tangent.xyz - normal * dot(normal, in.world_tangent.xyz);
        if (dot(tangent, tangent) > 0.000001) {
            let t = normalize(tangent);
            let b = cross(normal, t) * in.world_tangent.w;

            return normalize((t * sample.x + b * sample.y) * material.detail.z + normal * sample.z);
        }
    }
#endif
    return normal;
}

fn scene_detail_factor(in: VertexOutput, normal: vec3<f32>) -> f32 {
    if (material.detail.x <= 0.0 || simulation_time.z <= 0.0) { return 1.0; }

    let direction = normalize(vec3<f32>(40.0, select(80.0, 35.0, material.detail.w > 0.8), 30.0));
    let base = max(dot(normalize(in.world_normal), direction), 0.0);
    let perturbed = max(dot(normal, direction), 0.0);
    let limit = select(0.35, 0.5, material.detail.w > 0.8);
    return clamp(1.0 + material.detail.w * (perturbed - base), 1.0 - limit, 1.0 + limit);
}

fn grass_patch_hash(cell: vec2<f32>) -> f32 {
    let basis = fract(cell.xyx * 0.1031);
    let value = basis + dot(basis, basis.yzx + 33.33);
    return fract((value.x + value.y) * value.z);
}

fn grass_patch(point: vec2<f32>) -> f32 {
    let cell = floor(point);
    let fraction = fract(point);
    let weight = fraction * fraction * (3.0 - 2.0 * fraction);
    return mix(
        mix(grass_patch_hash(cell), grass_patch_hash(cell + vec2<f32>(1.0, 0.0)), weight.x),
        mix(grass_patch_hash(cell + vec2<f32>(0.0, 1.0)), grass_patch_hash(cell + 1.0), weight.x),
        weight.y);
}

fn scene_lighting(in: VertexOutput, detail_normal: vec3<f32>) -> vec3<f32> {

    if (material.base.mode == 2u || material.base.mode == 3u || material.base.mode == 4u) {
        return vec3<f32>(1.0);
    }
    var shadow = 1.0;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    if (lights.n_directional_lights > 0u && lighting.w > 0.0
        && (mesh[in.instance_index].flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u) {
        shadow = fetch_directional_shadow(0u, in.world_position, normalize(in.world_normal),
            (view.view_from_world * in.world_position).z, in.position.xy);
    }
#endif
    if (graphics.counts.w > 0.5) {
        let irradiance = mix(graphics.ground.rgb, graphics.sky.rgb, detail_normal.y * 0.5 + 0.5);
        let sun = graphics.sun_color.rgb * graphics.sun.w * max(dot(detail_normal, graphics.sun.xyz), 0.0);
        let exposure = select(graphics.environment.z, 1.0, unified_hdr());
        let linear = exposure * (1.60 * irradiance + sun * (0.55 * mix(1.0, shadow, lighting.w)));
        if (unified_hdr()) { return linear; }
        return pack_linear_to_srgb(linear);
    }
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    if (lighting.x > 0.0 && lights.n_directional_lights > 0u) {

        let ambient = lighting.y * mix(1.0, shadow, lighting.w * 0.5);
        let diffuse = ambient + lighting.z * max(dot(detail_normal, lights.directional_lights[0u].direction_to_light), 0.0) * shadow;
        return vec3<f32>(diffuse * select(1.0, scene_detail_factor(in, detail_normal), material.detail.w > 0.8));
    }
#endif
    return vec3<f32>(mix(1.0, shadow, lighting.w) * scene_detail_factor(in, detail_normal));
}

struct GraphicsFrame {
    sky: vec4<f32>, ground: vec4<f32>, sun: vec4<f32>, sun_color: vec4<f32>,
    environment: vec4<f32>,
    fog_bounds: array<vec4<f32>,8>, fog_colors: array<vec4<f32>,8>,
    contacts: array<vec4<f32>,16>, contact_normals: array<vec4<f32>,16>,
    counts: vec4<f32>,
    water_wind: vec4<f32>, water_wind_cross: vec4<f32>,
    color_pipeline: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(17) var<uniform> graphics: GraphicsFrame;

fn unified_hdr() -> bool {
#ifdef METEOR_HDR_TARGET
    return graphics.color_pipeline.x > 0.5;
#else
    return false;
#endif
}

fn pack_srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(c / 12.92, pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), c > vec3<f32>(0.04045));
}
fn pack_linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    return select(x * 12.92, 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055, x > vec3<f32>(0.0031308));
}
fn pack_env_uv(n: vec3<f32>) -> vec2<f32> {

    return vec2<f32>(fract(atan2(n.z, n.x) / 6.2831853 + 0.5 + graphics.environment.y / 6.2831853), asin(clamp(n.y,-1.0,1.0)) / 3.14159265 + 0.5);
}
fn pack_pbr(in: VertexOutput, n: vec3<f32>, texel: vec3<f32>, vertex: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    var base = texel;
    if (material.pbr_textures.y < 0.5) { base = pack_srgb_to_linear(base); }
    base *= material.pbr_tint.rgb * mix(vec3<f32>(1.0), pack_srgb_to_linear(vertex), material.pbr.w);
    var orm = vec3<f32>(1.0);
    if (material.pbr_textures.x > 0.5) { orm = textureSample(water_normal, water_normal_sampler, uv).rgb; }
    let rough = clamp(material.pbr.z * orm.g, 0.045, 1.0);
    let metal = clamp(material.pbr.y * orm.b, 0.0, 1.0);
    let v = normalize(view.world_position - in.world_position.xyz + vec3<f32>(0.00001));
    let l = normalize(graphics.sun.xyz + vec3<f32>(0.00001));
    let h = normalize(v + l + vec3<f32>(0.00001));
    let nv = max(dot(n,v),0.001);
    let nl = max(dot(n,l),0.0);
    let nh = max(dot(n,h),0.0);
    let vh = max(dot(v,h),0.0);
    let f0 = mix(vec3<f32>(0.04),base,metal);
    let f = f0 + (vec3<f32>(1.0)-f0) * pow(1.0-vh,5.0);
    let a2 = pow(rough,4.0);
    let d = a2 / max(3.14159265 * pow(nh*nh*(a2-1.0)+1.0,2.0),0.000001);
    let k = pow(rough+1.0,2.0)/8.0;
    let g = nv/(nv*(1.0-k)+k) * nl/(nl*(1.0-k)+k);
    var shadow = 1.0;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    if (lights.n_directional_lights > 0u && lighting.w > 0.0) {
        shadow = mix(1.0, fetch_directional_shadow(0u,in.world_position,n,(view.view_from_world*in.world_position).z,in.position.xy), lighting.w);
    }
#endif
    let direct = ((vec3<f32>(1.0)-f)*(1.0-metal)*base/3.14159265 + f*d*g/max(4.0*nv*nl,0.001))
        * graphics.sun_color.rgb * graphics.sun.w * nl * shadow;
    var irradiance = mix(graphics.ground.rgb,graphics.sky.rgb,n.y*0.5+0.5);
    if (material.pbr_tint.w > 0.5 && graphics.counts.w < 0.5) { irradiance = textureSampleLevel(water_depth,water_depth_sampler,pack_env_uv(n),0.0).rgb; }
    let r = reflect(-v,n);
    var radiance = mix(graphics.ground.rgb,graphics.sky.rgb,r.y*0.5+0.5);
    if (material.pbr_textures.z > 0.5 && graphics.counts.w < 0.5) { radiance = textureSampleLevel(water_reflection,water_reflection_sampler,pack_env_uv(r),rough*graphics.environment.w).rgb; }

    let coeff = rough*vec4<f32>(-1.0,-0.0275,-0.572,0.022) + vec4<f32>(1.0,0.0425,1.04,-0.04);
    let a004 = min(coeff.x*coeff.x,exp2(-9.28*nv))*coeff.x+coeff.y;
    let ab = vec2<f32>(-1.04,1.04)*a004+coeff.zw;
    let ambient = ((1.0-metal)*base*irradiance + radiance*(f0*ab.x+vec3<f32>(ab.y))) * graphics.environment.x * orm.r;
    let radiance_sum = max(direct * material.pbr_lighting.x + ambient * material.pbr_lighting.y, vec3<f32>(0.0));
    if (unified_hdr()) { return radiance_sum; }
    let x = radiance_sum * graphics.environment.z;
    let mapped = clamp((x*(2.51*x+0.03))/(x*(2.43*x+0.59)+0.14),vec3<f32>(0.0),vec3<f32>(1.0));
    return pack_linear_to_srgb(mapped);
}
fn pack_contacts(position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if (graphics.counts.y <= 0.0) { return 1.0; }
    var shade = 1.0;
    for (var i=0u; i<min(u32(graphics.counts.y),16u); i+=1u) {
        let c=graphics.contacts[i]; let n=graphics.contact_normals[i];
        let delta=position-c.xyz; let height=abs(dot(delta,n.xyz));
        let radial=length(delta-n.xyz*dot(delta,n.xyz))/max(c.w,0.01);
        let amount=(1.0-smoothstep(0.0,1.0,radial))*(1.0-smoothstep(0.04,0.18,height))
            *smoothstep(0.5,0.9,dot(normal,n.xyz))*n.w;
        shade *= 1.0-amount;
    }
    return max(shade,0.2);
}
fn pack_fog(position: vec3<f32>, color: vec3<f32>) -> vec3<f32> {
    if (graphics.counts.x <= 0.0) { return color; }
    let ray=position-view.world_position; let distance_to_surface=length(ray);
    let direction=ray/max(distance_to_surface,0.0001);
    var optical_depth=0.0; var tint=vec3<f32>(0.0);
    for (var i=0u;i<min(u32(graphics.counts.x),8u);i+=1u) {
        let sphere=graphics.fog_bounds[i]; let fog=graphics.fog_colors[i];
        let offset=view.world_position-sphere.xyz;
        let b=dot(offset,direction);let d=b*b-dot(offset,offset)+sphere.w*sphere.w;
        if (d>0.0) {
            let entry=max(-b-sqrt(d),0.0);let exit=min(-b+sqrt(d),distance_to_surface);
            let depth=max(exit-entry,0.0)*fog.w;
            optical_depth+=depth;
            tint+=select(fog.rgb, pack_srgb_to_linear(fog.rgb), unified_hdr())*depth;
        }
    }
    let transmittance=exp(-min(optical_depth * material.pbr_lighting.z,16.0));
    return color*transmittance+tint/max(optical_depth,0.0001)*(1.0-transmittance);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let params = material.base;

    if (params.mode == 3u && lighting.w > 0.0) { discard; }

    let fade_view = distance(view.world_position, occlusion.eye.xyz) < 0.01;
    let elapsed = simulation_time.xy - material.scroll.zw;
    let scroll_time = elapsed.x + elapsed.y;
    var uv_offset = params.uv_offset;
    if (any(material.scroll.xy != vec2<f32>(0.0))) {
        uv_offset = fract(params.uv_offset + fract(material.scroll.xy * elapsed.x) + fract(material.scroll.xy * elapsed.y));
    }
    let water = water_enhance_active(params.mode);

    var texel = vec4<f32>(1.0);
    if (params.has_texture == 1u) {
        let uv_a = in.uv + uv_offset;
        texel = textureSample(base_color_texture, base_color_sampler, uv_a);
    }
    var diffuse = vec4<f32>(1.0);
#ifdef VERTEX_COLORS
    diffuse = in.color;
#endif
    var shading_normal = normalize(in.world_normal);
    if (water) {
        shading_normal = water_surface_normal(in);
    } else {
        shading_normal = scene_detail_normal(in, in.uv + uv_offset);
    }
    var color = texel * diffuse;
    if (unified_hdr() && !water && material.pbr.x <= 0.5 && (params.mode == 0u || params.mode == 1u)) {
        color = vec4<f32>(pack_srgb_to_linear(color.rgb), color.a);
    }
    color.a *= params.opacity;

    if (params.mode == 3u) {

        color = vec4<f32>(vec3<f32>(1.0) - color.rgb, 1.0);
    } else {

        let shade = scene_lighting(in, shading_normal);
        if (material.water.x == 7.0) {

            if (graphics.counts.w > 0.5) {

                let light = vec3<f32>(0.10) + graphics.sky.rgb * 2.0 + graphics.sun_color.rgb * graphics.sun.w * 0.42;
                let exposure = select(graphics.environment.z, 1.0, unified_hdr());
                let waterfall_light = light * exposure;
                let gain = select(pack_linear_to_srgb(waterfall_light), waterfall_light, unified_hdr());
                color = vec4<f32>(color.rgb * sqrt(max(gain, vec3<f32>(0.0))), color.a);
            }
        } else if (material.water.x == 5.0) {

            let phase = fract(simulation_time.x * 0.19) + fract(simulation_time.y * 0.19);
            let phase_b = fract(simulation_time.x * 0.1197) + fract(simulation_time.y * 0.1197);
            let uv = in.uv;
            let drift = 0.022 * sin(uv.y * 4.1 + phase * 6.28318);
            let a = textureSample(base_color_texture, base_color_sampler, uv * vec2<f32>(0.8, 0.78) + vec2<f32>(drift, -phase)).r;
            let b = textureSample(base_color_texture, base_color_sampler, uv * vec2<f32>(1.13, 0.57) + vec2<f32>(0.37 - drift, -phase_b)).r;
            let foam = clamp(a * 0.65 + b * 0.55, 0.0, 1.0);
            let edge = smoothstep(0.0, 0.16, uv.x + drift) * (1.0 - smoothstep(0.81, 0.99, uv.x + drift));
            color = vec4<f32>(mix(vec3<f32>(0.52, 0.69, 0.73), vec3<f32>(0.94, 0.97, 0.94), foam), foam * edge * params.opacity);
            if (graphics.counts.w > 0.5) {
                let light = (graphics.sky.rgb * 1.15 + graphics.sun_color.rgb * graphics.sun.w * 0.42)
                    * select(graphics.environment.z, 1.0, unified_hdr());
                color = vec4<f32>(color.rgb * select(pack_linear_to_srgb(light), light, unified_hdr()), color.a);
            }
        } else if (material.water.x == 6.0) {

            let p = in.uv * 2.0 - vec2<f32>(1.0);
            let phase = fract(simulation_time.x * 0.045) + fract(simulation_time.y * 0.045);
            let phase_y = fract(simulation_time.x * 0.0135) + fract(simulation_time.y * 0.0135);
            let noise = textureSample(base_color_texture, base_color_sampler, in.uv * vec2<f32>(1.7, 0.6) + vec2<f32>(phase, -phase_y)).r;
            let cloud = 1.0 - smoothstep(0.35, 1.0, length(p * vec2<f32>(0.92, 1.15)));
            color = vec4<f32>(vec3<f32>(0.72, 0.81, 0.79), cloud * (0.3 + 0.7 * noise) * params.opacity);
            if (graphics.counts.w > 0.5) {
                let light = (graphics.sky.rgb * 1.15 + graphics.sun_color.rgb * graphics.sun.w * 0.42)
                    * select(graphics.environment.z, 1.0, unified_hdr());
                color = vec4<f32>(color.rgb * select(pack_linear_to_srgb(light), light, unified_hdr()), color.a);
            }
        } else if (water) {
            color = shade_water(in, shading_normal, color, dot(shade, vec3<f32>(0.2126, 0.7152, 0.0722)));
        } else if (material.water.x == 4.0) {
            color = shade_spray(in, shading_normal, color, dot(shade, vec3<f32>(0.2126, 0.7152, 0.0722)));
        } else if (material.pbr.x > 0.5) {
            color = vec4<f32>(pack_pbr(in, shading_normal, texel.rgb, diffuse.rgb, in.uv + uv_offset), color.a);
        } else {
            color = vec4<f32>(color.rgb * shade, color.a);
        }
        if (graphics.counts.z > 0.5 && material.grass_ground.w > 0.0 && material.water.x == 0.0) {
            let point = in.world_position.xz;
            let uv = (point - material.grass_bounds.xy) / material.grass_bounds.zw;
            let slope = smoothstep(0.50, 0.82, normalize(in.world_normal).y);

            let scan_uv = vec2<f32>(dot(point, vec2<f32>(0.94, 0.342)),
                                    dot(point, vec2<f32>(-0.342, 0.94))) * material.grass_underlay_scale;
            let scan_dx = dpdx(scan_uv);
            let scan_dy = dpdy(scan_uv);
            if (slope > 0.0 && all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0))) {
                let cover = textureSampleLevel(surface_detail, surface_detail_sampler, uv, 0.0).rg;
                let edge = smoothstep(0.035, 0.80, cover.r);
                if (edge > 0.0) {
                    let meadow = 2.0 * grass_patch(point * 0.19) - 1.0;
                    var ground_color: vec3<f32>;
                    if (material.grass_underlay_scale > 0.0) {
                        let scan = textureSampleGrad(surface_aux, surface_detail_sampler, scan_uv, scan_dx, scan_dy).rgb;

                        let lightness = dot(scan, vec3<f32>(0.2126, 0.7152, 0.0722));
                        let canopy = material.grass_ground.rgb * (0.40 + min(2.9 * lightness, 0.62));
                        let growth = smoothstep(0.12, 0.68, cover.g) * smoothstep(0.20, 0.85, edge);
                        ground_color = mix(scan, canopy, growth) * (0.94 + 0.16 * meadow);
                    } else {
                        let tuft = sin(dot(point, vec2<f32>(2.4, 2.9)) + meadow) * sin(dot(point, vec2<f32>(-3.3, 2.1)));
                        let distance_gain = 1.0 - smoothstep(20.0, 105.0, distance(in.world_position.xyz, view.world_position));
                        let variation = 0.58 + 0.12 * meadow + 0.13 * tuft * distance_gain;
                        let fringe = mix(vec3<f32>(1.12, 0.84, 0.65), vec3<f32>(1.06, 0.91, 0.78), edge);
                        ground_color = material.grass_ground.rgb * variation * fringe;
                    }
                    let amount = material.grass_ground.w * slope * edge;
                    color = vec4<f32>(mix(color.rgb, ground_color * shade, amount), color.a);
                }
            }
        }
    }
    if (material.pbr_textures.w > 0.5 && params.mode == 0u) {
        color = vec4<f32>(color.rgb * pack_contacts(in.world_position.xyz, normalize(in.world_normal)), color.a);
    }

    if (material.first_person_clip.w > 0.0 && occlusion.self_eye.w > 0.0
        && distance(view.world_position, occlusion.self_eye.xyz) < 0.01
        && distance(in.world_position.xyz, occlusion.self_eye.xyz) < material.first_person_clip.w) {
        discard;
    }
    if (params.alpha_cutoff > 0.0 && color.a < params.alpha_cutoff) {
        discard;
    }

    if (!(water && material.water.x == 1.0) && fade_view && material.first_person_clip.w <= 0.0
        && occlusion.eye.w > 0.0 && camera_fade(in.world_position.xyz, in.position.xy)) {
        discard;
    }

    if (material.body_glow.w > 0.0) {
        let t = simulation_time.y;

        let style = max(material.body_style, vec4<f32>(0.001));
        let pulse = 0.70 + 0.26 * sin(t * 9.0) + 0.30 * pow(max(sin(t * 3.0), 0.0), 6.0);
        var rim: f32 = 0.0;
        var rays: f32 = 0.0;
        var flow: f32 = 0.0;
#ifdef VERTEX_NORMALS
        let glow_normal = normalize(in.world_normal);
        let view_dir = normalize(view.world_position.xyz - in.world_position.xyz);
        rim = pow(1.0 - abs(dot(glow_normal, view_dir)), 2.0);

        let forward = -view_dir;
        let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), forward));
        let outline_up = cross(forward, right);
        let angle = atan2(dot(glow_normal, outline_up), dot(glow_normal, right));
        let surge = 0.30 + 0.70 * pow(max(sin(t * 2.1), 0.0), 3.0);
        let rays_a = pow(0.5 + 0.5 * sin(angle * style.z + t * style.w), 18.0);
        let rays_b = pow(0.5 + 0.5 * sin(angle * style.z * 2.0 - t * style.w * 0.7), 10.0);
        rays = (rays_a + 0.55 * rays_b) * surge;
#endif

        let v1 = in.uv.y * style.y - t * style.x;
        let v2 = in.uv.y * style.y * 2.0 - t * style.x * 1.6;
        let d1 = abs(fract(v1) - 0.5);
        let d2 = abs(fract(v2) - 0.5);
        flow = exp(-d1 * d1 * 260.0) + 0.5 * exp(-d2 * d2 * 520.0);

        let emission = material.body_glow.rgb * material.body_glow.w * pulse;
        let sheen = emission * (vec3<f32>(0.16) + texel.rgb * 0.20) * (0.30 + 1.20 * flow);
        let shell = emission * (0.18 + 3.4 * rim * rim * (0.45 + 1.6 * rays) + 1.1 * rim * flow);
        color = vec4<f32>(color.rgb + sheen + shell, color.a);
    }
    if (params.mode == 0u || params.mode == 1u) {
        color = vec4<f32>(pack_fog(in.world_position.xyz, color.rgb), color.a);
    }
    return color;
}
