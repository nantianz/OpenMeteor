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

#ifdef WATER_DEPTH_HIERARCHY
@group(#{MATERIAL_BIND_GROUP}) @binding(22) var water_depth_hierarchy: texture_2d<f32>;
#endif
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
    stage_render_flags: u32,
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

fn project_screen_ray(clip: vec4<f32>) -> ScreenRaySample {
    if (clip.w <= 0.0) { return ScreenRaySample(vec2<f32>(0.0), vec3<f32>(0.0), 0.0, false); }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (ndc.z <= 0.0 || ndc.z >= 1.0
        || any(uv <= vec2<f32>(0.01)) || any(uv >= vec2<f32>(0.99))) {
        return ScreenRaySample(uv, ndc, 0.0, false);
    }
    return ScreenRaySample(uv, ndc, 0.0, true);
}

fn load_screen_ray(sample: ScreenRaySample) -> ScreenRaySample {
    var result = sample;
    if (sample.valid) {
        result.depth = textureLoad(depth_prepass_texture, screen_ray_pixel(sample.uv), 0);
    }
    return result;
}
fn sample_screen_ray(world: vec3<f32>, ray: vec3<f32>, distance_along: f32) -> ScreenRaySample {
    let clip = view.clip_from_world * vec4<f32>(world + ray * distance_along, 1.0);
    return load_screen_ray(project_screen_ray(clip));
}
#ifdef WATER_DEPTH_HIERARCHY
fn screen_ray_block_in_front(first: ScreenRaySample, last: ScreenRaySample) -> bool {
    if (!first.valid || !last.valid) { return false; }
    let a = vec2<u32>(screen_ray_pixel(first.uv));
    let b = vec2<u32>(screen_ray_pixel(last.uv));
    let levels = textureNumLevels(water_depth_hierarchy);
    let differing_bits = (a.x ^ b.x) | (a.y ^ b.y);
    let shift = max(2u, 32u - countLeadingZeros(differing_bits));
    let level = shift - 2u;
    if (level >= levels) { return false; }
    let cell = a >> vec2<u32>(shift);
    if (any(cell >= textureDimensions(water_depth_hierarchy, level))) { return false; }
    let nearest_encoded = textureLoad(water_depth_hierarchy, vec2<i32>(cell), i32(level)).r;
    return nearest_encoded >= 1.0
        && min(first.ndc.z, last.ndc.z) > nearest_encoded - 1.0 + 0.000001;
}
#endif
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
    if (view.world_position.y <= world.y + 0.02 || view_dir.y <= 0.02) { return vec4<f32>(0.0); }
    let ray = normalize(reflect(-view_dir, normal));
    if (ray.y <= 0.02) { return vec4<f32>(0.0); }
    var distance_along = 0.20;
    var previous_distance = 0.0;
    var previous_front = true;
#ifdef WATER_DEPTH_HIERARCHY
    let ray_origin_clip = view.clip_from_world * vec4<f32>(world, 1.0);
    let ray_direction_clip = view.clip_from_world * vec4<f32>(ray, 0.0);
#endif
    for (var step = 0u; step < 6u; step += 1u) {
#ifdef WATER_DEPTH_HIERARCHY
        let first = project_screen_ray(ray_origin_clip + ray_direction_clip * distance_along);
        if (step + 3u < 6u) {
            let block_end = ((distance_along * 2.0) * 2.0) * 2.0;
            let last = project_screen_ray(ray_origin_clip + ray_direction_clip * block_end);
            if (screen_ray_block_in_front(first, last)) {
                previous_front = true;
                previous_distance = block_end;
                distance_along = block_end * 2.0;
                step += 3u;
                continue;
            }
        }
        let sample = load_screen_ray(first);
#else
        let sample = sample_screen_ray(world, ray, distance_along);
#endif
        if (!sample.valid) { break; }
        let behind = sample.depth > 0.0 && sample.ndc.z <= sample.depth;
        if (behind && previous_front) {
            var near = previous_distance;
            var far = distance_along;
            var hit = sample;

            for (var refine = 0u; refine < 2u; refine += 1u) {
                let middle = (near + far) * 0.5;
#ifdef WATER_DEPTH_HIERARCHY
                let candidate = load_screen_ray(project_screen_ray(ray_origin_clip + ray_direction_clip * middle));
#else
                let candidate = sample_screen_ray(world, ray, middle);
#endif
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
        distance_along *= 2.0;
    }
#endif
    return vec4<f32>(0.0);
}

fn water_screen_refraction(world: vec3<f32>, frag_coord: vec3<f32>,
    ray: vec3<f32>, path: f32) -> vec4<f32> {
#ifdef DEPTH_PREPASS
    if ((u32(material.detail.y) & 2u) == 0u || ray.y >= -0.02 || path <= 0.06) { return vec4<f32>(0.0); }
    let source_pixel = vec2<i32>(frag_coord.xy);
    let source_depth = textureLoad(depth_prepass_texture, source_pixel, 0);
    if (source_depth > frag_coord.z + 0.000001) { return vec4<f32>(0.0); }
    let refracted = sample_screen_ray(world, ray, min(path, 2.2));
    if (refracted.valid && refracted.depth > 0.0 && refracted.depth <= frag_coord.z + 0.000001) {
        let pixel = screen_ray_pixel(refracted.uv);
        let edge = max(abs(refracted.uv.x * 2.0 - 1.0), abs(refracted.uv.y * 2.0 - 1.0));
        let color = water_refraction_color(world, pixel, vec2<f32>(refracted.depth),
            1.0 - smoothstep(0.82, 0.98, edge));
        if (color.a > 0.0) { return color; }
            }
    return water_refraction_color(world, source_pixel, vec2<f32>(source_depth), 1.0);
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
    let classic = material.water_optics.control.w == 3.0;
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
    if (!poison && !classic) {
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
    let shore = smoothstep(0.0, 0.85, bed.a);
    let clear_absorption = clamp(absorption * mix(0.62, 1.0, shore) + 0.18 * shore, 0.0, 0.93);
    let alpha = mix(select(color.a, 0.58 * coverage, poison), select(clear_absorption, absorption, poison) * coverage, valid_bed);
    let authored_luma = clamp(dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
    let scatter = select(vec3<f32>(0.025, 0.090, 0.075), vec3<f32>(0.065, 0.27, 0.015), poison);
    let body = scatter * shade * (0.97 + 0.06 * authored_luma);
    let ndotv = clamp(abs(dot(reflection_normal, view_dir)), 0.0, 1.0);
    let f0 = select(0.045, 0.0204, poison);
    let f = f0 + (1.0 - f0) * pow(1.0 - ndotv, select(4.0, 5.0, poison));
    let reflection_weight = min(f * reflection_valid * select(1.5, 0.55, poison) + screen_reflection.a * 0.12, 0.8);
    let half_sum = view_dir + light_dir;
    let half_v = half_sum / max(length(half_sum), 0.0001);
    let spec = pow(max(dot(normal, half_v), 0.0), select(64.0, 28.0, poison)) * select(0.22, 0.09, poison) * min(shade, 1.0);
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

#ifndef NO_ARRAY_TEXTURES_SUPPORT
#import bevy_pbr::mesh_view_bindings::directional_shadow_textures;
#import bevy_pbr::shadows::{get_cascade_index, world_to_directional_light_local};
fn authored_shadow_sample(index: u32, position: vec4<f32>, normal: vec3<f32>, row: i32) -> f32 {
    let light = &lights.directional_lights[0u];
    let cosine = max(dot(normal, (*light).direction_to_light), 0.0);
    let offset = cosine * (*light).shadow_normal_bias * (*light).cascades[index].texel_size * normal;
    let biased = position.xyz + offset + (*light).shadow_depth_bias * (*light).direction_to_light;
    let projected = world_to_directional_light_local(0u, index, vec4<f32>(biased, position.w));
    if (projected.w == 0.0) { return 1.0; }
    let size = vec2<i32>(textureDimensions(directional_shadow_textures));
    let pixel = projected.xy * vec2<f32>(size);
    let first = vec2<i32>(floor(pixel - vec2<f32>(0.5 * f32(row - 1))));
    let layer = i32((*light).depth_texture_base_index + index);
    var samples = 0.0;
    for (var y = 0; y < row; y += 1) {
        for (var x = 0; x < row; x += 1) {
            let coordinate = clamp(first + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            let blocker = textureLoad(directional_shadow_textures, coordinate, layer, 0);
            samples += select(0.0, 1.0, projected.z >= blocker);
        }
    }
    return samples / f32(row * row);
}
fn authored_directional_shadow(position: vec4<f32>, normal: vec3<f32>, depth: f32, row: i32) -> f32 {
    let light = &lights.directional_lights[0u];
    let index = get_cascade_index(0u, depth);
    if (index >= (*light).num_cascades) { return 1.0; }
    let active_row = select(1, min(row, 3), index == 0u);
    let here = authored_shadow_sample(index, position, normal, active_row);
    if (index > 0u || (*light).cascades_overlap_proportion == 0.0 || index + 1u >= (*light).num_cascades) {
        return here;
    }
    let near = 0.0;
    let far = (*light).cascades[index].far_bound;
    let remaining = (far + depth) / max(far - near, 0.001);
    if (remaining >= 0.1) { return here; }
    let next = authored_shadow_sample(index + 1u, position, normal, 1);
    return mix(next, here, smoothstep(0.0, 0.1, remaining));
}
#endif
fn scene_lighting(in: VertexOutput, detail_normal: vec3<f32>) -> vec3<f32> {

    if (material.base.mode == 2u || material.base.mode == 3u || material.base.mode == 4u) {
        return vec3<f32>(1.0);
    }
    let sky_lit = graphics.counts.w > 0.5;
    let sun_cosine = max(dot(detail_normal, graphics.sun.xyz), 0.0);
    var shadow = 1.0;
#ifndef GMDL_CHARACTER
    let authored = (material.stage_render_flags & 1u) != 0u;
    let authored_sun = authored && !sky_lit;
    let authored_unlit = authored && (material.stage_render_flags & 2u) != 0u;
    let authored_lambert = select(1.0, max(dot(detail_normal, graphics.stage_render.sun.xyz), 0.0), authored_sun);
    let skip_authored_backface_shadow = authored_sun
        && !authored_unlit
        && graphics.stage_render.ambient.w > 0.0
        && material.base.has_texture != 0u
        && authored_lambert <= 0.0;
#else
    let skip_authored_backface_shadow = false;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    if (lights.n_directional_lights > 0u && lighting.w > 0.0
        && !skip_authored_backface_shadow
        && (!sky_lit || (graphics.sun.w > 0.0 && sun_cosine > 0.0))
        && (mesh[in.instance_index].flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u) {
        let view_depth = (view.view_from_world * in.world_position).z;
        let normal = normalize(in.world_normal);
#ifndef NO_ARRAY_TEXTURES_SUPPORT
#ifndef GMDL_CHARACTER
        if (authored_sun && graphics.stage_render.point_count.z > 0.0) {
            shadow = authored_directional_shadow(in.world_position, normal, view_depth,
                i32(graphics.stage_render.point_count.z));
        } else
#endif
        {
            shadow = fetch_directional_shadow(0u, in.world_position, normal, view_depth, in.position.xy);
        }
#else
        shadow = fetch_directional_shadow(0u, in.world_position, normal, view_depth, in.position.xy);
#endif
    }
#endif
#ifndef GMDL_CHARACTER
    if (authored && (material.stage_render_flags & 2u) != 0u) {
        let transmission = select(mix(1.0, shadow, lighting.w), mix(shadow, 1.0, graphics.stage_render.sun.w), authored_sun);
        return vec3<f32>(transmission);
    }
    if (authored_sun) {
        let room = &graphics.stage_render;
        let transmission = mix(shadow, 1.0, (*room).sun.w);
        if ((*room).ambient.w > 0.0 && material.base.has_texture != 0u) {
            let lambert = max(dot(detail_normal, (*room).sun.xyz), 0.0);
            let light = (*room).ambient.rgb + (*room).sun_color.rgb * lambert * transmission;
            return light + authored_point_lighting(in.world_position.xyz, detail_normal);
        }
    }
#endif
    if (sky_lit) {
        let irradiance = mix(graphics.ground.rgb, graphics.sky.rgb, detail_normal.y * 0.5 + 0.5);
        let sun = graphics.sun_color.rgb * graphics.sun.w * sun_cosine;
        let exposure = select(graphics.environment.z, 1.0, unified_hdr());
        let linear = exposure * (1.60 * irradiance + sun * (0.55 * mix(1.0, shadow, lighting.w)));
        var points = vec3<f32>(0.0);
#ifndef GMDL_CHARACTER
        if (authored && graphics.stage_render.point_count.x > 0.0) {
            points = authored_point_lighting(in.world_position.xyz, detail_normal);
        }
#endif
        if (unified_hdr()) { return linear + pack_srgb_to_linear(points); }
        return pack_linear_to_srgb(linear) + points;
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

struct StageRenderFrame {
    ambient: vec4<f32>, sun: vec4<f32>, sun_color: vec4<f32>,
    fog_params: vec4<f32>, fog_color: vec4<f32>, point_count: vec4<f32>,
    point_positions: array<vec4<f32>, 255>, point_colors: array<vec4<f32>, 255>,
    point_grid: vec4<f32>, point_cells: array<vec4<u32>, 256>,
};
struct GraphicsFrame {
    sky: vec4<f32>, ground: vec4<f32>, sun: vec4<f32>, sun_color: vec4<f32>,
    environment: vec4<f32>,
    fog_bounds: array<vec4<f32>,8>, fog_colors: array<vec4<f32>,8>,
    contacts: array<vec4<f32>,16>, contact_normals: array<vec4<f32>,16>,
    counts: vec4<f32>,
    water_wind: vec4<f32>, water_wind_cross: vec4<f32>,
    color_pipeline: vec4<f32>,
    fx_lights: array<vec4<f32>,8>, fx_light_colors: array<vec4<f32>,8>,
    stage_render: StageRenderFrame,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(17) var<uniform> graphics: GraphicsFrame;

fn authored_point_lighting(position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let room = &graphics.stage_render;
    let count = u32((*room).point_count.x);
    let localized = count > 16u;
    var indices = vec4<u32>(0u);
    if (localized) {
        let grid = (*room).point_grid;
        let cell = vec2<u32>(clamp((position.xz - grid.xy) * grid.zw, vec2<f32>(0.0), vec2<f32>(15.0)));
        indices = (*room).point_cells[cell.y * 16u + cell.x];
    }
    var sum = vec3<f32>(0.0);
    for (var slot = 0u; slot < min(count, 16u); slot += 1u) {
        let entry = select(slot + 1u, (indices[slot / 4u] >> ((slot % 4u) * 8u)) & 255u, localized);
        if (entry == 0u) { break; }
        let index = entry - 1u;
        let point = (*room).point_positions[index];
        let vector = point.xyz - position;
        let distance_sq = dot(vector, vector);
        let range_factor = 1.0 - distance_sq * point.w * point.w;
        if (range_factor > 0.0) {
            let diffuse = max(dot(normal, vector * inverseSqrt(max(distance_sq, 0.000001))), 0.0);
            sum += (*room).point_colors[index].rgb * range_factor * (1.0 + diffuse);
        }
    }
    return sum;
}
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
    if (nl > 0.0 && graphics.sun.w > 0.0 && material.pbr_lighting.x > 0.0
        && lights.n_directional_lights > 0u && lighting.w > 0.0) {
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

fn pack_fx_lights(position: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    let count = min(u32(graphics.color_pipeline.y), 8u);
    for (var i = 0u; i < count; i += 1u) {
        let light = graphics.fx_lights[i];
        let to_light = light.xyz - position;
        let distance_sq = max(dot(to_light, to_light), 0.01);
        let range = max(light.w, 0.01);
        let range_sq = range * range;
        if (distance_sq >= range_sq) { continue; }
        let window = 1.0 - distance_sq / range_sq;
        let n_dot_l = max(dot(normal, to_light * inverseSqrt(distance_sq)), 0.0);
        sum += graphics.fx_light_colors[i].rgb * (window * window * n_dot_l / (1.0 + distance_sq));
    }
    return sum;
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

fn authored_stage_fog(position: vec4<f32>, color: vec3<f32>) -> vec3<f32> {
    let stage = &graphics.stage_render;
    if ((material.stage_render_flags & 1u) == 0u || (*stage).fog_color.w == 0.0 ||
        all(color == vec3<f32>(0.0))) {
        return color;
    }
    let depth = -(view.view_from_world * position).z * (*stage).point_count.y;
    let params = (*stage).fog_params;
    var visible = saturate((params.x - depth) * params.y);
    if ((*stage).fog_color.w == 2.0) {
        let height = (position.y - params.z) * params.w;
        visible = min(visible, 1.0 - exp(-height * height));
    }
    let fog = select((*stage).fog_color.rgb, pack_srgb_to_linear((*stage).fog_color.rgb), unified_hdr());
    let background = select(fog, color + fog, (material.stage_render_flags & 4u) != 0u);
    return mix(background, color, visible);
}
@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let params = material.base;

#ifdef GMDL_MULTIPLY_SHADOW
    if (params.mode == 3u && lighting.w > 0.0) { discard; }

#endif
#ifdef GMDL_CHARACTER
    if (material.first_person_clip.w > 0.0 && occlusion.self_eye.w > 0.0
        && distance(view.world_position, occlusion.self_eye.xyz) < 0.01
        && distance(in.world_position.xyz, occlusion.self_eye.xyz) < material.first_person_clip.w) {
        discard;
    }
#endif
    let elapsed = simulation_time.xy - material.scroll.zw;
    let scroll_time = elapsed.x + elapsed.y;
    var uv_offset = params.uv_offset;
    if (any(material.scroll.xy != vec2<f32>(0.0))) {
        uv_offset = fract(params.uv_offset + fract(material.scroll.xy * elapsed.x) + fract(material.scroll.xy * elapsed.y));
    }
#ifdef GMDL_CHARACTER
    let water = false;
#else
    let water = water_enhance_active(params.mode);

#endif
    var texel = vec4<f32>(1.0);
    if (params.has_texture == 1u) {
        let uv_a = in.uv + uv_offset;
        texel = textureSample(base_color_texture, base_color_sampler, uv_a);
    }
    var diffuse = vec4<f32>(1.0);
#ifdef VERTEX_COLORS
    diffuse = in.color;
#endif
    var color = texel * diffuse;
    color.a *= params.opacity;
#ifdef GMDL_ALPHA_TEST
    if (params.alpha_cutoff > 0.0 && color.a < params.alpha_cutoff) {
        discard;
    }
#endif
#ifdef GMDL_CAMERA_FADE
    if (occlusion.eye.w > 0.0
        && distance(view.world_position, occlusion.eye.xyz) < 0.01
        && camera_fade(in.world_position.xyz, in.position.xy)) {
        discard;
    }
#endif
    var shading_normal = normalize(in.world_normal);
#ifndef GMDL_CHARACTER
    if (water) {
        shading_normal = water_surface_normal(in);
    } else {
        shading_normal = scene_detail_normal(in, in.uv + uv_offset);
    }
#endif
    if (unified_hdr() && !water && material.pbr.x <= 0.5 && (params.mode == 0u || params.mode == 1u)) {
        color = vec4<f32>(pack_srgb_to_linear(color.rgb), color.a);
    }

    if (params.mode == 3u) {

        color = vec4<f32>(vec3<f32>(1.0) - color.rgb, 1.0);
    } else {

        var shade = vec3<f32>(1.0);
#ifndef GMDL_CHARACTER
        let pbr = material.pbr.x > 0.5;
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
            shade = scene_lighting(in, shading_normal);
            color = shade_water(in, shading_normal, color, dot(shade, vec3<f32>(0.2126, 0.7152, 0.0722)));
        } else if (material.water.x == 4.0) {
            shade = scene_lighting(in, shading_normal);
            color = shade_spray(in, shading_normal, color, dot(shade, vec3<f32>(0.2126, 0.7152, 0.0722)));
        } else if (pbr) {
            color = vec4<f32>(pack_pbr(in, shading_normal, texel.rgb, diffuse.rgb, in.uv + uv_offset), color.a);
        } else
#endif
        {
            shade = scene_lighting(in, shading_normal);
            color = vec4<f32>(color.rgb * shade, color.a);
        }
#ifndef GMDL_CHARACTER
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
                    if (pbr) {
                        shade = scene_lighting(in, shading_normal);
                    }
                    color = vec4<f32>(mix(color.rgb, ground_color * shade, amount), color.a);
                }
            }
        }
#endif
    }
#ifndef GMDL_CHARACTER
    if (material.pbr_textures.w > 0.5 && params.mode == 0u) {
        color = vec4<f32>(color.rgb * pack_contacts(in.world_position.xyz, normalize(in.world_normal)), color.a);
    }

#endif

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
    if (graphics.color_pipeline.y > 0.0 && (params.mode == 0u || params.mode == 1u)) {
        let flash = pack_fx_lights(in.world_position.xyz, normalize(in.world_normal));
        let lit = texel.rgb * select(pack_linear_to_srgb(flash), flash, unified_hdr());
        color = vec4<f32>(color.rgb + lit, color.a);
    }
    if (params.mode == 0u || params.mode == 1u) {
#ifdef GMDL_CHARACTER
        let fogged = color.rgb;
#else
        let fogged = authored_stage_fog(in.world_position, color.rgb);
#endif
        color = vec4<f32>(pack_fog(in.world_position.xyz, fogged), color.a);
    }
    return color;
}
