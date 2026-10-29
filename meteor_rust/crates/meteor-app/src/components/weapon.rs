use bevy::prelude::*;
use meteor_core::WeaponId;

#[derive(Component, Debug, Clone)]
pub struct WeaponSlot {
    pub equipped: Option<WeaponInstance>,
}

#[derive(Debug, Clone)]
pub struct WeaponInstance {
    pub weapon_id: WeaponId,

    pub bone_index: usize,

    pub entity: Option<Entity>,
}
