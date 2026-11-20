#import bevy_pbr::{mesh_functions, mesh_view_bindings::{view, lights}}
#import bevy_pbr::shadows::fetch_directional_shadow

struct SnowTerrain {
    bounds: vec4<f32>,
    geometry: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> snow: SnowTerrain;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var coverage: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var coverage_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var deformation: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var deformation_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var crystal: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var crystal_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) @interpolate(flat) disturbed: u32,
}

fn map_uv(point: vec2<f32>) -> vec2<f32> {
    return (point - snow.bounds.xy) / snow.bounds.zw;
}

fn depth_factor(mask: f32) -> f32 {
    return smoothstep(0.025, 0.92, mask);
}

fn drift(point: vec2<f32>) -> f32 {
    return 0.86 + 0.13 * sin(dot(point, vec2<f32>(1.08, 0.62)))
        + 0.10 * sin(dot(point, vec2<f32>(-0.56, 1.47)));
}

fn displacement(depth: f32, change: vec2<f32>) -> f32 {
    return depth * (change.y * 0.29 - change.x * 0.89);
}

fn base_height(point: vec2<f32>) -> f32 {
    let mask = textureSampleLevel(coverage, coverage_sampler, map_uv(point), 0.0).r;
    return snow.geometry.x * depth_factor(mask) * drift(point);
}

fn refined_base(point: vec2<f32>, step: f32) -> f32 {
    let cell = floor(point / step);
    let q = point / step - cell;
    let corner = cell * step;
    let right = base_height(corner + vec2<f32>(step, 0.0));
    let far = base_height(corner + vec2<f32>(0.0, step));

    if (q.x + q.y <= 1.0) {
        return (1.0 - q.x - q.y) * base_height(corner) + q.x * right + q.y * far;
    }
    return (q.x + q.y - 1.0) * base_height(corner + step) + (1.0 - q.y) * right + (1.0 - q.x) * far;
}

@vertex
fn vertex(@location(0) point: vec3<f32>, @builtin(instance_index) instance: u32) -> VertexOutput {
    var local = point;
    let tag = mesh_functions::get_tag(instance);
    let tile = snow.geometry.y;
    let coarse = snow.geometry.z;

    if ((point.x == 0.0 && (tag & 2u) != 0u) || (point.x == tile && (tag & 4u) != 0u)) {
        local.z = floor(point.z / coarse) * coarse;
    }
    if ((point.z == 0.0 && (tag & 8u) != 0u) || (point.z == tile && (tag & 16u) != 0u)) {
        local.x = floor(point.x / coarse) * coarse;
    }
    var world = mesh_functions::mesh_position_local_to_world(
        mesh_functions::get_world_from_local(instance), vec4<f32>(local, 1.0),
    );
    let uv = map_uv(world.xz);
    var y: f32;
    if ((tag & 32u) != 0u) {
        y = refined_base(world.xz, coarse);
    } else {
        y = base_height(world.xz);
    }
    if ((tag & 1u) != 0u) {
        y += displacement(y, textureSampleLevel(deformation, deformation_sampler, uv, 0.0).rg);
    }
    world.y += y;
    var out: VertexOutput;
    out.position = view.clip_from_world * world;
    out.world = world.xyz;
    out.disturbed = tag & 1u;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let point = in.world.xz;
    let uv = map_uv(point);
    let mask = textureSample(coverage, coverage_sampler, uv).r;
    let footprint = max(length(dpdx(point)), length(dpdy(point)));
    let face = cross(dpdy(in.world), dpdx(in.world));
    let up_face = select(-face, face, face.y >= 0.0);
    var normal = normalize(up_face + vec3<f32>(0.0, 0.00000001, 0.0));
    let grain = textureSample(crystal, crystal_sampler, point * 0.87).r;

    let alpha = smoothstep(0.035, 0.80, mask + (grain - 0.5) * 0.23);
    if (mask <= 0.006 || alpha <= 0.001) { discard; }

    let thickness = snow.geometry.x * depth_factor(mask) * drift(point);
    var compaction = 0.0;
    if (in.disturbed != 0u) {
        let contact = textureSampleLevel(deformation, deformation_sampler, uv, 0.0).rg;
        compaction = contact.x;

        if (max(contact.x, contact.y) > 0.006 && footprint < 0.18) {
            let h = snow.geometry.w;
            let offset = vec2<f32>(h) / snow.bounds.zw;
            let dx = textureSampleLevel(deformation, deformation_sampler, uv + vec2<f32>(offset.x, 0.0), 0.0).rg;
            let dz = textureSampleLevel(deformation, deformation_sampler, uv + vec2<f32>(0.0, offset.y), 0.0).rg;
            let slope = (vec2<f32>(displacement(thickness, dx), displacement(thickness, dz))
                - displacement(thickness, contact)) / h;
            normal = normalize(mix(normal, normalize(vec3<f32>(-slope.x, 1.0, -slope.y)), 0.78));
        }
    }

    let rough = grain - 0.5;
    let loose = mix(vec3<f32>(0.57, 0.62, 0.70), vec3<f32>(0.81, 0.89, 0.99),
        smoothstep(0.09, 0.53, mask));
    let packed = vec3<f32>(0.48, 0.63, 0.81);
    let tint = mix(loose, packed, compaction * 0.78);
    var sun = normalize(vec3<f32>(-0.63, 0.56, -0.35));
    var shadow = 1.0;
    if (lights.n_directional_lights != 0u) {
        sun = normalize(sun + lights.directional_lights[0u].direction_to_light);
        if (lights.directional_lights[0u].num_cascades > 0u) {
            let world = vec4<f32>(in.world, 1.0);
            shadow = fetch_directional_shadow(0u, world, vec3<f32>(0.0, 1.0, 0.0),
                (view.view_from_world * world).z, in.position.xy);
        }
    }
    let sky_fill = 0.50 + normal.y * 0.14;
    let sunlight = (0.06 + 0.32 * max(0.0, dot(normal, sun))) * shadow;
    let shade = (sky_fill + sunlight) * (1.0 - 0.30 * compaction);
    let glint = smoothstep(0.66, 0.83, grain);
    let rgb = tint * (shade + 0.16 * rough) + (0.045 - 0.036 * compaction) * glint * shadow;
    return vec4<f32>(rgb, alpha);
}
