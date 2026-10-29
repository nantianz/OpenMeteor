pub type PoseId = u16;

pub type WeaponId = u8;

pub type CharacterId = u8;

pub type StageId = u8;

pub type FrameCount = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FighterState {
    Name,

    View,

    Think,

    Attack1,

    Attack2,

    Attack3,

    Guard,

    Dodge,

    Jump,

    Look,

    Burst,

    Cure,

    GetItem,

    Spawn,

    BeHit,

    Down,

    Fake,

    Dead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovementMode {
    Idle,

    Walk,

    Run,

    Crouch,

    Airborne,

    WallRun,

    Dodge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackType {
    Normal,
    BreakGuard,
    Unblockable,
    Grab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageType {
    Slash,
    Pierce,
    Blunt,
    Fire,
    Poison,
}
