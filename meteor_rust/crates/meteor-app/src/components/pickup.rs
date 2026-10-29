use bevy::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickupKind {
    HpSmall,
    HpLarge,
    QiSmall,
    QiLarge,
    Weapon(u8),
    Anger,
}

#[derive(Component, Debug, Clone)]
pub struct PickupItem {
    pub kind: PickupKind,
    pub value: f32,
}
