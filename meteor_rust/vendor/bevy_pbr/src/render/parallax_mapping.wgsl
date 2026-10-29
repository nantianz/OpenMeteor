#define_import_path bevy_pbr::parallax_mapping

#import bevy_render::bindless::{bindless_samplers_filtering, bindless_textures_2d}

#import bevy_pbr::{
    pbr_bindings::{depth_map_texture, depth_map_sampler},
    mesh_bindings::mesh
}

#ifdef BINDLESS
#import bevy_pbr::pbr_bindings::material_indices
#endif

fn sample_depth_map(uv: vec2<f32>, material_bind_group_slot: u32) -> f32 {

    return textureSampleLevel(
#ifdef BINDLESS
        bindless_textures_2d[material_indices[material_bind_group_slot].depth_map_texture],
        bindless_samplers_filtering[material_indices[material_bind_group_slot].depth_map_sampler],
#else
        depth_map_texture,
        depth_map_sampler,
#endif
        uv,
        0.0
    ).r;
}

fn parallaxed_uv(
    depth_scale: f32,
    max_layer_count: f32,
    max_steps: u32,

    original_uv: vec2<f32>,

    Vt: vec3<f32>,
    material_bind_group_slot: u32,
) -> vec2<f32> {
    if max_layer_count < 1.0 {
        return original_uv;
    }
    var uv = original_uv;

    let view_steepness = abs(Vt.z);

    let layer_count = mix(max_layer_count, 1.0, view_steepness);
    let layer_depth = 1.0 / layer_count;
    var delta_uv = depth_scale * layer_depth * Vt.xy * vec2(1.0, -1.0) / view_steepness;

    var current_layer_depth = 0.0;
    var texture_depth = sample_depth_map(uv, material_bind_group_slot);

    for (var i: i32 = 0; texture_depth > current_layer_depth && i <= i32(layer_count); i++) {
        current_layer_depth += layer_depth;
        uv += delta_uv;
        texture_depth = sample_depth_map(uv, material_bind_group_slot);
    }

#ifdef RELIEF_MAPPING

    delta_uv *= 0.5;
    var delta_depth = 0.5 * layer_depth;

    uv -= delta_uv;
    current_layer_depth -= delta_depth;

    for (var i: u32 = 0u; i < max_steps; i++) {
        texture_depth = sample_depth_map(uv, material_bind_group_slot);

        delta_uv *= 0.5;
        delta_depth *= 0.5;

        if (texture_depth > current_layer_depth) {
            uv += delta_uv;
            current_layer_depth += delta_depth;
        } else {
            uv -= delta_uv;
            current_layer_depth -= delta_depth;
        }
    }
#else

    let previous_uv = uv - delta_uv;
    let next_depth = texture_depth - current_layer_depth;
    let previous_depth = sample_depth_map(previous_uv, material_bind_group_slot) -
        current_layer_depth + layer_depth;

    let weight = next_depth / (next_depth - previous_depth);

    uv = mix(uv, previous_uv, weight);

    current_layer_depth += mix(next_depth, previous_depth, weight);
#endif

    return uv;
}
