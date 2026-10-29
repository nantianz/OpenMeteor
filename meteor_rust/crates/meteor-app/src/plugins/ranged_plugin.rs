use crate::app_state::AppState;
use crate::components::fighter::{
    Fighter, Health, MotionState, RangedFireGate, WeaponForm, WeaponModel,
};
use crate::components::projectile::{
    PendingRangedHits, Projectile, ProjectileKind, RangedSegmentHit,
};
use crate::plugins::stage_collider::StageCollider;
use crate::plugins::stage_material::GmdlMaterial;
use bevy::prelude::*;
use meteor_combat::ranged_weapons::{
    wheel_guide_score, DART_GRAVITY, DART_SPIN_SPEED, GUN_RANGE, PROJECTILE_MAX_AGE,
    WHEEL_PATH_EPSILON, WHEEL_RETURN_ACCEL, WHEEL_RETURN_SPEED_SCALE, WHEEL_SPAWN_HEIGHT,
    WHEEL_TARGET_LEAD_TIME,
};
use meteor_physics::collision::{MeshCollider, Point3};

pub struct RangedPlugin;

const SOURCE_MODEL_SCALE: f32 = 0.12;

type RangedFighterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Transform,
        &'static Health,
        Option<&'static MotionState>,
        Option<&'static crate::plugins::stage_plugin::LocalFighter>,
        Option<&'static crate::plugins::stage_plugin::MissionNpc>,
        Option<&'static crate::plugins::network_plugin::RemoteFighter>,
        Option<&'static Visibility>,
    ),
    (With<Fighter>, Without<Projectile>),
>;
type ProjectileQuery<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Projectile, &'static mut Transform),
    (With<Projectile>, Without<Fighter>),
>;

impl Plugin for RangedPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingRangedHits>()
            .add_systems(
                Update,
                update_projectiles
                    .in_set(crate::plugins::FighterFrameSet::Movement)
                    .run_if(crate::plugins::simulation_frame)
                    .run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                Update,
                attach_projectile_visuals.run_if(in_state(AppState::Playing)),
            )
            .add_systems(
                OnExit(AppState::Playing),
                clear_projectiles.run_if(crate::plugins::ends_playing_session),
            );
    }
}

fn point(v: Vec3) -> Point3 {
    Point3::new(v.x, v.y, v.z)
}

fn stage_hit_distance(collider: &StageCollider, start: Vec3, end: Vec3) -> Option<f32> {
    let mesh = collider.mesh.as_ref()?;
    let delta = end - start;
    let distance = delta.length();
    if distance <= 1e-8 {
        return None;
    }
    mesh.raycast_hit(point(start), point(delta / distance), distance)
        .map(|hit| hit.t)
}

fn choose_wheel_target(
    owner: Entity,
    owner_team: i32,
    origin: Vec3,
    yaw: f32,
    net: &crate::plugins::network_plugin::NetState,
    fighters: &RangedFighterQuery<'_, '_>,
) -> Option<Entity> {
    let mut best = None;
    let mut best_score = 0.0;
    for (entity, transform, health, _, local, mission, remote, visibility) in fighters.iter() {
        if entity == owner
            || health.current <= 0.0
            || visibility.is_some_and(|visibility| *visibility == Visibility::Hidden)
        {
            continue;
        }
        let target_team = crate::plugins::combat_plugin::fighter_combat_team(
            net,
            local.is_some(),
            remote,
            mission,
        );
        if !meteor_combat::attack_window::attack_can_reach_team_with_friendly_fire(
            owner_team,
            target_team,
            net.friendly_fire_enabled(),
        ) {
            continue;
        }
        let score = wheel_guide_score(
            meteor_combat::ranged_weapons::Point3 {
                x: origin.x,
                y: origin.y,
                z: origin.z,
            },
            yaw,
            meteor_combat::ranged_weapons::Point3 {
                x: transform.translation.x,
                y: transform.translation.y,
                z: transform.translation.z,
            },
        );
        let Some(score) = score else { continue };
        if score > best_score {
            best_score = score;
            best = Some(entity);
        }
    }
    best
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_projectiles(
    mut commands: Commands,
    time: Res<Time>,
    frame_clock: Option<Res<crate::plugins::FrameClock>>,
    collider: Res<StageCollider>,
    net: Option<Res<crate::plugins::network_plugin::NetState>>,
    mut scene_effects: MessageWriter<crate::plugins::effect_plugin::SceneEffectEvent>,
    mut pending: ResMut<PendingRangedHits>,
    fighters: RangedFighterQuery<'_, '_>,
    mut gates: Query<&mut RangedFireGate, With<Fighter>>,
    mut projectiles: ProjectileQuery<'_, '_>,
) {
    let dt = crate::plugins::simulation_dt(frame_clock.as_deref(), time.delta_secs());
    let default_net = crate::plugins::network_plugin::NetState::default();
    let net = net.as_deref().unwrap_or(&default_net);
    for (entity, mut projectile, mut transform) in &mut projectiles {
        projectile.age += dt;
        if projectile.age > PROJECTILE_MAX_AGE || fighters.get(projectile.source.owner).is_err() {
            if projectile.kind == ProjectileKind::FlyingWheel {
                if let Ok(mut gate) = gates.get_mut(projectile.source.owner) {
                    gate.wheel_out = false;
                }
            }
            commands.entity(entity).despawn();
            continue;
        }

        let start = transform.translation;
        if projectile.kind == ProjectileKind::GunRay {
            let direction = projectile.velocity.normalize_or_zero();
            let end = start + direction * GUN_RANGE;
            if let Some(distance) = stage_hit_distance(&collider, start, end) {
                scene_effects.write(crate::plugins::effect_plugin::SceneEffectEvent {
                    effect: "GunHit".to_string(),
                    position: start + direction * distance,
                });
            }
            pending.0.push(RangedSegmentHit {
                projectile: entity,
                source: projectile.source.clone(),
                start,
                end,
                hit_scale: projectile.hit_scale,
                stop_after_first: false,
                despawn_after: true,
            });
            continue;
        }

        if projectile.kind == ProjectileKind::Dart {
            projectile.velocity.y -= DART_GRAVITY * dt;
        }
        if projectile.kind == ProjectileKind::FlyingWheel && !projectile.guide_initialized {
            projectile.guide_initialized = true;

            let owner_yaw = projectile
                .source
                .attack_forward
                .x
                .atan2(projectile.source.attack_forward.z);
            projectile.guide_target = choose_wheel_target(
                projectile.source.owner,
                projectile.source.combat_team,
                start,
                owner_yaw,
                net,
                &fighters,
            );
            projectile.route_to_target = projectile.guide_target.is_some();
        }

        let mut end = start + projectile.velocity * dt;
        let ignore_collision =
            projectile.kind == ProjectileKind::FlyingWheel && projectile.collision_return;
        let wall_distance = (!ignore_collision)
            .then(|| stage_hit_distance(&collider, start, end))
            .flatten();
        if let Some(distance) = wall_distance {
            let length = start.distance(end);
            if length > 1e-8 {
                end = start.lerp(end, ((distance - 0.02).max(0.0) / length).clamp(0.0, 1.0));
            }
        }

        if !ignore_collision {
            pending.0.push(RangedSegmentHit {
                projectile: entity,
                source: projectile.source.clone(),
                start,
                end,
                hit_scale: projectile.hit_scale,
                stop_after_first: projectile.kind == ProjectileKind::Dart,
                despawn_after: projectile.kind == ProjectileKind::Dart && wall_distance.is_some(),
            });
        }
        transform.translation = end;
        if projectile.kind == ProjectileKind::Dart {
            transform.rotate_local_z(-DART_SPIN_SPEED * dt);
        } else {
            transform.rotate_y(DART_SPIN_SPEED * dt);
        }

        if projectile.kind != ProjectileKind::FlyingWheel {
            continue;
        }
        let target = if projectile.route_to_target {
            projectile
                .guide_target
                .and_then(|target| fighters.get(target).ok())
                .map(|(_, transform, _, motion, _, _, _, _)| {
                    let mut point = transform.translation + Vec3::Y * WHEEL_SPAWN_HEIGHT;
                    if let Some(motion) = motion {
                        point.x += motion.velocity.x * WHEEL_TARGET_LEAD_TIME;
                        point.z += motion.velocity.z * WHEEL_TARGET_LEAD_TIME;
                    }
                    point
                })
                .unwrap_or(projectile.launch_point)
        } else {
            projectile.launch_point
        };
        let route = target - transform.translation;
        if route.length_squared() <= WHEEL_PATH_EPSILON * WHEEL_PATH_EPSILON {
            if projectile.route_to_target {
                projectile.route_to_target = false;
                projectile.guide_target = None;
            } else {
                if let Ok(mut gate) = gates.get_mut(projectile.source.owner) {
                    gate.wheel_out = false;
                }
                commands.entity(entity).despawn();
                continue;
            }
        } else {
            projectile.velocity += route.normalize_or_zero() * WHEEL_RETURN_ACCEL * dt;
            let cap = projectile.speed
                * if projectile.collision_return {
                    WHEEL_RETURN_SPEED_SCALE
                } else {
                    1.0
                };
            if projectile.velocity.length_squared() > cap * cap {
                projectile.velocity = projectile.velocity.normalize_or_zero() * cap;
            }
        }
        if wall_distance.is_some() {
            projectile.velocity = Vec3::ZERO;
            if projectile.route_to_target {
                projectile.route_to_target = false;
                projectile.guide_target = None;
            } else {
                projectile.collision_return = true;
            }
        }
    }
}

fn attach_projectile_visuals(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    query: Query<(Entity, &Projectile), Added<Projectile>>,
    fighters: Query<(&Fighter, &WeaponForm)>,
    weapon_meshes: Query<(
        &WeaponModel,
        &Mesh3d,
        &MeshMaterial3d<GmdlMaterial>,
        &Transform,
    )>,
) {
    for (entity, projectile) in &query {
        if projectile.kind == ProjectileKind::GunRay {
            continue;
        }
        let source_state = fighters.get(projectile.source.owner).ok();
        let mut original_parts = 0usize;
        for (weapon, mesh, material, source_local) in &weapon_meshes {
            let Some((fighter, form)) = source_state else {
                continue;
            };
            if weapon.fighter != projectile.source.owner
                || weapon.weapon != projectile.source.weapon
                || weapon.slot != fighter.weapon_slot
                || weapon.variant != fighter.weapon_variant
                || weapon
                    .forms
                    .as_ref()
                    .is_some_and(|forms| !forms.contains(&form.current))
            {
                continue;
            }

            let source_basis = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
            let transform = Transform {
                translation: source_basis * (source_local.translation * SOURCE_MODEL_SCALE),
                rotation: source_basis * source_local.rotation,
                scale: source_local.scale * SOURCE_MODEL_SCALE,
            };
            let part = commands
                .spawn((mesh.clone(), material.clone(), transform))
                .id();
            commands.entity(entity).add_child(part);
            original_parts += 1;
        }
        if original_parts > 0 {
            continue;
        }
        warn!(
            "original projectile model unavailable for fighter {:?} weapon {}; using fallback mesh",
            projectile.source.owner, projectile.source.weapon
        );
        let (size, color) = match projectile.kind {
            ProjectileKind::Dart => (Vec3::new(0.05, 0.05, 0.45), Color::srgb(0.7, 0.72, 0.75)),
            ProjectileKind::FlyingWheel => {
                (Vec3::new(0.65, 0.08, 0.65), Color::srgb(0.75, 0.7, 0.45))
            }
            ProjectileKind::GunRay => unreachable!(),
        };
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Cuboid::from_size(size))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: color,
                metallic: 0.75,
                perceptual_roughness: 0.3,
                ..default()
            })),
        ));
    }
}

fn clear_projectiles(
    mut commands: Commands,
    mut pending: ResMut<PendingRangedHits>,
    query: Query<Entity, With<Projectile>>,
) {
    pending.0.clear();
    for entity in &query {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::projectile::RangedHitSource;

    fn attack() -> meteor_asset::pos::AttackBlock {
        meteor_asset::pos::AttackBlock {
            start: 1,
            end: 2,
            attack_type: 0,
            check_friend: 0,
            defense_value: 0.0,
            defense_move: 0.0,
            target_value: 0.0,
            target_move: 0.0,
            target_pose: None,
            target_pose_front: None,
            target_pose_back: None,
            target_pose_left: None,
            target_pose_right: None,
            bones: Vec::new(),
        }
    }

    fn fighter(weapon: u8, slot: u8) -> Fighter {
        Fighter {
            character_id: 0,
            player_slot: slot,
            weapon,
            weapon_slots: [weapon, 5],
            weapon_slot: 0,
            weapon_slot_variants: [0, 0],
            weapon_variant: 0,
        }
    }

    fn mission_npc(id: i64, team: u8) -> crate::plugins::stage_plugin::MissionNpc {
        crate::plugins::stage_plugin::MissionNpc {
            id,
            name: format!("npc{id}"),
            team,
            in_scene_roster: true,
            view: 400.0,
            think: 50.0,
            forced_behavior: None,
            is_vip: false,
            attack_target_slot: None,
            behavior_args: Vec::new(),
            attack_target_remaining: 0,
        }
    }

    #[test]
    fn production_gun_ray_emits_full_range_segment() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.init_resource::<PendingRangedHits>();
        app.init_resource::<StageCollider>();
        app.add_message::<crate::plugins::effect_plugin::SceneEffectEvent>();
        app.add_systems(Update, update_projectiles);
        let owner = app
            .world_mut()
            .spawn((
                Fighter {
                    character_id: 0,
                    player_slot: 0,
                    weapon: 3,
                    weapon_slots: [3, 5],
                    weapon_slot: 0,
                    weapon_slot_variants: [0, 0],
                    weapon_variant: 0,
                },
                Health::default(),
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Transform::default(),
            ))
            .id();
        app.world_mut().spawn((
            Projectile {
                kind: ProjectileKind::GunRay,
                source: RangedHitSource {
                    owner,
                    weapon: 3,
                    weapon_stats: None,
                    pose_id: 206,
                    pose_frame: 1.0,
                    pose_start: 0,
                    pose_instance_id: 1,
                    attack_index: 0,
                    attack: attack(),
                    player_slot: 0,
                    combat_team: 1,
                    script_character_id: 0,
                    script_team: None,
                    attack_origin: Vec3::ZERO,
                    attack_forward: Vec3::Z,
                },
                velocity: Vec3::Z,
                age: 0.0,
                hit_scale: 1.0,
                speed: 0.0,
                launch_point: Vec3::ZERO,
                guide_target: None,
                guide_initialized: false,
                route_to_target: false,
                collision_return: false,
            },
            Transform::default(),
        ));

        app.update();

        let pending = app.world().resource::<PendingRangedHits>();
        assert_eq!(pending.0.len(), 1);
        assert!((pending.0[0].end.z - GUN_RANGE).abs() < 1e-5);
        assert!(pending.0[0].despawn_after);
    }

    #[test]
    fn flying_wheel_guidance_skips_friendly_and_hidden_fighters() {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.init_resource::<PendingRangedHits>();
        app.init_resource::<StageCollider>();
        app.add_message::<crate::plugins::effect_plugin::SceneEffectEvent>();
        app.add_systems(Update, update_projectiles);
        let owner = app
            .world_mut()
            .spawn((
                fighter(2, 0),
                Health::default(),
                RangedFireGate {
                    wheel_out: true,
                    ..default()
                },
                MotionState {
                    movement_mode: meteor_core::MovementMode::Idle,
                    velocity: Vec3::ZERO,
                    facing: 0.0,
                },
                Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
                crate::plugins::stage_plugin::LocalFighter,
            ))
            .id();
        app.world_mut().spawn((
            fighter(6, 1),
            Health::default(),
            Transform::from_xyz(0.0, 0.0, 2.0),
            mission_npc(1, 1),
            Visibility::Visible,
        ));
        app.world_mut().spawn((
            fighter(6, 2),
            Health::default(),
            Transform::from_xyz(0.0, 0.0, 3.0),
            mission_npc(2, 2),
            Visibility::Hidden,
        ));
        let enemy = app
            .world_mut()
            .spawn((
                fighter(6, 3),
                Health::default(),
                Transform::from_xyz(0.0, 0.0, 4.0),
                mission_npc(3, 2),
                Visibility::Visible,
            ))
            .id();
        let wheel = app
            .world_mut()
            .spawn((
                Projectile {
                    kind: ProjectileKind::FlyingWheel,
                    source: RangedHitSource {
                        owner,
                        weapon: 2,
                        weapon_stats: None,
                        pose_id: 220,
                        pose_frame: 1.0,
                        pose_start: 0,
                        pose_instance_id: 1,
                        attack_index: 0,
                        attack: attack(),
                        player_slot: 0,
                        combat_team: 1,
                        script_character_id: 0,
                        script_team: None,
                        attack_origin: Vec3::ZERO,
                        attack_forward: Vec3::Z,
                    },
                    velocity: Vec3::Z * 48.0,
                    age: 0.0,
                    hit_scale: 2.6,
                    speed: 48.0,
                    launch_point: Vec3::Y * WHEEL_SPAWN_HEIGHT,
                    guide_target: None,
                    guide_initialized: false,
                    route_to_target: false,
                    collision_return: false,
                },
                Transform::from_translation(Vec3::Y * WHEEL_SPAWN_HEIGHT),
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<Projectile>(wheel).unwrap().guide_target,
            Some(enemy)
        );

        for _ in 0..120 {
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(1.0 / 60.0));
            app.update();
            if app.world().get::<Projectile>(wheel).is_none() {
                break;
            }
        }
        assert!(
            app.world().get::<Projectile>(wheel).is_none(),
            "wheel must return before lifetime expiry, even while the enemy remains alive"
        );
        assert!(!app.world().get::<RangedFireGate>(owner).unwrap().wheel_out);
    }

    #[test]
    fn dart_visual_matches_original_local_spin_across_headings_and_frame_rates() {
        for yaw in [0.0, 0.7, std::f32::consts::FRAC_PI_2, std::f32::consts::PI] {
            for fps in [30, 60, 120] {
                let mut app = App::new();
                app.init_resource::<Assets<Mesh>>()
                    .init_resource::<Assets<StandardMaterial>>()
                    .init_resource::<Time>()
                    .init_resource::<PendingRangedHits>()
                    .init_resource::<StageCollider>()
                    .add_message::<crate::plugins::effect_plugin::SceneEffectEvent>()
                    .add_systems(
                        Update,
                        (attach_projectile_visuals, update_projectiles).chain(),
                    );
                let owner = app
                    .world_mut()
                    .spawn((
                        fighter(1, 0),
                        WeaponForm::default(),
                        Health::default(),
                        Transform::default(),
                    ))
                    .id();
                let source_local =
                    Transform::from_xyz(0.25, -0.5, 0.75).with_rotation(Quat::from_rotation_z(0.2));
                app.world_mut().spawn((
                    WeaponModel {
                        fighter: owner,
                        weapon: 1,
                        slot: 0,
                        variant: 0,
                        forms: None,
                    },
                    Mesh3d(Handle::default()),
                    MeshMaterial3d::<GmdlMaterial>(Handle::default()),
                    source_local,
                ));
                let projectile = app
                    .world_mut()
                    .spawn((
                        Projectile {
                            kind: ProjectileKind::Dart,
                            source: RangedHitSource {
                                owner,
                                weapon: 1,
                                weapon_stats: None,
                                pose_id: 200,
                                pose_frame: 3511.0,
                                pose_start: 3511,
                                pose_instance_id: 1,
                                attack_index: 0,
                                attack: attack(),
                                player_slot: 0,
                                combat_team: 1,
                                script_character_id: 0,
                                script_team: None,
                                attack_origin: Vec3::ZERO,
                                attack_forward: Vec3::X,
                            },
                            velocity: Vec3::X,
                            age: 0.0,
                            hit_scale: 1.0,
                            speed: 72.0,
                            launch_point: Vec3::ZERO,
                            guide_target: None,
                            guide_initialized: false,
                            route_to_target: false,
                            collision_return: false,
                        },
                        Transform::from_rotation(Quat::from_rotation_y(yaw)),
                    ))
                    .id();

                app.update();

                let children = app
                    .world()
                    .get::<Children>(projectile)
                    .expect("projectile visual child");
                assert_eq!(children.len(), 1);
                let cloned = app.world().get::<Transform>(children[0]).unwrap();
                let source_basis = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
                assert_eq!(
                    *cloned,
                    Transform {
                        translation: source_basis * (source_local.translation * SOURCE_MODEL_SCALE),
                        rotation: source_basis * source_local.rotation,
                        scale: source_local.scale * SOURCE_MODEL_SCALE,
                    }
                );
                let child = children[0];
                let initial_basis = Quat::from_rotation_y(yaw) * source_basis;
                let x = initial_basis * Vec3::X;
                let y = initial_basis * Vec3::Y;
                let z = initial_basis * Vec3::Z;
                let dt = 1.0 / fps as f32;
                let mut expected_position = Vec3::ZERO;
                let mut expected_velocity = Vec3::X;
                for frame in 1..=fps {
                    app.world_mut()
                        .get_mut::<Transform>(owner)
                        .unwrap()
                        .rotation = Quat::from_rotation_y(yaw + frame as f32 * 0.2);
                    app.world_mut()
                        .resource_mut::<Time>()
                        .advance_by(std::time::Duration::from_secs_f32(dt));
                    app.world_mut()
                        .resource_mut::<PendingRangedHits>()
                        .0
                        .clear();
                    app.update();
                    let root = app.world().get::<Transform>(projectile).unwrap();
                    let visual = app.world().get::<Transform>(child).unwrap();

                    let angle = 8.0 * frame as f32 * dt;
                    let (sin, cos) = angle.sin_cos();
                    let expected_basis = Mat3::from_cols(x * cos - z * sin, y, x * sin + z * cos);
                    let actual = root.rotation * visual.rotation;
                    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
                        let expected = expected_basis * (source_local.rotation * axis);
                        assert!(
                            (actual * axis).distance(expected) < 2e-5,
                            "yaw={yaw}, fps={fps}, frame={frame}, axis={axis:?}"
                        );
                    }
                    let actual_offset = root.rotation * visual.translation;
                    let expected_offset =
                        expected_basis * (source_local.translation * SOURCE_MODEL_SCALE);
                    assert!(actual_offset.distance(expected_offset) < 2e-5);
                    assert_eq!(visual.scale, source_local.scale * SOURCE_MODEL_SCALE);
                    expected_velocity.y -= DART_GRAVITY * dt;
                    expected_position += expected_velocity * dt;
                    assert!(root.translation.distance(expected_position) < 2e-5);
                    let sweep = &app.world().resource::<PendingRangedHits>().0[0];
                    assert!(sweep.end.distance(expected_position) < 2e-5);
                }
            }
        }
    }
}
