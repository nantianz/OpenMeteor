use super::*;
use crate::plugins::{graphics_pack::runtime, stage_material::FrameUniforms};
use bevy::{
    ecs::system::RunSystemOnce,
    mesh::VertexAttributeValues,
    time::TimeUpdateStrategy,
    transform::{TransformPlugin, TransformSystems},
};
use std::{path::Path, time::Duration};

fn sample_pack() -> GraphicsPacks {
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    super::super::loader::load_with_reader(
        br#"{"schema_version":1,"enabled":true,"packs":["graphics-packs/sn06-campfires/pack.json"]}"#,
        |name| std::fs::read(config.join(name)).map_err(|error| error.to_string()),
    )
    .unwrap()
}

#[test]
fn sn06_campfires_ground_on_the_original_courtyard() {
    let stock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res");
    let Ok(bytes) = std::fs::read(stock.join("sn06/sn06.gmb")) else {
        return;
    };
    let text = |name| crate::text_codec::decode_text(&std::fs::read(stock.join(name)).unwrap());
    let gmdl = meteor_asset::gmdl::decode_gmdl(&bytes).unwrap();
    let desc = meteor_asset::des::parse_stage_description(&text("sn06/sn06.des"));
    let cob =
        meteor_asset::cob::parse_cob(&std::fs::read(stock.join("sn06/sn06.cob")).unwrap()).unwrap();
    let center = meteor_scene::stage_loader::get_stage_center("sn06", Some(&text("Level/sn06.wp")));
    let terrain = StageMeshCollider::from_gmdl(&gmdl, &desc, Some(&cob.face_ids), center);
    let packs = sample_pack();
    let fires = placements(Some(&packs), "sn06", Some(&terrain));
    assert_eq!(
        fires.len(),
        4,
        "hearth and pole must both have walkable ground"
    );
    assert!(fires.iter().all(|fire| fire.clears_grass(fire.position)));
    assert!(fires.iter().all(|fire| !fire.clears_grass(Vec3::ZERO)));
    assert!(placements(Some(&packs), "sn03", Some(&terrain)).is_empty());
    assert!(placements(None, "sn06", Some(&terrain)).is_empty());
}

fn campfire_app() -> App {
    let packs = sample_pack();
    let profile = packs.packs[0].config.scenes[0].campfires.as_ref().unwrap()[0].clone();
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        bevy::state::app::StatesPlugin,
    ))
    .insert_state(AppState::Playing)
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )))
    .insert_resource(LoadedStageInfo {
        stage_id: "sn06".into(),
        ..default()
    })
    .init_resource::<FrameUniforms>()
    .init_resource::<Assets<Mesh>>()
    .init_resource::<Assets<Image>>()
    .init_resource::<Assets<StandardMaterial>>()
    .init_resource::<Assets<FxMaterial>>()
    .add_systems(
        PostUpdate,
        runtime::update_frame
            .after(animate)
            .after(TransformSystems::Propagate),
    );
    install(&mut app);
    app.world_mut()
        .spawn((GameplayCamera, Transform::from_xyz(2.0, 3.0, 8.0)));
    app.world_mut()
        .run_system_once(
            move |mut commands: Commands,
                  mut meshes: ResMut<Assets<Mesh>>,
                  mut images: ResMut<Assets<Image>>,
                  mut materials: ResMut<Assets<StandardMaterial>>,
                  mut sprites: ResMut<Assets<FxMaterial>>| {
                let placement = Placement {
                    profile: &profile,
                    position: Vec3::ZERO,
                    rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
                };
                spawn(
                    &mut commands,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    &mut sprites,
                    &[placement],
                );
            },
        )
        .unwrap();
    app.insert_resource(packs);
    app
}

#[test]
fn cloth_fire_and_lights_animate_then_freeze_together_on_pause() {
    let mut app = campfire_app();
    let world = app.world_mut();
    let handle = world
        .query_filtered::<&Mesh3d, With<props::Banner>>()
        .single(world)
        .unwrap()
        .0
        .clone();
    let initial = positions(world, &handle);
    for _ in 0..75 {
        app.update();
    }
    let current = positions(app.world(), &handle);
    assert_eq!(initial[..props::COLUMNS], current[..props::COLUMNS]);
    assert!(initial
        .iter()
        .zip(current)
        .any(|(a, b)| Vec3::from_array(*a).distance(Vec3::from_array(b)) > 0.06));

    let world = app.world_mut();
    let mut plumes = world.query_filtered::<(&Mesh3d, &Visibility), With<particles::Plume>>();
    let visible: Vec<_> = plumes
        .iter(world)
        .map(|(mesh, visibility)| {
            assert_eq!(*visibility, Visibility::Visible);
            assert!(positions(world, &mesh.0).len() >= 14 * 4);
            (mesh.0.clone(), positions(world, &mesh.0))
        })
        .collect();
    assert_eq!(visible.len(), 3);
    let lit = world.resource::<FrameUniforms>().graphics;
    assert_eq!(lit.color_pipeline.y, 1.0);
    assert!(lit.fx_light_colors[0].x > lit.fx_light_colors[0].y);
    app.update();
    let moving = app.world().resource::<FrameUniforms>().graphics;
    assert_ne!(lit.fx_light_colors[0], moving.fx_light_colors[0]);
    assert!(visible
        .iter()
        .all(|(handle, old)| positions(app.world(), handle) != *old));

    let paused = positions(app.world(), &handle);
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Paused);
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(paused, positions(app.world(), &handle));
    assert_eq!(
        moving.fx_light_colors,
        app.world()
            .resource::<FrameUniforms>()
            .graphics
            .fx_light_colors
    );

    let world = app.world_mut();
    let root = world
        .query_filtered::<Entity, With<PlayingSceneEntity>>()
        .single(world)
        .unwrap();
    assert!(world.despawn(root));
    assert_eq!(plumes.iter(world).count(), 0);
    app.update();
    assert_eq!(
        app.world()
            .resource::<FrameUniforms>()
            .graphics
            .color_pipeline
            .y,
        0.0
    );
}

#[test]
fn scene_fighter_pushes_banner_and_creates_a_local_fire_wake() {
    use super::super::scene_actors::test_actor;
    let mut quiet = campfire_app();
    let mut interacting = campfire_app();
    quiet.update();
    interacting.update();

    let world = interacting.world_mut();
    let (draft, origin) = world
        .query::<(&CampfireDraft, &GlobalTransform)>()
        .single(world)
        .unwrap();
    let fire = origin
        .transform_point(draft.hearth)
        .with_y(origin.translation().y);
    let (banner, transform) = world
        .query_filtered::<(&Mesh3d, &GlobalTransform), With<props::Banner>>()
        .single(world)
        .unwrap();
    let banner_handle = banner.0.clone();
    let banner_contact = transform.transform_point(Vec3::new(0.8, -3.60, -0.45));
    let actor = interacting
        .world_mut()
        .spawn(test_actor(fire, Vec3::X * 14.0))
        .id();
    let quiet_banner = quiet
        .world_mut()
        .query_filtered::<&Mesh3d, With<props::Banner>>()
        .single(quiet.world())
        .unwrap()
        .0
        .clone();
    for _ in 0..12 {
        quiet.update();
        interacting.update();
    }
    let world = interacting.world_mut();
    let flow = world.query::<&CampfireDraft>().single(world).unwrap().flow;
    assert!(flow.x > 1.5 && flow.y.abs() < 1e-6, "{flow:?}");
    let meshes = world
        .query_filtered::<&Mesh3d, With<particles::Plume>>()
        .iter(world)
        .map(|mesh| mesh.0.clone())
        .collect::<Vec<_>>();
    let quiet_meshes = quiet
        .world_mut()
        .query_filtered::<&Mesh3d, With<particles::Plume>>()
        .iter(quiet.world())
        .map(|mesh| mesh.0.clone())
        .collect::<Vec<_>>();
    assert!(meshes
        .iter()
        .zip(quiet_meshes.iter())
        .all(|(with, without)| positions(interacting.world(), with)
            != positions(quiet.world(), without)));

    interacting
        .world_mut()
        .get_mut::<Transform>(actor)
        .unwrap()
        .translation = banner_contact;
    interacting
        .world_mut()
        .get_mut::<crate::components::fighter::MotionState>(actor)
        .unwrap()
        .velocity = Vec3::ZERO;
    for _ in 0..45 {
        quiet.update();
        interacting.update();
    }
    let pinned = positions(quiet.world(), &quiet_banner);
    let pushed = positions(interacting.world(), &banner_handle);
    assert_eq!(pinned[..props::COLUMNS], pushed[..props::COLUMNS]);
    assert!(pinned
        .iter()
        .zip(pushed)
        .any(|(a, b)| Vec3::from_array(*a).distance(Vec3::from_array(b)) > 0.12));
    let world = interacting.world_mut();
    assert!(
        world
            .query::<&CampfireDraft>()
            .single(world)
            .unwrap()
            .flow
            .length()
            < 0.01
    );
    assert_eq!(
        world.get::<Transform>(actor).unwrap().translation,
        banner_contact
    );
}

fn positions(world: &World, handle: &Handle<Mesh>) -> Vec<[f32; 3]> {
    let meshes = world.resource::<Assets<Mesh>>();
    let Some(VertexAttributeValues::Float32x3(positions)) = meshes
        .get(handle)
        .unwrap()
        .attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        panic!("expected position data");
    };
    positions.clone()
}
