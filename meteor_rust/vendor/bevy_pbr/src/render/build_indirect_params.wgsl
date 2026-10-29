

#import bevy_pbr::mesh_preprocess_types::{
    IndirectBatchSet,
    IndirectParametersIndexed,
    IndirectParametersNonIndexed,
    IndirectParametersCpuMetadata,
    IndirectParametersGpuMetadata,
    MeshInput
}

@group(0) @binding(0) var<storage> current_input: array<MeshInput>;

@group(0) @binding(1) var<storage> indirect_parameters_cpu_metadata:
    array<IndirectParametersCpuMetadata>;

@group(0) @binding(2) var<storage> indirect_parameters_gpu_metadata:
    array<IndirectParametersGpuMetadata>;

@group(0) @binding(3) var<storage, read_write> indirect_batch_sets: array<IndirectBatchSet>;

#ifdef INDEXED

@group(0) @binding(4) var<storage, read_write> indirect_parameters:
    array<IndirectParametersIndexed>;
#else

@group(0) @binding(4) var<storage, read_write> indirect_parameters:
    array<IndirectParametersNonIndexed>;
#endif

@compute
@workgroup_size(64)
fn main(@builtin(global_invocation_id) global_invocation_id: vec3<u32>) {

    let instance_index = global_invocation_id.x;
    if (instance_index >= arrayLength(&indirect_parameters_cpu_metadata)) {
        return;
    }

    let base_output_index = indirect_parameters_cpu_metadata[instance_index].base_output_index;
    let batch_set_index = indirect_parameters_cpu_metadata[instance_index].batch_set_index;
    let mesh_index = indirect_parameters_gpu_metadata[instance_index].mesh_index;

    let early_instance_count =
        indirect_parameters_gpu_metadata[instance_index].early_instance_count;
    let late_instance_count = indirect_parameters_gpu_metadata[instance_index].late_instance_count;

#ifdef EARLY_PHASE
    let instance_count = early_instance_count;
#else
#ifdef LATE_PHASE
    let instance_count = late_instance_count;
#else
    let instance_count = early_instance_count + late_instance_count;
#endif
#endif

    var indirect_parameters_index = instance_index;

#ifdef MULTI_DRAW_INDIRECT_COUNT_SUPPORTED

    if (batch_set_index != 0xffffffffu) {

        if (instance_count == 0u) {
            return;
        }

        let indirect_parameters_base =
            indirect_batch_sets[batch_set_index].indirect_parameters_base;
        let indirect_parameters_offset =
            atomicAdd(&indirect_batch_sets[batch_set_index].indirect_parameters_count, 1u);

        indirect_parameters_index = indirect_parameters_base + indirect_parameters_offset;
    }
#endif

    indirect_parameters[indirect_parameters_index].instance_count = instance_count;

#ifdef LATE_PHASE

    indirect_parameters[indirect_parameters_index].first_instance =
        base_output_index + early_instance_count;
#else
    indirect_parameters[indirect_parameters_index].first_instance = base_output_index;
#endif

    indirect_parameters[indirect_parameters_index].base_vertex =
        current_input[mesh_index].first_vertex_index;

#ifdef INDEXED
    indirect_parameters[indirect_parameters_index].index_count =
        current_input[mesh_index].index_count;
    indirect_parameters[indirect_parameters_index].first_index =
        current_input[mesh_index].first_index_index;
#else
    indirect_parameters[indirect_parameters_index].vertex_count =
        current_input[mesh_index].index_count;
#endif
}
