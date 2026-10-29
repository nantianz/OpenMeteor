#define_import_path bevy_pbr::shadow_sampling

#import bevy_pbr::{
    mesh_view_bindings as view_bindings,
    utils::interleaved_gradient_noise,
    utils,
}
#import bevy_render::maths::{orthonormalize, PI}

fn sample_shadow_map_hardware(light_local: vec2<f32>, depth: f32, array_index: i32) -> f32 {
#ifdef NO_ARRAY_TEXTURES_SUPPORT
    return textureSampleCompare(
        view_bindings::directional_shadow_textures,
        view_bindings::directional_shadow_textures_comparison_sampler,
        light_local,
        depth,
    );
#else
    return textureSampleCompareLevel(
        view_bindings::directional_shadow_textures,
        view_bindings::directional_shadow_textures_comparison_sampler,
        light_local,
        array_index,
        depth,
    );
#endif
}

fn search_for_blockers_in_shadow_map_hardware(
    light_local: vec2<f32>,
    depth: f32,
    array_index: i32,
) -> vec2<f32> {
#ifdef WEBGL2

    return vec2(0.0);
#else

#ifdef PCSS_SAMPLERS_AVAILABLE

#ifdef NO_ARRAY_TEXTURES_SUPPORT
    let sampled_depth = textureSampleLevel(
        view_bindings::directional_shadow_textures,
        view_bindings::directional_shadow_textures_linear_sampler,
        light_local,
        0u,
    );
#else
    let sampled_depth = textureSampleLevel(
        view_bindings::directional_shadow_textures,
        view_bindings::directional_shadow_textures_linear_sampler,
        light_local,
        array_index,
        0u,
    );
#endif
    return select(vec2(0.0), vec2(sampled_depth, 1.0), sampled_depth >= depth);

#else
    return vec2(0.0);
#endif

#endif
}

const SPOT_SHADOW_TEXEL_SIZE: f32 = 0.0134277345;
const POINT_SHADOW_SCALE: f32 = 0.003;
const POINT_SHADOW_TEMPORAL_OFFSET_SCALE: f32 = 0.5;

const D3D_SAMPLE_POINT_POSITIONS: array<vec2<f32>, 8> = array(
    vec2( 0.125, -0.375),
    vec2(-0.125,  0.375),
    vec2( 0.625,  0.125),
    vec2(-0.375, -0.625),
    vec2(-0.625,  0.625),
    vec2(-0.875, -0.125),
    vec2( 0.375,  0.875),
    vec2( 0.875, -0.875),
);

const D3D_SAMPLE_POINT_COEFFS: array<f32, 8> = array(
    0.157112,
    0.157112,
    0.138651,
    0.130251,
    0.114946,
    0.114946,
    0.107982,
    0.079001,
);

fn sample_shadow_map_castano_thirteen(light_local: vec2<f32>, depth: f32, array_index: i32) -> f32 {
    let shadow_map_size = vec2<f32>(textureDimensions(view_bindings::directional_shadow_textures));
    let inv_shadow_map_size = 1.0 / shadow_map_size;

    let uv = light_local * shadow_map_size;
    var base_uv = floor(uv + 0.5);
    let s = (uv.x + 0.5 - base_uv.x);
    let t = (uv.y + 0.5 - base_uv.y);
    base_uv -= 0.5;
    base_uv *= inv_shadow_map_size;

    let uw0 = (4.0 - 3.0 * s);
    let uw1 = 7.0;
    let uw2 = (1.0 + 3.0 * s);

    let u0 = (3.0 - 2.0 * s) / uw0 - 2.0;
    let u1 = (3.0 + s) / uw1;
    let u2 = s / uw2 + 2.0;

    let vw0 = (4.0 - 3.0 * t);
    let vw1 = 7.0;
    let vw2 = (1.0 + 3.0 * t);

    let v0 = (3.0 - 2.0 * t) / vw0 - 2.0;
    let v1 = (3.0 + t) / vw1;
    let v2 = t / vw2 + 2.0;

    var sum = 0.0;

    sum += uw0 * vw0 * sample_shadow_map_hardware(base_uv + (vec2(u0, v0) * inv_shadow_map_size), depth, array_index);
    sum += uw1 * vw0 * sample_shadow_map_hardware(base_uv + (vec2(u1, v0) * inv_shadow_map_size), depth, array_index);
    sum += uw2 * vw0 * sample_shadow_map_hardware(base_uv + (vec2(u2, v0) * inv_shadow_map_size), depth, array_index);

    sum += uw0 * vw1 * sample_shadow_map_hardware(base_uv + (vec2(u0, v1) * inv_shadow_map_size), depth, array_index);
    sum += uw1 * vw1 * sample_shadow_map_hardware(base_uv + (vec2(u1, v1) * inv_shadow_map_size), depth, array_index);
    sum += uw2 * vw1 * sample_shadow_map_hardware(base_uv + (vec2(u2, v1) * inv_shadow_map_size), depth, array_index);

    sum += uw0 * vw2 * sample_shadow_map_hardware(base_uv + (vec2(u0, v2) * inv_shadow_map_size), depth, array_index);
    sum += uw1 * vw2 * sample_shadow_map_hardware(base_uv + (vec2(u1, v2) * inv_shadow_map_size), depth, array_index);
    sum += uw2 * vw2 * sample_shadow_map_hardware(base_uv + (vec2(u2, v2) * inv_shadow_map_size), depth, array_index);

    return sum * (1.0 / 144.0);
}

fn map(min1: f32, max1: f32, min2: f32, max2: f32, value: f32) -> f32 {
    return min2 + (value - min1) * (max2 - min2) / (max1 - min1);
}

fn random_rotation_matrix(scale: vec2<f32>, temporal: bool) -> mat2x2<f32> {
    let random_angle = 2.0 * PI * interleaved_gradient_noise(
        scale, select(1u, view_bindings::globals.frame_count, temporal));
    let m = vec2(sin(random_angle), cos(random_angle));
    return mat2x2(
        m.y, -m.x,
        m.x, m.y
    );
}

fn calculate_uv_offset_scale_jimenez_fourteen(texel_size: f32, blur_size: f32) -> vec2<f32> {
    let shadow_map_size = vec2<f32>(textureDimensions(view_bindings::directional_shadow_textures));

    let f = map(0.00390625, 0.022949219, 0.015, 0.035, texel_size);
    return f * blur_size / (texel_size * shadow_map_size);
}

fn sample_shadow_map_jimenez_fourteen(
    light_local: vec2<f32>,
    depth: f32,
    array_index: i32,
    frag_coord_xy: vec2<f32>,
    texel_size: f32,
    blur_size: f32,
    temporal: bool,
) -> f32 {
    let rotation_matrix = random_rotation_matrix(frag_coord_xy, temporal);
    let uv_offset_scale = calculate_uv_offset_scale_jimenez_fourteen(texel_size, blur_size);

    let sample_offset0 = (rotation_matrix * utils::SPIRAL_OFFSET_0_) * uv_offset_scale;
    let sample_offset1 = (rotation_matrix * utils::SPIRAL_OFFSET_1_) * uv_offset_scale;
    let sample_offset2 = (rotation_matrix * utils::SPIRAL_OFFSET_2_) * uv_offset_scale;
    let sample_offset3 = (rotation_matrix * utils::SPIRAL_OFFSET_3_) * uv_offset_scale;
    let sample_offset4 = (rotation_matrix * utils::SPIRAL_OFFSET_4_) * uv_offset_scale;
    let sample_offset5 = (rotation_matrix * utils::SPIRAL_OFFSET_5_) * uv_offset_scale;
    let sample_offset6 = (rotation_matrix * utils::SPIRAL_OFFSET_6_) * uv_offset_scale;
    let sample_offset7 = (rotation_matrix * utils::SPIRAL_OFFSET_7_) * uv_offset_scale;

    var sum = 0.0;
    sum += sample_shadow_map_hardware(light_local + sample_offset0, depth, array_index);
    sum += sample_shadow_map_hardware(light_local + sample_offset1, depth, array_index);
    sum += sample_shadow_map_hardware(light_local + sample_offset2, depth, array_index);
    sum += sample_shadow_map_hardware(light_local + sample_offset3, depth, array_index);
    sum += sample_shadow_map_hardware(light_local + sample_offset4, depth, array_index);
    sum += sample_shadow_map_hardware(light_local + sample_offset5, depth, array_index);
    sum += sample_shadow_map_hardware(light_local + sample_offset6, depth, array_index);
    sum += sample_shadow_map_hardware(light_local + sample_offset7, depth, array_index);
    return sum / 8.0;
}

fn search_for_blockers_in_shadow_map(
    light_local: vec2<f32>,
    depth: f32,
    array_index: i32,
    texel_size: f32,
    search_size: f32,
) -> f32 {
    let shadow_map_size = vec2<f32>(textureDimensions(view_bindings::directional_shadow_textures));
    let uv_offset_scale = search_size / (texel_size * shadow_map_size);

    let offset0 = D3D_SAMPLE_POINT_POSITIONS[0] * uv_offset_scale;
    let offset1 = D3D_SAMPLE_POINT_POSITIONS[1] * uv_offset_scale;
    let offset2 = D3D_SAMPLE_POINT_POSITIONS[2] * uv_offset_scale;
    let offset3 = D3D_SAMPLE_POINT_POSITIONS[3] * uv_offset_scale;
    let offset4 = D3D_SAMPLE_POINT_POSITIONS[4] * uv_offset_scale;
    let offset5 = D3D_SAMPLE_POINT_POSITIONS[5] * uv_offset_scale;
    let offset6 = D3D_SAMPLE_POINT_POSITIONS[6] * uv_offset_scale;
    let offset7 = D3D_SAMPLE_POINT_POSITIONS[7] * uv_offset_scale;

    var sum = vec2(0.0);
    sum += search_for_blockers_in_shadow_map_hardware(light_local + offset0, depth, array_index);
    sum += search_for_blockers_in_shadow_map_hardware(light_local + offset1, depth, array_index);
    sum += search_for_blockers_in_shadow_map_hardware(light_local + offset2, depth, array_index);
    sum += search_for_blockers_in_shadow_map_hardware(light_local + offset3, depth, array_index);
    sum += search_for_blockers_in_shadow_map_hardware(light_local + offset4, depth, array_index);
    sum += search_for_blockers_in_shadow_map_hardware(light_local + offset5, depth, array_index);
    sum += search_for_blockers_in_shadow_map_hardware(light_local + offset6, depth, array_index);
    sum += search_for_blockers_in_shadow_map_hardware(light_local + offset7, depth, array_index);

    if (sum.y == 0.0) {
        return 0.0;
    }
    return sum.x / sum.y;
}

fn sample_shadow_map(
    light_local: vec2<f32>,
    depth: f32,
    array_index: i32,
    frag_coord_xy: vec2<f32>,
    texel_size: f32,
) -> f32 {
#ifdef SHADOW_FILTER_METHOD_GAUSSIAN
    return sample_shadow_map_castano_thirteen(light_local, depth, array_index);
#else ifdef SHADOW_FILTER_METHOD_TEMPORAL
    return sample_shadow_map_jimenez_fourteen(
        light_local, depth, array_index, frag_coord_xy, texel_size, 1.0, true);
#else ifdef SHADOW_FILTER_METHOD_HARDWARE_2X2
    return sample_shadow_map_hardware(light_local, depth, array_index);
#else

    return 0.0;
#endif
}

fn sample_shadow_map_pcss(
    light_local: vec2<f32>,
    depth: f32,
    array_index: i32,
    frag_coord_xy: vec2<f32>,
    texel_size: f32,
    light_size: f32,
) -> f32 {

    let z_blocker = search_for_blockers_in_shadow_map(
        light_local, depth, array_index, texel_size, light_size);

    let blur_size = max((z_blocker - depth) * light_size / depth, 0.5);

#ifdef SHADOW_FILTER_METHOD_TEMPORAL
    return sample_shadow_map_jimenez_fourteen(
        light_local, depth, array_index, frag_coord_xy, texel_size, blur_size, true);
#else
    return sample_shadow_map_jimenez_fourteen(
        light_local, depth, array_index, frag_coord_xy, texel_size, blur_size, false);
#endif
}

fn sample_shadow_cubemap_hardware(light_local: vec3<f32>, depth: f32, light_id: u32) -> f32 {
#ifdef NO_CUBE_ARRAY_TEXTURES_SUPPORT
    return textureSampleCompare(
        view_bindings::point_shadow_textures,
        view_bindings::point_shadow_textures_comparison_sampler,
        light_local,
        depth
    );
#else
    return textureSampleCompareLevel(
        view_bindings::point_shadow_textures,
        view_bindings::point_shadow_textures_comparison_sampler,
        light_local,
        i32(light_id),
        depth
    );
#endif
}

fn search_for_blockers_in_shadow_cubemap_hardware(
    light_local: vec3<f32>,
    depth: f32,
    light_id: u32,
) -> vec2<f32> {
#ifdef WEBGL2

    return vec2(0.0);
#else

#ifdef PCSS_SAMPLERS_AVAILABLE

#ifdef NO_CUBE_ARRAY_TEXTURES_SUPPORT
    let sampled_depth = textureSample(
        view_bindings::point_shadow_textures,
        view_bindings::point_shadow_textures_linear_sampler,
        light_local,
    );
#else
    let sampled_depth = textureSample(
        view_bindings::point_shadow_textures,
        view_bindings::point_shadow_textures_linear_sampler,
        light_local,
        i32(light_id),
    );
#endif

    return select(vec2(0.0), vec2(sampled_depth, 1.0), sampled_depth >= depth);

#else
    return vec2(0.0);
#endif

#endif
}

fn sample_shadow_cubemap_at_offset(
    position: vec2<f32>,
    coeff: f32,
    x_basis: vec3<f32>,
    y_basis: vec3<f32>,
    light_local: vec3<f32>,
    depth: f32,
    light_id: u32,
) -> f32 {
    return sample_shadow_cubemap_hardware(
        light_local + position.x * x_basis + position.y * y_basis,
        depth,
        light_id
    ) * coeff;
}

fn search_for_blockers_in_shadow_cubemap_at_offset(
    position: vec2<f32>,
    x_basis: vec3<f32>,
    y_basis: vec3<f32>,
    light_local: vec3<f32>,
    depth: f32,
    light_id: u32,
) -> vec2<f32> {
    return search_for_blockers_in_shadow_cubemap_hardware(
        light_local + position.x * x_basis + position.y * y_basis,
        depth,
        light_id
    );
}

fn sample_shadow_cubemap_gaussian(
    light_local: vec3<f32>,
    depth: f32,
    scale: f32,
    distance_to_light: f32,
    light_id: u32,
) -> f32 {

    let basis = orthonormalize(normalize(light_local)) * scale * distance_to_light;

    var sum: f32 = 0.0;
    sum += sample_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[0], D3D_SAMPLE_POINT_COEFFS[0],
        basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[1], D3D_SAMPLE_POINT_COEFFS[1],
        basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[2], D3D_SAMPLE_POINT_COEFFS[2],
        basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[3], D3D_SAMPLE_POINT_COEFFS[3],
        basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[4], D3D_SAMPLE_POINT_COEFFS[4],
        basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[5], D3D_SAMPLE_POINT_COEFFS[5],
        basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[6], D3D_SAMPLE_POINT_COEFFS[6],
        basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[7], D3D_SAMPLE_POINT_COEFFS[7],
        basis[0], basis[1], light_local, depth, light_id);
    return sum;
}

fn sample_shadow_cubemap_jittered(
    light_local: vec3<f32>,
    depth: f32,
    frag_coord_xy: vec2<f32>,
    scale: f32,
    distance_to_light: f32,
    light_id: u32,
    temporal: bool,
) -> f32 {
    let rotation_matrix = random_rotation_matrix(frag_coord_xy, temporal);

    let sample_offset0 = rotation_matrix * utils::SPIRAL_OFFSET_0_ *
        POINT_SHADOW_TEMPORAL_OFFSET_SCALE;
    let sample_offset1 = rotation_matrix * utils::SPIRAL_OFFSET_1_ *
        POINT_SHADOW_TEMPORAL_OFFSET_SCALE;
    let sample_offset2 = rotation_matrix * utils::SPIRAL_OFFSET_2_ *
        POINT_SHADOW_TEMPORAL_OFFSET_SCALE;
    let sample_offset3 = rotation_matrix * utils::SPIRAL_OFFSET_3_ *
        POINT_SHADOW_TEMPORAL_OFFSET_SCALE;
    let sample_offset4 = rotation_matrix * utils::SPIRAL_OFFSET_4_ *
        POINT_SHADOW_TEMPORAL_OFFSET_SCALE;
    let sample_offset5 = rotation_matrix * utils::SPIRAL_OFFSET_5_ *
        POINT_SHADOW_TEMPORAL_OFFSET_SCALE;
    let sample_offset6 = rotation_matrix * utils::SPIRAL_OFFSET_6_ *
        POINT_SHADOW_TEMPORAL_OFFSET_SCALE;
    let sample_offset7 = rotation_matrix * utils::SPIRAL_OFFSET_7_ *
        POINT_SHADOW_TEMPORAL_OFFSET_SCALE;

    let basis = orthonormalize(normalize(light_local)) * scale * distance_to_light;

    var sum: f32 = 0.0;
    sum += sample_shadow_cubemap_at_offset(
        sample_offset0, 0.125, basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        sample_offset1, 0.125, basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        sample_offset2, 0.125, basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        sample_offset3, 0.125, basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        sample_offset4, 0.125, basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        sample_offset5, 0.125, basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        sample_offset6, 0.125, basis[0], basis[1], light_local, depth, light_id);
    sum += sample_shadow_cubemap_at_offset(
        sample_offset7, 0.125, basis[0], basis[1], light_local, depth, light_id);
    return sum;
}

fn sample_shadow_cubemap(
    light_local: vec3<f32>,
    distance_to_light: f32,
    depth: f32,
    light_id: u32,
    frag_coord_xy: vec2<f32>,
) -> f32 {
#ifdef SHADOW_FILTER_METHOD_GAUSSIAN
    return sample_shadow_cubemap_gaussian(
        light_local, depth, POINT_SHADOW_SCALE, distance_to_light, light_id);
#else ifdef SHADOW_FILTER_METHOD_TEMPORAL
    return sample_shadow_cubemap_jittered(
        light_local, depth, frag_coord_xy, POINT_SHADOW_SCALE, distance_to_light, light_id, true);
#else ifdef SHADOW_FILTER_METHOD_HARDWARE_2X2
    return sample_shadow_cubemap_hardware(light_local, depth, light_id);
#else

    return 0.0;
#endif
}

fn search_for_blockers_in_shadow_cubemap(
    light_local: vec3<f32>,
    depth: f32,
    scale: f32,
    distance_to_light: f32,
    light_id: u32,
) -> f32 {

    let basis = orthonormalize(normalize(light_local)) * scale * distance_to_light;

    var sum: vec2<f32> = vec2(0.0);
    sum += search_for_blockers_in_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[0], basis[0], basis[1], light_local, depth, light_id);
    sum += search_for_blockers_in_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[1], basis[0], basis[1], light_local, depth, light_id);
    sum += search_for_blockers_in_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[2], basis[0], basis[1], light_local, depth, light_id);
    sum += search_for_blockers_in_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[3], basis[0], basis[1], light_local, depth, light_id);
    sum += search_for_blockers_in_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[4], basis[0], basis[1], light_local, depth, light_id);
    sum += search_for_blockers_in_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[5], basis[0], basis[1], light_local, depth, light_id);
    sum += search_for_blockers_in_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[6], basis[0], basis[1], light_local, depth, light_id);
    sum += search_for_blockers_in_shadow_cubemap_at_offset(
        D3D_SAMPLE_POINT_POSITIONS[7], basis[0], basis[1], light_local, depth, light_id);

    if (sum.y == 0.0) {
        return 0.0;
    }
    return sum.x / sum.y;
}

fn sample_shadow_cubemap_pcss(
    light_local: vec3<f32>,
    distance_to_light: f32,
    depth: f32,
    light_id: u32,
    light_size: f32,
    frag_coord_xy: vec2<f32>,
) -> f32 {
    let z_blocker = search_for_blockers_in_shadow_cubemap(
        light_local, depth, light_size, distance_to_light, light_id);

    let blur_size = max((z_blocker - depth) * light_size / depth, 0.5);

#ifdef SHADOW_FILTER_METHOD_TEMPORAL
    return sample_shadow_cubemap_jittered(
        light_local, depth, frag_coord_xy, POINT_SHADOW_SCALE * blur_size, distance_to_light, light_id, true);
#else
    return sample_shadow_cubemap_jittered(
        light_local, depth, frag_coord_xy, POINT_SHADOW_SCALE * blur_size, distance_to_light, light_id, false);
#endif
}
