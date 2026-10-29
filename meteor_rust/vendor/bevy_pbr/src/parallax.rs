use bevy_reflect::{std_traits::ReflectDefault, Reflect};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Reflect)]
#[reflect(Default, Clone, PartialEq)]
pub enum ParallaxMappingMethod {

    #[default]
    Occlusion,

    Relief {

        max_steps: u32,
    },
}

impl ParallaxMappingMethod {

    pub const DEFAULT_RELIEF_MAPPING: Self = ParallaxMappingMethod::Relief { max_steps: 5 };

    pub(crate) fn max_steps(&self) -> u32 {
        match self {
            ParallaxMappingMethod::Occlusion => 0,
            ParallaxMappingMethod::Relief { max_steps } => *max_steps,
        }
    }
}
