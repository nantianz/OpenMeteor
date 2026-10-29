#define_import_path bevy_pbr::pbr_functions

#import bevy_pbr::{
    pbr_types,
    pbr_bindings,
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    lighting,
    lighting::{LAYER_BASE, LAYER_CLEARCOAT},
    clustered_forward as clustering,
    shadows,
    ambient,
    view_transformations,
    raymarch,
    utils,
    mesh_types::{MESH_FLAGS_SHADOW_RECEIVER_BIT, MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT},
}
#import bevy_pbr::mesh_view_bindings::globals
#import bevy_pbr::view_transformations::{position_world_to_ndc}
#import bevy_render::maths::{E, powsafe}

#ifdef STANDARD_MATERIAL_SPECULAR_TRANSMISSION
#import bevy_pbr::transmission
#endif

#ifdef IRRADIANCE_VOLUME
#import bevy_pbr::irradiance_volume
#endif

#ifdef MESHLET_MESH_MATERIAL_PASS
#import bevy_pbr::meshlet_visibility_buffer_resolve::VertexOutput
#else ifdef PREPASS_PIPELINE
#import bevy_pbr::prepass_io::VertexOutput
#else
#import bevy_pbr::forward_io::VertexOutput
#endif

#ifdef ENVIRONMENT_MAP
#import bevy_pbr::environment_map
#endif

#ifdef TONEMAP_IN_SHADER
#import bevy_core_pipeline::tonemapping::{tone_mapping, screen_space_dither}
#endif

struct SampleBias {
#ifdef MESHLET_MESH_MATERIAL_PASS
    ddx_uv: vec2<f32>,
    ddy_uv: vec2<f32>,
#else
    mip_bias: f32,
#endif
}

const DITHER_THRESHOLD_MAP: vec4<u32> = vec4(
    0x0a020800,
    0x060e040c,
    0x09010b03,
    0x050d070f
);

#ifdef VISIBILITY_RANGE_DITHER
fn visibility_range_dither(frag_coord: vec4<f32>, dither: i32) {

    if (dither == 0) {
        return;
    }

    if (dither <= -16 || dither >= 16) {
        discard;
    }

    let coords = vec2<u32>(floor(frag_coord.xy)) % 4u;
    let threshold = i32((DITHER_THRESHOLD_MAP[coords.y] >> (coords.x * 8)) & 0xff);
    if ((dither >= 0 && dither + threshold >= 16) || (dither < 0 && 1 + dither + threshold <= 0)) {
        discard;
    }
}
#endif

fn alpha_discard(material: pbr_types::StandardMaterial, output_color: vec4<f32>) -> vec4<f32> {
    var color = output_color;
    let alpha_mode = material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    if alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_OPAQUE {

        color.a = 1.0;
    }

#ifdef MAY_DISCARD

    else if alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_MASK ||
            alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_ALPHA_TO_COVERAGE {
        if color.a >= material.alpha_cutoff {

            color.a = 1.0;
        } else {

            discard;
        }
    }
#endif

    return color;
}

fn prepare_world_normal(
    world_normal: vec3<f32>,
    double_sided: bool,
    is_front: bool,
) -> vec3<f32> {
    var output: vec3<f32> = world_normal;
#ifndef VERTEX_TANGENTS
#ifndef STANDARD_MATERIAL_NORMAL_MAP

    output = (f32(!double_sided || is_front) * 2.0 - 1.0) * output;
#endif
#endif
    return output;
}

fn calculate_tbn_mikktspace(world_normal: vec3<f32>, world_tangent: vec4<f32>) -> mat3x3<f32> {

    var N: vec3<f32> = world_normal;

    var T: vec3<f32> = world_tangent.xyz;
    var B: vec3<f32> = world_tangent.w * cross(N, T);

#ifdef MESHLET_MESH_MATERIAL_PASS

    let inverse_length_n = 1.0 / length(N);
    T *= inverse_length_n;
    B *= inverse_length_n;
    N *= inverse_length_n;
#endif

    return mat3x3(T, B, N);
}

fn apply_normal_mapping(
    standard_material_flags: u32,
    TBN: mat3x3<f32>,
    double_sided: bool,
    is_front: bool,
    in_Nt: vec3<f32>,
) -> vec3<f32> {

    var T = TBN[0];
    var B = TBN[1];
    var N = TBN[2];

    var Nt = in_Nt;
    if (standard_material_flags & pbr_types::STANDARD_MATERIAL_FLAGS_TWO_COMPONENT_NORMAL_MAP) != 0u {

        Nt = vec3<f32>(Nt.rg * 2.0 - 1.0, 0.0);
        Nt.z = sqrt(1.0 - Nt.x * Nt.x - Nt.y * Nt.y);
    } else {
        Nt = Nt * 2.0 - 1.0;
    }

    if (standard_material_flags & pbr_types::STANDARD_MATERIAL_FLAGS_FLIP_NORMAL_MAP_Y) != 0u {
        Nt.y = -Nt.y;
    }

    if double_sided && !is_front {
        Nt = -Nt;
    }

    N = Nt.x * T + Nt.y * B + Nt.z * N;

    return normalize(N);
}

#ifdef STANDARD_MATERIAL_ANISOTROPY

fn bend_normal_for_anisotropy(lighting_input: ptr<function, lighting::LightingInput>) {

    let N = (*lighting_input).layers[LAYER_BASE].N;
    let roughness = (*lighting_input).layers[LAYER_BASE].roughness;
    let V = (*lighting_input).V;
    let anisotropy = (*lighting_input).anisotropy;
    let Ba = (*lighting_input).Ba;

    var bent_normal = normalize(cross(cross(Ba, V), Ba));

    let a = pow(2.0, pow(2.0, 1.0 - anisotropy * (1.0 - roughness)));
    bent_normal = normalize(mix(bent_normal, N, a));

    let R = normalize(mix(reflect(-V, bent_normal), bent_normal, roughness * roughness));

    (*lighting_input).layers[LAYER_BASE].N = bent_normal;
    (*lighting_input).layers[LAYER_BASE].R = R;
}

#endif

fn calculate_view(
    world_position: vec4<f32>,
    is_orthographic: bool,
) -> vec3<f32> {
    var V: vec3<f32>;
    if is_orthographic {

        V = normalize(vec3<f32>(view_bindings::view.clip_from_world[0].z, view_bindings::view.clip_from_world[1].z, view_bindings::view.clip_from_world[2].z));
    } else {

        V = normalize(view_bindings::view.world_position.xyz - world_position.xyz);
    }
    return V;
}

fn calculate_diffuse_color(
    base_color: vec3<f32>,
    metallic: f32,
    specular_transmission: f32,
    diffuse_transmission: f32
) -> vec3<f32> {
    return base_color * (1.0 - metallic) * (1.0 - specular_transmission) *
        (1.0 - diffuse_transmission);
}

fn calculate_F0_dielectric(reflectance: vec3<f32>) -> vec3<f32> {
    return 0.16 * reflectance * reflectance;
}

fn calculate_F0(base_color: vec3<f32>, metallic: f32, reflectance: vec3<f32>) -> vec3<f32> {
    return mix(calculate_F0_dielectric(reflectance), base_color, metallic);
}

#ifdef CONTACT_SHADOWS
#ifdef DEPTH_PREPASS
fn calculate_contact_shadow(
    world_position: vec3<f32>,
    frag_coord: vec2<f32>,
    light_dir: vec3<f32>,
    contact_shadow_steps: u32,
) -> f32 {
#ifdef BLUE_NOISE_TEXTURE
    let noise_size = textureDimensions(view_bindings::blue_noise_texture, 0);
    let noise_layers = textureNumLayers(view_bindings::blue_noise_texture);
    let noise = textureLoad(
        view_bindings::blue_noise_texture,
        vec2<i32>(frag_coord) % vec2<i32>(noise_size),
        i32(view_bindings::globals.frame_count % noise_layers),
        0
    ).x;
#else
    let noise = utils::interleaved_gradient_noise(frag_coord, view_bindings::globals.frame_count);
#endif

    let depth_size = vec2<f32>(textureDimensions(view_bindings::depth_prepass_texture));
    var rm = raymarch::depth_ray_march_new_from_depth(depth_size);
    raymarch::depth_ray_march_from_cs(&rm, position_world_to_ndc(world_position));
    raymarch::depth_ray_march_to_ws(&rm, world_position + light_dir * view_bindings::contact_shadows_settings.length);
    rm.linear_steps = contact_shadow_steps;
    rm.depth_thickness_linear_z = view_bindings::contact_shadows_settings.thickness;
    rm.march_behind_surfaces = true;
    rm.jitter = noise;

    let rm_result = raymarch::depth_ray_march_march(&rm);
    if rm_result.hit {
        return clamp((rm_result.hit_penetration_frac - 0.5) / (1.0 - 0.5), 0.0, 1.0);
    }
    return 1.0;
}
#endif
#endif

#ifndef PREPASS_FRAGMENT
fn apply_pbr_lighting(
    in: pbr_types::PbrInput,
) -> vec4<f32> {
    var output_color: vec4<f32> = in.material.base_color;

    let emissive = in.material.emissive;

    let metallic = in.material.metallic;
    let perceptual_roughness = in.material.perceptual_roughness;
    let roughness = lighting::perceptualRoughnessToRoughness(perceptual_roughness);
    let ior = in.material.ior;
    let thickness = in.material.thickness;
    let reflectance = in.material.reflectance;
    let diffuse_transmission = in.material.diffuse_transmission;
    let specular_transmission = in.material.specular_transmission;

    let specular_transmissive_color = specular_transmission * in.material.base_color.rgb;

    let diffuse_occlusion = in.diffuse_occlusion;
    let specular_occlusion = in.specular_occlusion;

    let NdotV = max(dot(in.N, in.V), 0.0001);
    let R = reflect(-in.V, in.N);

#ifdef STANDARD_MATERIAL_CLEARCOAT

    let clearcoat = in.material.clearcoat;
    let clearcoat_perceptual_roughness = in.material.clearcoat_perceptual_roughness;
    let clearcoat_roughness = lighting::perceptualRoughnessToRoughness(clearcoat_perceptual_roughness);
    let clearcoat_N = in.clearcoat_N;
    let clearcoat_NdotV = max(dot(clearcoat_N, in.V), 0.0001);
    let clearcoat_R = reflect(-in.V, clearcoat_N);
#endif

    let diffuse_color = calculate_diffuse_color(
        output_color.rgb,
        metallic,
        specular_transmission,
        diffuse_transmission
    );

    let diffuse_transmissive_color = output_color.rgb * (1.0 - metallic) * (1.0 - specular_transmission) * diffuse_transmission;

    let diffuse_transmissive_lobe_world_position = in.world_position - vec4<f32>(in.world_normal, 0.0) * thickness;

    let F0 = calculate_F0(output_color.rgb, metallic, reflectance);
    let F_ab = lighting::F_AB(perceptual_roughness, NdotV);

    var direct_light: vec3<f32> = vec3<f32>(0.0);

    var transmitted_light: vec3<f32> = vec3<f32>(0.0);

    var lighting_input: lighting::LightingInput;
    lighting_input.layers[LAYER_BASE].NdotV = NdotV;
    lighting_input.layers[LAYER_BASE].N = in.N;
    lighting_input.layers[LAYER_BASE].R = R;
    lighting_input.layers[LAYER_BASE].perceptual_roughness = perceptual_roughness;
    lighting_input.layers[LAYER_BASE].roughness = roughness;
    lighting_input.P = in.world_position.xyz;
    lighting_input.V = in.V;
    lighting_input.diffuse_color = diffuse_color;
    lighting_input.metallic = metallic;
    lighting_input.F0_dielectric = calculate_F0_dielectric(reflectance);
    lighting_input.F0_metallic = output_color.rgb;
    lighting_input.F_ab = F_ab;
#ifdef STANDARD_MATERIAL_CLEARCOAT
    lighting_input.layers[LAYER_CLEARCOAT].NdotV = clearcoat_NdotV;
    lighting_input.layers[LAYER_CLEARCOAT].N = clearcoat_N;
    lighting_input.layers[LAYER_CLEARCOAT].R = clearcoat_R;
    lighting_input.layers[LAYER_CLEARCOAT].perceptual_roughness = clearcoat_perceptual_roughness;
    lighting_input.layers[LAYER_CLEARCOAT].roughness = clearcoat_roughness;
    lighting_input.clearcoat_strength = clearcoat;
#endif
#ifdef STANDARD_MATERIAL_ANISOTROPY
    lighting_input.anisotropy = in.anisotropy_strength;
    lighting_input.Ta = in.anisotropy_T;
    lighting_input.Ba = in.anisotropy_B;
#endif

#ifdef STANDARD_MATERIAL_DIFFUSE_TRANSMISSION
    var transmissive_lighting_input: lighting::LightingInput;
    transmissive_lighting_input.layers[LAYER_BASE].NdotV = 1.0;
    transmissive_lighting_input.layers[LAYER_BASE].N = -in.N;
    transmissive_lighting_input.layers[LAYER_BASE].R = vec3(0.0);
    transmissive_lighting_input.layers[LAYER_BASE].perceptual_roughness = 1.0;
    transmissive_lighting_input.layers[LAYER_BASE].roughness = 1.0;
    transmissive_lighting_input.P = diffuse_transmissive_lobe_world_position.xyz;
    transmissive_lighting_input.V = -in.V;
    transmissive_lighting_input.diffuse_color = diffuse_transmissive_color;
    transmissive_lighting_input.metallic = 0.0;
    transmissive_lighting_input.F0_dielectric = vec3(0.0);
    transmissive_lighting_input.F0_metallic = vec3(0.0);
    transmissive_lighting_input.F_ab = vec2(0.1);
#ifdef STANDARD_MATERIAL_CLEARCOAT
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].NdotV = 0.0;
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].N = vec3(0.0);
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].R = vec3(0.0);
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].perceptual_roughness = 0.0;
    transmissive_lighting_input.layers[LAYER_CLEARCOAT].roughness = 0.0;
    transmissive_lighting_input.clearcoat_strength = 0.0;
#endif
#ifdef STANDARD_MATERIAL_ANISOTROPY
    transmissive_lighting_input.anisotropy = in.anisotropy_strength;
    transmissive_lighting_input.Ta = in.anisotropy_T;
    transmissive_lighting_input.Ba = in.anisotropy_B;
#endif
#endif

    let view_z = dot(vec4<f32>(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z
    ), in.world_position);
    let cluster_index = clustering::view_fragment_cluster_index(in.frag_coord.xy, view_z, in.is_orthographic);
    var clusterable_object_index_ranges =
        clustering::unpack_clusterable_object_index_ranges(cluster_index);

#ifdef CONTACT_SHADOWS
    let contact_shadow_steps = view_bindings::contact_shadows_settings.linear_steps;
    let contact_shadow_enabled = contact_shadow_steps > 0u;
#endif

    for (var i: u32 = clusterable_object_index_ranges.first_point_light_index_offset;
            i < clusterable_object_index_ranges.first_spot_light_index_offset;
            i = i + 1u) {
        let light_id = clustering::get_clusterable_object_id(i);

#ifdef LIGHTMAP
        let enable_diffuse =
            (view_bindings::clustered_lights.data[light_id].flags &
                mesh_view_types::POINT_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE_BIT) != 0u;
#else
        let enable_diffuse = true;
#endif

        var shadow: f32 = 1.0;
        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
                && (view_bindings::clustered_lights.data[light_id].flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            shadow = shadows::fetch_point_shadow(light_id, in.world_position, in.world_normal, in.frag_coord.xy);
        }

#ifdef CONTACT_SHADOWS
#ifdef DEPTH_PREPASS
        if contact_shadow_enabled && (in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u && shadow > 0.0 &&
                (view_bindings::clustered_lights.data[light_id].flags &
                    mesh_view_types::POINT_LIGHT_FLAGS_CONTACT_SHADOWS_ENABLED_BIT) != 0u {
            let L = normalize(view_bindings::clustered_lights.data[light_id].position_radius.xyz - in.world_position.xyz);
            shadow *= calculate_contact_shadow(in.world_position.xyz, in.frag_coord.xy, L, contact_shadow_steps);
        }
#endif
#endif

        let light_contrib = lighting::point_light(light_id, &lighting_input, enable_diffuse, true);
        direct_light += light_contrib * shadow;

#ifdef STANDARD_MATERIAL_DIFFUSE_TRANSMISSION

        var transmitted_shadow: f32 = 1.0;
        if ((in.flags & (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)) == (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)
                && (view_bindings::clustered_lights.data[light_id].flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            transmitted_shadow = shadows::fetch_point_shadow(light_id, diffuse_transmissive_lobe_world_position, -in.world_normal, in.frag_coord.xy);
        }

        let transmitted_light_contrib =
            lighting::point_light(light_id, &transmissive_lighting_input, enable_diffuse, true);
        transmitted_light += transmitted_light_contrib * transmitted_shadow;
#endif
    }

    for (var i: u32 = clusterable_object_index_ranges.first_spot_light_index_offset;
            i < clusterable_object_index_ranges.first_reflection_probe_index_offset;
            i = i + 1u) {
        let light_id = clustering::get_clusterable_object_id(i);

#ifdef LIGHTMAP
        let enable_diffuse =
            (view_bindings::clustered_lights.data[light_id].flags &
                mesh_view_types::POINT_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE_BIT) != 0u;
#else
        let enable_diffuse = true;
#endif

        var shadow: f32 = 1.0;
        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
                && (view_bindings::clustered_lights.data[light_id].flags &
                    mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            shadow = shadows::fetch_spot_shadow(
                light_id,
                in.world_position,
                in.world_normal,
                view_bindings::clustered_lights.data[light_id].shadow_map_near_z,
                in.frag_coord.xy,
            );
        }

#ifdef CONTACT_SHADOWS
#ifdef DEPTH_PREPASS
        if contact_shadow_enabled && (in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u && shadow > 0.0 &&
                (view_bindings::clustered_lights.data[light_id].flags &
                    mesh_view_types::POINT_LIGHT_FLAGS_CONTACT_SHADOWS_ENABLED_BIT) != 0u {
            let L = normalize(view_bindings::clustered_lights.data[light_id].position_radius.xyz - in.world_position.xyz);
            shadow *= calculate_contact_shadow(in.world_position.xyz, in.frag_coord.xy, L, contact_shadow_steps);
        }
#endif
#endif

        let light_contrib = lighting::spot_light(light_id, &lighting_input, enable_diffuse);
        direct_light += light_contrib * shadow;

#ifdef STANDARD_MATERIAL_DIFFUSE_TRANSMISSION

        var transmitted_shadow: f32 = 1.0;
        if ((in.flags & (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)) == (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)
                && (view_bindings::clustered_lights.data[light_id].flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            transmitted_shadow = shadows::fetch_spot_shadow(
                light_id,
                diffuse_transmissive_lobe_world_position,
                -in.world_normal,
                view_bindings::clustered_lights.data[light_id].shadow_map_near_z,
                in.frag_coord.xy,
            );
        }

        let transmitted_light_contrib =
            lighting::spot_light(light_id, &transmissive_lighting_input, enable_diffuse);
        transmitted_light += transmitted_light_contrib * transmitted_shadow;
#endif
    }

    let n_directional_lights = view_bindings::lights.n_directional_lights;
    for (var i: u32 = 0u; i < n_directional_lights; i = i + 1u) {

        let light = &view_bindings::lights.directional_lights[i];

#ifdef LIGHTMAP
        let enable_diffuse =
            ((*light).flags &
                mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE_BIT) !=
                0u;
#else
        let enable_diffuse = true;
#endif

        var shadow: f32 = 1.0;
        if ((in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
                && (view_bindings::lights.directional_lights[i].flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            shadow = shadows::fetch_directional_shadow(i, in.world_position, in.world_normal, view_z, in.frag_coord.xy);
        }

#ifdef CONTACT_SHADOWS
#ifdef DEPTH_PREPASS
        if contact_shadow_enabled && (in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u && shadow > 0.0 &&
                (view_bindings::lights.directional_lights[i].flags &
                    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_CONTACT_SHADOWS_ENABLED_BIT) != 0u {
            let L = view_bindings::lights.directional_lights[i].direction_to_light;
            shadow *= calculate_contact_shadow(in.world_position.xyz, in.frag_coord.xy, L, contact_shadow_steps);
        }
#endif
#endif

        var light_contrib = lighting::directional_light(i, &lighting_input, enable_diffuse);

#ifdef DIRECTIONAL_LIGHT_SHADOW_MAP_DEBUG_CASCADES
        light_contrib = shadows::cascade_debug_visualization(light_contrib, i, view_z);
#endif
        direct_light += light_contrib * shadow;

#ifdef STANDARD_MATERIAL_DIFFUSE_TRANSMISSION

        var transmitted_shadow: f32 = 1.0;
        if ((in.flags & (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)) == (MESH_FLAGS_SHADOW_RECEIVER_BIT | MESH_FLAGS_TRANSMITTED_SHADOW_RECEIVER_BIT)
                && (view_bindings::lights.directional_lights[i].flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            transmitted_shadow = shadows::fetch_directional_shadow(i, diffuse_transmissive_lobe_world_position, -in.world_normal, view_z, in.frag_coord.xy);
        }

        let transmitted_light_contrib =
            lighting::directional_light(i, &transmissive_lighting_input, enable_diffuse);
        transmitted_light += transmitted_light_contrib * transmitted_shadow;
#endif
    }

#ifdef AREA_LIGHT_LUTS

    let n_rect_lights = view_bindings::lights.n_rect_lights;
    for (var i: u32 = 0u; i < n_rect_lights; i = i + 1u) {
        let enable_diffuse = true;
        let light_contrib = lighting::rect_light(i, &lighting_input, enable_diffuse);
        direct_light += light_contrib;

    #ifdef STANDARD_MATERIAL_DIFFUSE_TRANSMISSION
        let transmitted_light_contrib =
            lighting::rect_light(i, &transmissive_lighting_input, enable_diffuse);
        transmitted_light += transmitted_light_contrib;
    #endif
    }
#endif

#ifdef STANDARD_MATERIAL_DIFFUSE_TRANSMISSION

    transmitted_light += ambient::ambient_light(diffuse_transmissive_lobe_world_position, -in.N, -in.V, 1.0, diffuse_transmissive_color, vec3<f32>(0.0), 1.0, vec3<f32>(1.0));
#endif

    var indirect_light = vec3(0.0f);
    var found_diffuse_indirect = false;

#ifdef LIGHTMAP
    indirect_light += in.lightmap_light * diffuse_color;
    found_diffuse_indirect = true;
#endif

#ifdef IRRADIANCE_VOLUME

    if (!found_diffuse_indirect) {
        let irradiance_volume_light = irradiance_volume::irradiance_volume_light(
            in.world_position.xyz,
            in.N,
            &clusterable_object_index_ranges,
        );
        indirect_light += irradiance_volume_light * diffuse_color * diffuse_occlusion;
        found_diffuse_indirect = true;
    }
#endif

#ifdef ENVIRONMENT_MAP

#ifdef SCREEN_SPACE_REFLECTIONS
    let use_ssr = perceptual_roughness <= view_bindings::ssr_settings.max_perceptual_roughness
        && perceptual_roughness >= view_bindings::ssr_settings.min_perceptual_roughness;
#else
    let use_ssr = false;
#endif

#ifdef STANDARD_MATERIAL_ANISOTROPY
    var bent_normal_lighting_input = lighting_input;
    bend_normal_for_anisotropy(&bent_normal_lighting_input);
    let environment_map_lighting_input = &bent_normal_lighting_input;
#else
    let environment_map_lighting_input = &lighting_input;
#endif

    let environment_light = environment_map::environment_map_light(
        environment_map_lighting_input,
        &clusterable_object_index_ranges,
        found_diffuse_indirect,
    );

    indirect_light += environment_light.diffuse * diffuse_occlusion;
    if (!use_ssr) {
        indirect_light += environment_light.specular * specular_occlusion;
    }
#endif

#ifdef LIGHTMAP
    let enable_ambient = view_bindings::lights.ambient_light_affects_lightmapped_meshes != 0u;
#else
    let enable_ambient = true;
#endif
    if (enable_ambient) {
        indirect_light += ambient::ambient_light(in.world_position, in.N, in.V, NdotV, diffuse_color, F0, perceptual_roughness, diffuse_occlusion);
    }

    var specular_transmitted_environment_light = vec3<f32>(0.0);

#ifdef ENVIRONMENT_MAP

#ifdef STANDARD_MATERIAL_DIFFUSE_OR_SPECULAR_TRANSMISSION

    let T = -normalize(
        in.V +
        refract(in.V, -in.N, 1.0 / ior) * thickness
    );

    var transmissive_environment_light_input: lighting::LightingInput;
    transmissive_environment_light_input.diffuse_color = vec3(1.0);
    transmissive_environment_light_input.layers[LAYER_BASE].NdotV = 1.0;
    transmissive_environment_light_input.P = in.world_position.xyz;
    transmissive_environment_light_input.layers[LAYER_BASE].N = -in.N;
    transmissive_environment_light_input.V = in.V;
    transmissive_environment_light_input.layers[LAYER_BASE].R = T;
    transmissive_environment_light_input.layers[LAYER_BASE].perceptual_roughness = perceptual_roughness;
    transmissive_environment_light_input.layers[LAYER_BASE].roughness = roughness;
    transmissive_environment_light_input.metallic = 0.0;
    transmissive_environment_light_input.F0_dielectric = vec3<f32>(1.0);
    transmissive_environment_light_input.F0_metallic = vec3<f32>(0.0);
    transmissive_environment_light_input.F_ab = vec2(0.1);
#ifdef STANDARD_MATERIAL_CLEARCOAT

    transmissive_environment_light_input.clearcoat_strength = 0.0;
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].NdotV = 0.0;
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].N = in.N;
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].R = vec3(0.0);
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].perceptual_roughness = 0.0;
    transmissive_environment_light_input.layers[LAYER_CLEARCOAT].roughness = 0.0;
#endif

    let transmitted_environment_light = environment_map::environment_map_light(
        &transmissive_environment_light_input,
        &clusterable_object_index_ranges,
        false,
    );

#ifdef STANDARD_MATERIAL_DIFFUSE_TRANSMISSION
    transmitted_light += transmitted_environment_light.diffuse * diffuse_transmissive_color;
#endif
#ifdef STANDARD_MATERIAL_SPECULAR_TRANSMISSION
    specular_transmitted_environment_light = transmitted_environment_light.specular * specular_transmissive_color;
#endif

#endif

#endif

    var emissive_light = emissive.rgb * output_color.a;

#ifdef STANDARD_MATERIAL_CLEARCOAT
    let clearcoat_fresnel = lighting::F_Schlick(0.04, 1.0, clearcoat_NdotV);
    emissive_light = emissive_light * (1.0 - clearcoat * clearcoat_fresnel);
#endif

    emissive_light = emissive_light * mix(1.0, view_bindings::view.exposure, emissive.a);

#ifdef STANDARD_MATERIAL_SPECULAR_TRANSMISSION
    transmitted_light += transmission::specular_transmissive_light(in.world_position, in.frag_coord.xyz, view_z, in.N, in.V, F0, ior, thickness, perceptual_roughness, specular_transmissive_color, specular_transmitted_environment_light).rgb;

    if (in.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_ATTENUATION_ENABLED_BIT) != 0u {

        var attenuation_fog: mesh_view_types::Fog;
        attenuation_fog.base_color.a = 1.0;
        attenuation_fog.be = pow(1.0 - in.material.attenuation_color.rgb, vec3<f32>(E)) / in.material.attenuation_distance;

        transmitted_light = bevy_pbr::fog::atmospheric_fog(
            attenuation_fog, vec4<f32>(transmitted_light, 1.0), thickness,
            vec3<f32>(0.0)
        ).rgb;
    }
#endif

    output_color = vec4<f32>(
        (view_bindings::view.exposure * (transmitted_light + direct_light + indirect_light)) + emissive_light,
        output_color.a
    );

    output_color = clustering::cluster_debug_visualization(
        output_color,
        view_z,
        in.is_orthographic,
        clusterable_object_index_ranges,
        cluster_index,
    );

    return output_color;
}
#endif

#ifdef DISTANCE_FOG
fn apply_fog(
    fog_params: mesh_view_types::Fog,
    input_color: vec4<f32>,
    fragment_world_position: vec3<f32>,
    view_world_position: vec3<f32>,
    frag_coord_xy: vec2<f32>,
) -> vec4<f32> {
    let view_to_world = fragment_world_position.xyz - view_world_position.xyz;

    let distance = length(view_to_world);

    let view_pos = view_transformations::position_world_to_view(fragment_world_position);
    let view_z = view_pos.z;

    let view_direction_normal = normalize(-view_to_world);
    let fragment_world_position_vec4 = vec4<f32>(fragment_world_position, 1.0);

    var scattering = vec3<f32>(0.0);
    if fog_params.directional_light_color.a > 0.0 {
        let view_to_world_normalized = view_to_world / distance;
        let n_directional_lights = view_bindings::lights.n_directional_lights;
        for (var i: u32 = 0u; i < n_directional_lights; i = i + 1u) {
            let light = view_bindings::lights.directional_lights[i];
            let scattering_contribution = pow(
                max(
                    dot(view_to_world_normalized, light.direction_to_light),
                    0.0
                ),
                fog_params.directional_light_exponent
            ) * light.color.rgb * view_bindings::view.exposure;

            var shadow: f32 = 1.0;
            if ((light.flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
                shadow = shadows::fetch_directional_shadow(i, fragment_world_position_vec4, view_direction_normal, view_z, frag_coord_xy);
            }
            scattering += scattering_contribution * shadow;
        }
    }

    if fog_params.mode == mesh_view_types::FOG_MODE_LINEAR {
        return bevy_pbr::fog::linear_fog(fog_params, input_color, distance, scattering);
    } else if fog_params.mode == mesh_view_types::FOG_MODE_EXPONENTIAL {
        return bevy_pbr::fog::exponential_fog(fog_params, input_color, distance, scattering);
    } else if fog_params.mode == mesh_view_types::FOG_MODE_EXPONENTIAL_SQUARED {
        return bevy_pbr::fog::exponential_squared_fog(fog_params, input_color, distance, scattering);
    } else if fog_params.mode == mesh_view_types::FOG_MODE_ATMOSPHERIC {
        return bevy_pbr::fog::atmospheric_fog(fog_params, input_color, distance, scattering);
    } else {
        return input_color;
    }
}
#endif

#ifdef PREMULTIPLY_ALPHA
fn premultiply_alpha(standard_material_flags: u32, color: vec4<f32>) -> vec4<f32> {

#ifdef BLEND_PREMULTIPLIED_ALPHA

    let alpha_mode = standard_material_flags & pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_RESERVED_BITS;
    if alpha_mode == pbr_types::STANDARD_MATERIAL_FLAGS_ALPHA_MODE_ADD {

        return vec4<f32>(color.rgb * color.a, 0.0);
    } else {

        return color.rgba;
    }
#endif

#ifdef BLEND_MULTIPLY

    return vec4<f32>(color.rgb * color.a, color.a);
#endif
}
#endif

fn main_pass_post_lighting_processing(
    pbr_input: pbr_types::PbrInput,
    input_color: vec4<f32>,
) -> vec4<f32> {
    var output_color = input_color;

#ifdef DISTANCE_FOG

    if ((pbr_input.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT) != 0u) {
        output_color = apply_fog(
            view_bindings::fog,
            output_color,
            pbr_input.world_position.xyz,
            view_bindings::view.world_position.xyz,
            pbr_input.frag_coord.xy,
        );
    }
#endif

#ifdef TONEMAP_IN_SHADER
    output_color = tone_mapping(output_color, view_bindings::view.color_grading);
#ifdef DEBAND_DITHER
    var output_rgb = output_color.rgb;
    output_rgb = powsafe(output_rgb, 1.0 / 2.2);
    output_rgb += screen_space_dither(pbr_input.frag_coord.xy);

    output_rgb = powsafe(output_rgb, 2.2);
    output_color = vec4(output_rgb, output_color.a);
#endif
#endif
#ifdef PREMULTIPLY_ALPHA
    output_color = premultiply_alpha(pbr_input.material.flags, output_color);
#endif
    return output_color;
}
