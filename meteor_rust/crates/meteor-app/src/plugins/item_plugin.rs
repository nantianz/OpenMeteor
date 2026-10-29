use crate::app_state::AppState;
use crate::components::fighter::{
    CombatState, DeathRespawn, Fighter, Health, PoseRuntime, Qi, SmState,
};
use bevy::prelude::*;
use meteor_combat::death::RESPAWN_DELAY_SECONDS;
use meteor_core::FighterState;
use meteor_network::protocol::{ItemEffectWire, ItemWeaponSlotWire, Message};
use std::{collections::HashMap, path::PathBuf};

#[derive(Component, Debug, Clone)]
pub struct StageProp {
    pub name: String,

    pub model: String,

    pub center_offset: Vec3,

    pub half_extents: Vec3,

    pub hit_count: u32,

    pub respawn_timer: f32,

    pub interactive: bool,

    pub collision_enabled: bool,

    pub breakable: bool,

    pub broken: bool,
}

impl StageProp {
    pub(crate) fn world_center(&self, transform: &Transform) -> Vec3 {
        transform.translation + transform.rotation * self.center_offset
    }

    pub(crate) fn global_center(&self, transform: &GlobalTransform) -> Vec3 {
        let (_, rotation, translation) = transform.to_scale_rotation_translation();
        translation + rotation * self.center_offset
    }
}

pub fn is_breakable_model(model: &str) -> bool {
    ["box", "stone", "jug", "desk", "chair", "barrel"]
        .iter()
        .any(|key| model.contains(key))
}

#[derive(Resource, Debug, Clone)]
pub struct ItemBuffs {
    pub attack_bonus: i32,

    pub defense_bonus: i32,

    pub remaining: f32,

    pub max_hp_bonus: i32,
    pub max_hp_remaining: f32,

    pub speed_factor: f32,
    pub speed_remaining: f32,

    pub poison_per_tick: i32,
    pub poison_tick_every: f32,
    pub poison_total: f32,
    pub poison_tick: f32,

    pub anger_per_tick: i32,
    pub anger_tick_every: f32,
    pub anger_total: f32,
    pub anger_tick: f32,
}

impl Default for ItemBuffs {
    fn default() -> Self {
        Self {
            attack_bonus: 0,
            defense_bonus: 0,
            remaining: 0.0,
            max_hp_bonus: 0,
            max_hp_remaining: 0.0,
            speed_factor: 1.0,
            speed_remaining: 0.0,
            poison_per_tick: 0,
            poison_tick_every: 0.0,
            poison_total: 0.0,
            poison_tick: 0.0,
            anger_per_tick: 0,
            anger_tick_every: 0.0,
            anger_total: 0.0,
            anger_tick: 0.0,
        }
    }
}

#[derive(Resource, Default)]
pub struct PropHitGate {
    last_by_attacker: HashMap<Entity, f64>,
}

impl PropHitGate {
    pub fn accepts(&self, attacker: Entity, now: f64) -> bool {
        self.last_by_attacker
            .get(&attacker)
            .is_none_or(|last| now - *last > f64::from(PROP_HIT_INTERVAL))
    }

    pub fn reset(&mut self, attacker: Entity, now: f64) {
        self.last_by_attacker.insert(attacker, now);
    }

    fn clear(&mut self) {
        self.last_by_attacker.clear();
    }
}

#[derive(Resource, Default)]
struct PendingSceneItemClaims {
    expires_at: HashMap<String, f32>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource, Default)]
pub(crate) struct StockSceneManifestState {
    initialized: bool,
}

type BuffTickPlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Fighter,
        &'static Transform,
        &'static crate::components::fighter::MotionState,
        &'static mut Health,
        &'static mut Qi,
        &'static mut crate::components::fighter::WornEffect,
        &'static mut crate::components::fighter::Concealment,
        &'static mut DeathRespawn,
        &'static mut SmState,
        &'static mut CombatState,
        &'static mut PoseRuntime,
        Option<&'static mut crate::components::fighter::FighterStatusEffects>,
    ),
    With<crate::plugins::stage_plugin::LocalFighter>,
>;

type PickupPropsQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut StageProp,
        &'static Transform,
        &'static mut Visibility,
        Option<&'static crate::plugins::render_plugin::DynamicWeaponDrop>,
    ),
    (
        Without<crate::plugins::stage_plugin::LocalFighter>,
        Without<Fighter>,
        Without<super::escort_assets::EscortPropVisual>,
        With<StageProp>,
    ),
>;

type PickupPlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static mut Health,
        &'static mut Qi,
        &'static mut Fighter,
        &'static mut crate::components::fighter::WornEffect,
        &'static mut crate::components::fighter::Concealment,
        &'static mut crate::components::fighter::FighterStatusEffects,
        Option<&'static crate::plugins::stage_plugin::LocalFighter>,
        Option<&'static crate::plugins::stage_plugin::MissionNpc>,
        Option<&'static DeathRespawn>,
        Option<&'static SmState>,
        Option<&'static Visibility>,
    ),
    (
        Or<(
            With<crate::plugins::stage_plugin::LocalFighter>,
            With<crate::plugins::combat_plugin::NpcFighter>,
        )>,
        Without<crate::plugins::network_plugin::RemoteFighter>,
    ),
>;

type NetworkItemFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        Option<&'static crate::plugins::stage_plugin::LocalFighter>,
        Option<&'static crate::plugins::network_plugin::RemoteFighter>,
        &'static mut Health,
        &'static mut Qi,
        &'static mut Fighter,
        &'static mut crate::components::fighter::WornEffect,
        &'static mut crate::components::fighter::Concealment,
    ),
    With<Fighter>,
>;

const PICKUP_RANGE: f32 = 16.0 * 0.12;

const STOCK_PROP_MATCH_MAX_DISTANCE: f32 = PICKUP_RANGE * 2.0;
const PICKUP_CAPSULE_HEIGHT: f32 = 1.8;

pub const PROP_HIT_INTERVAL: f32 = 0.2;

fn active_weapon_slot(fighter: &Fighter) -> usize {
    usize::from(fighter.weapon_slot.min(1))
}

fn within_pickup_range(item: Vec3, fighter_feet: Vec3) -> bool {
    let capsule_y = item
        .y
        .clamp(fighter_feet.y, fighter_feet.y + PICKUP_CAPSULE_HEIGHT);
    let delta = item - Vec3::new(fighter_feet.x, capsule_y, fighter_feet.z);
    delta.length_squared() <= PICKUP_RANGE * PICKUP_RANGE
}

pub(crate) fn drop_spare_weapon(fighter: &mut Fighter) -> Option<(u8, u8)> {
    let active_slot = active_weapon_slot(fighter);
    let spare_slot = 1 - active_slot;
    let spare_weapon = fighter.weapon_slots[spare_slot];
    if fighter.weapon == 0
        || fighter.weapon_slots[active_slot] != fighter.weapon
        || spare_weapon == 0
    {
        return None;
    }
    let variant = fighter.weapon_slot_variants[spare_slot];
    fighter.weapon_slots[spare_slot] = 0;
    fighter.weapon_slot_variants[spare_slot] = 0;
    Some((spare_weapon, variant))
}

fn weapon_from_model(model: &str) -> Option<(u8, u8)> {
    let model = model.trim().to_ascii_lowercase();
    let (resource_class, variant_with_suffix) = model.strip_prefix('w')?.split_once('_')?;
    let resource_class = resource_class.parse::<u8>().ok()?;
    let variant = variant_with_suffix
        .trim_end_matches(['r', 'l', 'a', 'b'])
        .parse::<u8>()
        .ok()?;
    let weapon = match resource_class {
        0 => 6,
        1 => 5,
        2 => 3,
        3 => 1,
        4 => 9,
        5 => 8,
        6 => 2,
        7 => 7,
        8 => 4,
        9 => 10,
        10 => 11,
        11 => 12,
        _ => return None,
    };
    Some((weapon, variant))
}

fn weapon_respawn_seconds(model: &str) -> Option<f32> {
    let normalized = model.trim().to_ascii_lowercase();
    let (resource_class, variant_with_suffix) = normalized.strip_prefix('w')?.split_once('_')?;
    let resource_class = resource_class.parse::<u8>().ok()?;
    let variant = variant_with_suffix
        .trim_end_matches(['r', 'l', 'a', 'b'])
        .parse::<u8>()
        .ok()?;
    Some(match resource_class {
        0 => 30.0,
        1 => 45.0,
        2 => 75.0,
        3 | 4 => 30.0,
        5 => 60.0,
        6 => 45.0,
        7 => 75.0,
        8 => 60.0,
        9 | 10 => 30.0,
        11 if variant == 0 => 45.0,
        11 => 30.0,
        _ => return None,
    })
}

fn store_picked_weapon(fighter: &mut Fighter, weapon: u8, variant: u8) -> Option<usize> {
    if fighter
        .weapon_slots
        .iter()
        .zip(fighter.weapon_slot_variants)
        .any(|(held, held_variant)| *held == weapon && held_variant == variant)
    {
        return None;
    }
    let active_slot = active_weapon_slot(fighter);
    let inactive_slot = 1 - active_slot;
    let slot = (fighter.weapon_slots[active_slot] == 0)
        .then_some(active_slot)
        .or_else(|| (fighter.weapon_slots[inactive_slot] == 0).then_some(inactive_slot))?;
    fighter.weapon_slots[slot] = weapon;
    fighter.weapon_slot_variants[slot] = variant;
    if slot == active_slot {
        fighter.weapon = weapon;
        fighter.weapon_variant = variant;
    }
    Some(slot)
}

#[derive(Debug, Clone, Copy)]
struct ItemEffect {
    hp: i32,
    anger: i32,
    attack: i32,
    defense: i32,

    duration: f32,

    max_hp: i32,

    speed: f32,

    poison_per_tick: i32,
    poison_tick_every: f32,

    anger_per_tick: i32,
    anger_tick_every: f32,

    weapon_variant: i32,

    worn: Option<(&'static str, f32)>,
}

fn item_effect_for(model: &str) -> Option<(f32, ItemEffect)> {
    let effect = |hp: i32,
                  anger: i32,
                  attack: i32,
                  defense: i32,
                  duration: f32,
                  max_hp: i32,
                  speed: f32,
                  poison_per_tick: i32,
                  poison_tick_every: f32,
                  anger_per_tick: i32,
                  anger_tick_every: f32,
                  weapon_variant: i32,
                  worn: Option<(&'static str, f32)>| ItemEffect {
        hp,
        anger,
        attack,
        defense,
        duration,
        max_hp,
        speed,
        poison_per_tick,
        poison_tick_every,
        anger_per_tick,
        anger_tick_every,
        weapon_variant,
        worn,
    };
    let e = |hp: i32, anger: i32, attack: i32, defense: i32, duration: f32| {
        effect(
            hp, anger, attack, defense, duration, 0, 1.0, 0, 0.0, 0, 0.0, -1, None,
        )
    };
    Some(match model {
        "item01" => (120.0, e(300, 0, 0, 0, 0.0)),
        "item0101" => (150.0, e(500, 0, 0, 0, 0.0)),
        "item0102" => (60.0, e(100, 0, 0, 0, 0.0)),
        "item15" => (
            10.0,
            effect(
                50,
                0,
                0,
                0,
                0.0,
                0,
                1.0,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("Item15", 9999.9)),
            ),
        ),
        "item08" => (300.0, e(0, 100, 0, 0, 0.0)),
        "item11" => (99999.0, e(0, 25, 0, 0, 0.0)),
        "item16" => (
            10.0,
            effect(
                0,
                1,
                0,
                0,
                0.0,
                0,
                1.0,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("Item16", 9999.9)),
            ),
        ),
        "item03" => (
            300.0,
            effect(
                0,
                0,
                0,
                20,
                90.0,
                0,
                1.0,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("DefUP", 90.0)),
            ),
        ),
        "item12" => (
            10.0,
            effect(
                0,
                0,
                0,
                100,
                9999.9,
                0,
                1.0,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("Item12", 9999.9)),
            ),
        ),
        "item04" => (
            300.0,
            effect(
                0,
                0,
                25,
                0,
                60.0,
                0,
                1.0,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("PowUP", 60.0)),
            ),
        ),
        "item13" => (
            10.0,
            effect(
                0,
                0,
                125,
                0,
                9999.9,
                0,
                1.0,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("Item13", 9999.9)),
            ),
        ),

        "item02" => (
            6000.0,
            effect(
                0,
                0,
                0,
                0,
                600.0,
                500,
                1.0,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("MaxLifUP", 600.0)),
            ),
        ),
        "item10" => (
            15.0,
            effect(
                0,
                0,
                0,
                0,
                60.0,
                6999,
                1.0,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("Item17", 60.0)),
            ),
        ),

        "item05" => (
            180.0,
            effect(
                0,
                0,
                0,
                0,
                60.0,
                0,
                1.5,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("SpdUP", 60.0)),
            ),
        ),
        "item07" => (300.0, e(0, 0, 0, 0, 60.0)),
        "item14" => (
            10.0,
            effect(
                0,
                0,
                0,
                0,
                9999.9,
                0,
                1.5,
                0,
                0.0,
                0,
                0.0,
                -1,
                Some(("Item14", 9999.9)),
            ),
        ),

        "item0103" => (
            90.0,
            effect(
                0,
                0,
                0,
                0,
                30.0,
                0,
                1.0,
                10,
                5.0,
                0,
                0.0,
                -1,
                Some(("AttrPOIS", 30.0)),
            ),
        ),

        "hot" => (
            10.0,
            effect(
                0,
                0,
                0,
                0,
                9999.9,
                0,
                1.0,
                0,
                0.0,
                1,
                1.0,
                -1,
                Some(("HOT", 9999.9)),
            ),
        ),

        "item09" => (
            15.0,
            effect(0, 0, 0, 0, 0.0, 0, 1.0, 0, 0.0, 0, 0.0, 0, None),
        ),
        "item0901" => (
            15.0,
            effect(0, 0, 0, 0, 0.0, 0, 1.0, 0, 0.0, 0, 0.0, 1, None),
        ),
        "item0902" => (
            15.0,
            effect(0, 0, 0, 0, 0.0, 0, 1.0, 0, 0.0, 0, 0.0, 2, None),
        ),
        "item0903" => (
            15.0,
            effect(0, 0, 0, 0, 0.0, 0, 1.0, 0, 0.0, 0, 0.0, 3, None),
        ),
        "item0904" => (
            15.0,
            effect(0, 0, 0, 0, 0.0, 0, 1.0, 0, 0.0, 0, 0.0, 4, None),
        ),
        _ => return None,
    })
}

pub struct ItemPlugin;

impl Plugin for ItemPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ItemBuffs>()
            .init_resource::<PropHitGate>()
            .init_resource::<PendingSceneItemClaims>()
            .add_systems(
                OnEnter(AppState::Playing),
                clear_prop_hit_gate.run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                Update,
                (tick_npc_item_visuals, item_pickup)
                    .chain()
                    .before(super::status_plugin::tick_status_effects)
                    .before(crate::plugins::FighterFrameSet::SkeletonSample)
                    .before(super::fighter_frame::SerialFighterSet)
                    .run_if(in_state(AppState::Playing))
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(offline_item_authority_enabled),
            )
            .add_systems(
                Update,
                tick_item_buffs
                    .before(crate::plugins::FighterFrameSet::SkeletonSample)
                    .before(super::fighter_frame::SerialFighterSet)
                    .run_if(in_state(AppState::Playing))
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(local_item_timers_enabled),
            )
            .add_systems(
                Update,
                (
                    apply_network_item_states,
                    apply_network_item_effect_states,
                    apply_network_weapon_states,
                    apply_network_item_claims,
                    network_scene_item_claim,
                )
                    .chain()
                    .after(crate::plugins::network_plugin::NetworkFrameApply)
                    .before(crate::plugins::FighterFrameSet::SkeletonSample)
                    .before(super::fighter_frame::SerialFighterSet)
                    .run_if(in_state(AppState::Playing))
                    .run_if(network_item_authority_enabled),
            )
            .add_systems(
                Update,
                apply_stock_scene_item_pickups
                    .after(crate::plugins::network_plugin::NetworkFrameApply)
                    .before(crate::plugins::FighterFrameSet::SkeletonSample)
                    .before(super::fighter_frame::SerialFighterSet)
                    .run_if(in_state(AppState::Playing))
                    .run_if(stock_item_authority_enabled),
            );
        #[cfg(not(target_arch = "wasm32"))]
        app.init_resource::<StockSceneManifestState>()
            .add_systems(
                OnEnter(AppState::Playing),
                reset_stock_scene_manifest_state
                    .run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                Update,
                initialize_stock_scene_items
                    .after(crate::plugins::network_plugin::NetworkFrameApply)
                    .before(apply_stock_scene_item_pickups)
                    .run_if(in_state(AppState::Playing))
                    .run_if(stock_item_authority_enabled),
            );
    }
}

fn clear_prop_hit_gate(mut gate: ResMut<PropHitGate>) {
    gate.clear();
}

fn offline_item_authority_enabled(net: Res<crate::plugins::network_plugin::NetState>) -> bool {
    net.status != crate::plugins::network_plugin::NetStatus::InRoom
}

fn network_item_authority_enabled(net: Res<crate::plugins::network_plugin::NetState>) -> bool {
    net.status == crate::plugins::network_plugin::NetStatus::InRoom && !net.is_stock_udp()
}

fn stock_item_authority_enabled(net: Res<crate::plugins::network_plugin::NetState>) -> bool {
    net.status == crate::plugins::network_plugin::NetStatus::InRoom && net.is_stock_udp()
}

fn local_item_timers_enabled(net: Res<crate::plugins::network_plugin::NetState>) -> bool {
    net.status != crate::plugins::network_plugin::NetStatus::InRoom || net.is_stock_udp()
}

#[cfg(not(target_arch = "wasm32"))]
fn reset_stock_scene_manifest_state(mut state: ResMut<StockSceneManifestState>) {
    *state = StockSceneManifestState::default();
}

#[cfg(not(target_arch = "wasm32"))]
type StockManifestProps<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut StageProp,
        &'static Transform,
        &'static mut Visibility,
    ),
    (
        Without<super::escort_assets::EscortPropVisual>,
        Without<crate::plugins::render_plugin::DynamicWeaponDrop>,
    ),
>;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn initialize_stock_scene_items(
    net: Res<crate::plugins::network_plugin::NetState>,
    tx: Option<Res<crate::plugins::network_plugin::NetTx>>,
    mut state: ResMut<StockSceneManifestState>,
    mut props: StockManifestProps<'_, '_>,
    mut script: ResMut<crate::plugins::stage_plugin::StageScriptRuntime>,
) {
    if state.initialized || props.is_empty() {
        return;
    }
    let catalog = stock_scene_item_catalog();
    if catalog.is_empty() {
        return;
    }
    if net.is_stock_host() {
        let Some(tx) = tx.as_deref() else {
            return;
        };
        let mut seen = std::collections::HashSet::new();
        let items = props
            .iter()
            .filter(|(prop, _, _)| seen.insert(prop.name.to_ascii_lowercase()))
            .filter_map(|(prop, transform, visibility)| {
                stock_authored_scene_item(prop, transform, visibility, &catalog, script.script())
            })
            .collect::<Vec<_>>();
        tx.send_stock_scene_manifest(items);
    }

    for (mut prop, _, mut visibility) in &mut props {
        if prop.name.starts_with("D_wpStock")
            || !catalog.contains_key(&stock_item_model_key(&prop.model))
        {
            continue;
        }
        prop.respawn_timer = 0.0;
        prop.interactive = false;
        *visibility = Visibility::Hidden;
        if let Some(runtime) = script.script_mut() {
            for (key, value) in [("active", 0.0), ("interactive", 0.0)] {
                runtime.set_item_property(
                    &prop.name,
                    key,
                    meteor_scene::scene_script_runtime::PstValue::Num(value),
                );
            }
        }
    }
    state.initialized = true;
}

#[cfg(not(target_arch = "wasm32"))]
fn stock_authored_scene_item(
    prop: &StageProp,
    transform: &Transform,
    visibility: &Visibility,
    catalog: &HashMap<String, StockSceneItemMetadata>,
    script: Option<&meteor_scene::scene_script_runtime::SceneScriptRuntime>,
) -> Option<meteor_network::stock_game::StockAuthoredSceneItem> {
    let metadata = catalog.get(&stock_item_model_key(&prop.model))?;
    Some(meteor_network::stock_game::StockAuthoredSceneItem {
        item_id: metadata.item_id,
        category: metadata.category,
        position: meteor_network::role_state::RoleVec3 {
            x: transform.translation.x,
            y: transform.translation.y,
            z: transform.translation.z,
        },
        half_extents: prop.half_extents.to_array(),
        respawn_ms: metadata.respawn_ms,
        scene_reference: script
            .and_then(|runtime| runtime.host().resolve_name(&prop.name))
            .and_then(|index| u32::try_from(index).ok()),
        active: !prop.broken && *visibility != Visibility::Hidden,
        interactive: prop.interactive,
    })
}

fn resource_roots(workdir: &std::path::Path) -> Vec<PathBuf> {
    crate::plugins::audio_plugin::resource_roots(workdir)
}

pub(crate) fn pickup_effect_for_model(model: &str) -> Option<&'static str> {
    let model = model.trim().to_ascii_lowercase();
    if let Some((_, effect)) = item_effect_for(&model) {
        return Some(if effect.weapon_variant >= 0 {
            "WeaponUP"
        } else {
            "GetItem"
        });
    }
    weapon_from_model(&model).map(|_| "GetItem")
}

fn queue_pickup_effect(commands: &mut Commands, model: &str, position: Vec3) {
    let Some(effect) = pickup_effect_for_model(model) else {
        return;
    };
    commands.spawn(crate::plugins::effect_plugin::PickupEffectRequest(
        crate::plugins::effect_plugin::SceneEffectEvent {
            effect: effect.into(),
            position,
        },
    ));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn item_pickup(
    mut commands: Commands,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut buffs: ResMut<ItemBuffs>,
    mut players: PickupPlayerQuery<'_, '_>,
    mut props: PickupPropsQuery<'_, '_>,
    mut script: ResMut<crate::plugins::stage_plugin::StageScriptRuntime>,
    resources: Option<Res<super::combat_plugin::CombatResourceData>>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());

    let mut collectors = players
        .iter()
        .map(|entry| (entry.4.player_slot, entry.0))
        .collect::<Vec<_>>();
    collectors.sort_by_key(|(slot, entity)| (*slot, entity.to_bits()));

    let mut seen_props = std::collections::HashSet::new();
    let mut consumed_props = HashMap::new();
    'props: for (prop_entity, mut prop, transform, mut visibility, dynamic_drop) in &mut props {
        if prop.respawn_timer > 0.0 {
            prop.respawn_timer -= dt;
            if prop.respawn_timer <= 0.0 {
                prop.interactive = true;
                *visibility = Visibility::Visible;
                if let Some(runtime) = script.script_mut() {
                    for (key, value) in [("active", 1.0), ("interactive", 1.0)] {
                        runtime.set_item_property(
                            &prop.name,
                            key,
                            meteor_scene::scene_script_runtime::PstValue::Num(value),
                        );
                    }
                }
            }
            continue;
        }
        if *visibility == Visibility::Hidden {
            continue;
        }
        if !seen_props.insert(prop.name.to_ascii_lowercase()) {
            continue;
        }
        if !prop.interactive || prop.broken {
            continue;
        }
        for &(_, collector) in &collectors {
            let Ok((
                player_entity,
                player_transform,
                mut health,
                mut qi,
                mut fighter,
                mut worn,
                mut concealment,
                mut status,
                local,
                npc,
                death,
                sm,
                actor_visibility,
            )) = players.get_mut(collector)
            else {
                continue;
            };
            if health.current <= 0.0
                || death.is_some_and(|death| death.active)
                || sm.is_some_and(|sm| sm.current == FighterState::Dead)
                || npc.is_some_and(|npc| !npc.in_scene_roster)
                || actor_visibility.is_some_and(|visibility| *visibility == Visibility::Hidden)
            {
                continue;
            }
            let character_id = npc.map_or(i64::from(fighter.player_slot), |npc| npc.id);
            if !within_pickup_range(prop.world_center(transform), player_transform.translation) {
                continue;
            }
            let Some((respawn, effect)) = item_effect_for(&prop.model) else {
                let Some((weapon, variant)) = weapon_from_model(&prop.model) else {
                    continue;
                };
                let Some(_stored_slot) = store_picked_weapon(&mut fighter, weapon, variant) else {
                    continue;
                };

                commands
                    .entity(player_entity)
                    .insert(crate::components::fighter::WeaponVariantChanged);
                info!("weapon pickup {} stored as weapon {}", prop.model, weapon);
                queue_pickup_effect(&mut commands, &prop.model, transform.translation);
                if dynamic_drop.is_some() {
                    commands.entity(prop_entity).despawn();
                    continue 'props;
                }
                prop.hit_count += 1;
                prop.respawn_timer = weapon_respawn_seconds(&prop.model).unwrap_or(0.0);
                prop.interactive = false;
                *visibility = Visibility::Hidden;
                consumed_props.insert(
                    prop.name.to_ascii_lowercase(),
                    (prop.respawn_timer, prop.hit_count),
                );
                if let Some(runtime) = script.script_mut() {
                    for (key, value) in [("active", 0.0), ("interactive", 0.0)] {
                        runtime.set_item_property(
                            &prop.name,
                            key,
                            meteor_scene::scene_script_runtime::PstValue::Num(value),
                        );
                    }
                    if let Err(error) = runtime.call_item(
                        &prop.name,
                        "OnPickUp",
                        &[meteor_scene::scene_script_runtime::PstValue::Num(
                            character_id as f64,
                        )],
                    ) {
                        warn!("script callItem OnPickUp failed: {error}");
                    }
                }
                continue 'props;
            };
            if local.is_none() {
                let Some(definition) = resources
                    .as_ref()
                    .and_then(|resources| resources.0.status_effect_for_model(&prop.model))
                else {
                    continue;
                };

                status.apply(definition);
            } else {
                if effect.hp != 0 {
                    health.current = (health.current + effect.hp as f32).min(health.max);
                }
                if effect.anger != 0 {
                    qi.current = (qi.current + effect.anger as f32).max(0.0);
                }
                if effect.attack != 0 || effect.defense != 0 {
                    buffs.attack_bonus = effect.attack;
                    buffs.defense_bonus = effect.defense;
                    buffs.remaining = effect.duration;
                }
                if effect.max_hp != 0 {
                    health.max += effect.max_hp as f32;
                    health.current = health.current.min(health.max);
                    buffs.max_hp_bonus = effect.max_hp;
                    buffs.max_hp_remaining = effect.duration;
                }
                if effect.speed != 1.0 {
                    buffs.speed_factor = effect.speed;
                    buffs.speed_remaining = effect.duration;
                }
                if effect.poison_per_tick != 0 {
                    buffs.poison_per_tick = effect.poison_per_tick;
                    buffs.poison_tick_every = effect.poison_tick_every;
                    buffs.poison_total = effect.duration;
                    buffs.poison_tick = effect.poison_tick_every;
                }
                if effect.anger_per_tick != 0 {
                    buffs.anger_per_tick = effect.anger_per_tick;
                    buffs.anger_tick_every = effect.anger_tick_every;
                    buffs.anger_total = effect.duration;
                    buffs.anger_tick = effect.anger_tick_every;
                }
                if prop.model.eq_ignore_ascii_case("item07") {
                    concealment.remaining = effect.duration;
                }
            }

            if effect.weapon_variant >= 0 && fighter.weapon != 0 {
                let active_slot = active_weapon_slot(&fighter);
                fighter.weapon_variant = effect.weapon_variant as u8;
                fighter.weapon_slot_variants[active_slot] = fighter.weapon_variant;
                commands
                    .entity(player_entity)
                    .insert(crate::components::fighter::WeaponVariantChanged);
            }

            if let Some((name, duration)) = effect.worn {
                worn.refresh(name, duration);
                info!("worn effect: {name} ({duration}s)");
            }
            info!(
                "pickup {}: hp{} anger{} atk{} def{} maxhp{} speed{} poison{}/{}s anger{}/{}s",
                prop.model,
                effect.hp,
                effect.anger,
                effect.attack,
                effect.defense,
                effect.max_hp,
                effect.speed,
                effect.poison_per_tick,
                effect.poison_tick_every,
                effect.anger_per_tick,
                effect.anger_tick_every
            );

            queue_pickup_effect(&mut commands, &prop.model, transform.translation);
            prop.hit_count += 1;
            prop.respawn_timer = respawn;
            prop.interactive = false;
            *visibility = Visibility::Hidden;
            consumed_props.insert(
                prop.name.to_ascii_lowercase(),
                (prop.respawn_timer, prop.hit_count),
            );

            if let Some(runtime) = script.script_mut() {
                for (key, value) in [("active", 0.0), ("interactive", 0.0)] {
                    runtime.set_item_property(
                        &prop.name,
                        key,
                        meteor_scene::scene_script_runtime::PstValue::Num(value),
                    );
                }
                if let Err(error) = runtime.call_item(
                    &prop.name,
                    "OnPickUp",
                    &[meteor_scene::scene_script_runtime::PstValue::Num(
                        character_id as f64,
                    )],
                ) {
                    warn!("script callItem OnPickUp failed: {error}");
                }
            }
            continue 'props;
        }
    }

    for (_, mut prop, _, mut visibility, _) in &mut props {
        if let Some(&(respawn, hits)) = consumed_props.get(&prop.name.to_ascii_lowercase()) {
            prop.respawn_timer = respawn;
            prop.hit_count = hits;
            prop.interactive = false;
            *visibility = Visibility::Hidden;
        }
    }
}

fn tick_npc_item_visuals(
    time: Res<Time>,
    clock: Option<Res<super::FrameClock>>,
    mut npcs: Query<
        (
            &Health,
            &mut crate::components::fighter::WornEffect,
            &mut crate::components::fighter::Concealment,
            Option<&DeathRespawn>,
        ),
        (
            With<super::combat_plugin::NpcFighter>,
            Without<super::stage_plugin::LocalFighter>,
            Without<super::network_plugin::RemoteFighter>,
        ),
    >,
) {
    let dt = super::simulation_dt(clock.as_deref(), time.delta_secs());
    for (health, mut worn, mut concealment, death) in &mut npcs {
        if health.current <= 0.0 || death.is_some_and(|death| death.active) {
            worn.effects.clear();
            concealment.remaining = 0.0;
            continue;
        }
        for effect in &mut worn.effects {
            effect.remaining -= dt;
        }
        worn.effects.retain(|effect| effect.remaining > 0.0);
        concealment.remaining = (concealment.remaining - dt).max(0.0);
    }
}

fn network_scene_item_claim(
    time: Res<Time>,
    net: Res<crate::plugins::network_plugin::NetState>,
    net_tx: Option<Res<crate::plugins::network_plugin::NetTx>>,
    mut pending: ResMut<PendingSceneItemClaims>,
    local: Query<(&Transform, &Health), With<crate::plugins::stage_plugin::LocalFighter>>,
    remotes: Query<
        (
            &Transform,
            &Health,
            &crate::plugins::network_plugin::RemoteFighter,
        ),
        With<crate::plugins::network_plugin::RemoteFighter>,
    >,
    props: Query<
        (&StageProp, &Transform, &Visibility),
        Without<super::escort_assets::EscortPropVisual>,
    >,
) {
    let now = time.elapsed_secs();
    pending.expires_at.retain(|_, expires_at| *expires_at > now);
    if !net.round_active || !net.is_scene_authority() {
        return;
    }
    let Some(net_tx) = net_tx else {
        return;
    };

    let mut collectors = Vec::new();
    if let Ok((transform, health)) = local.single() {
        if health.current > 0.0 {
            collectors.push((net.player_id.clone(), transform.translation));
        }
    }
    collectors.extend(
        remotes
            .iter()
            .filter(|&(_, health, _)| health.current > 0.0)
            .map(|(transform, _, remote)| (remote.player_id.clone(), transform.translation)),
    );

    for (player_id, player_position) in collectors {
        for (prop, transform, visibility) in &props {
            if prop.broken
                || !prop.interactive
                || *visibility == Visibility::Hidden
                || pending.expires_at.contains_key(&prop.name)
                || !network_pickable_model(&prop.model)
            {
                continue;
            }
            if !within_pickup_range(prop.world_center(transform), player_position) {
                continue;
            }
            pending.expires_at.insert(prop.name.clone(), now + 1.2);
            net_tx.send_control(Message::ItemClaim {
                reference: prop.name.clone(),
                player_id,
                model: prop.model.clone(),
                position: Some(transform.translation.to_array()),
                renewable: true,
            });
            return;
        }
    }
}

type StockSceneProps<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut StageProp,
        &'static Transform,
        &'static mut Visibility,
    ),
    Without<super::escort_assets::EscortPropVisual>,
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_stock_scene_item_pickups(
    mut commands: Commands,
    mut net: ResMut<crate::plugins::network_plugin::NetState>,
    mut buffs: ResMut<ItemBuffs>,
    mut props: StockSceneProps<'_, '_>,
    mut fighters: NetworkItemFighterQuery<'_, '_>,
    mut script: ResMut<crate::plugins::stage_plugin::StageScriptRuntime>,
    actor_transforms: Query<&Transform, With<Fighter>>,
) {
    let grounded = std::mem::take(&mut net.pending_stock_scene_grounded);
    let pickups = std::mem::take(&mut net.pending_stock_scene_items);
    if grounded.is_empty() && pickups.is_empty() {
        return;
    }
    let names = stock_scene_item_names();

    for record in grounded {
        let Some(model) = names.get(&(record.category, record.item_id)) else {
            continue;
        };
        if let Some(entity) = nearest_stock_prop(&mut props, model, record.position, true) {
            if let Ok((_, mut prop, _, mut visibility)) = props.get_mut(entity) {
                prop.respawn_timer = 0.0;
                prop.interactive = true;
                *visibility = Visibility::Visible;
                if let Some(runtime) = script.script_mut() {
                    for (key, value) in [("active", 1.0), ("interactive", 1.0)] {
                        runtime.set_item_property(
                            &prop.name,
                            key,
                            meteor_scene::scene_script_runtime::PstValue::Num(value),
                        );
                    }
                }
            }
        } else if record.category == 1 {
            let reference = format!("D_wpStock{}", record.record_id);
            let drop = crate::plugins::network_plugin::AuthorityWeaponDrop {
                reference: reference.clone(),
                model: model.clone(),
                position: record.position,
                velocity: None,
                settled: true,
                pickup_ready: true,
            };
            net.dynamic_weapon_drops.insert(reference, drop.clone());
            if net.pending_weapon_drops.len() >= 64 {
                net.pending_weapon_drops.remove(0);
            }
            net.pending_weapon_drops.push(drop);
        }
    }

    for pickup in pickups {
        let Some(model) = names.get(&(pickup.category, pickup.item_id)) else {
            continue;
        };
        let player_id = format!("stock-{}", pickup.actor_id);
        let mut effect_position = pickup.position.map(Vec3::from_array);
        net.dynamic_weapon_drops
            .remove(&format!("D_wpStock{}", pickup.record_id));
        let script_character_id = net
            .members
            .iter()
            .position(|member| member.player_id == player_id)
            .map_or(-1.0, |index| index as f64);
        let prop_entity = pickup
            .position
            .and_then(|position| nearest_stock_prop(&mut props, model, position, false));
        if let Some(entity) = prop_entity {
            if let Ok((_, mut prop, transform, mut visibility)) = props.get_mut(entity) {
                effect_position = Some(transform.translation);
                prop.hit_count = prop.hit_count.saturating_add(1);
                prop.respawn_timer = 0.0;
                prop.interactive = false;
                *visibility = Visibility::Hidden;
                if !prop.name.starts_with("D_wpStock") {
                    if let Some(runtime) = script.script_mut() {
                        for (key, value) in [("active", 0.0), ("interactive", 0.0)] {
                            runtime.set_item_property(
                                &prop.name,
                                key,
                                meteor_scene::scene_script_runtime::PstValue::Num(value),
                            );
                        }
                        if let Err(error) = runtime.call_item(
                            &prop.name,
                            "OnPickUp",
                            &[meteor_scene::scene_script_runtime::PstValue::Num(
                                script_character_id,
                            )],
                        ) {
                            warn!("stock item OnPickUp failed: {error}");
                        }
                    }
                }
            }
        }

        for (entity, local, remote, mut health, mut qi, mut fighter, mut worn, mut concealment) in
            &mut fighters
        {
            let owns_pickup = local.is_some_and(|_| player_id == net.player_id)
                || remote.is_some_and(|remote| remote.player_id == player_id);
            if !owns_pickup {
                continue;
            }
            if effect_position.is_none() {
                effect_position = actor_transforms.get(entity).ok().map(|t| t.translation);
            }
            if pickup.category == 1 {
                if let Some((weapon, variant)) = weapon_from_model(model) {
                    if store_picked_weapon(&mut fighter, weapon, variant).is_some() {
                        commands
                            .entity(entity)
                            .insert(crate::components::fighter::WeaponVariantChanged);
                    }
                }
                continue;
            }
            let Some((_, effect)) = item_effect_for(model) else {
                continue;
            };
            if effect.hp != 0 {
                health.current = (health.current + effect.hp as f32).min(health.max);
            }
            if effect.anger != 0 {
                qi.current = (qi.current + effect.anger as f32).max(0.0);
            }
            if effect.max_hp != 0 {
                health.max += effect.max_hp as f32;
                health.current = health.current.min(health.max);
            }
            if local.is_some() {
                if effect.attack != 0 || effect.defense != 0 {
                    buffs.attack_bonus = effect.attack;
                    buffs.defense_bonus = effect.defense;
                    buffs.remaining = effect.duration;
                }
                if effect.max_hp != 0 {
                    buffs.max_hp_bonus = effect.max_hp;
                    buffs.max_hp_remaining = effect.duration;
                }
                if effect.speed != 1.0 {
                    buffs.speed_factor = effect.speed;
                    buffs.speed_remaining = effect.duration;
                }
                if effect.poison_per_tick != 0 {
                    buffs.poison_per_tick = effect.poison_per_tick;
                    buffs.poison_tick_every = effect.poison_tick_every;
                    buffs.poison_total = effect.duration;
                    buffs.poison_tick = effect.poison_tick_every;
                }
                if effect.anger_per_tick != 0 {
                    buffs.anger_per_tick = effect.anger_per_tick;
                    buffs.anger_tick_every = effect.anger_tick_every;
                    buffs.anger_total = effect.duration;
                    buffs.anger_tick = effect.anger_tick_every;
                }
            }
            if model.eq_ignore_ascii_case("item07") {
                concealment.remaining = effect.duration;
            }
            if effect.weapon_variant >= 0 {
                let active_slot = active_weapon_slot(&fighter);
                fighter.weapon_variant = effect.weapon_variant as u8;
                fighter.weapon_slot_variants[active_slot] = fighter.weapon_variant;
                commands
                    .entity(entity)
                    .insert(crate::components::fighter::WeaponVariantChanged);
            }
            if let Some((name, duration)) = effect.worn {
                worn.refresh(name, duration);
            }
        }
        if let Some(position) = effect_position {
            queue_pickup_effect(&mut commands, model, position);
        }
    }
}

fn nearest_stock_prop(
    props: &mut StockSceneProps<'_, '_>,
    model: &str,
    position: [f32; 3],
    authored_only: bool,
) -> Option<Entity> {
    let position = Vec3::from_array(position);
    let model_key = stock_item_model_key(model);
    props
        .iter_mut()
        .filter(|(_, prop, _, _)| {
            stock_item_model_key(&prop.model) == model_key
                && (!authored_only || !prop.name.starts_with("D_wpStock"))
        })
        .filter(|(_, _, transform, _)| {
            transform.translation.distance_squared(position)
                <= STOCK_PROP_MATCH_MAX_DISTANCE * STOCK_PROP_MATCH_MAX_DISTANCE
        })
        .min_by(|(_, _, left, _), (_, _, right, _)| {
            left.translation
                .distance_squared(position)
                .total_cmp(&right.translation.distance_squared(position))
        })
        .map(|(entity, _, _, _)| entity)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StockSceneItemMetadata {
    category: u32,
    item_id: u32,
    respawn_ms: Option<u64>,
}

fn stock_item_model_key(model: &str) -> String {
    let model = model.trim().to_ascii_lowercase();
    if weapon_from_model(&model).is_some() {
        model.trim_end_matches(['r', 'l', 'a', 'b']).to_string()
    } else {
        model
    }
}

fn stock_scene_item_catalog() -> HashMap<String, StockSceneItemMetadata> {
    let bytes = meteor_asset::runtime_assets::read_legacy_path("Meteor.res").or_else(|| {
        let current = std::env::current_dir().ok()?;
        resource_roots(&current)
            .into_iter()
            .find_map(|root| crate::resource_io::read(root.join("Meteor.res")).ok())
    });
    let Some(resource) = bytes
        .as_deref()
        .and_then(|bytes| meteor_asset::res::parse_meteor_res(bytes).ok())
    else {
        return HashMap::new();
    };
    resource
        .records
        .into_iter()
        .filter(|record| matches!(record.category, 1 | 2) && !record.name.trim().is_empty())
        .map(|record| {
            let metadata = StockSceneItemMetadata {
                category: record.category,
                item_id: record.id,
                respawn_ms: record
                    .prop_seed_value(13)
                    .filter(|seconds| *seconds > 0)
                    .and_then(|seconds| u64::try_from(seconds).ok())
                    .map(|seconds| seconds.saturating_mul(1_000)),
            };
            (stock_item_model_key(&record.name), metadata)
        })
        .collect()
}

fn stock_scene_item_names() -> HashMap<(u32, u32), String> {
    stock_scene_item_catalog()
        .into_iter()
        .map(|(name, metadata)| ((metadata.category, metadata.item_id), name))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn apply_network_item_claims(
    mut commands: Commands,
    mut net: ResMut<crate::plugins::network_plugin::NetState>,
    mut pending: ResMut<PendingSceneItemClaims>,
    mut buffs: ResMut<ItemBuffs>,
    mut props: StockSceneProps<'_, '_>,
    mut fighters: NetworkItemFighterQuery<'_, '_>,
    mut script: ResMut<crate::plugins::stage_plugin::StageScriptRuntime>,
    actor_transforms: Query<&Transform, With<Fighter>>,
) {
    let claims = std::mem::take(&mut net.pending_item_claims);
    for message in claims {
        let Message::ItemClaimed {
            reference,
            player_id,
            model,
            weapon,
            weapon_model,
            weapon_slot,
            weapon_slots,
            health: authoritative_health,
            max_health,
            anger,
            item_effects,
            ..
        } = message
        else {
            continue;
        };

        net.pending_status_snapshots
            .insert(player_id.clone(), item_effects.clone());
        pending.expires_at.remove(&reference);
        let mut effect_position = None;
        for (_, mut prop, transform, mut visibility) in &mut props {
            if prop.name.eq_ignore_ascii_case(&reference) {
                effect_position.get_or_insert(transform.translation);

                prop.interactive = false;
                prop.respawn_timer = 0.0;
                *visibility = Visibility::Hidden;
            }
        }

        let scene_host = net.is_scene_authority();
        let is_local_collector = player_id == net.player_id;
        for (entity, local, remote, mut health, mut qi, mut fighter, mut worn, mut concealment) in
            &mut fighters
        {
            let owns_claim = local.is_some_and(|_| is_local_collector)
                || remote.is_some_and(|remote| remote.player_id == player_id);
            if !owns_claim {
                continue;
            }
            if effect_position.is_none() {
                effect_position = actor_transforms.get(entity).ok().map(|t| t.translation);
            }
            apply_authoritative_combatant_projection(
                &mut commands,
                entity,
                &mut health,
                &mut qi,
                &mut fighter,
                authoritative_health,
                max_health,
                anger,
                &weapon,
                weapon_model.as_deref(),
                weapon_slot,
                &weapon_slots,
            );
            if local.is_some() {
                sync_network_item_effects(&mut buffs, &mut worn, &mut concealment, &item_effects);
            } else {
                sync_network_visual_effects(&mut worn, &mut concealment, &item_effects);
            }
        }
        if let Some(position) = effect_position {
            queue_pickup_effect(&mut commands, &model, position);
        }

        if scene_host && !reference.starts_with("D_wpNet") {
            let character_id = net
                .members
                .iter()
                .position(|member| member.player_id == player_id)
                .map_or(-1.0, |index| index as f64);
            if let Some(runtime) = script.script_mut() {
                if let Err(error) = runtime.call_item(
                    &reference,
                    "OnPickUp",
                    &[meteor_scene::scene_script_runtime::PstValue::Num(
                        character_id,
                    )],
                ) {
                    warn!("script callItem OnPickUp failed: {error}");
                }
            }
        }
    }
}

fn apply_network_item_states(
    mut commands: Commands,
    mut net: ResMut<crate::plugins::network_plugin::NetState>,
    mut buffs: ResMut<ItemBuffs>,
    mut fighters: NetworkItemFighterQuery<'_, '_>,
) {
    let states = std::mem::take(&mut net.pending_item_states);
    let mut deferred = Vec::new();
    for state in states {
        net.pending_status_snapshots
            .insert(state.player_id.clone(), state.item_effects.clone());
        let is_local_collector = state.player_id == net.player_id;
        let mut applied = false;
        for (entity, local, remote, mut health, mut qi, mut fighter, mut worn, mut concealment) in
            &mut fighters
        {
            let owns_state = local.is_some_and(|_| is_local_collector)
                || remote.is_some_and(|remote| remote.player_id == state.player_id);
            if !owns_state {
                continue;
            }
            apply_authoritative_combatant_projection(
                &mut commands,
                entity,
                &mut health,
                &mut qi,
                &mut fighter,
                state.health,
                state.max_health,
                state.anger,
                &state.weapon,
                state.weapon_model.as_deref(),
                state.weapon_slot,
                &state.weapon_slots,
            );
            if local.is_some() {
                sync_network_item_effects(
                    &mut buffs,
                    &mut worn,
                    &mut concealment,
                    &state.item_effects,
                );
            } else {
                sync_network_visual_effects(&mut worn, &mut concealment, &state.worn_effects);
            }
            applied = true;
            break;
        }
        if !applied {
            deferred.push(state);
        }
    }
    net.pending_item_states = deferred;
}

fn apply_network_item_effect_states(
    mut net: ResMut<crate::plugins::network_plugin::NetState>,
    mut buffs: ResMut<ItemBuffs>,
    mut fighters: NetworkItemFighterQuery<'_, '_>,
) {
    let states = std::mem::take(&mut net.pending_item_effect_states);
    let mut deferred = Vec::new();
    for state in states {
        net.pending_status_snapshots
            .insert(state.player_id.clone(), state.item_effects.clone());
        let is_local = state.player_id == net.player_id;
        let mut applied = false;
        for (_, local, remote, mut health, mut qi, _, mut worn, mut concealment) in &mut fighters {
            let owns_state = local.is_some_and(|_| is_local)
                || remote.is_some_and(|remote| remote.player_id == state.player_id);
            if !owns_state {
                continue;
            }
            health.max = state.max_health.max(1.0);
            health.current = state.health.clamp(0.0, health.max);
            qi.current = state.anger.clamp(0.0, qi.max);
            if local.is_some() {
                sync_network_item_effects(
                    &mut buffs,
                    &mut worn,
                    &mut concealment,
                    &state.item_effects,
                );
            } else {
                sync_network_visual_effects(&mut worn, &mut concealment, &state.item_effects);
            }
            applied = true;
            break;
        }
        if !applied {
            deferred.push(state);
        }
    }
    net.pending_item_effect_states = deferred;
}

fn apply_network_weapon_states(
    mut commands: Commands,
    mut net: ResMut<crate::plugins::network_plugin::NetState>,
    mut fighters: NetworkItemFighterQuery<'_, '_>,
) {
    let states = std::mem::take(&mut net.pending_weapon_states);
    let mut deferred = Vec::new();
    for state in states {
        let is_local = state.player_id == net.player_id;
        let mut applied = false;
        for (entity, local, remote, _, _, mut fighter, _, _) in &mut fighters {
            let owns_state = local.is_some_and(|_| is_local)
                || remote.is_some_and(|remote| remote.player_id == state.player_id);
            if !owns_state {
                continue;
            }
            apply_authoritative_weapon_projection(
                &mut commands,
                entity,
                &mut fighter,
                &state.weapon,
                state.weapon_model.as_deref(),
                state.weapon_slot,
                &state.weapon_slots,
            );
            applied = true;
            break;
        }
        if !applied {
            deferred.push(state);
        }
    }
    net.pending_weapon_states = deferred;
}

#[allow(clippy::too_many_arguments)]
fn apply_authoritative_combatant_projection(
    commands: &mut Commands,
    entity: Entity,
    health: &mut Health,
    qi: &mut Qi,
    fighter: &mut Fighter,
    authoritative_health: f32,
    max_health: f32,
    anger: f32,
    weapon: &str,
    weapon_model: Option<&str>,
    weapon_slot: u8,
    weapon_slots: &[ItemWeaponSlotWire],
) {
    health.max = max_health.max(1.0);
    health.current = authoritative_health.clamp(0.0, health.max);
    qi.current = anger.clamp(0.0, qi.max);

    apply_authoritative_weapon_projection(
        commands,
        entity,
        fighter,
        weapon,
        weapon_model,
        weapon_slot,
        weapon_slots,
    );
}

fn apply_authoritative_weapon_projection(
    commands: &mut Commands,
    entity: Entity,
    fighter: &mut Fighter,
    weapon: &str,
    weapon_model: Option<&str>,
    weapon_slot: u8,
    weapon_slots: &[ItemWeaponSlotWire],
) {
    let old_slots = fighter.weapon_slots;
    let old_slot_variants = fighter.weapon_slot_variants;
    if !weapon_slots.is_empty() {
        let (slots, variants) = weapon_slots_from_wire(weapon_slots);
        fighter.weapon_slots = slots;
        fighter.weapon_slot_variants = variants;
    }
    let slot = usize::from(weapon_slot).min(fighter.weapon_slots.len() - 1);
    fighter.weapon_slot = slot as u8;
    let slot_weapon = fighter.weapon_slots[slot];
    fighter.weapon = if slot_weapon != 0 {
        slot_weapon
    } else if let Some(projected) = network_weapon_id(weapon) {
        fighter.weapon_slots[slot] = projected;
        projected
    } else {
        fighter.weapon
    };
    fighter.weapon_variant = weapon_variant_from_model(weapon_model)
        .or_else(|| {
            weapon_slots
                .iter()
                .find(|entry| entry.index as usize == slot)
                .map(|entry| entry.variant)
        })
        .unwrap_or(fighter.weapon_slot_variants[slot]);
    fighter.weapon_slot_variants[slot] = fighter.weapon_variant;

    if weapon_inventory_changed(old_slots, old_slot_variants, fighter) {
        commands
            .entity(entity)
            .insert(crate::components::fighter::WeaponVariantChanged);
    }
}

fn weapon_inventory_changed(old_slots: [u8; 2], old_variants: [u8; 2], fighter: &Fighter) -> bool {
    fighter.weapon_slots != old_slots || fighter.weapon_slot_variants != old_variants
}

fn network_pickable_model(model: &str) -> bool {
    item_effect_for(&model.to_ascii_lowercase()).is_some()
        || weapon_variant_from_model(Some(model)).is_some()
}

fn weapon_slots_from_wire(entries: &[ItemWeaponSlotWire]) -> ([u8; 2], [u8; 2]) {
    let mut slots = [0, 0];
    let mut variants = [0, 0];
    for entry in entries {
        let index = entry.index as usize;
        if index < slots.len() && entry.present {
            slots[index] = entry
                .weapon_id
                .or_else(|| network_weapon_id(&entry.weapon))
                .unwrap_or(0);
            variants[index] = entry.variant;
        }
    }
    (slots, variants)
}

fn network_weapon_id(weapon: &str) -> Option<u8> {
    Some(match weapon {
        "dart" => 1,
        "wheel" => 2,
        "gun" => 3,
        "stinger" => 4,
        "dagger" => 5,
        "sword" => 6,
        "spear" => 7,
        "blade" => 8,
        "hammer" => 9,
        "staffBlade" => 10,
        "fist" => 11,
        "ninjaBlade" => 12,
        _ => return None,
    })
}

fn weapon_variant_from_model(model: Option<&str>) -> Option<u8> {
    let model = model?.trim().to_ascii_lowercase();
    let (class, variant) = model.strip_prefix('w')?.split_once('_')?;
    if class.is_empty() || !class.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let class = class.parse::<u8>().ok()?;
    let max_variant = match class {
        3 | 6 => 5,
        0..=8 => 4,
        9..=11 => 3,
        _ => return None,
    };
    let mut variant = variant.bytes();
    let value = variant.next()?.checked_sub(b'0')?;
    if value > max_variant || !variant.all(|byte| matches!(byte, b'r' | b'l' | b'a' | b'b')) {
        return None;
    }
    Some(value)
}

fn sync_network_item_effects(
    buffs: &mut ItemBuffs,
    worn: &mut crate::components::fighter::WornEffect,
    concealment: &mut crate::components::fighter::Concealment,
    effects: &[ItemEffectWire],
) {
    *buffs = ItemBuffs::default();
    sync_network_visual_effects(worn, concealment, effects);
    for active in effects {
        let Some((_, effect)) = item_effect_for(&active.model.to_ascii_lowercase()) else {
            continue;
        };
        let remaining = active.remaining.max(0.0);
        if effect.attack != 0 || effect.defense != 0 {
            buffs.attack_bonus += effect.attack;
            buffs.defense_bonus += effect.defense;
            buffs.remaining = buffs.remaining.max(remaining);
        }
        if effect.max_hp != 0 {
            buffs.max_hp_bonus += effect.max_hp;
            buffs.max_hp_remaining = buffs.max_hp_remaining.max(remaining);
        }
        if effect.speed != 1.0 {
            buffs.speed_factor = buffs.speed_factor.max(effect.speed);
            buffs.speed_remaining = buffs.speed_remaining.max(remaining);
        }
    }
}

fn sync_network_visual_effects(
    worn: &mut crate::components::fighter::WornEffect,
    concealment: &mut crate::components::fighter::Concealment,
    effects: &[ItemEffectWire],
) {
    worn.clear();
    concealment.remaining = 0.0;
    for active in effects {
        if active.model.eq_ignore_ascii_case("item07") {
            concealment.remaining = concealment.remaining.max(active.remaining.max(0.0));
        }
        let Some((_, effect)) = item_effect_for(&active.model.to_ascii_lowercase()) else {
            continue;
        };
        let Some((name, _)) = effect.worn else {
            continue;
        };
        worn.refresh(name, active.remaining);
    }
}

pub(crate) fn tick_item_buffs(
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    mut buffs: ResMut<ItemBuffs>,
    net: Res<crate::plugins::network_plugin::NetState>,
    mut death_weapon_drops: MessageWriter<crate::plugins::render_plugin::OfflineWeaponDeathDrop>,
    mut players: BuffTickPlayerQuery<'_, '_>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let Ok((
        fighter,
        transform,
        motion,
        mut health,
        mut qi,
        mut worn,
        mut concealment,
        mut death,
        mut sm_state,
        mut combat,
        mut pose,
        status,
    )) = players.single_mut()
    else {
        return;
    };

    for effect in &mut worn.effects {
        effect.remaining -= dt;
    }
    worn.effects.retain(|effect| {
        let active = effect.remaining > 0.0;
        if !active {
            info!("worn effect expired: {}", effect.name);
        }
        active
    });
    if concealment.remaining > 0.0 {
        concealment.remaining = (concealment.remaining - dt).max(0.0);
    }

    if buffs.remaining > 0.0 {
        buffs.remaining -= dt;
        if buffs.remaining <= 0.0 {
            buffs.attack_bonus = 0;
            buffs.defense_bonus = 0;
        }
    }

    if buffs.max_hp_remaining > 0.0 {
        buffs.max_hp_remaining -= dt;
        if buffs.max_hp_remaining <= 0.0 {
            health.max = (health.max - buffs.max_hp_bonus as f32).max(1.0);
            health.current = health.current.min(health.max);
            buffs.max_hp_bonus = 0;
        }
    }

    if buffs.speed_remaining > 0.0 {
        buffs.speed_remaining -= dt;
        if buffs.speed_remaining <= 0.0 {
            buffs.speed_factor = 1.0;
        }
    }

    if buffs.poison_total > 0.0 {
        buffs.poison_total -= dt;
        buffs.poison_tick -= dt;
        if buffs.poison_tick <= 0.0 {
            health.current = (health.current - buffs.poison_per_tick as f32).max(0.0);
            if let Some(mut status) = status {
                status.pending_periodic_hit |= buffs.poison_per_tick > 0 && health.current > 0.0;
            }
            buffs.poison_tick = buffs.poison_tick_every;
        }
        if buffs.poison_total <= 0.0 {
            buffs.poison_per_tick = 0;
            buffs.poison_tick_every = 0.0;
            buffs.poison_total = 0.0;
        }
    }

    if buffs.anger_total > 0.0 {
        buffs.anger_total -= dt;
        buffs.anger_tick -= dt;
        if buffs.anger_tick <= 0.0 {
            qi.current += buffs.anger_per_tick as f32;
            buffs.anger_tick = buffs.anger_tick_every;
        }
        if buffs.anger_total <= 0.0 {
            buffs.anger_per_tick = 0;
            buffs.anger_tick_every = 0.0;
            buffs.anger_total = 0.0;
        }
    }

    if health.current <= 0.0 && !death.active {
        health.current = 0.0;
        if net.status != crate::plugins::network_plugin::NetStatus::InRoom {
            if let Some(model) = crate::plugins::character_plugin::weapon_model_stem(
                fighter.weapon,
                fighter.weapon_variant,
            ) {
                let forward = meteor_network::role_state::forward_from_yaw(motion.facing);
                death_weapon_drops.write(crate::plugins::render_plugin::OfflineWeaponDeathDrop {
                    model,
                    position: (transform.translation + Vec3::Y * (50.0 * 0.12)).to_array(),
                    velocity: [forward.x * 12.0, 0.0, forward.z * 12.0],
                });
            }
        }
        death.active = true;
        death.timer = RESPAWN_DELAY_SECONDS;
        sm_state.current = FighterState::Dead;
        combat.state = FighterState::Dead;
        pose.pose_id = 0;
        pose.frame = 0.0;
        info!("poison killed local player");
    }
}

#[cfg(test)]
#[path = "item_plugin/npc_pickup_tests.rs"]
mod npc_pickup_tests;

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pickup_effect_names_match_original_meteor_res_key14() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let resource =
            meteor_asset::res::parse_meteor_res(&std::fs::read(root.join("Meteor.res")).unwrap())
                .unwrap();
        let names = std::fs::read_to_string(root.join("SFX/effect.lst")).unwrap();
        let effects = names.lines().collect::<Vec<_>>();
        let mut checked = 0;
        for record in resource
            .records
            .iter()
            .filter(|r| matches!(r.category, 1 | 2) && r.name != "0")
        {
            let expected = record
                .prop_seed_value(14)
                .filter(|id| *id > 0)
                .map(|id| effects[id as usize].trim().trim_end_matches(".ef"));
            assert_eq!(
                pickup_effect_for_model(&record.name),
                expected,
                "{} ({},{})",
                record.name,
                record.category,
                record.id
            );
            checked += 1;
        }
        assert!(checked >= 80);
        assert_eq!(pickup_effect_for_model("Item0901"), Some("WeaponUP"));
        assert_eq!(pickup_effect_for_model("Item06"), None);
    }

    #[test]
    fn ws_sn04_host_claims_unlocked_bun_for_guest() {
        use crate::plugins::network_plugin::{
            control_test_channel, NetState, NetStatus, RemoteFighter,
        };
        use crate::plugins::stage_plugin::LocalFighter;
        use bevy::ecs::system::RunSystemOnce;
        let (tx, drain) = control_test_channel();
        let mut net = NetState {
            status: NetStatus::InRoom,
            player_id: "host".into(),
            round_active: true,
            ..Default::default()
        };
        for player_id in ["host", "guest"] {
            net.members.push(meteor_network::protocol::RoomMemberWire {
                player_id: player_id.into(),
                name: player_id.into(),
                character_id: 0,
                weapon: 6,
                team: Some(1),
                ready: true,
                connected: true,
            });
        }
        let mut app = App::new();
        app.insert_resource(net)
            .insert_resource(tx)
            .init_resource::<Time>()
            .init_resource::<PendingSceneItemClaims>();
        app.world_mut().spawn((
            LocalFighter,
            Transform::from_xyz(50.0, 0.0, 0.0),
            Health::new(),
        ));
        app.world_mut().spawn((
            RemoteFighter {
                player_id: "guest".into(),
            },
            Transform::default(),
            Health {
                current: 400.0,
                max: 1000.0,
            },
        ));
        let bun = app
            .world_mut()
            .spawn((
                StageProp {
                    name: "D_itRJug0101".into(),
                    model: "Item01".into(),
                    center_offset: Vec3::ZERO,
                    half_extents: Vec3::ONE,
                    hit_count: 0,
                    respawn_timer: 0.0,
                    interactive: false,
                    collision_enabled: false,
                    breakable: false,
                    broken: false,
                },
                Transform::default(),
                Visibility::Hidden,
            ))
            .id();
        app.world_mut()
            .run_system_once(network_scene_item_claim)
            .unwrap();
        assert!(drain().is_empty(), "unbroken host jug must not grant food");
        app.world_mut()
            .get_mut::<StageProp>(bun)
            .unwrap()
            .interactive = true;
        *app.world_mut().get_mut::<Visibility>(bun).unwrap() = Visibility::Visible;
        app.world_mut()
            .run_system_once(network_scene_item_claim)
            .unwrap();
        assert!(
            matches!(drain().as_slice(), [Message::ItemClaim { player_id, reference, model, .. }]
            if player_id == "guest" && reference == "D_itRJug0101" && model == "Item01")
        );
        app.world_mut()
            .run_system_once(network_scene_item_claim)
            .unwrap();
        assert!(drain().is_empty(), "pending claim suppresses duplicates");
    }

    #[test]
    fn network_successful_claims_queue_one_effect_per_item_not_per_render_batch() {
        use crate::plugins::effect_plugin::PickupEffectRequest;
        use bevy::ecs::system::RunSystemOnce;
        for stock in [false, true] {
            for (model, item_id, expected) in
                [("item01", 1, "GetItem"), ("item0901", 23, "WeaponUP")]
            {
                let mut app = App::new();
                app.init_resource::<ItemBuffs>()
                    .init_resource::<PendingSceneItemClaims>()
                    .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
                    .init_resource::<crate::plugins::network_plugin::NetState>();
                for _ in 0..2 {
                    app.world_mut().spawn((
                        StageProp {
                            name: "D_itTest".into(),
                            model: model.into(),
                            center_offset: Vec3::ZERO,
                            half_extents: Vec3::ONE,
                            hit_count: 0,
                            respawn_timer: 0.0,
                            interactive: true,
                            collision_enabled: false,
                            breakable: false,
                            broken: false,
                        },
                        Transform::from_xyz(5.0, 2.0, 7.0),
                        Visibility::Visible,
                    ));
                }
                if stock {
                    app.world_mut()
                        .resource_mut::<crate::plugins::network_plugin::NetState>()
                        .pending_stock_scene_items
                        .push(crate::plugins::network_plugin::StockSceneItemPickup {
                            actor_id: 1,
                            record_id: 11,
                            category: 2,
                            item_id,
                            position: Some([5.0, 2.0, 7.0]),
                        });
                    app.world_mut()
                        .run_system_once(apply_stock_scene_item_pickups)
                        .unwrap();
                    app.world_mut()
                        .run_system_once(apply_stock_scene_item_pickups)
                        .unwrap();
                } else {
                    app.world_mut()
                        .resource_mut::<crate::plugins::network_plugin::NetState>()
                        .pending_item_claims
                        .push(Message::ItemClaimed {
                            reference: "D_itTest".into(),
                            player_id: "remote".into(),
                            model: model.into(),
                            kind: "item".into(),
                            picked_weapon_model: None,
                            weapon: "sword".into(),
                            weapon_model: None,
                            weapon_slot: 0,
                            weapon_slots: vec![],
                            health: 1000.0,
                            max_health: 1000.0,
                            anger: 0.0,
                            respawn_seconds: Some(15.0),
                            item_effects: vec![],
                        });
                    app.world_mut()
                        .run_system_once(apply_network_item_claims)
                        .unwrap();
                    app.world_mut()
                        .run_system_once(apply_network_item_claims)
                        .unwrap();
                }
                let effects = app
                    .world_mut()
                    .query::<&PickupEffectRequest>()
                    .iter(app.world())
                    .collect::<Vec<_>>();
                assert_eq!(effects.len(), 1, "stock={stock}, model={model}");
                assert_eq!(effects[0].0.effect, expected);
                assert_eq!(effects[0].0.position, Vec3::new(5.0, 2.0, 7.0));
            }
        }
    }

    #[test]
    fn stock_scene_item_ids_resolve_through_original_meteor_res() {
        let names = stock_scene_item_names();
        assert_eq!(names.get(&(2, 1)).map(String::as_str), Some("item01"));
        assert_eq!(names.get(&(1, 6)).map(String::as_str), Some("w0_0"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_host_manifest_uses_resource_id_respawn_and_stage_geometry() {
        let prop = StageProp {
            name: "D_Item01".into(),
            model: "item01".into(),
            center_offset: Vec3::ZERO,
            half_extents: Vec3::new(0.5, 1.0, 1.5),
            hit_count: 0,
            respawn_timer: 0.0,
            interactive: true,
            collision_enabled: false,
            breakable: false,
            broken: false,
        };
        let transform = Transform::from_xyz(1.0, 2.0, 3.0);
        let seed = stock_authored_scene_item(
            &prop,
            &transform,
            &Visibility::Visible,
            &stock_scene_item_catalog(),
            None,
        )
        .expect("Item01 original resource entry");
        assert_eq!((seed.category, seed.item_id), (2, 1));
        assert_eq!(
            [seed.position.x, seed.position.y, seed.position.z],
            [1.0, 2.0, 3.0]
        );
        assert_eq!(seed.half_extents, [0.5, 1.0, 1.5]);
        assert_eq!(seed.respawn_ms, Some(120_000));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_host_manifest_keeps_hidden_box_contents_and_deduplicates_render_batches() {
        let (tx, drain) = crate::plugins::network_plugin::stock_scene_manifest_test_channel();
        let mut app = App::new();
        app.insert_resource(crate::plugins::network_plugin::NetState {
            status: crate::plugins::network_plugin::NetStatus::InRoom,
            address: "udp-host://127.0.0.1:5230".into(),
            player_id: "stock-1".into(),
            ..default()
        })
        .insert_resource(tx)
        .init_resource::<StockSceneManifestState>()
        .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
        .add_systems(Update, initialize_stock_scene_items);
        for (name, model, visible) in [
            ("D_wpBBox01", "w0_0", false),
            ("D_wpBBox01", "w0_0", false),
            ("D_itBBox02", "item01", false),
            ("D_wpBBox02", "W8_0r", false),
            ("D_it01", "item01", true),
        ] {
            app.world_mut().spawn((
                StageProp {
                    name: name.into(),
                    model: model.into(),
                    center_offset: Vec3::ZERO,
                    half_extents: Vec3::ONE,
                    hit_count: 0,
                    respawn_timer: 0.0,
                    interactive: visible,
                    collision_enabled: false,
                    breakable: false,
                    broken: false,
                },
                Transform::default(),
                if visible {
                    Visibility::Visible
                } else {
                    Visibility::Hidden
                },
            ));
        }
        app.update();
        let batches = drain();
        assert_eq!(batches.len(), 1);
        assert_eq!(
            batches[0].len(),
            4,
            "hidden box contents need one host record per DES instance"
        );
        assert_eq!(
            batches[0]
                .iter()
                .filter(|item| item.active && item.interactive)
                .count(),
            1
        );
        assert!(batches[0]
            .iter()
            .any(|item| item.category == 1 && item.item_id == 4));
        app.update();
        assert!(
            drain().is_empty(),
            "do not replace live pickups on every frame"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn stock_event29_snapshot_is_the_authoritative_authored_prop_set() {
        let mut app = App::new();
        app.insert_resource(ItemBuffs::default())
            .insert_resource(crate::plugins::network_plugin::NetState {
                status: crate::plugins::network_plugin::NetStatus::InRoom,
                address: "udp://127.0.0.1:5230".into(),
                pending_stock_scene_grounded: vec![
                    crate::plugins::network_plugin::StockSceneItemGrounded {
                        record_id: 1,
                        category: 2,
                        item_id: 1,
                        position: [1.0, 2.0, 3.0],
                        spawned: false,
                    },
                ],
                ..Default::default()
            })
            .init_resource::<StockSceneManifestState>()
            .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
            .add_systems(
                Update,
                (initialize_stock_scene_items, apply_stock_scene_item_pickups).chain(),
            );
        let spawn_prop = |app: &mut App, name: &str, model: &str, position: Vec3| {
            app.world_mut()
                .spawn((
                    StageProp {
                        name: name.into(),
                        model: model.into(),
                        center_offset: Vec3::ZERO,
                        half_extents: Vec3::ONE,
                        hit_count: 0,
                        respawn_timer: 0.0,
                        interactive: true,
                        collision_enabled: false,
                        breakable: false,
                        broken: false,
                    },
                    Transform::from_translation(position),
                    Visibility::Visible,
                ))
                .id()
        };
        let enabled = spawn_prop(&mut app, "D_Item01", "item01", Vec3::new(1.0, 2.0, 3.0));
        let absent = spawn_prop(&mut app, "D_Item04", "item04", Vec3::new(8.0, 2.0, 3.0));

        app.update();

        assert!(app.world().get::<StageProp>(enabled).unwrap().interactive);
        assert_eq!(
            *app.world().get::<Visibility>(enabled).unwrap(),
            Visibility::Visible
        );
        assert!(!app.world().get::<StageProp>(absent).unwrap().interactive);
        assert_eq!(
            *app.world().get::<Visibility>(absent).unwrap(),
            Visibility::Hidden
        );
    }

    #[test]
    fn stock_event32_hides_matching_prop_and_applies_local_item() {
        let mut app = App::new();
        app.insert_resource(ItemBuffs::default())
            .insert_resource(crate::plugins::network_plugin::NetState {
                status: crate::plugins::network_plugin::NetStatus::InRoom,
                address: "udp://127.0.0.1:5230".into(),
                player_id: "stock-7".into(),
                pending_stock_scene_items: vec![
                    crate::plugins::network_plugin::StockSceneItemPickup {
                        actor_id: 7,
                        record_id: 44,
                        category: 2,
                        item_id: 1,
                        position: Some([1.0, 2.0, 3.0]),
                    },
                ],
                ..Default::default()
            })
            .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
            .add_systems(Update, apply_stock_scene_item_pickups);
        let player = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 6,
                    weapon_slots: [6, 0],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health {
                    current: 1_000.0,
                    max: 2_000.0,
                },
                Qi::default(),
                Transform::default(),
                crate::plugins::stage_plugin::LocalFighter,
            ))
            .id();
        let prop = app
            .world_mut()
            .spawn((
                StageProp {
                    name: "D_Item01".into(),
                    model: "item01".into(),
                    center_offset: Vec3::ZERO,
                    half_extents: Vec3::ONE,
                    hit_count: 0,
                    respawn_timer: 0.0,
                    interactive: true,
                    collision_enabled: false,
                    breakable: false,
                    broken: false,
                },
                Transform::from_xyz(1.0, 2.0, 3.0),
                Visibility::Visible,
            ))
            .id();

        app.update();

        assert_eq!(app.world().get::<Health>(player).unwrap().current, 1_300.0);
        assert!(!app.world().get::<StageProp>(prop).unwrap().interactive);
        assert_eq!(
            *app.world().get::<Visibility>(prop).unwrap(),
            Visibility::Hidden
        );
        assert!(app
            .world()
            .resource::<crate::plugins::network_plugin::NetState>()
            .pending_stock_scene_items
            .is_empty());
    }

    #[test]
    fn stock_runtime_event31_queues_dynamic_weapon_actor() {
        let mut app = App::new();
        app.insert_resource(ItemBuffs::default())
            .insert_resource(crate::plugins::network_plugin::NetState {
                status: crate::plugins::network_plugin::NetStatus::InRoom,
                address: "udp://127.0.0.1:5230".into(),
                pending_stock_scene_grounded: vec![
                    crate::plugins::network_plugin::StockSceneItemGrounded {
                        record_id: 91,
                        category: 1,
                        item_id: 6,
                        position: [4.0, 5.0, 6.0],
                        spawned: true,
                    },
                ],
                ..Default::default()
            })
            .init_resource::<crate::plugins::stage_plugin::StageScriptRuntime>()
            .add_systems(Update, apply_stock_scene_item_pickups);

        app.update();

        let net = app
            .world()
            .resource::<crate::plugins::network_plugin::NetState>();
        let drop = net.dynamic_weapon_drops.get("D_wpStock91").unwrap();
        assert_eq!(drop.model, "w0_0");
        assert_eq!(drop.position, [4.0, 5.0, 6.0]);
        assert!(drop.settled);
        assert_eq!(net.pending_weapon_drops.len(), 1);
        assert_eq!(net.pending_weapon_drops[0].reference, drop.reference);
    }

    #[test]
    fn prop_hit_stopwatch_is_strict_and_owned_per_attacker() {
        let mut world = World::new();
        let player = world.spawn_empty().id();
        let npc = world.spawn_empty().id();
        let mut gate = PropHitGate::default();
        assert!(gate.accepts(player, 1.0));
        assert!(gate.accepts(npc, 1.0));
        gate.reset(player, 0.0);
        assert!(!gate.accepts(player, 0.2), "exactly 0.2s is still rejected");
        assert!(gate.accepts(player, 0.200_001));
        assert!(gate.accepts(npc, 1.0), "player hit cannot throttle NPC");
        gate.reset(npc, 1.1);
        gate.clear();
        assert!(gate.accepts(player, 1.1));
        assert!(gate.accepts(npc, 1.1));
    }

    #[test]
    fn item_effect_table_subset() {
        assert_eq!(item_effect_for("item01").unwrap().1.hp, 300);
        assert_eq!(item_effect_for("item0101").unwrap().1.hp, 500);
        assert_eq!(item_effect_for("item15").unwrap().1.hp, 50);

        assert_eq!(item_effect_for("item08").unwrap().1.anger, 100);
        assert_eq!(item_effect_for("item16").unwrap().1.anger, 1);

        assert_eq!(item_effect_for("item04").unwrap().1.attack, 25);
        assert_eq!(item_effect_for("item03").unwrap().1.defense, 20);

        assert_eq!(item_effect_for("item02").unwrap().1.max_hp, 500);
        assert_eq!(item_effect_for("item10").unwrap().1.max_hp, 6999);
        assert_eq!(item_effect_for("item05").unwrap().1.speed, 1.5);
        assert_eq!(item_effect_for("item07").unwrap().1.duration, 60.0);
        assert_eq!(item_effect_for("item0103").unwrap().1.poison_per_tick, 10);
        assert_eq!(item_effect_for("hot").unwrap().1.anger_per_tick, 1);

        assert_eq!(item_effect_for("item09").unwrap().1.weapon_variant, 0);
        assert_eq!(item_effect_for("item0901").unwrap().1.weapon_variant, 1);
        assert_eq!(item_effect_for("item0904").unwrap().1.weapon_variant, 4);

        assert_eq!(
            item_effect_for("item02").unwrap().1.worn,
            Some(("MaxLifUP", 600.0))
        );
        assert_eq!(
            item_effect_for("item0103").unwrap().1.worn,
            Some(("AttrPOIS", 30.0))
        );
        assert_eq!(
            item_effect_for("hot").unwrap().1.worn,
            Some(("HOT", 9999.9))
        );
        for (model, effect) in [
            ("item12", "Item12"),
            ("item13", "Item13"),
            ("item14", "Item14"),
            ("item15", "Item15"),
            ("item16", "Item16"),
        ] {
            assert_eq!(
                item_effect_for(model).unwrap().1.worn,
                Some((effect, 9999.9)),
                "{model} keeps its Web key15 equipment visual"
            );
        }
        assert_eq!(item_effect_for("item01").unwrap().1.worn, None);

        assert!(item_effect_for("stone").is_none());
        assert!(item_effect_for("").is_none());
    }

    fn fighter(weapon: u8, weapon_slots: [u8; 2], weapon_variant: u8) -> Fighter {
        Fighter {
            character_id: 0,
            player_slot: 0,
            weapon,
            weapon_slots,
            weapon_slot: 0,
            weapon_slot_variants: [weapon_variant, 0],
            weapon_variant,
        }
    }

    #[test]
    fn offline_weapon_drop_clears_spare_and_preserves_active_slot_and_variant() {
        let mut armed = fighter(6, [6, 5], 3);
        armed.weapon_slot_variants[1] = 2;
        assert_eq!(drop_spare_weapon(&mut armed), Some((5, 2)));
        assert_eq!(armed.weapon_slots, [6, 0]);
        assert_eq!(armed.weapon_slot, 0);
        assert_eq!(armed.weapon_slot_variants, [3, 0]);
        assert_eq!(armed.weapon, 6);
        assert_eq!(armed.weapon_variant, 3);

        let mut switched = fighter(5, [6, 5], 2);
        switched.weapon_slot = 1;
        switched.weapon_slot_variants = [3, 2];
        assert_eq!(drop_spare_weapon(&mut switched), Some((6, 3)));
        assert_eq!(switched.weapon_slots, [0, 5]);
        assert_eq!(
            (
                switched.weapon,
                switched.weapon_variant,
                switched.weapon_slot
            ),
            (5, 2, 1)
        );

        let mut no_spare = fighter(6, [6, 0], 0);
        assert_eq!(drop_spare_weapon(&mut no_spare), None);
        assert_eq!(no_spare.weapon_slots, [6, 0]);
        assert_eq!(no_spare.weapon, 6);
    }

    #[test]
    fn weapon_pickup_uses_original_model_and_slot_rules() {
        assert_eq!(weapon_from_model("W0_3"), Some((6, 3)));
        assert_eq!(weapon_from_model("w10_0r"), Some((11, 0)));
        assert_eq!(weapon_from_model("item01"), None);
        assert_eq!(weapon_respawn_seconds("w0_0"), Some(30.0));
        assert_eq!(weapon_respawn_seconds("w1_0"), Some(45.0));
        assert_eq!(weapon_respawn_seconds("w11_0"), Some(45.0));
        assert_eq!(weapon_respawn_seconds("w11_1"), Some(30.0));

        let mut with_empty_spare = fighter(6, [6, 0], 0);
        assert_eq!(store_picked_weapon(&mut with_empty_spare, 5, 0), Some(1));
        assert_eq!(with_empty_spare.weapon_slots, [6, 5]);
        assert_eq!(with_empty_spare.weapon, 6);

        assert_eq!(store_picked_weapon(&mut with_empty_spare, 5, 0), None);
        assert_eq!(with_empty_spare.weapon_slots, [6, 5]);

        let mut same_class_other_variant = fighter(6, [6, 0], 0);
        assert_eq!(
            store_picked_weapon(&mut same_class_other_variant, 6, 2),
            Some(1)
        );
        assert_eq!(same_class_other_variant.weapon_slots, [6, 6]);
        assert_eq!(same_class_other_variant.weapon_slot_variants, [0, 2]);
    }

    #[test]
    fn authoritative_active_slot_switch_reuses_preheated_inventory() {
        let mut switched = fighter(6, [6, 5], 0);
        let old_slots = switched.weapon_slots;
        let old_variants = switched.weapon_slot_variants;
        switched.weapon_slot = 1;
        switched.weapon = 5;
        switched.weapon_variant = 0;
        assert!(!weapon_inventory_changed(
            old_slots,
            old_variants,
            &switched
        ));

        switched.weapon_slot_variants[1] = 2;
        assert!(weapon_inventory_changed(old_slots, old_variants, &switched));
    }

    #[test]
    fn pickup_distance_uses_the_web_vertical_capsule() {
        let feet = Vec3::new(2.0, 1.0, -3.0);
        assert!(within_pickup_range(
            feet + Vec3::new(PICKUP_RANGE - 0.001, 0.9, 0.0),
            feet
        ));
        assert!(within_pickup_range(
            feet + Vec3::new(0.0, PICKUP_CAPSULE_HEIGHT, PICKUP_RANGE - 0.001),
            feet
        ));
        assert!(!within_pickup_range(
            feet + Vec3::new(PICKUP_RANGE + 0.01, 0.9, 0.0),
            feet
        ));
        assert!(!within_pickup_range(
            feet + Vec3::new(0.0, PICKUP_CAPSULE_HEIGHT + PICKUP_RANGE + 0.01, 0.0),
            feet
        ));
    }

    #[test]
    fn pickup_distance_uses_the_authored_geometry_center_not_model_origin() {
        let prop = StageProp {
            name: "D_wp01".into(),
            model: "w0_0".into(),
            center_offset: Vec3::new(0.0, 0.0, 3.0),
            half_extents: Vec3::ONE,
            hit_count: 0,
            respawn_timer: 0.0,
            interactive: true,
            collision_enabled: false,
            breakable: false,
            broken: false,
        };
        let transform = Transform::from_translation(Vec3::new(0.0, 0.0, 2.5))
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let fighter = Vec3::new(3.0, 0.0, 2.5);
        assert!(
            within_pickup_range(prop.world_center(&transform), fighter),
            "rotated authored center overlaps the fighter capsule"
        );
        assert!(
            !within_pickup_range(transform.translation, fighter),
            "the old origin-only query can miss while the visible geometry overlaps"
        );
    }

    #[test]
    fn respawn_seconds_present() {
        for model in [
            "item01", "item0101", "item0102", "item15", "item08", "item11", "item16", "item03",
            "item12", "item04", "item13",
        ] {
            let (respawn, _) = item_effect_for(model).unwrap();
            assert!(respawn > 0.0, "{model} respawn must be positive");
        }
    }

    #[test]
    fn network_pickup_accepts_only_authored_item_or_weapon_models() {
        assert!((PICKUP_RANGE - 1.92).abs() < f32::EPSILON);
        assert!(network_pickable_model("item15"));
        assert!(network_pickable_model("W0_0"));
        assert!(!network_pickable_model("w12_4l"));
        assert!(network_pickable_model("w6_5"));
        assert!(!network_pickable_model("stone"));
        assert!(!network_pickable_model("w0_5"));
        assert!(!network_pickable_model("weapon_sword"));
    }

    #[test]
    fn authoritative_wire_slots_and_effects_rebuild_local_projection() {
        let slots = weapon_slots_from_wire(&[
            ItemWeaponSlotWire {
                index: 0,
                present: true,
                weapon: "sword".into(),
                weapon_id: Some(6),
                model: Some("W0_0".into()),
                resource_class: 0,
                variant: 0,
                attack_power: 110.0,
                defense_power: 15.0,
            },
            ItemWeaponSlotWire {
                index: 1,
                present: true,
                weapon: "blade".into(),
                weapon_id: Some(8),
                model: Some("W2_3".into()),
                resource_class: 2,
                variant: 3,
                attack_power: 90.0,
                defense_power: 7.0,
            },
        ]);
        assert_eq!(slots, ([6, 8], [0, 3]));
        assert_eq!(weapon_variant_from_model(Some("W2_3")), Some(3));

        let effects = vec![
            ItemEffectWire {
                model: "item04".into(),
                remaining: 56.0,
            },
            ItemEffectWire {
                model: "item03".into(),
                remaining: 84.0,
            },
            ItemEffectWire {
                model: "item02".into(),
                remaining: 512.0,
            },
        ];
        let mut buffs = ItemBuffs::default();
        let mut worn = crate::components::fighter::WornEffect::default();
        let mut concealment = crate::components::fighter::Concealment::default();
        sync_network_item_effects(&mut buffs, &mut worn, &mut concealment, &effects);
        assert_eq!(buffs.attack_bonus, 25);
        assert_eq!(buffs.defense_bonus, 20);
        assert_eq!(buffs.max_hp_bonus, 500);
        assert_eq!(worn.effects.len(), 3);
        assert!(worn.effects.iter().any(|effect| {
            effect.name == "MaxLifUP" && (effect.remaining - 512.0).abs() < f32::EPSILON
        }));
        assert!(worn.effects.iter().any(|effect| {
            effect.name == "PowUP" && (effect.remaining - 56.0).abs() < f32::EPSILON
        }));
        assert!(worn.effects.iter().any(|effect| {
            effect.name == "DefUP" && (effect.remaining - 84.0).abs() < f32::EPSILON
        }));
        assert_eq!(concealment.remaining, 0.0);
    }

    #[test]
    fn network_item07_projects_character_only_concealment() {
        let effects = [ItemEffectWire {
            model: "item07".into(),
            remaining: 42.5,
        }];
        let mut buffs = ItemBuffs::default();
        let mut worn = crate::components::fighter::WornEffect::default();
        let mut concealment = crate::components::fighter::Concealment::default();
        sync_network_item_effects(&mut buffs, &mut worn, &mut concealment, &effects);
        assert_eq!(concealment.remaining, 42.5);
        assert!(worn.effects.is_empty());
    }

    #[test]
    fn christmas_equipment_worn_effects_coexist_and_refresh_independently() {
        let effects = [
            ItemEffectWire {
                model: "item12".into(),
                remaining: 901.0,
            },
            ItemEffectWire {
                model: "item13".into(),
                remaining: 902.0,
            },
            ItemEffectWire {
                model: "item14".into(),
                remaining: 903.0,
            },
            ItemEffectWire {
                model: "item15".into(),
                remaining: 904.0,
            },
            ItemEffectWire {
                model: "item16".into(),
                remaining: 905.0,
            },
        ];
        let mut worn = crate::components::fighter::WornEffect::default();
        let mut concealment = crate::components::fighter::Concealment::default();
        sync_network_visual_effects(&mut worn, &mut concealment, &effects);

        assert_eq!(worn.effects.len(), 5);
        for (index, name) in ["Item12", "Item13", "Item14", "Item15", "Item16"]
            .into_iter()
            .enumerate()
        {
            assert!(worn.effects.iter().any(|effect| {
                effect.name == name
                    && (effect.remaining - (901.0 + index as f32)).abs() < f32::EPSILON
            }));
        }

        worn.refresh("Item15", 9999.9);
        assert_eq!(
            worn.effects.len(),
            5,
            "re-pick refreshes instead of stacking"
        );
        assert!(worn.effects.iter().any(|effect| {
            effect.name == "Item15" && (effect.remaining - 9999.9).abs() < f32::EPSILON
        }));
    }
}
