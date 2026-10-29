#define_import_path bevy_pbr::mesh_types

struct Mesh {

    world_from_local: mat3x4<f32>,
    previous_world_from_local: mat3x4<f32>,

    local_from_world_transpose_a: mat2x4<f32>,
    local_from_world_transpose_b: f32,

    flags: u32,
    lightmap_uv_rect: vec2<u32>,

    first_vertex_index: u32,
    current_skin_index: u32,

    material_and_lightmap_bind_group_slot: u32,

    tag: u32,
    morph_descriptor_index: u32,
};

#ifdef SKINNED
struct SkinnedMesh {
    data: array<mat4x4<f32>, 256u>,
};
#endif

#ifdef MORPH_TARGETS

struct MorphWeights {
    weights: array<vec4<f32>, 64u>,
};

struct MorphDescriptor {

    current_weights_offset: u32,

    prev_weights_offset: u32,

    targets_offset: u32,

    vertex_count: u32,

    weight_count: u32,
};

struct MorphAttributes {

    position: vec3<f32>,

    pad_a: f32,

    normal: vec3<f32>,

    pad_b: f32,

    tangent: vec3<f32>,

    pad_c: f32,
};

#endif

const MESH_FLAGS_VISIBILITY_RANGE_INDEX_BITS: u32     = (1u << 16u) - 1u;
const MESH_FLAGS_AABB_BASED_VISIBILITY_RANGE_BIT: u32 = 1u << 27u;
const MESH_FLAGS_NO_FRUSTUM_CULLING_BIT: u32          = 1u << 28u;
const MESH_FLAGS_SHADOW_RECEIVER_BIT: u32             = 1u << 29u;
const MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT: u32 = 1u << 30u;

const MESH_FLAGS_SIGN_DETERMINANT_MODEL_3X3_BIT: u32  = 1u << 31u;
