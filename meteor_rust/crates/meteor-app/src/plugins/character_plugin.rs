use crate::components::fighter::{EntryBlend, PoseRuntime};
use crate::plugins::pak_textures::{
    image_from_decoded, image_handle, pak_read_ci, resolve_pak_texture, resolve_runtime_texture,
};
use crate::plugins::stage_material::{GmdlMaterial, GmdlMaterialParams, GmdlMode};
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::{
    mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    mesh::{Indices, VertexAttributeValues},
    render_resource::PrimitiveTopology,
};
use meteor_anim::skeleton::Skeleton;
use meteor_asset::amb::{parse_amb, AmbAnimation};
use meteor_asset::bnc::parse_bones;
use meteor_asset::pak::PakArchive;
use meteor_asset::pos::parse_poses;
use meteor_asset::skc::parse_skin;
use std::path::PathBuf;

pub fn apply_color_variant(name: &str, color_index: u8) -> String {
    let Some(i) = name.find("b0") else {
        return name.to_string();
    };
    let digit_pos = i + 2;
    if digit_pos >= name.len() || !name.as_bytes()[digit_pos].is_ascii_digit() {
        return name.to_string();
    }
    let mut out = name.to_string();
    out.replace_range(digit_pos..digit_pos + 1, &(color_index + 1).to_string());
    out
}

fn color_index_for_team(team: Option<u32>) -> u8 {
    match team {
        Some(1) => 1,
        Some(2) => 2,
        _ => 0,
    }
}

fn network_skin_color_index(
    net: &crate::plugins::network_plugin::NetState,
    local: bool,
    remote: Option<&crate::plugins::network_plugin::RemoteFighter>,
) -> u8 {
    if net.status != crate::plugins::network_plugin::NetStatus::InRoom
        || !meteor_network::modes::is_team_mode(&net.mode)
    {
        return 0;
    }
    let player_id = if local {
        Some(net.player_id.as_str())
    } else {
        remote.map(|remote| remote.player_id.as_str())
    };
    color_index_for_team(
        player_id
            .and_then(|player_id| {
                net.members
                    .iter()
                    .find(|member| member.player_id == player_id)
            })
            .and_then(|member| member.team),
    )
}

pub struct CharacterPlugin;

#[cfg(test)]
#[path = "character_plugin/crowd_tests.rs"]
mod crowd_tests;

#[derive(Component)]
pub(crate) struct FighterModel {
    pub(crate) fighter: Entity,

    pub(crate) character: String,
}

#[derive(Component)]
struct FighterSkin {
    fighter: Entity,
    base_alpha_cutoff: f32,
    concealed: bool,
}

#[derive(Component)]
pub(crate) struct PendingFighterSkinReveal;

#[derive(Component)]
pub(crate) struct FighterPoseSampled;

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CharacterLoadingState<'w, 's> {
    fighters: Query<'w, 's, Entity, With<crate::components::fighter::Fighter>>,
    models: Query<'w, 's, (&'static FighterModel, Has<FighterPoseSampled>)>,
    pending: Query<'w, 's, (), With<PendingFighterSkinReveal>>,
    common: Option<Res<'w, CommonAmb>>,
    assets: Option<Res<'w, CharacterAssetsSet>>,
    failures: Option<Res<'w, CharacterLoadFailures>>,
}

impl CharacterLoadingState<'_, '_> {
    pub fn ready(&self) -> bool {
        self.common.is_some()
            && !self.fighters.is_empty()
            && self.pending.is_empty()
            && self.fighters.iter().all(|fighter| {
                self.models
                    .iter()
                    .any(|(model, sampled)| model.fighter == fighter && sampled)
            })
    }

    pub fn error(&self) -> Option<String> {
        if self.assets.is_some() && self.common.is_none() {
            return Some("公共动作库 Common/character.amb 加载失败".into());
        }
        if let Some(failures) = &self.failures {
            if !failures.0.is_empty() {
                return Some("人物模型或动作资源加载失败，请返回后重试".into());
            }
        }
        None
    }
}

#[derive(Component, Debug, Clone)]
pub(crate) struct WeaponEndpoint {
    pub(crate) name: String,
    pub(crate) fighter: Entity,
    pub(crate) weapon: u8,
    pub(crate) slot: u8,
    pub(crate) variant: u8,
    pub(crate) part: String,
    pub(crate) left: bool,
    pub(crate) forms: Option<Vec<u8>>,
}

#[derive(Component)]
#[relationship(relationship_target = FighterModelBones)]
pub(crate) struct FighterBone {
    #[relationship]
    pub(crate) model: Entity,

    pub(crate) name: String,
    pub(crate) skin_index: usize,
    #[allow(dead_code)]
    node_index: usize,
}

#[derive(Component)]
#[relationship(relationship_target = FighterModelDummies)]
pub(crate) struct FighterDummy {
    #[relationship]
    pub(crate) model: Entity,

    pub(crate) name: String,

    pub(crate) dummy_index: usize,
}

#[derive(Component, Default)]
#[relationship_target(relationship = FighterBone)]
pub(crate) struct FighterModelBones(Vec<Entity>);

#[derive(Component, Default)]
#[relationship_target(relationship = FighterDummy)]
pub(crate) struct FighterModelDummies(Vec<Entity>);

pub(crate) type FighterSkeletonQuery<'w, 's> = Query<
    'w,
    's,
    (
        Option<&'static FighterModelBones>,
        Option<&'static FighterModelDummies>,
    ),
>;

pub(crate) fn model_bones<'a>(model: Entity, skeletons: &'a FighterSkeletonQuery) -> &'a [Entity] {
    skeletons
        .get(model)
        .ok()
        .and_then(|(bones, _)| bones)
        .map_or(&[], |bones| &bones.0)
}

pub(crate) fn model_dummies<'a>(
    model: Entity,
    skeletons: &'a FighterSkeletonQuery,
) -> &'a [Entity] {
    skeletons
        .get(model)
        .ok()
        .and_then(|(_, dummies)| dummies)
        .map_or(&[], |dummies| &dummies.0)
}

#[derive(Resource, Default)]
pub(crate) struct CharacterAssetsSet(pub(crate) std::collections::HashMap<String, CharacterAssets>);

#[derive(Resource, Default)]
struct LocalVisualDiagnostics {
    logged: bool,
}

#[derive(Resource, Default)]
struct CharacterLoadFailures(std::collections::HashSet<String>);

#[derive(Resource)]
pub(crate) struct CommonAmb(pub(crate) AmbAnimation);

pub(crate) const MOTION_ROOT_DUMMY: &str = "d_base";

pub(crate) struct CharacterAssets {
    pub(crate) amb: AmbAnimation,
    pub(crate) skin: meteor_asset::skc::SkinMesh,
    pub(crate) skeleton: Skeleton,

    pub(crate) dummy_order: std::collections::HashMap<String, usize>,

    pub(crate) pose_frames: std::collections::BTreeMap<u16, (u32, u32)>,

    pub(crate) poses: std::collections::BTreeMap<u16, meteor_asset::pos::Pose>,

    pub(crate) ptexture: Option<PakArchive>,

    pub(crate) ctexture: Option<PakArchive>,

    pub(crate) texture_patch: Option<PakArchive>,

    pub(crate) act: meteor_asset::act::ActData,

    pub(crate) bone_local_bounds: Vec<Option<meteor_combat::hitbox::BoneLocalBox>>,
}

impl CharacterAssets {
    pub(crate) fn amb_for_pose<'a>(
        &'a self,
        common: &'a AmbAnimation,
        pose_id: u16,
    ) -> &'a AmbAnimation {
        match self.poses.get(&pose_id).map(|pose| pose.source) {
            Some(1) => &self.amb,
            _ => common,
        }
    }

    pub(crate) fn motion_root_at(
        &self,
        common: &AmbAnimation,
        pose_id: u16,
        frame: f32,
    ) -> [f32; 3] {
        self.amb_for_pose(common, pose_id)
            .sample_position_at(frame, self.dummy_order.get(MOTION_ROOT_DUMMY).copied())
    }

    pub(crate) fn authored_root_step(
        &self,
        common: &AmbAnimation,
        pose: &crate::components::fighter::PoseRuntime,
        dt: f32,
    ) -> [f32; 3] {
        let frame = pose.frame.floor();
        if dt <= 0.0 || !dt.is_finite() || !frame.is_finite() || frame >= pose.end as f32 {
            return [0.0; 3];
        }
        let a = self.motion_root_at(common, pose.pose_id, frame);
        let b = self.motion_root_at(common, pose.pose_id, frame + 1.0);
        std::array::from_fn(|i| (b[i] - a[i]) * (30.0 * dt))
    }

    pub(crate) fn apply_pose(
        &self,
        pose: &mut crate::components::fighter::PoseRuntime,
        pose_id: u16,
        hard: bool,
    ) {
        self.apply_pose_with_blend(
            pose,
            &mut crate::components::fighter::EntryBlend::default(),
            pose_id,
            hard,
            None,
            crate::components::fighter::EntryBlendKind::Generic,
            false,
            false,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_pose_with_blend(
        &self,
        pose: &mut crate::components::fighter::PoseRuntime,
        blend: &mut crate::components::fighter::EntryBlend,
        pose_id: u16,
        hard: bool,
        pending_blend_seconds: Option<f32>,
        kind: crate::components::fighter::EntryBlendKind,
        skip_blend: bool,
        force_blend: bool,
        strategy_id: Option<u8>,
    ) {
        apply_pose_transition(
            pose,
            blend,
            &self.pose_frames,
            &self.poses,
            pose_id,
            hard,
            pending_blend_seconds,
            kind,
            skip_blend,
            force_blend,
            strategy_id,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_pose_transition(
    pose: &mut crate::components::fighter::PoseRuntime,
    blend: &mut crate::components::fighter::EntryBlend,
    pose_frames: &std::collections::BTreeMap<u16, (u32, u32)>,
    poses: &std::collections::BTreeMap<u16, meteor_asset::pos::Pose>,
    pose_id: u16,
    hard: bool,
    pending_blend_seconds: Option<f32>,
    kind: crate::components::fighter::EntryBlendKind,
    skip_blend: bool,
    force_blend: bool,
    strategy_id: Option<u8>,
) {
    let Some(&(start, end)) = pose_frames.get(&pose_id) else {
        return;
    };
    let (loop_start, loop_end, loops) = match poses.get(&pose_id) {
        Some(def) if def.loops && !def.link.is_some_and(|id| id > 0 && poses.contains_key(&id)) => {
            (def.loop_start, def.loop_end, true)
        }
        _ => (start, end, false),
    };

    let previous_id = pose.pose_id;
    let pose_changed = previous_id != pose_id;
    pose.pose_id = pose_id;
    pose.start = start;
    pose.end = end;
    pose.loop_start = loop_start;
    pose.loop_end = loop_end;
    pose.loops = loops;
    if hard || pose_changed {
        pose.pose_instance_id = pose.pose_instance_id.wrapping_add(1);
    }
    if hard {
        pose.frame = start as f32;
    } else {
        let source_start = if pose_changed {
            pose_frames
                .get(&previous_id)
                .map(|(source_start, _)| *source_start)
                .unwrap_or(start)
        } else {
            start
        };
        let cursor = pose.frame - source_start as f32;
        pose.frame = (start as f32 + cursor).clamp(start as f32, end as f32);
    }

    let trigger = hard && !skip_blend;
    if !trigger && !force_blend {
        blend.duration = 0.0;
        blend.elapsed = 0.0;
        blend.started_at_seconds = None;
        return;
    }
    blend.duration = entry_blend_duration(pending_blend_seconds, strategy_id, pose_id);
    blend.elapsed = 0.0;
    blend.started_at_seconds = None;
    blend.pose_id = pose_id;
    blend.to_frame = if kind == crate::components::fighter::EntryBlendKind::Chained {
        poses
            .get(&pose_id)
            .map(|def| def.action_entry_frame)
            .unwrap_or(start) as f32
    } else {
        start as f32
    };
    pose.frame = blend.to_frame.clamp(start as f32, end as f32);

    blend.kind = kind;
}

pub(crate) fn entry_blend_duration(
    pending_blend_seconds: Option<f32>,
    strategy_id: Option<u8>,
    pose_id: u16,
) -> f32 {
    let authored = pending_blend_seconds
        .filter(|t| t.is_finite() && *t > 0.0)
        .map(|t| t.min(0.6));
    let fast =
        strategy_id.and_then(|sid| meteor_physics::locomotion::fast_blend_seconds(sid, pose_id));
    authored.or(fast).unwrap_or(0.12)
}

fn entry_blend_wall_elapsed(started_at_seconds: Option<f64>, now_seconds: f64) -> f32 {
    started_at_seconds
        .map(|started| (now_seconds - started).max(0.0) as f32)
        .unwrap_or(0.0)
}

fn blend_vec3(from: [f32; 3], target: [f32; 3], t: f32) -> [f32; 3] {
    [
        from[0] + (target[0] - from[0]) * t,
        from[1] + (target[1] - from[1]) * t,
        from[2] + (target[2] - from[2]) * t,
    ]
}

fn blend_quat(from: [f32; 4], target: Quat, t: f32) -> Quat {
    Quat::from_xyzw(from[0], from[1], from[2], from[3]).slerp(target, t)
}

#[allow(dead_code)]
const ACTIVE_CHARACTERS: [&str; 2] = ["p0", "p1"];

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        let actor_schedule = super::fighter_frame::schedule(app);
        let serial = super::fighter_frame::installed(app);
        app.add_systems(
            Update,
            drive_fighter_animation
                .after(crate::plugins::FighterFrameSet::PoseResolve)
                .run_if(in_state(crate::app_state::AppState::Playing))
                .run_if(
                    |loading: Option<Res<super::loading_ui::LoadingPresentation>>| {
                        loading.is_some_and(|loading| loading.active)
                    },
                ),
        );
        app.init_resource::<CharacterLoadFailures>()
            .init_resource::<LocalVisualDiagnostics>()
            .add_systems(
                OnEnter(crate::app_state::AppState::Playing),
                (
                    clear_character_load_failures,
                    reset_local_visual_diagnostics,
                    load_character_assets,
                )
                    .chain(),
            )
            .add_systems(
                OnExit(crate::app_state::AppState::Playing),
                despawn_fighter_models,
            )
            .add_systems(
                Update,
                (
                    reveal_ready_fighter_skins,
                    refresh_character_variant,
                    spawn_fighter_model,
                    rebuild_weapon_inventory,
                )
                    .chain()
                    .in_set(if serial {
                        crate::plugins::FighterFrameSet::PoseDecision
                    } else {
                        crate::plugins::FighterFrameSet::SkeletonSample
                    })
                    .run_if(in_state(crate::app_state::AppState::Playing)),
            )
            .add_systems(
                actor_schedule,
                (
                    drive_fighter_animation
                        .after(rebuild_weapon_inventory)
                        .after(crate::plugins::movement_plugin::hold_airborne_fall_pose),
                    head_track,
                    sync_weapon_form_visibility,
                    sync_skin_concealment,
                )
                    .chain()
                    .in_set(crate::plugins::FighterFrameSet::SkeletonSample)
                    .run_if(in_state(crate::app_state::AppState::Playing)),
            )
            .add_systems(
                actor_schedule,
                resample_late_air_pose_visual
                    .in_set(crate::plugins::FighterFrameSet::PoseResolve)
                    .after(crate::plugins::combat_plugin::CombatPoseResolveSet)
                    .run_if(in_state(crate::app_state::AppState::Playing)),
            )
            .add_systems(
                PostUpdate,
                log_local_visual_alignment
                    .after(bevy::transform::TransformSystems::Propagate)
                    .run_if(in_state(crate::app_state::AppState::Playing)),
            );
    }
}

fn reset_local_visual_diagnostics(mut diagnostics: ResMut<LocalVisualDiagnostics>) {
    diagnostics.logged = false;
}

fn log_local_visual_alignment(
    mut diagnostics: ResMut<LocalVisualDiagnostics>,
    fighters: Query<(Entity, &GlobalTransform), With<crate::plugins::stage_plugin::LocalFighter>>,
    models: Query<(Entity, &FighterModel, &GlobalTransform)>,
    bones: Query<(&FighterBone, &GlobalTransform)>,
    cameras: Query<&GlobalTransform, With<crate::plugins::camera_plugin::GameplayCamera>>,
) {
    if diagnostics.logged {
        return;
    }
    let Ok((fighter_entity, fighter_transform)) = fighters.single() else {
        return;
    };
    let Some((model_entity, _, model_transform)) = models
        .iter()
        .find(|(_, model, _)| model.fighter == fighter_entity)
    else {
        return;
    };
    let head = bones
        .iter()
        .find(|(bone, _)| bone.model == model_entity && bone.name.eq_ignore_ascii_case("bau_Head"))
        .map(|(_, transform)| transform.translation());
    let fighter = fighter_transform.translation();
    let model = model_transform.translation();
    let camera = cameras.single().ok().map(GlobalTransform::translation);
    info!(
        "local visual alignment: fighter={fighter:?} model={model:?} head={head:?} camera={camera:?} model_offset={:.4}",
        model.distance(fighter),
    );
    diagnostics.logged = true;
}

fn sync_weapon_form_visibility(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    fighters: Query<(
        &crate::components::fighter::Fighter,
        &crate::components::fighter::WeaponForm,
        &crate::components::fighter::PoseRuntime,
    )>,
    projectiles: Query<&crate::components::projectile::Projectile>,
    mut parts: Query<(&crate::components::fighter::WeaponModel, &mut Visibility)>,
) {
    let flying_wheels: std::collections::HashSet<Entity> = projectiles
        .iter()
        .filter(|projectile| {
            projectile.kind == crate::components::projectile::ProjectileKind::FlyingWheel
        })
        .map(|projectile| projectile.source.owner)
        .collect();
    for (part, mut visibility) in &mut parts {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), part.fighter) {
            continue;
        }
        let Ok((fighter, form, pose)) = fighters.get(part.fighter) else {
            continue;
        };
        *visibility = if weapon_part_is_visible(
            fighter,
            meteor_combat::combo_engine::weapon_visual_form(
                fighter.weapon,
                form.current,
                pose.pose_id,
            ),
            part,
            flying_wheels.contains(&part.fighter),
        ) {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

fn weapon_part_is_visible(
    fighter: &crate::components::fighter::Fighter,
    form: u8,
    part: &crate::components::fighter::WeaponModel,
    flying_wheel: bool,
) -> bool {
    fighter.weapon_slot == part.slot
        && fighter.weapon == part.weapon
        && fighter.weapon_variant == part.variant
        && part
            .forms
            .as_ref()
            .is_none_or(|forms| forms.contains(&form))
        && !(flying_wheel && part.weapon == 2)
}

fn weapon_marker_matches_inventory(
    fighter: &crate::components::fighter::Fighter,
    marker: &crate::components::fighter::WeaponModel,
) -> bool {
    let slot = usize::from(marker.slot);
    slot < fighter.weapon_slots.len()
        && fighter.weapon_slots[slot] != 0
        && fighter.weapon_slots[slot] == marker.weapon
        && fighter.weapon_slot_variants[slot] == marker.variant
}

fn sync_skin_concealment(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    fighters: Query<&crate::components::fighter::Concealment>,
    mut skins: Query<(&mut FighterSkin, &MeshMaterial3d<GmdlMaterial>)>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    for (mut skin, handle) in &mut skins {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), skin.fighter) {
            continue;
        }
        let concealed = fighters
            .get(skin.fighter)
            .is_ok_and(|state| state.remaining > 0.0);
        if concealed == skin.concealed {
            continue;
        }
        let Some(mut material) = materials.get_mut(&handle.0) else {
            continue;
        };
        skin.concealed = concealed;
        apply_skin_concealment_material(&mut material, skin.base_alpha_cutoff, concealed);
    }
}

fn apply_skin_concealment_material(
    material: &mut GmdlMaterial,
    base_alpha_cutoff: f32,
    concealed: bool,
) {
    material.params.opacity = if concealed { 0.1 } else { 1.0 };
    material.params.alpha_cutoff = if concealed { 0.0 } else { base_alpha_cutoff };
    material.mode = if concealed {
        GmdlMode::Blend
    } else {
        GmdlMode::Opaque
    };
    material.params.mode = material.mode.as_u32();
}

fn reveal_ready_fighter_skins(
    mut commands: Commands,
    mut skins: Query<(Entity, &SkinnedMesh, &mut Visibility), With<PendingFighterSkinReveal>>,
    joints: Query<&GlobalTransform>,
    inverse_binds: Res<Assets<SkinnedMeshInverseBindposes>>,
) {
    for (entity, skin, mut visibility) in &mut skins {
        let Some(inverse) = inverse_binds.get(&skin.inverse_bindposes) else {
            continue;
        };
        if inverse.len() != skin.joints.len()
            || skin.joints.iter().any(|joint| joints.get(*joint).is_err())
        {
            continue;
        }
        let root = skin
            .joints
            .first()
            .and_then(|joint| joints.get(*joint).ok())
            .map(GlobalTransform::translation);
        info!(
            "fighter skin GPU init ready: skin={entity:?} joints={} root={root:?}",
            skin.joints.len()
        );
        *visibility = Visibility::Visible;
        commands.entity(entity).remove::<PendingFighterSkinReveal>();
    }
}

fn despawn_fighter_models(mut commands: Commands, query: Query<Entity, With<FighterModel>>) {
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

fn refresh_character_variant(
    mut commands: Commands,
    assets: Option<ResMut<CharacterAssetsSet>>,
    failures: Option<ResMut<CharacterLoadFailures>>,
    mut fighters: Query<
        (
            Entity,
            &mut crate::components::fighter::Fighter,
            &mut PoseRuntime,
            &mut EntryBlend,
            &crate::components::fighter::CharacterVariantChanged,
        ),
        With<crate::components::fighter::CharacterVariantChanged>,
    >,
    models: Query<(Entity, &FighterModel)>,
) {
    let (Some(mut assets), Some(mut failures)) = (assets, failures) else {
        return;
    };
    let roots = resource_roots(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    for (fighter_entity, mut fighter, mut pose, mut blend, changed) in &mut fighters {
        let character = format!("p{}", changed.requested_character_id);
        if !ensure_character_assets(&mut assets, &mut failures, &roots, &character) {
            commands
                .entity(fighter_entity)
                .remove::<crate::components::fighter::CharacterVariantChanged>();
            continue;
        }
        fighter.character_id = changed.requested_character_id;
        for (model_entity, model) in &models {
            if model.fighter == fighter_entity {
                commands.entity(model_entity).despawn();
            }
        }
        if let Some(character_assets) = assets.0.get(&character) {
            let pose_id = if character_assets.poses.contains_key(&0) {
                0
            } else {
                character_assets.poses.keys().next().copied().unwrap_or(0)
            };
            character_assets.apply_pose(&mut pose, pose_id, true);
            *blend = EntryBlend::default();
        }
        commands
            .entity(fighter_entity)
            .remove::<crate::components::fighter::CharacterVariantChanged>();
    }
}

fn resource_roots(workdir: &std::path::Path) -> Vec<PathBuf> {
    crate::plugins::audio_plugin::resource_roots(workdir)
}

fn read_first(roots: &[PathBuf], path: &str) -> Option<Vec<u8>> {
    if let Some(bytes) = meteor_asset::runtime_assets::read_legacy_path(path) {
        return Some(bytes);
    }
    #[cfg(not(test))]
    {
        let _ = roots;
        None
    }
    #[cfg(test)]
    {
        roots.iter().find_map(|root| {
            let exact = root.join(path);
            if let Ok(bytes) = crate::resource_io::read(&exact) {
                return Some(bytes);
            }
            let mut current = root.clone();
            for component in std::path::Path::new(path).components() {
                let std::path::Component::Normal(wanted) = component else {
                    return None;
                };
                let entry = std::fs::read_dir(&current)
                    .ok()?
                    .filter_map(Result::ok)
                    .find(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .eq_ignore_ascii_case(&wanted.to_string_lossy())
                    })?;
                current = entry.path();
            }
            crate::resource_io::read(current).ok()
        })
    }
}

fn clear_character_load_failures(mut failures: ResMut<CharacterLoadFailures>) {
    failures.0.clear();
}

fn ensure_character_assets(
    assets: &mut CharacterAssetsSet,
    failures: &mut CharacterLoadFailures,
    roots: &[PathBuf],
    character: &str,
) -> bool {
    if assets.0.contains_key(character) {
        return true;
    }
    if failures.0.contains(character) {
        return false;
    }
    match load_character_asset(roots, character) {
        Some(loaded) => {
            info!("character {character} loaded on demand");
            assets.0.insert(character.to_string(), loaded);
            true
        }
        None => {
            warn!("character assets missing for {character}");
            failures.0.insert(character.to_string());
            false
        }
    }
}

pub(crate) fn load_character_assets(
    mut commands: Commands,
    request: Res<crate::app_state::LaunchRequest>,
    room: Res<crate::app_state::RoomState>,
    net: Res<crate::plugins::network_plugin::NetState>,
    mut cached: Option<ResMut<CharacterAssetsSet>>,
    common_cached: Option<Res<CommonAmb>>,
) {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let roots = resource_roots(&root);
    let mut ids = vec![format!("p{}", request.character_id)];

    for member in &net.members {
        let id = format!("p{}", member.character_id);
        if !ids.iter().any(|existing| existing == &id) {
            ids.push(id);
        }
    }

    if request.spawn_room_npcs {
        for member in &room.members {
            if !member.is_npc {
                continue;
            }
            let id = format!("p{}", member.character_id);
            if !ids.iter().any(|existing| existing == &id) {
                ids.push(id);
            }
        }
    }
    let mut assets = cached
        .as_deref_mut()
        .map(|cache| std::mem::take(&mut cache.0))
        .unwrap_or_default();
    assets.retain(|id, _| ids.contains(id));
    for character_id in &ids {
        if assets.contains_key(character_id) {
            continue;
        }
        if let Some(character) = load_character_asset(&roots, character_id) {
            assets.insert(character_id.clone(), character);
        }
    }
    info!("character assets ready: {} characters loaded", assets.len());
    commands.insert_resource(CharacterAssetsSet(assets));

    if common_cached.is_some() {
        return;
    }

    match read_first(&roots, "Common/character.amb").and_then(|bytes| match parse_amb(&bytes) {
        Ok(amb) => Some(amb),
        Err(error) => {
            warn!("unable to decode Common/character.amb: {error}");
            None
        }
    }) {
        Some(amb) => {
            info!("Common/character.amb loaded: {} frames", amb.num_frames);
            commands.insert_resource(CommonAmb(amb));
        }
        None => warn!("Common/character.amb missing; source=0 poses will freeze/corrupt"),
    }
}

pub(crate) struct PreparedNetworkModels {
    pub characters: std::collections::HashMap<String, CharacterAssets>,
    pub common: AmbAnimation,
}

pub(crate) fn prepare_network_models(
    players: &[meteor_network::protocol::RoomMemberWire],
) -> Result<PreparedNetworkModels, String> {
    let roots = resource_roots(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    prepare_network_models_at(&roots, players)
}

fn prepare_network_models_at(
    roots: &[PathBuf],
    players: &[meteor_network::protocol::RoomMemberWire],
) -> Result<PreparedNetworkModels, String> {
    if players.is_empty() || players.len() > 16 {
        return Err("整备成员数量无效".into());
    }
    let common = read_first(roots, "Common/character.amb").ok_or("缺少 Common/character.amb")?;
    let common = parse_amb(&common).map_err(|e| format!("Common/character.amb: {e}"))?;
    let mut characters = std::collections::HashMap::new();
    let mut checked_parts = std::collections::HashSet::new();
    let mut checked_textures = std::collections::HashSet::new();
    for player in players {
        if player.character_id > 19 || !(1..=12).contains(&player.weapon) {
            return Err(format!(
                "无效人物/兵器：p{} / {}",
                player.character_id, player.weapon
            ));
        }
        let id = format!("p{}", player.character_id);
        if !characters.contains_key(&id) {
            let character = load_character_asset(roots, &id)
                .ok_or_else(|| format!("人物 {id} 的 SKC/BNC/POS/AMB 缺失或损坏"))?;
            if character.skin.vertex_count == 0
                || character.skin.indices.is_empty()
                || character.skeleton.skin_bone_count() == 0
                || character.poses.is_empty()
            {
                return Err(format!("人物 {id} 的网格、骨骼或动作为空"));
            }
            characters.insert(id.clone(), character);
        }
        let character = &characters[&id];
        let mut textures: Vec<String> = character
            .skin
            .materials
            .iter()
            .filter_map(|m| m.texture.clone())
            .collect();
        for part in weapon_part_specs(player.weapon, 0) {
            if !checked_parts.insert(part.stem.clone()) {
                continue;
            }
            let base = format!("cmodel/{}", part.stem);
            if let Some(bytes) = read_first(roots, &format!("{base}.gmc")) {
                let mesh = meteor_asset::gmc::parse_gmc(&crate::text_codec::decode_text(&bytes));
                if !mesh
                    .objects
                    .iter()
                    .any(|o| !o.positions.is_empty() && !o.faces.is_empty())
                {
                    return Err(format!("兵器模型 {base}.gmc 为空或损坏"));
                }
                textures.extend(mesh.textures);
            } else {
                let bytes = read_first(roots, &format!("{base}.gmb"))
                    .ok_or_else(|| format!("缺少兵器模型 {base}.gmc/gmb"))?;
                let mesh = meteor_asset::gmdl::decode_gmdl(&bytes)
                    .map_err(|e| format!("{base}.gmb: {e}"))?;
                if !mesh
                    .objects
                    .iter()
                    .any(|o| !o.positions.is_empty() && !o.indices.is_empty())
                {
                    return Err(format!("兵器模型 {base}.gmb 为空"));
                }
                textures.extend(mesh.textures.into_iter().map(|t| t.name));
            }
        }
        for texture in textures {
            if !checked_textures.insert(texture.clone()) {
                continue;
            }
            let bytes = resolve_weapon_texture(character, &texture)
                .ok_or_else(|| format!("缺少模型贴图 {texture}"))?;
            meteor_asset::texture::decode_image(&bytes)
                .map_err(|e| format!("贴图 {texture}: {e}"))?;
        }
    }
    Ok(PreparedNetworkModels { characters, common })
}

pub(crate) fn load_character_asset(
    roots: &[PathBuf],
    character_id: &str,
) -> Option<CharacterAssets> {
    let skin_text = read_first(roots, &format!("pmodel/{character_id}.skc"))
        .map(|b| crate::text_codec::decode_text(&b))?;
    let bnc_text = read_first(
        roots,
        &format!("pmodel/P{}.bnc", character_id[1..].to_uppercase()),
    )
    .map(|b| crate::text_codec::decode_text(&b))?;
    let pos_text = read_first(
        roots,
        &format!("pmodel/P{}.POS", character_id[1..].to_uppercase()),
    )
    .map(|b| crate::text_codec::decode_text(&b))?;
    let amb_bytes = read_first(roots, &format!("pmodel/{character_id}.amb"))?;

    let act = read_first(roots, "Common/character.act")
        .map(|bytes| crate::text_codec::decode_text(&bytes))
        .map(|text| meteor_asset::act::parse_character_act(&text))
        .unwrap_or_default();

    let ptexture =
        read_first(roots, "ptexture.pak").and_then(|bytes| PakArchive::from_bytes(&bytes).ok());
    let ctexture =
        read_first(roots, "ctexture.pak").and_then(|bytes| PakArchive::from_bytes(&bytes).ok());
    let texture_patch =
        read_first(roots, "TexturePatch.pak").and_then(|bytes| PakArchive::from_bytes(&bytes).ok());
    match &ptexture {
        Some(pak) => info!("ptexture.pak loaded: {} entries", pak.len()),
        None if meteor_asset::runtime_assets::registered_pack_count() > 0 => {
            info!("legacy ptexture.pak not mounted; using runtime MPK texture scopes")
        }
        None => warn!("ptexture.pak missing (character untextured)"),
    }
    let skin = parse_skin(&skin_text);
    let bone_data = parse_bones(&bnc_text);
    let skeleton = Skeleton::from_bone_data(&bone_data);

    let dummy_order: std::collections::HashMap<String, usize> = bone_data
        .dummies
        .iter()
        .enumerate()
        .map(|(i, d)| (d.name.clone(), i))
        .collect();
    let amb = match parse_amb(&amb_bytes) {
        Ok(amb) => amb,
        Err(error) => {
            warn!("unable to decode {character_id}.amb: {error}");
            return None;
        }
    };
    let pose_frames: std::collections::BTreeMap<u16, (u32, u32)> = parse_poses(&pos_text)
        .into_iter()
        .map(|p| (p.id, (p.start, p.end)))
        .collect();
    let poses: std::collections::BTreeMap<u16, meteor_asset::pos::Pose> = parse_poses(&pos_text)
        .into_iter()
        .map(|p| (p.id, p))
        .collect();
    info!(
        "character {character_id} loaded: {} skin verts, {} bones, {} amb frames, {} poses",
        skin.vertex_count,
        skeleton.skin_bone_count(),
        amb.num_frames,
        pose_frames.len()
    );

    let bone_local_bounds = {
        let positions: Vec<meteor_core::Vec3> = skin
            .positions
            .chunks_exact(3)
            .map(|c| meteor_core::Vec3::new(c[0], c[1], c[2]))
            .collect();
        let influences: Vec<Vec<meteor_combat::hitbox::BoneInfluence>> = skin
            .skin_influences
            .iter()
            .map(|refs| {
                refs.iter()
                    .map(|r| meteor_combat::hitbox::BoneInfluence {
                        bone: r.bone,
                        weight: r.weight,
                    })
                    .collect()
            })
            .collect();
        meteor_combat::hitbox::build_bone_local_bounds(&positions, &influences)
    };
    Some(CharacterAssets {
        amb,
        skin,
        skeleton,
        dummy_order,
        pose_frames,
        poses,
        ptexture,
        ctexture,
        texture_patch,
        act,
        bone_local_bounds,
    })
}

#[cfg(test)]
pub(crate) fn load_character_asset_for_test(
    roots: &[PathBuf],
    character_id: &str,
) -> Option<CharacterAssets> {
    load_character_asset(roots, character_id)
}

type SpawnFighterModelQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static crate::components::fighter::Fighter,
        &'static crate::components::fighter::WeaponForm,
        &'static crate::components::fighter::PoseRuntime,
        Option<&'static crate::plugins::stage_plugin::LocalFighter>,
        Option<&'static crate::plugins::network_plugin::RemoteFighter>,
    ),
    Without<FighterModel>,
>;

#[allow(clippy::too_many_arguments)]
fn spawn_fighter_model(
    mut commands: Commands,
    assets: Option<ResMut<CharacterAssetsSet>>,
    failures: Option<ResMut<CharacterLoadFailures>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut images: ResMut<Assets<bevy::image::Image>>,
    mut inverse_binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    net: Res<crate::plugins::network_plugin::NetState>,
    fighters: SpawnFighterModelQuery<'_, '_>,
    models: Query<&FighterModel>,
) {
    let (Some(mut assets), Some(mut failures)) = (assets, failures) else {
        return;
    };
    let roots = resource_roots(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    for (fighter, fighter_meta, weapon_form, pose, local, remote) in &fighters {
        if models.iter().any(|model| model.fighter == fighter) {
            continue;
        }
        let character = format!("p{}", fighter_meta.character_id);
        if !ensure_character_assets(&mut assets, &mut failures, &roots, &character) {
            continue;
        }
        let Some(character_assets) = assets.0.get(&character) else {
            continue;
        };
        spawn_fighter_model_for(
            &mut commands,
            fighter,
            fighter_meta,
            meteor_combat::combo_engine::weapon_visual_form(
                fighter_meta.weapon,
                weapon_form.current,
                pose.pose_id,
            ),
            &character,
            character_assets,
            network_skin_color_index(&net, local.is_some(), remote),
            &mut meshes,
            &mut materials,
            &mut images,
            &mut inverse_binds,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_fighter_model_for(
    commands: &mut Commands,
    fighter: Entity,
    fighter_meta: &crate::components::fighter::Fighter,
    weapon_form: u8,
    character: &str,
    assets: &CharacterAssets,
    color_index: u8,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<GmdlMaterial>,
    images: &mut Assets<bevy::image::Image>,
    inverse_binds: &mut Assets<SkinnedMeshInverseBindposes>,
) {
    let skeleton = &assets.skeleton;
    let skin = &assets.skin;

    let model = commands
        .spawn((
            FighterModel {
                fighter,
                character: character.to_string(),
            },
            Visibility::default(),
            InheritedVisibility::default(),
            Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::splat(0.12)),
        ))
        .id();
    commands.entity(model).insert(ChildOf(fighter));

    let mut node_entities: Vec<Option<Entity>> = vec![None; skeleton.nodes.len()];
    let mut joint_entities: Vec<Entity> = Vec::with_capacity(skeleton.skin_bone_count());
    let skin_node_map: std::collections::HashMap<usize, usize> = skeleton
        .skin_indices
        .iter()
        .enumerate()
        .map(|(skin, node)| (*node, skin))
        .collect();
    let mut roots: Vec<usize> = Vec::new();

    let mut weapon_right_attach: Option<Entity> = None;
    let mut weapon_left_attach: Option<Entity> = None;
    for (node_index, node) in skeleton.nodes.iter().enumerate() {
        let transform = Transform {
            translation: Vec3::new(
                node.local_translation.x,
                node.local_translation.y,
                node.local_translation.z,
            ),
            rotation: Quat::from_xyzw(
                node.local_rotation.x,
                node.local_rotation.y,
                node.local_rotation.z,
                node.local_rotation.w,
            ),
            ..default()
        };
        let entity = if node.is_dummy {
            let entity = commands
                .spawn((
                    FighterDummy {
                        model,
                        name: node.name.clone(),
                        dummy_index: assets
                            .dummy_order
                            .get(&node.name)
                            .copied()
                            .unwrap_or(usize::MAX),
                    },
                    Visibility::default(),
                    InheritedVisibility::default(),
                    transform,
                ))
                .id();

            if node.name == "d_wpnR" {
                weapon_right_attach = Some(entity);
            } else if node.name == "d_wpnL" {
                weapon_left_attach = Some(entity);
            }
            entity
        } else {
            commands
                .spawn((
                    FighterBone {
                        model,
                        name: node.name.clone(),
                        skin_index: skin_node_map
                            .get(&node_index)
                            .copied()
                            .unwrap_or(usize::MAX),
                        node_index,
                    },
                    Visibility::default(),
                    InheritedVisibility::default(),
                    transform,
                ))
                .id()
        };
        node_entities[node_index] = Some(entity);
        if skin_node_map.contains_key(&node_index) {
            joint_entities.push(entity);
        }
        if node.parent.is_none() {
            roots.push(node_index);
        }
    }
    for (node_index, node) in skeleton.nodes.iter().enumerate() {
        if let Some(parent) = node.parent {
            if let Some(parent_entity) = node_entities[parent] {
                if let Some(entity) = node_entities[node_index] {
                    commands.entity(entity).insert(ChildOf(parent_entity));
                }
            }
        }
    }

    let inverse: Vec<bevy::math::Mat4> = skeleton
        .skin_world_bind_inverse
        .iter()
        .map(|m| bevy::math::Mat4::from_cols_array(&m.m))
        .collect();
    let inverse_handle = inverse_binds.add(SkinnedMeshInverseBindposes::from(inverse));

    let mut material_cache: std::collections::HashMap<usize, Handle<GmdlMaterial>> =
        std::collections::HashMap::new();
    for group in &skin.groups {
        let indices = skin.indices[group.start..group.start + group.count].to_vec();
        let mesh = build_group_mesh(skin, &indices);
        let def = skin.materials.get(group.material_index);
        let base_alpha_cutoff = if def.is_some_and(|def| def.alpha) {
            0.5
        } else {
            0.0
        };
        let material = material_cache
            .entry(group.material_index)
            .or_insert_with(|| build_skin_material(def, assets, color_index, images, materials))
            .clone();
        commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material),
                SkinnedMesh {
                    inverse_bindposes: inverse_handle.clone(),
                    joints: joint_entities.clone(),
                },
                FighterSkin {
                    fighter,
                    base_alpha_cutoff,
                    concealed: false,
                },
                PendingFighterSkinReveal,
                Visibility::Hidden,
                InheritedVisibility::default(),
                Transform::IDENTITY,
            ))
            .insert(ChildOf(model));
    }

    for root in roots {
        if let Some(entity) = node_entities[root] {
            commands.entity(entity).insert(ChildOf(model));
        }
    }

    if weapon_right_attach.is_some() || weapon_left_attach.is_some() {
        let attachments = WeaponAttachments {
            right: weapon_right_attach,
            left: weapon_left_attach,
        };
        for slot in 0..fighter_meta.weapon_slots.len() {
            let weapon = fighter_meta.weapon_slots[slot];
            if weapon == 0 {
                continue;
            }
            spawn_weapon(
                commands,
                fighter,
                slot as u8,
                fighter_meta.weapon_slot,
                weapon,
                fighter_meta.weapon_slot_variants[slot],
                weapon_form,
                attachments,
                assets,
                meshes,
                materials,
                images,
            );
        }
    }
    info!(
        "character model attached to fighter {:?}: {} material groups",
        fighter,
        skin.groups.len()
    );
}

pub(crate) fn weapon_model_stem(weapon_id: u8, variant: u8) -> Option<String> {
    let stem = match weapon_id {
        1 => format!("w3_{variant}"),
        2 => format!("w6_{variant}"),
        3 => format!("w2_{variant}"),
        4 => format!("w8_{variant}r"),
        5 => format!("w1_{variant}"),
        6 => format!("w0_{variant}"),
        7 => format!("w7_{variant}"),
        8 => format!("w5_{variant}"),
        9 => format!("w4_{variant}"),
        10 => format!("w9_{variant}r"),
        11 => format!("w10_{variant}r"),
        12 => format!("w11_{variant}"),
        _ => return None,
    };
    Some(stem)
}

pub(crate) fn weapon_slot_from_item(item_id: i64) -> Option<(u8, u8)> {
    if !(1..=59).contains(&item_id) {
        return None;
    }
    match item_id {
        47..=50 => return Some((10, (item_id - 47) as u8)),
        51..=54 => return Some((11, (item_id - 51) as u8)),
        55..=58 => return Some((12, (item_id - 55) as u8)),
        59 => return Some((2, 5)),
        _ => {}
    }
    const CLASSES: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 8, 9];
    let offset = (item_id - 1) as usize;
    Some((
        CLASSES[offset % CLASSES.len()],
        (offset / CLASSES.len()) as u8,
    ))
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct WeaponAttachments {
    pub right: Option<Entity>,
    pub left: Option<Entity>,
}

#[derive(Debug, Clone)]
struct WeaponPartSpec {
    stem: String,
    left: bool,
    forms: Option<Vec<u8>>,
}

fn weapon_part_specs(weapon_id: u8, variant: u8) -> Vec<WeaponPartSpec> {
    let part =
        |stem: String, left: bool, forms: Option<Vec<u8>>| WeaponPartSpec { stem, left, forms };
    match weapon_id {
        4 => vec![
            part(format!("w8_{variant}r"), false, None),
            part(format!("w8_{variant}l"), true, None),
        ],
        10 => vec![
            part(format!("w9_{variant}a"), true, Some(vec![1])),
            part(format!("w9_{variant}b"), true, Some(vec![2])),
            part(format!("w9_{variant}r"), false, Some(vec![0])),
            part(format!("w9_{variant}l"), true, Some(vec![0])),
        ],
        11 => vec![
            part(format!("w10_{variant}r"), false, None),
            part(format!("w10_{variant}l"), true, None),
        ],
        _ => weapon_model_stem(weapon_id, variant)
            .map(|stem| vec![part(stem, false, None)])
            .unwrap_or_default(),
    }
}

fn is_authored_weapon_collision_object(
    name: &str,
    vertices: usize,
    faces: usize,
    visibility: &std::collections::HashMap<String, bool>,
) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "atbox" | "latbox" | "ratbox"
    ) || (visibility
        .get(&name.to_ascii_lowercase())
        .is_some_and(|visible| !visible)
        && vertices == 8
        && faces == 12)
}

fn initial_weapon_part_visibility(
    marker: &crate::components::fighter::WeaponModel,
    active_slot: u8,
    active_form: u8,
) -> Visibility {
    if marker.slot != active_slot
        || marker
            .forms
            .as_ref()
            .is_some_and(|forms| !forms.contains(&active_form))
    {
        Visibility::Hidden
    } else {
        Visibility::Visible
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WeaponTextureAlpha {
    None,
    ColorKey,
    Companion,
    Luminance,
}

fn weapon_texture_alpha(texture_name: Option<&str>, texture_mode: &str) -> WeaponTextureAlpha {
    if texture_mode.eq_ignore_ascii_case("COLORKEY") {
        WeaponTextureAlpha::ColorKey
    } else if texture_mode.eq_ignore_ascii_case("ALPHACHANNEL")
        && texture_name.is_some_and(|name| {
            let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
            file.starts_with(['a', 'A'])
                && (file.to_ascii_lowercase().ends_with(".jpg")
                    || file.to_ascii_lowercase().ends_with(".jpeg"))
        })
    {
        WeaponTextureAlpha::Companion
    } else {
        WeaponTextureAlpha::None
    }
}

fn companion_alpha_name(name: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    format!("{stem}.bmp")
}

fn resolve_weapon_texture_exact(assets: &CharacterAssets, name: &str) -> Option<Vec<u8>> {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    for scope in ["patch", "ctexture", "ptexture"] {
        if let Some(bytes) =
            meteor_asset::runtime_assets::read_entry(&format!("texture/{scope}/{file}"))
        {
            return Some(bytes);
        }
    }
    assets
        .texture_patch
        .as_ref()
        .and_then(|pak| pak_read_ci(pak, "TexturePatch", file))
        .or_else(|| {
            assets
                .ctexture
                .as_ref()
                .and_then(|pak| pak_read_ci(pak, "ctexture", file))
        })
        .or_else(|| {
            assets
                .ptexture
                .as_ref()
                .and_then(|pak| pak_read_ci(pak, "ptexture", file))
        })
}

fn resolve_weapon_texture(assets: &CharacterAssets, name: &str) -> Option<Vec<u8>> {
    resolve_runtime_texture(&["patch", "ctexture", "ptexture"], name).or_else(|| {
        let paks: Vec<&PakArchive> = [
            assets.texture_patch.as_ref(),
            assets.ctexture.as_ref(),
            assets.ptexture.as_ref(),
        ]
        .into_iter()
        .flatten()
        .collect();
        resolve_pak_texture(&paks, name)
    })
}

fn apply_companion_alpha(
    color: &mut meteor_asset::texture::DecodedImage,
    alpha: &meteor_asset::texture::DecodedImage,
) -> bool {
    if color.width != alpha.width || color.height != alpha.height {
        return false;
    }
    for (color_pixel, alpha_pixel) in color
        .rgba
        .chunks_exact_mut(4)
        .zip(alpha.rgba.chunks_exact(4))
    {
        color_pixel[3] = alpha_pixel[0];
    }
    true
}

fn apply_luminance_alpha(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = (0.299 * f32::from(pixel[0])
            + 0.587 * f32::from(pixel[1])
            + 0.114 * f32::from(pixel[2]))
        .round()
        .clamp(0.0, 255.0) as u8;
    }
}

fn load_weapon_texture(
    images: &mut Assets<bevy::image::Image>,
    assets: &CharacterAssets,
    name: &str,
    alpha: WeaponTextureAlpha,
) -> Option<Handle<bevy::image::Image>> {
    let bytes = resolve_weapon_texture(assets, name)?;
    let mut decoded = meteor_asset::texture::decode_image(&bytes).ok()?;
    match alpha {
        WeaponTextureAlpha::None => {}
        WeaponTextureAlpha::ColorKey => {
            crate::plugins::pak_textures::punch_color_key(&mut decoded.rgba, 8, 26);
        }
        WeaponTextureAlpha::Luminance => {
            let companion = crate::plugins::pak_textures::legacy_alpha_companion_name(name)
                .and_then(|name| resolve_weapon_texture_exact(assets, &name))
                .and_then(|bytes| meteor_asset::texture::decode_image(&bytes).ok());
            let applied = companion
                .as_ref()
                .is_some_and(|alpha| apply_companion_alpha(&mut decoded, alpha));
            if !applied && !name.to_ascii_lowercase().ends_with(".tga") {
                apply_luminance_alpha(&mut decoded.rgba);
            }
        }
        WeaponTextureAlpha::Companion => {
            let alpha_name = companion_alpha_name(name);
            if let Some(alpha_bytes) = resolve_weapon_texture_exact(assets, &alpha_name) {
                if let Ok(alpha_image) = meteor_asset::texture::decode_image(&alpha_bytes) {
                    apply_companion_alpha(&mut decoded, &alpha_image);
                }
            }
        }
    }
    Some(image_from_decoded(images, &decoded))
}

fn classify_gmc_shader(shader: &meteor_asset::gmc::GmcShader) -> (GmdlMode, f32, f32) {
    if shader.texture_mode.eq_ignore_ascii_case("COLORKEY") {
        return (GmdlMode::Opaque, shader.opacity, 127.0 / 255.0);
    }
    if shader.blend_enabled {
        let material = meteor_asset::gmdl::GmdlMaterial {
            tex_index: shader.texture_index,
            blend: shader.texture_mode.clone(),
            two_side: shader.two_side,
            state: format!(
                "ENABLE {} {}",
                shader.blend_source, shader.blend_destination
            ),
            alpha: shader.opacity,
        };
        return crate::plugins::render_plugin::classify_gmdl_material(&material);
    }
    if shader.texture_mode.eq_ignore_ascii_case("ALPHACHANNEL") || shader.opacity < 1.0 {
        (GmdlMode::Blend, shader.opacity, 0.0)
    } else {
        (GmdlMode::Opaque, 1.0, 0.0)
    }
}

fn gmdl_weapon_texture_alpha(mat: &meteor_asset::gmdl::GmdlMaterial) -> WeaponTextureAlpha {
    if mat.blend.eq_ignore_ascii_case("COLORKEY") {
        WeaponTextureAlpha::ColorKey
    } else if mat.blend.eq_ignore_ascii_case("ALPHACHANNEL") {
        WeaponTextureAlpha::Luminance
    } else {
        WeaponTextureAlpha::None
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_weapon(
    commands: &mut Commands,
    fighter: Entity,
    slot: u8,
    active_slot: u8,
    weapon: u8,
    variant: u8,
    active_form: u8,
    attachments: WeaponAttachments,
    assets: &CharacterAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<GmdlMaterial>,
    images: &mut Assets<bevy::image::Image>,
) {
    let specs = weapon_part_specs(weapon, variant);
    if specs.is_empty() {
        info!("weapon id {weapon} has no model (fists)");
        return;
    }
    for spec in specs {
        let attach = if spec.left {
            attachments.left
        } else {
            attachments.right
        };
        let Some(attach) = attach else {
            warn!(
                "weapon part {} missing {} attachment",
                spec.stem,
                if spec.left { "d_wpnL" } else { "d_wpnR" }
            );
            continue;
        };
        spawn_weapon_part(
            commands,
            fighter,
            slot,
            active_slot,
            weapon,
            variant,
            active_form,
            &spec.stem,
            spec.left,
            spec.forms,
            attach,
            assets,
            meshes,
            materials,
            images,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_weapon_part(
    commands: &mut Commands,
    fighter: Entity,
    slot: u8,
    active_slot: u8,
    weapon: u8,
    variant: u8,
    active_form: u8,
    stem: &str,
    left: bool,
    forms: Option<Vec<u8>>,
    attach: Entity,
    assets: &CharacterAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<GmdlMaterial>,
    images: &mut Assets<bevy::image::Image>,
) {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let roots = resource_roots(&root);
    let des_text = read_first(&roots, &format!("cmodel/{stem}.des"))
        .map(|bytes| crate::text_codec::decode_text(&bytes));
    let visibility = des_text
        .as_deref()
        .map(meteor_asset::des::parse_object_visibility)
        .unwrap_or_default();
    let gmc_text = read_first(&roots, &format!("cmodel/{stem}.GMC"))
        .or_else(|| read_first(&roots, &format!("cmodel/{stem}.gmc")))
        .map(|b| crate::text_codec::decode_text(&b));
    let gmb_bytes = read_first(&roots, &format!("cmodel/{stem}.gmb"))
        .or_else(|| read_first(&roots, &format!("cmodel/{stem}.GMB")));
    let marker = crate::components::fighter::WeaponModel {
        fighter,
        weapon,
        slot,
        variant,
        forms: forms.clone(),
    };
    let mut spawned = 0usize;
    let collision_boxes;
    if let Some(text) = gmc_text {
        let gmc = meteor_asset::gmc::parse_gmc(&text);
        collision_boxes =
            meteor_asset::weapon_collision::extract_gmc_weapon_collision_boxes(&gmc, &visibility);
        spawned += spawn_gmc_weapon(
            commands,
            &gmc,
            &visibility,
            attach,
            &marker,
            assets,
            meshes,
            materials,
            images,
            active_slot,
            active_form,
        );
    } else if let Some(bytes) = gmb_bytes {
        if let Ok(gmdl) = meteor_asset::gmdl::decode_gmdl(&bytes) {
            collision_boxes = meteor_asset::weapon_collision::extract_gmdl_weapon_collision_boxes(
                &gmdl,
                &visibility,
            );
            spawned += spawn_gmdl_weapon(
                commands,
                &gmdl,
                &visibility,
                attach,
                &marker,
                assets,
                meshes,
                materials,
                images,
                active_slot,
                active_form,
            );
        } else {
            collision_boxes = Vec::new();
        }
    } else {
        collision_boxes = Vec::new();
    }
    if spawned == 0 {
        warn!("weapon model {stem} not found under cmodel/ (roots={roots:?})");
    } else {
        info!("weapon model {stem} attached: {spawned} mesh parts");
    }
    if !collision_boxes.is_empty() {
        let collision_visibility =
            initial_weapon_part_visibility(&marker, active_slot, active_form);
        commands
            .spawn((
                crate::components::fighter::WeaponCollisionGeometry {
                    fighter,
                    weapon,
                    slot,
                    variant,
                    part: stem.to_string(),
                    left,
                    boxes: collision_boxes,
                    forms: forms.clone(),
                },
                marker.clone(),
                Transform::IDENTITY,
                collision_visibility,
            ))
            .insert(ChildOf(attach));
    } else {
        warn!("weapon model {stem} has no collision geometry");
    }

    if let Some(des_text) = des_text {
        for (name, position) in meteor_asset::des::parse_object_positions(&des_text) {
            if !name.to_lowercase().starts_with("d_wpn") {
                continue;
            }
            commands
                .spawn((
                    WeaponEndpoint {
                        name: name.to_lowercase(),
                        fighter,
                        weapon,
                        slot,
                        variant,
                        part: stem.to_string(),
                        left,
                        forms: forms.clone(),
                    },
                    marker.clone(),
                    Transform::from_translation(Vec3::new(position[0], position[1], position[2])),
                ))
                .insert(ChildOf(attach));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_gmc_weapon(
    commands: &mut Commands,
    gmc: &meteor_asset::gmc::GmcMesh,
    visibility: &std::collections::HashMap<String, bool>,
    attach: Entity,
    marker: &crate::components::fighter::WeaponModel,
    assets: &CharacterAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<GmdlMaterial>,
    images: &mut Assets<bevy::image::Image>,
    active_slot: u8,
    active_form: u8,
) -> usize {
    let mut spawned = 0usize;
    let mut material_cache: std::collections::HashMap<usize, Handle<GmdlMaterial>> =
        std::collections::HashMap::new();
    for object in &gmc.objects {
        if object.positions.is_empty() || object.faces.is_empty() {
            continue;
        }

        if is_authored_weapon_collision_object(
            &object.name,
            object.positions.len() / 3,
            object.faces.len(),
            visibility,
        ) {
            continue;
        }

        let mut groups: std::collections::HashMap<i32, Vec<u32>> = std::collections::HashMap::new();
        for face in &object.faces {
            if face.shader < 0 {
                continue;
            }
            groups
                .entry(face.shader)
                .or_default()
                .extend_from_slice(&face.indices);
        }
        if groups.is_empty() {
            continue;
        }
        for (shader_id, indices) in groups {
            let shader = gmc
                .shaders
                .iter()
                .find(|s| s.id == shader_id as usize)
                .or_else(|| gmc.shaders.first());
            let material = material_cache
                .entry(shader_id as usize)
                .or_insert_with(|| {
                    let texture_name = shader.and_then(|shader| {
                        (shader.texture_index >= 0)
                            .then(|| gmc.textures.get(shader.texture_index as usize))
                            .flatten()
                            .map(String::as_str)
                    });
                    let alpha = weapon_texture_alpha(
                        texture_name,
                        shader.map_or("NORMAL", |shader| shader.texture_mode.as_str()),
                    );
                    let texture = texture_name
                        .and_then(|name| load_weapon_texture(images, assets, name, alpha));
                    let two_side = shader.map(|s| s.two_side).unwrap_or(false);
                    let (mode, opacity, alpha_cutoff) = shader
                        .map(classify_gmc_shader)
                        .unwrap_or((GmdlMode::Opaque, 1.0, 0.0));
                    materials.add(GmdlMaterial::new(
                        GmdlMaterialParams {
                            opacity,
                            alpha_cutoff,
                            mode: mode.as_u32(),
                            has_texture: texture.is_some() as u32,

                            uv_offset: Vec2::ZERO,
                        },
                        texture,
                        two_side,
                        mode,
                    ))
                })
                .clone();
            let mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                object
                    .positions
                    .chunks_exact(3)
                    .map(|v| [v[0], v[1], v[2]])
                    .collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_NORMAL,
                object
                    .normals
                    .chunks_exact(3)
                    .map(|v| [v[0], v[1], v[2]])
                    .collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_UV_0,
                object
                    .uvs
                    .chunks_exact(2)
                    .map(|v| [v[0], v[1]])
                    .collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_COLOR,
                object
                    .colors
                    .chunks_exact(4)
                    .map(|c| {
                        [
                            c[0] as f32 / 255.0,
                            c[1] as f32 / 255.0,
                            c[2] as f32 / 255.0,
                            1.0,
                        ]
                    })
                    .collect::<Vec<_>>(),
            )
            .with_inserted_indices(Indices::U32(indices));
            commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material),
                    initial_weapon_part_visibility(marker, active_slot, active_form),
                    InheritedVisibility::default(),
                    Transform::IDENTITY,
                    marker.clone(),
                ))
                .insert(ChildOf(attach));
            spawned += 1;
        }
    }
    spawned
}

#[allow(clippy::too_many_arguments)]
fn spawn_gmdl_weapon(
    commands: &mut Commands,
    gmdl: &meteor_asset::gmdl::GmdlMesh,
    visibility: &std::collections::HashMap<String, bool>,
    attach: Entity,
    marker: &crate::components::fighter::WeaponModel,
    assets: &CharacterAssets,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<GmdlMaterial>,
    images: &mut Assets<bevy::image::Image>,
    active_slot: u8,
    active_form: u8,
) -> usize {
    let mut spawned = 0usize;
    for object in &gmdl.objects {
        if is_authored_weapon_collision_object(
            &object.name,
            object.num_verts,
            object.num_faces,
            visibility,
        ) {
            continue;
        }
        if object.indices.is_empty() || object.positions.is_empty() {
            continue;
        }
        let mut groups: std::collections::HashMap<i32, Vec<u32>> = std::collections::HashMap::new();
        for (face, mat_idx) in object.face_mat.iter().enumerate() {
            if *mat_idx < 0 || *mat_idx as usize >= gmdl.materials.len() {
                continue;
            }
            groups
                .entry(*mat_idx)
                .or_default()
                .extend_from_slice(&object.indices[face * 3..face * 3 + 3]);
        }
        for (mat_idx, indices) in groups {
            let mat = &gmdl.materials[mat_idx as usize];
            let (mode, opacity, alpha_cutoff) =
                crate::plugins::render_plugin::classify_gmdl_material(mat);
            let texture_name = gmdl.resolve_texture_name(mat);
            let alpha = gmdl_weapon_texture_alpha(mat);
            let texture =
                texture_name.and_then(|name| load_weapon_texture(images, assets, name, alpha));
            let material = materials.add(GmdlMaterial::new(
                GmdlMaterialParams {
                    opacity,
                    alpha_cutoff,
                    mode: mode.as_u32(),
                    has_texture: texture.is_some() as u32,

                    uv_offset: Vec2::ZERO,
                },
                texture,
                mat.two_side,
                mode,
            ));
            let mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_POSITION,
                object
                    .positions
                    .chunks_exact(3)
                    .map(|v| [v[0], v[1], v[2]])
                    .collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_NORMAL,
                object
                    .normals
                    .chunks_exact(3)
                    .map(|v| [v[0], v[1], v[2]])
                    .collect::<Vec<_>>(),
            )
            .with_inserted_attribute(
                Mesh::ATTRIBUTE_UV_0,
                object
                    .uvs
                    .chunks_exact(2)
                    .map(|v| [v[0], v[1]])
                    .collect::<Vec<_>>(),
            )
            .with_inserted_indices(Indices::U32(indices));
            commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material),
                    initial_weapon_part_visibility(marker, active_slot, active_form),
                    InheritedVisibility::default(),
                    Transform::IDENTITY,
                    marker.clone(),
                ))
                .insert(ChildOf(attach));
            spawned += 1;
        }
    }
    spawned
}

fn build_group_mesh(skin: &meteor_asset::skc::SkinMesh, indices: &[u32]) -> Mesh {
    let joint_indices: Vec<[u16; 4]> = skin
        .skin_indices
        .chunks_exact(4)
        .map(|c| [c[0] as u16, c[1] as u16, c[2] as u16, c[3] as u16])
        .collect();
    let joint_weights: Vec<[f32; 4]> = skin
        .skin_weights
        .chunks_exact(4)
        .map(|c| [c[0], c[1], c[2], c[3]])
        .collect();
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        skin.positions
            .chunks_exact(3)
            .map(|v| [v[0], v[1], v[2]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_UV_0,
        skin.uvs
            .chunks_exact(2)
            .map(|v| [v[0], v[1]])
            .collect::<Vec<_>>(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(joint_indices),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, joint_weights)
    .with_inserted_indices(Indices::U32(indices.to_vec()))
}

fn build_skin_material(
    def: Option<&meteor_asset::skc::SkinMaterial>,
    assets: &CharacterAssets,
    color_index: u8,
    images: &mut Assets<bevy::image::Image>,
    materials: &mut Assets<GmdlMaterial>,
) -> Handle<GmdlMaterial> {
    let Some(def) = def else {
        return materials.add(GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.0,
                mode: 0,
                has_texture: 0,
                uv_offset: Vec2::ZERO,
            },
            None,
            false,
            GmdlMode::Opaque,
        ));
    };
    let texture = def.texture.as_deref().and_then(|name| {
        let variant = apply_color_variant(name, color_index);
        let paks: Vec<&PakArchive> = assets
            .ptexture
            .iter()
            .chain(assets.ctexture.iter())
            .collect();
        let Some(bytes) = resolve_runtime_texture(&["ptexture", "ctexture"], &variant)
            .or_else(|| resolve_pak_texture(&paks, &variant))
        else {
            info!("character texture missing: {variant} (untextured fallback)");
            return None;
        };
        image_handle(images, &bytes).or_else(|| {
            warn!("character texture decode failed: {variant}");
            None
        })
    });
    materials.add(GmdlMaterial::new(
        GmdlMaterialParams {
            opacity: 1.0,

            alpha_cutoff: if def.alpha { 0.5 } else { 0.0 },
            mode: 0,
            has_texture: texture.is_some() as u32,

            uv_offset: Vec2::ZERO,
        },
        texture,
        def.two_side,
        GmdlMode::Opaque,
    ))
}

type ModelTransformQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static FighterModel, &'static mut Transform),
    (Without<FighterBone>, Without<FighterDummy>),
>;

fn visual_root_owned(
    state: meteor_core::FighterState,
    airborne: bool,
    remote: bool,
    hit_reaction: bool,
    weapon_switch: bool,
) -> bool {
    remote
        || airborne
        || hit_reaction
        || weapon_switch
        || matches!(
            state,
            meteor_core::FighterState::Attack1
                | meteor_core::FighterState::Attack2
                | meteor_core::FighterState::Attack3
                | meteor_core::FighterState::Dodge
                | meteor_core::FighterState::Burst
        )
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
fn drive_fighter_animation(
    mut commands: Commands,
    sampled_models: Query<(), With<FighterPoseSampled>>,
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    time: Res<Time<bevy::time::Real>>,
    assets: Option<Res<CharacterAssetsSet>>,
    common_amb: Option<Res<CommonAmb>>,
    mut fighters: Query<
        (
            &crate::components::fighter::MotionState,
            &crate::components::fighter::PoseRuntime,
            &'static mut crate::components::fighter::EntryBlend,
            &crate::components::fighter::SmState,
            &crate::components::fighter::FighterHitReaction,
            &crate::components::fighter::GunAimState,
            Option<&'static crate::plugins::movement_plugin::FighterAir>,
            Option<&'static mut crate::components::fighter::VisualRootPin>,
            Option<&'static crate::components::fighter::WeaponSwitchActive>,
            Option<&'static crate::plugins::network_plugin::RemoteFighter>,
        ),
        With<crate::components::fighter::Fighter>,
    >,
    skeletons: FighterSkeletonQuery,
    mut bones: Query<(&FighterBone, &mut Transform), Without<FighterDummy>>,
    mut dummies: Query<(&FighterDummy, &mut Transform)>,
    mut models: ModelTransformQuery<'_, '_>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(7);
    let Some(assets) = assets else {
        return;
    };
    let Some(common_amb) = common_amb else {
        return;
    };
    let now_seconds = time.elapsed_secs_f64();

    for (model_entity, model, mut model_transform) in models.iter_mut() {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), model.fighter) {
            continue;
        }
        let fighter = model.fighter;
        let Some(character_assets) = assets.0.get(&model.character) else {
            continue;
        };
        let Ok((
            motion,
            pose,
            mut blend,
            sm,
            hit_reaction,
            gun_aim,
            air,
            mut visual_root,
            weapon_switch,
            remote,
        )) = fighters.get_mut(fighter)
        else {
            continue;
        };
        let motion_root_pin = air.and_then(|air| air.motion_root_pin);
        let frame = pose.frame;
        let sampled = character_assets
            .amb_for_pose(&common_amb.0, pose.pose_id)
            .sample_at(frame);

        model_transform.rotation = Quat::from_rotation_y(motion.facing)
            * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);

        let blend_active = blend.duration > 0.0 && blend.pose_id == pose.pose_id;
        if blend_active && blend.started_at_seconds.is_none() {
            let mut root_snap = [0.0f32; 3];
            blend.from_quats.clear();
            for (bone, transform) in bones.iter_many(model_bones(model_entity, &skeletons)) {
                if bone.skin_index == 0 {
                    root_snap = [
                        transform.translation.x,
                        transform.translation.y,
                        transform.translation.z,
                    ];
                }
                if bone.skin_index != usize::MAX {
                    while blend.from_quats.len() <= bone.skin_index {
                        blend.from_quats.push([0.0, 0.0, 0.0, 1.0]);
                    }
                    blend.from_quats[bone.skin_index] = [
                        transform.rotation.x,
                        transform.rotation.y,
                        transform.rotation.z,
                        transform.rotation.w,
                    ];
                }
            }
            blend.from_root = root_snap;

            blend.from_dummy_pos.clear();
            blend.from_dummy_quats.clear();
            for (dummy, transform) in dummies.iter_many(model_dummies(model_entity, &skeletons)) {
                if dummy.dummy_index == usize::MAX {
                    continue;
                }
                while blend.from_dummy_pos.len() <= dummy.dummy_index {
                    blend.from_dummy_pos.push([0.0, 0.0, 0.0]);
                    blend.from_dummy_quats.push([0.0, 0.0, 0.0, 1.0]);
                }
                blend.from_dummy_pos[dummy.dummy_index] = [
                    transform.translation.x,
                    transform.translation.y,
                    transform.translation.z,
                ];
                blend.from_dummy_quats[dummy.dummy_index] = [
                    transform.rotation.x,
                    transform.rotation.y,
                    transform.rotation.z,
                    transform.rotation.w,
                ];
            }
            blend.started_at_seconds = Some(now_seconds);
        }

        if blend_active {
            blend.elapsed = entry_blend_wall_elapsed(blend.started_at_seconds, now_seconds);
        }
        let t = if blend_active && blend.duration > 0.0 {
            (blend.elapsed / blend.duration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let base_dummy_index = character_assets.dummy_order.get("d_base").copied();

        let airborne = air.is_some_and(|air| air.air.airborne);

        let root_owned = gun_aim.aiming
            || visual_root_owned(
                sm.current,
                airborne,
                remote.is_some(),
                hit_reaction.current.is_some(),
                weapon_switch.is_some(),
            );
        if let Some(pin) = visual_root.as_deref_mut() {
            if root_owned {
                pin.baseline = [0.0; 3];
                pin.active = true;
                pin.initialized = true;
                pin.pose_id = pose.pose_id;
                pin.lock_xy = true;

                pin.lock_z = true;
                pin.release_after_sample = false;
            } else if pin.active {
                pin.release_after_sample = true;
            }
            if pin.active && blend_active {
                if let Some(base_index) = base_dummy_index {
                    if let Some(from) = blend.from_dummy_pos.get_mut(base_index) {
                        if pin.lock_xy {
                            from[0] = pin.baseline[0];
                            from[1] = pin.baseline[1];
                        }
                        if pin.lock_z {
                            from[2] = pin.baseline[2];
                        }
                    }
                } else {
                    if pin.lock_xy {
                        blend.from_root[0] = pin.baseline[0];
                        blend.from_root[1] = pin.baseline[1];
                    }
                    if pin.lock_z {
                        blend.from_root[2] = pin.baseline[2];
                    }
                }
            }
        }

        let mut model_bones = bones.iter_many_mut(model_bones(model_entity, &skeletons));
        while let Some((bone, mut transform)) = model_bones.fetch_next() {
            let quat = sampled.bone_quats.get(bone.skin_index).copied();
            if let Some(q) = quat {
                let target = Quat::from_xyzw(q[0], q[1], q[2], q[3]);
                if t < 1.0 && bone.skin_index < blend.from_quats.len() {
                    transform.rotation = blend_quat(blend.from_quats[bone.skin_index], target, t);
                } else {
                    transform.rotation = target;
                }
            }
            if bone.skin_index == 0 {
                let raw_root = if base_dummy_index.is_none() {
                    motion_root_pin.unwrap_or(sampled.root_pivot)
                } else {
                    sampled.root_pivot
                };
                let mut blended = if t < 1.0 {
                    blend_vec3(blend.from_root, raw_root, t)
                } else {
                    raw_root
                };
                if base_dummy_index.is_none() {
                    if let Some(pin) = visual_root.as_deref() {
                        if pin.active {
                            if pin.lock_xy {
                                blended[0] = pin.baseline[0];
                                blended[1] = pin.baseline[1];
                            }
                            if pin.lock_z {
                                blended[2] = pin.baseline[2];
                            }
                        }
                    }
                }
                transform.translation = Vec3::new(blended[0], blended[1], blended[2]);
            }
        }

        let mut model_dummies = dummies.iter_many_mut(model_dummies(model_entity, &skeletons));
        while let Some((dummy, mut transform)) = model_dummies.fetch_next() {
            if let Some(pos) = sampled.dummy_pos.get(dummy.dummy_index) {
                let raw = [pos[0], pos[1], pos[2]];
                let mut blended = if t < 1.0 && dummy.dummy_index < blend.from_dummy_pos.len() {
                    blend_vec3(blend.from_dummy_pos[dummy.dummy_index], raw, t)
                } else {
                    raw
                };
                if base_dummy_index == Some(dummy.dummy_index) {
                    if let Some(pin) = visual_root.as_deref() {
                        if pin.active {
                            if pin.lock_xy {
                                blended[0] = pin.baseline[0];
                                blended[1] = pin.baseline[1];
                            }
                            if pin.lock_z {
                                blended[2] = pin.baseline[2];
                            }
                        }
                    }
                }
                transform.translation = Vec3::new(blended[0], blended[1], blended[2]);
            }
            if let Some(q) = sampled.dummy_quats.get(dummy.dummy_index) {
                let target = Quat::from_xyzw(q[0], q[1], q[2], q[3]);
                if t < 1.0 && dummy.dummy_index < blend.from_dummy_quats.len() {
                    transform.rotation =
                        blend_quat(blend.from_dummy_quats[dummy.dummy_index], target, t);
                } else {
                    transform.rotation = target;
                }
            }
        }

        if let Some(pin) = visual_root.as_deref_mut() {
            if pin.active && pin.release_after_sample {
                pin.active = false;
                pin.initialized = false;
                pin.release_after_sample = false;
                pin.lock_xy = false;
                pin.lock_z = false;
            }
        }

        if !sampled_models.contains(model_entity) {
            commands.entity(model_entity).insert(FighterPoseSampled);
        }

        if blend_active && t >= 1.0 {
            blend.duration = 0.0;
            blend.elapsed = 0.0;
            blend.started_at_seconds = None;
        }
    }
}

pub(crate) fn sync_fighter_model_facing(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    fighters: Query<&crate::components::fighter::MotionState>,
    mut models: ModelTransformQuery<'_, '_>,
) {
    for (_, model, mut transform) in &mut models {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), model.fighter) {
            continue;
        }
        if let Ok(motion) = fighters.get(model.fighter) {
            transform.rotation = Quat::from_rotation_y(motion.facing)
                * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        }
    }
}

#[cfg(test)]
pub(crate) fn drive_fighter_animation_for_test(world: &mut World) {
    use bevy::ecs::system::RunSystemOnce;
    world
        .run_system_once(drive_fighter_animation)
        .expect("production fighter animation");
}

#[cfg(test)]
pub(crate) fn install_crowd_animation_for_test(app: &mut App) {
    app.add_systems(
        super::fighter_frame::schedule(app),
        (
            drive_fighter_animation.in_set(super::FighterFrameSet::SkeletonSample),
            resample_late_air_pose_visual.in_set(super::FighterFrameSet::PoseResolve),
        ),
    );
}

#[cfg(test)]
pub(crate) fn spawn_fighter_model_for_test(world: &mut World, fighter: Entity) {
    let meta = world
        .get::<crate::components::fighter::Fighter>(fighter)
        .unwrap()
        .clone();
    world.resource_scope(|world, assets: Mut<CharacterAssetsSet>| {
        let character = format!("p{}", meta.character_id);
        let mut state = bevy::ecs::system::SystemState::<Commands>::new(world);
        let mut commands = state.get_mut(world).unwrap();
        spawn_fighter_model_for(
            &mut commands,
            fighter,
            &meta,
            0,
            &character,
            &assets.0[&character],
            0,
            &mut Assets::default(),
            &mut Assets::default(),
            &mut Assets::default(),
            &mut Assets::default(),
        );
        state.apply(world);
    });
}

fn resample_late_air_pose_visual(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    assets: Option<Res<CharacterAssetsSet>>,
    common_amb: Option<Res<CommonAmb>>,
    mut fighters: Query<(
        &crate::components::fighter::PoseRuntime,
        &mut crate::plugins::movement_plugin::FighterAir,
        Option<&mut crate::components::fighter::VisualRootPin>,
    )>,
    models: Query<(Entity, &FighterModel)>,
    skeletons: FighterSkeletonQuery,
    mut bones: Query<(&FighterBone, &mut Transform), Without<FighterDummy>>,
    mut dummies: Query<(&FighterDummy, &mut Transform)>,
) {
    #[cfg(not(target_arch = "wasm32"))]
    let _perf = super::perf_plugin::Scope::new(7);
    let (Some(assets), Some(common_amb)) = (assets, common_amb) else {
        return;
    };
    for (model_entity, model) in &models {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), model.fighter) {
            continue;
        }
        let Ok((pose, mut air, mut visual_root)) = fighters.get_mut(model.fighter) else {
            continue;
        };
        if !air.late_visual_pose_snap {
            continue;
        }
        air.late_visual_pose_snap = false;
        if air.motion_root_pin_pose_id != pose.pose_id {
            continue;
        }
        let Some(character_assets) = assets.0.get(&model.character) else {
            continue;
        };
        let sampled = character_assets
            .amb_for_pose(&common_amb.0, pose.pose_id)
            .sample_at(pose.frame);
        let base_dummy_index = character_assets.dummy_order.get("d_base").copied();
        let baseline = [0.0; 3];

        if let Some(pin) = visual_root.as_deref_mut() {
            pin.active = true;
            pin.initialized = true;
            pin.release_after_sample = false;
            pin.pose_id = pose.pose_id;
            pin.baseline = baseline;
            pin.lock_xy = true;
            pin.lock_z = true;
        }

        let mut model_bones = bones.iter_many_mut(model_bones(model_entity, &skeletons));
        while let Some((bone, mut transform)) = model_bones.fetch_next() {
            if let Some(q) = sampled.bone_quats.get(bone.skin_index) {
                transform.rotation = Quat::from_xyzw(q[0], q[1], q[2], q[3]);
            }
            if bone.skin_index == 0 {
                let root = if base_dummy_index.is_none() {
                    baseline
                } else {
                    sampled.root_pivot
                };
                transform.translation = Vec3::new(root[0], root[1], root[2]);
            }
        }
        let mut model_dummies = dummies.iter_many_mut(model_dummies(model_entity, &skeletons));
        while let Some((dummy, mut transform)) = model_dummies.fetch_next() {
            if let Some(pos) = sampled.dummy_pos.get(dummy.dummy_index) {
                let pos = if base_dummy_index == Some(dummy.dummy_index) {
                    baseline
                } else {
                    *pos
                };
                transform.translation = Vec3::new(pos[0], pos[1], pos[2]);
            }
            if let Some(q) = sampled.dummy_quats.get(dummy.dummy_index) {
                transform.rotation = Quat::from_xyzw(q[0], q[1], q[2], q[3]);
            }
        }
    }
}

type RebuildWeaponModelQuery<'w, 's> = Query<'w, 's, (Entity, &'static FighterModel)>;
type RebuildWeaponDummyQuery<'w, 's> = Query<'w, 's, (Entity, &'static FighterDummy)>;

#[allow(clippy::too_many_arguments)]
fn rebuild_weapon_inventory(
    mut commands: Commands,
    assets: Option<Res<CharacterAssetsSet>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut images: ResMut<Assets<bevy::image::Image>>,
    fighters: Query<
        (
            Entity,
            &crate::components::fighter::Fighter,
            &crate::components::fighter::WeaponForm,
            &crate::components::fighter::PoseRuntime,
            &crate::components::fighter::WeaponVariantChanged,
        ),
        With<crate::components::fighter::Fighter>,
    >,
    models: RebuildWeaponModelQuery<'_, '_>,
    dummies: RebuildWeaponDummyQuery<'_, '_>,
    weapons: Query<(Entity, &crate::components::fighter::WeaponModel)>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (entity, fighter, form, pose, _) in &fighters {
        let Some(character_assets) = assets.0.get(&format!("p{}", fighter.character_id)) else {
            continue;
        };
        let model_entity = models
            .iter()
            .find_map(|(model_entity, model)| (model.fighter == entity).then_some(model_entity));
        let find_attach = |name: &str| {
            let model_entity = model_entity?;
            let index = character_assets.dummy_order.get(name).copied()?;
            dummies.iter().find_map(|(dummy_entity, dummy)| {
                (dummy.model == model_entity && dummy.dummy_index == index).then_some(dummy_entity)
            })
        };
        let attachments = WeaponAttachments {
            right: find_attach("d_wpnR"),
            left: find_attach("d_wpnL"),
        };
        let mut ready = [false; 2];
        for (weapon_entity, marker) in weapons.iter().filter(|(_, w)| w.fighter == entity) {
            let slot = usize::from(marker.slot);
            if weapon_marker_matches_inventory(fighter, marker) {
                ready[slot] = true;
            } else {
                commands.entity(weapon_entity).despawn();
            }
        }
        for (slot, ((&weapon, &variant), &slot_ready)) in fighter
            .weapon_slots
            .iter()
            .zip(fighter.weapon_slot_variants.iter())
            .zip(ready.iter())
            .enumerate()
        {
            if weapon == 0 || slot_ready {
                continue;
            }
            spawn_weapon(
                &mut commands,
                entity,
                slot as u8,
                fighter.weapon_slot,
                weapon,
                variant,
                meteor_combat::combo_engine::weapon_visual_form(weapon, form.current, pose.pose_id),
                attachments,
                character_assets,
                &mut meshes,
                &mut materials,
                &mut images,
            );
        }
        commands
            .entity(entity)
            .remove::<crate::components::fighter::WeaponVariantChanged>();
    }
}

fn head_track(
    active_fighter: Option<Res<super::fighter_frame::ActiveFighter>>,
    assets: Option<Res<CharacterAssetsSet>>,
    players: Query<
        (Entity, &crate::components::fighter::HeadTrackState),
        With<crate::plugins::stage_plugin::LocalFighter>,
    >,
    models: Query<(Entity, &FighterModel)>,
    skeletons: FighterSkeletonQuery,
    mut bones: Query<(&FighterBone, &mut Transform), Without<FighterDummy>>,
) {
    let Some(assets) = assets else {
        return;
    };
    for (fighter_entity, head) in &players {
        if !super::fighter_frame::accepts(active_fighter.as_deref(), fighter_entity) {
            continue;
        }
        if head.current.abs() < 1e-8 {
            continue;
        }
        let Some((model_entity, model)) = models.iter().find(|(_, m)| m.fighter == fighter_entity)
        else {
            continue;
        };
        let Some(character_assets) = assets.0.get(&model.character) else {
            continue;
        };
        let Some(head_index) = character_assets
            .skeleton
            .nodes
            .iter()
            .position(|node| node.name == "bau_Head")
        else {
            continue;
        };
        let rotation = Quat::from_rotation_x(head.current);
        let mut model_bones = bones.iter_many_mut(model_bones(model_entity, &skeletons));
        while let Some((bone, mut transform)) = model_bones.fetch_next() {
            if bone.node_index == head_index {
                transform.rotation *= rotation;
            }
        }
    }
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_asset_barrier_preloads_all_twelve_weapon_models() {
        let players: Vec<_> = (1..=12)
            .map(|weapon| meteor_network::protocol::RoomMemberWire {
                player_id: format!("p{weapon}"),
                name: String::new(),
                character_id: 0,
                weapon,
                team: None,
                ready: false,
                connected: true,
            })
            .collect();
        let prepared = super::prepare_network_models(&players)
            .expect("all original weapon parts and textures must preload");
        assert_eq!(prepared.characters.len(), 1);
        assert!(prepared.common.num_frames > 0);
    }

    #[test]
    fn network_asset_barrier_reports_missing_models() {
        let players = vec![meteor_network::protocol::RoomMemberWire {
            player_id: "p".into(),
            name: String::new(),
            character_id: 0,
            weapon: 6,
            team: None,
            ready: false,
            connected: true,
        }];
        assert!(super::prepare_network_models_at(&[], &players).is_err());
    }

    #[test]
    fn head_track_keeps_applying_a_decaying_angle_after_target_loss() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let Some(character) = load_character_asset_for_test(std::slice::from_ref(&root), "p0")
        else {
            eprintln!("p0 assets missing, skipping");
            return;
        };
        let head_index = character
            .skeleton
            .nodes
            .iter()
            .position(|node| node.name == "bau_Head")
            .expect("p0 bau_Head");
        let skin_index = character
            .skeleton
            .skin_indices
            .iter()
            .position(|node| *node == head_index)
            .unwrap_or(usize::MAX);
        let mut app = App::new();
        app.insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p0".to_string(),
            character,
        )])));
        app.add_systems(Update, head_track);
        let fighter = app
            .world_mut()
            .spawn((
                crate::plugins::stage_plugin::LocalFighter,
                crate::components::fighter::HeadTrackState { current: 0.25 },
            ))
            .id();
        let model = app
            .world_mut()
            .spawn(FighterModel {
                fighter,
                character: "p0".to_string(),
            })
            .id();
        let authored = Quat::from_rotation_y(0.2);
        let head = app
            .world_mut()
            .spawn((
                FighterBone {
                    model,
                    name: "bau_Head".to_string(),
                    skin_index,
                    node_index: head_index,
                },
                Transform::from_rotation(authored),
            ))
            .id();

        app.update();

        let actual = app.world().get::<Transform>(head).unwrap().rotation;
        let expected = authored * Quat::from_rotation_x(0.25);
        assert!(actual.dot(expected).abs() > 0.9999);
    }

    #[test]
    fn skin_reveal_waits_for_joint_global_transforms() {
        let mut app = App::new();
        app.init_resource::<Assets<SkinnedMeshInverseBindposes>>()
            .add_systems(Update, reveal_ready_fighter_skins);
        let inverse = app
            .world_mut()
            .resource_mut::<Assets<SkinnedMeshInverseBindposes>>()
            .add(SkinnedMeshInverseBindposes::from(vec![
                bevy::math::Mat4::IDENTITY,
            ]));
        let joint = app.world_mut().spawn_empty().id();
        let skin = app
            .world_mut()
            .spawn((
                SkinnedMesh {
                    inverse_bindposes: inverse,
                    joints: vec![joint],
                },
                PendingFighterSkinReveal,
                Visibility::Hidden,
            ))
            .id();

        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(skin).unwrap(),
            Visibility::Hidden
        );
        assert!(app.world().get::<PendingFighterSkinReveal>(skin).is_some());

        app.world_mut()
            .entity_mut(joint)
            .insert(GlobalTransform::IDENTITY);
        app.update();
        assert_eq!(
            *app.world().get::<Visibility>(skin).unwrap(),
            Visibility::Visible
        );
        assert!(app.world().get::<PendingFighterSkinReveal>(skin).is_none());
    }

    #[test]
    fn remote_snapshot_always_owns_visual_root() {
        assert!(visual_root_owned(
            meteor_core::FighterState::Name,
            false,
            true,
            false,
            false,
        ));
        assert!(visual_root_owned(
            meteor_core::FighterState::Attack1,
            false,
            false,
            false,
            false,
        ));
        assert!(visual_root_owned(
            meteor_core::FighterState::Jump,
            true,
            false,
            false,
            false,
        ));
        assert!(visual_root_owned(
            meteor_core::FighterState::BeHit,
            false,
            false,
            true,
            false,
        ));
        assert!(visual_root_owned(
            meteor_core::FighterState::Dodge,
            false,
            false,
            false,
            false,
        ));
        assert!(visual_root_owned(
            meteor_core::FighterState::Burst,
            false,
            false,
            false,
            false,
        ));
        assert!(visual_root_owned(
            meteor_core::FighterState::Name,
            false,
            false,
            false,
            true,
        ));
        assert!(!visual_root_owned(
            meteor_core::FighterState::Name,
            false,
            false,
            false,
            false,
        ));
    }

    #[test]
    fn ninja_root_velocity_matches_stock_integer_pair_and_not_act_cursor_speed() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let assets =
            load_character_asset_for_test(std::slice::from_ref(&root), "p1").expect("p1 assets");
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).unwrap(),
        )
        .unwrap();

        for (id, frame, expected_speed) in [(464, 14874.25, 12.50553), (471, 15211.75, 6.16771)] {
            let mut pose = crate::components::fighter::PoseRuntime::idle();
            assets.apply_pose(&mut pose, id, true);
            pose.frame = frame;
            for fps in [30.0, 60.0, 120.0] {
                let [x, y, _] = assets.authored_root_step(&common, &pose, 1.0 / fps);
                let speed = x.hypot(y) * fps * meteor_core::MODEL_SCALE;
                assert!(
                    (speed - expected_speed).abs() < 0.001,
                    "pose={id} fps={fps} speed={speed}"
                );
            }
            pose.frame = pose.end as f32;
            assert_eq!(
                assets.authored_root_step(&common, &pose, 1.0 / 60.0),
                [0.0; 3]
            );
        }
    }

    #[test]
    fn late_wall_entry_resamples_visible_pose_after_hit_judge() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        if !root.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let character =
            load_character_asset_for_test(std::slice::from_ref(&root), "p1").expect("p1 assets");
        let common = meteor_asset::amb::parse_amb(
            &std::fs::read(root.join("Common/character.amb")).expect("character.amb"),
        )
        .expect("parse character.amb");
        let pose_id = meteor_physics::locomotion::pose::WALL_CLING_F;
        let pose_def = character.poses[&pose_id].clone();
        let expected = character.motion_root_at(&common, pose_id, pose_def.start as f32);
        let base_index = character.dummy_order["d_base"];
        let mut pose = crate::components::fighter::PoseRuntime::idle();
        character.apply_pose(&mut pose, pose_id, true);

        let mut app = App::new();
        app.insert_resource(CharacterAssetsSet(std::collections::HashMap::from([(
            "p1".to_string(),
            character,
        )])))
        .insert_resource(CommonAmb(common))
        .add_systems(Update, resample_late_air_pose_visual);
        let fighter = app
            .world_mut()
            .spawn((
                pose,
                crate::plugins::movement_plugin::FighterAir {
                    motion_root_pin: Some(expected),
                    motion_root_pin_pose_id: pose_id,
                    late_visual_pose_snap: true,
                    ..default()
                },
                crate::components::fighter::VisualRootPin::default(),
            ))
            .id();
        let model = app
            .world_mut()
            .spawn(FighterModel {
                fighter,
                character: "p1".to_string(),
            })
            .id();
        app.world_mut().spawn((
            FighterBone {
                model,
                name: "root".to_string(),
                skin_index: 0,
                node_index: 0,
            },
            Transform::from_translation(Vec3::splat(99.0)),
        ));
        let d_base = app
            .world_mut()
            .spawn((
                FighterDummy {
                    model,
                    name: "d_base".to_string(),
                    dummy_index: base_index,
                },
                Transform::from_translation(Vec3::splat(99.0)),
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<Transform>(d_base).unwrap().translation,
            Vec3::ZERO
        );
        let air = app
            .world()
            .get::<crate::plugins::movement_plugin::FighterAir>(fighter)
            .unwrap();
        assert!(!air.late_visual_pose_snap);
        let pin = app
            .world()
            .get::<crate::components::fighter::VisualRootPin>(fighter)
            .unwrap();
        assert!(pin.active);
        assert_eq!(pin.pose_id, pose_id);
        assert_eq!(pin.baseline, [0.0; 3]);
    }

    #[test]
    fn network_team_selects_the_same_body_texture_variant_as_web() {
        let mut net = crate::plugins::network_plugin::NetState {
            status: crate::plugins::network_plugin::NetStatus::InRoom,
            player_id: "local".into(),
            mode: "deathmatch".into(),
            ..default()
        };
        net.members = vec![
            meteor_network::protocol::RoomMemberWire {
                connected: true,
                player_id: "local".into(),
                name: "local".into(),
                character_id: 0,
                weapon: 6,
                team: Some(1),
                ready: true,
            },
            meteor_network::protocol::RoomMemberWire {
                connected: true,
                player_id: "peer".into(),
                name: "peer".into(),
                character_id: 1,
                weapon: 5,
                team: Some(2),
                ready: true,
            },
        ];
        let remote = crate::plugins::network_plugin::RemoteFighter {
            player_id: "peer".into(),
        };
        assert_eq!(network_skin_color_index(&net, true, None), 1);
        assert_eq!(network_skin_color_index(&net, false, Some(&remote)), 2);
        assert_eq!(apply_color_variant("nm001b01.tga", 1), "nm001b02.tga");
        assert_eq!(apply_color_variant("nm001b01.tga", 2), "nm001b03.tga");

        net.mode = "leader".into();
        assert_eq!(network_skin_color_index(&net, true, None), 0);
        assert_eq!(network_skin_color_index(&net, false, Some(&remote)), 0);
    }

    #[test]
    fn origin_p0_team_body_variants_all_resolve_from_ptexture() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let path = root.join("ptexture.pak");
        if !path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let pak = PakArchive::from_bytes(&std::fs::read(path).expect("ptexture.pak"))
            .expect("ptexture archive");
        for color_index in 0..=2 {
            let name = apply_color_variant("nm001b01.tga", color_index);
            assert!(
                pak_read_ci(&pak, "ptexture", &name).is_some(),
                "missing team skin {name}"
            );
        }
    }

    #[test]
    fn gmc_material_modes_match_web_weapon_rules() {
        let normal = meteor_asset::gmc::GmcShader {
            texture_mode: "NORMAL".into(),
            opacity: 1.0,
            ..default()
        };
        assert_eq!(classify_gmc_shader(&normal), (GmdlMode::Opaque, 1.0, 0.0));

        let color_key = meteor_asset::gmc::GmcShader {
            texture_mode: "COLORKEY".into(),
            opacity: 1.0,
            ..default()
        };
        let (mode, opacity, cutoff) = classify_gmc_shader(&color_key);
        assert_eq!(mode, GmdlMode::Opaque);
        assert_eq!(opacity, 1.0);
        assert!((cutoff - 127.0 / 255.0).abs() < 1e-5);

        let additive_alpha = meteor_asset::gmc::GmcShader {
            texture_mode: "ALPHACHANNEL".into(),
            blend_enabled: true,
            blend_source: "sa".into(),
            blend_destination: "1".into(),
            opacity: 0.75,
            ..default()
        };
        assert_eq!(
            classify_gmc_shader(&additive_alpha),
            (GmdlMode::Add, 0.75, 0.0)
        );
        assert_eq!(
            weapon_texture_alpha(Some("AItem0902.jpg"), "ALPHACHANNEL"),
            WeaponTextureAlpha::Companion
        );
    }

    #[test]
    fn origin_weapon_alpha_assets_are_composited_like_web() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let patch_path = root.join("TexturePatch.pak");
        let ctexture_path = root.join("ctexture.pak");
        if !patch_path.exists() || !ctexture_path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let patch = PakArchive::from_bytes(&std::fs::read(patch_path).expect("TexturePatch.pak"))
            .expect("TexturePatch archive");
        let ctexture = PakArchive::from_bytes(&std::fs::read(ctexture_path).expect("ctexture.pak"))
            .expect("ctexture archive");

        let mut item_color = meteor_asset::texture::decode_image(
            &pak_read_ci(&patch, "TexturePatch", "AItem0902.jpg").expect("AItem0902.jpg color"),
        )
        .expect("decode AItem0902.jpg");
        let item_alpha = meteor_asset::texture::decode_image(
            &pak_read_ci(&patch, "TexturePatch", "AItem0902.bmp").expect("AItem0902.bmp alpha"),
        )
        .expect("decode AItem0902.bmp");
        assert!(apply_companion_alpha(&mut item_color, &item_alpha));
        assert!(item_color.rgba.chunks_exact(4).any(|pixel| pixel[3] < 255));
        assert!(item_color.rgba.chunks_exact(4).any(|pixel| pixel[3] > 0));

        let mut keyed = meteor_asset::texture::decode_image(
            &pak_read_ci(&ctexture, "ctexture", "kmwpn03.jpg").expect("kmwpn03.jpg color-key map"),
        )
        .expect("decode kmwpn03.jpg");
        crate::plugins::pak_textures::punch_color_key(&mut keyed.rgba, 8, 26);
        assert!(keyed.rgba.chunks_exact(4).any(|pixel| pixel[3] == 0));
        assert!(keyed.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));
    }

    #[test]
    fn gmb_weapon_texture_uses_authored_companion_alpha() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let assets = load_character_asset_for_test(&[root], "p0").expect("original p0 assets");
        let mut images = Assets::<Image>::default();
        let handle = load_weapon_texture(
            &mut images,
            &assets,
            "AItem0902.jpg",
            WeaponTextureAlpha::Luminance,
        )
        .unwrap();
        let bytes = resolve_weapon_texture_exact(&assets, "AItem0902.bmp").unwrap();
        let alpha = meteor_asset::texture::decode_image(&bytes).unwrap();
        let actual: Vec<_> = images
            .get(&handle)
            .unwrap()
            .data
            .as_ref()
            .unwrap()
            .chunks_exact(4)
            .map(|p| p[3])
            .collect();
        let expected: Vec<_> = alpha.rgba.chunks_exact(4).map(|p| p[0]).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn gmb_weapon_material_uses_shared_gmdl_classifier() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let path = root.join("cmodel/Item0902.gmb");
        if !path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let mesh = meteor_asset::gmdl::decode_gmdl(&std::fs::read(path).expect("Item0902.gmb"))
            .expect("decode Item0902.gmb");
        let material = mesh.materials.first().expect("Item0902 material 0");
        assert_eq!(material.blend, "ALPHACHANNEL");
        assert_eq!(
            crate::plugins::render_plugin::classify_gmdl_material(material).0,
            GmdlMode::Add
        );
        assert_eq!(
            gmdl_weapon_texture_alpha(material),
            WeaponTextureAlpha::Luminance
        );
    }

    fn fighter_with_same_class_slots() -> crate::components::fighter::Fighter {
        crate::components::fighter::Fighter {
            character_id: 0,
            player_slot: 0,
            weapon: 6,
            weapon_slots: [6, 6],
            weapon_slot: 0,
            weapon_slot_variants: [0, 2],
            weapon_variant: 0,
        }
    }

    fn weapon_marker(
        slot: u8,
        variant: u8,
        forms: Option<Vec<u8>>,
    ) -> crate::components::fighter::WeaponModel {
        crate::components::fighter::WeaponModel {
            fighter: Entity::from_raw_u32(1).expect("valid test entity index"),
            weapon: 6,
            slot,
            variant,
            forms,
        }
    }

    #[test]
    fn mission_item_ids_preserve_weapon_class_and_refinement_variant() {
        assert_eq!(weapon_slot_from_item(0), None);
        assert_eq!(weapon_slot_from_item(5), Some((5, 0)));
        assert_eq!(weapon_slot_from_item(1), Some((1, 0)));
        assert_eq!(weapon_slot_from_item(24), Some((6, 2)));
        assert_eq!(weapon_slot_from_item(21), Some((3, 2)));
        assert_eq!(weapon_slot_from_item(47), Some((10, 0)));
        assert_eq!(weapon_slot_from_item(51), Some((11, 0)));
        assert_eq!(weapon_slot_from_item(55), Some((12, 0)));
        assert_eq!(weapon_slot_from_item(59), Some((2, 5)));
        assert_eq!(weapon_slot_from_item(60), None);
    }

    #[test]
    fn preheated_weapon_visibility_isolated_by_slot_variant_and_form() {
        let mut fighter = fighter_with_same_class_slots();
        let primary = weapon_marker(0, 0, Some(vec![0]));
        let secondary = weapon_marker(1, 2, Some(vec![1]));

        assert!(weapon_part_is_visible(&fighter, 0, &primary, false));
        assert!(!weapon_part_is_visible(&fighter, 0, &secondary, false));
        assert!(weapon_marker_matches_inventory(&fighter, &primary));
        assert!(weapon_marker_matches_inventory(&fighter, &secondary));
        assert!(!weapon_marker_matches_inventory(
            &fighter,
            &weapon_marker(1, 1, None)
        ));
        assert!(matches!(
            initial_weapon_part_visibility(&secondary, fighter.weapon_slot, 0),
            Visibility::Hidden
        ));

        fighter.weapon_slot = 1;
        fighter.weapon_variant = 2;
        assert!(!weapon_part_is_visible(&fighter, 1, &primary, false));
        assert!(weapon_part_is_visible(&fighter, 1, &secondary, false));
        assert!(!weapon_part_is_visible(&fighter, 0, &secondary, false));
    }

    #[test]
    fn flying_wheel_hides_only_the_active_wheel_part() {
        let fighter = crate::components::fighter::Fighter {
            weapon: 2,
            weapon_slots: [2, 5],
            ..fighter_with_same_class_slots()
        };
        let wheel = crate::components::fighter::WeaponModel {
            weapon: 2,
            ..weapon_marker(0, 0, None)
        };
        assert!(weapon_part_is_visible(&fighter, 0, &wheel, false));
        assert!(!weapon_part_is_visible(&fighter, 0, &wheel, true));
    }

    #[test]
    fn staff_blade_sheathing_visibility_tracks_live_pose_and_active_slot() {
        use crate::components::fighter::{PoseRuntime, WeaponForm, WeaponModel};
        let mut app = App::new();
        app.add_systems(Update, sync_weapon_form_visibility);
        let fighter = app
            .world_mut()
            .spawn((
                crate::components::fighter::Fighter {
                    weapon: 10,
                    weapon_slots: [10, 10],
                    ..fighter_with_same_class_slots()
                },
                WeaponForm { current: 2 },
                PoseRuntime::idle(),
            ))
            .id();
        let parts: Vec<_> = [0, 1]
            .into_iter()
            .flat_map(|slot| {
                weapon_part_specs(10, 0)
                    .into_iter()
                    .map(move |spec| (slot, spec))
            })
            .map(|(slot, spec)| {
                let marker = WeaponModel {
                    fighter,
                    weapon: 10,
                    slot,
                    variant: 0,
                    forms: spec.forms,
                };
                let id = app.world_mut().spawn((marker, Visibility::Visible)).id();
                (id, slot, spec.stem)
            })
            .collect();
        for pose in [431, 485, 486, 403, 520, 522, 524, 451, 452, 456, 487] {
            app.world_mut()
                .get_mut::<PoseRuntime>(fighter)
                .unwrap()
                .pose_id = pose;
            app.update();
            let combined = matches!(pose, 486 | 403 | 520 | 524 | 451 | 487);
            for (id, slot, stem) in &parts {
                let expected = *slot == 0
                    && if combined {
                        stem.ends_with('b')
                    } else {
                        stem.ends_with('r') || stem.ends_with('l')
                    };
                assert_eq!(
                    *app.world().get::<Visibility>(*id).unwrap() == Visibility::Visible,
                    expected,
                    "pose {pose}, slot {slot}, part {stem}"
                );
            }
            assert_eq!(app.world().get::<WeaponForm>(fighter).unwrap().current, 2);
        }
    }

    #[test]
    fn weapon_part_specs_match_web_dual_and_staff_loadouts() {
        let stinger = weapon_part_specs(4, 3);
        assert_eq!(stinger.len(), 2);
        assert_eq!(stinger[0].stem, "w8_3r");
        assert!(!stinger[0].left);
        assert_eq!(stinger[1].stem, "w8_3l");
        assert!(stinger[1].left);

        let staff = weapon_part_specs(10, 2);
        assert_eq!(staff.len(), 4);
        assert_eq!(staff[0].stem, "w9_2a");
        assert_eq!(staff[0].forms.as_deref(), Some(&[1][..]));
        assert_eq!(staff[1].forms.as_deref(), Some(&[2][..]));
        assert_eq!(staff[2].forms.as_deref(), Some(&[0][..]));
        assert_eq!(staff[3].forms.as_deref(), Some(&[0][..]));

        let fist = weapon_part_specs(11, 1);
        assert_eq!(fist.len(), 2);
        assert_eq!(fist[0].stem, "w10_1r");
        assert_eq!(fist[1].stem, "w10_1l");
    }

    #[test]
    fn weapon_resource_read_is_case_insensitive_per_path_segment() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let bytes = read_first(&[root], "CMODEL/w10_0R.GMB").expect("W10_0r.gmb");
        assert!(bytes.starts_with(b"GMDL V1.00"));
    }

    #[test]
    fn color_variant_default_keeps_b01() {
        assert_eq!(apply_color_variant("nm001b01.tga", 0), "nm001b01.tga");
        assert_eq!(apply_color_variant("nm001b05.tga", 0), "nm001b01.tga");
    }

    #[test]
    fn color_variant_cold_and_red() {
        assert_eq!(apply_color_variant("nm001b01.tga", 1), "nm001b02.tga");
        assert_eq!(apply_color_variant("nm001b01.tga", 2), "nm001b03.tga");
    }

    #[test]
    fn color_variant_untouched_without_b0() {
        assert_eq!(apply_color_variant("face01.tga", 2), "face01.tga");
        assert_eq!(apply_color_variant("nm001c01.tga", 1), "nm001c01.tga");
    }

    #[test]
    fn color_variant_digit_boundary() {
        assert_eq!(apply_color_variant("x.b0", 1), "x.b0");
    }

    #[test]
    fn entry_blend_duration_defaults_to_0_12() {
        assert!((entry_blend_duration(None, None, 999) - 0.12).abs() < 1e-6);

        assert!((entry_blend_duration(None, Some(0), 999) - 0.12).abs() < 1e-6);
    }

    #[test]
    fn entry_blend_duration_fast_strategy_pose_is_0_01() {
        assert!((entry_blend_duration(None, Some(0), 60) - 0.01).abs() < 1e-6);
        assert!((entry_blend_duration(None, Some(0), 82) - 0.01).abs() < 1e-6);

        assert!((entry_blend_duration(None, Some(0), 61 + 1000) - 0.12).abs() < 1e-6);
    }

    #[test]
    fn entry_blend_duration_authored_wins_over_fast() {
        let d = entry_blend_duration(Some(0.25), Some(0), 60);
        assert!((d - 0.25).abs() < 1e-6);
    }

    #[test]
    fn entry_blend_duration_authored_capped_at_0_6() {
        let d = entry_blend_duration(Some(5.0), None, 0);
        assert!((d - 0.6).abs() < 1e-6);
    }

    #[test]
    fn entry_blend_duration_non_positive_authored_falls_through() {
        assert!((entry_blend_duration(Some(0.0), None, 0) - 0.12).abs() < 1e-6);
        assert!((entry_blend_duration(Some(-1.0), None, 0) - 0.12).abs() < 1e-6);
        assert!((entry_blend_duration(Some(f32::NAN), None, 0) - 0.12).abs() < 1e-6);
        assert!((entry_blend_duration(Some(f32::INFINITY), None, 0) - 0.12).abs() < 1e-6);
    }

    fn minimal_pose_frames_and_poses(
        pose_id: u16,
        start: u32,
        end: u32,
    ) -> (
        std::collections::BTreeMap<u16, (u32, u32)>,
        std::collections::BTreeMap<u16, meteor_asset::pos::Pose>,
    ) {
        let mut frames = std::collections::BTreeMap::new();
        frames.insert(pose_id, (start, end));
        let mut poses = std::collections::BTreeMap::new();
        poses.insert(
            pose_id,
            meteor_asset::pos::Pose {
                id: pose_id,
                source: 1,
                start,
                end,
                loop_start: start,
                loop_end: end,
                loops: false,
                link: None,
                note: String::new(),
                effect_type: None,
                effect_id: None,
                attacks: vec![],
                next_pose: None,
                blends: vec![],
                actions: vec![],
                action_entry_frame: start,
                drags: vec![],
            },
        );
        (frames, poses)
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_with(
        pose: &mut crate::components::fighter::PoseRuntime,
        blend: &mut crate::components::fighter::EntryBlend,
        pose_frames: &std::collections::BTreeMap<u16, (u32, u32)>,
        poses: &std::collections::BTreeMap<u16, meteor_asset::pos::Pose>,
        pose_id: u16,
        hard: bool,
        pending_blend_seconds: Option<f32>,
        kind: crate::components::fighter::EntryBlendKind,
        skip_blend: bool,
        force_blend: bool,
        strategy_id: Option<u8>,
    ) {
        apply_pose_transition(
            pose,
            blend,
            pose_frames,
            poses,
            pose_id,
            hard,
            pending_blend_seconds,
            kind,
            skip_blend,
            force_blend,
            strategy_id,
        );
    }

    #[test]
    fn production_pose_transition_triggers_default_blend_on_hard_switch() {
        let (frames, poses) = minimal_pose_frames_and_poses(5, 10, 20);
        let mut pose = crate::components::fighter::PoseRuntime::idle();
        let mut blend = crate::components::fighter::EntryBlend::default();
        apply_with(
            &mut pose,
            &mut blend,
            &frames,
            &poses,
            5,
            true,
            None,
            crate::components::fighter::EntryBlendKind::Generic,
            false,
            false,
            None,
        );
        assert!((blend.duration - 0.12).abs() < 1e-6);
        assert_eq!(blend.pose_id, 5);
        assert_eq!(blend.to_frame, 10.0);
        assert_eq!(pose.frame, 10.0);
        assert_eq!(pose.pose_instance_id, 1);
    }

    #[test]
    fn production_pose_transition_jump_hard_cut_skips_blend() {
        let (frames, poses) = minimal_pose_frames_and_poses(151, 0, 30);
        let mut pose = crate::components::fighter::PoseRuntime::idle();
        let mut blend = crate::components::fighter::EntryBlend::default();
        apply_with(
            &mut pose,
            &mut blend,
            &frames,
            &poses,
            151,
            true,
            None,
            crate::components::fighter::EntryBlendKind::Generic,
            true,
            false,
            None,
        );
        assert_eq!(blend.duration, 0.0);
        assert_eq!(pose.pose_id, 151);
        assert_eq!(pose.pose_instance_id, 1);
    }

    #[test]
    fn production_pose_transition_same_pose_hard_reset_retriggers() {
        let (frames, poses) = minimal_pose_frames_and_poses(3, 0, 10);
        let mut pose = crate::components::fighter::PoseRuntime::idle();
        pose.pose_id = 3;
        pose.frame = 7.0;
        let mut blend = crate::components::fighter::EntryBlend::default();
        apply_with(
            &mut pose,
            &mut blend,
            &frames,
            &poses,
            3,
            true,
            None,
            crate::components::fighter::EntryBlendKind::Generic,
            false,
            false,
            None,
        );
        assert!((blend.duration - 0.12).abs() < 1e-6);
        assert_eq!(pose.frame, 0.0);
        assert_eq!(pose.pose_instance_id, 1);
    }

    #[test]
    fn production_pose_transition_soft_switch_preserves_relative_cursor() {
        let (mut frames, mut poses) = minimal_pose_frames_and_poses(8, 100, 140);
        let (target_frames, target_poses) = minimal_pose_frames_and_poses(9, 200, 240);
        frames.extend(target_frames);
        poses.extend(target_poses);
        let mut pose = crate::components::fighter::PoseRuntime {
            pose_id: 8,
            frame: 112.5,
            start: 100,
            end: 140,
            loop_start: 100,
            loop_end: 140,
            loops: false,
            pose_instance_id: 4,
        };
        let mut blend = crate::components::fighter::EntryBlend::default();
        apply_with(
            &mut pose,
            &mut blend,
            &frames,
            &poses,
            9,
            false,
            None,
            crate::components::fighter::EntryBlendKind::Generic,
            false,
            false,
            None,
        );
        assert_eq!(blend.duration, 0.0);
        assert_eq!(pose.frame, 212.5);
        assert_eq!(pose.pose_instance_id, 5);
    }

    #[test]
    fn production_pose_transition_force_overrides_skip_once() {
        let (frames, poses) = minimal_pose_frames_and_poses(0, 10, 20);
        let mut pose = crate::components::fighter::PoseRuntime::idle();
        let mut blend = crate::components::fighter::EntryBlend::default();
        apply_with(
            &mut pose,
            &mut blend,
            &frames,
            &poses,
            0,
            true,
            None,
            crate::components::fighter::EntryBlendKind::Generic,
            true,
            true,
            None,
        );
        assert!((blend.duration - 0.12).abs() < 1e-6);
        assert_eq!(pose.frame, 10.0);
    }

    #[test]
    fn production_pose_transition_chained_targets_action_entry_only() {
        let (frames, mut poses) = minimal_pose_frames_and_poses(12, 10, 30);
        poses.get_mut(&12).unwrap().action_entry_frame = 17;
        let mut pose = crate::components::fighter::PoseRuntime::idle();
        let mut blend = crate::components::fighter::EntryBlend::default();
        apply_with(
            &mut pose,
            &mut blend,
            &frames,
            &poses,
            12,
            true,
            Some(0.25),
            crate::components::fighter::EntryBlendKind::Chained,
            false,
            false,
            None,
        );
        assert_eq!(blend.to_frame, 17.0);
        assert_eq!(pose.frame, 17.0);
        assert!((blend.duration - 0.25).abs() < 1e-6);

        apply_with(
            &mut pose,
            &mut blend,
            &frames,
            &poses,
            12,
            true,
            Some(0.25),
            crate::components::fighter::EntryBlendKind::Generic,
            false,
            false,
            None,
        );
        assert_eq!(blend.to_frame, 10.0);
        assert_eq!(pose.frame, 10.0);
    }

    #[test]
    fn entry_blend_wall_clock_elapsed_ignores_frame_delta_caps() {
        assert_eq!(entry_blend_wall_elapsed(None, 100.0), 0.0);
        assert!((entry_blend_wall_elapsed(Some(10.0), 10.5) - 0.5).abs() < 1e-6);
        assert_eq!(entry_blend_wall_elapsed(Some(11.0), 10.5), 0.0);
    }

    #[test]
    fn entry_blend_root_dummy_and_quat_interpolation_matches_web_math() {
        assert_eq!(
            blend_vec3([0.0, 2.0, 4.0], [10.0, 6.0, 0.0], 0.5),
            [5.0, 4.0, 2.0]
        );

        let target = -Quat::from_rotation_y(0.4);
        let actual = blend_quat([0.0, 0.0, 0.0, 1.0], target, 0.5);
        let expected = Quat::from_rotation_y(0.2);
        assert!(actual.dot(expected).abs() > 0.9999);
    }

    #[test]
    fn concealment_blends_skin_at_point_one_and_restores_alpha_test() {
        let mut material = GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.5,
                mode: GmdlMode::Opaque.as_u32(),
                has_texture: 0,
                uv_offset: Vec2::ZERO,
            },
            None,
            false,
            GmdlMode::Opaque,
        );
        apply_skin_concealment_material(&mut material, 0.5, true);
        assert_eq!(material.params.opacity, 0.1);
        assert_eq!(material.params.alpha_cutoff, 0.0);
        assert_eq!(material.mode, GmdlMode::Blend);

        apply_skin_concealment_material(&mut material, 0.5, false);
        assert_eq!(material.params.opacity, 1.0);
        assert_eq!(material.params.alpha_cutoff, 0.5);
        assert_eq!(material.mode, GmdlMode::Opaque);
    }
}

#[cfg(test)]
pub(crate) fn spawn_live_portrait_fixture(
    mut commands: Commands,
    fighters: Query<(Entity, &crate::components::fighter::Fighter)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut inverse_binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let roots = [std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res")];
    let assets = load_character_asset_for_test(&roots, "p1").expect("original p1 assets");
    for (entity, fighter) in &fighters {
        spawn_fighter_model_for(
            &mut commands,
            entity,
            fighter,
            0,
            "p1",
            &assets,
            0,
            &mut meshes,
            &mut materials,
            &mut images,
            &mut inverse_binds,
        );
    }
}
#[cfg(test)]
pub(crate) fn animate_live_portrait_fixture(
    time: Res<Time>,
    mut bones: Query<(&FighterBone, &mut Transform)>,
    mut skins: Query<&mut Visibility, With<SkinnedMesh>>,
) {
    for (bone, mut transform) in &mut bones {
        if bone.name == "bau_Head" {
            transform.rotation = Quat::from_rotation_z((time.elapsed_secs() * 1.5).sin() * 0.12);
        }
    }
    if time.elapsed_secs() > 0.15 {
        for mut visibility in &mut skins {
            *visibility = Visibility::Inherited;
        }
    }
}
