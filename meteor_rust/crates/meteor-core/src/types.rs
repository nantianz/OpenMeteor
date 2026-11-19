pub type PoseId = u16;

pub type WeaponId = u8;

pub fn weapon_kind_and_variant_from_item(item_id: u32) -> Option<(WeaponId, u8)> {
    match item_id {
        1..=45 => Some((((item_id - 1) % 9 + 1) as u8, ((item_id - 1) / 9) as u8)),
        46 => Some((1, 5)),
        47..=50 => Some((10, (item_id - 47) as u8)),
        51..=54 => Some((11, (item_id - 51) as u8)),
        55..=58 => Some((12, (item_id - 55) as u8)),
        59 => Some((2, 5)),
        _ => None,
    }
}

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
