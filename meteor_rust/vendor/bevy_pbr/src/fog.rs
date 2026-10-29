use bevy_camera::Camera;
use bevy_color::{Color, ColorToComponents, LinearRgba};
use bevy_ecs::prelude::*;
use bevy_math::{ops, Vec3};
use bevy_reflect::{std_traits::ReflectDefault, Reflect};
use bevy_render::extract_component::ExtractComponent;

use crate::ViewFogUniformOffset;

#[derive(Debug, Clone, Component, Reflect, ExtractComponent)]
#[extract_component_filter(With<Camera>)]
#[extract_component_sync_target((Self, ViewFogUniformOffset))]
#[reflect(Component, Default, Debug, Clone)]
pub struct DistanceFog {

    pub color: Color,

    pub directional_light_color: Color,

    pub directional_light_exponent: f32,

    pub falloff: FogFalloff,
}

#[derive(Debug, Clone, Reflect)]
#[reflect(Clone)]
pub enum FogFalloff {

    Linear {

        start: f32,

        end: f32,
    },

    Exponential {

        density: f32,
    },

    ExponentialSquared {

        density: f32,
    },

    Atmospheric {

        extinction: Vec3,

        inscattering: Vec3,
    },
}

impl FogFalloff {

    pub fn from_visibility(visibility: f32) -> FogFalloff {
        FogFalloff::from_visibility_contrast(
            visibility,
            FogFalloff::REVISED_KOSCHMIEDER_CONTRAST_THRESHOLD,
        )
    }

    pub fn from_visibility_contrast(visibility: f32, contrast_threshold: f32) -> FogFalloff {
        FogFalloff::Exponential {
            density: FogFalloff::koschmieder(visibility, contrast_threshold),
        }
    }

    pub fn from_visibility_squared(visibility: f32) -> FogFalloff {
        FogFalloff::from_visibility_contrast_squared(
            visibility,
            FogFalloff::REVISED_KOSCHMIEDER_CONTRAST_THRESHOLD,
        )
    }

    pub fn from_visibility_contrast_squared(
        visibility: f32,
        contrast_threshold: f32,
    ) -> FogFalloff {
        FogFalloff::ExponentialSquared {
            density: (FogFalloff::koschmieder(visibility, contrast_threshold) / visibility).sqrt(),
        }
    }

    pub fn from_visibility_color(
        visibility: f32,
        extinction_inscattering_color: Color,
    ) -> FogFalloff {
        FogFalloff::from_visibility_contrast_colors(
            visibility,
            FogFalloff::REVISED_KOSCHMIEDER_CONTRAST_THRESHOLD,
            extinction_inscattering_color,
            extinction_inscattering_color,
        )
    }

    pub fn from_visibility_colors(
        visibility: f32,
        extinction_color: Color,
        inscattering_color: Color,
    ) -> FogFalloff {
        FogFalloff::from_visibility_contrast_colors(
            visibility,
            FogFalloff::REVISED_KOSCHMIEDER_CONTRAST_THRESHOLD,
            extinction_color,
            inscattering_color,
        )
    }

    pub fn from_visibility_contrast_color(
        visibility: f32,
        contrast_threshold: f32,
        extinction_inscattering_color: Color,
    ) -> FogFalloff {
        FogFalloff::from_visibility_contrast_colors(
            visibility,
            contrast_threshold,
            extinction_inscattering_color,
            extinction_inscattering_color,
        )
    }

    pub fn from_visibility_contrast_colors(
        visibility: f32,
        contrast_threshold: f32,
        extinction_color: Color,
        inscattering_color: Color,
    ) -> FogFalloff {
        use core::f32::consts::E;

        let [r_e, g_e, b_e, a_e] = LinearRgba::from(extinction_color).to_f32_array();
        let [r_i, g_i, b_i, a_i] = LinearRgba::from(inscattering_color).to_f32_array();

        FogFalloff::Atmospheric {
            extinction: Vec3::new(

                ops::powf(1.0 - r_e, E),
                ops::powf(1.0 - g_e, E),
                ops::powf(1.0 - b_e, E),
            ) * FogFalloff::koschmieder(visibility, contrast_threshold)
                * ops::powf(a_e, E),

            inscattering: Vec3::new(ops::powf(r_i, E), ops::powf(g_i, E), ops::powf(b_i, E))
                * FogFalloff::koschmieder(visibility, contrast_threshold)
                * ops::powf(a_i, E),
        }
    }

    pub const REVISED_KOSCHMIEDER_CONTRAST_THRESHOLD: f32 = 0.05;

    pub fn koschmieder(v: f32, c_t: f32) -> f32 {
        -ops::ln(c_t) / v
    }
}

impl Default for DistanceFog {
    fn default() -> Self {
        DistanceFog {
            color: Color::WHITE,
            falloff: FogFalloff::Linear {
                start: 0.0,
                end: 100.0,
            },
            directional_light_color: Color::NONE,
            directional_light_exponent: 8.0,
        }
    }
}
