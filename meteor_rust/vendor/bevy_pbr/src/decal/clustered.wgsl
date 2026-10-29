

#define_import_path bevy_pbr::decal::clustered

#import bevy_pbr::clustered_forward
#import bevy_pbr::clustered_forward::ClusterableObjectIndexRanges
#import bevy_pbr::mesh_view_bindings
#import bevy_pbr::pbr_functions
#import bevy_pbr::pbr_types::PbrInput
#import bevy_pbr::utils::porter_duff_over
#import bevy_render::maths

#ifdef MESHLET_MESH_MATERIAL_PASS
#import bevy_pbr::meshlet_visibility_buffer_resolve::VertexOutput
#else ifdef PREPASS_PIPELINE
#import bevy_pbr::prepass_io::VertexOutput
#else
#import bevy_pbr::forward_io::VertexOutput
#endif

struct ClusteredDecalIterator {

    base_color_texture_index: i32,

    normal_map_texture_index: i32,

    metallic_roughness_texture_index: i32,

    emissive_texture_index: i32,

    uv: vec2<f32>,

    tag: u32,

    decal_index_offset: i32,

    end_offset: i32,

    world_position: vec3<f32>,
}

#ifdef CLUSTERED_DECALS_ARE_USABLE

fn clustered_decal_iterator_new(
    world_position: vec3<f32>,
    clusterable_object_index_ranges: ptr<function, ClusterableObjectIndexRanges>
) -> ClusteredDecalIterator {
    return ClusteredDecalIterator(
        -1,
        -1,
        -1,
        -1,
        vec2(0.0),
        0u,

        i32((*clusterable_object_index_ranges).first_decal_offset) - 1,
        i32((*clusterable_object_index_ranges).last_clusterable_object_index_offset),
        world_position,
    );
}

fn clustered_decal_iterator_next(iterator: ptr<function, ClusteredDecalIterator>) -> bool {
    if ((*iterator).decal_index_offset == (*iterator).end_offset) {
        return false;
    }

    (*iterator).decal_index_offset += 1;

    while ((*iterator).decal_index_offset < (*iterator).end_offset) {
        let decal_index = i32(clustered_forward::get_clusterable_object_id(
            u32((*iterator).decal_index_offset)
        ));
        let decal_space_vector =
            (mesh_view_bindings::clustered_decals.decals[decal_index].local_from_world *
            vec4((*iterator).world_position, 1.0)).xyz;

        if (all(decal_space_vector >= vec3(-0.5)) && all(decal_space_vector <= vec3(0.5))) {
            (*iterator).base_color_texture_index = i32(
                mesh_view_bindings::clustered_decals.decals[decal_index].base_color_texture_index
            );
            (*iterator).normal_map_texture_index = i32(
                mesh_view_bindings::clustered_decals.decals[decal_index].normal_map_texture_index
            );
            (*iterator).metallic_roughness_texture_index = i32(
                mesh_view_bindings::clustered_decals.decals[
                    decal_index
                ].metallic_roughness_texture_index
            );
            (*iterator).emissive_texture_index = i32(
                mesh_view_bindings::clustered_decals.decals[decal_index].emissive_texture_index
            );
            (*iterator).uv = decal_space_vector.xy * vec2(1.0, -1.0) + vec2(0.5);
            (*iterator).tag =
                mesh_view_bindings::clustered_decals.decals[decal_index].tag;
            return true;
        }

        (*iterator).decal_index_offset += 1;
    }

    return false;
}

#endif

fn get_view_z(world_position: vec3<f32>) -> f32 {
    return dot(vec4<f32>(
        mesh_view_bindings::view.view_from_world[0].z,
        mesh_view_bindings::view.view_from_world[1].z,
        mesh_view_bindings::view.view_from_world[2].z,
        mesh_view_bindings::view.view_from_world[3].z
    ), vec4(world_position, 1.0));
}

fn view_is_orthographic() -> bool {
    return mesh_view_bindings::view.clip_from_view[3].w == 1.0;
}

fn apply_decals(pbr_input: ptr<function, PbrInput>) {
    let world_position = (*pbr_input).world_position.xyz;
    let world_normal = (*pbr_input).world_normal;
    let frag_coord = (*pbr_input).frag_coord.xy;

    var base_color = (*pbr_input).material.base_color;
    var emissive = (*pbr_input).material.emissive;
    var Nt = (*pbr_input).N;
    var metallic = (*pbr_input).material.metallic;
    var perceptual_roughness = (*pbr_input).material.perceptual_roughness;

#ifdef CLUSTERED_DECALS_ARE_USABLE

    let view_z = get_view_z(world_position);
    let is_orthographic = view_is_orthographic();

    let cluster_index =
        clustered_forward::view_fragment_cluster_index(frag_coord, view_z, is_orthographic);
    var clusterable_object_index_ranges =
        clustered_forward::unpack_clusterable_object_index_ranges(cluster_index);

    var iterator = clustered_decal_iterator_new(world_position, &clusterable_object_index_ranges);
    while (clustered_decal_iterator_next(&iterator)) {

        if (iterator.base_color_texture_index >= 0) {
            let decal_base_color = textureSampleLevel(
                mesh_view_bindings::clustered_decal_textures[iterator.base_color_texture_index],
                mesh_view_bindings::clustered_decal_sampler,
                iterator.uv,
                0.0
            );

            if (iterator.metallic_roughness_texture_index >= 0) {
                let metallic_roughness_sampler = textureSampleLevel(
                    mesh_view_bindings::clustered_decal_textures[
                        iterator.metallic_roughness_texture_index
                    ],
                    mesh_view_bindings::clustered_decal_sampler,
                    iterator.uv,
                    0.0
                );

                metallic = mix(
                    metallic * base_color.a,
                    metallic_roughness_sampler.b,
                    decal_base_color.a
                );
                perceptual_roughness = mix(
                    perceptual_roughness * base_color.a,
                    metallic_roughness_sampler.g,
                    decal_base_color.a
                );
            }

            base_color = porter_duff_over(base_color, decal_base_color);
        }

#ifdef VERTEX_TANGENTS
        if (iterator.normal_map_texture_index >= 0) {
            let Nd = textureSampleLevel(
                mesh_view_bindings::clustered_decal_textures[iterator.normal_map_texture_index],
                mesh_view_bindings::clustered_decal_sampler,
                iterator.uv,
                0.0
            ).rgb * 2.0 - 1.0;

            Nt = vec3(Nt.xy + Nd.xy, Nt.z * Nd.z);
        }
#endif

        if (iterator.emissive_texture_index >= 0) {
            let decal_emissive = textureSampleLevel(
                mesh_view_bindings::clustered_decal_textures[iterator.emissive_texture_index],
                mesh_view_bindings::clustered_decal_sampler,
                iterator.uv,
                0.0
            );
            emissive += vec4(decal_emissive.rgb, 0.0);
        }
    }
#endif

    (*pbr_input).material.base_color = base_color;
    (*pbr_input).material.emissive = emissive;
    (*pbr_input).N = normalize(Nt);
    (*pbr_input).material.metallic = metallic;
    (*pbr_input).material.perceptual_roughness = perceptual_roughness;
}
