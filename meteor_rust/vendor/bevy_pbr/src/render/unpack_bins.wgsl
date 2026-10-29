

#import bevy_pbr::mesh_preprocess_types::PreprocessWorkItem

struct BinUnpackingMetadata {

    base_output_work_item_index: u32,

    base_indirect_parameters_index: u32,

    binned_mesh_instance_count: u32,

    pad_a: u32,

    pad_b: array<vec4<u32>, 15>,
};

struct BinnedMeshInstance {

    input_uniform_index: u32,

    bin_index: u32,
};

@group(0) @binding(0) var<uniform> bin_unpacking_metadata: BinUnpackingMetadata;

@group(0) @binding(1) var<storage> binned_mesh_instances: array<BinnedMeshInstance>;

@group(0) @binding(2) var<storage, read_write> preprocess_work_items: array<PreprocessWorkItem>;

@group(0) @binding(3) var<storage> bin_index_to_indirect_parameters_offset: array<u32>;

@compute
@workgroup_size(64)
fn main(@builtin(global_invocation_id) global_invocation_id: vec3<u32>) {

    let global_id = global_invocation_id.x;
    if (global_id >= bin_unpacking_metadata.binned_mesh_instance_count) {
        return;
    }

    let input_uniform_index = binned_mesh_instances[global_id].input_uniform_index;
    let bin_index = binned_mesh_instances[global_id].bin_index;

    let indirect_parameters_offset = bin_index_to_indirect_parameters_offset[bin_index];

    let output_index = bin_unpacking_metadata.base_output_work_item_index + global_id;

    preprocess_work_items[output_index].input_index = input_uniform_index;
    preprocess_work_items[output_index].output_or_indirect_parameters_index =
        bin_unpacking_metadata.base_indirect_parameters_index + indirect_parameters_offset;
}
