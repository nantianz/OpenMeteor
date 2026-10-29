#define_import_path bevy_pbr::atmosphere::types

struct Atmosphere {
    ground_albedo: vec3<f32>,

    inner_radius: f32,

    outer_radius: f32,

    world_to_atmosphere: mat4x4<f32>,
}

struct AtmosphereSettings {
    transmittance_lut_size: vec2<u32>,
    multiscattering_lut_size: vec2<u32>,
    sky_view_lut_size: vec2<u32>,
    aerial_view_lut_size: vec3<u32>,
    transmittance_lut_samples: u32,
    multiscattering_lut_dirs: u32,
    multiscattering_lut_samples: u32,
    sky_view_lut_samples: u32,
    aerial_view_lut_samples: u32,
    aerial_view_lut_max_distance: f32,
    sky_max_samples: u32,
    rendering_method: u32,
}

struct AtmosphereTransforms {
    world_from_atmosphere: mat4x4<f32>,
    atmosphere_from_world: mat4x4<f32>,
}
