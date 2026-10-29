

#define_import_path bevy_pbr::mesh_preprocess_types

struct MeshInput {

    world_from_local: mat3x4<f32>,

    lightmap_uv_rect: vec2<u32>,

    flags: u32,
    previous_input_index: u32,
    first_vertex_index: u32,
    first_index_index: u32,
    index_count: u32,
    current_skin_index: u32,

    material_and_lightmap_bind_group_slot: u32,
    timestamp: u32,

    tag: u32,

    morph_descriptor_index: u32,
}

struct IndirectParametersIndexed {

    index_count: u32,

    instance_count: u32,

    first_index: u32,

    base_vertex: u32,

    first_instance: u32,
}

struct IndirectParametersNonIndexed {
    vertex_count: u32,
    instance_count: u32,
    base_vertex: u32,
    first_instance: u32,
}

struct IndirectParametersCpuMetadata {
    base_output_index: u32,
    batch_set_index: u32,
}

struct IndirectParametersGpuMetadata {
    mesh_index: u32,
#ifdef WRITE_INDIRECT_PARAMETERS_METADATA
    early_instance_count: atomic<u32>,
    late_instance_count: atomic<u32>,
#else
    early_instance_count: u32,
    late_instance_count: u32,
#endif
}

struct IndirectBatchSet {
    indirect_parameters_count: atomic<u32>,
    indirect_parameters_base: u32,
}

struct PreprocessWorkItem {

    input_index: u32,

    output_or_indirect_parameters_index: u32,
}
