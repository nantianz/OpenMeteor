use super::{FighterModel, Skeleton, SkinnedMesh};
use crate::app_state::{AppState, Settings};
use crate::components::fighter::{FighterHitReaction, PoseRuntime};
use bevy::prelude::*;
use meteor_combat::hit_reaction::{FacingBucket, HitReactKind, DOWN_HOLD_POSES};

const DURATION: f32 = 0.24;
const MAX_ANGLE: f32 = 4.0 * std::f32::consts::PI / 180.0;
const DAMPING: f32 = 12.0;
const FREQUENCY: f32 = 28.0;

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct HitSwaySet;

#[derive(Component)]
struct RenderHitJoint;

struct RenderNode {
    source: Entity,
    render: Entity,
    parent: Option<usize>,
    weight: f32,
    correction: Mat4,
    lock_parent_correction: bool,
}

#[derive(Component)]
struct RenderHitSkeleton {
    nodes: Vec<RenderNode>,
    order: Vec<usize>,
    original_joints: Vec<Entity>,
    render_joints: Vec<Entity>,
    sway: Sway,
}

#[derive(Default)]
struct Sway {
    token: Option<u32>,
    was_airborne: bool,
    elapsed: f32,
    displacement: Vec2,
    velocity: Vec2,
}

impl Sway {
    fn envelope(&self) -> f32 {
        (1.0 - (self.elapsed / DURATION).powi(2)).max(0.0).powi(2)
    }

    fn clear_motion(&mut self) {
        self.elapsed = DURATION;
        self.displacement = Vec2::ZERO;
        self.velocity = Vec2::ZERO;
    }

    fn sample(
        &mut self,
        enabled: bool,
        dt: f32,
        pose: &PoseRuntime,
        reaction: &FighterHitReaction,
    ) -> Vec2 {
        let hit = reaction.current.as_ref().filter(|hit| {
            hit.kind == HitReactKind::BeHit
                && hit.pose_id == Some(pose.pose_id)
                && !DOWN_HOLD_POSES.contains(&pose.pose_id)
                && (pose.loops || pose.frame < pose.end as f32)
        });

        let token = hit.map(|_| pose.pose_instance_id);
        let new_hit = token.is_some() && token != self.token;
        let landed = self.was_airborne && !reaction.airborne;
        self.token = token;
        self.was_airborne = reaction.airborne;
        let Some(hit) = hit.filter(|_| enabled && !landed) else {
            self.clear_motion();
            return Vec2::ZERO;
        };
        if new_hit {
            let impulse = match hit.bucket {
                FacingBucket::Front | FacingBucket::Default => Vec2::X,
                FacingBucket::Back => Vec2::NEG_X,
                FacingBucket::Left => Vec2::Y,
                FacingBucket::Right => Vec2::NEG_Y,
            };
            let envelope = self.envelope();
            self.displacement *= envelope;
            self.velocity = (self.velocity * envelope + impulse * 2.8).clamp_length_max(4.0);
            self.elapsed = 0.0;
        }
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.elapsed += dt;
        if self.elapsed >= DURATION {
            self.clear_motion();
            return Vec2::ZERO;
        }

        let (sin, cos) = (FREQUENCY * dt).sin_cos();
        let b = (self.velocity + DAMPING * self.displacement) / FREQUENCY;
        let decay = (-DAMPING * dt).exp();
        let displacement = decay * (self.displacement * cos + b * sin);
        self.velocity =
            decay * FREQUENCY * (-self.displacement * sin + b * cos) - DAMPING * displacement;
        self.displacement = displacement.clamp_length_max(MAX_ANGLE);

        let end_weight = if pose.loops {
            1.0
        } else {
            let t = ((pose.end as f32 - pose.frame) / 3.0).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        self.displacement * self.envelope() * end_weight
    }
}

fn bone_weight(name: &str) -> f32 {
    match name {
        "bau_Spine1" => 0.7,
        "bau_Neck" => 0.2,
        "bau_Head" => 0.15,
        _ => 0.0,
    }
}

pub(super) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        update_render_joints
            .after(bevy::transform::TransformSystems::Propagate)
            .in_set(HitSwaySet),
    );
}

pub(super) fn spawn_render_joints(
    commands: &mut Commands,
    model: Entity,
    skeleton: &Skeleton,
    sources: &[Option<Entity>],
    original_joints: &[Entity],
) {
    let nodes: Vec<_> = skeleton
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| RenderNode {
            source: sources[i].expect("spawned source joint"),
            render: commands
                .spawn((RenderHitJoint, GlobalTransform::IDENTITY, ChildOf(model)))
                .id(),
            parent: node.parent,
            weight: bone_weight(&node.name),
            correction: Mat4::IDENTITY,

            lock_parent_correction: original_joints.len() > skeleton.skin_bone_count()
                && node.name.contains("Clavicle"),
        })
        .collect();

    let mut order = Vec::with_capacity(nodes.len());
    let mut pending: Vec<_> = skeleton
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, node)| node.parent.is_none().then_some(i))
        .collect();
    while let Some(i) = pending.pop() {
        order.push(i);
        pending.extend(skeleton.nodes[i].children.iter().copied());
    }
    let render_joints = original_joints
        .iter()
        .map(|source| {
            nodes
                .iter()
                .find(|node| node.source == *source)
                .map_or(*source, |node| node.render)
        })
        .collect();
    commands.entity(model).insert(RenderHitSkeleton {
        nodes,
        order,
        original_joints: original_joints.to_vec(),
        render_joints,
        sway: Sway::default(),
    });
}

pub(super) fn rebind_render_joints(
    commands: &mut Commands,
    model: Entity,
    skeleton: &Skeleton,
    joints: &[Entity],
) {
    let lock_arms = joints.len() > skeleton.skin_bone_count();
    let arm_roots: Vec<_> = skeleton
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, node)| node.name.contains("Clavicle").then_some(i))
        .collect();
    let joints = joints.to_vec();
    commands.queue(move |world: &mut World| {
        let Some(mut skeleton) = world.get_mut::<RenderHitSkeleton>(model) else {
            return;
        };
        for (i, node) in skeleton.nodes.iter_mut().enumerate() {
            node.lock_parent_correction = lock_arms && arm_roots.contains(&i);
        }
        skeleton.render_joints = joints
            .iter()
            .map(|source| {
                skeleton
                    .nodes
                    .iter()
                    .find(|node| node.source == *source)
                    .map_or(*source, |node| node.render)
            })
            .collect();
        skeleton.original_joints = joints;
    });
}

fn update_render_joints(
    settings: Option<Res<Settings>>,
    state: Option<Res<State<AppState>>>,
    time: Option<Res<Time>>,
    clock: Option<Res<crate::plugins::FrameClock>>,
    fighters: Query<(&PoseRuntime, &FighterHitReaction)>,
    mut models: Query<(Entity, &FighterModel, &mut RenderHitSkeleton)>,
    sources: Query<&GlobalTransform, Without<RenderHitJoint>>,
    mut joints: Query<&mut GlobalTransform, With<RenderHitJoint>>,

    mut skins: Query<(&ChildOf, &mut SkinnedMesh), With<super::FighterSkin>>,
) {
    let enabled = settings.is_some_and(|settings| settings.hit_sway);
    let playing = state
        .as_ref()
        .is_none_or(|state| *state.get() == AppState::Playing);
    let dt = if playing {
        crate::plugins::simulation_dt(clock.as_deref(), time.map_or(0.0, |time| time.delta_secs()))
    } else {
        0.0
    };
    for (entity, model, mut skeleton) in &mut models {
        let angle = if let Ok((pose, reaction)) = fighters.get(model.fighter) {
            skeleton.sway.sample(enabled, dt, pose, reaction)
        } else {
            skeleton.sway = Sway::default();
            Vec2::ZERO
        };
        let active = angle != Vec2::ZERO;
        if let Some(model_world) = active.then(|| sources.get(entity).ok()).flatten() {
            let world_angle = model_world.rotation() * Vec3::new(angle.x, angle.y, 0.0);
            for order_index in 0..skeleton.order.len() {
                let i = skeleton.order[order_index];
                let parent = if skeleton.nodes[i].lock_parent_correction {
                    Mat4::IDENTITY
                } else {
                    skeleton.nodes[i]
                        .parent
                        .map_or(Mat4::IDENTITY, |parent| skeleton.nodes[parent].correction)
                };
                let node = &mut skeleton.nodes[i];
                let Ok(source) = sources.get(node.source) else {
                    continue;
                };
                let pivot = source.translation();
                let local = if node.weight == 0.0 {
                    Mat4::IDENTITY
                } else {
                    Mat4::from_translation(pivot)
                        * Mat4::from_quat(Quat::from_scaled_axis(world_angle * node.weight))
                        * Mat4::from_translation(-pivot)
                };
                node.correction = parent * local;
                if let Ok(mut render) = joints.get_mut(node.render) {
                    *render = if node.correction == Mat4::IDENTITY {
                        *source
                    } else {
                        GlobalTransform::from(node.correction * source.to_matrix())
                    };
                }
            }
        }

        let selected = if active {
            &skeleton.render_joints
        } else {
            &skeleton.original_joints
        };
        for (parent, mut skin) in &mut skins {
            if parent.parent() == entity && skin.joints != *selected {
                skin.joints.clone_from(selected);
            }
        }
    }
}

#[cfg(test)]
mod tests;
