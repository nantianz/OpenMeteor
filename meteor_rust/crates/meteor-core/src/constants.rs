pub const MODEL_SCALE: f32 = 0.12;

pub const DES_SCALE: f32 = 0.035;

pub const GRAVITY: f32 = 980.0;

pub const SLOPE_THRESHOLD: f32 = 0.42;

pub const SEAM_RADIUS: f32 = 0.15;

pub const SEAM_DROP: f32 = 1.2;

pub const TICK_DURATION: f32 = 1.0 / 30.0;

pub const ANIM_FPS: f32 = 30.0;

pub const MAX_FRAME_DT: f32 = 0.05;

pub const MAX_COMBO_STAGE: usize = 3;

pub const MAX_WEAPON_SLOTS: usize = 2;

pub const WEAPON_FIST: u8 = 0;
pub const WEAPON_SWORD: u8 = 1;
pub const WEAPON_BLADE: u8 = 2;
pub const WEAPON_SPEAR: u8 = 3;
pub const WEAPON_DAGGER: u8 = 4;
pub const WEAPON_HAMMER: u8 = 5;
pub const WEAPON_FLY_WHEEL: u8 = 6;
pub const WEAPON_DART: u8 = 7;
pub const WEAPON_GUN: u8 = 8;
pub const WEAPON_DOUBLE_DAGGER: u8 = 9;
pub const WEAPON_GREAT_SWORD: u8 = 10;
pub const WEAPON_CLAW: u8 = 11;
pub const WEAPON_HIDDEN: u8 = 12;

pub const CHAR_MENG: u8 = 0;
pub const CHAR_LENG: u8 = 1;
pub const CHAR_TIE: u8 = 2;
pub const CHAR_YE: u8 = 3;
pub const CHAR_GAO: u8 = 4;
pub const CHAR_XIAO: u8 = 5;
pub const CHAR_ZI: u8 = 6;
pub const CHAR_FENG: u8 = 7;
pub const CHAR_WANG: u8 = 8;
pub const CHAR_WU: u8 = 9;
pub const CHAR_SHI: u8 = 10;
pub const CHAR_FAN: u8 = 11;
pub const CHAR_LU: u8 = 12;

pub const OFFICIAL_STAGES: &[u8] = &[
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22,
];

pub const CUSTOM_STAGE_MIN: u8 = 23;
pub const CUSTOM_STAGE_MAX: u8 = 99;

pub const DEFAULT_HP: f32 = 1000.0;

pub const DEFAULT_QI_MAX: f32 = 100.0;

pub const QI_BURST_ANGER_COST: f32 = 50.0;

pub const DEFAULT_ATTACK_WINDOW: u32 = 30;
