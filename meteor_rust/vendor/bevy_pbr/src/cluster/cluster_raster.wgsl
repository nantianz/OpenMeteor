#import bevy_pbr::cluster::{
    Aabb, CLUSTERABLE_OBJECT_TYPE_DECAL, CLUSTERABLE_OBJECT_TYPE_IRRADIANCE_VOLUME,
    CLUSTERABLE_OBJECT_TYPE_POINT_LIGHT, CLUSTERABLE_OBJECT_TYPE_REFLECTION_PROBE,
    CLUSTERABLE_OBJECT_TYPE_SPOT_LIGHT, ClusterableObjectZSlice,
    calculate_sphere_cluster_bounds, compute_view_from_world_scale
}
#import bevy_pbr::clustered_forward
#import bevy_pbr::light_probes::transpose_affine_matrix
#import bevy_pbr::mesh_view_types::{
    ClusterOffsetsAndCounts, ClusterableObjectIndexLists, ClusteredDecals, ClusteredLights,
    LightProbes, Lights, POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE
}
#import bevy_render::view::View

struct Vertex {
    @builtin(instance_index) instance_id: u32,
    @location(0) position: vec2<f32>,
}

struct Varyings {
    @builtin(position) position: vec4<f32>,

    @location(0) @interpolate(flat) instance_id: u32,

    @location(1) @interpolate(flat) sphere_position: vec3<f32>,

    @location(2) @interpolate(flat) sphere_radius: f32,
}

struct ClusterOffsetsAndCountsAtomic {
    data: array<ClusterOffsetsAndCountsElementAtomic>,
}

struct ClusterOffsetsAndCountsElementAtomic {
    offset: atomic<u32>,
    point_lights: atomic<u32>,
    spot_lights: atomic<u32>,
    reflection_probes: atomic<u32>,
    irradiance_volumes: atomic<u32>,
    decals: atomic<u32>,
    pad_a: u32,
    pad_b: u32,
}

@group(0) @binding(0) var<storage> z_slices: array<ClusterableObjectZSlice>;
#ifndef VERTEX_SHADER

@group(0) @binding(1) var<storage, read_write> index_lists: ClusterableObjectIndexLists;
#endif

@group(0) @binding(2) var<storage> clustered_lights: ClusteredLights;

@group(0) @binding(3) var<uniform> light_probes: LightProbes;

@group(0) @binding(4) var<storage> clustered_decals: ClusteredDecals;

@group(0) @binding(5) var<uniform> lights: Lights;

@group(0) @binding(6) var<uniform> view: View;
#ifndef VERTEX_SHADER
#ifdef POPULATE_PASS

@group(0) @binding(7) var<storage> offsets_and_counts: ClusterOffsetsAndCounts;

@group(0) @binding(8) var<storage, read_write> scratchpad_offsets_and_counts:
    ClusterOffsetsAndCountsAtomic;
#else

@group(0) @binding(7) var<storage, read_write> offsets_and_counts: ClusterOffsetsAndCountsAtomic;
#endif
#endif

@vertex
fn vertex_main(vertex: Vertex) -> Varyings {
    let instance_id = vertex.instance_id;
    let object_index = z_slices[instance_id].object_index;
    let object_type = z_slices[instance_id].object_type;

    let bounding_sphere = get_object_bounding_sphere(object_index, object_type);
    let position = bounding_sphere.xyz;
    let radius = bounding_sphere.w;

    let view_from_world_scale = compute_view_from_world_scale(view.world_from_view);
    let max_view_from_world_scale = max(view_from_world_scale.x,
        max(view_from_world_scale.y, view_from_world_scale.z));
    let is_orthographic = view.clip_from_view[3].w == 1.0;

    let cluster_bounds = calculate_sphere_cluster_bounds(
        position,
        radius,
        view.view_from_world,
        view.clip_from_view,
        view_from_world_scale,
        lights.cluster_dimensions.xyz,
        lights.cluster_factors.zw,
        is_orthographic
    );
    let cluster_bounds_xy = vec4<u32>(cluster_bounds.min.xy, cluster_bounds.max.xy + vec2<u32>(1u));

    let view_position = (view.view_from_world * vec4(position, 1.0)).xyz;
    let view_radius = max_view_from_world_scale * radius;

    return Varyings(
        calculate_vertex_position(vertex, cluster_bounds_xy),
        instance_id,
        view_position,
        view_radius
    );
}

fn calculate_vertex_position(vertex: Vertex, cluster_bounds: vec4<u32>) -> vec4<f32> {
    let framebuffer_position = vec2<u32>(
        select(cluster_bounds.x, cluster_bounds.z, vertex.position.x == 1.0),
        select(cluster_bounds.y, cluster_bounds.w, vertex.position.y == 1.0)
    );
    let vertex_position =
        vec2<f32>(framebuffer_position) / vec2<f32>(lights.cluster_dimensions.xy);
    return vec4(mix(vec2(-1.0, 1.0), vec2(1.0, -1.0), vertex_position), 0.0, 1.0);
}

#ifndef VERTEX_SHADER

@fragment
fn fragment_main(varyings: Varyings) -> @location(0) vec4<f32> {
    let instance_id = varyings.instance_id;
    let object_index = z_slices[instance_id].object_index;
    let object_type = z_slices[instance_id].object_type;
    let z_slice = z_slices[instance_id].z_slice;

    let is_orthographic = view.clip_from_view[3].w == 1.0;
    let screen_size = view.viewport.zw;
    let tile_size = screen_size / vec2<f32>(lights.cluster_dimensions.xy);

    let z_near_far = compute_z_near_and_z_far(is_orthographic);
    let z_near = z_near_far.x;
    let z_far = z_near_far.y;

    let cluster_position = vec3<u32>(vec2<u32>(floor(varyings.position.xy)), z_slice);
    let cluster_aabb = compute_aabb_for_cluster(
        z_near,
        z_far,
        tile_size,
        screen_size,
        view.view_from_clip,
        is_orthographic,
        lights.cluster_dimensions.xyz,
        cluster_position
    );
    let cluster_aabb_center = (cluster_aabb.max + cluster_aabb.min) * 0.5;
    let cluster_aabb_half_size = (cluster_aabb.max - cluster_aabb.min) * 0.5;

    let object_intersects_cluster_aabb = sphere_intersects_aabb(
        varyings.sphere_position,
        varyings.sphere_radius,
        cluster_aabb_center,
        cluster_aabb_half_size
    );
    if (!object_intersects_cluster_aabb) {
        return vec4<f32>(0.0);
    }

    if (object_type == CLUSTERABLE_OBJECT_TYPE_SPOT_LIGHT && cull_spot_light(
        object_index,
        cluster_aabb_center,
        length(cluster_aabb_half_size),
        varyings.sphere_position,
        varyings.sphere_radius
    )) {
        return vec4<f32>(0.0);
    }

    let cluster_index =
        clustered_forward::fragment_cluster_index(cluster_position, lights.cluster_dimensions);

#ifdef POPULATE_PASS
    let output_index = allocate_list_entry(cluster_index, object_type);
    if (output_index < arrayLength(&index_lists.data)) {
        index_lists.data[output_index] = object_index;
    }
#else
    increment_object_count(cluster_index, object_type);
#endif

    return vec4<f32>(0.0);
}

#endif

fn sphere_intersects_aabb(
    sphere_center: vec3<f32>,
    sphere_radius: f32,
    aabb_center: vec3<f32>,
    aabb_half_size: vec3<f32>
) -> bool {
    let delta = max(vec3(0.0), abs(aabb_center - sphere_center) - aabb_half_size);
    let dist_sq = dot(delta, delta);
    return dist_sq <= sphere_radius * sphere_radius;
}

fn compute_aabb_for_cluster(
    z_near: f32,
    z_far: f32,
    tile_size: vec2<f32>,
    screen_size: vec2<f32>,
    view_from_clip: mat4x4<f32>,
    is_orthographic: bool,
    cluster_dimensions: vec3<u32>,
    ijk_u: vec3<u32>
) -> Aabb {
    let ijk = vec3<f32>(ijk_u);

    let p_min = ijk.xy * tile_size;
    let p_max = p_min + tile_size;

    var cluster_min: vec3<f32>;
    var cluster_max: vec3<f32>;
    if (is_orthographic) {

        var p_min = screen_to_view(screen_size, view_from_clip, p_min, 0.0).xyz;
        var p_max = screen_to_view(screen_size, view_from_clip, p_max, 0.0).xyz;

        p_min.z = -z_near + (z_near - z_far) * ijk.z / f32(cluster_dimensions.z);
        p_max.z = -z_near + (z_near - z_far) * (ijk.z + 1.0) / f32(cluster_dimensions.z);

        cluster_min = min(p_min, p_max);
        cluster_max = max(p_min, p_max);
    } else {

        let p_min = screen_to_view(screen_size, view_from_clip, p_min, 1.0);
        let p_max = screen_to_view(screen_size, view_from_clip, p_max, 1.0);

        let z_far_over_z_near = -z_far / -z_near;
        var cluster_near = 0.0;
        if (ijk.z != 0.0) {
            cluster_near = -z_near *
                pow(z_far_over_z_near, (ijk.z - 1.0) / f32(cluster_dimensions.z - 1u));
        }

        var cluster_far: f32;
        if (cluster_dimensions.z == 1u) {
            cluster_far = -z_far;
        } else {
            cluster_far = -z_near * pow(z_far_over_z_near, ijk.z / f32(cluster_dimensions.z - 1u));
        }

        let p_min_near = line_intersection_to_z_plane(vec3(0.0), p_min.xyz, cluster_near);
        let p_min_far = line_intersection_to_z_plane(vec3(0.0), p_min.xyz, cluster_far);
        let p_max_near = line_intersection_to_z_plane(vec3(0.0), p_max.xyz, cluster_near);
        let p_max_far = line_intersection_to_z_plane(vec3(0.0), p_max.xyz, cluster_far);

        cluster_min = min(min(p_min_near, p_min_far), min(p_max_near, p_max_far));
        cluster_max = max(max(p_min_near, p_min_far), max(p_max_near, p_max_far));
    }

    return Aabb(cluster_min, cluster_max);
}

fn screen_to_view(
    screen_size: vec2<f32>,
    view_from_clip: mat4x4<f32>,
    screen: vec2<f32>,
    ndc_z: f32
) -> vec4<f32> {
    let tex_coord = screen / screen_size;
    let clip = vec4(
        tex_coord.x * 2.0 - 1.0,
        (1.0 - tex_coord.y) * 2.0 - 1.0,
        ndc_z,
        1.0
    );
    return clip_to_view(view_from_clip, clip);
}

fn clip_to_view(view_from_clip: mat4x4<f32>, clip: vec4<f32>) -> vec4<f32> {
    let view = view_from_clip * clip;
    return view / view.w;
}

fn line_intersection_to_z_plane(origin: vec3<f32>, p: vec3<f32>, z: f32) -> vec3<f32> {
    let v = p - origin;
    let t = (z - dot(vec3(0.0, 0.0, 1.0), origin)) / dot(vec3(0.0, 0.0, 1.0), v);
    return origin + t * v;
}

fn compute_z_near_and_z_far(is_orthographic: bool) -> vec2<f32> {
    let cluster_dimensions = vec3<f32>(lights.cluster_dimensions.xyz);
    let cluster_factors = lights.cluster_factors;
    var z_near: f32;
    var z_far: f32;
    if (is_orthographic) {
        z_near = -cluster_factors.z;
        z_far = -(cluster_dimensions.z + cluster_factors.w * cluster_factors.z) / cluster_factors.w;
    } else {
        z_near = exp(cluster_factors.w / cluster_factors.z);
        z_far = exp((cluster_dimensions.z + cluster_factors.w - 1.0) / cluster_factors.z);
    }
    return vec2<f32>(z_near, z_far);
}

fn cull_spot_light(
    object_index: u32,
    cluster_aabb_sphere_center: vec3<f32>,
    cluster_aabb_sphere_radius: f32,
    sphere_position: vec3<f32>,
    sphere_radius: f32
) -> bool {
    let light_custom_data = clustered_lights.data[object_index].light_custom_data;
    let light_flags = clustered_lights.data[object_index].flags;
    let light_tan_angle = clustered_lights.data[object_index].spot_light_tan_angle;

    let world_light_direction_rev_xz = light_custom_data.xy;
    let world_light_direction_rev_y_sign = select(
        1.0,
        -1.0,
        (light_flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u
    );
    let world_light_direction_rev = vec3(
        world_light_direction_rev_xz.x,
        world_light_direction_rev_y_sign * sqrt(
            1.0 -
                world_light_direction_rev_xz.x * world_light_direction_rev_xz.x -
                world_light_direction_rev_xz.y * world_light_direction_rev_xz.y
        ),
        world_light_direction_rev_xz.y
    );
    let world_light_direction = -world_light_direction_rev;
    let view_light_direction = normalize((view.view_from_world *
        vec4(world_light_direction, 0.0)).xyz);

    let angle_cos = cos_atan(light_tan_angle);
    let angle_sin = sin_atan(light_tan_angle);

    let spot_light_offset = sphere_position - cluster_aabb_sphere_center;
    let spot_light_dist_sq = dot(spot_light_offset, spot_light_offset);
    let v1_len = dot(spot_light_offset, view_light_direction);

    let distance_closest_point = angle_cos * sqrt(spot_light_dist_sq - v1_len * v1_len) -
        v1_len * angle_sin;
    let angle_cull = distance_closest_point > cluster_aabb_sphere_radius;

    let front_cull = v1_len > cluster_aabb_sphere_radius + sphere_radius;
    let back_cull = v1_len < -cluster_aabb_sphere_radius;

    return angle_cull || front_cull || back_cull;
}

fn cos_atan(tan_theta: f32) -> f32 {
    return inverseSqrt(1.0 + tan_theta * tan_theta);
}

fn sin_atan(tan_theta: f32) -> f32 {
    return tan_theta * inverseSqrt(1.0 + tan_theta * tan_theta);
}

#ifndef VERTEX_SHADER
#ifdef POPULATE_PASS

fn allocate_list_entry(cluster_index: u32, object_type: u32) -> u32 {
    switch (object_type) {
        case CLUSTERABLE_OBJECT_TYPE_POINT_LIGHT: {
            return offsets_and_counts.data[cluster_index][0u].x +
                atomicAdd(&scratchpad_offsets_and_counts.data[cluster_index].point_lights, 1u);
        }
        case CLUSTERABLE_OBJECT_TYPE_SPOT_LIGHT: {
            return offsets_and_counts.data[cluster_index][0u].x +
                offsets_and_counts.data[cluster_index][0u].y +
                atomicAdd(&scratchpad_offsets_and_counts.data[cluster_index].spot_lights, 1u);
        }
        case CLUSTERABLE_OBJECT_TYPE_REFLECTION_PROBE: {
            return offsets_and_counts.data[cluster_index][0u].x +
                offsets_and_counts.data[cluster_index][0u].y +
                offsets_and_counts.data[cluster_index][0u].z +
                atomicAdd(&scratchpad_offsets_and_counts.data[cluster_index].reflection_probes, 1u);
        }
        case CLUSTERABLE_OBJECT_TYPE_IRRADIANCE_VOLUME: {
            return offsets_and_counts.data[cluster_index][0u].x +
                offsets_and_counts.data[cluster_index][0u].y +
                offsets_and_counts.data[cluster_index][0u].z +
                offsets_and_counts.data[cluster_index][0u].w +
                atomicAdd(
                    &scratchpad_offsets_and_counts.data[cluster_index].irradiance_volumes,
                    1u
                );
        }
        case CLUSTERABLE_OBJECT_TYPE_DECAL: {
            return offsets_and_counts.data[cluster_index][0u].x +
                offsets_and_counts.data[cluster_index][0u].y +
                offsets_and_counts.data[cluster_index][0u].z +
                offsets_and_counts.data[cluster_index][0u].w +
                offsets_and_counts.data[cluster_index][1u].x +
                atomicAdd(&scratchpad_offsets_and_counts.data[cluster_index].decals, 1u);
        }
        default: {}
    }
    return 0xffffffffu;
}
#else

fn increment_object_count(cluster_index: u32, object_type: u32) {
    switch (object_type) {
        case CLUSTERABLE_OBJECT_TYPE_POINT_LIGHT: {
            atomicAdd(&offsets_and_counts.data[cluster_index].point_lights, 1u);
        }
        case CLUSTERABLE_OBJECT_TYPE_SPOT_LIGHT: {
            atomicAdd(&offsets_and_counts.data[cluster_index].spot_lights, 1u);
        }
        case CLUSTERABLE_OBJECT_TYPE_REFLECTION_PROBE: {
            atomicAdd(&offsets_and_counts.data[cluster_index].reflection_probes, 1u);
        }
        case CLUSTERABLE_OBJECT_TYPE_IRRADIANCE_VOLUME: {
            atomicAdd(&offsets_and_counts.data[cluster_index].irradiance_volumes, 1u);
        }
        case CLUSTERABLE_OBJECT_TYPE_DECAL: {
            atomicAdd(&offsets_and_counts.data[cluster_index].decals, 1u);
        }
        default: {}
    }
}
#endif
#endif

fn get_object_bounding_sphere(object_index: u32, object_type: u32) -> vec4<f32> {
    var position = vec3<f32>(0.0);
    var radius = 0.0;
    switch (object_type) {
        case CLUSTERABLE_OBJECT_TYPE_POINT_LIGHT: {
            position = clustered_lights.data[object_index].position_radius.xyz;
            radius = clustered_lights.data[object_index].range;
        }
        case CLUSTERABLE_OBJECT_TYPE_SPOT_LIGHT: {
            position = clustered_lights.data[object_index].position_radius.xyz;
            radius = clustered_lights.data[object_index].range;
        }
        case CLUSTERABLE_OBJECT_TYPE_REFLECTION_PROBE: {
            position = light_probes.reflection_probes[object_index].world_position;
            radius = light_probes.reflection_probes[object_index].bounding_sphere_radius;
        }
        case CLUSTERABLE_OBJECT_TYPE_IRRADIANCE_VOLUME: {
            position = light_probes.irradiance_volumes[object_index].world_position;
            radius = light_probes.irradiance_volumes[object_index].bounding_sphere_radius;
        }
        case CLUSTERABLE_OBJECT_TYPE_DECAL: {
            position = clustered_decals.decals[object_index].world_position;
            radius = clustered_decals.decals[object_index].bounding_sphere_radius;
        }
        default: {}
    }
    return vec4(position, radius);
}
