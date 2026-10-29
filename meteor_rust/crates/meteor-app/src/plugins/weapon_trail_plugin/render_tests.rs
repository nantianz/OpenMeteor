use super::*;
use bevy::{
    app::PluginsState,
    camera::{RenderTarget, ScalingMode},
    core_pipeline::tonemapping::Tonemapping,
    render::{
        gpu_readback::{Readback, ReadbackComplete},
        render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
        Render, RenderApp, RenderSystems,
    },
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type CapturedFrame = Arc<Mutex<(u64, Vec<u8>)>>;

fn capture(app: &mut App, pixels: &CapturedFrame) -> Vec<u8> {
    let first = pixels.lock().unwrap().0;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "offscreen readback timed out");
        app.update();
        let frame = pixels.lock().unwrap();

        if frame.0 >= first + 30 {
            return frame.1.clone();
        }
        drop(frame);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn air_refraction_preserves_flat_color_and_displaces_pattern_without_dark_faces() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(bevy::render::RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::audio::AudioPlugin>(),
    );
    app.init_state::<crate::app_state::AppState>();
    app.add_plugins(crate::plugins::stage_material::GmdlMaterialPlugin);
    app.add_plugins(MaterialPlugin::<BladeRippleMaterial>::default());
    app.world_mut()
        .resource_mut::<Assets<Shader>>()
        .insert(
            BLADE_RIPPLE_SHADER_HANDLE.id(),
            Shader::from_wgsl(include_str!("../blade_ripple.wgsl"), "blade_ripple.wgsl"),
        )
        .unwrap();
    let mut image = Image::new_target_texture(64, 64, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.world_mut().spawn((
        Camera3d::default(),
        Camera {
            clear_color: ClearColorConfig::Custom(Color::linear_rgb(0.25, 0.25, 0.25)),
            ..default()
        },
        bevy::pbr::ScreenSpaceTransmission {
            steps: 2,
            ..default()
        },
        RenderTarget::Image(target.clone().into()),
        Msaa::Sample4,
        Tonemapping::None,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::Fixed {
                width: 4.0,
                height: 4.0,
            },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 0.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let mesh = Mesh::from(Rectangle::new(2.0, 2.0))
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; 4]);
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
    let material = app
        .world_mut()
        .resource_mut::<Assets<BladeRippleMaterial>>()
        .add(BladeRippleMaterial {
            params: BladeRippleParams {
                time: 0.13,
                impact: 1.0,
                color: Vec3::ZERO,
            },
        });
    let ribbon_mesh = mesh.clone();
    let ripple_material = material.clone();
    let ribbon = app
        .world_mut()
        .spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_xyz(0.0, 0.0, 1.0),
        ))
        .id();
    let has_transmission = Arc::new(Mutex::new(false));
    let observed = has_transmission.clone();
    app.sub_app_mut(RenderApp).add_systems(
        Render,
        (move |textures: Query<&bevy::pbr::ViewTransmissionTexture>| {
            *observed.lock().unwrap() = !textures.is_empty();
        })
        .after(RenderSystems::Prepare)
        .before(RenderSystems::Render),
    );
    let pixels: CapturedFrame = Arc::new(Mutex::new((0, Vec::new())));
    let observed = pixels.clone();
    app.world_mut()
        .spawn(Readback::texture(target))
        .observe(move |event: On<ReadbackComplete>| {
            let mut frame = observed.lock().unwrap();
            frame.0 += 1;
            frame.1 = event.data.clone();
        });
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.plugins_state() != PluginsState::Ready {
        assert!(
            Instant::now() < deadline,
            "renderer initialization timed out"
        );
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::sleep(Duration::from_millis(2));
    }
    app.finish();
    app.cleanup();
    let flat = capture(&mut app, &pixels);
    assert!(
        *has_transmission.lock().unwrap(),
        "material must create the real transmission texture"
    );
    assert_eq!(flat.len(), 64 * 64 * 4);
    let gray = flat[0];
    assert!(gray > 100 && gray < 160, "expected middle gray, got {gray}");
    for pixel in flat.chunks_exact(4) {
        assert!(
            pixel[..3].iter().all(|channel| channel.abs_diff(gray) <= 1),
            "air must not darken or illuminate a uniform background: {pixel:?} vs {gray}"
        );
    }

    app.world_mut()
        .resource_mut::<Assets<BladeRippleMaterial>>()
        .get_mut(&ripple_material)
        .unwrap()
        .params
        .color = Vec3::new(0.42, 0.58, 0.66);
    let tinted = capture(&mut app, &pixels);
    let mut changed = 0;
    for y in 0..64 {
        for x in 0..64 {
            let pixel = &tinted[(y * 64 + x) * 4..][..3];
            assert!(
                pixel
                    .iter()
                    .all(|v| *v >= gray.saturating_sub(1) && *v <= gray + 35),
                "wind sheen stays soft and cannot darken the scene: {pixel:?}"
            );
            if !(16..48).contains(&x) || !(16..48).contains(&y) {
                assert!(pixel.iter().all(|v| v.abs_diff(gray) <= 1));
            }
            changed += usize::from(pixel[2] > gray + 3);
        }
    }
    assert!(
        changed > 60,
        "sheet must be visible on a uniform background"
    );
    let mut interior = Vec::new();
    let mut rim = Vec::new();
    for y in 16..48 {
        for x in 20..40 {
            let value = tinted[(y * 64 + x) * 4 + 2];
            if (21..43).contains(&y) {
                interior.push(value);
            }
            if y <= 19 || y >= 44 {
                rim.push(value);
            }
        }
    }
    assert!(
        interior.iter().max().unwrap() - interior.iter().min().unwrap() > 5,
        "arc surface must have uneven folds, not a uniform fill"
    );
    assert!(
        rim.iter().any(|v| *v > gray + 6) && rim.iter().any(|v| *v <= gray + 1),
        "outer rim must have both visible arcs and transparent gaps"
    );

    let energy = |frame: &[u8]| -> u64 {
        frame
            .chunks_exact(4)
            .map(|p| u64::from(p[2].saturating_sub(gray)))
            .sum()
    };
    let full_energy = energy(&tinted);
    let mut previous_energy = full_energy;
    for life in [0.75, 0.5, 0.25, 0.1] {
        app.world_mut()
            .resource_mut::<Assets<Mesh>>()
            .get_mut(&ribbon_mesh)
            .unwrap()
            .insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[life, life, 0.0, 1.0]; 4]);
        let fading = capture(&mut app, &pixels);
        let current_energy = energy(&fading);
        assert!(current_energy > 0 && current_energy < previous_energy,
            "wake must fade progressively through its last tenth: {current_energy}/{previous_energy}");
        assert!(
            (current_energy as f32 / full_energy as f32 - life).abs() < 0.12,
            "opacity must not vanish early through compounded fading"
        );
        previous_energy = current_energy;
    }
    app.world_mut()
        .resource_mut::<Assets<Mesh>>()
        .get_mut(&ribbon_mesh)
        .unwrap()
        .insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0, 0.0, 0.0, 1.0]; 4]);
    let faded = capture(&mut app, &pixels);
    assert!(
        flat.iter().zip(&faded).all(|(a, b)| a.abs_diff(*b) <= 1),
        "expired wake must leave no sheet or outer rim"
    );
    app.world_mut()
        .resource_mut::<Assets<Mesh>>()
        .get_mut(&ribbon_mesh)
        .unwrap()
        .insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; 4]);
    app.world_mut()
        .resource_mut::<Assets<BladeRippleMaterial>>()
        .get_mut(&ripple_material)
        .unwrap()
        .params
        .color = Vec3::ZERO;

    let mut pattern = Vec::new();
    for _y in 0..64 {
        for x in 0..64 {
            let value = 35 + x * 3;
            pattern.extend([value, value, value, 255]);
        }
    }
    let texture = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new(
            Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            pattern,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        ));
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Rectangle::new(4.0, 4.0));
    let mut authored = GmdlMaterial::new(
        GmdlMaterialParams {
            opacity: 0.55,
            alpha_cutoff: 0.0,
            mode: GmdlMode::Add.as_u32(),
            has_texture: 1,
            uv_offset: Vec2::ZERO,
        },
        Some(texture),
        true,
        GmdlMode::Add,
    );
    authored.weapon_refraction_source = true;
    let material = app
        .world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .add(authored);
    let authored_handle = material.clone();
    app.world_mut()
        .spawn((Mesh3d(mesh), MeshMaterial3d(material), Transform::IDENTITY));
    *app.world_mut().get_mut::<Visibility>(ribbon).unwrap() = Visibility::Hidden;
    let baseline = capture(&mut app, &pixels);
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&authored_handle)
        .unwrap()
        .weapon_refraction_source = false;
    let ordinary_additive = capture(&mut app, &pixels);
    assert!(
        baseline
            .iter()
            .zip(&ordinary_additive)
            .all(|(a, b)| a.abs_diff(*b) <= 1),
        "moving authored light into the first transmission layer must preserve its appearance"
    );
    app.world_mut()
        .resource_mut::<Assets<GmdlMaterial>>()
        .get_mut(&authored_handle)
        .unwrap()
        .weapon_refraction_source = true;
    *app.world_mut().get_mut::<Visibility>(ribbon).unwrap() = Visibility::Visible;
    let refracted = capture(&mut app, &pixels);
    let mut changed = 0;
    for y in 0..64 {
        for x in 0..64 {
            let index = (y * 64 + x) * 4;
            let difference = baseline[index].abs_diff(refracted[index]);
            if (17..47).contains(&x) && (17..47).contains(&y) {
                changed += usize::from(difference > 2);
                assert!(
                    refracted[index] + 1 >= baseline[index],
                    "leftward sweep must sample the brighter right side"
                );
            } else if !(15..49).contains(&x) || !(15..49).contains(&y) {
                assert!(
                    difference <= 1,
                    "refraction leaked outside ribbon at {x},{y}: {} -> {}",
                    baseline[index],
                    refracted[index]
                );
            }
        }
    }
    assert!(
        changed > 12,
        "authored additive light must visibly bend, changed pixels: {changed}"
    );

    {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        let mut mesh = meshes.get_mut(&ribbon_mesh).unwrap();
        let bevy::mesh::VertexAttributeValues::Float32x2(uvs) =
            mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap()
        else {
            panic!("UV layout");
        };
        let reversed: Vec<[f32; 2]> = uvs.iter().map(|uv| [1.0 - uv[0], uv[1]]).collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, reversed);
    }
    let reversed = capture(&mut app, &pixels);
    let mut reverse_changed = 0;
    for y in 17..47 {
        for x in 17..47 {
            let index = (y * 64 + x) * 4;
            assert!(
                reversed[index] <= baseline[index] + 1,
                "rightward sweep must sample the darker left side"
            );
            reverse_changed += usize::from(baseline[index].abs_diff(reversed[index]) > 2);
        }
    }
    assert!(
        reverse_changed > 12,
        "reversed swing must visibly reverse the displacement"
    );
}
