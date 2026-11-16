use super::character_plugin::{
    model_bones, FighterBone, FighterModel, FighterSkeletonQuery, FighterSkin,
};
use super::network_plugin::{NetState, NetStatus};
use super::stage_material::{GmdlMaterial, GmdlMaterialParams, GmdlMode};
use crate::components::fighter::{Health, MotionState, PoseRuntime};
use bevy::prelude::*;
use bevy::render::mesh::skinning::SkinnedMesh;

#[derive(Component)]
pub(crate) struct BalanceGlow {
    pub armor_pose: Option<u16>,
    pub remaining: f32,
    seen_pose: bool,
}
impl BalanceGlow {
    pub fn armor(pose: u16) -> Self {
        Self {
            armor_pose: Some(pose),
            remaining: 4.0,
            seen_pose: false,
        }
    }
    pub fn immunity() -> Self {
        Self {
            armor_pose: None,
            remaining: 0.45,
            seen_pose: false,
        }
    }
}

fn glow_tint(glow: &BalanceGlow) -> Vec4 {
    if glow.armor_pose.is_some() {
        Vec4::new(1.0, 0.035, 0.015, 0.9)
    } else {
        Vec4::new(0.025, 0.25, 1.0, 0.9)
    }
}

fn glow_style(glow: &BalanceGlow) -> Vec4 {
    if glow.armor_pose.is_some() {
        Vec4::new(1.3, 4.0, 3.0, 1.7)
    } else {
        Vec4::new(3.2, 6.0, 5.0, 3.4)
    }
}

#[derive(Component)]
pub(crate) struct BalanceAfterimageJoint;

const AFTERIMAGE_OPACITY: f32 = 0.5;
const AFTERIMAGE_GLOW_SCALE: f32 = 0.6;
const AFTERIMAGE_BACKWARD: f32 = 0.32;
const AFTERIMAGE_TRAIL: f32 = 0.10;
const AFTERIMAGE_MAX: f32 = 0.9;
const AFTERIMAGE_SMOOTH: f32 = 8.0;

#[derive(Component)]
pub(crate) struct BalanceAfterimage {
    pub(crate) source_joints: Vec<Entity>,
    pub(crate) ghost_joints: Vec<Entity>,
    pub(crate) ghost_meshes: Vec<Entity>,
    pub(crate) materials: Vec<Handle<GmdlMaterial>>,
    pub(crate) last_anchor: Vec3,
    pub(crate) offset: Vec3,
    pub(crate) tint: Vec4,
    pub(crate) style: Vec4,

    revealed: bool,
}

pub(crate) struct RebalancePlugin;
impl Plugin for RebalancePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            (
                sync_glow.before(bevy::transform::TransformSystems::Propagate),
                sync_afterimage.after(bevy::transform::TransformSystems::Propagate),
            ),
        );
    }
}

pub(super) fn sync_glow(
    mut commands: Commands,
    time: Res<Time>,
    net: Res<NetState>,
    state: Res<State<crate::app_state::AppState>>,
    mut actors: Query<(Entity, &Health, &PoseRuntime, &mut BalanceGlow)>,
    skins: Query<(&FighterSkin, &MeshMaterial3d<GmdlMaterial>)>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    let mut active = std::collections::HashMap::new();
    for (entity, health, pose, mut glow) in &mut actors {
        if *state.get() != crate::app_state::AppState::Paused {
            glow.remaining -= time.delta_secs();
        }
        let pose_ended = if let Some(expected) = glow.armor_pose {
            let ended = glow.seen_pose && pose.pose_id != expected;
            glow.seen_pose |= pose.pose_id == expected;
            if glow.seen_pose && !ended {
                glow.remaining = 4.0;
            }
            ended
        } else {
            false
        };
        if health.current <= 0.0
            || glow.remaining <= 0.0
            || pose_ended
            || net.status != NetStatus::InRoom
            || !net.battle_rules.enabled()
        {
            commands.entity(entity).remove::<BalanceGlow>();
            continue;
        }
        active.insert(entity, (glow_tint(&glow), glow_style(&glow)));
    }

    for (skin, handle) in &skins {
        let desired = active
            .get(&skin.fighter)
            .copied()
            .unwrap_or((Vec4::ZERO, Vec4::ZERO));
        if materials
            .get(&handle.0)
            .is_some_and(|m| m.body_glow != desired.0 || m.body_style != desired.1)
        {
            let mut material = materials.get_mut(&handle.0).unwrap();
            material.body_glow = desired.0;
            material.body_style = desired.1;
        }
    }
}

pub(super) fn sync_afterimage(
    mut commands: Commands,
    time: Res<Time>,
    state: Res<State<crate::app_state::AppState>>,
    net: Res<NetState>,
    fighters: Query<(&BalanceGlow, &MotionState)>,
    models: Query<(Entity, &FighterModel, &GlobalTransform), Without<BalanceAfterimageJoint>>,
    skins: Query<(
        &FighterSkin,
        &Mesh3d,
        &MeshMaterial3d<GmdlMaterial>,
        &SkinnedMesh,
        &ChildOf,
    )>,
    skeletons: FighterSkeletonQuery,
    bones: Query<&FighterBone>,
    mut hosts: Query<(Entity, &mut BalanceAfterimage)>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    sources: Query<&GlobalTransform, Without<BalanceAfterimageJoint>>,
    mut ghost_joints: Query<&mut GlobalTransform, With<BalanceAfterimageJoint>>,
) {
    let paused = *state.get() == crate::app_state::AppState::Paused;
    let dt = if paused {
        0.0
    } else {
        time.delta_secs().min(0.1)
    };
    let battle_on = net.status == NetStatus::InRoom && net.battle_rules.enabled();

    for (model, mut host) in &mut hosts {
        let glow = models
            .get(model)
            .ok()
            .and_then(|(_, model, _)| fighters.get(model.fighter).ok())
            .filter(|_| battle_on);
        let Some((glow, motion)) = glow else {
            despawn_afterimage(&mut commands, model, &host);
            continue;
        };
        update_afterimage(
            &mut host,
            glow,
            motion,
            models
                .get(model)
                .map(|(_, _, tf)| tf.translation())
                .unwrap_or(Vec3::ZERO),
            dt,
            &sources,
            &mut ghost_joints,
            &mut materials,
        );

        if !host.revealed
            && host
                .ghost_joints
                .iter()
                .all(|joint| ghost_joints.contains(*joint))
        {
            for mesh in &host.ghost_meshes {
                commands.entity(*mesh).insert(Visibility::Visible);
            }
            host.revealed = true;
        }
    }

    if !battle_on {
        return;
    }
    for (model, model_component, model_transform) in &models {
        if hosts.contains(model) {
            continue;
        }
        let Ok((glow, _)) = fighters.get(model_component.fighter) else {
            continue;
        };
        let sources_of_model: Vec<_> = skins
            .iter()
            .filter(|(skin, _, _, _, parent)| {
                skin.fighter == model_component.fighter && parent.parent() == model
            })
            .collect();
        let Some((_, _, _, first_skin, _)) = sources_of_model.first().copied() else {
            continue;
        };

        let mut ordered: Vec<(usize, Entity)> = model_bones(model, &skeletons)
            .iter()
            .filter_map(|entity| {
                bones
                    .get(*entity)
                    .ok()
                    .map(|bone| (bone.skin_index, *entity))
            })
            .filter(|(skin_index, _)| *skin_index != usize::MAX)
            .collect();
        ordered.sort_by_key(|(skin_index, _)| *skin_index);
        let mut source_joints: Vec<Entity> =
            ordered.into_iter().map(|(_, entity)| entity).collect();
        if source_joints.len() == 30 && first_skin.joints.len() == 38 {
            source_joints.extend_from_slice(&first_skin.joints[30..]);
        }
        if source_joints.len() != first_skin.joints.len() {
            source_joints = first_skin.joints.clone();
        }
        if source_joints.is_empty() {
            continue;
        }
        let tint = glow_tint(glow);
        let style = glow_style(glow);
        let mut ghost_joint_entities = Vec::with_capacity(source_joints.len());
        for source in &source_joints {
            let pose = sources
                .get(*source)
                .copied()
                .unwrap_or(GlobalTransform::IDENTITY);
            ghost_joint_entities.push(
                commands
                    .spawn((BalanceAfterimageJoint, pose, ChildOf(model)))
                    .id(),
            );
        }
        let mut ghost_meshes = Vec::with_capacity(sources_of_model.len());
        let mut ghost_materials = Vec::with_capacity(sources_of_model.len());
        for (_, mesh, material, skin, _) in sources_of_model {
            let source = materials.get(&material.0).cloned();
            let handle = materials.add(ghost_material(source.as_ref(), tint, style));
            ghost_materials.push(handle.clone());
            ghost_meshes.push(
                commands
                    .spawn((
                        Mesh3d(mesh.0.clone()),
                        MeshMaterial3d(handle),
                        SkinnedMesh {
                            inverse_bindposes: skin.inverse_bindposes.clone(),
                            joints: ghost_joint_entities.clone(),
                        },
                        Visibility::Hidden,
                        InheritedVisibility::default(),
                        Transform::IDENTITY,
                        ChildOf(model),
                    ))
                    .id(),
            );
        }

        commands.entity(model).insert(BalanceAfterimage {
            source_joints,
            ghost_joints: ghost_joint_entities,
            ghost_meshes,
            materials: ghost_materials,
            last_anchor: model_transform.translation(),
            offset: Vec3::ZERO,
            tint,
            style,
            revealed: false,
        });
    }
}

fn ghost_material(source: Option<&GmdlMaterial>, tint: Vec4, style: Vec4) -> GmdlMaterial {
    let ghost_tint = Vec4::new(
        tint.x * AFTERIMAGE_GLOW_SCALE,
        tint.y * AFTERIMAGE_GLOW_SCALE,
        tint.z * AFTERIMAGE_GLOW_SCALE,
        tint.w * AFTERIMAGE_GLOW_SCALE,
    );
    match source.cloned() {
        Some(mut material) => {
            material.params.opacity = AFTERIMAGE_OPACITY;
            material.params.alpha_cutoff = 0.0;
            material.params.mode = GmdlMode::Blend.as_u32();
            material.mode = GmdlMode::Blend;
            material.body_glow = ghost_tint;
            material.body_style = style;
            material.camera_occlusion_enabled = false;
            material.weapon_refraction_source = false;
            material
        }
        None => {
            let mut material = GmdlMaterial::new(
                GmdlMaterialParams {
                    opacity: AFTERIMAGE_OPACITY,
                    alpha_cutoff: 0.0,
                    mode: GmdlMode::Blend.as_u32(),
                    has_texture: 0,
                    uv_offset: Vec2::ZERO,
                },
                None,
                false,
                GmdlMode::Blend,
            );
            material.body_glow = ghost_tint;
            material.body_style = style;
            material
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn update_afterimage(
    host: &mut BalanceAfterimage,
    glow: &BalanceGlow,
    motion: &MotionState,
    anchor: Vec3,
    dt: f32,
    sources: &Query<&GlobalTransform, Without<BalanceAfterimageJoint>>,
    ghost_joints: &mut Query<&mut GlobalTransform, With<BalanceAfterimageJoint>>,
    materials: &mut Assets<GmdlMaterial>,
) {
    let tint = glow_tint(glow);
    let style = glow_style(glow);
    if tint != host.tint || style != host.style {
        for handle in &host.materials {
            if let Some(mut material) = materials.get_mut(handle) {
                material.body_glow = Vec4::new(
                    tint.x * AFTERIMAGE_GLOW_SCALE,
                    tint.y * AFTERIMAGE_GLOW_SCALE,
                    tint.z * AFTERIMAGE_GLOW_SCALE,
                    tint.w * AFTERIMAGE_GLOW_SCALE,
                );
                material.body_style = style;
            }
        }
        host.tint = tint;
        host.style = style;
    }

    let mut delta = anchor - host.last_anchor;
    if !delta.is_finite() {
        delta = Vec3::ZERO;
    }
    host.last_anchor = anchor;
    let velocity = if dt > 0.0 {
        (delta / dt).clamp_length_max(8.0)
    } else {
        Vec3::ZERO
    };

    let backward = Vec3::new(-motion.facing.sin(), 0.0, -motion.facing.cos()) * AFTERIMAGE_BACKWARD;
    let target = (backward - velocity * AFTERIMAGE_TRAIL).clamp_length_max(AFTERIMAGE_MAX);
    let smoothing = if dt > 0.0 {
        1.0 - (-AFTERIMAGE_SMOOTH * dt).exp()
    } else {
        0.0
    };
    host.offset = host.offset.lerp(target, smoothing);

    let translation = Mat4::from_translation(host.offset);
    for (source, ghost) in host.source_joints.iter().zip(&host.ghost_joints) {
        let Ok(pose) = sources.get(*source) else {
            continue;
        };
        if let Ok(mut target) = ghost_joints.get_mut(*ghost) {
            *target = GlobalTransform::from(translation * pose.to_matrix());
        }
    }
}

fn despawn_afterimage(commands: &mut Commands, model: Entity, host: &BalanceAfterimage) {
    for entity in host.ghost_joints.iter().chain(&host.ghost_meshes) {
        commands.entity(*entity).despawn();
    }
    commands.entity(model).remove::<BalanceAfterimage>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebalance_glow_ends_with_pose_or_room_without_spawning_geometry() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .insert_resource(State::new(crate::app_state::AppState::Playing))
            .insert_resource(NetState {
                status: NetStatus::InRoom,
                battle_rules: meteor_combat::battle_rules::BattleRules::bundled(),
                ..Default::default()
            })
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .add_systems(Update, sync_glow);
        let mut pose = PoseRuntime::idle();
        pose.pose_id = 302;
        let actor = app
            .world_mut()
            .spawn((Health::default(), pose, BalanceGlow::armor(302)))
            .id();
        app.update();
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            0
        );
        app.world_mut()
            .get_mut::<PoseRuntime>(actor)
            .unwrap()
            .pose_id = 0;
        app.update();
        assert!(app.world().get::<BalanceGlow>(actor).is_none());
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            0
        );
        app.world_mut()
            .entity_mut(actor)
            .insert(BalanceGlow::immunity());
        app.update();
        app.world_mut().entity_mut(actor).remove::<BalanceGlow>();
        app.update();
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            0
        );
        app.world_mut()
            .entity_mut(actor)
            .insert(BalanceGlow::immunity());
        app.world_mut().resource_mut::<NetState>().battle_rules = Default::default();
        app.update();
        assert!(app.world().get::<BalanceGlow>(actor).is_none());
    }

    #[test]
    fn glow_tint_keeps_armor_red_and_immunity_blue() {
        let armor = glow_tint(&BalanceGlow::armor(302));
        assert!(armor.x > armor.z && armor.w > 0.0);
        let immunity = glow_tint(&BalanceGlow::immunity());
        assert!(immunity.z > immunity.x && immunity.w > 0.0);
    }

    #[test]
    fn glow_style_is_positive_and_slower_for_armor_than_immunity() {
        let armor = glow_style(&BalanceGlow::armor(302));
        let immunity = glow_style(&BalanceGlow::immunity());
        assert!(armor.x > 0.0 && immunity.x > 0.0);
        assert!(armor.y > 0.0 && immunity.y > 0.0);
        assert!(armor.z > 0.0 && immunity.z > 0.0);
        assert!(armor.w > 0.0 && immunity.w > 0.0);

        assert!(armor.x < immunity.x);
        assert!(armor.w < immunity.w);
    }

    #[test]
    fn ghost_material_is_translucent_and_keeps_the_source_texture() {
        let source = GmdlMaterial::new(
            GmdlMaterialParams {
                opacity: 1.0,
                alpha_cutoff: 0.5,
                mode: GmdlMode::Opaque.as_u32(),
                has_texture: 1,
                uv_offset: Vec2::ZERO,
            },
            None,
            false,
            GmdlMode::Opaque,
        );
        let tint = glow_tint(&BalanceGlow::armor(302));
        let style = glow_style(&BalanceGlow::armor(302));
        let ghost = ghost_material(Some(&source), tint, style);
        assert_eq!(ghost.mode, GmdlMode::Blend);
        assert_eq!(ghost.params.mode, GmdlMode::Blend.as_u32());
        assert_eq!(ghost.params.opacity, AFTERIMAGE_OPACITY);
        assert_eq!(ghost.params.alpha_cutoff, 0.0);
        assert!(ghost.body_glow.x > ghost.body_glow.z);
        assert!(ghost.body_glow.w > 0.0 && ghost.body_glow.w < tint.w);
        assert_eq!(ghost.body_style, style);
    }
}
