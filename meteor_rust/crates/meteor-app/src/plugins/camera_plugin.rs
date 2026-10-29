use crate::app_state::{AppState, Settings};
use crate::plugins::stage_collider::StageCollider;
use crate::plugins::stage_plugin::LocalFighter;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::{MouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use meteor_core::MODEL_SCALE;
use meteor_physics::collision::MeshCollider;

mod spectator;

pub struct CameraPlugin;

#[derive(Resource, Default)]
pub struct CameraMode {
    pub fly: bool,
    pub freecam_hold: bool,
}

impl CameraMode {
    pub fn freecam(&self) -> bool {
        self.freecam_hold
    }
}

#[derive(Resource, Default)]
pub struct CameraRecentreRequest(pub bool);

#[derive(Resource, Default)]
pub struct CameraBasis {
    pub yaw: f32,
}

const DEFAULT_DISTANCE: f32 = 10.0;

const ACTION_VIEW_DISTANCE: f32 = 10.0;

const LOOK_HEIGHT: f32 = 3.4;

const DEFAULT_PITCH: f32 = 0.2;

const PITCH_MIN: f32 = -0.5;
const PITCH_MAX: f32 = 1.2;

const DEG: f32 = std::f32::consts::PI / 180.0;

const YAW_SMOOTH: f32 = 10.0;

const FOLLOW_GROUND: f32 = 12.0;
const FOLLOW_AIR: f32 = 9.0;

const COLLISION_SKIN: f32 = 0.3;

const ZOOM_STEP: f32 = 0.5;
const MIN_DISTANCE: f32 = 8.0;
const MAX_DISTANCE: f32 = 12.0;

const LOCK_FOCUS_FRACTION: f32 = 0.25;

const LOCK_FOCUS_HEIGHT: f32 = 24.0 * MODEL_SCALE;

const LOCK_DISTANCE: f32 = 120.0 * MODEL_SCALE;

const LOCK_EYE_HEIGHT: f32 = 10.0 * MODEL_SCALE;

const LOCK_SIDE_REFERENCE: f32 = 60.0 * MODEL_SCALE;

const LOCK_FOCUS_RESPONSE: f32 = 5.0;

const LOCK_POSITION_RESPONSE: f32 = 20.0 / 9.0;

const LOCK_POSITION_DT_LIMIT: f32 = 0.3;

const LOCK_POSITION_STALL_ALPHA: f32 = 2.0 / 3.0;

const ORIGINAL_H_FOV_DEG: f32 = 60.0;
const ORIGINAL_ASPECT: f32 = 4.0 / 3.0;

const CAMERA_NEAR: f32 = 2.0 * MODEL_SCALE;

const CAMERA_FAR: f32 = 1200.0;

const AIR_CHASE_UP: f32 = 6.0;
const AIR_CHASE_DOWN: f32 = 9.0;
const LAND_RECOVER: f32 = 12.0;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraMode>()
            .init_resource::<spectator::SpectatorState>()
            .init_resource::<CameraRecentreRequest>()
            .init_resource::<CameraBasis>()
            .add_systems(Startup, setup_camera)
            .add_systems(
                OnEnter(AppState::Playing),
                reset_follow_camera.run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(Update, update_camera_visibility)
            .add_systems(Update, toggle_camera_mode)
            .add_systems(Update, sync_pointer_lock)
            .add_systems(
                Update,
                spectator::update_hint.after(spectator::update_spectator),
            )
            .add_systems(OnExit(AppState::Playing), spectator::reset_spectator)
            .add_systems(OnExit(AppState::Playing), release_pointer_lock)
            .add_systems(
                Update,
                (
                    spectator::update_spectator,
                    apply_follow_camera_input,
                    prepare_move_basis,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .after(update_camera_visibility)
                    .run_if(in_state(AppState::Playing))
                    .run_if(|mode: Res<CameraMode>| !mode.fly),
            )
            .add_systems(
                Update,
                fly_camera_control
                    .run_if(in_state(AppState::Playing))
                    .run_if(|mode: Res<CameraMode>| mode.fly),
            )
            .add_systems(
                Update,
                follow_fighter_camera
                    .in_set(crate::plugins::FighterFrameSet::CameraFinalize)
                    .run_if(in_state(AppState::Playing))
                    .run_if(|mode: Res<CameraMode>| !mode.fly),
            );
    }
}

#[derive(Component, Clone, Copy, bevy::render::extract_component::ExtractComponent)]
pub(crate) struct GameplayCamera;

fn sync_pointer_lock(
    state: Res<State<AppState>>,
    dbg: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    mut windows: Query<(&Window, &mut CursorOptions), With<PrimaryWindow>>,
) {
    let Ok((window, mut cursor)) = windows.single_mut() else {
        return;
    };

    let playing = matches!(state.get(), AppState::Playing);
    let debug_capture = dbg.is_some_and(|dbg| dbg.captures_input());
    let focused = !cfg!(target_os = "windows") || window.focused;
    let lock = playing && !debug_capture && focused && !cfg!(target_os = "android");
    let want_mode = if lock {
        if cfg!(target_os = "windows") {
            CursorGrabMode::Confined
        } else {
            CursorGrabMode::Locked
        }
    } else {
        CursorGrabMode::None
    };
    if cursor.grab_mode != want_mode {
        cursor.grab_mode = want_mode;
    }
    let visible = (!playing || debug_capture || !focused) && !cfg!(target_os = "android");
    if cursor.visible != visible {
        cursor.visible = visible;
    }
}

fn reset_follow_camera(
    mut mode: ResMut<CameraMode>,
    mut query: Query<&mut ThirdPersonState, With<GameplayCamera>>,
) {
    mode.fly = false;
    if let Ok(mut camera) = query.single_mut() {
        *camera = ThirdPersonState::default();
    }
}

fn release_pointer_lock(mut windows: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if let Ok(mut cursor) = windows.single_mut() {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

#[derive(Component)]
pub(crate) struct UiCamera;

#[derive(Component)]
pub(crate) struct ThirdPersonState {
    yaw: f32,

    pitch: f32,

    distance: f32,

    smooth_yaw: f32,

    position: Vec3,

    target: Vec3,

    lock_side: f32,

    free_look_pending: bool,
    free_look_from_ideal: Vec3,
    free_look_from_yaw: f32,
    free_look_from_pitch: f32,

    prev_target_y: Option<f32>,

    follow_ready: bool,
    last_fighter_pos: Vec3,

    lock_was_active: bool,

    pitch_axis_hold: f32,
    yaw_axis_hold: f32,

    fly_speed: f32,

    pub(crate) scene_fade_required: bool,
    followed_entity: Option<Entity>,
}

impl ThirdPersonState {
    #[cfg(test)]
    pub(crate) fn follow_focus_for_test(&self) -> Vec3 {
        self.target
    }

    pub(crate) fn debug_snapshot(&self) -> (f32, f32, f32) {
        (self.yaw, self.pitch, self.distance)
    }
}

impl Default for ThirdPersonState {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: DEFAULT_PITCH,
            distance: DEFAULT_DISTANCE,
            smooth_yaw: 0.0,
            position: Vec3::ZERO,
            target: Vec3::ZERO,
            lock_side: 1.0,
            free_look_pending: false,
            free_look_from_ideal: Vec3::ZERO,
            free_look_from_yaw: 0.0,
            free_look_from_pitch: DEFAULT_PITCH,
            prev_target_y: None,
            follow_ready: false,
            last_fighter_pos: Vec3::ZERO,
            lock_was_active: false,
            pitch_axis_hold: 0.0,
            yaw_axis_hold: 0.0,
            fly_speed: 60.0,
            scene_fade_required: false,
            followed_entity: None,
        }
    }
}

struct VerticalAnchor;

impl VerticalAnchor {
    fn update(current_y: f32, look_y: f32, dt: f32, in_air: bool) -> f32 {
        let rate = if in_air {
            if look_y > current_y {
                AIR_CHASE_UP
            } else {
                AIR_CHASE_DOWN
            }
        } else {
            LAND_RECOVER
        };
        let next = damp(current_y, look_y, rate, dt);
        if !in_air && (next - look_y).abs() < 0.01 {
            look_y
        } else {
            next
        }
    }
}

fn damp(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    current + (target - current) * (1.0 - (-rate * dt.max(0.0)).exp())
}

fn nearest_equivalent_angle(angle: f32, reference: f32) -> f32 {
    reference + (angle - reference).sin().atan2((angle - reference).cos())
}

fn lock_release_yaw(fighter_facing: f32, visible_yaw: f32) -> f32 {
    nearest_equivalent_angle(fighter_facing, visible_yaw)
}

fn inject_ground_anchor_y(position_y: f32, target_y: f32, previous_y: f32, in_air: bool) -> f32 {
    if in_air {
        position_y
    } else {
        position_y + target_y - previous_y
    }
}

fn camera_position_alpha(
    lock_ready: bool,
    lock_snap: bool,
    airborne: bool,
    wall_clinging: bool,
    wall_dt: f32,
) -> f32 {
    if lock_ready {
        if wall_dt > LOCK_POSITION_DT_LIMIT {
            LOCK_POSITION_STALL_ALPHA
        } else {
            wall_dt * LOCK_POSITION_RESPONSE
        }
    } else if lock_snap {
        1.0
    } else {
        1.0 - (-(if airborne || wall_clinging {
            FOLLOW_AIR
        } else {
            FOLLOW_GROUND
        }) * wall_dt)
            .exp()
    }
}

fn vertical_fov_for_aspect(aspect: f32) -> f32 {
    let half_h = (ORIGINAL_H_FOV_DEG * std::f32::consts::PI / 360.0).tan() / ORIGINAL_ASPECT;
    let tan_half_v = if aspect >= ORIGINAL_ASPECT {
        half_h
    } else {
        (30.0f32 * std::f32::consts::PI / 180.0).tan() / aspect.max(1e-3)
    };
    2.0 * tan_half_v.atan()
}

fn resolve_collision(
    from: Vec3,
    ideal: Vec3,
    raycast: impl Fn(Vec3, Vec3, f32) -> Option<f32>,
) -> Vec3 {
    let delta = ideal - from;
    let distance = delta.length();
    if distance < 1e-4 {
        return ideal;
    }
    let direction = delta / distance;
    let right = direction.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
    let up = right.cross(direction).normalize_or_zero();
    let mut safe = distance;
    for offset in [Vec3::ZERO, right + up, right - up, -right + up, -right - up] {
        if let Some(hit) = raycast(from + offset * COLLISION_SKIN, direction, distance) {
            safe = safe.min((hit - COLLISION_SKIN).max(0.0));
        }
    }
    from + direction * safe
}

fn resolve_free_collision(
    anchor: Vec3,
    ideal: Vec3,
    requested_distance: f32,
    raycast: impl Fn(Vec3, Vec3, f32) -> Option<f32>,
) -> (Vec3, bool) {
    let safe = resolve_collision(anchor, ideal, raycast);
    let delta = ideal - anchor;
    let minimum = ACTION_VIEW_DISTANCE
        .min(requested_distance)
        .min(delta.length());
    if safe.distance(anchor) + 0.001 < minimum {
        (anchor + delta.normalize_or_zero() * minimum, true)
    } else {
        (safe, false)
    }
}

fn frame_locked_pair(
    focus: Vec3,
    eye: Vec3,
    player: Vec3,
    target: Vec3,
    fov: f32,
    aspect: f32,
) -> Vec3 {
    let back = (eye - focus).normalize_or_zero();
    let right = Vec3::Y.cross(back).normalize_or_zero();
    let up = back.cross(right).normalize_or_zero();
    let tan_v = (fov * 0.5).tan() * 0.8;
    let tan_h = tan_v * aspect.max(0.1);
    let mut distance = eye.distance(focus);
    for root in [player, target] {
        for height in [0.0, 5.0] {
            let offset = root + Vec3::Y * height - focus;
            let depth = offset.dot(back);
            distance = distance.max(depth + (offset.dot(right).abs() + 0.8) / tan_h);
            distance = distance.max(depth + (offset.dot(up).abs() + 0.5) / tan_v);
        }
    }
    focus + back * distance
}

fn avoid_locked_obstacles(
    anchor: Vec3,
    ideal: Vec3,
    raycast: impl Fn(Vec3, Vec3, f32) -> Option<f32> + Copy,
) -> Vec3 {
    let preferred = resolve_collision(anchor, ideal, raycast);
    if preferred.distance(anchor) >= ideal.distance(anchor) * 0.8 {
        return preferred;
    }
    let offset = ideal - anchor;
    let mut best = preferred;
    let mut score = preferred.distance(anchor);
    for angle in [-0.65_f32, 0.65, -1.2, 1.2] {
        for lift in [0.0, 3.0] {
            let candidate = anchor + Quat::from_rotation_y(angle) * offset + Vec3::Y * lift;
            let safe = resolve_collision(anchor, candidate, raycast);
            let quality = safe.distance(anchor) - safe.distance(ideal) * 0.35;
            if quality > score + 0.5 {
                best = safe;
                score = quality;
            }
        }
    }
    best
}

fn locked_eye(
    desired_focus: Vec3,
    camera_pos: Vec3,
    player: Vec3,
    watched: Vec3,
    lock_side: &mut f32,
    fallback_yaw: f32,
) -> Vec3 {
    let back_x = player.x - watched.x;
    let back_z = player.z - watched.z;
    let planar = (back_x * back_x + back_z * back_z).sqrt();
    let dir;
    if planar > 1e-8 {
        let d = Vec3::new(back_x / planar, 0.0, back_z / planar);
        let side_weight = LOCK_SIDE_REFERENCE / planar.max(MODEL_SCALE);
        let v = desired_focus - camera_pos;
        let cross_z = v.z * back_x - v.x * back_z;
        if cross_z.abs() > 1e-6 {
            *lock_side = if cross_z > 0.0 { 1.0 } else { -1.0 };
        }
        let right = Vec3::new(
            d.z * *lock_side * side_weight,
            0.0,
            -d.x * *lock_side * side_weight,
        );
        dir = (d + right).normalize_or_zero();
    } else {
        dir = Vec3::new(-fallback_yaw.sin(), 0.0, -fallback_yaw.cos());
    }
    desired_focus + dir * LOCK_DISTANCE + Vec3::new(0.0, LOCK_EYE_HEIGHT, 0.0)
}

fn toggle_camera_mode(
    keys: Res<ButtonInput<KeyCode>>,
    dbg: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    mut mode: ResMut<CameraMode>,
) {
    if dbg.is_some_and(|dbg| dbg.captures_input()) {
        return;
    }
    if keys.just_pressed(KeyCode::F5) {
        mode.fly = !mode.fly;
        info!("camera mode: {}", if mode.fly { "fly" } else { "follow" });
    }
}

pub(super) fn setup_camera(mut commands: Commands) {
    commands.spawn((
        GameplayCamera,
        Camera3d::default(),
        bevy::pbr::ScreenSpaceTransmission {
            steps: 2,
            ..default()
        },
        Camera {
            order: 0,
            ..default()
        },
        Projection::Perspective(PerspectiveProjection {
            fov: vertical_fov_for_aspect(ORIGINAL_ASPECT),
            near: CAMERA_NEAR,
            far: CAMERA_FAR,
            ..default()
        }),
        Tonemapping::None,
        Transform::from_xyz(0.0, 80.0, 130.0).looking_at(Vec3::ZERO, Vec3::Y),
        bevy::audio::SpatialListener::new(0.3),
        ThirdPersonState::default(),
    ));

    commands.spawn((
        UiCamera,
        Camera2d,
        Msaa::Off,
        Camera {
            order: 1,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            output_mode: bevy::camera::CameraOutputMode::Write {
                blend_state: Some(
                    bevy::render::render_resource::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
                ),
                clear_color: ClearColorConfig::None,
            },
            ..default()
        },
        IsDefaultUiCamera,
    ));
}

fn update_camera_visibility(
    _state: Res<State<AppState>>,
    mut query: Query<&mut Camera, With<GameplayCamera>>,
) {
    for mut camera in &mut query {
        camera.is_active = true;
    }
}

fn update_camera_axis_hold(current: f32, held: bool, restarted: bool, wall_dt: f32) -> f32 {
    if !held || restarted {
        0.0
    } else {
        (current + wall_dt.max(0.0)).min(1.5)
    }
}

fn camera_axis_step_degrees(axis: f32, hold: f32, mouse_speed: f32, dt: f32) -> f32 {
    if axis == 0.0 || dt <= 0.0 {
        return 0.0;
    }
    let seconds = hold.clamp(0.0, 1.5);
    let rate = (mouse_speed + 1.0) * 1.5;
    axis * seconds * (1.0 + seconds) * rate * dt
}

fn apply_camera_look(cam: &mut ThirdPersonState, lock_active: bool, dx_deg: f32, dy_deg: f32) {
    let free_look = !lock_active && !cam.lock_was_active;
    let from_yaw = if free_look { cam.smooth_yaw } else { cam.yaw };
    let from_pitch = cam.pitch;
    let next_yaw = from_yaw - dx_deg * DEG;
    let next_pitch = (from_pitch + dy_deg * DEG).clamp(PITCH_MIN, PITCH_MAX);
    if next_yaw == from_yaw && next_pitch == from_pitch {
        return;
    }
    if free_look {
        if !cam.free_look_pending {
            cam.free_look_from_yaw = from_yaw;
            cam.free_look_from_pitch = from_pitch;
        }
        cam.free_look_pending = true;
        cam.smooth_yaw = next_yaw;
    }
    cam.yaw = next_yaw;
    cam.pitch = next_pitch;
}

pub(crate) const TOUCH_LOOK_FULL_SWEEP_DEG: f32 = 240.0;

pub(crate) fn touch_look_degrees(delta: Vec2, width: f32, height: f32, mouse_speed: f32) -> Vec2 {
    let speed = ((mouse_speed + 1.0) / 50.0).clamp(0.2, 3.0);
    Vec2::new(
        delta.x / width.max(1.0) * TOUCH_LOOK_FULL_SWEEP_DEG * speed,
        delta.y / height.max(1.0) * TOUCH_LOOK_FULL_SWEEP_DEG * speed,
    )
}

fn apply_keyboard_yaw_to_fighter(
    motion: &mut crate::components::fighter::MotionState,
    state: meteor_core::FighterState,
    switching_weapon: bool,
    yaw_step_deg: f32,
) -> bool {
    if state != meteor_core::FighterState::Name || switching_weapon || yaw_step_deg == 0.0 {
        return false;
    }
    motion.facing -= yaw_step_deg * DEG;
    true
}

#[allow(clippy::too_many_arguments)]
fn apply_follow_camera_input(
    dbg: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    (replay, spectator): (
        Option<Res<crate::plugins::replay_plugin::MrcPlayback>>,
        Option<Res<spectator::SpectatorState>>,
    ),
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    window: Query<&CursorOptions, With<PrimaryWindow>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    settings: Res<Settings>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<crate::app_state::Keymap>,
    mut mouse_motion: MessageReader<MouseMotion>,
    mut scroll: MessageReader<MouseWheel>,
    mut mobile_look: Option<ResMut<crate::plugins::mobile_controls_plugin::MobileLookDelta>>,
    lock: Res<crate::components::fighter::LockOnController>,
    mut camera: Query<&mut ThirdPersonState, With<GameplayCamera>>,
    mut fighter: Query<
        (
            &mut crate::components::fighter::MotionState,
            &crate::components::fighter::SmState,
            Option<&crate::components::fighter::WeaponSwitchActive>,
            Option<&crate::plugins::movement_plugin::FighterAir>,
        ),
        With<LocalFighter>,
    >,
) {
    if dbg.is_some_and(|dbg| dbg.captures_input()) {
        mouse_motion.clear();
        scroll.clear();
        if let Some(delta) = mobile_look.as_deref_mut() {
            delta.0.x = 0.0;
            delta.0.y = 0.0;
        }
        return;
    }
    let (Ok(mut cam), Ok((mut fighter_motion, fighter_state, switching, air))) =
        (camera.single_mut(), fighter.single_mut())
    else {
        return;
    };
    let sim_dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let wall_dt = frame_clock
        .as_deref()
        .map(|clock| clock.wall_dt)
        .unwrap_or_else(|| time.delta_secs().max(0.0));
    let pitch_up = keymap.camera_up.pressed(&keys, &mouse);
    let pitch_down = keymap.camera_down.pressed(&keys, &mouse);
    let yaw_right = keymap.right_turn.pressed(&keys, &mouse);
    let yaw_left = keymap.left_turn.pressed(&keys, &mouse);
    cam.pitch_axis_hold = update_camera_axis_hold(
        cam.pitch_axis_hold,
        pitch_up || pitch_down,
        keymap.camera_up.just_pressed(&keys, &mouse)
            || keymap.camera_down.just_pressed(&keys, &mouse),
        wall_dt,
    );
    cam.yaw_axis_hold = update_camera_axis_hold(
        cam.yaw_axis_hold,
        yaw_right || yaw_left,
        keymap.right_turn.just_pressed(&keys, &mouse)
            || keymap.left_turn.just_pressed(&keys, &mouse),
        wall_dt,
    );
    let pitch_axis = f32::from(pitch_up) - f32::from(pitch_down);

    let spectating = spectator.as_deref().is_some_and(|state| state.active);
    let wall_running =
        air.is_some_and(|air| air.air.motion_mode == meteor_physics::jump::AirMotionMode::Wall);
    let horizontal_look = spectating
        || (switching.is_none()
            && !wall_running
            && !matches!(
                fighter_state.current,
                meteor_core::FighterState::Attack1
                    | meteor_core::FighterState::Attack2
                    | meteor_core::FighterState::Attack3
                    | meteor_core::FighterState::Dodge
                    | meteor_core::FighterState::Burst
                    | meteor_core::FighterState::Guard
                    | meteor_core::FighterState::BeHit
                    | meteor_core::FighterState::Down
                    | meteor_core::FighterState::Fake
                    | meteor_core::FighterState::Dead
                    | meteor_core::FighterState::Cure
            ));
    let yaw_axis = if horizontal_look {
        f32::from(yaw_right) - f32::from(yaw_left)
    } else {
        0.0
    };
    let pitch_step = camera_axis_step_degrees(
        pitch_axis,
        cam.pitch_axis_hold,
        settings.mouse_speed,
        sim_dt,
    );
    let yaw_step =
        camera_axis_step_degrees(yaw_axis, cam.yaw_axis_hold, settings.mouse_speed, sim_dt);
    let lock_active = !spectating && lock.lock_entity.is_some();
    if pitch_step != 0.0 {
        apply_camera_look(&mut cam, lock_active, 0.0, pitch_step);
    }
    if yaw_step != 0.0 {
        apply_camera_look(&mut cam, lock_active, yaw_step, 0.0);
        if !spectating {
            let _ = apply_keyboard_yaw_to_fighter(
                &mut fighter_motion,
                fighter_state.current,
                switching.is_some(),
                yaw_step,
            );
        }
    }

    let mobile_delta = mobile_look
        .as_deref_mut()
        .map(|delta| std::mem::take(&mut delta.0))
        .unwrap_or(Vec2::ZERO);
    let pointer_locked = window
        .single()
        .is_ok_and(|cursor| cursor.grab_mode != CursorGrabMode::None);
    let looking = pointer_locked || mouse.pressed(MouseButton::Left) || mobile_delta != Vec2::ZERO;
    let deg_per_count = (settings.mouse_speed + 1.0) / 140.0;
    let reverse_y = if settings.reverse_mouse { -1.0 } else { 1.0 };
    let mut pointer_yaw_deg = 0.0;
    if looking {
        for event in mouse_motion.read() {
            let dx_deg = if horizontal_look {
                event.delta.x * deg_per_count
            } else {
                0.0
            };
            pointer_yaw_deg += dx_deg;
            apply_camera_look(
                &mut cam,
                lock_active,
                dx_deg,
                event.delta.y * deg_per_count * reverse_y,
            );
        }
        if mobile_delta != Vec2::ZERO {
            let (width, height) = windows
                .single()
                .map(|window| (window.width(), window.height()))
                .unwrap_or((1280.0, 720.0));
            let mut touch_deg =
                touch_look_degrees(mobile_delta, width, height, settings.mouse_speed);
            if !horizontal_look {
                touch_deg.x = 0.0;
            }
            pointer_yaw_deg += touch_deg.x;
            apply_camera_look(&mut cam, lock_active, touch_deg.x, touch_deg.y * reverse_y);
        }
    } else {
        mouse_motion.clear();
    }

    let can_turn_from_pointer = !spectating
        && !lock_active
        && !replay.as_deref().is_some_and(|replay| replay.active)
        && switching.is_none()
        && matches!(
            fighter_state.current,
            meteor_core::FighterState::Name | meteor_core::FighterState::Jump
        )
        && air.is_none_or(|air| {
            !air.air.airborne
                || air.air.motion_mode == meteor_physics::jump::AirMotionMode::Ballistic
        });
    let facing_before = fighter_motion.facing;
    if can_turn_from_pointer {
        fighter_motion.facing -= pointer_yaw_deg * DEG;
    }
    if pointer_yaw_deg != 0.0 && air.is_some_and(|air| air.air.airborne) {
        debug!(
            target: "meteor_air_turn",
            "air-turn state={:?} mode={:?} auto={} lock={:?} switch={} replay={} pointer_locked={} delta_deg={:.4} applied={} facing={:.6}->{:.6} camera={:.6}",
            fighter_state.current,
            air.map(|air| air.air.motion_mode),
            settings.auto_camera,
            lock.lock_entity,
            switching.is_some(),
            replay.as_deref().is_some_and(|replay| replay.active),
            pointer_locked,
            pointer_yaw_deg,
            can_turn_from_pointer,
            facing_before,
            fighter_motion.facing,
            cam.yaw,
        );
    }
    for event in scroll.read() {
        cam.distance = (cam.distance - event.y * ZOOM_STEP).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }
}

pub(crate) fn prepare_move_basis(
    mut basis: ResMut<CameraBasis>,
    query: Query<&ThirdPersonState, With<GameplayCamera>>,
) {
    if let Ok(state) = query.single() {
        basis.yaw = state.yaw;
    }
}

fn fly_camera_control(
    dbg: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut motion: MessageReader<MouseMotion>,
    mut scroll: MessageReader<MouseWheel>,
    mut query: Query<(&mut Transform, &mut ThirdPersonState), With<GameplayCamera>>,
) {
    let Ok((mut transform, mut cam)) = query.single_mut() else {
        return;
    };
    if dbg.is_some_and(|dbg| dbg.captures_input()) {
        motion.clear();
        scroll.clear();
        return;
    }

    let mut yaw_input = 0.0f32;
    let mut pitch_input = 0.0f32;
    if keys.pressed(KeyCode::ArrowLeft) {
        yaw_input -= 1.0;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        yaw_input += 1.0;
    }
    if keys.pressed(KeyCode::ArrowUp) {
        pitch_input += 1.0;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        pitch_input -= 1.0;
    }
    if mouse.pressed(MouseButton::Left) {
        for event in motion.read() {
            yaw_input -= event.delta.x * 0.01;
            pitch_input += event.delta.y * 0.01;
        }
    }
    cam.yaw += yaw_input * 2.4 * time.delta_secs();
    cam.pitch = (cam.pitch + pitch_input * 2.4 * time.delta_secs()).clamp(-1.5, 1.5);

    let rot = Quat::from_euler(EulerRot::YXZ, cam.yaw, cam.pitch, 0.0);
    let forward = rot * Vec3::NEG_Z;
    let right = rot * Vec3::X;
    let mut move_dir = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        move_dir += forward;
    }
    if keys.pressed(KeyCode::KeyS) {
        move_dir -= forward;
    }
    if keys.pressed(KeyCode::KeyA) {
        move_dir -= right;
    }
    if keys.pressed(KeyCode::KeyD) {
        move_dir += right;
    }
    if keys.pressed(KeyCode::KeyQ) {
        move_dir -= Vec3::Y;
    }
    if keys.pressed(KeyCode::KeyE) {
        move_dir += Vec3::Y;
    }
    let speed = cam.fly_speed * time.delta_secs();
    transform.translation += move_dir.normalize_or_zero() * speed;
    for event in scroll.read() {
        transform.translation += forward * event.y * speed * 2.0;
    }
    transform.rotation = rot;
}

type LocalFighterCamQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static LocalFighter>,
        &'static Transform,
        Option<&'static crate::plugins::movement_plugin::FighterAir>,
        &'static crate::components::fighter::MotionState,
    ),
    Without<GameplayCamera>,
>;

#[allow(clippy::too_many_arguments)]
fn follow_fighter_camera(
    spectator: Option<Res<spectator::SpectatorState>>,
    mode: Res<CameraMode>,
    mut recentre: ResMut<CameraRecentreRequest>,
    console: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    window: Query<&Window, With<PrimaryWindow>>,
    mrc_frame: Option<Res<crate::plugins::replay_plugin::MrcFrameOverride>>,
    lock: Res<crate::components::fighter::LockOnController>,
    collider: Res<StageCollider>,
    fighters: LocalFighterCamQuery<'_, '_>,
    mut query: Query<
        (&mut Transform, &mut Projection, &mut ThirdPersonState),
        With<GameplayCamera>,
    >,
    targets: Query<&Transform, (Without<LocalFighter>, Without<GameplayCamera>)>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(4);
    let Ok((mut transform, mut projection, mut cam)) = query.single_mut() else {
        return;
    };
    let spectating = spectator.as_deref().is_some_and(|state| state.active);
    let followed = if spectating {
        spectator
            .as_deref()
            .and_then(|state| state.target)
            .and_then(|entity| fighters.get(entity).ok())
    } else {
        fighters.iter().find(|(_, local, ..)| local.is_some())
    };
    let Some((entity, _, fighter, fighter_air, motion_state)) = followed else {
        return;
    };
    if cam.followed_entity != Some(entity) {
        if spectating || cam.followed_entity.is_some() {
            cam.follow_ready = false;
            cam.prev_target_y = None;
            cam.lock_was_active = false;
            cam.free_look_pending = false;
            cam.target = fighter.translation + Vec3::Y * LOOK_HEIGHT;
            cam.position = free_ideal(&cam, cam.smooth_yaw, cam.pitch);
        }
        cam.followed_entity = Some(entity);
    }

    if mode.freecam_hold {
        return;
    }

    if std::mem::take(&mut recentre.0) {
        let yaw = nearest_equivalent_angle(motion_state.facing, cam.smooth_yaw);
        cam.yaw = yaw;
        cam.smooth_yaw = yaw;
    }

    if console.is_some_and(|console| console.captures_input()) {
        return;
    }

    let wall_dt = frame_clock
        .as_deref()
        .map(|clock| clock.wall_dt)
        .unwrap_or_else(|| time.delta_secs().max(0.0));
    if mrc_frame.as_ref().is_some_and(|frame| frame.facing_applied) {
        cam.yaw = nearest_equivalent_angle(motion_state.facing, cam.smooth_yaw);
    }

    let air_airborne = fighter_air.map(|air| air.air.airborne).unwrap_or(false);
    let wall_clinging = fighter_air
        .map(|air| air.air.motion_mode == meteor_physics::jump::AirMotionMode::Wall)
        .unwrap_or(false);
    let airborne = air_airborne
        || wall_clinging
        || motion_state.movement_mode == meteor_core::MovementMode::Airborne
        || motion_state.movement_mode == meteor_core::MovementMode::WallRun;

    if let Ok(window) = window.single() {
        let aspect = window.width() / window.height().max(1.0);
        let vfov = vertical_fov_for_aspect(aspect);
        if let Projection::Perspective(projection) = projection.as_mut() {
            if (projection.fov - vfov).abs() > 1e-4 {
                projection.fov = vfov;
            }
        }
    }
    let lock_entity = (!spectating)
        .then_some(lock.lock_entity)
        .flatten()
        .and_then(|e| targets.get(e).ok());

    let fighter_pos = fighter.translation;
    let look_y = fighter_pos.y + LOOK_HEIGHT;
    let lock_target_pos = lock_entity.map(|t| t.translation);
    let lock_ready = lock_target_pos.is_some();

    let wall_follow = wall_clinging && !spectating && !lock_ready;
    if wall_follow {
        cam.yaw = nearest_equivalent_angle(motion_state.facing, cam.smooth_yaw);
        cam.smooth_yaw = cam.yaw;
        cam.free_look_pending = false;
    }

    let lock_snap = !lock_ready && cam.lock_was_active;
    if lock_snap {
        cam.yaw = lock_release_yaw(motion_state.facing, cam.smooth_yaw);
    }
    if !lock_ready && !lock_snap {
        cam.smooth_yaw = damp(cam.smooth_yaw, cam.yaw, YAW_SMOOTH, wall_dt);
    }
    cam.lock_was_active = lock_ready;

    if cam.follow_ready {
        if !lock_ready && !lock_snap {
            let dx = fighter_pos.x - cam.last_fighter_pos.x;
            let dz = fighter_pos.z - cam.last_fighter_pos.z;
            cam.target.x += dx;
            cam.target.z += dz;
            cam.position.x += dx;
            cam.position.z += dz;
        }
    } else {
        cam.target = Vec3::new(fighter_pos.x, look_y, fighter_pos.z);
        cam.follow_ready = true;
    }

    let desired_focus: Option<Vec3> = if lock_ready {
        let player = fighter_pos;
        let watched = lock_target_pos.unwrap();
        let focus = Vec3::new(
            player.x + (watched.x - player.x) * LOCK_FOCUS_FRACTION,
            player.y + (watched.y - player.y) * LOCK_FOCUS_FRACTION + LOCK_FOCUS_HEIGHT,
            player.z + (watched.z - player.z) * LOCK_FOCUS_FRACTION,
        );
        cam.target = cam
            .target
            .lerp(focus, (wall_dt * LOCK_FOCUS_RESPONSE).min(1.0));
        cam.prev_target_y = Some(cam.target.y);
        Some(focus)
    } else if lock_snap {
        cam.smooth_yaw = cam.yaw;
        cam.target = Vec3::new(fighter_pos.x, look_y, fighter_pos.z);
        cam.prev_target_y = Some(cam.target.y);
        None
    } else {
        let in_air = airborne || wall_clinging;
        cam.target.y = VerticalAnchor::update(cam.target.y, look_y, wall_dt, in_air);
        cam.target.x = fighter_pos.x;
        cam.target.z = fighter_pos.z;

        cam.position.y = inject_ground_anchor_y(
            cam.position.y,
            cam.target.y,
            cam.prev_target_y.unwrap_or(cam.target.y),
            in_air,
        );
        cam.prev_target_y = Some(cam.target.y);
        None
    };
    cam.last_fighter_pos = fighter_pos;

    let raycast = |o: Vec3, d: Vec3, max: f32| {
        collider
            .mesh
            .as_ref()
            .and_then(|m| m.raycast_hit(point(o), point(d), max))
            .map(|hit| hit.t)
    };
    let ideal = if lock_ready {
        let cam_pos = cam.position;
        let mut lock_side = cam.lock_side;
        let eye = locked_eye(
            desired_focus.unwrap(),
            cam_pos,
            fighter_pos,
            lock_target_pos.unwrap(),
            &mut lock_side,
            cam.smooth_yaw,
        );
        cam.lock_side = lock_side;
        eye
    } else {
        if cam.free_look_pending {
            let from_ideal = free_ideal(&cam, cam.free_look_from_yaw, cam.free_look_from_pitch);
            cam.free_look_from_ideal =
                resolve_free_collision(cam.target, from_ideal, cam.distance, raycast).0;
        }
        free_ideal(&cam, cam.smooth_yaw, cam.pitch)
    };

    let view_target = if lock_ready {
        desired_focus.unwrap()
    } else {
        cam.target
    };

    let anchor = if lock_ready {
        fighter_pos + Vec3::Y * LOOK_HEIGHT
    } else {
        view_target
    };
    cam.scene_fade_required = false;
    let ideal = if let (Some(target), Projection::Perspective(lens)) =
        (lock_target_pos, projection.as_ref())
    {
        let framed = frame_locked_pair(
            view_target,
            ideal,
            fighter_pos,
            target,
            lens.fov,
            lens.aspect_ratio,
        );
        avoid_locked_obstacles(anchor, framed, raycast)
    } else if !lock_ready {
        let (eye, fade) = resolve_free_collision(anchor, ideal, cam.distance, raycast);
        cam.scene_fade_required = fade;
        eye
    } else {
        resolve_collision(anchor, ideal, raycast)
    };
    if !lock_ready && cam.free_look_pending {
        cam.position = cam.position + ideal - cam.free_look_from_ideal;
        cam.free_look_pending = false;
    }

    let alpha = if wall_follow {
        1.0
    } else {
        camera_position_alpha(lock_ready, lock_snap, airborne, wall_clinging, wall_dt)
    };

    let smoothed = cam.position.lerp(ideal, alpha);
    cam.position = if lock_ready {
        resolve_collision(anchor, smoothed, raycast)
    } else {
        let (eye, fade) = resolve_free_collision(anchor, smoothed, cam.distance, raycast);
        cam.scene_fade_required |= fade;
        eye
    };
    if lock_ready {
        let view = view_target - cam.position;
        if view.x * view.x + view.z * view.z > 1e-8 {
            cam.smooth_yaw = nearest_equivalent_angle(view.x.atan2(view.z), cam.smooth_yaw);
        }
    }

    transform.translation = cam.position;
    transform.look_at(view_target, Vec3::Y);
}

#[cfg(test)]
pub(crate) fn install_camera_input_for_test(app: &mut App) {
    app.init_resource::<Messages<MouseMotion>>()
        .init_resource::<Messages<MouseWheel>>()
        .add_systems(
            Update,
            (apply_follow_camera_input, prepare_move_basis)
                .chain()
                .in_set(crate::plugins::FighterFrameSet::Movement),
        );
}

#[cfg(test)]
pub(crate) fn install_follow_camera_for_test(app: &mut App) -> Entity {
    app.init_resource::<CameraMode>()
        .init_resource::<CameraRecentreRequest>()
        .add_systems(
            Update,
            follow_fighter_camera.in_set(crate::plugins::FighterFrameSet::CameraFinalize),
        );
    app.world_mut()
        .spawn((
            GameplayCamera,
            Transform::default(),
            Projection::Perspective(PerspectiveProjection::default()),
            ThirdPersonState {
                position: Vec3::new(0.0, LOOK_HEIGHT, -16.0),
                target: Vec3::Y * LOOK_HEIGHT,
                follow_ready: true,
                ..default()
            },
        ))
        .id()
}

fn free_ideal(cam: &ThirdPersonState, yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    Vec3::new(
        cam.target.x - sy * cam.distance * cp,
        cam.target.y + sp * cam.distance + 0.35,
        cam.target.z - cy * cam.distance * cp,
    )
}

fn point(v: Vec3) -> meteor_physics::collision::Point3 {
    meteor_physics::collision::Point3::new(v.x, v.y, v.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_cursor_follows_gameplay_pause_focus_and_debug_ui() {
        use crate::plugins::debug_plugin::DebugConsoleState;

        let mut app = App::new();
        app.insert_resource(State::new(AppState::Menu))
            .init_resource::<DebugConsoleState>()
            .add_systems(Update, sync_pointer_lock);
        let entity = app
            .world_mut()
            .spawn((
                Window {
                    focused: true,
                    ..default()
                },
                CursorOptions::default(),
                PrimaryWindow,
            ))
            .id();

        for (state, focused, open, panel_open, captured) in [
            (AppState::Menu, true, false, false, false),
            (AppState::Playing, true, false, false, true),
            (AppState::Paused, true, false, false, false),
            (AppState::Playing, true, false, false, true),
            (AppState::Playing, false, false, false, false),
            (AppState::Playing, true, false, false, true),
            (AppState::Playing, true, true, false, false),
            (AppState::Playing, true, false, true, false),
            (AppState::Playing, true, false, false, true),
            (AppState::Result, true, false, false, false),
        ] {
            app.insert_resource(State::new(state));
            app.insert_resource(DebugConsoleState { open, panel_open });
            app.world_mut().get_mut::<Window>(entity).unwrap().focused = focused;
            app.update();
            let cursor = app.world().get::<CursorOptions>(entity).unwrap();
            assert_eq!(
                cursor.grab_mode,
                if captured {
                    CursorGrabMode::Confined
                } else {
                    CursorGrabMode::None
                },
                "{state:?}, focused={focused}, console={open}, panel={panel_open}"
            );
            assert_eq!(cursor.visible, !captured);
        }
    }

    #[test]
    fn production_locked_camera_corrects_the_smoothed_eye_against_a_wall() {
        use crate::components::fighter::{LockOnController, MotionState};
        let mut mesh = crate::plugins::stage_collider::StageMeshCollider::default();
        mesh.append_world_triangles(
            &[
                [-50.0, -10.0, -2.0],
                [50.0, -10.0, -2.0],
                [50.0, 50.0, -2.0],
                [-50.0, 50.0, -2.0],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        let mut app = App::new();
        app.insert_resource(time)
            .insert_resource(StageCollider { mesh: Some(mesh) })
            .init_resource::<CameraMode>()
            .init_resource::<CameraRecentreRequest>()
            .init_resource::<LockOnController>()
            .add_systems(Update, follow_fighter_camera);
        app.world_mut().spawn((
            LocalFighter,
            Transform::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
        ));
        let target = app
            .world_mut()
            .spawn(Transform::from_xyz(0.0, 0.0, 3.0))
            .id();
        app.world_mut()
            .resource_mut::<LockOnController>()
            .lock_entity = Some(target);
        let camera = app
            .world_mut()
            .spawn((
                GameplayCamera,
                Transform::default(),
                Projection::Perspective(PerspectiveProjection::default()),
                ThirdPersonState {
                    position: Vec3::new(0.0, LOOK_HEIGHT, -10.0),
                    target: Vec3::Y * LOOK_HEIGHT,
                    follow_ready: true,
                    lock_was_active: true,
                    ..default()
                },
            ))
            .id();
        app.update();
        let eye = app.world().get::<Transform>(camera).unwrap().translation;
        assert!(
            eye.z > -2.0 + 0.25,
            "smoothed camera remained behind the wall: {eye:?}"
        );
        assert!(eye.is_finite());
    }

    #[test]
    fn production_free_camera_keeps_action_distance_and_recovers_after_wall() {
        use crate::components::fighter::{LockOnController, MotionState};
        let mut mesh = crate::plugins::stage_collider::StageMeshCollider::default();
        mesh.append_world_triangles(
            &[
                [-50.0, -10.0, -2.0],
                [50.0, -10.0, -2.0],
                [50.0, 50.0, -2.0],
                [-50.0, 50.0, -2.0],
            ],
            &[0, 1, 2, 0, 2, 3],
        );
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        let mut app = App::new();
        app.insert_resource(time)
            .insert_resource(StageCollider { mesh: Some(mesh) })
            .init_resource::<CameraMode>()
            .init_resource::<CameraRecentreRequest>()
            .init_resource::<LockOnController>()
            .add_systems(Update, follow_fighter_camera);
        app.world_mut().spawn((
            LocalFighter,
            Transform::default(),
            MotionState {
                movement_mode: meteor_core::MovementMode::Idle,
                velocity: Vec3::ZERO,
                facing: 0.0,
            },
        ));
        let camera = app
            .world_mut()
            .spawn((
                GameplayCamera,
                Transform::default(),
                Projection::Perspective(PerspectiveProjection::default()),
                ThirdPersonState {
                    position: Vec3::new(0.0, LOOK_HEIGHT, -MAX_DISTANCE),
                    distance: MAX_DISTANCE,
                    target: Vec3::Y * LOOK_HEIGHT,
                    follow_ready: true,
                    lock_was_active: false,
                    ..default()
                },
            ))
            .id();
        app.update();
        let eye = app.world().get::<Transform>(camera).unwrap().translation;
        assert!(eye.distance(Vec3::Y * LOOK_HEIGHT) >= ACTION_VIEW_DISTANCE - 0.01);
        assert!(
            app.world()
                .get::<ThirdPersonState>(camera)
                .unwrap()
                .scene_fade_required
        );
        app.world_mut().resource_mut::<StageCollider>().mesh = None;
        app.update();
        let recovering = app.world().get::<Transform>(camera).unwrap().translation;
        assert!(recovering.distance(Vec3::Y * LOOK_HEIGHT) > eye.distance(Vec3::Y * LOOK_HEIGHT));
        assert!(recovering.distance(Vec3::Y * LOOK_HEIGHT) < MAX_DISTANCE);
        assert!(
            !app.world()
                .get::<ThirdPersonState>(camera)
                .unwrap()
                .scene_fade_required
        );
        for _ in 0..40 {
            app.update();
        }
        assert!(
            app.world()
                .get::<Transform>(camera)
                .unwrap()
                .translation
                .distance(Vec3::Y * LOOK_HEIGHT)
                > MAX_DISTANCE - 0.1
        );
    }

    #[test]
    fn free_collision_respects_manual_zoom_and_smooth_pullback() {
        for requested in [2.0_f32, 6.0, 16.0] {
            let (eye, fade) =
                resolve_free_collision(Vec3::ZERO, -Vec3::Z * requested, requested, |_, _, _| {
                    Some(1.0)
                });
            assert!(fade);
            assert!((eye.length() - requested.min(ACTION_VIEW_DISTANCE)).abs() < 1e-5);
        }
        let ideal = Vec3::new(0.0, 2.0, -16.0);
        let (eye, fade) = resolve_free_collision(Vec3::ZERO, ideal, 16.0, |_, _, _| None);
        assert_eq!(eye, ideal);
        assert!(!fade);
        let (eye, fade) =
            resolve_free_collision(Vec3::ZERO, -Vec3::Z * 3.0, 16.0, |_, _, _| Some(1.0));
        assert!(fade);
        assert!(
            (eye.length() - 3.0).abs() < 1e-5,
            "do not hard-cut a smoothed pullback to ten units"
        );
    }

    #[test]
    fn production_wall_follow_binds_root_heading_without_eye_chase_and_preserves_pitch() {
        use crate::components::fighter::{LockOnController, MotionState};
        use crate::plugins::movement_plugin::FighterAir;
        use meteor_physics::jump::AirMotionMode;
        for fps in [30.0_f32, 60.0, 120.0] {
            for facing in [-1.2_f32, 0.0, 1.2] {
                let mut time: Time = Time::default();
                time.advance_by(std::time::Duration::from_secs_f32(1.0 / fps));
                let mut app = App::new();
                app.insert_resource(time)
                    .init_resource::<StageCollider>()
                    .init_resource::<CameraMode>()
                    .init_resource::<CameraRecentreRequest>()
                    .init_resource::<LockOnController>()
                    .add_systems(Update, follow_fighter_camera);
                let mut air = FighterAir::default();
                air.air.airborne = true;
                air.air.motion_mode = AirMotionMode::Wall;
                let fighter = app
                    .world_mut()
                    .spawn((
                        LocalFighter,
                        Transform::from_xyz(2.0, 10.0, 3.0),
                        MotionState {
                            movement_mode: meteor_core::MovementMode::WallRun,
                            velocity: Vec3::ZERO,
                            facing,
                        },
                        air,
                    ))
                    .id();
                let camera = app
                    .world_mut()
                    .spawn((
                        GameplayCamera,
                        Transform::default(),
                        Projection::Perspective(PerspectiveProjection::default()),
                        ThirdPersonState {
                            yaw: facing + 0.8,
                            smooth_yaw: facing + 0.4,
                            position: Vec3::new(9.0, 5.0, -8.0),
                            free_look_pending: true,
                            ..default()
                        },
                    ))
                    .id();
                for pitch in [0.2_f32, 0.65, -0.1] {
                    app.world_mut()
                        .get_mut::<ThirdPersonState>(camera)
                        .unwrap()
                        .pitch = pitch;
                    app.world_mut()
                        .get_mut::<Transform>(fighter)
                        .unwrap()
                        .translation += Vec3::new(0.1, 0.25, 0.2);
                    app.update();
                    let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
                    let eye = app.world().get::<Transform>(camera).unwrap().translation;
                    assert!((cam.yaw - facing).abs() < 1e-6);
                    assert!((cam.smooth_yaw - facing).abs() < 1e-6);
                    assert_eq!(
                        cam.pitch, pitch,
                        "wall entry must not reset independent pitch"
                    );
                    assert!(!cam.free_look_pending);

                    let expected = cam.target
                        + Vec3::new(
                            -facing.sin() * DEFAULT_DISTANCE * pitch.cos(),
                            pitch.sin() * DEFAULT_DISTANCE + 0.35,
                            -facing.cos() * DEFAULT_DISTANCE * pitch.cos(),
                        );
                    assert!(eye.distance(expected) < 1e-5, "fps={fps}, facing={facing}");
                }

                app.world_mut()
                    .get_mut::<FighterAir>(fighter)
                    .unwrap()
                    .air
                    .motion_mode = AirMotionMode::Ballistic;
                {
                    let mut cam = app.world_mut().get_mut::<ThirdPersonState>(camera).unwrap();
                    apply_camera_look(&mut cam, false, 10.0, 0.0);
                }
                app.update();
                let cam = app.world().get::<ThirdPersonState>(camera).unwrap();
                assert!((cam.yaw - (facing - 10.0 * DEG)).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn locked_pair_fits_portrait_landscape_and_height_difference() {
        for aspect in [0.55, 4.0 / 3.0, 2.2] {
            for target in [Vec3::new(0.0, 0.0, 28.0), Vec3::new(2.0, 11.0, 2.0)] {
                let player = Vec3::ZERO;
                let focus = target * LOCK_FOCUS_FRACTION + Vec3::Y * LOCK_FOCUS_HEIGHT;
                let mut side = 1.0;
                let eye = locked_eye(
                    focus,
                    Vec3::new(0.0, 4.0, -10.0),
                    player,
                    target,
                    &mut side,
                    0.0,
                );
                let fov = vertical_fov_for_aspect(aspect);
                let eye = frame_locked_pair(focus, eye, player, target, fov, aspect);
                let camera = Transform::from_translation(eye).looking_at(focus, Vec3::Y);
                for root in [player, target] {
                    for height in [0.0, 5.0] {
                        let local = camera.rotation.inverse() * (root + Vec3::Y * height - eye);
                        let half = -local.z * (fov * 0.5).tan();
                        assert!(local.z < 0.0);
                        assert!(local.y.abs() < half * 0.81);
                        assert!(local.x.abs() < half * aspect * 0.81);
                    }
                }
            }
        }
    }

    #[test]
    fn camera_corner_probe_catches_obstacle_missed_by_center() {
        let safe = resolve_collision(Vec3::ZERO, Vec3::Z * 10.0, |origin, _, _| {
            (origin.x.abs() > 0.2).then_some(2.0)
        });
        assert!((safe.z - 1.7).abs() < 1e-4);
    }

    #[test]
    fn locked_camera_searches_a_clear_side_when_backed_against_wall() {
        let safe = avoid_locked_obstacles(Vec3::ZERO, -Vec3::Z * 14.0, |_, direction, _| {
            (direction.x.abs() < 0.3).then_some(1.0)
        });
        assert!(safe.length() > 10.0);
        assert!(safe.x.abs() > 3.0);
    }

    #[test]
    fn touch_look_degrees_normalize_by_screen_size() {
        let deg = touch_look_degrees(Vec2::new(400.0, 0.0), 800.0, 360.0, 49.0);
        assert!((deg.x - 120.0).abs() < 1e-3);

        let wide = touch_look_degrees(Vec2::new(400.0, 0.0), 1600.0, 900.0, 49.0);
        assert!(wide.x < deg.x);

        let fast = touch_look_degrees(Vec2::new(400.0, 0.0), 800.0, 360.0, 100.0);
        assert!(fast.x > deg.x * 1.8);
    }

    #[test]
    fn damp_converges_and_is_frame_rate_independent() {
        let mut a = 0.0_f32;
        for _ in 0..60 {
            a = damp(a, 10.0, 12.0, 1.0 / 60.0);
        }
        let mut b = 0.0_f32;
        for _ in 0..30 {
            b = damp(b, 10.0, 12.0, 1.0 / 30.0);
        }
        assert!((a - b).abs() < 1e-4);
        assert!(a > 9.9);
    }

    #[test]
    fn lerp_angle_shortest_path() {
        let r = meteor_core::math::lerp_angle(0.0, std::f32::consts::TAU * 0.75, 0.5);
        assert!((r + std::f32::consts::FRAC_PI_2 * 0.5).abs() < 1e-4);
    }

    #[test]
    fn nearest_equivalent_angle_wraps() {
        let r =
            nearest_equivalent_angle(-std::f32::consts::FRAC_PI_2, std::f32::consts::TAU * 0.75);
        assert!((r - std::f32::consts::TAU * 0.75).abs() < 1e-4);
    }

    #[test]
    fn lock_release_restores_fighter_facing_near_visible_angle() {
        let visible = 3.0;
        let released = lock_release_yaw(-3.0, visible);
        assert!((released - (std::f32::consts::TAU - 3.0)).abs() < 1e-4);
        assert!((released - visible).abs() < std::f32::consts::PI);
    }

    #[test]
    fn held_camera_axis_uses_web_ramp_and_mouse_speed() {
        assert_eq!(camera_axis_step_degrees(1.0, 0.0, 49.0, 1.0 / 60.0), 0.0);
        let half = camera_axis_step_degrees(1.0, 0.5, 49.0, 1.0 / 60.0);
        let expected_half = 0.5 * 1.5 * (49.0 + 1.0) * 1.5 / 60.0;
        assert!((half - expected_half).abs() < 1e-6);
        let capped = camera_axis_step_degrees(1.0, 9.0, 49.0, 1.0 / 60.0);
        let expected_cap = 1.5 * 2.5 * (49.0 + 1.0) * 1.5 / 60.0;
        assert!((capped - expected_cap).abs() < 1e-6);

        assert_eq!(update_camera_axis_hold(0.8, false, false, 0.1), 0.0);
        assert_eq!(update_camera_axis_hold(0.8, true, true, 0.1), 0.0);
        assert_eq!(update_camera_axis_hold(1.48, true, false, 0.1), 1.5);
    }

    #[test]
    fn free_mouse_look_is_immediate_but_locked_look_stays_hidden() {
        let mut free = ThirdPersonState::default();
        apply_camera_look(&mut free, false, 10.0, 0.0);
        assert!((free.yaw + 10.0 * DEG).abs() < 1e-6);
        assert_eq!(free.smooth_yaw, free.yaw);
        assert!(free.free_look_pending);

        let mut locked = ThirdPersonState::default();
        apply_camera_look(&mut locked, true, 10.0, 0.0);
        assert!((locked.yaw + 10.0 * DEG).abs() < 1e-6);
        assert_eq!(locked.smooth_yaw, 0.0);
        assert!(!locked.free_look_pending);
    }

    #[test]
    fn held_turn_rotates_only_name_state_fighter() {
        let mut motion = crate::components::fighter::MotionState {
            movement_mode: meteor_core::MovementMode::Idle,
            velocity: Vec3::ZERO,
            facing: 0.0,
        };
        assert!(apply_keyboard_yaw_to_fighter(
            &mut motion,
            meteor_core::FighterState::Name,
            false,
            12.0,
        ));
        assert!((motion.facing + 12.0 * DEG).abs() < 1e-6);

        let facing = motion.facing;
        assert!(!apply_keyboard_yaw_to_fighter(
            &mut motion,
            meteor_core::FighterState::Attack1,
            false,
            12.0,
        ));
        assert!(!apply_keyboard_yaw_to_fighter(
            &mut motion,
            meteor_core::FighterState::Name,
            true,
            12.0,
        ));
        assert_eq!(motion.facing, facing);
    }

    #[test]
    fn production_camera_input_publishes_same_tick_basis_and_facing() {
        let mut time: Time = Time::default();
        time.advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ArrowRight);
        keys.clear_just_pressed(KeyCode::ArrowRight);
        let mut app = App::new();
        app.insert_resource(time)
            .insert_resource(keys)
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<crate::app_state::Keymap>()
            .init_resource::<Settings>()
            .init_resource::<crate::components::fighter::LockOnController>()
            .init_resource::<CameraBasis>()
            .init_resource::<Messages<MouseMotion>>()
            .init_resource::<Messages<MouseWheel>>()
            .add_systems(
                Update,
                (apply_follow_camera_input, prepare_move_basis).chain(),
            );
        let camera_entity = app
            .world_mut()
            .spawn((
                GameplayCamera,
                ThirdPersonState {
                    yaw_axis_hold: 0.5,
                    ..default()
                },
            ))
            .id();
        let fighter = app
            .world_mut()
            .spawn((
                LocalFighter,
                crate::components::fighter::MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                crate::components::fighter::SmState::default(),
            ))
            .id();

        app.update();

        let camera = app.world().get::<ThirdPersonState>(camera_entity).unwrap();
        let facing = app
            .world()
            .get::<crate::components::fighter::MotionState>(fighter)
            .unwrap()
            .facing;
        let basis = app.world().resource::<CameraBasis>().yaw;
        assert!(camera.yaw < 0.0);
        assert_eq!(camera.smooth_yaw, camera.yaw);
        assert_eq!(facing, camera.yaw);
        assert_eq!(basis, camera.yaw);
    }

    fn airborne_pointer_app() -> (App, Entity) {
        use crate::components::fighter::*;
        use crate::plugins::movement_plugin::*;
        let mut app = App::new();
        crate::plugins::configure_fighter_frame_sets(&mut app);
        app.init_resource::<Time>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<crate::app_state::Keymap>()
            .init_resource::<Settings>()
            .init_resource::<LockOnController>()
            .init_resource::<CameraBasis>()
            .init_resource::<Messages<MouseMotion>>()
            .init_resource::<Messages<MouseWheel>>()
            .init_resource::<crate::plugins::mobile_controls_plugin::MobileLookDelta>()
            .init_resource::<crate::plugins::item_plugin::ItemBuffs>()
            .init_resource::<crate::plugins::input_plugin::LocalCrouchInput>()
            .init_resource::<crate::plugins::input_plugin::LocalGuardInput>()
            .insert_resource(StageCollider {
                mesh: Some(crate::plugins::stage_collider::StageMeshCollider::default()),
            })
            .add_plugins(MovementPlugin)
            .add_systems(
                Update,
                (apply_follow_camera_input, prepare_move_basis)
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::Movement),
            );
        app.world_mut()
            .spawn((GameplayCamera, ThirdPersonState::default()));
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        let mut air = FighterAir::default();
        air.air.start_fall(0.0, 12.0);
        let fighter = app
            .world_mut()
            .spawn((
                LocalFighter,
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 5,
                    weapon_slots: [5, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Transform::from_xyz(0.0, 10.0, 0.0),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Airborne,
                    velocity: Vec3::new(0.0, 0.0, 12.0),
                    facing: 0.0,
                },
                air,
                FighterDodge::default(),
                FighterWall::default(),
                PoseRuntime::idle(),
                EntryBlend::default(),
                SmState {
                    current: meteor_core::FighterState::Jump,
                    ..default()
                },
                CombatState {
                    state: meteor_core::FighterState::Jump,
                },
            ))
            .id();
        (app, fighter)
    }

    #[test]
    fn production_air_mouse_and_touch_turn_facing_before_direction_drive() {
        use crate::components::fighter::MotionState;
        use crate::plugins::movement_plugin::FighterAir;
        for fps in [30.0_f32, 60.0, 120.0] {
            for touch in [false, true] {
                for forward in [false, true] {
                    let (mut app, fighter) = airborne_pointer_app();
                    let world = app.world_mut();
                    world
                        .resource_mut::<Time>()
                        .advance_by(std::time::Duration::from_secs_f32(1.0 / fps));
                    if forward {
                        world
                            .resource_mut::<ButtonInput<KeyCode>>()
                            .press(KeyCode::KeyW);
                    }

                    if touch {
                        world.resource_mut::<crate::plugins::mobile_controls_plugin::MobileLookDelta>()
                            .0 = Vec2::new(240.0, 0.0);
                    } else {
                        world
                            .resource_mut::<Messages<MouseMotion>>()
                            .write(MouseMotion {
                                delta: Vec2::new(126.0, 0.0),
                            });
                    }
                    app.update();
                    let motion = app.world().get::<MotionState>(fighter).unwrap();
                    assert!((motion.facing + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
                    let air = &app.world().get::<FighterAir>(fighter).unwrap().air;
                    assert!((air.air_time - 1.0 / fps).abs() < 1e-6);
                    assert!(air.planar_z > 11.0, "turn must preserve forward inertia");
                    if forward {
                        assert!(air.planar_x < -0.05, "same-tick W must use the new facing");
                        assert!(app.world().get::<Transform>(fighter).unwrap().translation.x < 0.0);
                    } else {
                        assert!(
                            air.planar_x.abs() < 1e-6,
                            "look alone must not rotate velocity"
                        );
                    }
                    let facing = motion.facing;
                    app.update();
                    assert_eq!(
                        app.world().get::<MotionState>(fighter).unwrap().facing,
                        facing,
                        "pointer delta must only be consumed once"
                    );
                }
            }
        }
    }

    #[test]
    fn production_air_pointer_turn_does_not_require_auto_camera() {
        let (mut app, fighter) = airborne_pointer_app();
        app.world_mut().resource_mut::<Settings>().auto_camera = false;
        app.world_mut()
            .resource_mut::<Messages<MouseMotion>>()
            .write(MouseMotion {
                delta: Vec2::new(126.0, 0.0),
            });
        app.update();
        let facing = app
            .world()
            .get::<crate::components::fighter::MotionState>(fighter)
            .unwrap()
            .facing;
        assert!((facing + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }

    #[test]
    fn production_action_camera_consumes_horizontal_input_and_keeps_pitch() {
        use crate::components::fighter::{MotionState, SmState, WeaponSwitchActive};
        use crate::plugins::mobile_controls_plugin::MobileLookDelta;
        use crate::plugins::movement_plugin::FighterAir;
        use meteor_core::FighterState;
        for state in [
            FighterState::Attack1,
            FighterState::Attack2,
            FighterState::Attack3,
            FighterState::Dodge,
            FighterState::Burst,
            FighterState::Guard,
            FighterState::BeHit,
            FighterState::Down,
            FighterState::Fake,
            FighterState::Dead,
            FighterState::Cure,
            FighterState::Jump,
            FighterState::Name,
        ] {
            for airborne in [false, true] {
                let (mut app, fighter) = airborne_pointer_app();
                app.world_mut().get_mut::<SmState>(fighter).unwrap().current = state;
                if state == FighterState::Name {
                    app.world_mut()
                        .entity_mut(fighter)
                        .insert(WeaponSwitchActive::default());
                }
                if state == FighterState::Jump {
                    app.world_mut()
                        .get_mut::<FighterAir>(fighter)
                        .unwrap()
                        .air
                        .motion_mode = meteor_physics::jump::AirMotionMode::Wall;
                }
                if !airborne && state != FighterState::Jump {
                    app.world_mut().entity_mut(fighter).remove::<FighterAir>();
                }
                app.world_mut()
                    .resource_mut::<Time>()
                    .advance_by(std::time::Duration::from_secs_f32(0.1));
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .press(KeyCode::ArrowRight);
                app.world_mut()
                    .resource_mut::<Messages<MouseMotion>>()
                    .write(MouseMotion {
                        delta: Vec2::new(28.0, 8.0),
                    });
                app.world_mut().resource_mut::<MobileLookDelta>().0 = Vec2::new(30.0, 8.0);
                app.update();
                let camera = app
                    .world_mut()
                    .query::<&ThirdPersonState>()
                    .single(app.world())
                    .unwrap();
                assert_eq!(camera.yaw, 0.0, "{state:?}, airborne={airborne}");
                assert_eq!(camera.smooth_yaw, 0.0);
                assert_ne!(camera.pitch, DEFAULT_PITCH, "pitch remains independent");
                assert_eq!(app.world().get::<MotionState>(fighter).unwrap().facing, 0.0);
                assert_eq!(app.world().resource::<MobileLookDelta>().0, Vec2::ZERO);
                app.world_mut().get_mut::<SmState>(fighter).unwrap().current = FighterState::Name;
                app.world_mut()
                    .entity_mut(fighter)
                    .remove::<WeaponSwitchActive>();
                if let Some(mut air) = app.world_mut().get_mut::<FighterAir>(fighter) {
                    air.air.motion_mode = meteor_physics::jump::AirMotionMode::Ballistic;
                }
                app.world_mut()
                    .resource_mut::<ButtonInput<KeyCode>>()
                    .reset_all();
                app.update();
                assert_eq!(
                    app.world().resource::<CameraBasis>().yaw,
                    0.0,
                    "no deferred turn after recovery"
                );
                app.world_mut()
                    .resource_mut::<Messages<MouseMotion>>()
                    .write(MouseMotion {
                        delta: Vec2::new(28.0, 0.0),
                    });
                app.update();
                assert!(
                    app.world().resource::<CameraBasis>().yaw < 0.0,
                    "fresh input works after recovery"
                );
            }
        }
    }

    #[test]
    fn production_air_pointer_respects_action_lock_and_replay_ownership() {
        use crate::components::fighter::{MotionState, SmState, WeaponSwitchActive};
        use crate::plugins::movement_plugin::FighterAir;
        use meteor_physics::jump::AirMotionMode;
        for gate in [
            "lock", "attack", "wall", "curve", "switch", "replay", "ledge",
        ] {
            let (mut app, fighter) = airborne_pointer_app();
            let world = app.world_mut();
            match gate {
                "lock" => {
                    let target = world.spawn(Transform::from_xyz(5.0, 10.0, 0.0)).id();
                    world
                        .resource_mut::<crate::components::fighter::LockOnController>()
                        .lock_entity = Some(target);
                }
                "attack" => {
                    world.get_mut::<SmState>(fighter).unwrap().current =
                        meteor_core::FighterState::Attack1
                }
                "wall" => {
                    world
                        .get_mut::<FighterAir>(fighter)
                        .unwrap()
                        .air
                        .motion_mode = AirMotionMode::Wall
                }
                "curve" => {
                    world
                        .get_mut::<FighterAir>(fighter)
                        .unwrap()
                        .air
                        .motion_mode = AirMotionMode::Curve
                }
                "switch" => {
                    world
                        .entity_mut(fighter)
                        .insert(WeaponSwitchActive::default());
                }
                "replay" => {
                    world.init_resource::<crate::plugins::replay_plugin::MrcPlayback>();
                    world
                        .resource_mut::<crate::plugins::replay_plugin::MrcPlayback>()
                        .active = true;
                }
                "ledge" => {
                    world.get_mut::<SmState>(fighter).unwrap().current =
                        meteor_core::FighterState::Name
                }
                _ => unreachable!(),
            }
            world
                .resource_mut::<Messages<MouseMotion>>()
                .write(MouseMotion {
                    delta: Vec2::new(28.0, 0.0),
                });
            app.update();
            let facing = app.world().get::<MotionState>(fighter).unwrap().facing;
            let expected = if gate == "ledge" { -20.0 * DEG } else { 0.0 };
            assert!((facing - expected).abs() < 1e-6, "gate={gate}: {facing}");
            let yaw = app.world().resource::<CameraBasis>().yaw;
            if gate == "attack" || gate == "wall" || gate == "switch" {
                assert_eq!(yaw, 0.0, "action blocks horizontal camera input");
            } else {
                assert!(yaw < 0.0, "camera remains usable: {gate}");
            }
        }
    }

    #[test]
    fn vertical_anchor_delta_is_injected_only_on_ground() {
        assert!((inject_ground_anchor_y(10.0, 5.0, 3.0, false) - 12.0).abs() < 1e-6);
        assert!((inject_ground_anchor_y(10.0, 5.0, 3.0, true) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn first_free_camera_frame_keeps_web_follow_lag_instead_of_hard_cutting() {
        let alpha = camera_position_alpha(false, false, false, false, 1.0 / 60.0);
        let expected = 1.0 - (-FOLLOW_GROUND / 60.0).exp();
        assert!((alpha - expected).abs() < 1e-6);
        assert!(alpha > 0.0 && alpha < 1.0);
    }

    #[test]
    fn only_unlock_snap_hard_cuts_while_lock_uses_its_own_response() {
        assert_eq!(camera_position_alpha(false, true, false, false, 0.0), 1.0);
        assert!(
            (camera_position_alpha(true, false, false, false, 1.0 / 60.0)
                - LOCK_POSITION_RESPONSE / 60.0)
                .abs()
                < 1e-6
        );
        assert_eq!(
            camera_position_alpha(true, false, false, false, 0.31),
            LOCK_POSITION_STALL_ALPHA
        );
    }

    #[test]
    fn vertical_fov_wide_keeps_vertical() {
        let v43 = vertical_fov_for_aspect(4.0 / 3.0);
        let v169 = vertical_fov_for_aspect(16.0 / 9.0);
        assert!((v43 - v169).abs() < 1e-3);

        let expect = 2.0 * ((30.0f32 * std::f32::consts::PI / 180.0).tan() / (4.0 / 3.0)).atan();
        assert!((v43 - expect).abs() < 1e-4);
    }

    #[test]
    fn resolve_collision_pulls_in_with_skin() {
        let from = Vec3::ZERO;
        let ideal = Vec3::new(0.0, 0.0, 5.0);
        let r = resolve_collision(
            from,
            ideal,
            |_, _, max| {
                if max > 1.0 {
                    Some(1.0)
                } else {
                    None
                }
            },
        );
        assert!((r.z - 0.7).abs() < 1e-4);

        let r = resolve_collision(from, ideal, |_, _, _| None);
        assert!((r.z - 5.0).abs() < 1e-4);

        let r = resolve_collision(from, ideal, |_, _, _| Some(0.2));
        assert!(r.z.abs() < 1e-4);
    }

    #[test]
    fn locked_eye_stays_behind_player() {
        let mut side = 1.0_f32;
        let focus = Vec3::new(0.0, 2.88, 0.0);
        let eye = locked_eye(
            focus,
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 2.0),
            &mut side,
            0.0,
        );

        assert!(eye.z < 0.0);
        assert!(eye.x < 0.0);

        assert!((eye.y - (focus.y + 10.0 * MODEL_SCALE)).abs() < 1e-4);

        let dist = (eye - focus).length();
        assert!((dist - 14.4).abs() < 0.5);
    }

    #[test]
    fn vertical_anchor_air_rates() {
        let grounded_target_y = 3.4_f32;
        let mut target_y =
            VerticalAnchor::update(grounded_target_y, grounded_target_y, 1.0 / 60.0, false);
        let airborne_look_y = 8.852 + 3.4;
        let y0 = target_y;
        target_y = VerticalAnchor::update(target_y, airborne_look_y, 1.0 / 60.0, true);
        assert!(target_y > y0 && target_y < airborne_look_y - 0.5);
        let mut rising = 0;
        let mut prev = target_y;
        for _ in 0..45 {
            target_y = VerticalAnchor::update(target_y, airborne_look_y, 1.0 / 60.0, true);
            if target_y > prev + 1e-4 {
                rising += 1;
            }
            prev = target_y;
        }
        assert!(rising > 20);
        assert!((target_y - airborne_look_y).abs() < 0.15);
        for _ in 0..120 {
            target_y = VerticalAnchor::update(target_y, grounded_target_y, 1.0 / 60.0, false);
        }
        assert!((target_y - grounded_target_y).abs() < 0.05);
    }
}
