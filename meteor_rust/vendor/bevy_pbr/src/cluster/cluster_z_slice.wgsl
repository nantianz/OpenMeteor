#import bevy_pbr::cluster::{
    CLUSTERABLE_OBJECT_TYPE_DECAL, CLUSTERABLE_OBJECT_TYPE_IRRADIANCE_VOLUME,
    CLUSTERABLE_OBJECT_TYPE_POINT_LIGHT, CLUSTERABLE_OBJECT_TYPE_REFLECTION_PROBE,
    CLUSTERABLE_OBJECT_TYPE_SPOT_LIGHT, ClusterMetadata, ClusterableObjectZSlice,
    calculate_sphere_cluster_bounds, compute_view_from_world_scale
}
#import bevy_pbr::mesh_view_types::{
    ClusteredDecals, ClusteredLights, LightProbes, Lights, POINT_LIGHT_FLAGS_SPOT_LIGHT_BIT
}
#import bevy_render::view::View

@group(0) @binding(0) var<storage, read_write> cluster_metadata: ClusterMetadata;

@group(0) @binding(1) var<storage, read_write> z_slices: array<ClusterableObjectZSlice>;

@group(0) @binding(2) var<storage> clustered_lights: ClusteredLights;

@group(0) @binding(3) var<uniform> light_probes: LightProbes;

@group(0) @binding(4) var<storage> clustered_decals: ClusteredDecals;

@group(0) @binding(5) var<uniform> lights: Lights;

@group(0) @binding(6) var<uniform> view: View;

var<workgroup> shared_farthest_z: array<f32, 64>;

@compute @workgroup_size(64, 1, 1)
fn z_slice_main(
    @builtin(global_invocation_id) global_invocation_id: vec3<u32>,
    @builtin(local_invocation_id) local_invocation_id: vec3<u32>
) {
    let id = global_invocation_id.x;
    let local_id = local_invocation_id.x;

    var object_index: u32 = 0u;
    var object_type: u32 = 0xffffffffu;
    var position: vec3<f32> = vec3<f32>(0.0);
    var radius: f32 = 0.0;

    let last_clustered_light_id = cluster_metadata.clustered_light_count;
    let last_reflection_probe_id = last_clustered_light_id +
        cluster_metadata.reflection_probe_count;
    let last_irradiance_volume_id = last_reflection_probe_id +
        cluster_metadata.irradiance_volume_count;
    let last_decal_id = last_irradiance_volume_id + cluster_metadata.decal_count;

    if (id < last_clustered_light_id) {

        object_index = id;
        let flags = clustered_lights.data[object_index].flags;
        object_type = select(
            CLUSTERABLE_OBJECT_TYPE_POINT_LIGHT,
            CLUSTERABLE_OBJECT_TYPE_SPOT_LIGHT,
            (flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_BIT) != 0u
        );
        position = clustered_lights.data[object_index].position_radius.xyz;
        radius = clustered_lights.data[object_index].range;
    } else if (id < last_reflection_probe_id) {

        object_index = id - last_clustered_light_id;
        object_type = CLUSTERABLE_OBJECT_TYPE_REFLECTION_PROBE;
        position = light_probes.reflection_probes[object_index].world_position;
        radius = light_probes.reflection_probes[object_index].bounding_sphere_radius;
    } else if (id < last_irradiance_volume_id) {

        object_index = id - last_reflection_probe_id;
        object_type = CLUSTERABLE_OBJECT_TYPE_IRRADIANCE_VOLUME;
        position = light_probes.irradiance_volumes[object_index].world_position;
        radius = light_probes.irradiance_volumes[object_index].bounding_sphere_radius;
    } else if (id < last_decal_id) {

        object_index = id - last_irradiance_volume_id;
        object_type = CLUSTERABLE_OBJECT_TYPE_DECAL;
        position = clustered_decals.decals[object_index].world_position;
        radius = clustered_decals.decals[object_index].bounding_sphere_radius;
    }

    let view_from_world_scale = compute_view_from_world_scale(view.world_from_view);
    let is_orthographic = view.clip_from_view[3].w == 1.0;

    accumulate_farthest_z_value(local_id, position, radius, view_from_world_scale, is_orthographic);

    if (object_type == 0xffffffffu) {
        return;
    }

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

    for (var z_slice = cluster_bounds.min.z; z_slice <= cluster_bounds.max.z; z_slice += 1u) {
        try_write_z_slice(object_index, object_type, z_slice);
    }
}

fn try_write_z_slice(object_index: u32, object_type: u32, z_slice: u32) {
    let z_slice_offset = atomicAdd(&cluster_metadata.indirect_draw_params.instance_count, 1u);
    if (z_slice_offset >= cluster_metadata.z_slice_list_capacity) {
        return;
    }

    z_slices[z_slice_offset].object_index = object_index;
    z_slices[z_slice_offset].object_type = object_type;
    z_slices[z_slice_offset].z_slice = z_slice;
}

fn accumulate_farthest_z_value(
    local_id: u32,
    position: vec3<f32>,
    radius: f32,
    view_from_world_scale: vec3<f32>,
    is_orthographic: bool
) {

    let view_from_world_row_2 = transpose(view.view_from_world)[2];
    let far_z = dot(-view_from_world_row_2, vec4(position, 1.0)) + radius * view_from_world_scale.z;
    shared_farthest_z[local_id] = far_z;
    workgroupBarrier();

    for (var stride = 32u; stride > 0u; stride /= 2u) {
        if (local_id < stride) {
            shared_farthest_z[local_id] = max(
                shared_farthest_z[local_id],
                shared_farthest_z[local_id + stride]
            );
        }
        workgroupBarrier();
    }

    if (local_id != 0u) {
        return;
    }

    atomicMax(&cluster_metadata.farthest_z, f32_bits_to_sortable_u32(bitcast<u32>(shared_farthest_z[0u])));
}

fn f32_bits_to_sortable_u32(bits: u32) -> u32 {
    let mask = bitcast<u32>(bitcast<i32>(bits) >> 31) | 0x80000000u;
    return bits ^ mask;
}
