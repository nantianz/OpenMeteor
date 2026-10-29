#define_import_path bevy_pbr::light_probe

#import bevy_pbr::clustered_forward
#import bevy_pbr::clustered_forward::ClusterableObjectIndexRanges
#import bevy_pbr::mesh_view_bindings::light_probes
#import bevy_pbr::mesh_view_types::{
    LightProbe, LIGHT_PROBE_FLAG_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE,
    LIGHT_PROBE_FLAG_PARALLAX_CORRECT
}

struct LightProbeQueryResult {

    texture_index: i32,

    intensity: f32,

    light_from_world: mat4x4<f32>,

    parallax_correction_bounds: vec3<f32>,

    weight: f32,

    flags: u32,
};

fn transpose_affine_matrix(matrix: mat3x4<f32>) -> mat4x4<f32> {
    let matrix4x4 = mat4x4<f32>(
        matrix[0],
        matrix[1],
        matrix[2],
        vec4<f32>(0.0, 0.0, 0.0, 1.0));
    return transpose(matrix4x4);
}

#if AVAILABLE_STORAGE_BUFFER_BINDINGS >= 3

struct LightProbeIterator {

    current_offset: u32,

    end_offset: u32,

    world_position: vec3<f32>,

    is_irradiance_volume: bool,
}

fn light_probe_iterator_new(
    world_position: vec3<f32>,
    is_irradiance_volume: bool,
    clusterable_object_index_ranges: ptr<function, ClusterableObjectIndexRanges>,
) -> LightProbeIterator {

    if is_irradiance_volume {
        return LightProbeIterator(
            (*clusterable_object_index_ranges).first_irradiance_volume_index_offset,
            (*clusterable_object_index_ranges).first_decal_offset,
            world_position,
            true
        );
    }

    return LightProbeIterator(
        (*clusterable_object_index_ranges).first_reflection_probe_index_offset,
        (*clusterable_object_index_ranges).first_irradiance_volume_index_offset,
        world_position,
        false
    );
}

fn light_probe_iterator_next(iterator: ptr<function, LightProbeIterator>) -> LightProbeQueryResult {
    let world_position = (*iterator).world_position;

    var result: LightProbeQueryResult;
    result.texture_index = -1;
    result.weight = 0.0;

    while ((*iterator).current_offset < (*iterator).end_offset) {
        let light_probe_index = i32(clustered_forward::get_clusterable_object_id(
            (*iterator).current_offset));
        (*iterator).current_offset += 1u;

        if (light_probe_index < 0) {
            continue;
        }

        var light_probe: LightProbe;
        if (*iterator).is_irradiance_volume {
            light_probe = light_probes.irradiance_volumes[light_probe_index];
        } else {
            light_probe = light_probes.reflection_probes[light_probe_index];
        }

        let light_from_world =
            transpose_affine_matrix(light_probe.light_from_world_transposed);

        let probe_space_pos = (light_from_world * vec4<f32>(world_position, 1.0f)).xyz;

        let falloff = max(light_probe.falloff, vec3(0.0001));

        let axis_weights = saturate((1.0 - 2.0 * abs(probe_space_pos)) / (2.0 * falloff));

        let weight = min(min(axis_weights.x, axis_weights.y), axis_weights.z);

        if (weight == 0.0) {
            continue;
        }

        result.texture_index = light_probe.cubemap_index;
        result.intensity = light_probe.intensity;
        result.light_from_world = light_from_world;
        result.parallax_correction_bounds = light_probe.parallax_correction_bounds;
        result.flags = light_probe.flags;
        result.weight = weight;
        return result;
    }

    return result;
}

#else

struct LightProbeIterator {

    current_index: u32,

    end_index: u32,

    world_position: vec3<f32>,

    is_irradiance_volume: bool,
}

fn light_probe_iterator_new(
    world_position: vec3<f32>,
    is_irradiance_volume: bool,
    clusterable_object_index_ranges: ptr<function, ClusterableObjectIndexRanges>,
) -> LightProbeIterator {
    return LightProbeIterator(
        0,
        select(
            light_probes.reflection_probe_count,
            light_probes.irradiance_volume_count,
            is_irradiance_volume
        ),
        world_position,
        is_irradiance_volume
    );
}

fn light_probe_iterator_next(iterator: ptr<function, LightProbeIterator>) -> LightProbeQueryResult {
    var result: LightProbeQueryResult;
    result.texture_index = -1;
    result.weight = 0.0;

    while (true) {
        let light_probe_index = (*iterator).current_index;

        var light_probe: LightProbe;
        if is_irradiance_volume {
            light_probe = light_probes.irradiance_volumes[light_probe_index];
        } else {
            light_probe = light_probes.reflection_probes[light_probe_index];
        }

        let light_from_world =
            transpose_affine_matrix(light_probe.light_from_world_transposed);

        let probe_space_pos = (light_from_world * vec4<f32>(world_position, 1.0f)).xyz;

        let falloff = max(light_probe.falloff, vec3(0.0001));

        let axis_weights = saturate((1.0 - 2.0 * abs(probe_space_pos)) / (2.0 * falloff));

        let weight = min(min(axis_weights.x, axis_weights.y), axis_weights.z);

        if (weight == 0.0) {
            continue;
        }

        result.texture_index = light_probe.cubemap_index;
        result.intensity = light_probe.intensity;
        result.light_from_world = light_from_world;
        result.parallax_correction_bounds = light_probe.parallax_correction_bounds;
        result.flags = light_probe.flags;
        result.weight = weight;
        return result;
    }

    return result;
}

#endif
