#define_import_path bevy_pbr::lighting

#import bevy_pbr::{
    mesh_view_types::POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE,
    mesh_view_bindings as view_bindings,
    atmosphere::functions::{calculate_visible_sun_ratio, clamp_to_surface},
    atmosphere::bruneton_functions::transmittance_lut_r_mu_to_uv,
}
#import bevy_render::maths::{PI, orthonormalize}

const LAYER_BASE: u32 = 0;
const LAYER_CLEARCOAT: u32 = 1;

struct LayerLightingInput {

    N: vec3<f32>,

    R: vec3<f32>,

    NdotV: f32,

    perceptual_roughness: f32,

    roughness: f32,
}

struct LightingInput {
#ifdef STANDARD_MATERIAL_CLEARCOAT
    layers: array<LayerLightingInput, 2>,
#else
    layers: array<LayerLightingInput, 1>,
#endif

    P: vec3<f32>,

    V: vec3<f32>,

    diffuse_color: vec3<f32>,

    metallic: f32,

    F0_dielectric: vec3<f32>,
    F0_metallic: vec3<f32>,

    F_ab: vec2<f32>,

#ifdef STANDARD_MATERIAL_CLEARCOAT

    clearcoat_strength: f32,
#endif

#ifdef STANDARD_MATERIAL_ANISOTROPY

    anisotropy: f32,

    Ta: vec3<f32>,

    Ba: vec3<f32>,
#endif
}

struct DerivedLightingInput {

    H: vec3<f32>,

    NdotL: f32,

    NdotH: f32,

    LdotH: f32,
}

fn getDistanceAttenuation(distanceSquare: f32, inverseRangeSquared: f32) -> f32 {
    return getRangeFalloff(distanceSquare, inverseRangeSquared) * 1.0 / max(distanceSquare, 0.0001);
}

fn getRangeFalloff(distanceSquare: f32, inverseRangeSquared: f32) -> f32 {
    let factor = distanceSquare * inverseRangeSquared;
    let smoothFactor = saturate(1.0 - factor * factor);
    return smoothFactor * smoothFactor;
}

fn D_GGX(roughness: f32, NdotH: f32) -> f32 {
    let oneMinusNdotHSquared = 1.0 - NdotH * NdotH;
    let a = NdotH * roughness;
    let k = roughness / (oneMinusNdotHSquared + a * a);
    let d = k * k * (1.0 / PI);
    return d;
}

fn D_GGX_anisotropic(at: f32, ab: f32, NdotH: f32, TdotH: f32, BdotH: f32) -> f32 {
    let a2 = at * ab;
    let f = vec3(ab * TdotH, at * BdotH, a2 * NdotH);
    let w2 = a2 / dot(f, f);
    let d = a2 * w2 * w2 * (1.0 / PI);
    return d;
}

fn V_SmithGGXCorrelated(roughness: f32, NdotV: f32, NdotL: f32) -> f32 {
    let a2 = roughness * roughness;
    let lambdaV = NdotL * sqrt((NdotV - a2 * NdotV) * NdotV + a2);
    let lambdaL = NdotV * sqrt((NdotL - a2 * NdotL) * NdotL + a2);
    let v = 0.5 / (lambdaV + lambdaL);
    return v;
}

fn V_GGX_anisotropic(
    at: f32,
    ab: f32,
    NdotL: f32,
    NdotV: f32,
    BdotV: f32,
    TdotV: f32,
    TdotL: f32,
    BdotL: f32,
) -> f32 {
    let GGX_V = NdotL * length(vec3(at * TdotV, ab * BdotV, NdotV));
    let GGX_L = NdotV * length(vec3(at * TdotL, ab * BdotL, NdotL));
    let v = 0.5 / (GGX_V + GGX_L);
    return saturate(v);
}

fn ggx_vndf_pdf(i: vec3<f32>, NdotH: f32, roughness: f32) -> f32 {
    let ndf = D_GGX(roughness, NdotH);

    let ai = roughness * i.xy;
    let len2 = dot(ai, ai);
    let t = sqrt(len2 + i.z * i.z);
    if i.z >= 0.0 {
        let a = roughness;
        let s = 1.0 + length(i.xy);
        let a2 = a * a;
        let s2 = s * s;
        let k = (1.0 - a2) * s2 / (s2 + a2 * i.z * i.z);
        return ndf / (2.0 * (k * i.z + t));
    }

    return ndf * (t - i.z) / (2.0 * len2);
}

fn sample_visible_ggx(
    xi: vec2<f32>,
    roughness: f32,
    normal: vec3<f32>,
    view: vec3<f32>,
) -> vec3<f32> {
    let n = normal;
    let alpha = roughness;

    let wi_n = dot(view, n);
    let wi_z = -n * wi_n;
    let wi_xy = view + wi_z;

    let wi_std = -normalize(alpha * wi_xy + wi_z);

    let wi_std_z = dot(wi_std, n);

    let s = 1.0 + length(wi_xy);
    let a = clamp(alpha, 0.0, 1.0);
    let a2 = a * a;
    let s2 = s * s;
    let k = (1.0 - a2) * s2 / (s2 + a2 * wi_n * wi_n);
    let b = select(wi_std_z, k * wi_std_z, wi_n > 0.0);

    let z = 1.0 - xi.y * (1.0 + b);
    let sin_theta = sqrt(max(0.0, 1.0 - z * z));
    let phi = 2.0 * PI * xi.x - PI;
    let x = sin_theta * cos(phi);
    let y = sin_theta * sin(phi);
    let c_std = vec3f(x, y, z);

    let TBN = orthonormalize(n);
    let c = TBN * c_std;

    let wm_std = c + wi_std;
    let wm_std_z = n * dot(n, wm_std);
    let wm_std_xy = wm_std_z - wm_std;

    let H = normalize(alpha * wm_std_xy + wm_std_z);

    return reflect(-view, H);
}

fn G_Smith(NdotV: f32, NdotL: f32, roughness: f32) -> f32 {
    let k = roughness / 2.0;
    let GGXL = NdotL / (NdotL * (1.0 - k) + k);
    let GGXV = NdotV / (NdotV * (1.0 - k) + k);
    return GGXL * GGXV;
}

fn V_Kelemen(LdotH: f32) -> f32 {
    return 0.25 / (LdotH * LdotH);
}

fn F_Schlick_vec(f0: vec3<f32>, f90: f32, VdotH: f32) -> vec3<f32> {

    return f0 + (f90 - f0) * pow(1.0 - VdotH, 5.0);
}

fn F_Schlick(f0: f32, f90: f32, VdotH: f32) -> f32 {

    return f0 + (f90 - f0) * pow(1.0 - VdotH, 5.0);
}

fn fresnel(f0: vec3<f32>, LdotH: f32) -> vec3<f32> {

    let f90 = saturate(dot(f0, vec3<f32>(50.0 * 0.33)));
    return F_Schlick_vec(f0, f90, LdotH);
}

fn specular_multiscatter(
    D: f32,
    V: f32,
    F: vec3<f32>,
    F0: vec3<f32>,
    F_ab: vec2<f32>,
    specular_intensity: f32,
) -> vec3<f32> {
    var Fr = (specular_intensity * D * V) * F;

    Fr *= 1.0 + F0 * (1.0 / (F_ab.x + F_ab.y) - 1.0);
    return Fr;
}

fn derive_lighting_input(N: vec3<f32>, V: vec3<f32>, L: vec3<f32>) -> DerivedLightingInput {
    var input: DerivedLightingInput;
    var H: vec3<f32> = normalize(L + V);
    input.H = H;
    input.NdotL = saturate(dot(N, L));
    input.NdotH = saturate(dot(N, H));
    input.LdotH = saturate(dot(L, H));
    return input;
}

fn compute_specular_layer_values_for_point_light(
    input: ptr<function, LightingInput>,
    layer: u32,
    V: vec3<f32>,
    light_to_frag: vec3<f32>,
    light_radius: f32,
    distance: f32,
) -> vec4<f32> {

    let R = (*input).layers[layer].R;
    let a = (*input).layers[layer].roughness;

    var LtFdotR = dot(light_to_frag, R);

    LtFdotR = max(0.0001, LtFdotR);

    let centerToRay = LtFdotR * R - light_to_frag;
    let closestPoint = light_to_frag + centerToRay * saturate(
        light_radius * inverseSqrt(dot(centerToRay, centerToRay)));
    let LspecLengthInverse = inverseSqrt(dot(closestPoint, closestPoint));

    let a_prime = saturate(a + light_radius / (2.0 * distance));

    let L: vec3<f32> = closestPoint * LspecLengthInverse;
    return vec4(L, a_prime);
}

fn specular(
    input: ptr<function, LightingInput>,
    derived_input: ptr<function, DerivedLightingInput>,
    roughness: f32,
    specular_intensity: f32,
) -> vec3<f32> {

    let NdotV = (*input).layers[LAYER_BASE].NdotV;
    let F0 = mix((*input).F0_dielectric, (*input).F0_metallic, (*input).metallic);
    let NdotL = (*derived_input).NdotL;
    let NdotH = (*derived_input).NdotH;
    let LdotH = (*derived_input).LdotH;

    let D = D_GGX(roughness, NdotH);

    let V = V_SmithGGXCorrelated(roughness, NdotV, NdotL);

    let F = fresnel(F0, LdotH);

    let Fr = specular_multiscatter(D, V, F, F0, (*input).F_ab, specular_intensity);
    return Fr;
}

fn specular_clearcoat(
    input: ptr<function, LightingInput>,
    derived_input: ptr<function, DerivedLightingInput>,
    clearcoat_strength: f32,
    roughness: f32,
    specular_intensity: f32,
) -> vec2<f32> {

    let NdotH = (*derived_input).NdotH;
    let LdotH = (*derived_input).LdotH;

    let Dc = D_GGX(roughness, NdotH);

    let Vc = V_Kelemen(LdotH);

    let Fc = F_Schlick(0.04, 1.0, LdotH) * clearcoat_strength;

    let Frc = (specular_intensity * Dc * Vc) * Fc;
    return vec2(Fc, Frc);
}

#ifdef STANDARD_MATERIAL_ANISOTROPY

fn specular_anisotropy(
    input: ptr<function, LightingInput>,
    derived_input: ptr<function, DerivedLightingInput>,
    L: vec3<f32>,
    roughness: f32,
    specular_intensity: f32,
) -> vec3<f32> {

    let NdotV = (*input).layers[LAYER_BASE].NdotV;
    let V = (*input).V;
    let F0 = mix((*input).F0_dielectric, (*input).F0_metallic, (*input).metallic);
    let anisotropy = (*input).anisotropy;
    let Ta = (*input).Ta;
    let Ba = (*input).Ba;
    let H = (*derived_input).H;
    let NdotL = (*derived_input).NdotL;
    let NdotH = (*derived_input).NdotH;
    let LdotH = (*derived_input).LdotH;

    let TdotL = dot(Ta, L);
    let BdotL = dot(Ba, L);
    let TdotH = dot(Ta, H);
    let BdotH = dot(Ba, H);
    let TdotV = dot(Ta, V);
    let BdotV = dot(Ba, V);

    let ab = roughness * roughness;
    let at = mix(ab, 1.0, anisotropy * anisotropy);

    let Da = D_GGX_anisotropic(at, ab, NdotH, TdotH, BdotH);
    let Va = V_GGX_anisotropic(at, ab, NdotL, NdotV, BdotV, TdotV, TdotL, BdotL);
    let Fa = fresnel(F0, LdotH);

    let Fr = specular_multiscatter(Da, Va, Fa, F0, (*input).F_ab, specular_intensity);
    return Fr;
}

#endif

fn Fd_Burley(
    input: ptr<function, LightingInput>,
    derived_input: ptr<function, DerivedLightingInput>,
) -> f32 {

    let roughness = (*input).layers[LAYER_BASE].roughness;
    let NdotV = (*input).layers[LAYER_BASE].NdotV;
    let NdotL = (*derived_input).NdotL;
    let LdotH = (*derived_input).LdotH;

    let f90 = 0.5 + 2.0 * roughness * LdotH * LdotH;
    let lightScatter = F_Schlick(1.0, f90, NdotL);
    let viewScatter = F_Schlick(1.0, f90, NdotV);
    return lightScatter * viewScatter * (1.0 / PI);
}

fn F_AB(perceptual_roughness: f32, NdotV: f32) -> vec2<f32> {
#ifdef DFG_LUT
    return textureSampleLevel(view_bindings::dfg_lut, view_bindings::dfg_lut_sampler, vec2<f32>(NdotV, perceptual_roughness), 0.0).rg;
#else

    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = perceptual_roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * NdotV)) * r.x + r.y;

    let f_ab_epsilon = 0.00005;
    return max(vec2<f32>(-1.04, 1.04) * a004 + r.zw, vec2<f32>(f_ab_epsilon));
#endif
}

fn EnvBRDFApprox(F0: vec3<f32>, F_ab: vec2<f32>) -> vec3<f32> {
    return F0 * F_ab.x + F_ab.y;
}

fn perceptualRoughnessToRoughness(perceptualRoughness: f32) -> f32 {

    let clampedPerceptualRoughness = clamp(perceptualRoughness, 0.089, 1.0);
    return clampedPerceptualRoughness * clampedPerceptualRoughness;
}

const CUBEMAP_TYPE_CROSS_VERTICAL: u32 = 0;
const CUBEMAP_TYPE_CROSS_HORIZONTAL: u32 = 1;
const CUBEMAP_TYPE_SEQUENCE_VERTICAL: u32 = 2;
const CUBEMAP_TYPE_SEQUENCE_HORIZONTAL: u32 = 3;

const X_PLUS: u32 = 0;
const X_MINUS: u32 = 1;
const Y_PLUS: u32 = 2;
const Y_MINUS: u32 = 3;
const Z_MINUS: u32 = 4;
const Z_PLUS: u32 = 5;

fn cubemap_uv(direction: vec3<f32>, cubemap_type: u32) -> vec2<f32> {
    let abs_direction = abs(direction);
    let max_axis = max(abs_direction.x, max(abs_direction.y, abs_direction.z));

    let face_index = select(
        select(X_PLUS, X_MINUS, direction.x < 0.0),
        select(
            select(Y_PLUS, Y_MINUS, direction.y < 0.0),
            select(Z_PLUS, Z_MINUS, direction.z < 0.0),
            max_axis != abs_direction.y
        ),
        max_axis != abs_direction.x
    );

    var face_uv: vec2<f32>;
    var divisor: f32;
    var corner_uv: vec2<u32> = vec2(0, 0);
    var face_size: vec2<f32>;

    switch face_index {
        case X_PLUS:  { face_uv = vec2<f32>(direction.z, -direction.y); divisor = direction.x; }
        case X_MINUS: { face_uv = vec2<f32>(-direction.z, -direction.y); divisor = -direction.x; }
        case Y_PLUS:  { face_uv = vec2<f32>(direction.x,  -direction.z); divisor = direction.y; }
        case Y_MINUS: { face_uv = vec2<f32>(direction.x, direction.z); divisor = -direction.y; }
        case Z_PLUS:  { face_uv = vec2<f32>(direction.x, direction.y); divisor = direction.z; }
        case Z_MINUS: { face_uv = vec2<f32>(direction.x, -direction.y); divisor = -direction.z; }
        default: {}
    }
    face_uv = (face_uv / divisor) * 0.5 + 0.5;

    switch cubemap_type {
        case CUBEMAP_TYPE_CROSS_VERTICAL: {
            face_size = vec2(1.0/3.0, 1.0/4.0);
            corner_uv = vec2<u32>((0x111102u >> (4 * face_index)) & 0xFu, (0x132011u >> (4 * face_index)) & 0xFu);
        }
        case CUBEMAP_TYPE_CROSS_HORIZONTAL: {
            face_size = vec2(1.0/4.0, 1.0/3.0);
            corner_uv = vec2<u32>((0x131102u >> (4 * face_index)) & 0xFu, (0x112011u >> (4 * face_index)) & 0xFu);
        }
        case CUBEMAP_TYPE_SEQUENCE_HORIZONTAL: {
            face_size = vec2(1.0/6.0, 1.0);
            corner_uv.x = face_index;
        }
        case CUBEMAP_TYPE_SEQUENCE_VERTICAL: {
            face_size = vec2(1.0, 1.0/6.0);
            corner_uv.y = face_index;
        }
        default: {}
    }

    return (vec2<f32>(corner_uv) + face_uv) * face_size;
}

fn specular_fix_remap(a: f32) -> f32 {
    let inv_a_sq = (1.0 - a) * (1.0 - a);
    return 1.0 - inv_a_sq * inv_a_sq;
}

fn point_light(
    light_id: u32,
    input: ptr<function, LightingInput>,
    enable_diffuse: bool,
    enable_texture: bool,
) -> vec3<f32> {

    let diffuse_color = (*input).diffuse_color;
    let P = (*input).P;
    let N = (*input).layers[LAYER_BASE].N;
    let V = (*input).V;

    let light = &view_bindings::clustered_lights.data[light_id];
    let light_to_frag = (*light).position_radius.xyz - P;
    let L = normalize(light_to_frag);
    let distance_square = dot(light_to_frag, light_to_frag);
    let distance = sqrt(distance_square);
    let rangeAttenuation = getDistanceAttenuation(distance_square, (*light).color_inverse_square_range.w);

    let a = (*input).layers[LAYER_BASE].roughness;
    let specular_L_a_prime = compute_specular_layer_values_for_point_light(
        input,
        LAYER_BASE,
        V,
        light_to_frag,
        (*light).position_radius.w,
        distance,
    );
    let L_spec = specular_L_a_prime.xyz;
    let a_prime = specular_L_a_prime.w;
    var specular_derived_input = derive_lighting_input(N, V, L_spec);

    let normalizationFactor = a / a_prime;
    let specular_intensity = normalizationFactor * normalizationFactor;

    let brdf_roughness = mix(a, a_prime, specular_fix_remap(a));

#ifdef STANDARD_MATERIAL_ANISOTROPY
    var specular_light = specular_anisotropy(input, &specular_derived_input, L, brdf_roughness, specular_intensity);
#else
    var specular_light = specular(input, &specular_derived_input, brdf_roughness, specular_intensity);
#endif

    let light_radius = (*light).position_radius.w;
    if light_radius > 0.0 {
        let solid_angle = light_radius * light_radius / (distance * distance);
        specular_light *= saturate(specular_derived_input.NdotL / max(specular_derived_input.NdotL + solid_angle, 1e-4));
    }

#ifdef STANDARD_MATERIAL_CLEARCOAT

    let clearcoat_N = (*input).layers[LAYER_CLEARCOAT].N;
    let clearcoat_strength = (*input).clearcoat_strength;
    let clearcoat_a = (*input).layers[LAYER_CLEARCOAT].roughness;

    let clearcoat_specular_L_a_prime = compute_specular_layer_values_for_point_light(
        input,
        LAYER_CLEARCOAT,
        V,
        light_to_frag,
        (*light).position_radius.w,
        distance,
    );
    let L_clearcoat_spec = clearcoat_specular_L_a_prime.xyz;
    let clearcoat_a_prime = clearcoat_specular_L_a_prime.w;
    var clearcoat_specular_derived_input =
        derive_lighting_input(clearcoat_N, V, L_clearcoat_spec);

    let clearcoat_normalizationFactor = clearcoat_a / clearcoat_a_prime;

    let clearcoat_specular_intensity = clearcoat_normalizationFactor * clearcoat_normalizationFactor;

    let clearcoat_brdf_roughness = mix(clearcoat_a, clearcoat_a_prime, specular_fix_remap(clearcoat_a));

    let Fc_Frc = specular_clearcoat(
        input,
        &clearcoat_specular_derived_input,
        clearcoat_strength,
        clearcoat_brdf_roughness,
        clearcoat_specular_intensity
    );
    let inv_Fc = 1.0 - Fc_Frc.r;
    var Frc = Fc_Frc.g;

    if light_radius > 0.0 {
        let solid_angle = light_radius * light_radius / (distance * distance);
        Frc *= saturate(clearcoat_specular_derived_input.NdotL / max(clearcoat_specular_derived_input.NdotL + solid_angle, 1e-4));
    }
#endif

    var derived_input = derive_lighting_input(N, V, L);
    var diffuse = vec3(0.0);
    if (enable_diffuse) {
        diffuse = diffuse_color * Fd_Burley(input, &derived_input);
    }

    var color_times_NdotL: vec3<f32>;
#ifdef STANDARD_MATERIAL_CLEARCOAT

    color_times_NdotL = (diffuse * derived_input.NdotL + specular_light * specular_derived_input.NdotL * inv_Fc) * inv_Fc + Frc * clearcoat_specular_derived_input.NdotL;
#else
    color_times_NdotL = diffuse * derived_input.NdotL + specular_light * specular_derived_input.NdotL;
#endif

    var texture_sample = 1f;

#ifdef LIGHT_TEXTURES
    if enable_texture && (*light).decal_index != 0xFFFFFFFFu {
        let relative_position = (view_bindings::clustered_decals.decals[(*light).decal_index].local_from_world * vec4(P, 1.0)).xyz;
        let cubemap_type = view_bindings::clustered_decals.decals[(*light).decal_index].tag;
        let decal_uv = cubemap_uv(relative_position, cubemap_type);
        let image_index = view_bindings::clustered_decals.decals[(*light).decal_index].base_color_texture_index;

        texture_sample = textureSampleLevel(
            view_bindings::clustered_decal_textures[image_index],
            view_bindings::clustered_decal_sampler,
            decal_uv,
            0.0
        ).r;
    }
#endif

    return color_times_NdotL * (*light).color_inverse_square_range.rgb *
        rangeAttenuation * texture_sample;
}

fn spot_light(
    light_id: u32,
    input: ptr<function, LightingInput>,
    enable_diffuse: bool
) -> vec3<f32> {

    let point_light = point_light(light_id, input, enable_diffuse, false);

    let light = &view_bindings::clustered_lights.data[light_id];

    var spot_dir = vec3<f32>((*light).light_custom_data.x, 0.0, (*light).light_custom_data.y);
    spot_dir.y = sqrt(max(0.0, 1.0 - spot_dir.x * spot_dir.x - spot_dir.z * spot_dir.z));
    if ((*light).flags & POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u {
        spot_dir.y = -spot_dir.y;
    }
    let light_to_frag = (*light).position_radius.xyz - (*input).P.xyz;

    let cd = dot(-spot_dir, normalize(light_to_frag));
    let attenuation = saturate(cd * (*light).light_custom_data.z + (*light).light_custom_data.w);
    let spot_attenuation = attenuation * attenuation;

    var texture_sample = 1f;

#ifdef LIGHT_TEXTURES
    if (*light).decal_index != 0xFFFFFFFFu {
        let local_position = (view_bindings::clustered_decals.decals[(*light).decal_index].local_from_world *
            vec4((*input).P, 1.0)).xyz;
        if local_position.z < 0.0 {
            let decal_uv = (local_position.xy / (local_position.z * (*light).spot_light_tan_angle)) * vec2(-0.5, 0.5) + 0.5;
            let image_index = view_bindings::clustered_decals.decals[(*light).decal_index].base_color_texture_index;

            texture_sample = textureSampleLevel(
                view_bindings::clustered_decal_textures[image_index],
                view_bindings::clustered_decal_sampler,
                decal_uv,
                0.0
            ).r;
        }
    }
#endif

    return point_light * spot_attenuation * texture_sample;
}

fn directional_light(
    light_id: u32,
    input: ptr<function, LightingInput>,
    enable_diffuse: bool
) -> vec3<f32> {

    let diffuse_color = (*input).diffuse_color;
    let NdotV = (*input).layers[LAYER_BASE].NdotV;
    let N = (*input).layers[LAYER_BASE].N;
    let V = (*input).V;
    let roughness = (*input).layers[LAYER_BASE].roughness;

    let light = &view_bindings::lights.directional_lights[light_id];

    let L = (*light).direction_to_light.xyz;
    var derived_input = derive_lighting_input(N, V, L);

    var diffuse = vec3(0.0);
    if (enable_diffuse) {
        diffuse = diffuse_color * Fd_Burley(input, &derived_input);
    }

#ifdef STANDARD_MATERIAL_ANISOTROPY
    let specular_light = specular_anisotropy(input, &derived_input, L, roughness, 1.0);
#else
    let specular_light = specular(input, &derived_input, roughness, 1.0);
#endif

#ifdef STANDARD_MATERIAL_CLEARCOAT
    let clearcoat_N = (*input).layers[LAYER_CLEARCOAT].N;
    let clearcoat_strength = (*input).clearcoat_strength;
    let clearcoat_roughness = (*input).layers[LAYER_CLEARCOAT].roughness;

    var derived_clearcoat_input = derive_lighting_input(clearcoat_N, V, L);

    let Fc_Frc =
        specular_clearcoat(input, &derived_clearcoat_input, clearcoat_strength, clearcoat_roughness, 1.0);
    let inv_Fc = 1.0 - Fc_Frc.r;
    let Frc = Fc_Frc.g;
#endif

    var color: vec3<f32>;
#ifdef STANDARD_MATERIAL_CLEARCOAT

    color = (diffuse + specular_light * inv_Fc) * inv_Fc * derived_input.NdotL +
        Frc * derived_clearcoat_input.NdotL;
#else
    color = (diffuse + specular_light) * derived_input.NdotL;
#endif

    var texture_sample = 1f;

#ifdef LIGHT_TEXTURES
    if (*light).decal_index != 0xFFFFFFFFu {
        let local_position = (view_bindings::clustered_decals.decals[(*light).decal_index].local_from_world *
            vec4((*input).P, 1.0)).xyz;
        let decal_uv = local_position.xy * vec2(-0.5, 0.5) + 0.5;

        if (view_bindings::clustered_decals.decals[(*light).decal_index].tag != 0u)
                || all(clamp(decal_uv, vec2(0.0), vec2(1.0)) == decal_uv)
        {
            let image_index = view_bindings::clustered_decals.decals[(*light).decal_index].base_color_texture_index;

            texture_sample = textureSampleLevel(
                view_bindings::clustered_decal_textures[image_index],
                view_bindings::clustered_decal_sampler,
                decal_uv - floor(decal_uv),
                0.0
            ).r;
        } else {
            texture_sample = 0f;
        }
    }
#endif

color *= (*light).color.rgb * texture_sample;

#ifdef ATMOSPHERE
    let P = (*input).P;
    let atmosphere = view_bindings::atmosphere;
    let P_as = (
        view_bindings::atmosphere.world_to_atmosphere * vec4(P, 1.0)
    ).xyz;
    let P_clamped = clamp_to_surface(atmosphere, P_as);
    let r = length(P_clamped);
    let local_up = normalize(P_clamped);
    let mu_light = dot(L, local_up);

    let transmittance = sample_transmittance_lut(r, mu_light);
    let sun_visibility = calculate_visible_sun_ratio(atmosphere, r, mu_light, (*light).sun_disk_angular_size);

    color *= transmittance * sun_visibility;
#endif

    return color;
}

fn ltc_integrate_edge(v1: vec3<f32>, v2: vec3<f32>) -> f32 {
    let x = dot(v1, v2);
    let y = abs(x);
    let a = 0.8543985 + (0.4965155 + 0.0145206 * y) * y;
    let b = 3.4175940 + (4.1616724 + y) * y;
    let v = a / b;
    let theta_sintheta = select(0.5 * inverseSqrt(max(1.0 - x * x, 1e-7)) - v, v, x > 0.0);
    return cross(v1, v2).z * theta_sintheta;
}

fn ltc_integrate_quad(
    N: vec3<f32>,
    V: vec3<f32>,
    P: vec3<f32>,
    Minv: mat3x3<f32>,
    points: array<vec3<f32>, 4>
) -> f32 {
    let T1 = normalize(V - N * dot(V, N));
    let T2 = -cross(N, T1);

    let Minv_local = Minv * transpose(mat3x3<f32>(T1, T2, N));

    var L: array<vec3<f32>, 4>;
    L[0] = Minv_local * (points[0] - P);
    L[1] = Minv_local * (points[1] - P);
    L[2] = Minv_local * (points[2] - P);
    L[3] = Minv_local * (points[3] - P);

    var clipped: array<vec3<f32>, 5>;
    var n_clipped: i32 = 0;

    for (var i = 0i; i < 4i; i++) {
        let a = L[i];
        let b = L[(i + 1) % 4];
        if (a.z >= 0.0) {
            clipped[n_clipped] = a;
            n_clipped++;
        }
        if ((a.z >= 0.0) != (b.z >= 0.0)) {
            let t = a.z / (a.z - b.z);
            clipped[n_clipped] = mix(a, b, t);
            n_clipped++;
        }
    }

    if (n_clipped == 0) {
        return 0.0;
    }

    for (var i = 0i; i < n_clipped; i++) {
        clipped[i] = normalize(clipped[i]);
    }

    var sum = 0.0;
    for (var i = 0i; i < n_clipped; i++) {
        sum += ltc_integrate_edge(clipped[i], clipped[(i + 1) % n_clipped]);
    }

    return sum;
}

#ifdef AREA_LIGHT_LUTS
fn rect_light(
    light_id: u32,
    input: ptr<function, LightingInput>,
    enable_diffuse: bool,
) -> vec3<f32> {

    let diffuse_color = (*input).diffuse_color;
    let P = (*input).P;
    let N = (*input).layers[LAYER_BASE].N;
    let V = (*input).V;
    let NdotV = (*input).layers[LAYER_BASE].NdotV;
    let perceptual_roughness = (*input).layers[LAYER_BASE].perceptual_roughness;

    let light = &view_bindings::lights.rect_lights[light_id];
    let light_to_frag = (*light).position - P;
    let distance_square = dot(light_to_frag, light_to_frag);
    let inverse_range_squared = 1.0 / max((*light).range * (*light).range, 0.0001);
    let range_falloff = getRangeFalloff(distance_square, inverse_range_squared);

    let light_normal = cross((*light).up, (*light).right);
    let hw = (*light).right * (*light).width  * 0.5;
    let hh = (*light).up   * (*light).height * 0.5;
    var corners: array<vec3<f32>, 4>;
    corners[0] = (*light).position + hw - hh;
    corners[1] = (*light).position - hw - hh;
    corners[2] = (*light).position - hw + hh;
    corners[3] = (*light).position + hw + hh;

    if dot(light_normal, P - corners[0]) <= 0.0 {
        return vec3<f32>(0.0);
    }

    let LUT_SCALE = 63.0 / 64.0;
    let LUT_BIAS  =  0.5 / 64.0;
    let uv = vec2<f32>(perceptual_roughness, sqrt(1.0 - NdotV)) * LUT_SCALE + LUT_BIAS;
    let t1 = textureSampleLevel(view_bindings::area_light_luts, view_bindings::area_light_luts_sampler, uv, 0, 0.0);
    let t2 = textureSampleLevel(view_bindings::area_light_luts, view_bindings::area_light_luts_sampler, uv, 1, 0.0);

    let Minv = mat3x3<f32>(
        vec3<f32>(t1.x, 0.0, t1.y),
        vec3<f32>(0.0,  1.0, 0.0),
        vec3<f32>(t1.z, 0.0, t1.w),
    );
    let spec = ltc_integrate_quad(N, V, P, Minv, corners);

    let identity = mat3x3<f32>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0),
    );
    let diff = select(0.0, ltc_integrate_quad(N, V, P, identity, corners), enable_diffuse);

    let F0 = mix((*input).F0_dielectric, (*input).F0_metallic, (*input).metallic);
    let spec_weight = F0 * t2.x + (1.0 - F0) * t2.y;

#ifdef STANDARD_MATERIAL_CLEARCOAT
    let clearcoat_N = (*input).layers[LAYER_CLEARCOAT].N;
    let clearcoat_strength = (*input).clearcoat_strength;
    let clearcoat_perceptual_roughness = (*input).layers[LAYER_CLEARCOAT].perceptual_roughness;
    let clearcoat_NdotV = (*input).layers[LAYER_CLEARCOAT].NdotV;

    let cc_uv = vec2<f32>(clearcoat_perceptual_roughness, sqrt(1.0 - clearcoat_NdotV)) * LUT_SCALE + LUT_BIAS;
    let tc1 = textureSampleLevel(view_bindings::area_light_luts, view_bindings::area_light_luts_sampler, cc_uv, 0, 0.0);
    let tc2 = textureSampleLevel(view_bindings::area_light_luts, view_bindings::area_light_luts_sampler, cc_uv, 1, 0.0);
    let Minv_cc = mat3x3<f32>(
        vec3<f32>(tc1.x, 0.0, tc1.y),
        vec3<f32>(0.0,   1.0, 0.0),
        vec3<f32>(tc1.z, 0.0, tc1.w),
    );
    let spec_cc = ltc_integrate_quad(clearcoat_N, V, P, Minv_cc, corners);

    let spec_weight_cc = 0.04 * tc2.x + (1.0 - 0.04) * tc2.y;
    let Fc = clearcoat_strength * spec_weight_cc;
    let inv_Fc = 1.0 - Fc;

    return ((spec_weight * spec * inv_Fc + diffuse_color * diff) * inv_Fc
        + spec_weight_cc * spec_cc * clearcoat_strength) * (*light).color.rgb * range_falloff;
#else
    return (spec_weight * spec + diffuse_color * diff) * (*light).color.rgb * range_falloff;
#endif
}
#endif

#ifdef ATMOSPHERE
fn sample_transmittance_lut(r: f32, mu: f32) -> vec3<f32> {
    let uv = transmittance_lut_r_mu_to_uv(view_bindings::atmosphere, r, mu);
    return textureSampleLevel(
        view_bindings::atmosphere_transmittance_texture,
        view_bindings::atmosphere_transmittance_sampler, uv, 0.0).rgb;
}
#endif
