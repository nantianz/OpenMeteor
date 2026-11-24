use super::{PonEfDragRibbon, PonEfMaterial, PonEfNodeEntity, PonEfParticle};
use crate::plugins::graphics_pack::{effects::EffectEnhancements, fx};
use bevy::prelude::*;

#[derive(Component, Debug, Clone)]
pub(crate) struct PonEfEffectName(pub String);

#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct PonEfEnhance {
    pub intensity: f32,
}

const SOFT_DISTANCE: f32 = 0.5;

type VisualChildren<'w, 's> = Query<
    'w,
    's,
    (Entity, Option<&'static MeshMaterial3d<PonEfMaterial>>),
    Or<(
        With<PonEfNodeEntity>,
        With<PonEfParticle>,
        With<PonEfDragRibbon>,
    )>,
>;

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_effect_rules(
    mut commands: Commands,
    enhancements: Option<Res<EffectEnhancements>>,
    roots: Query<
        (
            Entity,
            &PonEfEffectName,
            &Transform,
            Option<&ChildOf>,
            Option<&Children>,
        ),
        Added<PonEfEffectName>,
    >,
    parents: Query<&GlobalTransform>,
    visual: VisualChildren<'_, '_>,
    mut materials: ResMut<Assets<PonEfMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,

    fx_materials: Option<ResMut<Assets<fx::FxMaterial>>>,
    budget: Option<ResMut<fx::FxBudget>>,
) {
    let Some(enhancements) = enhancements.filter(|e| e.enabled) else {
        return;
    };
    let (Some(mut fx_materials), Some(mut budget)) = (fx_materials, budget) else {
        return;
    };
    for (root, name, transform, parent, children) in &roots {
        let Some(resolved) = enhancements.rule(&name.0) else {
            continue;
        };
        let rule = &resolved.rule;

        let root_world = match parent.and_then(|p| parents.get(p.parent()).ok()) {
            Some(parent_global) => parent_global.mul_transform(*transform),
            None => GlobalTransform::from(*transform),
        };
        let children: Vec<Entity> = children
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        if rule.replace {
            for child in &children {
                if visual.contains(*child) {
                    commands.entity(*child).despawn();
                }
            }
        } else {
            if (rule.intensity - 1.0).abs() > 1e-4 {
                commands.entity(root).insert(PonEfEnhance {
                    intensity: rule.intensity,
                });
            }
            if rule.soft {
                for child in &children {
                    if let Ok((_, Some(handle))) = visual.get(*child) {
                        if let Some(mut material) = materials.get_mut(&handle.0) {
                            material.params.soft = SOFT_DISTANCE;
                        }
                    }
                }
            }
        }
        if !rule.emitters.is_empty() {
            fx::spawn_emitters(
                &mut commands,
                &mut meshes,
                &mut fx_materials,
                &mut budget,
                root,
                root_world,
                &rule.emitters,
                &resolved.textures,
                root.to_bits() as u32,
            );
        }
    }
}

pub(super) fn boost(color: Vec4, enhance: Option<&PonEfEnhance>) -> Vec4 {
    match enhance {
        Some(enhance) => (color.truncate() * enhance.intensity).extend(color.w),
        None => color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::graphics_pack::config::{EffectEmitter, EffectRule, EmitterKind};

    fn app(rule: EffectRule) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<PonEfMaterial>>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<fx::FxMaterial>>()
            .init_resource::<fx::FxBudget>()
            .insert_resource(EffectEnhancements::with_rule("w02bl112", rule))
            .add_systems(Update, apply_effect_rules);
        app
    }

    fn spawn_player(app: &mut App, name: &str) -> (Entity, Entity, Handle<PonEfMaterial>) {
        let material = app
            .world_mut()
            .resource_mut::<Assets<PonEfMaterial>>()
            .add(PonEfMaterial::test_untextured());
        let root = app
            .world_mut()
            .spawn((
                PonEfEffectName(name.into()),
                Transform::from_xyz(1.0, 2.0, 3.0),
            ))
            .id();
        let node = app
            .world_mut()
            .spawn((
                PonEfNodeEntity(0),
                MeshMaterial3d(material.clone()),
                ChildOf(root),
            ))
            .id();
        (root, node, material)
    }

    #[test]
    fn replace_rule_drops_visual_nodes_and_spawns_emitters_at_the_root() {
        let mut app = app(EffectRule {
            replace: true,
            emitters: vec![
                EffectEmitter {
                    kind: EmitterKind::Burst,
                    ..default()
                },
                EffectEmitter {
                    kind: EmitterKind::Light,
                    ..default()
                },
            ],
            ..default()
        });
        let (root, node, _) = spawn_player(&mut app, "w02bl112");
        let (_, other_node, _) = spawn_player(&mut app, "pos431");
        app.update();
        assert!(
            app.world().get_entity(node).is_err(),
            "replaced node is gone"
        );
        assert!(
            app.world().get_entity(other_node).is_ok(),
            "unmatched effect keeps its nodes"
        );
        assert!(
            app.world().get_entity(root).is_ok(),
            "the player root survives for timing/audio"
        );
        let emitters: Vec<_> = app
            .world_mut()
            .query::<&fx::FxEmitter>()
            .iter(app.world())
            .collect();
        assert_eq!(emitters.len(), 1);
        assert_eq!(emitters[0].root, root);
        assert_eq!(
            emitters[0].root_world.translation(),
            Vec3::new(1.0, 2.0, 3.0)
        );
        assert_eq!(
            app.world_mut()
                .query::<&fx::FxLight>()
                .iter(app.world())
                .count(),
            1
        );
        assert_eq!(app.world().resource::<fx::FxBudget>().emitters, 1);
        assert_eq!(app.world().resource::<fx::FxBudget>().lights, 1);
    }

    #[test]
    fn override_rule_boosts_intensity_and_marks_nodes_soft() {
        let mut app = app(EffectRule {
            intensity: 2.5,
            soft: true,
            ..default()
        });
        let (root, node, material) = spawn_player(&mut app, "W02BL112");
        app.update();
        assert!(app.world().get_entity(node).is_ok());
        let enhance = app
            .world()
            .get::<PonEfEnhance>(root)
            .expect("intensity marker");
        assert_eq!(enhance.intensity, 2.5);
        let soft = app
            .world()
            .resource::<Assets<PonEfMaterial>>()
            .get(&material)
            .unwrap()
            .params
            .soft;
        assert_eq!(soft, SOFT_DISTANCE);
        assert_eq!(
            boost(Vec4::new(1.0, 0.5, 0.0, 0.3), Some(enhance)),
            Vec4::new(2.5, 1.25, 0.0, 0.3)
        );
        assert_eq!(boost(Vec4::ONE, None), Vec4::ONE);
    }
}
