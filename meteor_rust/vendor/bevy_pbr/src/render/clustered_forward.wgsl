#define_import_path bevy_pbr::clustered_forward

#import bevy_pbr::{
    mesh_view_bindings as bindings,
    utils::rand_f,
}

#import bevy_render::{
   color_operations::hsv_to_rgb,
   maths::PI_2,
}

struct ClusterableObjectIndexRanges {

    first_point_light_index_offset: u32,

    first_spot_light_index_offset: u32,

    first_reflection_probe_index_offset: u32,

    first_irradiance_volume_index_offset: u32,
    first_decal_offset: u32,

    last_clusterable_object_index_offset: u32,
}

fn view_z_to_z_slice(
    cluster_factors: vec2<f32>,
    z_slices: u32,
    view_z: f32,
    is_orthographic: bool
) -> u32 {
    var z_slice: u32 = 0u;
    if is_orthographic {

        z_slice = u32(floor((view_z - cluster_factors.x) * cluster_factors.y));
    } else {

        z_slice = u32(log(-view_z) * cluster_factors.x - cluster_factors.y + 1.0);
    }

    return min(z_slice, z_slices - 1u);
}

fn view_fragment_cluster_index(frag_coord: vec2<f32>, view_z: f32, is_orthographic: bool) -> u32 {
    let xy = vec2<u32>(floor((frag_coord - bindings::view.viewport.xy) * bindings::lights.cluster_factors.xy));
    let z_slice = view_z_to_z_slice(
        bindings::lights.cluster_factors.zw,
        bindings::lights.cluster_dimensions.z,
        view_z,
        is_orthographic
    );
    return fragment_cluster_index(vec3(xy, z_slice), bindings::lights.cluster_dimensions);
}

fn fragment_cluster_index(p: vec3<u32>, cluster_dimensions: vec4<u32>) -> u32 {

    return min(
        (p.y * cluster_dimensions.x + p.x) * cluster_dimensions.z + p.z,
        cluster_dimensions.w - 1u
    );
}

const CLUSTER_COUNT_SIZE = 9u;

fn unpack_clusterable_object_index_ranges(cluster_index: u32) -> ClusterableObjectIndexRanges {
#if AVAILABLE_STORAGE_BUFFER_BINDINGS >= 3

    let offset_and_counts_a = bindings::cluster_offsets_and_counts.data[cluster_index][0];
    let offset_and_counts_b = bindings::cluster_offsets_and_counts.data[cluster_index][1];

    let point_light_offset = offset_and_counts_a.x;
    let spot_light_offset = point_light_offset + offset_and_counts_a.y;
    let reflection_probe_offset = spot_light_offset + offset_and_counts_a.z;
    let irradiance_volume_offset = reflection_probe_offset + offset_and_counts_a.w;
    let decal_offset = irradiance_volume_offset + offset_and_counts_b.x;
    let last_clusterable_offset = decal_offset + offset_and_counts_b.y;
    return ClusterableObjectIndexRanges(
        point_light_offset,
        spot_light_offset,
        reflection_probe_offset,
        irradiance_volume_offset,
        decal_offset,
        last_clusterable_offset
    );

#else

    let raw_offset_and_counts = bindings::cluster_offsets_and_counts.data[cluster_index >> 2u][cluster_index & ((1u << 2u) - 1u)];

    let offset_and_counts = vec3<u32>(
        (raw_offset_and_counts >> (CLUSTER_COUNT_SIZE * 2u)) & ((1u << (32u - (CLUSTER_COUNT_SIZE * 2u))) - 1u),
        (raw_offset_and_counts >> CLUSTER_COUNT_SIZE)        & ((1u << CLUSTER_COUNT_SIZE) - 1u),
        raw_offset_and_counts                                & ((1u << CLUSTER_COUNT_SIZE) - 1u),
    );

    let offset_a = offset_and_counts.x;
    let offset_b = offset_a + offset_and_counts.y;
    let offset_c = offset_b + offset_and_counts.z;

    return ClusterableObjectIndexRanges(offset_a, offset_b, offset_c, offset_c, offset_c, offset_c);

#endif
}

fn get_clusterable_object_id(index: u32) -> u32 {
#if AVAILABLE_STORAGE_BUFFER_BINDINGS >= 3
    return bindings::clusterable_object_index_lists.data[index];
#else

    let indices = bindings::clusterable_object_index_lists.data[index >> 4u][(index >> 2u) &
        ((1u << 2u) - 1u)];

    return (indices >> (8u * (index & ((1u << 2u) - 1u)))) & ((1u << 8u) - 1u);
#endif
}

fn cluster_debug_visualization(
    input_color: vec4<f32>,
    view_z: f32,
    is_orthographic: bool,
    clusterable_object_index_ranges: ClusterableObjectIndexRanges,
    cluster_index: u32,
) -> vec4<f32> {
    var output_color = input_color;

#ifdef CLUSTERED_FORWARD_DEBUG_Z_SLICES

    let cluster_overlay_alpha = 0.1;
    var z_slice: u32 = view_z_to_z_slice(
        bindings::lights.cluster_factors.zw,
        bindings::lights.cluster_dimensions.z,
        view_z,
        is_orthographic
    );

    if (z_slice & 1u) == 1u {
        z_slice = z_slice + bindings::lights.cluster_dimensions.z / 2u;
    }
    let slice_color_hsv = vec3(
        f32(z_slice) / f32(bindings::lights.cluster_dimensions.z + 1u) * PI_2,
        1.0,
        0.5
    );
    let slice_color = hsv_to_rgb(slice_color_hsv);
    output_color = vec4<f32>(
        (1.0 - cluster_overlay_alpha) * output_color.rgb + cluster_overlay_alpha * slice_color,
        output_color.a
    );
#endif
#ifdef CLUSTERED_FORWARD_DEBUG_CLUSTER_COMPLEXITY

    let cluster_overlay_alpha = 0.1;
    let max_complexity_per_cluster = 64.0;
    let object_count = clusterable_object_index_ranges.first_reflection_probe_index_offset -
        clusterable_object_index_ranges.first_point_light_index_offset;
    output_color.r = (1.0 - cluster_overlay_alpha) * output_color.r + cluster_overlay_alpha *
        smoothstep(0.0, max_complexity_per_cluster, f32(object_count));
    output_color.g = (1.0 - cluster_overlay_alpha) * output_color.g + cluster_overlay_alpha *
        (1.0 - smoothstep(0.0, max_complexity_per_cluster, f32(object_count)));
#endif
#ifdef CLUSTERED_FORWARD_DEBUG_CLUSTER_COHERENCY

    let cluster_overlay_alpha = 0.1;
    var rng = cluster_index;
    let cluster_color_hsv = vec3(rand_f(&rng) * PI_2, 1.0, 0.5);
    let cluster_color = hsv_to_rgb(cluster_color_hsv);
    output_color = vec4<f32>(
        (1.0 - cluster_overlay_alpha) * output_color.rgb + cluster_overlay_alpha * cluster_color,
        output_color.a
    );
#endif

    return output_color;
}
