#define_import_path bevy_pbr::shadows

#import bevy_pbr::{
    mesh_view_types::POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE,
    mesh_view_bindings as view_bindings,
    shadow_sampling::{
        SPOT_SHADOW_TEXEL_SIZE, sample_shadow_cubemap, sample_shadow_cubemap_pcss,
        sample_shadow_map, sample_shadow_map_pcss,
    }
}

#import bevy_render::{
    color_operations::hsv_to_rgb,
    maths::{orthonormalize, PI_2}
}

const flip_z: vec3<f32> = vec3<f32>(1.0, 1.0, -1.0);

fn fetch_point_shadow(
    light_id: u32,
    frag_position: vec4<f32>,
    surface_normal: vec3<f32>,
    frag_coord_xy: vec2<f32>,
) -> f32 {
    let light = &view_bindings::clustered_lights.data[light_id];

    let surface_to_light = (*light).position_radius.xyz - frag_position.xyz;
    let surface_to_light_abs = abs(surface_to_light);
    let distance_to_light = max(surface_to_light_abs.x, max(surface_to_light_abs.y, surface_to_light_abs.z));

    let normal_offset = (*light).shadow_normal_bias * distance_to_light * surface_normal.xyz;
    let depth_offset = (*light).shadow_depth_bias * normalize(surface_to_light.xyz);
    let offset_position = frag_position.xyz + normal_offset + depth_offset;

    let frag_ls = offset_position.xyz - (*light).position_radius.xyz ;
    let abs_position_ls = abs(frag_ls);
    let major_axis_magnitude = max(abs_position_ls.x, max(abs_position_ls.y, abs_position_ls.z));

    let zw = -major_axis_magnitude * (*light).light_custom_data.xy + (*light).light_custom_data.zw;
    let depth = zw.x / zw.y;

    if ((*light).soft_shadow_size > 0.0) {
        return sample_shadow_cubemap_pcss(
            frag_ls * flip_z,
            distance_to_light,
            depth,
            light_id,
            (*light).soft_shadow_size,
            frag_coord_xy,
        );
    }

    return sample_shadow_cubemap(frag_ls * flip_z, distance_to_light, depth, light_id, frag_coord_xy);
}

fn fetch_spot_shadow(
    light_id: u32,
    frag_position: vec4<f32>,
    surface_normal: vec3<f32>,
    near_z: f32,
    frag_coord_xy: vec2<f32>,
) -> f32 {
    let light = &view_bindings::clustered_lights.data[light_id];

    let surface_to_light = (*light).position_radius.xyz - frag_position.xyz;

    var spot_dir = vec3<f32>((*light).light_custom_data.x, 0.0, (*light).light_custom_data.y);

    spot_dir.y = sqrt(max(0.0, 1.0 - spot_dir.x * spot_dir.x - spot_dir.z * spot_dir.z));
    if (((*light).flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u) {
        spot_dir.y = -spot_dir.y;
    }

    let fwd = -spot_dir;
    let distance_to_light = dot(fwd, surface_to_light);
    let offset_position =
        -surface_to_light
        + ((*light).shadow_depth_bias * normalize(surface_to_light))
        + (surface_normal.xyz * (*light).shadow_normal_bias) * distance_to_light;

    let light_inv_rot = orthonormalize(fwd);

    let projected_position = offset_position * light_inv_rot;

    let f_div_minus_z = 1.0 / ((*light).spot_light_tan_angle * -projected_position.z);
    let shadow_xy_ndc = projected_position.xy * f_div_minus_z;

    let shadow_uv = shadow_xy_ndc * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);

    let depth = near_z / -projected_position.z;

    let array_index = i32(light_id) + view_bindings::lights.spot_light_shadowmap_offset;
    if ((*light).soft_shadow_size > 0.0) {
        return sample_shadow_map_pcss(
            shadow_uv,
            depth,
            array_index,
            frag_coord_xy,
            SPOT_SHADOW_TEXEL_SIZE,
            (*light).soft_shadow_size,
        );
    }

    return sample_shadow_map(
        shadow_uv,
        depth,
        array_index,
        frag_coord_xy,
        SPOT_SHADOW_TEXEL_SIZE,
    );
}

fn get_cascade_index(light_id: u32, view_z: f32) -> u32 {
    let light = &view_bindings::lights.directional_lights[light_id];

    for (var i: u32 = 0u; i < (*light).num_cascades; i = i + 1u) {
        if (-view_z < (*light).cascades[i].far_bound) {
            return i;
        }
    }
    return (*light).num_cascades;
}

fn world_to_directional_light_local(
    light_id: u32,
    cascade_index: u32,
    offset_position: vec4<f32>
) -> vec4<f32> {
    let light = &view_bindings::lights.directional_lights[light_id];
    let cascade = &(*light).cascades[cascade_index];

    let offset_position_clip = (*cascade).clip_from_world * offset_position;
    if (offset_position_clip.w <= 0.0) {
        return vec4(0.0);
    }
    let offset_position_ndc = offset_position_clip.xyz / offset_position_clip.w;

    if (any(offset_position_ndc.xy < vec2<f32>(-1.0)) || offset_position_ndc.z < 0.0
            || any(offset_position_ndc > vec3<f32>(1.0))) {
        return vec4(0.0);
    }

    let flip_correction = vec2<f32>(0.5, -0.5);
    let light_local = offset_position_ndc.xy * flip_correction + vec2<f32>(0.5, 0.5);

    let depth = offset_position_ndc.z;

    return vec4(light_local, depth, 1.0);
}

fn sample_directional_cascade(
    light_id: u32,
    cascade_index: u32,
    frag_position: vec4<f32>,
    surface_normal: vec3<f32>,
    frag_coord_xy: vec2<f32>,
) -> f32 {
    let light = &view_bindings::lights.directional_lights[light_id];
    let cascade = &(*light).cascades[cascade_index];

    let normal_offset = (*light).shadow_normal_bias * (*cascade).texel_size * surface_normal.xyz;
    let depth_offset = (*light).shadow_depth_bias * (*light).direction_to_light.xyz;
    let offset_position = vec4<f32>(frag_position.xyz + normal_offset + depth_offset, frag_position.w);

    let light_local = world_to_directional_light_local(light_id, cascade_index, offset_position);
    if (light_local.w == 0.0) {
        return 1.0;
    }

    let array_index = i32((*light).depth_texture_base_index + cascade_index);
    let texel_size = (*cascade).texel_size;

    if ((*light).soft_shadow_size > 0.0) {
        return sample_shadow_map_pcss(
            light_local.xy,
            light_local.z,
            array_index,
            frag_coord_xy,
            texel_size,
            (*light).soft_shadow_size,
        );
    }

    return sample_shadow_map(
        light_local.xy,
        light_local.z,
        array_index,
        frag_coord_xy,
        texel_size,
    );
}

fn fetch_directional_shadow(
    light_id: u32,
    frag_position: vec4<f32>,
    surface_normal: vec3<f32>,
    view_z: f32,
    frag_coord_xy: vec2<f32>,
) -> f32 {
    let light = &view_bindings::lights.directional_lights[light_id];
    let cascade_index = get_cascade_index(light_id, view_z);

    if (cascade_index >= (*light).num_cascades) {
        return 1.0;
    }

    var shadow = sample_directional_cascade(
        light_id,
        cascade_index,
        frag_position,
        surface_normal,
        frag_coord_xy,
    );

    let next_cascade_index = cascade_index + 1u;
    if (next_cascade_index < (*light).num_cascades) {
        let this_far_bound = (*light).cascades[cascade_index].far_bound;
        let next_near_bound = (1.0 - (*light).cascades_overlap_proportion) * this_far_bound;
        if (-view_z >= next_near_bound) {
            let next_shadow = sample_directional_cascade(
                light_id,
                next_cascade_index,
                frag_position,
                surface_normal,
                frag_coord_xy,
            );
            shadow = mix(shadow, next_shadow, (-view_z - next_near_bound) / (this_far_bound - next_near_bound));
        }
    }
    return shadow;
}

fn cascade_debug_visualization(
    output_color: vec3<f32>,
    light_id: u32,
    view_z: f32,
) -> vec3<f32> {
    let overlay_alpha = 0.95;
    let cascade_index = get_cascade_index(light_id, view_z);
    let cascade_color_hsv = vec3(
        f32(cascade_index) / f32(#{MAX_CASCADES_PER_LIGHT}u + 1u) * PI_2,
        1.0,
        0.5
    );
    let cascade_color = hsv_to_rgb(cascade_color_hsv);
    return vec3<f32>(
        (1.0 - overlay_alpha) * output_color.rgb + overlay_alpha * cascade_color
    );
}
