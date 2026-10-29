

#import bevy_pbr::mesh_preprocess_types::{
    IndirectParametersCpuMetadata, IndirectParametersGpuMetadata, MeshInput, PreprocessWorkItem
}
#import bevy_pbr::mesh_types::{
    Mesh, MESH_FLAGS_AABB_BASED_VISIBILITY_RANGE_BIT, MESH_FLAGS_NO_FRUSTUM_CULLING_BIT,
    MESH_FLAGS_VISIBILITY_RANGE_INDEX_BITS
}
#import bevy_pbr::mesh_view_bindings::view
#import bevy_pbr::occlusion_culling
#import bevy_pbr::prepass_bindings::previous_view_uniforms
#import bevy_pbr::view_transformations::{
    position_world_to_ndc, position_world_to_view, ndc_to_uv, view_z_to_depth_ndc,
    position_world_to_prev_ndc, position_world_to_prev_view, prev_view_z_to_depth_ndc
}
#import bevy_render::maths
#import bevy_render::view::View

struct MeshCullingData {

    aabb_center: vec4<f32>,

    aabb_half_extents: vec4<f32>,
}

struct LatePreprocessWorkItemIndirectParameters {

    dispatch_x: atomic<u32>,

    dispatch_y: u32,

    dispatch_z: u32,

    work_item_count: atomic<u32>,

    pad: vec4<u32>,
}

struct Immediates {

    late_preprocess_work_item_indirect_offset: u32,
}

@group(0) @binding(3) var<storage> current_input: array<MeshInput>;

@group(0) @binding(4) var<storage> previous_input: array<MeshInput>;

@group(0) @binding(5) var<storage> work_items: array<PreprocessWorkItem>;

@group(0) @binding(6) var<storage, read_write> output: array<Mesh>;

#ifdef INDIRECT

@group(0) @binding(7) var<storage> indirect_parameters_cpu_metadata:
    array<IndirectParametersCpuMetadata>;

@group(0) @binding(8) var<storage, read_write> indirect_parameters_gpu_metadata:
    array<IndirectParametersGpuMetadata>;
#endif

#ifdef FRUSTUM_CULLING

@group(0) @binding(9) var<storage> mesh_culling_data: array<MeshCullingData>;

@group(0) @binding(10) var<storage> visibility_ranges: array<vec4<f32>>;
#endif

#ifdef OCCLUSION_CULLING
@group(0) @binding(11) var depth_pyramid: texture_2d<f32>;

#ifdef EARLY_PHASE
@group(0) @binding(12) var<storage, read_write> late_preprocess_work_items:
    array<PreprocessWorkItem>;

@group(0) @binding(13) var<storage, read_write> late_preprocess_work_item_indirect_parameters:
    array<LatePreprocessWorkItemIndirectParameters>;
#endif

#ifdef LATE_PHASE
@group(0) @binding(13) var<storage, read> late_preprocess_work_item_indirect_parameters:
    array<LatePreprocessWorkItemIndirectParameters>;
#endif

var<immediate> immediates: Immediates;
#endif

#ifdef FRUSTUM_CULLING

fn view_frustum_intersects_obb(
    world_from_local: mat4x4<f32>,
    aabb_center: vec4<f32>,
    aabb_half_extents: vec3<f32>,
) -> bool {

    for (var i = 0; i < 5; i += 1) {

        let plane_normal = view.frustum[i];
        let relative_radius = dot(
            abs(
                vec3(
                    dot(plane_normal.xyz, world_from_local[0].xyz),
                    dot(plane_normal.xyz, world_from_local[1].xyz),
                    dot(plane_normal.xyz, world_from_local[2].xyz),
                )
            ),
            aabb_half_extents
        );

        if (!maths::sphere_intersects_plane_half_space(
                plane_normal, aabb_center, relative_radius)) {
            return false;
        }
    }

    return true;
}
#endif

@compute
@workgroup_size(64)
fn main(@builtin(global_invocation_id) global_invocation_id: vec3<u32>) {

    let instance_index = global_invocation_id.x;

#ifdef LATE_PHASE
    if (instance_index >= atomicLoad(&late_preprocess_work_item_indirect_parameters[
            immediates.late_preprocess_work_item_indirect_offset].work_item_count)) {
        return;
    }
#else
    if (instance_index >= arrayLength(&work_items)) {
        return;
    }
#endif

    let input_index = work_items[instance_index].input_index;
#ifdef INDIRECT
    let indirect_parameters_index = work_items[instance_index].output_or_indirect_parameters_index;

#ifndef LATE_PHASE
    if (instance_index == 0u) || (work_items[instance_index - 1].output_or_indirect_parameters_index != indirect_parameters_index) {
        indirect_parameters_gpu_metadata[indirect_parameters_index].mesh_index = input_index;
    }
#endif

#else
    let mesh_output_index = work_items[instance_index].output_or_indirect_parameters_index;
#endif

    let world_from_local_affine_transpose = current_input[input_index].world_from_local;
    let world_from_local = maths::affine3_to_square(world_from_local_affine_transpose);

#ifdef FRUSTUM_CULLING

    if ((current_input[input_index].flags & MESH_FLAGS_NO_FRUSTUM_CULLING_BIT) == 0u) {
        let aabb_center = mesh_culling_data[input_index].aabb_center.xyz;
        let aabb_half_extents = mesh_culling_data[input_index].aabb_half_extents.xyz;

        let model_center = world_from_local * vec4(aabb_center, 1.0);
        if (!view_frustum_intersects_obb(world_from_local, model_center, aabb_half_extents)) {
            return;
        }
    }

    let visibility_buffer_array_len = arrayLength(&visibility_ranges);
    let visibility_buffer_index =
        current_input[input_index].flags & MESH_FLAGS_VISIBILITY_RANGE_INDEX_BITS;
    if (visibility_buffer_index < visibility_buffer_array_len) {
        let lod_range = visibility_ranges[visibility_buffer_index];

        var world_pos: vec3<f32>;
        if ((current_input[input_index].flags & MESH_FLAGS_AABB_BASED_VISIBILITY_RANGE_BIT) != 0u) {
            let aabb_center = mesh_culling_data[input_index].aabb_center.xyz;
            world_pos = (world_from_local * vec4(aabb_center, 1.0)).xyz;
        } else {
            world_pos = world_from_local[3].xyz;
        }

        let camera_distance = length(world_pos - view.lod_view_world_position);

        if (camera_distance < lod_range.x || camera_distance >= lod_range.w) {
            return;
        }
    }
#endif

    let timestamp = current_input[input_index].timestamp;
    let mesh_changed_this_frame = timestamp == view.frame_count;

    let previous_input_index = current_input[input_index].previous_input_index;
    var previous_world_from_local_affine_transpose: mat3x4<f32>;
    if (mesh_changed_this_frame && previous_input_index != 0xffffffffu) {
        previous_world_from_local_affine_transpose =
            previous_input[previous_input_index].world_from_local;
    } else {
        previous_world_from_local_affine_transpose = world_from_local_affine_transpose;
    }
    let previous_world_from_local =
        maths::affine3_to_square(previous_world_from_local_affine_transpose);

#ifdef OCCLUSION_CULLING
    let aabb_center = mesh_culling_data[input_index].aabb_center.xyz;
    let aabb_half_extents = mesh_culling_data[input_index].aabb_half_extents.xyz;

    let infinity = bitcast<f32>(0x7f800000u);
    let neg_infinity = bitcast<f32>(0xff800000u);
    var aabb = vec4(infinity, infinity, neg_infinity, neg_infinity);
    var max_depth_view = neg_infinity;

    for (var i = 0u; i < 8u; i += 1u) {
        let local_pos = aabb_center + select(
            vec3(-1.0),
            vec3(1.0),
            vec3((i & 1) != 0, (i & 2) != 0, (i & 4) != 0)
        ) * aabb_half_extents;

#ifdef EARLY_PHASE

        let prev_world_pos = (previous_world_from_local * vec4(local_pos, 1.0)).xyz;
        let view_pos = position_world_to_prev_view(prev_world_pos);
        let ndc_pos = position_world_to_prev_ndc(prev_world_pos);
#else

        let world_pos = (world_from_local * vec4(local_pos, 1.0)).xyz;
        let view_pos = position_world_to_view(world_pos);
        let ndc_pos = position_world_to_ndc(world_pos);
#endif

        let uv_pos = ndc_to_uv(ndc_pos.xy);

        aabb = vec4(min(aabb.xy, uv_pos), max(aabb.zw, uv_pos));
        max_depth_view = max(max_depth_view, view_pos.z);
    }

#ifdef EARLY_PHASE
    max_depth_view = min(-previous_view_uniforms.clip_from_view[3][2], max_depth_view);
#else
    max_depth_view = min(-view.clip_from_view[3][2], max_depth_view);
#endif

    let aabb_pixel_size = occlusion_culling::get_aabb_size_in_pixels(aabb, depth_pyramid);
    let occluder_depth_ndc =
        occlusion_culling::get_occluder_depth(aabb, aabb_pixel_size, depth_pyramid);

#ifdef EARLY_PHASE
    let max_depth_ndc = prev_view_z_to_depth_ndc(max_depth_view);
#else
    let max_depth_ndc = view_z_to_depth_ndc(max_depth_view);
#endif

    if (max_depth_ndc < occluder_depth_ndc) {
#ifdef EARLY_PHASE

        let output_work_item_index = atomicAdd(&late_preprocess_work_item_indirect_parameters[
            immediates.late_preprocess_work_item_indirect_offset].work_item_count, 1u);
        if (output_work_item_index % 64u == 0u) {

            atomicAdd(&late_preprocess_work_item_indirect_parameters[
                immediates.late_preprocess_work_item_indirect_offset].dispatch_x, 1u);
        }

        late_preprocess_work_items[output_work_item_index].input_index = input_index;
        late_preprocess_work_items[output_work_item_index].output_or_indirect_parameters_index =
            indirect_parameters_index;
#endif

        return;
    }
#endif

    let local_from_world_transpose = transpose(maths::inverse_affine3(transpose(
        world_from_local_affine_transpose)));

    let local_from_world_transpose_a = mat2x4<f32>(
        vec4<f32>(local_from_world_transpose[0].xyz, local_from_world_transpose[1].x),
        vec4<f32>(local_from_world_transpose[1].yz, local_from_world_transpose[2].xy));
    let local_from_world_transpose_b = local_from_world_transpose[2].z;

#ifdef INDIRECT
#ifdef LATE_PHASE
    let batch_output_index = atomicLoad(
        &indirect_parameters_gpu_metadata[indirect_parameters_index].early_instance_count
    ) + atomicAdd(
        &indirect_parameters_gpu_metadata[indirect_parameters_index].late_instance_count,
        1u
    );
#else
    let batch_output_index = atomicAdd(
        &indirect_parameters_gpu_metadata[indirect_parameters_index].early_instance_count,
        1u
    );
#endif

    let mesh_output_index =
        indirect_parameters_cpu_metadata[indirect_parameters_index].base_output_index +
        batch_output_index;

#endif

    output[mesh_output_index].world_from_local = world_from_local_affine_transpose;
    output[mesh_output_index].previous_world_from_local =
        previous_world_from_local_affine_transpose;
    output[mesh_output_index].local_from_world_transpose_a = local_from_world_transpose_a;
    output[mesh_output_index].local_from_world_transpose_b = local_from_world_transpose_b;
    output[mesh_output_index].flags = current_input[input_index].flags;
    output[mesh_output_index].lightmap_uv_rect = current_input[input_index].lightmap_uv_rect;
    output[mesh_output_index].first_vertex_index = current_input[input_index].first_vertex_index;
    output[mesh_output_index].current_skin_index = current_input[input_index].current_skin_index;
    output[mesh_output_index].material_and_lightmap_bind_group_slot =
        current_input[input_index].material_and_lightmap_bind_group_slot;
    output[mesh_output_index].tag = current_input[input_index].tag;
    output[mesh_output_index].morph_descriptor_index = current_input[input_index].morph_descriptor_index;
}
