#define_import_path bevy_pbr::mesh_functions

#import bevy_pbr::{
    mesh_view_bindings::{
        view,
        visibility_ranges,
        VISIBILITY_RANGE_UNIFORM_BUFFER_SIZE
    },
    mesh_bindings::mesh,
    mesh_types::MESH_FLAGS_SIGN_DETERMINANT_MODEL_3X3_BIT,
    view_transformations::position_world_to_clip,
}
#import bevy_render::maths::{affine3_to_square, mat2x4_f32_to_mat3x3_unpack}

#ifndef MESHLET_MESH_MATERIAL_PASS

fn get_world_from_local(instance_index: u32) -> mat4x4<f32> {
    return affine3_to_square(mesh[instance_index].world_from_local);
}

fn get_previous_world_from_local(instance_index: u32) -> mat4x4<f32> {
    return affine3_to_square(mesh[instance_index].previous_world_from_local);
}

fn get_local_from_world(instance_index: u32) -> mat4x4<f32> {

    let inverse_model_3x3 = transpose(mat2x4_f32_to_mat3x3_unpack(
        mesh[instance_index].local_from_world_transpose_a,
        mesh[instance_index].local_from_world_transpose_b,
    ));

    let inverse_model_4x4_no_trans = mat4x4<f32>(
        vec4(inverse_model_3x3[0], 0.0),
        vec4(inverse_model_3x3[1], 0.0),
        vec4(inverse_model_3x3[2], 0.0),
        vec4(0.0,0.0,0.0,1.0)
    );

    let model = get_world_from_local(instance_index);
    let inverse_model_4x4_only_trans = mat4x4<f32>(
        vec4(1.0,0.0,0.0,0.0),
        vec4(0.0,1.0,0.0,0.0),
        vec4(0.0,0.0,1.0,0.0),
        vec4(-model[3].xyz, 1.0)
    );

    return inverse_model_4x4_no_trans * inverse_model_4x4_only_trans;
}

#endif

fn mesh_position_local_to_world(world_from_local: mat4x4<f32>, vertex_position: vec4<f32>) -> vec4<f32> {
    return world_from_local * vertex_position;
}

fn mesh_position_local_to_clip(world_from_local: mat4x4<f32>, vertex_position: vec4<f32>) -> vec4<f32> {
    let world_position = mesh_position_local_to_world(world_from_local, vertex_position);
    return position_world_to_clip(world_position.xyz);
}

#ifndef MESHLET_MESH_MATERIAL_PASS

fn mesh_normal_local_to_world(vertex_normal: vec3<f32>, instance_index: u32) -> vec3<f32> {

    if any(vertex_normal != vec3<f32>(0.0)) {
        return normalize(
            mat2x4_f32_to_mat3x3_unpack(
                mesh[instance_index].local_from_world_transpose_a,
                mesh[instance_index].local_from_world_transpose_b,
            ) * vertex_normal
        );
    } else {
        return vertex_normal;
    }
}

#endif

fn sign_determinant_model_3x3m(mesh_flags: u32) -> f32 {

    return f32(bool(mesh_flags & MESH_FLAGS_SIGN_DETERMINANT_MODEL_3X3_BIT)) * 2.0 - 1.0;
}

#ifndef MESHLET_MESH_MATERIAL_PASS

fn mesh_tangent_local_to_world(world_from_local: mat4x4<f32>, vertex_tangent: vec4<f32>, instance_index: u32) -> vec4<f32> {

    if any(vertex_tangent != vec4<f32>(0.0)) {
        return vec4<f32>(
            normalize(
                mat3x3<f32>(
                    world_from_local[0].xyz,
                    world_from_local[1].xyz,
                    world_from_local[2].xyz,
                ) * vertex_tangent.xyz
            ),

            vertex_tangent.w * sign_determinant_model_3x3m(mesh[instance_index].flags)
        );
    } else {
        return vertex_tangent;
    }
}

#endif

#ifdef VISIBILITY_RANGE_DITHER
fn get_visibility_range_dither_level(instance_index: u32, world_position: vec4<f32>) -> i32 {
#if AVAILABLE_STORAGE_BUFFER_BINDINGS >= 6

    let visibility_buffer_array_len = arrayLength(&visibility_ranges);
#else

    let visibility_buffer_array_len = VISIBILITY_RANGE_UNIFORM_BUFFER_SIZE;
#endif

    let visibility_buffer_index = mesh[instance_index].flags & 0xffffu;
    if (visibility_buffer_index > visibility_buffer_array_len) {
        return -16;
    }

    let lod_range = visibility_ranges[visibility_buffer_index];
    let camera_distance = length(view.lod_view_world_position.xyz - world_position.xyz);

    let offset = select(-16, 0, camera_distance >= lod_range.z);
    let bounds = select(lod_range.xy, lod_range.zw, camera_distance >= lod_range.z);
    let level = i32(round((camera_distance - bounds.x) / (bounds.y - bounds.x) * 16.0));
    return offset + clamp(level, 0, 16);
}
#endif

#ifndef MESHLET_MESH_MATERIAL_PASS
fn get_tag(instance_index: u32) -> u32 {
    return mesh[instance_index].tag;
}
#endif
