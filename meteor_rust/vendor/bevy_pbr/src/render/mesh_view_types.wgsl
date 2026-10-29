#define_import_path bevy_pbr::mesh_view_types

struct ClusteredLight {

    light_custom_data: vec4<f32>,
    color_inverse_square_range: vec4<f32>,
    position_radius: vec4<f32>,

    flags: u32,
    shadow_depth_bias: f32,
    shadow_normal_bias: f32,
    spot_light_tan_angle: f32,
    soft_shadow_size: f32,
    shadow_map_near_z: f32,
    decal_index: u32,
    range: f32,
};

const POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT: u32                    = 1u << 0u;
const POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE: u32                  = 1u << 1u;
const POINT_LIGHT_FLAGS_VOLUMETRIC_BIT: u32                         = 1u << 2u;
const POINT_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE_BIT: u32   = 1u << 3u;
const POINT_LIGHT_FLAGS_CONTACT_SHADOWS_ENABLED_BIT: u32            = 1u << 4u;
const POINT_LIGHT_FLAGS_SPOT_LIGHT_BIT: u32                         = 1u << 5u;

struct DirectionalCascade {
    clip_from_world: mat4x4<f32>,
    texel_size: f32,
    far_bound: f32,
}

struct DirectionalLight {
    cascades: array<DirectionalCascade, #{MAX_CASCADES_PER_LIGHT}>,
    color: vec4<f32>,
    direction_to_light: vec3<f32>,

    flags: u32,
    soft_shadow_size: f32,
    shadow_depth_bias: f32,
    shadow_normal_bias: f32,
    num_cascades: u32,
    cascades_overlap_proportion: f32,
    depth_texture_base_index: u32,
    decal_index: u32,
    sun_disk_angular_size: f32,
    sun_disk_intensity: f32,
};

const DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT: u32                  = 1u << 0u;
const DIRECTIONAL_LIGHT_FLAGS_VOLUMETRIC_BIT: u32                       = 1u << 1u;
const DIRECTIONAL_LIGHT_FLAGS_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE_BIT: u32 = 1u << 2u;
const DIRECTIONAL_LIGHT_FLAGS_CONTACT_SHADOWS_ENABLED_BIT: u32           = 1u << 3u;

struct RectLight {
    color: vec4<f32>,
    position: vec3<f32>,
    width: f32,
    right: vec3<f32>,
    height: f32,
    up: vec3<f32>,
    range: f32,
};

struct Lights {

    directional_lights: array<DirectionalLight, #{MAX_DIRECTIONAL_LIGHTS}u>,
    ambient_color: vec4<f32>,

    cluster_dimensions: vec4<u32>,

    cluster_factors: vec4<f32>,
    n_directional_lights: u32,
    spot_light_shadowmap_offset: i32,
    ambient_light_affects_lightmapped_meshes: u32,
    n_rect_lights: u32,
    rect_lights: array<RectLight, #{MAX_RECT_LIGHTS}u>,
};

struct Fog {
    base_color: vec4<f32>,
    directional_light_color: vec4<f32>,

    be: vec3<f32>,
    directional_light_exponent: f32,
    bi: vec3<f32>,
    mode: u32,
}

const FOG_MODE_OFF: u32                   = 0u;
const FOG_MODE_LINEAR: u32                = 1u;
const FOG_MODE_EXPONENTIAL: u32           = 2u;
const FOG_MODE_EXPONENTIAL_SQUARED: u32   = 3u;
const FOG_MODE_ATMOSPHERIC: u32           = 4u;

#if AVAILABLE_STORAGE_BUFFER_BINDINGS >= 3
struct ClusteredLights {
    data: array<ClusteredLight>,
};
struct ClusterableObjectIndexLists {
    data: array<u32>,
};
struct ClusterOffsetsAndCounts {
    data: array<array<vec4<u32>, 2>>,
};
#else
struct ClusteredLights {
    data: array<ClusteredLight, 204u>,
};
struct ClusterableObjectIndexLists {

    data: array<vec4<u32>, 1024u>,
};
struct ClusterOffsetsAndCounts {

    data: array<vec4<u32>, 1024u>,
};
#endif

const LIGHT_PROBE_FLAG_AFFECTS_LIGHTMAPPED_MESH_DIFFUSE: u32 = 1;

const LIGHT_PROBE_FLAG_PARALLAX_CORRECT:                 u32 = 2;

struct LightProbe {

    light_from_world_transposed: mat3x4<f32>,

    falloff: vec3<f32>,
    bounding_sphere_radius: f32,

    parallax_correction_bounds: vec3<f32>,
    intensity: f32,
    world_position: vec3<f32>,
    cubemap_index: i32,

    flags: u32,
};

struct LightProbes {

    reflection_probes: array<LightProbe, 8u>,
    irradiance_volumes: array<LightProbe, 8u>,
    reflection_probe_count: i32,
    irradiance_volume_count: i32,

    view_cubemap_index: i32,

    smallest_specular_mip_level_for_view: u32,
    view_rotation: vec4<f32>,

    intensity_for_view: f32,

    view_environment_map_affects_lightmapped_mesh_diffuse: u32,
};

struct ScreenSpaceReflectionsSettings {
    min_perceptual_roughness: f32,
    min_perceptual_roughness_fully_active: f32,
    max_perceptual_roughness_starts_to_fade: f32,
    max_perceptual_roughness: f32,
    edge_fadeout_fully_active: f32,
    edge_fadeout_no_longer_active: f32,
    thickness: f32,
    linear_steps: u32,
    linear_march_exponent: f32,
    bisection_steps: u32,
    use_secant: u32,
};

struct ContactShadowsSettings {
    linear_steps: u32,
    thickness: f32,
    length: f32,
#ifdef SIXTEEN_BYTE_ALIGNMENT
    _padding: f32,
#endif
};

struct EnvironmentMapUniform {

    transform: mat4x4<f32>,
};

struct OrderIndependentTransparencySettings {
  sorted_fragment_max_count: u32,
  fragments_per_pixel_average: f32,
  alpha_threshold: f32,
};

struct OitFragmentNode {
    color: u32,
    depth_alpha: u32,
    next: u32,
};

struct ClusteredDecal {
    local_from_world: mat4x4<f32>,
    base_color_texture_index: i32,
    normal_map_texture_index: i32,
    metallic_roughness_texture_index: i32,
    emissive_texture_index: i32,
    world_position: vec3<f32>,
    bounding_sphere_radius: f32,
    tag: u32,
    pad_a: u32,
    pad_b: u32,
    pad_c: u32,
}

struct ClusteredDecals {
    decals: array<ClusteredDecal>,
}
