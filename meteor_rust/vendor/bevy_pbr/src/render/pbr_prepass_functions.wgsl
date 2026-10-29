#define_import_path bevy_pbr::pbr_prepass_functions

#import bevy_render::bindless::{bindless_samplers_filtering, bindless_textures_2d}

#import bevy_pbr::{
    prepass_io::VertexOutput,
    prepass_bindings::previous_view_uniforms,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
    pbr_bindings,
    pbr_types,
}

#ifdef BINDLESS
#import bevy_pbr::pbr_bindings::material_indices
#endif

const PREMULTIPLIED_ALPHA_CUTOFF = 0.05;

fn prepass_alpha_discard(in: VertexOutput) {

#ifdef MAY_DISCARD
#ifdef BINDLESS
    let slot = mesh[in.instance_index].material_and_lightmap_bind_group_slot & 0xffffu;
    var output_color: vec4<f32> = pbr_bindings::material_array[material_indices[slot].material].base_color;
    let flags = pbr_bindings::material_array[material_indices[slot].material].flags;
#else
    var output_color: vec4<f32> = pbr_bindings::material.base_color;
    let flags = pbr_bindings::material.flags;
#endif

#ifdef VERTEX_UVS
#ifdef STANDARD_MATERIAL_BASE_COLOR_UV_B
    var uv = in.uv_b;
#else
    var uv = in.uv;
#endif

#ifdef BINDLESS
    let uv_transform = pbr_bindings::material_array[material_indices[slot].material].uv_transform;
#else
    let uv_transform = pbr_bindings::material.uv_transform;
#endif

    uv = (uv_transform * vec3(uv, 1.0)).xy;
    if (flags & pbr_types::STANDARD_MATERIAL_FLAGS_BASE_COLOR_TEXTURE_BIT) != 0u {
        output_color = output_color * textureSampleBias(
#ifdef BINDLESS
            bindless_textures_2d[material_indices[slot].base_color_texture],
            bindless_samplers_filtering[material_indices[slot].base_color_sampler],
#else
            pbr_bindings::base_color_texture,
            pbr_bindings::base_color_sampler,
#endif
            uv,
            view.mip_bias
        );
    }
#endif

    let alpha_mode = flags & pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    if alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_MASK {
#ifdef BINDLESS
        let alpha_cutoff = pbr_bindings::material_array[material_indices[slot].material].alpha_cutoff;
#else
        let alpha_cutoff = pbr_bindings::material.alpha_cutoff;
#endif
        if output_color.a < alpha_cutoff {
            discard;
        }
    } else if (alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_BLEND ||
            alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_ADD ||
            alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_ALPHA_TO_COVERAGE) {
        if output_color.a < PREMULTIPLIED_ALPHA_CUTOFF {
            discard;
        }
    } else if alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_PREMULTIPLIED {
        if all(output_color < vec4(PREMULTIPLIED_ALPHA_CUTOFF)) {
            discard;
        }
    }

#endif
}

#ifdef MOTION_VECTOR_PREPASS
fn calculate_motion_vector(world_position: vec4<f32>, previous_world_position: vec4<f32>) -> vec2<f32> {
    let clip_position_t = view.unjittered_clip_from_world * world_position;
    let clip_position = clip_position_t.xy / clip_position_t.w;
    let previous_clip_position_t = previous_view_uniforms.clip_from_world * previous_world_position;
    let previous_clip_position = previous_clip_position_t.xy / previous_clip_position_t.w;

    return (clip_position - previous_clip_position) * vec2(0.5, -0.5);
}
#endif
