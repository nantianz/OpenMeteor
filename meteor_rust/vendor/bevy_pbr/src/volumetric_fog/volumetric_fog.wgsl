

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_pbr::atmosphere::{
    functions::{calculate_visible_sun_ratio, clamp_to_surface},
    bruneton_functions::transmittance_lut_r_mu_to_uv,
}
#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_clip}
#import bevy_pbr::mesh_view_bindings::{
    globals, lights, view, clustered_lights,
    atmosphere, atmosphere_transmittance_texture, atmosphere_transmittance_sampler
}
#import bevy_pbr::mesh_view_types::{
    DIRECTIONAL_LIGHT_FLAGS_VOLUMETRIC_BIT,
    POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    POINT_LIGHT_FLAGS_VOLUMETRIC_BIT,
    POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE,
}
#import bevy_pbr::shadow_sampling::{
    sample_shadow_map_hardware,
    sample_shadow_cubemap,
    sample_shadow_map,
    SPOT_SHADOW_TEXEL_SIZE
}
#import bevy_pbr::shadows::{get_cascade_index, world_to_directional_light_local}
#import bevy_pbr::utils::interleaved_gradient_noise
#import bevy_pbr::view_transformations::{
    depth_ndc_to_view_z,
    frag_coord_to_ndc,
    position_ndc_to_view,
    position_ndc_to_world,
    position_view_to_world
}
#import bevy_render::maths::orthonormalize
#import bevy_pbr::clustered_forward as clustering
#import bevy_pbr::lighting::getDistanceAttenuation;

struct VolumetricFog {
    clip_from_local: mat4x4<f32>,
    uvw_from_world: mat4x4<f32>,
    far_planes: array<vec4<f32>, 6>,
    fog_color: vec3<f32>,
    light_tint: vec3<f32>,
    ambient_color: vec3<f32>,
    ambient_intensity: f32,
    step_count: u32,
    bounding_radius: f32,
    absorption: f32,
    scattering: f32,
    density_factor: f32,
    density_texture_offset: vec3<f32>,
    scattering_asymmetry: f32,
    light_intensity: f32,
    jitter_strength: f32,
}

@group(1) @binding(0) var<uniform> volumetric_fog: VolumetricFog;

#ifdef MULTISAMPLED
@group(1) @binding(1) var depth_texture: texture_depth_multisampled_2d;
#else
@group(1) @binding(1) var depth_texture: texture_depth_2d;
#endif

#ifdef DENSITY_TEXTURE
@group(1) @binding(2) var density_texture: texture_3d<f32>;
@group(1) @binding(3) var density_sampler: sampler;
#endif

const FRAC_4_PI: f32 = 0.07957747154594767;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
}

@vertex
fn vertex(vertex: Vertex) -> @builtin(position) vec4<f32> {
    return volumetric_fog.clip_from_local * vec4<f32>(vertex.position, 1.0);
}

fn henyey_greenstein(neg_LdotV: f32) -> f32 {
    let g = volumetric_fog.scattering_asymmetry;
    let denom = 1.0 + g * g - 2.0 * g * neg_LdotV;
    return FRAC_4_PI * (1.0 - g * g) / (denom * sqrt(denom));
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {

    let uvw_from_world = volumetric_fog.uvw_from_world;
    let fog_color = volumetric_fog.fog_color;
    let ambient_color = volumetric_fog.ambient_color;
    let ambient_intensity = volumetric_fog.ambient_intensity;
    let step_count = volumetric_fog.step_count;
    let bounding_radius = volumetric_fog.bounding_radius;
    let absorption = volumetric_fog.absorption;
    let scattering = volumetric_fog.scattering;
    let density_factor = volumetric_fog.density_factor;
    let density_texture_offset = volumetric_fog.density_texture_offset;
    let light_tint = volumetric_fog.light_tint;
    let light_intensity = volumetric_fog.light_intensity;
    let jitter_strength = volumetric_fog.jitter_strength;

    let exposure = view.exposure;

    let frag_coord = position;
    let ndc_end_depth_from_buffer = textureLoad(depth_texture, vec2<i32>(frag_coord.xy), 0);
    let view_end_depth_from_buffer = -position_ndc_to_view(
        frag_coord_to_ndc(vec4(position.xy, ndc_end_depth_from_buffer, 1.0))).z;

    let view_start_pos = position_ndc_to_view(frag_coord_to_ndc(frag_coord));

    var end_depth_view = 0.0;
    for (var plane_index = 0; plane_index < 6; plane_index += 1) {
        let plane = volumetric_fog.far_planes[plane_index];
        let other_plane_a = volumetric_fog.far_planes[(plane_index + 1) % 6];
        let other_plane_b = volumetric_fog.far_planes[(plane_index + 2) % 6];
        let other_plane_c = volumetric_fog.far_planes[(plane_index + 3) % 6];
        let other_plane_d = volumetric_fog.far_planes[(plane_index + 4) % 6];
        let other_plane_e = volumetric_fog.far_planes[(plane_index + 5) % 6];

        let t = -plane.w / dot(plane.xyz, view_start_pos.xyz);
        if (t < 0.0) {
            continue;
        }
        let hit_pos = view_start_pos.xyz * t;

        if (dot(vec4(hit_pos, 1.0), other_plane_a) >= 0.0 &&
            dot(vec4(hit_pos, 1.0), other_plane_b) >= 0.0 &&
            dot(vec4(hit_pos, 1.0), other_plane_c) >= 0.0 &&
            dot(vec4(hit_pos, 1.0), other_plane_d) >= 0.0 &&
            dot(vec4(hit_pos, 1.0), other_plane_e) >= 0.0)
        {
            end_depth_view = -hit_pos.z;
            break;
        }
    }

    end_depth_view = min(end_depth_view, view_end_depth_from_buffer);

    let start_depth_view = -depth_ndc_to_view_z(frag_coord.z);

    let ray_length_view = max(0.0, end_depth_view - start_depth_view);

    if (ray_length_view == 0.0) {
        return vec4(0.0, 0.0, 0.0, 0.0);
    }
    let inv_step_count = 1.0 / f32(step_count);
    let step_size_world = ray_length_view * inv_step_count;

    let directional_light_count = lights.n_directional_lights;

    let Rd_ndc = vec3(frag_coord_to_ndc(position).xy, 1.0);
    let Rd_view = normalize(position_ndc_to_view(Rd_ndc));
    var Ro_world = position_view_to_world(view_start_pos.xyz);
    let Rd_world = normalize(position_ndc_to_world(Rd_ndc) - view.world_position);

    let jitter = interleaved_gradient_noise(position.xy, globals.frame_count) * jitter_strength;
    Ro_world += Rd_world * jitter;

    var accumulated_color = exp(-ray_length_view * (absorption + scattering)) * ambient_color *
        ambient_intensity;

    var background_alpha = 1.0;

#ifdef DENSITY_TEXTURE
    let Ro_uvw = (uvw_from_world * vec4(Ro_world, 1.0)).xyz;
    let Rd_step_uvw = mat3x3(uvw_from_world[0].xyz, uvw_from_world[1].xyz, uvw_from_world[2].xyz) *
        (Rd_world * step_size_world);
#endif

    for (var light_index = 0u; light_index < directional_light_count; light_index += 1u) {

        let light = &lights.directional_lights[light_index];
        if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_VOLUMETRIC_BIT) == 0) {
            break;
        }

        let L = (*light).direction_to_light.xyz;

        let depth_offset = (*light).shadow_depth_bias * L;

        let neg_LdotV = dot(normalize(L), Rd_world);
        let phase = henyey_greenstein(neg_LdotV);

        background_alpha = 1.0;

        for (var step = 0u; step < step_count; step += 1u) {

            if (background_alpha < 0.001) {
                break;
            }

            let P_world = Ro_world + Rd_world * f32(step) * step_size_world;
            let P_view = view_start_pos + Rd_view * f32(step) * step_size_world;

            var density = density_factor;
#ifdef DENSITY_TEXTURE

            let P_uvw = Ro_uvw + Rd_step_uvw * f32(step);
            if (all(P_uvw >= vec3(0.0)) && all(P_uvw <= vec3(1.0))) {
                density *= textureSampleLevel(density_texture, density_sampler, P_uvw + density_texture_offset, 0.0).r;
            } else {
                density = 0.0;
            }
#endif

            let sample_attenuation = exp(-step_size_world * density * (absorption + scattering));

            background_alpha *= sample_attenuation;

            let cascade_index = get_cascade_index(light_index, P_view.z);
            let light_local = world_to_directional_light_local(
                light_index,
                cascade_index,
                vec4(P_world + depth_offset, 1.0)
            );

            var local_light_attenuation = f32(light_local.w != 0.0);

            if (local_light_attenuation != 0.0) {
                let cascade = &(*light).cascades[cascade_index];
                let array_index = i32((*light).depth_texture_base_index + cascade_index);
                local_light_attenuation =
                    sample_shadow_map_hardware(light_local.xy, light_local.z, array_index);
            }

            if (local_light_attenuation != 0.0) {
                let light_attenuation = exp(-density * bounding_radius * (absorption + scattering));
                var light_factors_per_step = fog_color * light_tint * light_attenuation *
                    scattering * density * step_size_world * light_intensity * exposure;

#ifdef ATMOSPHERE

                let P = P_world + depth_offset;
                let P_as = (atmosphere.world_to_atmosphere * vec4(P, 1.0)).xyz;
                let P_clamped = clamp_to_surface(atmosphere, P_as);
                let r = length(P_clamped);
                let local_up = normalize(P_clamped);
                let mu_light = dot(L, local_up);

                let transmittance = sample_transmittance_lut(r, mu_light);
                let sun_visibility = calculate_visible_sun_ratio(atmosphere, r, mu_light, (*light).sun_disk_angular_size);
                light_factors_per_step *= transmittance * sun_visibility;
#endif

                let light_color_per_step = (*light).color.rgb * phase * light_factors_per_step;

                accumulated_color += light_color_per_step * local_light_attenuation *
                    background_alpha;
            }
        }
    }

    let is_orthographic = view.clip_from_view[3].w == 1.0;

    background_alpha = 1.0;

    for (var step = 0u; step < step_count; step += 1u) {

        if (background_alpha < 0.001) {
            break;
        }

        let P_world = Ro_world + Rd_world * f32(step) * step_size_world;
        let P_view = view_start_pos + Rd_view * f32(step) * step_size_world;

        var density = density_factor;
#ifdef DENSITY_TEXTURE

            let P_uvw = Ro_uvw + Rd_step_uvw * f32(step);
            if (all(P_uvw >= vec3(0.0)) && all(P_uvw <= vec3(1.0))) {
                density *= textureSampleLevel(density_texture, density_sampler, P_uvw + density_texture_offset, 0.0).r;
            } else {
                density = 0.0;
            }
#endif

        var sample_color = vec3(0.0);

        let cluster_index = clustering::view_fragment_cluster_index(frag_coord.xy, P_view.z, is_orthographic);
        var clusterable_object_index_ranges = clustering::unpack_clusterable_object_index_ranges(cluster_index);
        for (var i: u32 = clusterable_object_index_ranges.first_point_light_index_offset;
            i < clusterable_object_index_ranges.first_reflection_probe_index_offset;
            i = i + 1u)
        {
            let light_id = clustering::get_clusterable_object_id(i);
            let light = &clustered_lights.data[light_id];
            if (((*light).flags & POINT_LIGHT_FLAGS_VOLUMETRIC_BIT) == 0) {
                continue;
            }

            let light_to_frag = (*light).position_radius.xyz - P_world;
            let V = Rd_world;
            let L = normalize(light_to_frag);
            let distance_square = dot(light_to_frag, light_to_frag);
            let distance_atten = getDistanceAttenuation(distance_square, (*light).color_inverse_square_range.w);
            var local_light_attenuation = distance_atten;
            if (i < clusterable_object_index_ranges.first_spot_light_index_offset) {
                var shadow: f32 = 1.0;
                if (((*light).flags & POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
                    shadow = fetch_point_shadow_without_normal(light_id, vec4(P_world, 1.0), position.xy);
                }
                local_light_attenuation *= shadow;
            } else {

                var spot_dir = vec3<f32>((*light).light_custom_data.x, 0.0, (*light).light_custom_data.y);
                spot_dir.y = sqrt(max(0.0, 1.0 - spot_dir.x * spot_dir.x - spot_dir.z * spot_dir.z));
                if ((*light).flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u {
                    spot_dir.y = -spot_dir.y;
                }

                let cd = dot(-spot_dir, normalize(light_to_frag));
                let attenuation = saturate(cd * (*light).light_custom_data.z + (*light).light_custom_data.w);
                let spot_attenuation = attenuation * attenuation;

                var shadow: f32 = 1.0;
                if (((*light).flags & POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
                    shadow = fetch_spot_shadow_without_normal(light_id, vec4(P_world, 1.0), position.xy);
                }
                local_light_attenuation *= spot_attenuation * shadow;
            }

            let light_attenuation = exp(-density * bounding_radius * (absorption + scattering));
            let light_factors_per_step = fog_color * light_tint * light_attenuation *
                scattering * density * step_size_world * light_intensity * exposure;

            let light_color_per_step = (*light).color_inverse_square_range.rgb * light_factors_per_step;

            sample_color += light_color_per_step * local_light_attenuation;
        }

        let sample_attenuation = exp(-step_size_world * density * (absorption + scattering));

        background_alpha *= sample_attenuation;
        accumulated_color += sample_color * background_alpha;
    }

    return vec4(accumulated_color, 1.0 - background_alpha);
}

fn fetch_point_shadow_without_normal(light_id: u32, frag_position: vec4<f32>, frag_coord_xy: vec2<f32>) -> f32 {
    let light = &clustered_lights.data[light_id];

    let surface_to_light = (*light).position_radius.xyz - frag_position.xyz;
    let surface_to_light_abs = abs(surface_to_light);
    let distance_to_light = max(surface_to_light_abs.x, max(surface_to_light_abs.y, surface_to_light_abs.z));

    let depth_offset = (*light).shadow_depth_bias * normalize(surface_to_light.xyz);
    let offset_position = frag_position.xyz + depth_offset;

    let frag_ls = offset_position.xyz - (*light).position_radius.xyz;
    let abs_position_ls = abs(frag_ls);
    let major_axis_magnitude = max(abs_position_ls.x, max(abs_position_ls.y, abs_position_ls.z));

    let zw = -major_axis_magnitude * (*light).light_custom_data.xy + (*light).light_custom_data.zw;
    let depth = zw.x / zw.y;

    let flip_z = vec3(1.0, 1.0, -1.0);
    return sample_shadow_cubemap(frag_ls * flip_z, distance_to_light, depth, light_id, frag_coord_xy);
}

fn fetch_spot_shadow_without_normal(light_id: u32, frag_position: vec4<f32>, frag_coord_xy: vec2<f32>) -> f32 {
    let light = &clustered_lights.data[light_id];

    let surface_to_light = (*light).position_radius.xyz - frag_position.xyz;

    var spot_dir = vec3<f32>((*light).light_custom_data.x, 0.0, (*light).light_custom_data.y);

    spot_dir.y = sqrt(max(0.0, 1.0 - spot_dir.x * spot_dir.x - spot_dir.z * spot_dir.z));
    if (((*light).flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u) {
        spot_dir.y = -spot_dir.y;
    }

    let fwd = -spot_dir;
    let offset_position =
        -surface_to_light
        + ((*light).shadow_depth_bias * normalize(surface_to_light));

    let light_inv_rot = orthonormalize(fwd);

    let projected_position = offset_position * light_inv_rot;

    let f_div_minus_z = 1.0 / ((*light).spot_light_tan_angle * -projected_position.z);
    let shadow_xy_ndc = projected_position.xy * f_div_minus_z;

    let shadow_uv = shadow_xy_ndc * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);

    let depth = 0.1 / -projected_position.z;

    return sample_shadow_map(
        shadow_uv,
        depth,
        i32(light_id) + lights.spot_light_shadowmap_offset,
        frag_coord_xy,
        SPOT_SHADOW_TEXEL_SIZE
    );
}

#ifdef ATMOSPHERE
fn sample_transmittance_lut(r: f32, mu: f32) -> vec3<f32> {
    let uv = transmittance_lut_r_mu_to_uv(atmosphere, r, mu);
    return textureSampleLevel(
        atmosphere_transmittance_texture,
        atmosphere_transmittance_sampler, uv, 0.0).rgb;
}
#endif
