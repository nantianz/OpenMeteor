#import bevy_render::maths::PI
#import bevy_pbr::{
    lighting,
    utils::{sample_cosine_hemisphere, dir_to_cube_uv, sample_cube_dir, hammersley_2d, rand_vec2f}
}

struct FilteringConstants {
    mip_level: f32,
    sample_count: u32,
    roughness: f32,
    noise_size_bits: vec2u,
}

@group(0) @binding(0) var input_texture: texture_2d_array<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@group(0) @binding(2) var output_texture: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(3) var<uniform> constants: FilteringConstants;
@group(0) @binding(4) var blue_noise_texture: texture_2d_array<f32>;

fn sample_environment(dir: vec3f, level: f32) -> vec4f {
    let cube_uv = dir_to_cube_uv(dir);
    return textureSampleLevel(input_texture, input_sampler, cube_uv.uv, cube_uv.face, level);
}

#ifdef HAS_BLUE_NOISE
fn sample_noise(pixel_coords: vec2u) -> vec4f {
    let noise_size = vec2u(1) << constants.noise_size_bits;
    let noise_size_mask = noise_size - vec2u(1u);
    let noise_coords = pixel_coords & noise_size_mask;
    let uv = vec2f(noise_coords) / vec2f(noise_size);
    return textureSampleLevel(blue_noise_texture, input_sampler, uv, 0u, 0.0);
}
#else

fn sample_noise(pixel_coords: vec2u) -> vec4f {
    var rng_state: u32 = (pixel_coords.x * 3966231743u) ^ (pixel_coords.y * 3928936651u);
    let rnd = rand_vec2f(&rng_state);
    return vec4f(rnd, 0.0, 0.0);
}
#endif

fn calculate_environment_map_lod(pdf: f32, width: f32, samples: f32) -> f32 {

    let omega_s = 1.0 / (samples * pdf);

    let omega_p = 4.0 * PI / (6.0 * width * width);

    return 0.5 * log2(omega_s / omega_p);
}

@compute
@workgroup_size(8, 8, 1)
fn generate_radiance_map(@builtin(global_invocation_id) global_id: vec3u) {
    let size = textureDimensions(output_texture).xy;
    let invSize = 1.0 / vec2f(size);

    let coords = vec2u(global_id.xy);
    let face = global_id.z;

    if (any(coords >= size)) {
        return;
    }

    let uv = (vec2f(coords) + 0.5) * invSize;
    let normal = sample_cube_dir(uv, face);

    let view = normal;

    let perceptual_roughness = constants.roughness;
    let roughness = lighting::perceptualRoughnessToRoughness(perceptual_roughness);

    let vector_noise = sample_noise(coords);

    var radiance = vec3f(0.0);
    var total_weight = 0.0;

    if (roughness < 0.01) {
        radiance = sample_environment(normal, 0.0).rgb;
        textureStore(output_texture, coords, face, vec4f(radiance, 1.0));
        return;
    }

    let sample_count = constants.sample_count;

    for (var i = 0u; i < sample_count; i++) {

        var xi = hammersley_2d(i, sample_count);
        xi = fract(xi + vector_noise.rg);

        let light_dir = lighting::sample_visible_ggx(xi, roughness, normal, view);

        let NdotL = dot(normal, light_dir);

        if (NdotL > 0.0) {

            let half_vector = normalize(view + light_dir);
            let NdotH = dot(normal, half_vector);
            let NdotV = dot(normal, view);

            let G = lighting::G_Smith(NdotV, NdotL, roughness);

            let pdf = lighting::ggx_vndf_pdf(view, NdotH, roughness);

            let width = f32(size.x);
            let lod = calculate_environment_map_lod(pdf, width, f32(sample_count));

            let source_mip = max(0.0, lod);

            var sample_color = sample_environment(light_dir, source_mip).rgb;

            radiance += sample_color * NdotL * G;
            total_weight += NdotL * G;
        }
    }

    if (total_weight > 0.0) {
        radiance = radiance / total_weight;
    }

    textureStore(output_texture, coords, face, vec4f(radiance, 1.0));
}

@compute
@workgroup_size(8, 8, 1)
fn generate_irradiance_map(@builtin(global_invocation_id) global_id: vec3u) {
    let size = textureDimensions(output_texture).xy;
    let invSize = 1.0 / vec2f(size);

    let coords = vec2u(global_id.xy);
    let face = global_id.z;

    if (any(coords >= size)) {
        return;
    }

    let uv = (vec2f(coords) + 0.5) * invSize;
    let normal = sample_cube_dir(uv, face);

    var irradiance = vec3f(0.0);

    for (var i = 0u; i < constants.sample_count; i++) {

        var rng: u32 = (coords.x * 2131358057u) ^ (coords.y * 3416869721u) ^ (face * 1199786941u) ^ (i * 566200673u);

        var sample_dir = sample_cosine_hemisphere(normal, &rng);

        var sample_color = sample_environment(sample_dir, 0.0).rgb;

        irradiance += sample_color;
    }

    irradiance = irradiance / f32(constants.sample_count);

    textureStore(output_texture, coords, face, vec4f(irradiance, 1.0));
}
