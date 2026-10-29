#import bevy_pbr::cluster::ClusterMetadata
#import bevy_pbr::mesh_view_types::{ClusterOffsetsAndCounts, Lights}

@group(0) @binding(0) var<storage, read_write> offsets_and_counts: ClusterOffsetsAndCounts;
@group(0) @binding(1) var<uniform> lights: Lights;
@group(0) @binding(2) var<storage, read_write> clustering_metadata: ClusterMetadata;
@group(0) @binding(3) var<storage, read_write> scratchpad_offsets_and_counts:
    ClusterOffsetsAndCounts;

var<workgroup> block_offsets: array<u32, 256>;

@compute @workgroup_size(256, 1, 1)
fn allocate_local_main(
    @builtin(local_invocation_id) local_id: vec3<u32>,
    @builtin(workgroup_id) group_id: vec3<u32>,
    @builtin(global_invocation_id) global_id: vec3<u32>
) {
    let cluster_count =
        lights.cluster_dimensions.x *
        lights.cluster_dimensions.y *
        lights.cluster_dimensions.z;

    let block_start = group_id.x * 256u;
    let block_end = min(block_start + 256u, cluster_count);

    block_offsets[local_id.x] = 0u;
    workgroupBarrier();

    if (global_id.x < block_end && local_id.x < 255u) {
        block_offsets[local_id.x + 1] = cluster_object_count(global_id.x);
    }
    workgroupBarrier();

    for (var offset = 1u; offset < 256u; offset *= 2u) {
        var term = 0u;
        if (local_id.x >= offset) {
            term = block_offsets[local_id.x - offset];
        }
        workgroupBarrier();
        block_offsets[local_id.x] += term;
        workgroupBarrier();
    }

    if (global_id.x < block_end) {

        offsets_and_counts.data[global_id.x][0].x = block_offsets[local_id.x];

        scratchpad_offsets_and_counts.data[global_id.x][0u] = vec4(0u);
        scratchpad_offsets_and_counts.data[global_id.x][1u] = vec4(0u);
    }
}

@compute @workgroup_size(256, 1, 1)
fn allocate_global_main(@builtin(local_invocation_id) local_id: vec3<u32>) {
    let cluster_count =
        lights.cluster_dimensions.x *
        lights.cluster_dimensions.y *
        lights.cluster_dimensions.z;

    var current_offset = 0u;
    for (var i = 0u; i < cluster_count; i += 256u) {
        if (i + local_id.x < cluster_count) {
            offsets_and_counts.data[i + local_id.x][0].x += current_offset;
        }
        storageBarrier();

        if (i + 255u < cluster_count) {
            current_offset = offsets_and_counts.data[i + 255u][0].x +
                cluster_object_count(i + 255u);
        }
    }
    storageBarrier();

    if (local_id.x == 0u) {
        clustering_metadata.index_list_size = offsets_and_counts.data[cluster_count - 1][0].x +
            cluster_object_count(cluster_count - 1);
    }
}

fn cluster_object_count(cluster_index: u32) -> u32 {
    return
        offsets_and_counts.data[cluster_index][0].y +
        offsets_and_counts.data[cluster_index][0].z +
        offsets_and_counts.data[cluster_index][0].w +
        offsets_and_counts.data[cluster_index][1].x +
        offsets_and_counts.data[cluster_index][1].y;
}
