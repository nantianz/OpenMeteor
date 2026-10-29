use crate::app_state::{AppState, Keymap};
use crate::components::fighter::{
    DeathRespawn, FighterHitReaction, GunAimState, SmState, WeaponSwitchActive,
};
use crate::plugins::stage_plugin::LocalFighter;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use meteor_core::FighterState;
use meteor_input::command_parser::{CommandInput, Dir};

const DODGE_TAP_WINDOW_SECONDS: f64 = 0.25;

#[derive(Resource, Debug, Default)]
pub struct LocalCommandInput(pub CommandInput);

#[derive(Resource, Debug, Default)]
pub(crate) struct LocalCrouchInput {
    pub held: bool,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct LocalGuardInput {
    pub held: bool,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct LocalCombatInput {
    pub edges: Vec<LocalCombatInputEdge>,

    pub raw_bindings: Vec<crate::app_state::InputBinding>,

    pub command_update_ms: Option<f64>,

    pub initial_crouch_held: bool,

    pub(crate) window_event_floor: usize,
}

impl LocalCombatInput {
    fn begin_frame(&mut self) {
        self.edges.clear();
        self.raw_bindings.clear();
        self.command_update_ms = None;
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    fn saw_raw_binding(&self, binding: crate::app_state::InputBinding) -> bool {
        self.raw_bindings.contains(&binding)
    }
}

#[cfg(test)]
pub(crate) fn local_discrete_action_pressed(
    input: Option<&LocalCombatInput>,
    edge: LocalCombatInputEdge,
    binding: crate::app_state::InputBinding,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
) -> bool {
    input.is_some_and(|input| input.edges.contains(&edge))
        || (input.is_none_or(|input| !input.raw_bindings.contains(&binding))
            && binding.just_pressed(keys, mouse))
}

pub(crate) fn ordered_local_discrete_actions(
    input: Option<&LocalCombatInput>,
    bindings: &[(LocalCombatInputEdge, crate::app_state::InputBinding)],
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
) -> Vec<LocalCombatInputEdge> {
    let mut actions = input
        .map(|input| {
            input
                .edges
                .iter()
                .copied()
                .filter(|edge| bindings.iter().any(|(candidate, _)| candidate == edge))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (edge, binding) in bindings {
        if input.is_some_and(|input| input.raw_bindings.contains(binding)) {
            continue;
        }
        if binding.just_pressed(keys, mouse) {
            actions.push(*edge);
        }
    }
    actions
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalCombatInputEdge {
    DirectionPress(Dir),
    DirectionRelease(Dir),
    Dodge { direction: Dir, burst: bool },
    DirectDodge { burst: bool },
    Attack,
    GuardPress,
    GuardRelease,
    CrouchPress,
    CrouchRelease,
    LockToggle,
    WeaponSwitch,
    QiBurst,
    Fake,
    Jump,
    Cure,
    DropWeapon,
    DropItem,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct SyntheticInputStream {
    edges: Vec<SyntheticInputEdge>,
    frame_clock_seconds: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyntheticAction {
    Attack,
    Jump,
    Guard,
    Crouch,
    LockToggle,
    WeaponSwitch,
    QiBurst,
    Fake,
    Cure,
    DropWeapon,
    DropItem,
}

impl SyntheticInputStream {
    pub(crate) fn push_live_binding(
        &mut self,
        binding: crate::app_state::InputBinding,
        pressed: bool,
    ) {
        self.edges.push(SyntheticInputEdge::Binding {
            edge: DirectionInputEdge {
                binding,
                pressed,
                repeat: false,
            },
            record_direction: true,
            detect_dodge: true,
            event_seconds: None,
        });
    }

    pub(crate) fn push_mobile_direction(
        &mut self,
        binding: crate::app_state::InputBinding,
        pressed: bool,
    ) {
        self.edges.push(SyntheticInputEdge::Binding {
            edge: DirectionInputEdge {
                binding,
                pressed,
                repeat: false,
            },
            record_direction: true,
            detect_dodge: false,
            event_seconds: None,
        });
    }

    pub(crate) fn push_mobile_movement(
        &mut self,
        binding: crate::app_state::InputBinding,
        pressed: bool,
    ) {
        self.edges.push(SyntheticInputEdge::Binding {
            edge: DirectionInputEdge {
                binding,
                pressed,
                repeat: false,
            },
            record_direction: false,
            detect_dodge: false,
            event_seconds: None,
        });
    }

    pub(crate) fn push_replay_binding(
        &mut self,
        binding: crate::app_state::InputBinding,
        pressed: bool,
        event_seconds: Option<f64>,
    ) {
        self.edges.push(SyntheticInputEdge::Binding {
            edge: DirectionInputEdge {
                binding,
                pressed,
                repeat: false,
            },
            record_direction: false,
            detect_dodge: false,
            event_seconds,
        });
    }

    pub(crate) fn push_dodge(&mut self, direction: Dir, burst: bool) {
        self.edges
            .push(SyntheticInputEdge::Dodge { direction, burst });
    }

    pub(crate) fn push_action(
        &mut self,
        binding: crate::app_state::InputBinding,
        action: SyntheticAction,
        pressed: bool,
        event_seconds: Option<f64>,
    ) {
        self.edges.push(SyntheticInputEdge::Action {
            binding,
            action,
            pressed,
            event_seconds,
        });
    }

    pub(crate) fn set_frame_clock_seconds(&mut self, seconds: f64) {
        if seconds.is_finite() {
            self.frame_clock_seconds = Some(seconds.max(0.0));
        }
    }

    fn take_frame_clock_seconds(&mut self) -> Option<f64> {
        self.frame_clock_seconds.take()
    }

    pub(crate) fn cancel_binding(&mut self, binding: crate::app_state::InputBinding) {
        self.edges
            .push(SyntheticInputEdge::CancelBinding { binding });
    }

    fn drain(&mut self) -> impl Iterator<Item = SyntheticInputEdge> + '_ {
        self.edges.drain(..)
    }

    pub(crate) fn clear(&mut self) {
        self.edges.clear();
        self.frame_clock_seconds = None;
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.edges.len()
    }

    #[cfg(test)]
    pub(crate) fn contains_dodge(&self, direction: Dir, burst: bool) -> bool {
        self.edges
            .contains(&SyntheticInputEdge::Dodge { direction, burst })
    }

    #[cfg(test)]
    pub(crate) fn contains_cancel(&self, binding: crate::app_state::InputBinding) -> bool {
        self.edges
            .contains(&SyntheticInputEdge::CancelBinding { binding })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SyntheticInputEdge {
    Binding {
        edge: DirectionInputEdge,
        record_direction: bool,
        detect_dodge: bool,
        event_seconds: Option<f64>,
    },
    Dodge {
        direction: Dir,
        burst: bool,
    },
    Action {
        binding: crate::app_state::InputBinding,
        action: SyntheticAction,
        pressed: bool,
        event_seconds: Option<f64>,
    },
    CancelBinding {
        binding: crate::app_state::InputBinding,
    },
}

#[derive(Resource, Debug, Default)]
pub(crate) struct LocalJumpInput {
    pub pressed: bool,
    pub released: bool,

    pub action_accepted: bool,
    pub held: bool,
    pub hold_seconds: f32,
    pressed_at: Option<f64>,
}

impl LocalJumpInput {
    fn begin_frame(&mut self, now: f64) {
        self.pressed = false;
        self.released = false;
        self.action_accepted = false;
        if let Some(start) = self.pressed_at {
            self.hold_seconds = (now - start).max(0.0) as f32;
        } else if !self.held {
            self.hold_seconds = 0.0;
        }
    }

    fn press(&mut self, now: f64) {
        self.pressed = true;
        self.held = true;
        self.hold_seconds = 0.0;
        self.pressed_at = Some(now);
    }

    fn release(&mut self, now: f64) -> bool {
        let Some(start) = self.pressed_at.take() else {
            return false;
        };
        self.released = true;
        self.held = false;
        self.hold_seconds = (now - start).max(0.0) as f32;
        true
    }

    pub(crate) fn consume_charge(&mut self) {
        self.pressed_at = None;
        self.held = false;
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DodgeGesture {
    pub direction: Vec2,
    pub burst: bool,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct DesktopDodgeInput {
    last_tap: [Option<f64>; 4],
    pub pending: Option<DodgeGesture>,

    pub pending_state_accepted: bool,

    pub pending_roll: Option<bool>,
}

impl DesktopDodgeInput {
    fn detect_tap(
        &mut self,
        index: usize,
        direction: Vec2,
        burst: bool,
        now: f64,
    ) -> Option<DodgeGesture> {
        let previous = self.last_tap[index];
        if previous
            .is_some_and(|previous| now >= previous && now - previous < DODGE_TAP_WINDOW_SECONDS)
        {
            self.last_tap[index] = None;
            Some(DodgeGesture { direction, burst })
        } else {
            self.last_tap[index] = Some(now);
            None
        }
    }

    fn register_tap(&mut self, index: usize, direction: Vec2, burst: bool, now: f64) {
        if let Some(gesture) = self.detect_tap(index, direction, burst, now) {
            if self.pending.is_none() {
                self.pending = Some(gesture);
                self.pending_state_accepted = false;
                self.pending_roll = None;
            }
        }
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalCommandInput>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .init_resource::<LocalCombatInput>()
            .init_resource::<LocalJumpInput>()
            .init_resource::<DesktopDodgeInput>()
            .init_resource::<SyntheticInputStream>()
            .add_systems(
                OnEnter(AppState::Playing),
                (reset_local_input_state, reset_gameplay_window_events).chain(),
            )
            .add_systems(OnExit(AppState::Playing), reset_local_input_state)
            .add_systems(
                Update,
                capture_local_command_input
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .before(crate::plugins::FighterFrameSet::PoseDecision),
            )
            .add_systems(
                Update,
                capture_local_hold_inputs
                    .after(capture_local_command_input)
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .before(crate::plugins::FighterFrameSet::PoseDecision)
                    .run_if(in_state(AppState::Playing)),
            );
    }
}

fn reset_gameplay_window_events(
    events: Res<Messages<bevy::window::WindowEvent>>,
    mut combat: ResMut<LocalCombatInput>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
) {
    combat.window_event_floor = events.oldest_message_count() + events.len();
    keys.clear();
    mouse.clear();
}

fn reset_local_input_state(
    mut command_input: ResMut<LocalCommandInput>,
    mut crouch: ResMut<LocalCrouchInput>,
    mut guard: ResMut<LocalGuardInput>,
    mut combat: ResMut<LocalCombatInput>,
    mut jump: ResMut<LocalJumpInput>,
    mut dodge: ResMut<DesktopDodgeInput>,
    mut synthetic: ResMut<SyntheticInputStream>,
) {
    command_input.0.clear();
    crouch.held = false;
    guard.held = false;
    combat.clear();
    jump.clear();
    dodge.clear();
    synthetic.clear();
}

#[derive(Debug, Clone, Copy, Default)]
struct CrouchInputFrame {
    pressed: bool,
    released: bool,
    physically_held: bool,
    input_blocked: bool,
    switching_weapon: bool,
    stunned: bool,
    dead: bool,
    gun_aiming: bool,
}

fn update_crouch_latch(crouch: &mut LocalCrouchInput, mut frame: CrouchInputFrame) {
    if frame.pressed && frame.released {
        frame.pressed = frame.physically_held;
        frame.released = !frame.physically_held;
    }
    if frame.dead {
        crouch.held = false;
        return;
    }

    if frame.input_blocked {
        if frame.released {
            crouch.held = false;
        }
        return;
    }

    if frame.switching_weapon {
        if frame.released {
            crouch.held = false;
        } else if frame.pressed {
            crouch.held = true;
        }
        return;
    }

    if frame.stunned {
        return;
    }
    if frame.released {
        crouch.held = false;
    } else if frame.pressed && !frame.gun_aiming {
        crouch.held = true;
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct GuardInputFrame {
    pressed: bool,
    released: bool,
    physically_held: bool,
    input_blocked: bool,
    dead: bool,
}

fn update_guard_latch(guard: &mut LocalGuardInput, mut frame: GuardInputFrame) {
    if frame.pressed && frame.released {
        frame.pressed = frame.physically_held;
        frame.released = !frame.physically_held;
    }
    if frame.dead {
        guard.held = false;
        return;
    }

    if frame.input_blocked {
        if frame.released {
            guard.held = false;
        }
        return;
    }

    if frame.released {
        guard.held = false;
    } else if frame.pressed {
        guard.held = true;
    }
}

type LocalCrouchOwnerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static SmState,
        &'static GunAimState,
        &'static FighterHitReaction,
        &'static DeathRespawn,
        Option<&'static WeaponSwitchActive>,
    ),
    With<LocalFighter>,
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn capture_local_hold_inputs(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keymap: Res<Keymap>,
    dbg: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    owner: LocalCrouchOwnerQuery<'_, '_>,
    mut crouch: ResMut<LocalCrouchInput>,
    mut guard: ResMut<LocalGuardInput>,
    net: Option<Res<super::network_plugin::NetState>>,
) {
    if net.is_some_and(|net| net.battle_result.is_some()) {
        crouch.held = false;
        guard.held = false;
        return;
    }
    let Ok((state, gun_aim, reaction, death, switching)) = owner.single() else {
        return;
    };

    if dbg.is_some_and(|dbg| dbg.captures_input()) {
        return;
    }
    let input_blocked = performance
        .as_deref()
        .is_some_and(|performance| performance.input_blocked);
    let stunned = reaction.current.is_some()
        || matches!(state.current, FighterState::BeHit | FighterState::Down);
    update_crouch_latch(
        &mut crouch,
        CrouchInputFrame {
            pressed: keymap.crouch.just_pressed(&keys, &mouse),
            released: keymap.crouch.just_released(&keys, &mouse),
            physically_held: keymap.crouch.pressed(&keys, &mouse),
            input_blocked,
            switching_weapon: switching.is_some(),
            stunned,
            dead: death.active || state.current == FighterState::Dead,
            gun_aiming: gun_aim.aiming,
        },
    );
    update_guard_latch(
        &mut guard,
        GuardInputFrame {
            pressed: keymap.guard.just_pressed(&keys, &mouse),
            released: keymap.guard.just_released(&keys, &mouse),
            physically_held: keymap.guard.pressed(&keys, &mouse),
            input_blocked,
            dead: death.active || state.current == FighterState::Dead,
        },
    );
}

#[cfg(test)]
fn update_desktop_dodge_input(
    dodge: &mut DesktopDodgeInput,
    keymap: &Keymap,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    now: f64,
) {
    update_desktop_dodge_input_skipping(dodge, keymap, keys, mouse, now, [false; 4]);
}

fn update_desktop_dodge_input_skipping(
    dodge: &mut DesktopDodgeInput,
    keymap: &Keymap,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    now: f64,
    ordered_edges_seen: [bool; 4],
) {
    let burst = keymap.guard.pressed(keys, mouse);
    for (index, binding, direction) in [
        (0, keymap.move_up, Vec2::new(0.0, 1.0)),
        (1, keymap.move_down, Vec2::new(0.0, -1.0)),
        (2, keymap.move_left, Vec2::new(-1.0, 0.0)),
        (3, keymap.move_right, Vec2::new(1.0, 0.0)),
    ] {
        if !ordered_edges_seen[index] && binding.just_pressed(keys, mouse) {
            dodge.register_tap(index, direction, burst, now);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectionInputEdge {
    binding: crate::app_state::InputBinding,
    pressed: bool,
    repeat: bool,
}

impl From<DirectionInputEdge> for SyntheticInputEdge {
    fn from(edge: DirectionInputEdge) -> Self {
        Self::Binding {
            edge,
            record_direction: true,
            detect_dodge: true,
            event_seconds: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct OrderedInputInitial {
    guard_held: bool,
    crouch_held: bool,
}

#[cfg(test)]
fn apply_ordered_direction_edges(
    command_input: &mut CommandInput,
    dodge: &mut DesktopDodgeInput,
    keymap: &Keymap,
    edges: impl IntoIterator<Item = DirectionInputEdge>,
    now_seconds: f64,
    initial_guard_held: bool,
) -> [bool; 4] {
    let directions = [
        (keymap.move_up, Dir::Up, Vec2::Y),
        (keymap.move_down, Dir::Down, Vec2::NEG_Y),
        (keymap.move_left, Dir::Left, Vec2::NEG_X),
        (keymap.move_right, Dir::Right, Vec2::X),
    ];
    let mut seen = [false; 4];
    let mut guard_held = initial_guard_held;
    for edge in edges {
        let mut direction_matched = false;
        for (index, (binding, direction, dodge_direction)) in directions.iter().enumerate() {
            if edge.binding != *binding {
                continue;
            }
            direction_matched = true;
            seen[index] = true;
            if edge.pressed {
                if !edge.repeat {
                    command_input.push_direction(*direction, now_seconds * 1_000.0);
                    dodge.register_tap(index, *dodge_direction, guard_held, now_seconds);
                }
            } else {
                command_input.release_direction(*direction);
            }
            break;
        }

        if !direction_matched && edge.binding == keymap.guard && !edge.repeat {
            guard_held = edge.pressed;
        }
    }
    seen
}

#[cfg(test)]
fn apply_ordered_jump_edges(
    jump: &mut LocalJumpInput,
    binding: crate::app_state::InputBinding,
    edges: impl IntoIterator<Item = DirectionInputEdge>,
    now_seconds: f64,
) -> bool {
    let mut seen = false;
    for edge in edges {
        if edge.binding != binding {
            continue;
        }
        seen = true;
        if edge.repeat {
            continue;
        }
        if edge.pressed {
            jump.press(now_seconds);
        } else {
            jump.release(now_seconds);
        }
    }
    seen
}

fn collect_ordered_combat_edges<I, E>(
    combat: &mut LocalCombatInput,
    dodge: &mut DesktopDodgeInput,
    jump: &mut LocalJumpInput,
    keymap: &Keymap,
    edges: I,
    now_seconds: f64,
    initial: OrderedInputInitial,
) -> ([bool; 4], bool)
where
    I: IntoIterator<Item = E>,
    E: Into<SyntheticInputEdge>,
{
    combat.initial_crouch_held = initial.crouch_held;
    let directions = [
        (keymap.move_up, Dir::Up, Vec2::Y),
        (keymap.move_down, Dir::Down, Vec2::NEG_Y),
        (keymap.move_left, Dir::Left, Vec2::NEG_X),
        (keymap.move_right, Dir::Right, Vec2::X),
    ];
    let mut direction_seen = [false; 4];
    let mut jump_seen = false;
    let mut guard_held = initial.guard_held;
    for captured in edges {
        let (edge, record_direction, detect_dodge, event_seconds) = match captured.into() {
            SyntheticInputEdge::Dodge { direction, burst } => {
                combat
                    .edges
                    .push(LocalCombatInputEdge::Dodge { direction, burst });
                continue;
            }
            SyntheticInputEdge::Action {
                binding,
                action,
                pressed,
                event_seconds,
            } => {
                if !combat.raw_bindings.contains(&binding) {
                    combat.raw_bindings.push(binding);
                }
                for (index, (direction_binding, _, _)) in directions.iter().enumerate() {
                    if binding == *direction_binding {
                        direction_seen[index] = true;
                    }
                }
                let edge_seconds = event_seconds.unwrap_or(now_seconds);
                match action {
                    SyntheticAction::Attack if pressed => {
                        combat.edges.push(LocalCombatInputEdge::Attack)
                    }
                    SyntheticAction::Jump if pressed => {
                        jump.press(edge_seconds);
                        jump_seen = true;
                    }
                    SyntheticAction::Jump => {
                        jump_seen = true;
                        if jump.release(edge_seconds) {
                            combat.edges.push(LocalCombatInputEdge::Jump);
                        }
                    }
                    SyntheticAction::Guard => {
                        guard_held = pressed;
                        combat.edges.push(if pressed {
                            LocalCombatInputEdge::GuardPress
                        } else {
                            LocalCombatInputEdge::GuardRelease
                        });
                    }
                    SyntheticAction::Crouch => {
                        combat.edges.push(if pressed {
                            LocalCombatInputEdge::CrouchPress
                        } else {
                            LocalCombatInputEdge::CrouchRelease
                        });
                    }
                    SyntheticAction::LockToggle if pressed => {
                        combat.edges.push(LocalCombatInputEdge::LockToggle)
                    }
                    SyntheticAction::WeaponSwitch if pressed => {
                        combat.edges.push(LocalCombatInputEdge::WeaponSwitch)
                    }
                    SyntheticAction::QiBurst if pressed => {
                        combat.edges.push(LocalCombatInputEdge::QiBurst)
                    }
                    SyntheticAction::Fake if pressed => {
                        combat.edges.push(LocalCombatInputEdge::Fake)
                    }
                    SyntheticAction::Cure if pressed => {
                        combat.edges.push(LocalCombatInputEdge::Cure)
                    }
                    SyntheticAction::DropWeapon if pressed => {
                        combat.edges.push(LocalCombatInputEdge::DropWeapon)
                    }
                    SyntheticAction::DropItem if pressed => {
                        combat.edges.push(LocalCombatInputEdge::DropItem)
                    }
                    _ => {}
                }
                continue;
            }
            SyntheticInputEdge::CancelBinding { binding } => {
                if !combat.raw_bindings.contains(&binding) {
                    combat.raw_bindings.push(binding);
                }
                for (index, (direction_binding, _, _)) in directions.iter().enumerate() {
                    if binding == *direction_binding {
                        direction_seen[index] = true;
                    }
                }
                if binding == keymap.jump {
                    jump.clear();
                    jump_seen = true;
                }
                continue;
            }
            SyntheticInputEdge::Binding {
                edge,
                record_direction,
                detect_dodge,
                event_seconds,
            } => (edge, record_direction, detect_dodge, event_seconds),
        };
        let edge_seconds = event_seconds.unwrap_or(now_seconds);
        if !combat.raw_bindings.contains(&edge.binding) {
            combat.raw_bindings.push(edge.binding);
        }

        let mut matched_direction = false;
        for (index, (binding, direction, dodge_direction)) in directions.iter().enumerate() {
            if edge.binding != *binding {
                continue;
            }
            matched_direction = true;
            direction_seen[index] = true;
            if edge.pressed {
                if !edge.repeat {
                    if record_direction {
                        combat
                            .edges
                            .push(LocalCombatInputEdge::DirectionPress(*direction));
                    }
                    if detect_dodge {
                        if let Some(gesture) =
                            dodge.detect_tap(index, *dodge_direction, guard_held, edge_seconds)
                        {
                            combat.edges.push(LocalCombatInputEdge::Dodge {
                                direction: *direction,
                                burst: gesture.burst,
                            });
                        }
                    }
                }
            } else if record_direction {
                combat
                    .edges
                    .push(LocalCombatInputEdge::DirectionRelease(*direction));
            }
            break;
        }
        if matched_direction {
            continue;
        }
        if edge.binding == keymap.attack {
            if edge.pressed && !edge.repeat {
                combat.edges.push(LocalCombatInputEdge::Attack);
            }
            continue;
        }
        if edge.binding == keymap.guard {
            if edge.repeat {
                continue;
            }
            guard_held = edge.pressed;
            combat.edges.push(if edge.pressed {
                LocalCombatInputEdge::GuardPress
            } else {
                LocalCombatInputEdge::GuardRelease
            });
            continue;
        }
        if edge.binding == keymap.crouch {
            if edge.repeat {
                continue;
            }
            combat.edges.push(if edge.pressed {
                LocalCombatInputEdge::CrouchPress
            } else {
                LocalCombatInputEdge::CrouchRelease
            });
            continue;
        }
        if edge.binding == keymap.lock_toggle {
            if edge.pressed && !edge.repeat {
                combat.edges.push(LocalCombatInputEdge::LockToggle);
            }
            continue;
        }
        if edge.binding == keymap.jump {
            jump_seen = true;
            if edge.repeat {
                continue;
            }
            if edge.pressed {
                jump.press(edge_seconds);
            } else if jump.release(edge_seconds) {
                combat.edges.push(LocalCombatInputEdge::Jump);
            }
            continue;
        }
        if edge.binding == keymap.dodge {
            if edge.pressed && !edge.repeat {
                combat
                    .edges
                    .push(LocalCombatInputEdge::DirectDodge { burst: guard_held });
            }
            continue;
        }
        if !edge.pressed || edge.repeat {
            continue;
        }
        let action = if edge.binding == keymap.weapon_switch {
            Some(LocalCombatInputEdge::WeaponSwitch)
        } else if edge.binding == keymap.qi_burst {
            Some(LocalCombatInputEdge::QiBurst)
        } else if edge.binding == keymap.fake {
            Some(LocalCombatInputEdge::Fake)
        } else if edge.binding == keymap.cure {
            Some(LocalCombatInputEdge::Cure)
        } else if edge.binding == keymap.drop_weapon {
            Some(LocalCombatInputEdge::DropWeapon)
        } else if edge.binding == keymap.drop_item {
            Some(LocalCombatInputEdge::DropItem)
        } else {
            None
        };
        if let Some(action) = action {
            combat.edges.push(action);
        }
    }
    (direction_seen, jump_seen)
}

fn append_fallback_direction_edges(
    combat: &mut LocalCombatInput,
    dodge: &mut DesktopDodgeInput,
    keymap: &Keymap,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    now_seconds: f64,
    raw_direction_seen: [bool; 4],
) {
    let guard_held = keymap.guard.pressed(keys, mouse);
    for (index, (binding, direction, dodge_direction)) in [
        (keymap.move_up, Dir::Up, Vec2::Y),
        (keymap.move_down, Dir::Down, Vec2::NEG_Y),
        (keymap.move_left, Dir::Left, Vec2::NEG_X),
        (keymap.move_right, Dir::Right, Vec2::X),
    ]
    .into_iter()
    .enumerate()
    {
        if raw_direction_seen[index] {
            continue;
        }
        if binding.just_pressed(keys, mouse) {
            combat
                .edges
                .push(LocalCombatInputEdge::DirectionPress(direction));
            if let Some(gesture) = dodge.detect_tap(index, dodge_direction, guard_held, now_seconds)
            {
                combat.edges.push(LocalCombatInputEdge::Dodge {
                    direction,
                    burst: gesture.burst,
                });
            }
        }
        if binding.just_released(keys, mouse) {
            combat
                .edges
                .push(LocalCombatInputEdge::DirectionRelease(direction));
        }
    }
}

#[cfg(test)]
fn update_command_input(
    command_input: &mut CommandInput,
    keymap: &Keymap,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    now_ms: f64,
) {
    update_command_input_skipping(command_input, keymap, keys, mouse, now_ms, [false; 4]);
}

fn update_command_input_skipping(
    command_input: &mut CommandInput,
    keymap: &Keymap,
    keys: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
    now_ms: f64,
    ordered_edges_seen: [bool; 4],
) {
    for (index, (binding, dir)) in [
        (keymap.move_up, Dir::Up),
        (keymap.move_down, Dir::Down),
        (keymap.move_left, Dir::Left),
        (keymap.move_right, Dir::Right),
    ]
    .into_iter()
    .enumerate()
    {
        if !ordered_edges_seen[index] && binding.just_pressed(keys, mouse) {
            command_input.push_direction(dir, now_ms);
        }
        if !ordered_edges_seen[index] && binding.just_released(keys, mouse) {
            command_input.release_direction(dir);
        }
    }

    command_input.update(now_ms);
}

#[derive(SystemParam)]
pub(crate) struct DesktopDirectionInput<'w, 's> {
    window_events: MessageReader<'w, 's, bevy::window::WindowEvent>,
    synthetic: ResMut<'w, SyntheticInputStream>,
    keys: Res<'w, ButtonInput<KeyCode>>,
    mouse: Res<'w, ButtonInput<MouseButton>>,
    keymap: Res<'w, Keymap>,
}

#[derive(SystemParam)]
pub(crate) struct LocalInputFrameState<'w> {
    command_input: ResMut<'w, LocalCommandInput>,
    crouch: Res<'w, LocalCrouchInput>,
    guard: Res<'w, LocalGuardInput>,
    combat: ResMut<'w, LocalCombatInput>,
    dodge: ResMut<'w, DesktopDodgeInput>,
    jump: ResMut<'w, LocalJumpInput>,
}

pub(crate) fn capture_local_command_input(
    time: Res<Time<bevy::time::Real>>,
    dbg: Option<Res<crate::plugins::debug_plugin::DebugConsoleState>>,
    mut input: DesktopDirectionInput<'_, '_>,
    performance: Option<Res<crate::plugins::stage_plugin::PlayerPerformance>>,
    mut state: LocalInputFrameState<'_>,
    net: Option<Res<super::network_plugin::NetState>>,
    app_state: Option<Res<State<AppState>>>,
) {
    if app_state.is_some_and(|state| *state.get() != AppState::Playing) {
        input.window_events.clear();
        input.synthetic.clear();
        return;
    }
    if net.is_some_and(|net| net.battle_result.is_some()) {
        input.window_events.read().for_each(drop);
        input.synthetic.clear();
        state.command_input.0.clear();
        state.combat.clear();
        state.dodge.clear();
        state.jump.clear();
        return;
    }

    if dbg.is_some_and(|dbg| dbg.captures_input()) {
        input.window_events.read().for_each(drop);
        return;
    }

    let mut ordered_edges: Vec<SyntheticInputEdge> = input
        .window_events
        .read_with_id()
        .filter(|(_, id)| id.id >= state.combat.window_event_floor)
        .filter_map(|(event, _)| match event {
            bevy::window::WindowEvent::KeyboardInput(event) => Some(
                DirectionInputEdge {
                    binding: crate::app_state::InputBinding::Key(event.key_code),
                    pressed: event.state == bevy::input::ButtonState::Pressed,
                    repeat: event.repeat,
                }
                .into(),
            ),
            bevy::window::WindowEvent::MouseButtonInput(event) => Some(
                DirectionInputEdge {
                    binding: crate::app_state::InputBinding::Mouse(event.button),
                    pressed: event.state == bevy::input::ButtonState::Pressed,
                    repeat: false,
                }
                .into(),
            ),
            _ => None,
        })
        .collect();
    ordered_edges.extend(input.synthetic.drain());
    let now_seconds = input
        .synthetic
        .take_frame_clock_seconds()
        .unwrap_or_else(|| time.elapsed_secs_f64());
    state.combat.begin_frame();
    state.jump.begin_frame(now_seconds);
    let (raw_direction_seen, jump_edge_seen) = collect_ordered_combat_edges(
        &mut state.combat,
        &mut state.dodge,
        &mut state.jump,
        &input.keymap,
        ordered_edges.iter().copied(),
        now_seconds,
        OrderedInputInitial {
            guard_held: state.guard.held,
            crouch_held: state.crouch.held,
        },
    );
    if performance.is_some_and(|performance| performance.input_blocked) {
        state.command_input.0.clear();
        state.dodge.clear();
        state.jump.clear();

        state.combat.edges.retain(|edge| {
            matches!(
                edge,
                LocalCombatInputEdge::LockToggle
                    | LocalCombatInputEdge::GuardRelease
                    | LocalCombatInputEdge::CrouchRelease
            )
        });
        return;
    }
    if !jump_edge_seen {
        if input.keymap.jump.just_pressed(&input.keys, &input.mouse) {
            state.jump.press(now_seconds);
        }
        if input.keymap.jump.just_released(&input.keys, &input.mouse) {
            state.jump.release(now_seconds);
        } else if state.jump.held && !input.keymap.jump.pressed(&input.keys, &input.mouse) {
            state.jump.clear();
        }
    }
    if !state.combat.raw_bindings.contains(&input.keymap.dodge)
        && input.keymap.dodge.just_pressed(&input.keys, &input.mouse)
    {
        state.combat.edges.push(LocalCombatInputEdge::DirectDodge {
            burst: input.keymap.guard.pressed(&input.keys, &input.mouse),
        });
    }
    if state.combat.raw_bindings.is_empty() {
        update_desktop_dodge_input_skipping(
            &mut state.dodge,
            &input.keymap,
            &input.keys,
            &input.mouse,
            now_seconds,
            raw_direction_seen,
        );
        update_command_input_skipping(
            &mut state.command_input.0,
            &input.keymap,
            &input.keys,
            &input.mouse,
            now_seconds * 1_000.0,
            raw_direction_seen,
        );
    } else {
        append_fallback_direction_edges(
            &mut state.combat,
            &mut state.dodge,
            &input.keymap,
            &input.keys,
            &input.mouse,
            now_seconds,
            raw_direction_seen,
        );
        state.combat.command_update_ms = Some(now_seconds * 1_000.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_state::InputBinding;

    #[test]
    fn captures_rebound_keyboard_direction_edges() {
        let keymap = Keymap {
            move_up: InputBinding::Key(KeyCode::KeyI),
            ..default()
        };
        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        let mut command_input = CommandInput::new();

        keys.press(KeyCode::KeyI);
        update_command_input(&mut command_input, &keymap, &keys, &mouse, 125.0);

        assert_eq!(command_input.command_string(), "u");
        assert_eq!(command_input.held_direction(), Some(Dir::Up));
        assert_eq!(command_input.last_direction_at, Some(125.0));

        keys.clear();
        keys.release(KeyCode::KeyI);
        update_command_input(&mut command_input, &keymap, &keys, &mouse, 150.0);

        assert_eq!(command_input.command_string(), "u");
        assert_eq!(command_input.held_direction(), None);
    }

    #[test]
    fn ordered_hardware_edges_preserve_same_frame_command_order_and_double_tap() {
        let keymap = Keymap::default();
        let mut command = CommandInput::new();
        let mut dodge = DesktopDodgeInput::default();

        let seen = apply_ordered_direction_edges(
            &mut command,
            &mut dodge,
            &keymap,
            [
                DirectionInputEdge {
                    binding: keymap.move_right,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.move_up,
                    pressed: true,
                    repeat: false,
                },
            ],
            1.0,
            false,
        );

        assert_eq!(command.command_string(), "ru");
        assert_eq!(seen, [true, false, false, true]);

        command.clear();
        dodge.clear();
        apply_ordered_direction_edges(
            &mut command,
            &mut dodge,
            &keymap,
            [
                DirectionInputEdge {
                    binding: keymap.move_up,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.move_up,
                    pressed: false,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.move_up,
                    pressed: true,
                    repeat: false,
                },
            ],
            2.0,
            false,
        );

        assert_eq!(command.command_string(), "uu");
        assert_eq!(
            dodge.pending,
            Some(DodgeGesture {
                direction: Vec2::Y,
                burst: false,
            })
        );
    }

    #[test]
    fn ordered_same_frame_double_tap_keeps_guard_burst_kind() {
        let keymap = Keymap::default();
        let mut command = CommandInput::new();
        let mut dodge = DesktopDodgeInput::default();

        apply_ordered_direction_edges(
            &mut command,
            &mut dodge,
            &keymap,
            [
                DirectionInputEdge {
                    binding: keymap.move_left,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.move_left,
                    pressed: false,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.move_left,
                    pressed: true,
                    repeat: false,
                },
            ],
            3.0,
            true,
        );

        assert_eq!(command.command_string(), "ll");
        assert_eq!(
            dodge.pending,
            Some(DodgeGesture {
                direction: Vec2::NEG_X,
                burst: true,
            })
        );
    }

    #[test]
    fn ordered_guard_edges_choose_dodge_or_burst_at_direction_event_time() {
        fn second_tap_with_edges(
            keymap: &Keymap,
            initial_guard_held: bool,
            edges: impl IntoIterator<Item = DirectionInputEdge>,
        ) -> DodgeGesture {
            let mut command = CommandInput::new();
            let mut dodge = DesktopDodgeInput::default();
            apply_ordered_direction_edges(
                &mut command,
                &mut dodge,
                keymap,
                [
                    DirectionInputEdge {
                        binding: keymap.move_up,
                        pressed: true,
                        repeat: false,
                    },
                    DirectionInputEdge {
                        binding: keymap.move_up,
                        pressed: false,
                        repeat: false,
                    },
                ],
                1.0,
                initial_guard_held,
            );
            apply_ordered_direction_edges(
                &mut command,
                &mut dodge,
                keymap,
                edges,
                1.1,
                initial_guard_held,
            );
            dodge
                .pending
                .expect("the second tap should create a gesture")
        }

        let keymap = Keymap::default();
        let direction = DirectionInputEdge {
            binding: keymap.move_up,
            pressed: true,
            repeat: false,
        };
        let guard_press = DirectionInputEdge {
            binding: keymap.guard,
            pressed: true,
            repeat: false,
        };
        let guard_release = DirectionInputEdge {
            binding: keymap.guard,
            pressed: false,
            repeat: false,
        };

        assert!(!second_tap_with_edges(&keymap, false, [direction, guard_press]).burst);
        assert!(second_tap_with_edges(&keymap, false, [guard_press, direction]).burst);
        assert!(second_tap_with_edges(&keymap, true, [direction, guard_release]).burst);
        assert!(!second_tap_with_edges(&keymap, true, [guard_release, direction]).burst);
    }

    #[test]
    fn ordered_guard_and_direction_edges_preserve_cross_device_order() {
        let keymap = Keymap {
            move_up: InputBinding::Mouse(MouseButton::Middle),
            guard: InputBinding::Key(KeyCode::KeyG),
            ..default()
        };
        let mut command = CommandInput::new();
        let mut dodge = DesktopDodgeInput::default();
        let first_tap = [
            DirectionInputEdge {
                binding: keymap.move_up,
                pressed: true,
                repeat: false,
            },
            DirectionInputEdge {
                binding: keymap.move_up,
                pressed: false,
                repeat: false,
            },
        ];
        apply_ordered_direction_edges(&mut command, &mut dodge, &keymap, first_tap, 2.0, false);
        apply_ordered_direction_edges(
            &mut command,
            &mut dodge,
            &keymap,
            [
                DirectionInputEdge {
                    binding: keymap.guard,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.move_up,
                    pressed: true,
                    repeat: false,
                },
            ],
            2.1,
            false,
        );

        assert_eq!(command.command_string(), "uu");
        assert_eq!(
            dodge.pending,
            Some(DodgeGesture {
                direction: Vec2::Y,
                burst: true,
            })
        );
    }

    #[test]
    fn playing_entry_discards_menu_click_but_accepts_fresh_attack() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::input::{mouse::MouseButtonInput, ButtonState};
        use bevy::window::WindowEvent;
        let mut app = App::new();
        app.init_resource::<Time<bevy::time::Real>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(Keymap {
                attack: InputBinding::Mouse(MouseButton::Left),
                ..default()
            })
            .init_resource::<LocalCommandInput>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .init_resource::<LocalCombatInput>()
            .init_resource::<LocalJumpInput>()
            .init_resource::<DesktopDodgeInput>()
            .init_resource::<SyntheticInputStream>()
            .add_message::<WindowEvent>()
            .add_systems(Update, capture_local_command_input);
        let click = || {
            WindowEvent::MouseButtonInput(MouseButtonInput {
                button: MouseButton::Left,
                state: ButtonState::Pressed,
                window: Entity::PLACEHOLDER,
            })
        };
        app.world_mut().write_message(click());
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        app.world_mut()
            .run_system_once(reset_local_input_state)
            .unwrap();
        app.world_mut()
            .run_system_once(reset_gameplay_window_events)
            .unwrap();

        assert_eq!(app.world().resource::<Messages<WindowEvent>>().len(), 1);
        app.update();
        assert!(app.world().resource::<LocalCombatInput>().edges.is_empty());
        assert!(!app
            .world()
            .resource::<ButtonInput<MouseButton>>()
            .just_pressed(MouseButton::Left));
        app.world_mut().write_message(click());
        app.update();
        assert_eq!(
            app.world().resource::<LocalCombatInput>().edges,
            vec![LocalCombatInputEdge::Attack]
        );
    }

    #[test]
    fn production_window_stream_preserves_keyboard_mouse_guard_order() {
        use bevy::input::keyboard::{Key, KeyboardInput};
        use bevy::input::mouse::MouseButtonInput;
        use bevy::input::ButtonState;
        use bevy::window::WindowEvent;

        let keymap = Keymap {
            move_up: InputBinding::Mouse(MouseButton::Middle),
            guard: InputBinding::Key(KeyCode::KeyG),
            ..default()
        };
        let mut app = App::new();
        app.init_resource::<Time<bevy::time::Real>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(keymap)
            .init_resource::<LocalCommandInput>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .init_resource::<LocalCombatInput>()
            .init_resource::<LocalJumpInput>()
            .init_resource::<DesktopDodgeInput>()
            .init_resource::<SyntheticInputStream>()
            .add_message::<WindowEvent>()
            .add_systems(Update, capture_local_command_input);

        let mouse_edge = |state| {
            WindowEvent::MouseButtonInput(MouseButtonInput {
                button: MouseButton::Middle,
                state,
                window: Entity::PLACEHOLDER,
            })
        };
        let guard_edge = |state| {
            WindowEvent::KeyboardInput(KeyboardInput {
                key_code: KeyCode::KeyG,
                logical_key: Key::Character("g".into()),
                state,
                text: None,
                repeat: false,
                window: Entity::PLACEHOLDER,
            })
        };

        app.world_mut()
            .write_message(mouse_edge(ButtonState::Pressed));
        app.world_mut()
            .write_message(mouse_edge(ButtonState::Released));
        app.update();

        app.world_mut()
            .write_message(guard_edge(ButtonState::Pressed));
        app.world_mut()
            .write_message(mouse_edge(ButtonState::Pressed));
        app.update();

        assert_eq!(
            app.world().resource::<LocalCombatInput>().edges,
            vec![
                LocalCombatInputEdge::GuardPress,
                LocalCombatInputEdge::DirectionPress(Dir::Up),
                LocalCombatInputEdge::Dodge {
                    direction: Dir::Up,
                    burst: true,
                },
            ]
        );
    }

    #[test]
    fn ordered_combat_edges_preserve_guard_attack_direction() {
        let keymap = Keymap::default();
        let guard_press = DirectionInputEdge {
            binding: keymap.guard,
            pressed: true,
            repeat: false,
        };
        let attack_press = DirectionInputEdge {
            binding: keymap.attack,
            pressed: true,
            repeat: false,
        };

        let mut guard_first = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();
        collect_ordered_combat_edges(
            &mut guard_first,
            &mut dodge,
            &mut jump,
            &keymap,
            [guard_press, attack_press],
            1.0,
            OrderedInputInitial::default(),
        );
        assert_eq!(
            guard_first.edges,
            vec![
                LocalCombatInputEdge::GuardPress,
                LocalCombatInputEdge::Attack
            ]
        );
        assert!(guard_first.saw_raw_binding(keymap.guard));
        assert!(guard_first.saw_raw_binding(keymap.attack));

        let mut attack_first = LocalCombatInput::default();
        collect_ordered_combat_edges(
            &mut attack_first,
            &mut dodge,
            &mut jump,
            &keymap,
            [attack_press, guard_press],
            1.0,
            OrderedInputInitial::default(),
        );
        assert_eq!(
            attack_first.edges,
            vec![
                LocalCombatInputEdge::Attack,
                LocalCombatInputEdge::GuardPress
            ]
        );
    }

    #[test]
    fn direction_binding_keeps_web_precedence_over_duplicate_attack_binding() {
        let keymap = Keymap {
            attack: InputBinding::Key(KeyCode::KeyW),
            ..default()
        };
        let mut combat = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();
        collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            [DirectionInputEdge {
                binding: keymap.move_up,
                pressed: true,
                repeat: false,
            }],
            1.0,
            OrderedInputInitial::default(),
        );

        assert_eq!(
            combat.edges,
            vec![LocalCombatInputEdge::DirectionPress(Dir::Up)]
        );
        assert!(combat.saw_raw_binding(keymap.move_up));
    }

    #[test]
    fn ordered_action_edges_preserve_attack_lock_switch_and_jump_order() {
        let keymap = Keymap::default();
        let mut combat = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();
        jump.press(1.0);

        collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            [
                DirectionInputEdge {
                    binding: keymap.attack,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.lock_toggle,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.weapon_switch,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.jump,
                    pressed: false,
                    repeat: false,
                },
            ],
            1.08,
            OrderedInputInitial::default(),
        );

        assert_eq!(
            combat.edges,
            vec![
                LocalCombatInputEdge::Attack,
                LocalCombatInputEdge::LockToggle,
                LocalCombatInputEdge::WeaponSwitch,
                LocalCombatInputEdge::Jump,
            ]
        );
        assert!(combat.saw_raw_binding(keymap.attack));
        assert!(combat.saw_raw_binding(keymap.lock_toggle));
        assert!(combat.saw_raw_binding(keymap.weapon_switch));
        assert!(combat.saw_raw_binding(keymap.jump));
        assert!(jump.released);
        assert!((jump.hold_seconds - 0.08).abs() < 1e-5);
    }

    #[test]
    fn direction_press_and_generated_dodge_stay_at_their_device_event_position() {
        let keymap = Keymap::default();
        let attack = DirectionInputEdge {
            binding: keymap.attack,
            pressed: true,
            repeat: false,
        };
        let forward = DirectionInputEdge {
            binding: keymap.move_up,
            pressed: true,
            repeat: false,
        };

        for attack_first in [true, false] {
            let mut combat = LocalCombatInput::default();
            let mut jump = LocalJumpInput::default();
            let mut dodge = DesktopDodgeInput::default();
            assert!(dodge.detect_tap(0, Vec2::Y, false, 0.8).is_none());
            let edges = if attack_first {
                [attack, forward]
            } else {
                [forward, attack]
            };

            let (seen, _) = collect_ordered_combat_edges(
                &mut combat,
                &mut dodge,
                &mut jump,
                &keymap,
                edges,
                1.0,
                OrderedInputInitial::default(),
            );

            let direction_and_dodge = [
                LocalCombatInputEdge::DirectionPress(Dir::Up),
                LocalCombatInputEdge::Dodge {
                    direction: Dir::Up,
                    burst: false,
                },
            ];
            if attack_first {
                assert_eq!(combat.edges[0], LocalCombatInputEdge::Attack);
                assert_eq!(combat.edges[1..], direction_and_dodge);
            } else {
                assert_eq!(combat.edges[..2], direction_and_dodge);
                assert_eq!(combat.edges[2], LocalCombatInputEdge::Attack);
            }
            assert!(seen[0]);
            assert!(dodge.pending.is_none());
        }
    }

    #[test]
    fn crouch_edges_surround_the_generated_dodge_for_event_time_roll_selection() {
        let keymap = Keymap::default();
        let mut combat = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();
        assert!(dodge.detect_tap(0, Vec2::Y, false, 0.8).is_none());

        collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            [
                DirectionInputEdge {
                    binding: keymap.crouch,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.move_up,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.crouch,
                    pressed: false,
                    repeat: false,
                },
            ],
            1.0,
            OrderedInputInitial::default(),
        );

        assert_eq!(
            combat.edges,
            vec![
                LocalCombatInputEdge::CrouchPress,
                LocalCombatInputEdge::DirectionPress(Dir::Up),
                LocalCombatInputEdge::Dodge {
                    direction: Dir::Up,
                    burst: false,
                },
                LocalCombatInputEdge::CrouchRelease,
            ]
        );
    }

    #[test]
    fn synthetic_mobile_and_replay_sources_keep_distinct_direction_semantics() {
        let keymap = Keymap::default();

        let mut mobile = SyntheticInputStream::default();
        mobile.push_mobile_direction(keymap.move_up, true);
        mobile.push_dodge(Dir::Up, false);
        mobile.push_action(keymap.attack, SyntheticAction::Attack, true, None);
        let mut combat = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();
        collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            mobile.drain(),
            1.0,
            OrderedInputInitial::default(),
        );
        assert_eq!(
            combat.edges,
            vec![
                LocalCombatInputEdge::DirectionPress(Dir::Up),
                LocalCombatInputEdge::Dodge {
                    direction: Dir::Up,
                    burst: false,
                },
                LocalCombatInputEdge::Attack,
            ]
        );

        let mut replay = SyntheticInputStream::default();
        replay.push_replay_binding(keymap.move_up, true, None);
        replay.push_replay_binding(keymap.attack, true, None);
        combat.begin_frame();
        collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            replay.drain(),
            2.0,
            OrderedInputInitial::default(),
        );
        assert_eq!(combat.edges, vec![LocalCombatInputEdge::Attack]);
        assert!(combat.saw_raw_binding(keymap.move_up));
    }

    #[test]
    fn synthetic_drop_actions_survive_until_the_ordered_consumer_frame() {
        let keymap = Keymap::default();
        let mut synthetic = SyntheticInputStream::default();
        synthetic.push_action(keymap.drop_weapon, SyntheticAction::DropWeapon, true, None);
        synthetic.push_action(keymap.drop_item, SyntheticAction::DropItem, true, None);
        let mut combat = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();

        collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            synthetic.drain(),
            1.0,
            OrderedInputInitial::default(),
        );

        assert!(combat.edges.contains(&LocalCombatInputEdge::DropWeapon));
        assert!(combat.edges.contains(&LocalCombatInputEdge::DropItem));
    }

    #[test]
    fn discrete_action_prefers_ordered_edge_and_keeps_button_input_fallback() {
        let keymap = Keymap::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keys.press(KeyCode::KeyG);

        assert!(local_discrete_action_pressed(
            None,
            LocalCombatInputEdge::DropWeapon,
            keymap.drop_weapon,
            &keys,
            &mouse,
        ));

        let raw_without_semantic_edge = LocalCombatInput {
            raw_bindings: vec![keymap.drop_weapon],
            ..Default::default()
        };
        assert!(!local_discrete_action_pressed(
            Some(&raw_without_semantic_edge),
            LocalCombatInputEdge::DropWeapon,
            keymap.drop_weapon,
            &keys,
            &mouse,
        ));

        let ordered = LocalCombatInput {
            edges: vec![LocalCombatInputEdge::DropWeapon],
            raw_bindings: vec![keymap.drop_weapon],
            ..Default::default()
        };
        assert!(local_discrete_action_pressed(
            Some(&ordered),
            LocalCombatInputEdge::DropWeapon,
            keymap.drop_weapon,
            &keys,
            &mouse,
        ));
    }

    #[test]
    fn ordered_discrete_actions_preserve_h_before_g_and_append_unseen_fallbacks() {
        let keymap = Keymap::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keys.press(KeyCode::KeyF);
        let input = LocalCombatInput {
            edges: vec![
                LocalCombatInputEdge::DropItem,
                LocalCombatInputEdge::DropWeapon,
            ],
            raw_bindings: vec![keymap.drop_item, keymap.drop_weapon],
            ..Default::default()
        };
        assert_eq!(
            ordered_local_discrete_actions(
                Some(&input),
                &[
                    (LocalCombatInputEdge::QiBurst, keymap.qi_burst),
                    (LocalCombatInputEdge::DropWeapon, keymap.drop_weapon),
                    (LocalCombatInputEdge::DropItem, keymap.drop_item),
                ],
                &keys,
                &mouse,
            ),
            vec![
                LocalCombatInputEdge::DropItem,
                LocalCombatInputEdge::DropWeapon,
                LocalCombatInputEdge::QiBurst,
            ]
        );
    }

    #[test]
    fn semantic_mobile_action_is_not_reinterpreted_by_a_duplicate_binding() {
        let keymap = Keymap {
            attack: InputBinding::Key(KeyCode::KeyW),
            ..default()
        };
        let mut synthetic = SyntheticInputStream::default();
        synthetic.push_action(keymap.attack, SyntheticAction::Attack, true, None);
        let mut combat = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();

        let (seen, _) = collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            synthetic.drain(),
            1.0,
            OrderedInputInitial::default(),
        );

        assert_eq!(combat.edges, vec![LocalCombatInputEdge::Attack]);
        assert!(seen[0], "semantic action suppresses direction fallback");
    }

    #[test]
    fn synthetic_jump_cancel_clears_charge_without_emitting_jump() {
        let keymap = Keymap::default();
        let mut synthetic = SyntheticInputStream::default();
        synthetic.push_live_binding(keymap.jump, true);
        synthetic.cancel_binding(keymap.jump);
        let mut combat = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();

        collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            synthetic.drain(),
            1.0,
            OrderedInputInitial::default(),
        );

        assert!(combat.edges.is_empty());
        assert!(!jump.held);
        assert!(!jump.released);
    }

    #[test]
    fn replay_jump_uses_recorded_edge_times_when_multiple_edges_drain_together() {
        let keymap = Keymap::default();
        let mut synthetic = SyntheticInputStream::default();
        synthetic.push_replay_binding(keymap.jump, true, Some(3.0));
        synthetic.push_replay_binding(keymap.jump, false, Some(3.18));
        let mut combat = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();

        collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            synthetic.drain(),
            10.0,
            OrderedInputInitial::default(),
        );

        assert_eq!(combat.edges, vec![LocalCombatInputEdge::Jump]);
        assert!(jump.released);
        assert!((jump.hold_seconds - 0.18).abs() < 1e-5);
    }

    #[test]
    fn direct_dodge_binding_enters_the_ordered_action_stream() {
        let keymap = Keymap::default();
        let mut combat = LocalCombatInput::default();
        let mut jump = LocalJumpInput::default();
        let mut dodge = DesktopDodgeInput::default();

        collect_ordered_combat_edges(
            &mut combat,
            &mut dodge,
            &mut jump,
            &keymap,
            [DirectionInputEdge {
                binding: keymap.dodge,
                pressed: true,
                repeat: false,
            }],
            1.0,
            OrderedInputInitial {
                guard_held: true,
                crouch_held: false,
            },
        );

        assert_eq!(
            combat.edges,
            vec![LocalCombatInputEdge::DirectDodge { burst: true }]
        );
    }

    #[test]
    fn jump_latch_uses_event_wall_time_and_same_frame_tap_stays_normal() {
        let keymap = Keymap::default();
        let mut jump = LocalJumpInput::default();
        jump.begin_frame(10.0);

        assert!(apply_ordered_jump_edges(
            &mut jump,
            keymap.jump,
            [
                DirectionInputEdge {
                    binding: keymap.jump,
                    pressed: true,
                    repeat: false,
                },
                DirectionInputEdge {
                    binding: keymap.jump,
                    pressed: false,
                    repeat: false,
                },
            ],
            10.0,
        ));
        assert!(jump.pressed);
        assert!(jump.released);
        assert!(!jump.held);
        assert_eq!(jump.hold_seconds, 0.0);

        jump.begin_frame(20.0);
        apply_ordered_jump_edges(
            &mut jump,
            keymap.jump,
            [DirectionInputEdge {
                binding: keymap.jump,
                pressed: true,
                repeat: false,
            }],
            20.0,
        );
        jump.begin_frame(20.12);
        apply_ordered_jump_edges(
            &mut jump,
            keymap.jump,
            [DirectionInputEdge {
                binding: keymap.jump,
                pressed: true,
                repeat: true,
            }],
            20.12,
        );
        assert!((jump.hold_seconds - 0.12).abs() < 1e-5);
        apply_ordered_jump_edges(
            &mut jump,
            keymap.jump,
            [DirectionInputEdge {
                binding: keymap.jump,
                pressed: false,
                repeat: false,
            }],
            20.13,
        );
        assert!((jump.hold_seconds - 0.13).abs() < 1e-5);
    }

    #[test]
    fn crouch_latch_obeys_web_controller_and_firearm_gates() {
        let mut crouch = LocalCrouchInput::default();
        update_crouch_latch(
            &mut crouch,
            CrouchInputFrame {
                pressed: true,
                physically_held: true,
                ..default()
            },
        );
        assert!(crouch.held);

        update_crouch_latch(
            &mut crouch,
            CrouchInputFrame {
                released: true,
                stunned: true,
                ..default()
            },
        );
        assert!(crouch.held);

        update_crouch_latch(
            &mut crouch,
            CrouchInputFrame {
                released: true,
                input_blocked: true,
                ..default()
            },
        );
        assert!(!crouch.held);

        update_crouch_latch(
            &mut crouch,
            CrouchInputFrame {
                pressed: true,
                physically_held: true,
                gun_aiming: true,
                ..default()
            },
        );
        assert!(!crouch.held);
        update_crouch_latch(
            &mut crouch,
            CrouchInputFrame {
                physically_held: true,
                ..default()
            },
        );
        assert!(!crouch.held);

        update_crouch_latch(
            &mut crouch,
            CrouchInputFrame {
                pressed: true,
                physically_held: true,
                switching_weapon: true,
                ..default()
            },
        );
        assert!(crouch.held);
        update_crouch_latch(
            &mut crouch,
            CrouchInputFrame {
                physically_held: true,
                dead: true,
                ..default()
            },
        );
        assert!(!crouch.held);
    }

    #[test]
    fn guard_latch_survives_control_stalls_but_release_still_clears() {
        let mut guard = LocalGuardInput::default();
        update_guard_latch(
            &mut guard,
            GuardInputFrame {
                pressed: true,
                physically_held: true,
                ..default()
            },
        );
        assert!(guard.held);

        update_guard_latch(
            &mut guard,
            GuardInputFrame {
                physically_held: true,
                input_blocked: true,
                ..default()
            },
        );
        assert!(guard.held, "a no-edge control stall is not a key release");

        update_guard_latch(
            &mut guard,
            GuardInputFrame {
                released: true,
                input_blocked: true,
                ..default()
            },
        );
        assert!(
            !guard.held,
            "GuardRelease is forwarded while control is blocked"
        );

        update_guard_latch(
            &mut guard,
            GuardInputFrame {
                pressed: true,
                physically_held: true,
                input_blocked: true,
                ..default()
            },
        );
        assert!(!guard.held);
        update_guard_latch(
            &mut guard,
            GuardInputFrame {
                physically_held: true,
                ..default()
            },
        );
        assert!(
            !guard.held,
            "an ignored press cannot arm itself after control returns"
        );
    }

    #[test]
    fn player_perform_crouch_can_be_released_after_control_returns() {
        use super::super::stage_plugin::{
            drive_player_performance, NpcPerformCommand, PlayerPerformance,
        };
        use meteor_scene::pst_runtime::PstValue;
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Keymap>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .init_resource::<PlayerPerformance>()
            .add_systems(
                Update,
                (drive_player_performance, capture_local_hold_inputs).chain(),
            );
        app.world_mut().spawn((
            LocalFighter,
            SmState::default(),
            GunAimState::default(),
            FighterHitReaction::default(),
            DeathRespawn::default(),
        ));

        for (action, value) in [("block", 0.0), ("crouch", 1.0), ("block", 1.0)] {
            app.world_mut()
                .resource_mut::<PlayerPerformance>()
                .queue
                .push_back(NpcPerformCommand {
                    action: action.into(),
                    args: vec![PstValue::Num(value)],
                });
        }
        for _ in 0..12 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_millis(100));
            app.update();
        }
        assert!(!app.world().resource::<PlayerPerformance>().input_blocked);
        assert!(app.world().resource::<PlayerPerformance>().queue.is_empty());
        assert!(app.world().resource::<LocalCrouchInput>().held);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyC);
        app.update();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyC);
        app.update();
        assert!(!app.world().resource::<LocalCrouchInput>().held);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        for _ in 0..8 {
            app.update();
            assert!(
                !app.world().resource::<LocalCrouchInput>().held,
                "the consumed script command must not reapply crouch"
            );
        }
    }

    #[test]
    fn production_crouch_press_blocked_by_aim_does_not_arm_after_q_release() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Keymap>()
            .init_resource::<LocalCrouchInput>()
            .init_resource::<LocalGuardInput>()
            .add_systems(Update, capture_local_hold_inputs);
        let local = app
            .world_mut()
            .spawn((
                LocalFighter,
                SmState::default(),
                GunAimState::default(),
                FighterHitReaction::default(),
                DeathRespawn::default(),
            ))
            .id();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyC);
        app.update();
        assert!(app.world().resource::<LocalCrouchInput>().held);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyC);
        app.update();
        assert!(!app.world().resource::<LocalCrouchInput>().held);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .get_mut::<GunAimState>(local)
            .unwrap()
            .aiming = true;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyC);
        app.update();
        assert!(!app.world().resource::<LocalCrouchInput>().held);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .get_mut::<GunAimState>(local)
            .unwrap()
            .aiming = false;
        app.update();
        assert!(!app.world().resource::<LocalCrouchInput>().held);
    }

    #[test]
    fn captures_mouse_bound_direction_and_updates_elapsed_time() {
        let keymap = Keymap {
            move_left: InputBinding::Mouse(MouseButton::Middle),
            ..default()
        };
        let keys = ButtonInput::<KeyCode>::default();
        let mut mouse = ButtonInput::<MouseButton>::default();
        let mut command_input = CommandInput::new();

        mouse.press(MouseButton::Middle);
        update_command_input(&mut command_input, &keymap, &keys, &mouse, 10.0);
        assert_eq!(command_input.command_string(), "l");

        let no_mouse_edges = ButtonInput::<MouseButton>::default();
        update_command_input(&mut command_input, &keymap, &keys, &no_mouse_edges, 310.0);

        assert_eq!(command_input.command_string(), "l");
        assert_eq!(command_input.generation, 1);
    }

    #[test]
    fn desktop_direction_double_tap_emits_dodge_and_guard_burst() {
        let keymap = Keymap::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        let mut dodge = DesktopDodgeInput::default();

        keys.press(KeyCode::KeyW);
        update_desktop_dodge_input(&mut dodge, &keymap, &keys, &mouse, 1.0);
        assert_eq!(dodge.pending, None);
        keys.clear();
        keys.release(KeyCode::KeyW);
        keys.clear();
        keys.press(KeyCode::KeyW);
        update_desktop_dodge_input(&mut dodge, &keymap, &keys, &mouse, 1.24);
        assert_eq!(
            dodge.pending,
            Some(DodgeGesture {
                direction: Vec2::Y,
                burst: false,
            })
        );

        let mut guarded = DesktopDodgeInput::default();
        let mut guarded_keys = ButtonInput::<KeyCode>::default();
        guarded_keys.press(KeyCode::KeyA);
        update_desktop_dodge_input(&mut guarded, &keymap, &guarded_keys, &mouse, 2.0);
        guarded_keys.clear();
        guarded_keys.release(KeyCode::KeyA);
        guarded_keys.clear();
        guarded_keys.press(KeyCode::KeyA);
        let mut guarded_mouse = ButtonInput::<MouseButton>::default();
        guarded_mouse.press(MouseButton::Right);
        update_desktop_dodge_input(&mut guarded, &keymap, &guarded_keys, &guarded_mouse, 2.24);
        assert_eq!(
            guarded.pending,
            Some(DodgeGesture {
                direction: Vec2::NEG_X,
                burst: true,
            })
        );
    }

    #[test]
    fn desktop_double_tap_window_is_strictly_less_than_quarter_second() {
        let mut dodge = DesktopDodgeInput::default();
        dodge.register_tap(3, Vec2::X, false, 4.0);
        dodge.register_tap(3, Vec2::X, false, 4.25);
        assert_eq!(dodge.pending, None);
    }

    #[test]
    fn first_same_frame_double_tap_request_wins() {
        let mut dodge = DesktopDodgeInput::default();
        dodge.register_tap(0, Vec2::Y, false, 1.0);
        dodge.register_tap(2, Vec2::NEG_X, false, 1.0);
        dodge.register_tap(0, Vec2::Y, false, 1.2);
        dodge.register_tap(2, Vec2::NEG_X, true, 1.2);
        assert_eq!(
            dodge.pending,
            Some(DodgeGesture {
                direction: Vec2::Y,
                burst: false,
            })
        );
    }
}
