use bevy::prelude::*;
use meteor_asset::pos::AttackBlock;
use meteor_asset::res::WeaponCombatStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectileKind {
    Dart,
    GunRay,
    FlyingWheel,
}

#[derive(Debug, Clone)]
pub struct RangedHitSource {
    pub owner: Entity,
    pub weapon: u8,
    pub weapon_stats: Option<WeaponCombatStats>,
    pub pose_id: u16,
    pub pose_frame: f32,
    pub pose_start: u32,
    pub pose_instance_id: u32,
    pub attack_index: u32,
    pub attack: AttackBlock,
    pub player_slot: u8,
    pub combat_team: i32,
    pub script_character_id: i64,
    pub script_team: Option<i64>,
    pub attack_origin: Vec3,
    pub attack_forward: Vec3,
}

#[derive(Component, Debug, Clone)]
pub struct Projectile {
    pub kind: ProjectileKind,
    pub source: RangedHitSource,
    pub velocity: Vec3,
    pub age: f32,
    pub hit_scale: f32,

    pub speed: f32,
    pub launch_point: Vec3,
    pub guide_target: Option<Entity>,

    pub guide_initialized: bool,
    pub route_to_target: bool,
    pub collision_return: bool,
}

#[derive(Debug, Clone)]
pub struct RangedSegmentHit {
    pub projectile: Entity,
    pub source: RangedHitSource,
    pub start: Vec3,
    pub end: Vec3,
    pub hit_scale: f32,
    pub stop_after_first: bool,
    pub despawn_after: bool,
}

#[derive(Resource, Default)]
pub struct PendingRangedHits(pub Vec<RangedSegmentHit>);
