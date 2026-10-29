use bevy::prelude::*;
use bevy_dlss5::{DlssNeuralRendering, DlssNeuralRenderingPlugin, DlssNeuralRenderingStatus};

fn main() {
    App::new()
        .add_plugins(DlssNeuralRenderingPlugin::default())
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .add_systems(Update, (toggle, report))
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let metal = materials.add(StandardMaterial {
        base_color: Color::srgb(0.8, 0.7, 0.5),
        metallic: 1.0,
        perceptual_roughness: 0.25,
        ..default()
    });
    let floor = materials.add(StandardMaterial {
        base_color: Color::srgb(0.3, 0.3, 0.32),
        perceptual_roughness: 0.9,
        ..default()
    });
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(12.0, 12.0))),
        MeshMaterial3d(floor),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(0.6).mesh().uv(48, 24))),
        MeshMaterial3d(metal.clone()),
        Transform::from_xyz(0.0, 0.6, 0.0),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(0.7, 0.7, 0.7))),
        MeshMaterial3d(metal),
        Transform::from_xyz(1.3, 0.35, -0.4),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 6_500.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(3.0, 5.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn((
        Camera3d::default(),
        DlssNeuralRendering::default(),
        bevy::anti_alias::taa::TemporalAntiAliasing::default(),
        Msaa::Off,
        Transform::from_xyz(2.5, 1.6, 3.2).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
}

fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    on: Query<Entity, (With<Camera3d>, With<DlssNeuralRendering>)>,
    off: Query<Entity, (With<Camera3d>, Without<DlssNeuralRendering>)>,
) {
    if !keys.just_pressed(KeyCode::KeyN) {
        return;
    }
    for e in &on {
        commands.entity(e).remove::<DlssNeuralRendering>();
        info!("neural rendering OFF");
    }
    for e in &off {
        commands.entity(e).insert(DlssNeuralRendering::default());
        info!("neural rendering ON");
    }
}

fn report(status: Res<DlssNeuralRenderingStatus>, mut done: Local<bool>) {
    if *done {
        return;
    }
    if status.is_active() {
        info!("bevy_dlss5 example: neural rendering is ACTIVE");
        *done = true;
    } else if status.is_unavailable() {
        warn!("bevy_dlss5 example: neural rendering unavailable (see earlier warning)");
        *done = true;
    }
}
