#define_import_path bevy_pbr::transmission

#import bevy_pbr::{
    lighting,
    prepass_utils,
    utils::interleaved_gradient_noise,
    utils,
    mesh_view_bindings as view_bindings,
};

#import bevy_render::maths::PI

#ifdef TONEMAP_IN_SHADER
#import bevy_core_pipeline::tonemapping::approximate_inverse_tone_mapping
#endif

fn specular_transmissive_light(world_position: vec4<f32>, frag_coord: vec3<f32>, view_z: f32, N: vec3<f32>, V: vec3<f32>, F0: vec3<f32>, ior: f32, thickness: f32, perceptual_roughness: f32, specular_transmissive_color: vec3<f32>, transmitted_environment_light_specular: vec3<f32>) -> vec3<f32> {

    let eta = 1.0 / ior;

    let I = -V;
    let NdotI = dot(N, I);

    let k = 1.0 - eta * eta * (1.0 - NdotI * NdotI);
    let T = eta * I - (eta * NdotI + sqrt(k)) * N;

    let exit_position = world_position.xyz + T * thickness;

    let clip_exit_position = view_bindings::view.clip_from_world * vec4<f32>(exit_position, 1.0);

    let subview_position = (clip_exit_position.xy / clip_exit_position.w) * vec2<f32>(0.5, -0.5) + 0.5;
    let viewport_origin = view_bindings::view.viewport.xy;
    let viewport_size = view_bindings::view.viewport.zw;
    let full_size = vec2<f32>(textureDimensions(view_bindings::view_transmission_texture));
    let offset_position = (subview_position * viewport_size + viewport_origin) / full_size;

    var background_color: vec4<f32>;
    if perceptual_roughness == 0.0 {

        background_color = fetch_transmissive_background_non_rough(offset_position, frag_coord);
    } else {
        background_color = fetch_transmissive_background(offset_position, frag_coord, view_z, perceptual_roughness);
    }

    background_color = vec4(background_color.rgb / view_bindings::view.exposure, background_color.a);

    let MinusNdotT = dot(-N, T);

    let F = vec3(1.0) - lighting::fresnel(F0, MinusNdotT);

    return F * specular_transmissive_color * mix(transmitted_environment_light_specular, background_color.rgb, background_color.a);
}

fn fetch_transmissive_background_non_rough(offset_position: vec2<f32>, frag_coord: vec3<f32>) -> vec4<f32> {
    var background_color = textureSampleLevel(
        view_bindings::view_transmission_texture,
        view_bindings::view_transmission_sampler,
        offset_position,
        0.0
    );

#ifdef DEPTH_PREPASS
#ifndef WEBGL2

    if prepass_utils::prepass_depth(vec4<f32>(offset_position * view_bindings::view.viewport.zw, 0.0, 0.0), 0u) > frag_coord.z {
        background_color.a = 0.0;
    }
#endif
#endif

#ifdef TONEMAP_IN_SHADER
    background_color = approximate_inverse_tone_mapping(background_color, view_bindings::view.color_grading);
#endif

    return background_color;
}

fn fetch_transmissive_background(offset_position: vec2<f32>, frag_coord: vec3<f32>, view_z: f32, perceptual_roughness: f32) -> vec4<f32> {

    let aspect = view_bindings::view.viewport.z / view_bindings::view.viewport.w;

    let blur_intensity = (perceptual_roughness * perceptual_roughness) / view_z;

#ifdef SCREEN_SPACE_SPECULAR_TRANSMISSION_BLUR_TAPS
    let num_taps = #{SCREEN_SPACE_SPECULAR_TRANSMISSION_BLUR_TAPS};
#else
    let num_taps = 8;
#endif
    let num_spirals = i32(ceil(f32(num_taps) / 8.0));
#ifdef TEMPORAL_JITTER
    let random_angle = interleaved_gradient_noise(frag_coord.xy, view_bindings::globals.frame_count);
#else
    let random_angle = interleaved_gradient_noise(frag_coord.xy, 0u);
#endif

    let pixel_checkboard = (
#ifdef TEMPORAL_JITTER

        (i32(frag_coord.x) + i32(frag_coord.y) + i32(view_bindings::globals.frame_count)) % 2
#else

        (i32(frag_coord.x) + i32(frag_coord.y)) % 2
#endif
    );

    var result = vec4<f32>(0.0);
    for (var i: i32 = 0; i < num_taps; i = i + 1) {
        let current_spiral = (i >> 3u);
        let angle = (random_angle + f32(current_spiral) / f32(num_spirals)) * 2.0 * PI;
        let m = vec2(sin(angle), cos(angle));
        let rotation_matrix = mat2x2(
            m.y, -m.x,
            m.x, m.y
        );

        var spiral_offset: vec2<f32>;
        switch i & 7 {

            case 0: { spiral_offset = utils::SPIRAL_OFFSET_0_; }
            case 1: { spiral_offset = utils::SPIRAL_OFFSET_2_; }
            case 2: { spiral_offset = utils::SPIRAL_OFFSET_4_; }
            case 3: { spiral_offset = utils::SPIRAL_OFFSET_6_; }
            case 4: { spiral_offset = utils::SPIRAL_OFFSET_1_; }
            case 5: { spiral_offset = utils::SPIRAL_OFFSET_3_; }
            case 6: { spiral_offset = utils::SPIRAL_OFFSET_5_; }
            case 7: { spiral_offset = utils::SPIRAL_OFFSET_7_; }
            default: {}
        }

        spiral_offset *= 1.0 - (0.5 * f32(current_spiral + 1) / f32(num_spirals));

        let rotated_spiral_offset = (rotation_matrix * spiral_offset) * vec2(1.0, aspect);

        let modified_offset_position = offset_position + rotated_spiral_offset * blur_intensity * (1.0 - f32(pixel_checkboard) * 0.1);

        var sample = textureSampleLevel(
            view_bindings::view_transmission_texture,
            view_bindings::view_transmission_sampler,
            modified_offset_position,
            0.0
        );

#ifdef DEPTH_PREPASS
#ifndef WEBGL2

        if prepass_utils::prepass_depth(vec4<f32>(modified_offset_position * view_bindings::view.viewport.zw, 0.0, 0.0), 0u) > frag_coord.z {
            sample = vec4<f32>(0.0);
        }
#endif
#endif

        let normalized_rgb = normalize(sample.rgb);
        result += vec4(min(sample.rgb, normalized_rgb / saturate(blur_intensity / 2.0)), sample.a);
    }

    result /= f32(num_taps);

#ifdef TONEMAP_IN_SHADER
    result = approximate_inverse_tone_mapping(result, view_bindings::view.color_grading);
#endif

    return result;
}
