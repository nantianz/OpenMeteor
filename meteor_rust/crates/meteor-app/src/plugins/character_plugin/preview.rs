use super::*;
use crate::app_state::{AppState, LaunchRequest, MenuScreen, Settings};
use crate::components::fighter::{Fighter, WeaponModel};
use crate::plugins::network_plugin::{NetState, NetStatus};
use bevy::camera::{
    visibility::{RenderLayers, VisibilitySystems},
    RenderTarget,
};
use bevy::render::render_resource::TextureFormat;

const PREVIEW_LAYER: usize = 29;
const PREVIEW_WIDTH: u32 = 768;
const PREVIEW_HEIGHT: u32 = 768;
const PREVIEW_FOV_RAD: f32 = 30.0 * std::f32::consts::PI / 180.0;

#[derive(Component)]
pub(crate) struct CharacterPreviewViewport;

#[derive(Component)]
struct PreviewCamera;

#[derive(Component)]
struct PreviewActor {
    signature: PreviewSignature,
    pose_id: u16,
    start: u32,
    end: u32,
    frame: f32,
}

#[derive(Component)]
struct PreviewTaggedLayer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreviewSignature {
    character_id: u8,
    primary_weapon: u8,
    primary_variant: u8,
    secondary_weapon: u8,
    secondary_variant: u8,
    color_index: u8,
    new_character_models: bool,
}

#[derive(Resource)]
struct PreviewStudio {
    image: Handle<bevy::image::Image>,
}

pub(super) fn install(app: &mut App) {
    app.add_systems(
        Update,
        (
            attach_preview_viewports,
            sync_preview_actor,
            tag_preview_layers,
            reveal_preview_skins,
            animate_preview_actor,
        )
            .chain(),
    )
    .add_systems(
        PostUpdate,
        update_preview_camera
            .after(bevy::transform::TransformSystems::Propagate)
            .before(VisibilitySystems::UpdateFrusta)
            .before(VisibilitySystems::CheckVisibility),
    );
}

fn weapon_holding_pose(weapon_id: u8, weapon_form: u8) -> u16 {
    use meteor_physics::locomotion::{
        guard_pose_for_weapon, locked_idle_pose_for_weapon, FIREARM_STRATEGY,
    };
    let strategy = meteor_combat::weapon_strategy::weapon_to_strategy(weapon_id);
    if strategy == FIREARM_STRATEGY {
        guard_pose_for_weapon(strategy).unwrap_or(0)
    } else {
        locked_idle_pose_for_weapon(strategy, weapon_form).unwrap_or(0)
    }
}
fn resolve_preview_pose(weapon_id: u8, weapon_form: u8, assets: &CharacterAssets) -> u16 {
    let candidate = weapon_holding_pose(weapon_id, weapon_form);
    if assets.pose_frames.contains_key(&candidate) {
        candidate
    } else if assets.pose_frames.contains_key(&0) {
        0
    } else {
        assets.pose_frames.keys().next().copied().unwrap_or(0)
    }
}

fn desired_signature(
    state: Option<&State<AppState>>,
    screen: Option<&MenuScreen>,
    request: &LaunchRequest,
    net: Option<&NetState>,
    settings: &Settings,
) -> Option<PreviewSignature> {
    let app_state = state.map(State::get).copied().unwrap_or(AppState::Menu);
    match app_state {
        AppState::NetRoom => {
            let member = net.and_then(|net| {
                net.members
                    .iter()
                    .find(|m| !net.player_id.is_empty() && m.player_id == net.player_id)
            });
            let character_id = member
                .map_or(request.character_id, |m| m.character_id)
                .min(19);
            let primary_weapon = member.map_or(request.weapon, |m| m.weapon).clamp(1, 12);
            let team = member
                .and_then(|m| m.team)
                .or_else(|| (request.stock_camp > 0).then_some(request.stock_camp as u32));
            let color_index = if net.is_some_and(|n| {
                n.status == NetStatus::InRoom && meteor_network::modes::is_team_mode(&n.mode)
            }) {
                color_index_for_team(team)
            } else {
                0
            };
            Some(PreviewSignature {
                character_id,
                primary_weapon,
                primary_variant: 0,
                secondary_weapon: 0,
                secondary_variant: 0,
                color_index,
                new_character_models: settings.new_character_models,
            })
        }
        AppState::Menu => match screen.copied().unwrap_or(MenuScreen::Main) {
            MenuScreen::Stages => {
                let ((pw, pv), (sw, sv)) =
                    crate::plugins::campaign_ui::mission_weapons(&request.stage_id);
                Some(PreviewSignature {
                    character_id: 0,
                    primary_weapon: pw,
                    primary_variant: pv,
                    secondary_weapon: sw,
                    secondary_variant: sv,
                    color_index: 0,
                    new_character_models: settings.new_character_models,
                })
            }
            MenuScreen::Network => {
                let color_index = match request.stock_camp {
                    1 => 1,
                    2 => 2,
                    _ => 0,
                };
                Some(PreviewSignature {
                    character_id: request.character_id.min(19),
                    primary_weapon: request.weapon.clamp(1, 12),
                    primary_variant: 0,
                    secondary_weapon: 0,
                    secondary_variant: 0,
                    color_index,
                    new_character_models: settings.new_character_models,
                })
            }
            _ => None,
        },
        _ => None,
    }
}

fn attach_preview_viewports(
    mut commands: Commands,
    studio: Option<Res<PreviewStudio>>,
    images: Option<ResMut<Assets<bevy::image::Image>>>,
    cameras: Query<Entity, With<PreviewCamera>>,
    new_viewports: Query<Entity, (With<CharacterPreviewViewport>, Without<ImageNode>)>,
    mut active_viewports: Query<(&ComputedNode, &mut ImageNode), With<CharacterPreviewViewport>>,
) {
    let target = if let Some(studio) = studio {
        studio.image.clone()
    } else if !new_viewports.is_empty() {
        let Some(mut images) = images else {
            return;
        };
        let image = images.add(bevy::image::Image::new_target_texture(
            PREVIEW_WIDTH,
            PREVIEW_HEIGHT,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
        commands.insert_resource(PreviewStudio {
            image: image.clone(),
        });
        image
    } else {
        return;
    };

    if cameras.is_empty() {
        commands.spawn((
            Camera3d::default(),
            Camera {
                order: -2,
                is_active: false,
                clear_color: ClearColorConfig::Custom(Color::NONE),
                ..default()
            },
            RenderTarget::Image(target.clone().into()),
            Projection::Perspective(PerspectiveProjection {
                fov: PREVIEW_FOV_RAD,
                aspect_ratio: 1.0,
                near: 0.1,
                far: 45.0,
                ..default()
            }),
            bevy::core_pipeline::tonemapping::Tonemapping::None,
            Msaa::Off,
            RenderLayers::layer(PREVIEW_LAYER),
            Transform::from_xyz(0.0, 2.2, 10.5).looking_at(Vec3::new(0.0, 2.1, 0.0), Vec3::Y),
            PreviewCamera,
        ));
    }

    for entity in &new_viewports {
        commands.entity(entity).insert(ImageNode {
            image: target.clone(),
            ..default()
        });
    }
    for (_computed, mut image) in &mut active_viewports {
        if image.rect.is_some() {
            image.rect = None;
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn sync_preview_actor(
    mut commands: Commands,
    state: Option<Res<State<AppState>>>,
    screen: Option<Res<MenuScreen>>,
    request: Option<Res<LaunchRequest>>,
    net: Option<Res<NetState>>,
    settings: Option<Res<Settings>>,
    viewports: Query<&ComputedNode, With<CharacterPreviewViewport>>,
    actors: Query<(Entity, &PreviewActor)>,
    models: Query<(Entity, &FighterModel)>,
    mut cached_assets: Option<ResMut<CharacterAssetsSet>>,
    common_cached: Option<Res<CommonAmb>>,
    failures: Option<ResMut<CharacterLoadFailures>>,
    gpu_assets: (
        Option<ResMut<Assets<Mesh>>>,
        Option<ResMut<Assets<GmdlMaterial>>>,
        Option<ResMut<Assets<bevy::image::Image>>>,
        Option<ResMut<Assets<SkinnedMeshInverseBindposes>>>,
        Option<ResMut<crate::plugins::pak_textures::QualityTextureHandles>>,
    ),
) {
    let (Some(request), Some(settings)) = (request, settings) else {
        return;
    };
    let any_visible = viewports
        .iter()
        .any(|node| node.size().x > 4.0 && node.size().y > 4.0);
    let wanted = if any_visible {
        desired_signature(
            state.as_deref(),
            screen.as_deref(),
            &request,
            net.as_deref(),
            &settings,
        )
    } else {
        None
    };

    let Some(signature) = wanted else {
        for (actor, _) in &actors {
            for (model_entity, model) in &models {
                if model.fighter == actor {
                    commands.entity(model_entity).despawn();
                }
            }
            commands.entity(actor).despawn();
        }
        return;
    };

    if actors.iter().any(|(_, actor)| actor.signature == signature) {
        return;
    }

    for (actor, _) in &actors {
        for (model_entity, model) in &models {
            if model.fighter == actor {
                commands.entity(model_entity).despawn();
            }
        }
        commands.entity(actor).despawn();
    }

    let (
        (
            Some(mut meshes),
            Some(mut materials),
            Some(mut images),
            Some(mut inverse_binds),
            Some(mut quality_handles),
        ),
        Some(mut failures),
    ) = (gpu_assets, failures)
    else {
        return;
    };

    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let roots = resource_roots(&root);
    if common_cached.is_none() {
        if let Some(amb) =
            read_first(&roots, "Common/character.amb").and_then(|bytes| parse_amb(&bytes).ok())
        {
            commands.insert_resource(CommonAmb(amb));
        }
    }
    let character_key = format!("p{}", signature.character_id);
    let mut owned_set = CharacterAssetsSet::default();
    let assets_set = if let Some(ref mut cached) = cached_assets {
        &mut **cached
    } else {
        &mut owned_set
    };
    if !ensure_character_assets(assets_set, &mut failures, &roots, &character_key) {
        return;
    }
    let Some(character_assets) = assets_set.0.get(&character_key) else {
        return;
    };
    let weapon_form = 0;
    let pose_id = resolve_preview_pose(signature.primary_weapon, weapon_form, character_assets);
    let (start, end) = character_assets
        .pose_frames
        .get(&pose_id)
        .copied()
        .unwrap_or((0, 0));
    let (loop_start, loop_end) = character_assets
        .poses
        .get(&pose_id)
        .filter(|def| def.loops && def.loop_end >= def.loop_start && def.loop_start >= start)
        .map_or((start, end), |def| (def.loop_start, def.loop_end));

    let actor_entity = commands
        .spawn((
            PreviewActor {
                signature,
                pose_id,
                start: loop_start,
                end: loop_end.max(loop_start),
                frame: loop_start as f32,
            },
            Visibility::default(),
            InheritedVisibility::default(),
            Transform::IDENTITY,
        ))
        .id();

    let fighter_meta = Fighter {
        character_id: signature.character_id,
        player_slot: 0,
        weapon: signature.primary_weapon,
        weapon_slots: [signature.primary_weapon, signature.secondary_weapon],
        weapon_slot: 0,
        weapon_slot_variants: [signature.primary_variant, signature.secondary_variant],
        weapon_variant: signature.primary_variant,
    };
    spawn_fighter_model_for(
        &mut commands,
        actor_entity,
        &fighter_meta,
        weapon_form,
        &character_key,
        character_assets,
        0.24,
        signature.color_index,
        &mut meshes,
        &mut materials,
        &mut images,
        &mut inverse_binds,
        signature.new_character_models,
        settings.texture_quality,
        &mut quality_handles,
    );
    if cached_assets.is_none() {
        commands.insert_resource(owned_set);
    }
}
#[cfg(all(test, feature = "original-assets"))]
mod tests {
    use super::*;
    #[test]
    fn preview_matches_all_original_weapon_strategies_and_staff_blade_forms() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
        let assets = super::super::load_character_asset_for_test(&[root], "p0").unwrap();
        for (weapon, expected) in (1..=12).zip([1, 3, 12, 4, 5, 6, 7, 8, 7, 401, 400, 404]) {
            assert_eq!(
                resolve_preview_pose(weapon, 0, &assets),
                expected,
                "weapon {weapon}"
            );
        }
        assert_eq!(resolve_preview_pose(10, 1, &assets), 402);
        assert_eq!(resolve_preview_pose(10, 2, &assets), 403);
    }
}

fn tag_preview_layers(
    mut commands: Commands,
    actors: Query<Entity, With<PreviewActor>>,
    skins: Query<(Entity, &FighterSkin), Without<PreviewTaggedLayer>>,
    weapons: Query<(Entity, &WeaponModel), (With<Mesh3d>, Without<PreviewTaggedLayer>)>,
) {
    if actors.is_empty() {
        return;
    }
    for (entity, skin) in &skins {
        if actors.get(skin.fighter).is_ok() {
            commands
                .entity(entity)
                .insert((RenderLayers::layer(PREVIEW_LAYER), PreviewTaggedLayer));
        }
    }
    for (entity, weapon) in &weapons {
        if actors.get(weapon.fighter).is_ok() {
            commands
                .entity(entity)
                .insert((RenderLayers::layer(PREVIEW_LAYER), PreviewTaggedLayer));
        }
    }
}

fn reveal_preview_skins(
    mut commands: Commands,
    actors: Query<Entity, With<PreviewActor>>,
    mut skins: Query<
        (Entity, &FighterSkin, &SkinnedMesh, &mut Visibility),
        With<PendingFighterSkinReveal>,
    >,
    joints: Query<&GlobalTransform>,
    inverse_binds: Option<Res<Assets<SkinnedMeshInverseBindposes>>>,
) {
    let Some(inverse_binds) = inverse_binds else {
        return;
    };
    for (entity, fighter_skin, skin, mut visibility) in &mut skins {
        if actors.get(fighter_skin.fighter).is_err() {
            continue;
        }
        let Some(inverse) = inverse_binds.get(&skin.inverse_bindposes) else {
            continue;
        };
        if inverse.len() != skin.joints.len()
            || skin.joints.iter().any(|joint| joints.get(*joint).is_err())
        {
            continue;
        }
        *visibility = Visibility::Visible;
        commands.entity(entity).remove::<PendingFighterSkinReveal>();
    }
}

#[allow(clippy::too_many_arguments)]
fn animate_preview_actor(
    time: Option<Res<Time>>,
    assets: Option<Res<CharacterAssetsSet>>,
    common_amb: Option<Res<CommonAmb>>,
    mut actors: Query<(Entity, &mut PreviewActor)>,
    mut models: Query<(Entity, &FighterModel, &mut Transform)>,
    skeletons: FighterSkeletonQuery,
    mut bones: Query<
        (&FighterBone, &mut Transform, Option<&mut HeadPoseSample>),
        (Without<FighterDummy>, Without<FighterModel>),
    >,
    mut dummies: Query<
        (&FighterDummy, &mut Transform),
        (Without<FighterBone>, Without<FighterModel>),
    >,
) {
    let (Some(assets), Some(common_amb)) = (assets, common_amb) else {
        return;
    };
    let (dt, elapsed) = time.map_or((0.016, 0.0), |t| {
        (t.delta_secs().clamp(0.0, 0.05), t.elapsed_secs())
    });
    for (actor_entity, mut actor) in &mut actors {
        let span = (actor.end.saturating_sub(actor.start)) as f32;
        if span > 0.5 {
            actor.frame += dt * 20.0;
            if actor.frame > actor.end as f32 {
                actor.frame = actor.start as f32 + (actor.frame - actor.start as f32) % span;
            }
        } else {
            actor.frame = actor.start as f32;
        }
        let Some((model_entity, model, mut model_transform)) = models
            .iter_mut()
            .find(|(_, m, _)| m.fighter == actor_entity)
        else {
            continue;
        };
        let Some(character_assets) = assets.0.get(&model.character) else {
            continue;
        };
        let yaw = 0.24 + (elapsed * 0.45).sin() * 0.07;
        model_transform.rotation = fighter_model_rotation(yaw);

        let amb = character_assets.amb_for_pose(&common_amb.0, actor.pose_id);
        let sampled = amb.sample_at(actor.frame);
        let baseline = amb.sample_at(actor.start as f32);
        let base_dummy_index = character_assets.dummy_order.get(MOTION_ROOT_DUMMY).copied();

        let mut model_bones = bones.iter_many_mut(model_bones(model_entity, &skeletons));
        while let Some((bone, mut transform, head_sample)) = model_bones.fetch_next() {
            if let Some(q) = sampled.bone_quats.get(bone.skin_index).copied() {
                let rotation = Quat::from_xyzw(q[0], q[1], q[2], q[3]);
                transform.rotation = rotation;
                if let Some(mut sample) = head_sample {
                    sample.0 = rotation;
                }
            }
            if bone.skin_index == 0 {
                let mut pivot = sampled.root_pivot;
                if base_dummy_index.is_none() {
                    pivot[0] -= baseline.root_pivot[0];
                    pivot[1] -= baseline.root_pivot[1];
                }
                transform.translation = Vec3::new(pivot[0], pivot[1], pivot[2]);
            }
        }

        let mut model_dummies = dummies.iter_many_mut(model_dummies(model_entity, &skeletons));
        while let Some((dummy, mut transform)) = model_dummies.fetch_next() {
            if let Some(pos) = sampled.dummy_pos.get(dummy.dummy_index) {
                let mut p = [pos[0], pos[1], pos[2]];
                if base_dummy_index == Some(dummy.dummy_index) {
                    p[0] = 0.0;
                    p[1] = 0.0;
                }
                transform.translation = Vec3::new(p[0], p[1], p[2]);
            }
            if let Some(q) = sampled.dummy_quats.get(dummy.dummy_index) {
                transform.rotation = Quat::from_xyzw(q[0], q[1], q[2], q[3]);
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_preview_camera(
    actors: Query<Entity, With<PreviewActor>>,
    models: Query<(Entity, &FighterModel)>,
    bones: Query<(&FighterBone, &GlobalTransform), Without<PreviewCamera>>,
    viewports: Query<&ComputedNode, With<CharacterPreviewViewport>>,
    mut cameras: Query<
        (
            &mut Camera,
            &mut Projection,
            &mut Transform,
            &mut GlobalTransform,
        ),
        With<PreviewCamera>,
    >,
) {
    let active = !actors.is_empty();
    let aspect = viewports
        .iter()
        .map(|node| node.size())
        .filter(|size| size.x > 4.0 && size.y > 4.0)
        .max_by(|a, b| (a.x * a.y).total_cmp(&(b.x * b.y)))
        .map_or(1.0, |size| (size.x / size.y).clamp(0.45, 3.0));

    let framing = actors.iter().next().and_then(|actor| {
        let (model_entity, _) = models.iter().find(|(_, m)| m.fighter == actor)?;
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        let mut count = 0usize;
        for (bone, gt) in &bones {
            if bone.model != model_entity {
                continue;
            }
            let p = gt.translation();
            if p.is_finite() {
                min = min.min(p);
                max = max.max(p);
                count += 1;
            }
        }
        if count == 0 {
            return None;
        }
        let raw_height = (max.y - min.y).max(0.8);
        let raw_width = (max.x - min.x).max(raw_height * 0.55);
        let center = Vec3::new(
            (min.x + max.x) * 0.5,
            (min.y + max.y) * 0.5 + raw_height * 0.04,
            (min.z + max.z) * 0.5,
        );
        let fit_h = raw_height * 1.48;
        let fit_w = raw_width * 1.55;
        let half_v_tan = (PREVIEW_FOV_RAD * 0.5).tan();
        let half_h_tan = half_v_tan * aspect;
        let distance = ((fit_h * 0.5) / half_v_tan)
            .max((fit_w * 0.5) / half_h_tan)
            .clamp(2.8, 22.0);
        let eye = center + Vec3::new(distance * 0.04, raw_height * 0.05, distance);
        Some(Transform::from_translation(eye).looking_at(center, Vec3::Y))
    });
    for (mut camera, mut projection, mut transform, mut global) in &mut cameras {
        camera.is_active = active;
        if let Projection::Perspective(ref mut persp) = *projection {
            if (persp.aspect_ratio - aspect).abs() > 0.005 {
                persp.aspect_ratio = aspect;
            }
        }
        if let Some(shot) = framing {
            *transform = shot;
            *global = GlobalTransform::from(shot);
        }
    }
}
