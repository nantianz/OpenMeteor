use super::GraphicsPacks;
use crate::{
    app_state::{AppState, LoadedStageInfo},
    plugins::camera_plugin::GameplayCamera,
};
use bevy::{
    camera::Hdr,
    core_pipeline::tonemapping::Tonemapping,
    post_process::bloom::{Bloom, BloomCompositeMode, BloomPrefilter},
    prelude::*,
    render::{renderer::RenderAdapterInfo, settings::Backends, view::ColorGrading},
};

#[derive(Component)]
pub(super) struct UnifiedColorPipeline {
    previous_hdr: bool,
    previous_tone: Option<Tonemapping>,
    previous_grading: Option<ColorGrading>,
}

#[cfg(test)]
pub(super) fn test_marker() -> UnifiedColorPipeline {
    UnifiedColorPipeline {
        previous_hdr: false,
        previous_tone: None,
        previous_grading: None,
    }
}

fn bloom_component(profile: &super::config::Bloom) -> Option<Bloom> {
    (profile.intensity > 0.0).then(|| Bloom {
        intensity: profile.intensity,
        low_frequency_boost: profile.low_frequency_boost,
        low_frequency_boost_curvature: 0.95,
        high_pass_frequency: 1.0,
        prefilter: BloomPrefilter {
            threshold: profile.threshold,
            threshold_softness: profile.threshold_softness,
        },
        composite_mode: BloomCompositeMode::Additive,
        ..Bloom::default()
    })
}

fn same_bloom(a: &Bloom, b: &Bloom) -> bool {
    a.intensity == b.intensity
        && a.low_frequency_boost == b.low_frequency_boost
        && a.prefilter.threshold == b.prefilter.threshold
        && a.prefilter.threshold_softness == b.prefilter.threshold_softness
}

pub(super) fn install(app: &mut App) {
    app.add_systems(Update, sync.after(super::sky::update_cycle));
}

fn sync(
    mut commands: Commands,
    state: Option<Res<State<AppState>>>,
    loaded: Option<Res<LoadedStageInfo>>,
    packs: Option<Res<GraphicsPacks>>,
    adapter: Option<Res<RenderAdapterInfo>>,
    sky: Query<&super::sky::CloudSky>,
    cameras: Query<
        (
            Entity,
            Option<&Hdr>,
            Option<&Tonemapping>,
            Option<&ColorGrading>,
            Option<&Bloom>,
            Option<&UnifiedColorPipeline>,
        ),
        With<GameplayCamera>,
    >,
) {
    let supported = !cfg!(any(target_arch = "wasm32", target_os = "android"))
        && adapter.as_ref().is_some_and(|adapter| {
            Backends::from(adapter.0.backend)
                .intersects(Backends::METAL | Backends::VULKAN | Backends::DX12)
        });
    let active = state
        .as_ref()
        .is_some_and(|state| matches!(state.get(), AppState::Playing | AppState::Paused));
    let selection = packs
        .as_deref()
        .zip(loaded.as_deref())
        .and_then(|(packs, loaded)| {
            let mut pipeline = None;
            let mut exposure = 1.0;
            for (_, scene) in packs.scenes(&loaded.stage_id) {
                if let Some(profile) = &scene.color_pipeline {
                    pipeline = Some(profile);
                }
                if let Some(environment) = &scene.environment {
                    exposure = environment.exposure;
                }
            }
            let night = sky
                .iter()
                .find_map(|sky| sky.0)
                .map_or(0.0, |light| light.night_factor);
            pipeline.filter(|profile| profile.enabled).map(|profile| {
                (
                    profile.exposure_ev + profile.night_exposure_ev * night + exposure.log2(),
                    profile.bloom.as_ref().and_then(bloom_component),
                    match profile.tonemapping {
                        super::config::ToneMapper::Aces => Tonemapping::AcesFitted,
                        super::config::ToneMapper::None => Tonemapping::None,
                        super::config::ToneMapper::Reinhard => Tonemapping::Reinhard,
                        super::config::ToneMapper::TonyMcMapface => Tonemapping::TonyMcMapface,
                    },
                )
            })
        })
        .filter(|_| active && supported);
    for (entity, hdr, tone, grading, bloom, managed) in &cameras {
        let mut entity = commands.entity(entity);
        if let Some((exposure, wanted_bloom, wanted_tone)) = &selection {
            let exposure = *exposure;
            if managed.is_none() {
                entity.insert(UnifiedColorPipeline {
                    previous_hdr: hdr.is_some(),
                    previous_tone: tone.copied(),
                    previous_grading: grading.cloned(),
                });
            }
            if hdr.is_none() {
                entity.insert(Hdr);
            }

            match (wanted_bloom, bloom) {
                (Some(wanted), current)
                    if current.is_none_or(|current| !same_bloom(current, wanted)) =>
                {
                    entity.insert(wanted.clone());
                }
                (None, Some(_)) => {
                    entity.remove::<Bloom>();
                }
                _ => {}
            }
            if tone != Some(wanted_tone) {
                entity.insert(*wanted_tone);
            }
            if managed.is_none() || grading.is_none_or(|value| value.global.exposure != exposure) {
                let mut value = ColorGrading::default();
                value.global.exposure = exposure;
                entity.insert(value);
            }
        } else if let Some(managed) = managed {
            if bloom.is_some() {
                entity.remove::<Bloom>();
            }
            if !managed.previous_hdr {
                entity.remove::<Hdr>();
            }
            if let Some(tone) = managed.previous_tone {
                entity.insert(tone);
            } else {
                entity.remove::<Tonemapping>();
            }
            if let Some(grading) = &managed.previous_grading {
                entity.insert(grading.clone());
            } else {
                entity.remove::<ColorGrading>();
            }
            entity.remove::<UnifiedColorPipeline>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{render::renderer::WgpuWrapper, state::app::StatesPlugin};

    fn adapter(backend: wgpu::Backend) -> RenderAdapterInfo {
        RenderAdapterInfo(WgpuWrapper::new(wgpu::AdapterInfo {
            name: "fixture".into(),
            vendor: 0,
            device: 0,
            device_type: wgpu::DeviceType::Other,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend,
            subgroup_min_size: 4,
            subgroup_max_size: 32,
            transient_saves_memory: false,
        }))
    }

    #[test]
    fn stage_hdr_is_opt_in_restores_camera_and_leaves_other_cameras_alone() {
        let packs = super::super::load_with_reader(
            br#"{"schema_version":1,"enabled":true,"packs":["p.json"]}"#,
            |_| Ok(br#"{"schema_version":1,"id":"hdr","scenes":[{"stage":"sn06","color_pipeline":{"enabled":true,"exposure_ev":1.0,"night_exposure_ev":1.0},"environment":{"exposure":2.0}}]}"#.to_vec()),
        ).unwrap();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .insert_state(AppState::Playing)
            .insert_resource(packs)
            .insert_resource(LoadedStageInfo {
                stage_id: "sn06".into(),
                ..default()
            })
            .insert_resource(adapter(wgpu::Backend::Metal))
            .add_systems(Update, sync);
        let mut original = ColorGrading::default();
        original.global.exposure = -0.5;
        let camera = app
            .world_mut()
            .spawn((GameplayCamera, Tonemapping::None, original))
            .id();
        let ui = app.world_mut().spawn(Tonemapping::None).id();
        app.update();
        assert!(app.world().get::<Hdr>(camera).is_some());
        assert_eq!(
            app.world().get::<Tonemapping>(camera),
            Some(&Tonemapping::AcesFitted)
        );
        assert_eq!(
            app.world()
                .get::<ColorGrading>(camera)
                .unwrap()
                .global
                .exposure,
            2.0
        );
        assert!(app.world().get::<Hdr>(ui).is_none());
        let night = app
            .world_mut()
            .spawn(super::super::sky::CloudSky(Some(
                super::super::sky::SkyLighting {
                    sky: Vec3::ZERO,
                    ground: Vec3::ZERO,
                    sun: Vec4::Y,
                    sun_color: Vec3::ONE,
                    night_factor: 1.0,
                },
            )))
            .id();
        app.update();
        assert_eq!(
            app.world()
                .get::<ColorGrading>(camera)
                .unwrap()
                .global
                .exposure,
            3.0
        );
        app.world_mut().despawn(night);
        app.world_mut().resource_mut::<LoadedStageInfo>().stage_id = "sn03".into();
        app.update();
        assert!(app.world().get::<Hdr>(camera).is_none());
        assert_eq!(
            app.world().get::<Tonemapping>(camera),
            Some(&Tonemapping::None)
        );
        assert_eq!(
            app.world()
                .get::<ColorGrading>(camera)
                .unwrap()
                .global
                .exposure,
            -0.5
        );
        app.world_mut().resource_mut::<LoadedStageInfo>().stage_id = "sn06".into();
        app.update();
        assert!(app.world().get::<Hdr>(camera).is_some());
        app.insert_state(AppState::Result);
        app.update();
        assert!(
            app.world().get::<Hdr>(camera).is_none(),
            "result screen uses the original frame pipeline"
        );
        app.insert_state(AppState::Playing);
        app.insert_resource(adapter(wgpu::Backend::Gl));
        app.update();
        assert!(
            app.world().get::<Hdr>(camera).is_none(),
            "unsupported backend must fall back"
        );
    }

    #[test]
    fn bloom_and_tone_mapper_follow_the_pack_and_are_removed_on_exit() {
        let packs = super::super::load_with_reader(
            br#"{"schema_version":1,"enabled":true,"packs":["p.json"]}"#,
            |_| Ok(br#"{"schema_version":1,"id":"fx","scenes":[{"stage":"*","color_pipeline":{"enabled":true,"tonemapping":"none","bloom":{"intensity":0.3,"threshold":1.2,"threshold_softness":0.5}}}]}"#.to_vec()),
        ).unwrap();
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, StatesPlugin))
            .insert_state(AppState::Playing)
            .insert_resource(packs)
            .insert_resource(LoadedStageInfo {
                stage_id: "sn03".into(),
                ..default()
            })
            .insert_resource(adapter(wgpu::Backend::Metal))
            .add_systems(Update, sync);
        let camera = app
            .world_mut()
            .spawn((GameplayCamera, Tonemapping::AcesFitted))
            .id();
        app.update();
        let bloom = app.world().get::<Bloom>(camera).expect("bloom inserted");
        assert_eq!(bloom.intensity, 0.3);
        assert_eq!(bloom.prefilter.threshold, 1.2);
        assert_eq!(bloom.composite_mode, BloomCompositeMode::Additive);
        assert_eq!(
            app.world().get::<Tonemapping>(camera),
            Some(&Tonemapping::None)
        );
        app.insert_state(AppState::Result);
        app.update();
        assert!(
            app.world().get::<Bloom>(camera).is_none(),
            "bloom leaves with the pipeline"
        );
        assert_eq!(
            app.world().get::<Tonemapping>(camera),
            Some(&Tonemapping::AcesFitted)
        );
    }
}
