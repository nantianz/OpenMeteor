

#import bevy_pbr::mesh_preprocess_types::IndirectBatchSet

@group(0) @binding(0) var<storage, read_write> indirect_batch_sets: array<IndirectBatchSet>;

@compute
@workgroup_size(64)
fn main(@builtin(global_invocation_id) global_invocation_id: vec3<u32>) {

    let instance_index = global_invocation_id.x;
    if (instance_index >= arrayLength(&indirect_batch_sets)) {
        return;
    }

    atomicStore(&indirect_batch_sets[instance_index].indirect_parameters_count, 0u);
}
