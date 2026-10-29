#[cfg(any(target_os = "android", test))]
use bevy::ecs::schedule::SingleThreadedExecutor;
use bevy::light::SimulationLightSystems;
use bevy::prelude::*;

pub(crate) fn install(app: &mut App) {
    app.configure_sets(
        PostUpdate,
        SimulationLightSystems::CheckLightVisibility.run_if(has_light),
    );
    #[cfg(target_os = "android")]
    serial_post_update(app);
}

fn has_light(
    lights: Query<(), Or<(With<DirectionalLight>, With<PointLight>, With<SpotLight>)>>,
) -> bool {
    !lights.is_empty()
}

#[cfg(any(target_os = "android", test))]
pub(crate) fn serial_post_update(app: &mut App) {
    app.edit_schedule(PostUpdate, |schedule| {
        schedule.set_executor(SingleThreadedExecutor::new());
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) const fn post_update_executor() -> &'static str {
    if cfg!(target_os = "android") {
        "single_threaded"
    } else {
        "multi_threaded"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::AssetPlugin,
        camera::{
            primitives::Frustum,
            visibility::{VisibilityPlugin, VisibilitySystems, VisibleEntities},
        },
        mesh::skinning::SkinnedMeshInverseBindposes,
        transform::TransformSystems,
    };

    #[derive(Resource, Default)]
    struct Runs(u32);

    #[test]
    fn shadow_gate_restores_work_for_each_light_kind_and_after_removal() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).init_resource::<Runs>();
        install(&mut app);
        app.add_systems(
            PostUpdate,
            (|mut r: ResMut<Runs>| r.0 += 1).in_set(SimulationLightSystems::CheckLightVisibility),
        );
        app.update();
        assert_eq!(app.world().resource::<Runs>().0, 0);
        let directional = app.world_mut().spawn(DirectionalLight::default()).id();
        app.update();
        assert_eq!(app.world().resource::<Runs>().0, 1);
        app.world_mut().despawn(directional);
        app.update();
        assert_eq!(app.world().resource::<Runs>().0, 1);
        let point = app.world_mut().spawn(PointLight::default()).id();
        app.update();
        assert_eq!(app.world().resource::<Runs>().0, 2);
        app.world_mut().despawn(point);
        let spot = app.world_mut().spawn(SpotLight::default()).id();
        app.update();
        assert_eq!(app.world().resource::<Runs>().0, 3);
        app.world_mut().despawn(spot);
        app.update();
        assert_eq!(app.world().resource::<Runs>().0, 3);
    }

    #[derive(Component)]
    struct Key(u32);
    #[derive(Component)]
    struct Variant<const N: u8>;
    #[derive(Resource, Default)]
    struct Frame(u32);

    fn move_and_spawn(
        mut commands: Commands,
        frame: Res<Frame>,
        mut roots: Query<(Entity, &Key, &mut Transform, &mut Visibility), Without<ChildOf>>,
    ) {
        for (entity, key, mut transform, mut visibility) in &mut roots {
            transform.translation.x = (frame.0 as f32 * 0.03 + key.0 as f32).sin();
            if frame.0 == 30 {
                *visibility = Visibility::Hidden;
            }
            if frame.0 == 40 {
                *visibility = Visibility::Inherited;
            }
            if frame.0 == 20 {
                commands.spawn((
                    Key(10000 + key.0),
                    Transform::from_xyz(1.0, 2.0, 3.0),
                    Visibility::Inherited,
                    ChildOf(entity),
                ));
            }
        }
    }

    fn workload(gate: bool, serial: bool) -> (Vec<Vec<u32>>, f64) {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::transform::TransformPlugin,
            VisibilityPlugin,
        ))
        .init_asset::<Mesh>()
        .init_asset::<SkinnedMeshInverseBindposes>()
        .init_resource::<Frame>()
        .add_systems(
            PostUpdate,
            move_and_spawn.before(TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            bevy::light::check_dir_light_mesh_visibility
                .in_set(SimulationLightSystems::CheckLightVisibility)
                .after(VisibilitySystems::CheckVisibility)
                .before(VisibilitySystems::MarkNewlyHiddenEntitiesInvisible),
        );
        if gate {
            install(&mut app);
        }
        if serial {
            serial_post_update(&mut app);
        }
        let mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(Rectangle::new(1.0, 1.0));
        app.world_mut().spawn((
            Camera::default(),
            Frustum::default(),
            VisibleEntities::default(),
            Transform::IDENTITY,
        ));
        for actor in 0..14 {
            let root = app
                .world_mut()
                .spawn((Key(actor), Transform::IDENTITY, Visibility::Inherited))
                .id();
            let mut parent = root;
            for bone in 0..52 {
                parent = app
                    .world_mut()
                    .spawn((
                        Key(100 + actor * 52 + bone),
                        Transform::from_xyz(0.01, 0.02, 0.03),
                        Visibility::Inherited,
                        ChildOf(parent),
                    ))
                    .id();
            }
            for particle in 0..160 {
                let mut entity = app.world_mut().spawn((
                    Key(1000 + actor * 160 + particle),
                    Mesh3d(mesh.clone()),
                    Transform::from_xyz(0.1, 0.2, 0.3),
                    ChildOf(root),
                ));
                if particle & 1 != 0 {
                    entity.insert(Variant::<0>);
                }
                if particle & 2 != 0 {
                    entity.insert(Variant::<1>);
                }
                if particle & 4 != 0 {
                    entity.insert(Variant::<2>);
                }
                if particle & 8 != 0 {
                    entity.insert(Variant::<3>);
                }
                if particle & 16 != 0 {
                    entity.insert(Variant::<4>);
                }
                if particle & 32 != 0 {
                    entity.insert(Variant::<5>);
                }
            }
        }
        app.finish();
        app.cleanup();
        app.update();
        let mut snapshots = Vec::new();
        let mut elapsed = 0.0;
        for frame in 1..=90 {
            app.world_mut().resource_mut::<Frame>().0 = frame;
            let start = std::time::Instant::now();
            app.world_mut().run_schedule(PostUpdate);
            if frame > 10 {
                elapsed += start.elapsed().as_secs_f64() * 1000.0;
            }
            let world = app.world_mut();
            let mut rows: Vec<_> = world
                .query::<(&Key, &GlobalTransform, &InheritedVisibility)>()
                .iter(world)
                .map(|(key, t, v)| {
                    (
                        key.0,
                        t.to_matrix().to_cols_array().map(f32::to_bits),
                        v.get(),
                    )
                })
                .collect();
            rows.sort_by_key(|r| r.0);
            snapshots.push(
                rows.into_iter()
                    .flat_map(|(key, m, v)| std::iter::once(key).chain(m).chain([v as u32]))
                    .collect(),
            );
        }
        (snapshots, elapsed / 80.0)
    }

    #[test]
    fn post_update_preserves_live_hierarchy_visibility_and_deferred_spawns() {
        let (original, _) = workload(false, false);
        let (optimized, _) = workload(true, true);
        assert_eq!(original, optimized);
    }

    #[test]
    #[ignore = "PostUpdate CPU benchmark; not an Android FPS measurement"]
    fn post_update_cpu_benchmark() {
        let (original, a) = workload(false, false);
        let (gated, b) = workload(true, false);
        let (serial, c) = workload(true, true);
        assert_eq!(original, gated);
        assert_eq!(original, serial);
        eprintln!("post-update ms: original={a:.3} no-empty-shadow={b:.3} serial+gate={c:.3}; 90 frame snapshots equal");
    }
}
