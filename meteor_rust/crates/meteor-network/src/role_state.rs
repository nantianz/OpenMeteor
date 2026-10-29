use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyEffect {
    pub actor_id: u32,
    pub category: u32,
    pub resource_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoseChain {
    pub pose_id: u16,
    pub minimum_seconds: f32,
    pub sequence: u16,
    pub blend_seconds: f32,
}

impl PoseChain {
    pub fn is_valid(self) -> bool {
        self.pose_id <= 2048
            && self.minimum_seconds.is_finite()
            && (0.0..=327.67).contains(&self.minimum_seconds)
            && self.blend_seconds.is_finite()
            && (0.0..=327.67).contains(&self.blend_seconds)
    }
}

pub fn pose_sequence_is_newer(sequence: u16, previous: u16) -> bool {
    let distance = sequence.wrapping_sub(previous);
    distance != 0 && distance < 0x8000
}

pub const REQUEST1_STATE_BYTES: usize = 20;
pub const EVENT2_STATE_BYTES: usize = 14;
pub const ROLE_UPLINK_HZ: u32 = 20;
pub const ROLE_DOWNLINK_HZ: u32 = 16;
pub const ROLE_WORLD_SCALE: f32 = 0.12;

const ROLE_FAST_SPEED_SQ: f32 = 100.0 * ROLE_WORLD_SCALE * ROLE_WORLD_SCALE;
const ROLE_CORRECTION_SPEED_SQ: f32 = 10.0 * ROLE_WORLD_SCALE * ROLE_WORLD_SCALE;
const ROLE_CORRECTION_NEAR_DISTANCE_SQ: f32 = 36.0 * ROLE_WORLD_SCALE * ROLE_WORLD_SCALE;
const ROLE_CORRECTION_SNAP_DISTANCE_SQ: f32 = 40_000.0 * ROLE_WORLD_SCALE * ROLE_WORLD_SCALE;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RoleVec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl RoleVec3 {
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    fn finite_or_zero(self) -> Self {
        Self {
            x: finite(self.x, 0.0),
            y: finite(self.y, 0.0),
            z: finite(self.z, 0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateWords {
    pub state_word0: u16,
    pub state_word1: u16,
    pub pose_id: u16,
    pub grounded: bool,
    pub guard_route: bool,
    pub playback_arg: i8,
    pub pose_sequence: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoleState {
    pub position: RoleVec3,

    pub forward: RoleVec3,
    pub velocity: RoleVec3,
    pub state_words: StateWords,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoleStateRuntime {
    pub position: RoleVec3,
    pub forward: RoleVec3,
    pub velocity: RoleVec3,
    pub pose_id: u16,
    pub grounded: bool,
    pub guard_route: bool,
    pub playback_arg: i8,
    pub pose_sequence: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemotePoseRuntime {
    pub pose_id: u16,
    pub ground_sequence: u16,
    pub applied_sequence: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoseTransition {
    pub apply: bool,
    pub ground_sequence: u16,
    pub applied_sequence: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StalePoseRecovery {
    pub clear_active: bool,
    pub restore_pose_id: Option<u16>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RoleStateError {
    #[error("role state requires {expected} bytes, received {actual}")]
    InvalidLength { expected: usize, actual: usize },
}

pub fn pack_state_word0(pose_id: u16, grounded: bool, guard_route: bool, playback_arg: i8) -> u16 {
    let pose = pose_id.min(0x07ff);
    let mut word = pose;
    if grounded {
        word |= 0x0800;
    }
    if guard_route {
        word |= 0x1000;
    }
    word | (((playback_arg as i16 as u16) & 0x0007) << 13)
}

pub fn unpack_state_words(state_word0: u16, state_word1: u16) -> StateWords {
    let signed_three_bit = ((state_word0 >> 13) & 0x0007) as i8;
    StateWords {
        state_word0,
        state_word1,
        pose_id: state_word0 & 0x07ff,
        grounded: state_word0 & 0x0800 != 0,
        guard_route: state_word0 & 0x1000 != 0,
        playback_arg: if signed_three_bit >= 4 {
            signed_three_bit - 8
        } else {
            signed_three_bit
        },
        pose_sequence: state_word1,
    }
}

pub fn role_state_from_runtime(runtime: RoleStateRuntime) -> RoleState {
    let forward = normalize_forward(runtime.forward);
    RoleState {
        position: runtime.position.finite_or_zero(),
        forward,
        velocity: runtime.velocity.finite_or_zero(),
        state_words: unpack_state_words(
            pack_state_word0(
                runtime.pose_id,
                runtime.grounded,
                runtime.guard_route,
                runtime.playback_arg,
            ),
            runtime.pose_sequence,
        ),
    }
}

pub fn yaw_from_forward(forward: RoleVec3) -> f32 {
    let forward = normalize_forward(forward);
    forward.x.atan2(forward.z)
}

pub fn forward_from_yaw(yaw: f32) -> RoleVec3 {
    let yaw = finite(yaw, 0.0);
    RoleVec3 {
        x: yaw.sin(),
        y: 0.0,
        z: yaw.cos(),
    }
}

pub fn encode_request1_state(state: RoleState) -> [u8; REQUEST1_STATE_BYTES] {
    let state = canonical_state(state);
    let mut bytes = [0_u8; REQUEST1_STATE_BYTES];
    let fields = [
        meteor_quantize(meteor_units(state.position.x), 10.0, 0.0),
        meteor_quantize(meteor_units(state.position.z), 10.0, 0.0),
        meteor_quantize(meteor_units(state.position.y), 10.0, 0.0),
        meteor_quantize(state.forward.x, 1000.0, 0.0),
        meteor_quantize(state.forward.z, 1000.0, 0.0),
        meteor_quantize(meteor_units(state.velocity.x), 50.0, 0.0),
        meteor_quantize(meteor_units(state.velocity.z), 50.0, 0.0),
        meteor_quantize(meteor_units(state.velocity.y), 50.0, 0.0),
    ];
    for (index, value) in fields.into_iter().enumerate() {
        let value = value.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        bytes[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes[16..18].copy_from_slice(&state.state_words.state_word0.to_le_bytes());
    bytes[18..20].copy_from_slice(&state.state_words.state_word1.to_le_bytes());
    bytes
}

pub fn decode_request1_state(bytes: &[u8]) -> Result<RoleState, RoleStateError> {
    if bytes.len() != REQUEST1_STATE_BYTES {
        return Err(RoleStateError::InvalidLength {
            expected: REQUEST1_STATE_BYTES,
            actual: bytes.len(),
        });
    }
    let i16_at = |offset: usize| i16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let u16_at = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    Ok(RoleState {
        position: RoleVec3 {
            x: web_units(i16_at(0) as f32 * 0.1),
            y: web_units(i16_at(4) as f32 * 0.1),
            z: web_units(i16_at(2) as f32 * 0.1),
        },
        forward: normalize_forward(RoleVec3 {
            x: i16_at(6) as f32 * 0.001,
            y: 0.0,
            z: i16_at(8) as f32 * 0.001,
        }),
        velocity: RoleVec3 {
            x: web_units(i16_at(10) as f32 * 0.02),
            y: web_units(i16_at(14) as f32 * 0.02),
            z: web_units(i16_at(12) as f32 * 0.02),
        },
        state_words: unpack_state_words(u16_at(16), u16_at(18)),
    })
}

pub fn encode_event2_state(state: RoleState) -> [u8; EVENT2_STATE_BYTES] {
    let state = canonical_state(state);
    let mut writer = BitWriter::default();
    writer.write(
        clamp_u32(
            meteor_quantize(meteor_units(state.position.x), 2.0, 8191.0),
            0,
            0x3fff,
        ),
        14,
    );
    writer.write(
        clamp_u32(
            meteor_quantize(meteor_units(state.position.z), 2.0, 8191.0),
            0,
            0x3fff,
        ),
        14,
    );
    writer.write(
        clamp_u32(
            meteor_quantize(meteor_units(state.position.y), 2.0, 2047.0),
            0,
            0x0fff,
        ),
        12,
    );
    let meteor_angle = normalize_angle_positive(state.forward.z.atan2(state.forward.x));
    writer.write(
        clamp_u32(meteor_quantize(meteor_angle, 10.0, 0.0), 0, 0x3f),
        6,
    );
    writer.write(
        clamp_u32(
            meteor_quantize(meteor_units(state.velocity.x), 3.5, 1023.0),
            0,
            0x07ff,
        ),
        11,
    );
    writer.write(
        clamp_u32(
            meteor_quantize(meteor_units(state.velocity.z), 3.5, 1023.0),
            0,
            0x07ff,
        ),
        11,
    );
    writer.write(
        clamp_u32(
            meteor_quantize(meteor_units(state.velocity.y), 3.5, 1023.0),
            0,
            0x07ff,
        ),
        11,
    );
    writer.write(state.state_words.state_word0 as u32, 16);
    writer.write(state.state_words.state_word1 as u32, 16);
    writer.bytes
}

pub fn decode_event2_state(bytes: &[u8]) -> Result<RoleState, RoleStateError> {
    if bytes.len() != EVENT2_STATE_BYTES {
        return Err(RoleStateError::InvalidLength {
            expected: EVENT2_STATE_BYTES,
            actual: bytes.len(),
        });
    }
    let mut reader = BitReader::new(bytes);
    let meteor_x = (reader.read(14) as i32 - 8191) as f32 * 0.5;
    let meteor_y = (reader.read(14) as i32 - 8191) as f32 * 0.5;
    let meteor_z = (reader.read(12) as i32 - 2047) as f32 * 0.5;
    let angle = reader.read(6) as f32 * 0.1;
    let velocity_x = (reader.read(11) as i32 - 1023) as f32 * (2.0 / 7.0);
    let velocity_y = (reader.read(11) as i32 - 1023) as f32 * (2.0 / 7.0);
    let velocity_z = (reader.read(11) as i32 - 1023) as f32 * (2.0 / 7.0);
    let state_word0 = reader.read(16) as u16;
    let state_word1 = reader.read(16) as u16;
    Ok(RoleState {
        position: RoleVec3 {
            x: web_units(meteor_x),
            y: web_units(meteor_z),
            z: web_units(meteor_y),
        },
        forward: normalize_forward(RoleVec3 {
            x: angle.cos(),
            y: 0.0,
            z: angle.sin(),
        }),
        velocity: RoleVec3 {
            x: web_units(velocity_x),
            y: web_units(velocity_z),
            z: web_units(velocity_y),
        },
        state_words: unpack_state_words(state_word0, state_word1),
    })
}

pub fn decide_pose_transition(runtime: RemotePoseRuntime, state: RoleState) -> PoseTransition {
    let incoming = state.state_words.pose_sequence;
    if runtime.pose_id == state.state_words.pose_id {
        return PoseTransition {
            apply: false,
            ground_sequence: runtime.ground_sequence,
            applied_sequence: incoming,
        };
    }

    let apply = if state.state_words.grounded {
        i32::from(runtime.ground_sequence as i16) < i32::from(incoming)
    } else {
        runtime.applied_sequence < incoming
    };
    PoseTransition {
        apply,
        ground_sequence: if apply {
            incoming
        } else {
            runtime.ground_sequence
        },
        applied_sequence: if apply {
            incoming
        } else {
            runtime.applied_sequence
        },
    }
}

pub fn remote_velocity_delta(
    state_age: f32,
    hit_age: f32,
    velocity: RoleVec3,
    dt: f32,
) -> RoleVec3 {
    let step = finite(dt, 0.0).max(0.0);
    let state_seconds = finite(state_age, 0.0).max(0.0);
    let hit_seconds = finite(hit_age, 10.0).max(0.0);
    let velocity = velocity.finite_or_zero();
    let speed_sq = velocity.x * velocity.x + velocity.y * velocity.y + velocity.z * velocity.z;
    if step <= 0.0
        || state_seconds > 0.5
        || hit_seconds < 0.4
        || (state_seconds > 0.2 && speed_sq > ROLE_FAST_SPEED_SQ)
    {
        return RoleVec3::ZERO;
    }
    RoleVec3 {
        x: velocity.x * step,
        y: velocity.y * step,
        z: velocity.z * step,
    }
}

pub fn stale_role_pose_recovery(
    state_age: f32,
    animation_complete: bool,
    network_pose_active: bool,
    pose_id: u16,
) -> StalePoseRecovery {
    if finite(state_age, 0.0) <= 0.5 || !animation_complete || !network_pose_active {
        return StalePoseRecovery {
            clear_active: false,
            restore_pose_id: None,
        };
    }
    StalePoseRecovery {
        clear_active: true,
        restore_pose_id: (!matches!(pose_id, 112 | 113)).then_some(0),
    }
}

pub fn role_position_correction_factor(
    distance_sq: f32,
    speed_sq: f32,
    dt: f32,
    hit_age: f32,
) -> f32 {
    let distance = finite(distance_sq, 0.0).max(0.0);
    let speed = finite(speed_sq, 0.0).max(0.0);
    let step = finite(dt, 0.0).max(0.0);
    let hit_seconds = finite(hit_age, 10.0).max(0.0);
    if step <= 0.0 {
        return 0.0;
    }
    if distance > ROLE_CORRECTION_SNAP_DISTANCE_SQ {
        return 1.0;
    }
    let rate = if hit_seconds <= 0.5 {
        let base = if distance > ROLE_CORRECTION_NEAR_DISTANCE_SQ {
            10.0
        } else {
            2.0
        };
        base * 2.0 * hit_seconds
    } else if speed > ROLE_CORRECTION_SPEED_SQ {
        6.0
    } else if distance > ROLE_CORRECTION_NEAR_DISTANCE_SQ {
        20.0
    } else {
        5.0
    };
    (step * rate).min(1.0)
}

pub fn role_yaw_correction_factor(dt: f32, hit_age: f32) -> f32 {
    let step = finite(dt, 0.0).max(0.0);
    let hit_seconds = finite(hit_age, 10.0).max(0.0);
    let hit_scale = if hit_seconds <= 0.5 {
        2.0 * hit_seconds
    } else {
        1.0
    };
    (step * 8.0 * hit_scale).min(1.0)
}

fn canonical_state(state: RoleState) -> RoleState {
    RoleState {
        position: state.position.finite_or_zero(),
        forward: normalize_forward(state.forward),
        velocity: state.velocity.finite_or_zero(),
        state_words: state.state_words,
    }
}

fn normalize_forward(value: RoleVec3) -> RoleVec3 {
    let x = finite(value.x, 0.0);
    let z = finite(value.z, 1.0);
    let length = x.hypot(z);
    if length < 1e-8 {
        RoleVec3 {
            x: 0.0,
            y: 0.0,
            z: 1.0,
        }
    } else {
        RoleVec3 {
            x: x / length,
            y: 0.0,
            z: z / length,
        }
    }
}

fn finite(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

fn meteor_units(value: f32) -> f32 {
    finite(value, 0.0) / ROLE_WORLD_SCALE
}

fn web_units(value: f32) -> f32 {
    finite(value, 0.0) * ROLE_WORLD_SCALE
}

fn meteor_quantize(value: f32, scale: f32, offset: f32) -> i32 {
    (finite(value, 0.0) * scale + offset + 0.5).trunc() as i32
}

fn normalize_angle_positive(value: f32) -> f32 {
    let mut angle = finite(value, 0.0) % TAU;
    if angle < 0.0 {
        angle += TAU;
    }
    angle
}

fn clamp_u32(value: i32, min: u32, max: u32) -> u32 {
    value.clamp(min as i32, max as i32) as u32
}

#[derive(Default)]
struct BitWriter {
    bytes: [u8; EVENT2_STATE_BYTES],
    offset: usize,
}

impl BitWriter {
    fn write(&mut self, value: u32, width: usize) {
        for bit in 0..width {
            if value & (1 << bit) != 0 {
                let offset = self.offset + bit;
                self.bytes[offset >> 3] |= 1 << (offset & 7);
            }
        }
        self.offset += width;
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn read(&mut self, width: usize) -> u32 {
        let mut value = 0_u32;
        for bit in 0..width {
            let offset = self.offset + bit;
            value |= (((self.bytes[offset >> 3] >> (offset & 7)) & 1) as u32) << bit;
        }
        self.offset += width;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> RoleState {
        role_state_from_runtime(RoleStateRuntime {
            position: RoleVec3 {
                x: 1.2,
                y: -0.6,
                z: 2.4,
            },
            forward: RoleVec3 {
                x: 0.6,
                y: 0.0,
                z: 0.8,
            },
            velocity: RoleVec3 {
                x: -0.48,
                y: 0.24,
                z: 0.72,
            },
            pose_id: 24,
            grounded: true,
            guard_route: true,
            playback_arg: -2,
            pose_sequence: 42,
        })
    }

    #[test]
    fn request1_and_event2_match_web_role_state_vectors() {
        let state = sample_state();
        assert_eq!(
            encode_request1_state(state).as_slice(),
            hex_bytes("6400c800cfff5802200339ff2c01640018d82a00").as_slice()
        );
        assert_eq!(
            encode_event2_state(state).as_slice(),
            hex_bytes("13e009587f49fc2868400c6c1500").as_slice()
        );

        let request = decode_request1_state(&encode_request1_state(state)).unwrap();
        approx(request.position.x, 1.2);
        approx(request.position.y, -0.588);
        approx(request.velocity.x, -0.4776);
        assert_eq!(request.state_words.pose_id, 24);
        assert_eq!(request.state_words.playback_arg, -2);

        let event2 = decode_event2_state(&encode_event2_state(state)).unwrap();
        approx(event2.position.x, 1.2);
        approx(event2.position.y, -0.6);
        approx(event2.position.z, 2.4);
        approx(event2.velocity.x, -0.48);
        approx(event2.forward.x, 0.621_609_9);
        assert_eq!(event2.state_words.pose_sequence, 42);
    }

    #[test]
    fn pose_stale_and_correction_gates_match_web() {
        let state = sample_state();
        assert_eq!(
            decide_pose_transition(
                RemotePoseRuntime {
                    pose_id: 1,
                    ground_sequence: 40,
                    applied_sequence: 1,
                },
                state,
            ),
            PoseTransition {
                apply: true,
                ground_sequence: 42,
                applied_sequence: 42,
            }
        );
        assert!(
            !decide_pose_transition(
                RemotePoseRuntime {
                    pose_id: 24,
                    ground_sequence: 40,
                    applied_sequence: 1,
                },
                state,
            )
            .apply
        );
        let airborne = role_state_from_runtime(RoleStateRuntime {
            position: state.position,
            forward: state.forward,
            velocity: state.velocity,
            pose_id: 25,
            grounded: false,
            guard_route: false,
            playback_arg: 0,
            pose_sequence: 1,
        });
        assert!(
            !decide_pose_transition(
                RemotePoseRuntime {
                    pose_id: 1,
                    ground_sequence: 0,
                    applied_sequence: 65_000,
                },
                airborne,
            )
            .apply,
            "airborne sequence comparison is unsigned"
        );
        assert_eq!(
            remote_velocity_delta(
                0.1,
                1.0,
                RoleVec3 {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                0.05,
            ),
            RoleVec3 {
                x: 0.05,
                y: 0.1,
                z: 0.15,
            }
        );
        assert_eq!(
            remote_velocity_delta(
                0.21,
                1.0,
                RoleVec3 {
                    x: 2.0,
                    y: 0.0,
                    z: 0.0
                },
                0.05
            ),
            RoleVec3::ZERO
        );
        approx(role_position_correction_factor(4.0, 1.0, 0.05, 1.0), 0.3);
        assert_eq!(role_position_correction_factor(999.0, 0.0, 0.1, 1.0), 1.0);
        assert_eq!(
            stale_role_pose_recovery(0.6, true, true, 112),
            StalePoseRecovery {
                clear_active: true,
                restore_pose_id: None,
            }
        );
        assert_eq!(role_yaw_correction_factor(0.1, 0.25), 0.4);
    }

    #[test]
    fn grounded_pose_generation_matches_exe_signed_actor_unsigned_packet() {
        for (previous, incoming, expected) in [
            (32767, 32768, true),
            (32768, 32768, true),
            (65535, 0, true),
            (0, 65535, true),
            (42, 42, false),
        ] {
            let state = RoleState {
                state_words: unpack_state_words(24 | (1 << 11), incoming),
                position: RoleVec3::ZERO,
                forward: RoleVec3::ZERO,
                velocity: RoleVec3::ZERO,
            };
            let transition = decide_pose_transition(
                RemotePoseRuntime {
                    pose_id: 0,
                    ground_sequence: previous,
                    applied_sequence: previous,
                },
                state,
            );
            assert_eq!(transition.apply, expected, "{previous} -> {incoming}");
        }
    }

    #[test]
    fn malformed_lengths_are_rejected() {
        assert_eq!(
            decode_request1_state(&[0; 19]),
            Err(RoleStateError::InvalidLength {
                expected: 20,
                actual: 19,
            })
        );
        assert_eq!(
            decode_event2_state(&[0; 13]),
            Err(RoleStateError::InvalidLength {
                expected: 14,
                actual: 13,
            })
        );
    }

    fn hex_bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
            .collect()
    }

    fn approx(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.000_01,
            "expected {expected}, received {actual}"
        );
    }
}
