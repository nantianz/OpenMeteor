use super::*;
use crate::components::fighter::{Concealment, Health, WeaponModel};
use crate::plugins::effect_plugin::WornVisual;

#[derive(Component)]
pub(super) struct ConcealedEquipment {
    original: Handle<GmdlMaterial>,
    faded: Handle<GmdlMaterial>,
}

pub(super) fn sync_equipment_concealment(
    mut commands: Commands,
    fighters: Query<(&Concealment, &Health)>,
    parents: Query<&ChildOf>,
    worn: Query<(), With<WornVisual>>,
    mut meshes: Query<
        (
            Entity,
            &mut MeshMaterial3d<GmdlMaterial>,
            Option<&WeaponModel>,
            Option<&ConcealedEquipment>,
        ),
        Without<FighterSkin>,
    >,
    mut materials: ResMut<Assets<GmdlMaterial>>,
) {
    for (entity, mut material, weapon, state) in &mut meshes {
        let owner = weapon.map(|weapon| weapon.fighter).or_else(|| {
            let mut current = entity;
            let mut equipment = false;
            while let Ok(parent) = parents.get(current) {
                equipment |= worn.contains(current);
                current = parent.parent();
                if fighters.contains(current) {
                    return equipment.then_some(current);
                }
            }
            None
        });
        let concealed = owner
            .and_then(|owner| fighters.get(owner).ok())
            .is_some_and(|(concealment, health)| {
                health.current > 0.0 && concealment.remaining > 0.0
            });
        match (concealed, state) {
            (true, None) => {
                let Some(mut faded) = materials.get(&material.0).cloned() else {
                    continue;
                };
                faded.params.opacity *= 0.1;
                faded.params.alpha_cutoff = 0.0;
                faded.mode = GmdlMode::Blend;
                faded.params.mode = faded.mode.as_u32();
                let faded = materials.add(faded);
                let original = std::mem::replace(&mut material.0, faded.clone());
                commands
                    .entity(entity)
                    .insert(ConcealedEquipment { original, faded });
            }
            (false, Some(state)) => {
                if material.0 == state.faded {
                    material.0 = state.original.clone();
                }
                commands.entity(entity).remove::<ConcealedEquipment>();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn equipment_concealment_is_owner_local_and_restores_authored_material() {
        let mut app = App::new();
        app.init_resource::<Assets<GmdlMaterial>>()
            .add_systems(Update, sync_equipment_concealment);
        let original =
            app.world_mut()
                .resource_mut::<Assets<GmdlMaterial>>()
                .add(GmdlMaterial::new(
                    GmdlMaterialParams {
                        opacity: 0.7,
                        alpha_cutoff: 0.5,
                        mode: GmdlMode::Opaque.as_u32(),
                        has_texture: 0,
                        uv_offset: Vec2::ZERO,
                    },
                    None,
                    true,
                    GmdlMode::Opaque,
                ));
        let fighter = app
            .world_mut()
            .spawn((Concealment { remaining: 30.0 }, Health::default()))
            .id();
        let bone = app.world_mut().spawn(ChildOf(fighter)).id();
        let worn = app
            .world_mut()
            .spawn((
                WornVisual {
                    effect: "Item15".into(),
                },
                ChildOf(fighter),
            ))
            .id();
        let model_node = app.world_mut().spawn(ChildOf(worn)).id();
        let hat = app
            .world_mut()
            .spawn((MeshMaterial3d(original.clone()), ChildOf(model_node)))
            .id();
        let weapon = app
            .world_mut()
            .spawn((
                MeshMaterial3d(original.clone()),
                ChildOf(bone),
                WeaponModel {
                    fighter,
                    weapon: 6,
                    slot: 0,
                    variant: 0,
                    forms: None,
                },
            ))
            .id();
        let ground_drop = app.world_mut().spawn(MeshMaterial3d(original.clone())).id();
        for _ in 0..3 {
            app.update();
        }
        for entity in [hat, weapon] {
            let handle = &app
                .world()
                .get::<MeshMaterial3d<GmdlMaterial>>(entity)
                .unwrap()
                .0;
            assert_ne!(handle, &original);
            let material = app
                .world()
                .resource::<Assets<GmdlMaterial>>()
                .get(handle)
                .unwrap();
            assert!(
                (material.params.opacity - 0.07).abs() < 1e-6,
                "must not compound each frame"
            );
            assert_eq!(material.params.alpha_cutoff, 0.0);
            assert_eq!(material.mode, GmdlMode::Blend);
        }
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<GmdlMaterial>>(ground_drop)
                .unwrap()
                .0,
            original
        );
        assert_eq!(
            app.world()
                .resource::<Assets<GmdlMaterial>>()
                .get(&original)
                .unwrap()
                .params
                .opacity,
            0.7
        );

        app.world_mut().get_mut::<Health>(fighter).unwrap().current = 0.0;
        app.update();
        for entity in [hat, weapon] {
            assert_eq!(
                app.world()
                    .get::<MeshMaterial3d<GmdlMaterial>>(entity)
                    .unwrap()
                    .0,
                original
            );
            assert!(app.world().get::<ConcealedEquipment>(entity).is_none());
        }
        app.world_mut().get_mut::<Health>(fighter).unwrap().current = 100.0;
        app.update();
        app.world_mut()
            .get_mut::<Concealment>(fighter)
            .unwrap()
            .remaining = 0.0;
        app.update();
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<GmdlMaterial>>(hat)
                .unwrap()
                .0,
            original
        );
    }
}
