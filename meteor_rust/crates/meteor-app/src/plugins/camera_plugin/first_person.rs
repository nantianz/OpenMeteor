use super::*;
use crate::plugins::character_plugin::{FighterBone, FighterModel, FighterSkin};
use crate::plugins::stage_material::GmdlMaterial;
use bevy::transform::helper::TransformHelper;

pub(super) const NEAR: f32 = 0.05;

#[derive(Resource, Default)]
pub(super) struct HeadAnchor(pub Option<(Entity, Vec3)>);

#[derive(Resource, Default)]
struct SelfClipMaterials(Vec<(Handle<GmdlMaterial>, bool)>);

const SELF_CLIP_RADIUS: f32 = 0.9;

pub(super) fn install(app: &mut App) {
    app.init_resource::<HeadAnchor>()
        .init_resource::<SelfClipMaterials>()
        .add_systems(Update, sync_self_clip.after(follow_fighter_camera));
}

pub(super) fn sample_head(
    settings: Option<Res<Settings>>,
    mode: Res<CameraMode>,
    spectator: Option<Res<spectator::SpectatorState>>,
    locals: Query<Entity, With<LocalFighter>>,
    models: Query<(Entity, &FighterModel)>,
    bones: Query<(Entity, &FighterBone)>,
    transforms: TransformHelper,
    mut anchor: ResMut<HeadAnchor>,
) {
    anchor.0 = None;
    if !settings.is_some_and(|s| s.first_person)
        || mode.fly
        || mode.freecam_hold
        || spectator.is_some_and(|s| s.active)
    {
        return;
    }
    let Ok(fighter) = locals.single() else { return };
    let Some((model, _)) = models.iter().find(|(_, m)| m.fighter == fighter) else {
        return;
    };
    let Some((head, _)) = bones
        .iter()
        .find(|(_, b)| b.model == model && b.name.eq_ignore_ascii_case("bau_Head"))
    else {
        return;
    };

    if let Ok(global) = transforms.compute_global_transform(head) {
        let eye = global.translation();
        if eye.is_finite() {
            anchor.0 = Some((fighter, eye));
        }
    }
}

pub(super) fn view_direction(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    Vec3::new(sy * cp, -sp, cy * cp)
}

fn sync_self_clip(
    mode: Res<CameraMode>,
    cameras: Query<&ThirdPersonState, With<GameplayCamera>>,
    locals: Query<Entity, With<LocalFighter>>,
    skins: Query<(&FighterSkin, &MeshMaterial3d<GmdlMaterial>)>,
    materials: Option<ResMut<Assets<GmdlMaterial>>>,
    mut touched: ResMut<SelfClipMaterials>,
) {
    let Some(mut materials) = materials else {
        return;
    };
    let local = cameras
        .single()
        .ok()
        .filter(|c| c.first_person_active && !mode.fly)
        .and_then(|_| locals.single().ok());
    let active: Vec<_> = skins
        .iter()
        .filter(|(skin, _)| local == Some(skin.fighter))
        .map(|(_, handle)| handle.0.clone())
        .collect();
    touched.0.retain(|(handle, previous)| {
        if active.contains(handle) {
            return true;
        }
        if let Some(mut material) = materials.get_mut(handle) {
            material.first_person_clip = Vec4::ZERO;
            material.camera_occlusion_enabled = *previous;
        }
        false
    });
    for handle in active {
        let Some(mut material) = materials.get_mut(&handle) else {
            continue;
        };
        if !touched.0.iter().any(|(old, _)| *old == handle) {
            touched.0.push((handle, material.camera_occlusion_enabled));
        }
        let clip = Vec4::new(0.0, 0.0, 0.0, SELF_CLIP_RADIUS);
        if material.first_person_clip != clip {
            material.first_person_clip = clip;
            material.camera_occlusion_enabled = true;
        }
    }
}

#[cfg(test)]
mod tests;
