use super::{GraphicsPacks, SceneGraphics};
use crate::{
    app_state::LoadedStageInfo,
    components::fighter::WeaponModel,
    plugins::{
        character_plugin::WeaponSurfaceName,
        stage_material::{GmdlMaterial, GmdlMode},
    },
};
use bevy::prelude::*;

#[derive(Component)]
pub(super) struct WeaponGraphicsResolved;

pub(super) fn apply_weapon_materials(
    mut commands: Commands,
    graphics: Option<Res<GraphicsPacks>>,
    stage: Option<Res<LoadedStageInfo>>,
    mut weapons: Query<
        (
            Entity,
            &WeaponModel,
            &WeaponSurfaceName,
            &mut Mesh3d,
            &mut MeshMaterial3d<GmdlMaterial>,
        ),
        Without<WeaponGraphicsResolved>,
    >,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<GmdlMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let (Some(graphics), Some(stage)) = (graphics, stage) else {
        return;
    };
    if graphics.packs.is_empty() {
        return;
    }
    let mut scene = SceneGraphics::new(Some(&graphics), &stage.stage_id);
    for (entity, weapon, name, mut mesh_handle, mut material_handle) in &mut weapons {
        let rule = graphics
            .packs
            .iter()
            .enumerate()
            .flat_map(|(index, pack)| {
                pack.config
                    .scenes
                    .iter()
                    .filter(|s| s.stage == "*" || s.stage.eq_ignore_ascii_case(&stage.stage_id))
                    .flat_map(move |s| s.weapons.iter().map(move |rule| (index, rule)))
            })
            .filter(|(_, rule)| {
                rule.weapon == weapon.weapon
                    && rule.variant == weapon.variant
                    && rule.object.eq_ignore_ascii_case(&name.object)
                    && rule.texture.eq_ignore_ascii_case(&name.texture)
            })
            .last();
        if let Some((index, rule)) = rule {
            if let (Some(original_mesh), Some(original_material)) = (
                meshes.get(&mesh_handle.0),
                materials.get(&material_handle.0),
            ) {
                if original_material.mode == GmdlMode::Opaque {
                    let mut mesh = original_mesh.clone();
                    let mut material = original_material.clone();
                    scene.apply_pbr(
                        index,
                        &rule.material,
                        &name.object,
                        &mut material,
                        &mut mesh,
                        &mut images,
                    );
                    mesh_handle.0 = meshes.add(mesh);
                    material_handle.0 = materials.add(material);
                }
            }
        }
        commands.entity(entity).insert(WeaponGraphicsResolved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::stage_material::{GmdlMaterialParams, GmdlMode};

    #[test]
    fn weapon_rules_only_copy_matching_opaque_materials() {
        let mut app = App::new();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
        let packs = super::super::load_with_reader(
            &std::fs::read(root.join("graphics-enhancement-longsword-preview.json")).unwrap(),
            |name| std::fs::read(root.join(name)).map_err(|e| e.to_string()),
        )
        .unwrap();
        app.insert_resource(packs)
            .insert_resource(LoadedStageInfo {
                stage_id: "sn06".into(),
                ..default()
            })
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<Assets<GmdlMaterial>>()
            .add_systems(Update, apply_weapon_materials);
        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Plane3d::default().mesh().build());
        let material =
            app.world_mut()
                .resource_mut::<Assets<GmdlMaterial>>()
                .add(GmdlMaterial::new(
                    GmdlMaterialParams {
                        opacity: 1.,
                        alpha_cutoff: 0.,
                        mode: 0,
                        has_texture: 0,
                        uv_offset: Vec2::ZERO,
                    },
                    None,
                    false,
                    GmdlMode::Opaque,
                ));
        let fighter = app.world_mut().spawn_empty().id();
        let mut spawn = |weapon, variant| {
            app.world_mut()
                .spawn((
                    WeaponModel {
                        fighter,
                        weapon,
                        variant,
                        slot: 0,
                        forms: None,
                    },
                    WeaponSurfaceName {
                        object: "wpnLS01".into(),
                        texture: "wpnLS01.jpg".into(),
                    },
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                ))
                .id()
        };
        let matching = spawn(6, 0);
        let other_variant = spawn(6, 1);
        let other_weapon = spawn(5, 0);
        app.update();
        let enhanced = &app
            .world()
            .get::<MeshMaterial3d<GmdlMaterial>>(matching)
            .unwrap()
            .0;
        assert_ne!(enhanced, &material);
        let materials = app.world().resource::<Assets<GmdlMaterial>>();
        assert_eq!(materials.get(enhanced).unwrap().pbr.x, 1.);
        assert_eq!(materials.get(&material).unwrap().pbr.x, 0.);
        for entity in [other_variant, other_weapon] {
            assert_eq!(
                &app.world()
                    .get::<MeshMaterial3d<GmdlMaterial>>(entity)
                    .unwrap()
                    .0,
                &material
            );
        }
    }
}
