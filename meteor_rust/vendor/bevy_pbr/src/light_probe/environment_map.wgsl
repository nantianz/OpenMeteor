#define_import_path bevy_pbr::environment_map

#import bevy_pbr::light_probe::{light_probe_iterator_new, light_probe_iterator_next}
#import bevy_pbr::mesh_view_bindings as bindings
#import bevy_pbr::mesh_view_bindings::light_probes
#import bevy_pbr::mesh_view_types::{
    LIGHT_PROBE_FLAG_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE, LIGHT_PROBE_FLAG_PARALLAX_CORRECT
}
#import bevy_pbr::lighting::{F_Schlick_vec, LightingInput, LayerLightingInput, LAYER_BASE, LAYER_CLEARCOAT}
#import bevy_pbr::clustered_forward::ClusterableObjectIndexRanges

const FLOAT_MAX: f32 = 3.40282347e+38;

struct EnvironmentMapLight {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
};

struct EnvironmentMapRadiances {
    irradiance: vec3<f32>,
    radiance: vec3<f32>,
}

struct MultiscatterResult {
    FssEss: vec3<f32>,
    FmsEms: vec3<f32>,
    Edss: vec3<f32>,
}

fn compute_cubemap_sample_dir(
    world_ray_origin: vec3<f32>,
    world_ray_direction: vec3<f32>,
    light_from_world: mat4x4<f32>,
    parallax_correction_bounds: vec3<f32>,
    parallax_correct: bool,
    view_rotation: vec4<f32>,
) -> vec3<f32> {
    var sample_dir: vec3<f32>;

    if (parallax_correct) {

        let ray_origin = (light_from_world * vec4(world_ray_origin, 1.0)).xyz;
        let ray_direction = (light_from_world * vec4(world_ray_direction, 0.0)).xyz;

        var t0 = (-parallax_correction_bounds - ray_origin) / ray_direction;
        var t1 = (parallax_correction_bounds - ray_origin) / ray_direction;

        t0 = select(vec3(FLOAT_MAX), t0, t0 >= vec3(0.0));
        t1 = select(vec3(FLOAT_MAX), t1, t1 >= vec3(0.0));

        let t_min = min(t0, t1);
        let t = min(min(t_min.x, t_min.y), t_min.z);

        sample_dir = ray_origin + ray_direction * t;
    } else {

        sample_dir = (light_from_world * vec4(world_ray_direction, 0.0)).xyz;
    }

    sample_dir = quat_rotate(view_rotation, sample_dir);

    sample_dir.z = -sample_dir.z;
    return sample_dir;
}

#ifdef MULTIPLE_LIGHT_PROBES_IN_ARRAY

fn compute_radiances(
    input: LayerLightingInput,
    clusterable_object_index_ranges: ptr<function, ClusterableObjectIndexRanges>,
    world_position: vec3<f32>,
    found_diffuse_indirect: bool,
) -> EnvironmentMapRadiances {

    let N = input.N;
    let R = input.R;
    let perceptual_roughness = input.perceptual_roughness;
    let roughness = input.roughness;

    var radiances: EnvironmentMapRadiances;

    var iterator = light_probe_iterator_new(
        world_position,
                                  false,
        clusterable_object_index_ranges,
    );

    var total_weight = 0.0;
    radiances.irradiance = vec3(0.0);
    radiances.radiance = vec3(0.0);

    while (true) {
        var query_result = light_probe_iterator_next(&iterator);
        var view_rotation = vec4<f32>(0.0, 0.0, 0.0, 1.0);

        if (query_result.texture_index < 0 && total_weight < 0.9999) {
            query_result.texture_index = light_probes.view_cubemap_index;
            query_result.intensity = light_probes.intensity_for_view;
            view_rotation = light_probes.view_rotation;
            query_result.light_from_world = mat4x4(
                vec4(1.0, 0.0, 0.0, 0.0),
                vec4(0.0, 1.0, 0.0, 0.0),
                vec4(0.0, 0.0, 1.0, 0.0),
                vec4(0.0, 0.0, 0.0, 1.0)
            );
            query_result.parallax_correction_bounds = vec3(0.0);
            if light_probes.view_environment_map_affects_lightmapped_mesh_diffuse != 0u {
                query_result.flags = LIGHT_PROBE_FLAG_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE;
            } else {
                query_result.flags = 0u;
            }
            query_result.weight = 1.0 - total_weight;
        }

        if (query_result.texture_index < 0) {
            break;
        }

        let radiance_level = perceptual_roughness * f32(textureNumLevels(
            bindings::specular_environment_maps[query_result.texture_index]) - 1u);

        var enable_diffuse = !found_diffuse_indirect;
#ifdef LIGHTMAP
        enable_diffuse = enable_diffuse &&
            (query_result.flags & LIGHT_PROBE_FLAG_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE) != 0u;
#endif

        let parallax_correct = (query_result.flags & LIGHT_PROBE_FLAG_PARALLAX_CORRECT) != 0u;

        if (enable_diffuse) {
            let irradiance_sample_dir = compute_cubemap_sample_dir(
                world_position,
                N,
                query_result.light_from_world,
                query_result.parallax_correction_bounds,
                parallax_correct,
                view_rotation,
            );
            radiances.irradiance += textureSampleLevel(
                bindings::diffuse_environment_maps[query_result.texture_index],
                bindings::environment_map_sampler,
                irradiance_sample_dir,
                0.0).rgb * query_result.intensity * query_result.weight;
        }

        var radiance_sample_dir = radiance_sample_direction(N, R, roughness);
        radiance_sample_dir = compute_cubemap_sample_dir(
            world_position,
            radiance_sample_dir,
            query_result.light_from_world,
            query_result.parallax_correction_bounds,
            parallax_correct,
            view_rotation,
        );
        radiances.radiance +=
            textureSampleLevel(
                bindings::specular_environment_maps[query_result.texture_index],
                bindings::environment_map_sampler,
                radiance_sample_dir,
                radiance_level).rgb * query_result.intensity * query_result.weight;

        total_weight += query_result.weight;
    }

    if (total_weight != 0.0) {
        radiances.irradiance /= total_weight;
        radiances.radiance /= total_weight;
    }

    return radiances;
}

#else

fn compute_radiances(
    input: LayerLightingInput,
    clusterable_object_index_ranges: ptr<function, ClusterableObjectIndexRanges>,
    world_position: vec3<f32>,
    found_diffuse_indirect: bool,
) -> EnvironmentMapRadiances {

    let N = input.N;
    let R = input.R;
    let perceptual_roughness = input.perceptual_roughness;
    let roughness = input.roughness;

    var radiances: EnvironmentMapRadiances;

    if (light_probes.view_cubemap_index < 0) {
        radiances.irradiance = vec3(0.0);
        radiances.radiance = vec3(0.0);
        return radiances;
    }

    let radiance_level = perceptual_roughness * f32(light_probes.smallest_specular_mip_level_for_view);

    let intensity = light_probes.intensity_for_view;

    var enable_diffuse = !found_diffuse_indirect;
#ifdef LIGHTMAP
    enable_diffuse = enable_diffuse &&
        light_probes.view_environment_map_affects_lightmapped_mesh_diffuse;
#endif

    if (enable_diffuse) {
        var irradiance_sample_dir = N;

        irradiance_sample_dir = quat_rotate(light_probes.view_rotation, irradiance_sample_dir);

        irradiance_sample_dir.z = -irradiance_sample_dir.z;
        radiances.irradiance = textureSampleLevel(
            bindings::diffuse_environment_map,
            bindings::environment_map_sampler,
            irradiance_sample_dir,
            0.0).rgb * intensity;
    }

    var radiance_sample_dir = radiance_sample_direction(N, R, roughness);

    radiance_sample_dir = quat_rotate(light_probes.view_rotation, radiance_sample_dir);

    radiance_sample_dir.z = -radiance_sample_dir.z;
    radiances.radiance = textureSampleLevel(
        bindings::specular_environment_map,
        bindings::environment_map_sampler,
        radiance_sample_dir,
        radiance_level).rgb * intensity;

    return radiances;
}

#endif

#ifdef STANDARD_MATERIAL_CLEARCOAT

fn environment_map_light_clearcoat(
    out: ptr<function, EnvironmentMapLight>,
    input: ptr<function, LightingInput>,
    clusterable_object_index_ranges: ptr<function, ClusterableObjectIndexRanges>,
    found_diffuse_indirect: bool,
) {

    let world_position = (*input).P;
    let clearcoat_NdotV = (*input).layers[LAYER_CLEARCOAT].NdotV;
    let clearcoat_strength = (*input).clearcoat_strength;

    let clearcoat_F0 = vec3<f32>(0.04);
    let Fc = F_Schlick_vec(clearcoat_F0, 1.0, clearcoat_NdotV) * clearcoat_strength;
    let inv_Fc = 1.0 - Fc;

    let clearcoat_radiances = compute_radiances(
        (*input).layers[LAYER_CLEARCOAT],
        clusterable_object_index_ranges,
        world_position,
        found_diffuse_indirect,
    );

    (*out).diffuse *= inv_Fc;
    (*out).specular = (*out).specular * inv_Fc * inv_Fc + clearcoat_radiances.radiance * Fc;
}

#endif

fn compute_multiscatter(
    F0: vec3<f32>,
    F_ab: vec2<f32>,
    Ems: f32,
    specular_occlusion: f32,
) -> MultiscatterResult {
    let FssEss = (F0 * F_ab.x + F_ab.y) * specular_occlusion;
    let Favg = F0 + (1.0 - F0) / 21.0;
    let FmsEms = FssEss * Favg / (1.0 - Ems * Favg) * Ems;
    let Edss = 1.0 - (FssEss + FmsEms);

    return MultiscatterResult(FssEss, FmsEms, Edss);
}

fn environment_map_light(
    input: ptr<function, LightingInput>,
    clusterable_object_index_ranges: ptr<function, ClusterableObjectIndexRanges>,
    found_diffuse_indirect: bool,
) -> EnvironmentMapLight {

    let roughness = (*input).layers[LAYER_BASE].roughness;
    let diffuse_color = (*input).diffuse_color;
    let metallic = (*input).metallic;
    let NdotV = (*input).layers[LAYER_BASE].NdotV;
    let F_ab = (*input).F_ab;
    let F0_dielectric = (*input).F0_dielectric;
    let F0_metallic = (*input).F0_metallic;
    let world_position = (*input).P;

    var out: EnvironmentMapLight;

    let radiances = compute_radiances(
        (*input).layers[LAYER_BASE],
        clusterable_object_index_ranges,
        world_position,
        found_diffuse_indirect,
    );

    if (all(radiances.irradiance == vec3(0.0)) && all(radiances.radiance == vec3(0.0))) {
        out.diffuse = vec3(0.0);
        out.specular = vec3(0.0);
        return out;
    }

    let F0_surface = mix(F0_dielectric, F0_metallic, metallic);
    let specular_occlusion = saturate(dot(F0_surface, vec3(50.0 * 0.33)));

    let Ems = 1.0 - (F_ab.x + F_ab.y);

    let ms_dielectric = compute_multiscatter(F0_dielectric, F_ab, Ems, specular_occlusion);
    let ms_metallic = compute_multiscatter(F0_metallic, F_ab, Ems, specular_occlusion);

    let FssEss = mix(ms_dielectric.FssEss, ms_metallic.FssEss, metallic);
    let FmsEms = mix(ms_dielectric.FmsEms, ms_metallic.FmsEms, metallic);
    let kD = diffuse_color * ms_dielectric.Edss;

    if (!found_diffuse_indirect) {
        out.diffuse = (FmsEms + kD) * radiances.irradiance;
    } else {
        out.diffuse = vec3(0.0);
    }

    out.specular = FssEss * radiances.radiance;

#ifdef STANDARD_MATERIAL_CLEARCOAT
    environment_map_light_clearcoat(
        &out,
        input,
        clusterable_object_index_ranges,
        found_diffuse_indirect,
    );
#endif

    return out;
}

fn radiance_sample_direction(N: vec3<f32>, R: vec3<f32>, roughness: f32) -> vec3<f32> {
    let smoothness = saturate(1.0 - roughness);
    let lerp_factor = smoothness * (sqrt(smoothness) + roughness);
    return mix(N, R, lerp_factor);
}

fn quat_rotate(q: vec4<f32>, dir: vec3<f32>) -> vec3<f32> {
    let t = 2.0 * cross(q.xyz, dir);
    return dir + q.w * t + cross(q.xyz, t);
}
