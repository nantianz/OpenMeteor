use bevy::prelude::*;

#[derive(Component, Debug, Clone)]
pub struct StageObject {
    pub object_id: u32,
    pub name: String,
}

#[derive(Component, Debug, Clone)]
pub struct ScriptInstance {
    pub pc: usize,

    pub running: bool,
}

#[derive(Component, Debug, Clone)]
pub struct WallCollider;

#[derive(Component, Debug, Clone)]
pub struct GroundCollider;

#[derive(Component, Debug, Clone)]
pub struct HazardZone {
    pub damage_per_tick: f32,
    pub hazard_type: HazardType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HazardType {
    Fire,
    Poison,
    Spawn,
}
