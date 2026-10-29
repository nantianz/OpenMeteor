use crate::app_state::{AppState, LaunchRequest};
use bevy::audio::SpatialScale;
use bevy::audio::{
    AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackMode, PlaybackSettings, Volume,
};
use bevy::prelude::*;
use std::collections::HashMap;

pub const STAGE_MUSIC: [(&str, u32); 22] = [
    ("sn01", 2),
    ("sn02", 2),
    ("sn03", 3),
    ("sn04", 5),
    ("sn05", 7),
    ("sn06", 5),
    ("sn07", 8),
    ("sn08", 8),
    ("sn09", 8),
    ("sn10", 9),
    ("sn11", 9),
    ("sn12", 1),
    ("sn13", 4),
    ("sn14", 1),
    ("sn15", 6),
    ("sn16", 6),
    ("sn17", 7),
    ("sn18", 5),
    ("sn19", 4),
    ("sn20", 7),
    ("sn21", 10),
    ("sn22", 5),
];

#[derive(Resource, Default)]
struct BgmEntity(Option<Entity>);

#[derive(Resource)]
pub struct SfxCache {
    map: HashMap<String, Handle<AudioSource>>,

    master_volume: f32,
}

impl Default for SfxCache {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            master_volume: 0.0,
        }
    }
}

impl SfxCache {
    pub fn effective_volume(&self, base: f32) -> f32 {
        (base * self.master_volume).clamp(0.0, 1.0)
    }
}

pub const HIT_SFX: &str = "W00HIT01.wav";
pub const SFX_REFERENCE_DISTANCE: f32 = 100.0 * meteor_core::MODEL_SCALE;
pub const SFX_MAX_DISTANCE: f32 = 400.0 * meteor_core::MODEL_SCALE;
const RODIO_SPATIAL_SCALE: f32 = 0.001;

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct SpatialSfx {
    pub(super) base_volume: f32,
}

pub fn openal_distance_gain(distance: f32) -> f32 {
    if distance >= SFX_MAX_DISTANCE {
        return 0.0;
    }
    SFX_REFERENCE_DISTANCE / distance.max(SFX_REFERENCE_DISTANCE)
}

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(super::audio_output::OutputPlugin)
            .init_resource::<BgmEntity>()
            .init_resource::<SfxCache>()
            .add_systems(
                OnEnter(AppState::Playing),
                play_stage_music.run_if(crate::plugins::starts_fresh_playing_session),
            )
            .add_systems(
                OnExit(AppState::Playing),
                stop_bgm
                    .run_if(|bgm: Res<BgmEntity>| bgm.0.is_some())
                    .run_if(crate::plugins::ends_playing_session),
            )
            .add_systems(OnExit(AppState::Menu), stop_bgm)
            .add_systems(Update, sync_audio_from_settings);
        #[cfg(all(not(target_arch = "wasm32"), not(target_os = "android")))]
        app.add_systems(OnEnter(AppState::Menu), play_menu_music);
        #[cfg(target_os = "android")]
        app.add_systems(
            OnEnter(AppState::Menu),
            (crate::android_assets::load_menu_assets, play_menu_music).chain(),
        );
        #[cfg(target_arch = "wasm32")]
        app.add_systems(
            Update,
            crate::wasm_assets::prepare_menu_assets.run_if(in_state(AppState::Menu)),
        )
        .add_systems(
            Update,
            play_menu_music
                .run_if(in_state(AppState::Menu))
                .run_if(crate::wasm_assets::menu_music_ready)
                .run_if(|bgm: Res<BgmEntity>| bgm.0.is_none())
                .after(crate::wasm_assets::prepare_menu_assets),
        );
    }
}

fn sync_audio_from_settings(
    settings: Res<crate::app_state::Settings>,
    bgm: Res<BgmEntity>,
    mut cache: ResMut<SfxCache>,
    mut players: Query<&mut PlaybackSettings>,
    mut sinks: Query<&mut AudioSink>,
) {
    if settings.is_changed() {
        cache.master_volume = settings.audio_volume_linear();
    }
    if !settings.is_changed() {
        return;
    }
    let Some(entity) = bgm.0 else {
        return;
    };
    let vol = if settings.music_enabled {
        settings.music_volume_linear()
    } else {
        0.0
    };

    if let Ok(mut playback) = players.get_mut(entity) {
        playback.volume = Volume::Linear(vol);
        playback.paused = !settings.music_enabled;
    }
    if let Ok(mut sink) = sinks.get_mut(entity) {
        sink.set_volume(Volume::Linear(vol));
        if settings.music_enabled {
            sink.play();
        } else {
            sink.pause();
        }
    }
}

pub fn resource_roots(workdir: &std::path::Path) -> Vec<std::path::PathBuf> {
    #[cfg(not(test))]
    {
        let _ = workdir;
        vec![std::path::PathBuf::new()]
    }
    #[cfg(test)]
    {
        let mut roots = vec![workdir.to_path_buf()];

        for ancestor in workdir.ancestors().take(6) {
            roots.push(ancestor.join("origin_res"));
            let marker = ancestor.join("assets");
            if marker.is_dir() {
                roots.push(marker);
            } else if let Ok(target) = std::fs::read_to_string(&marker) {
                let target = target.trim();
                if !target.is_empty() {
                    let path = std::path::PathBuf::from(target);
                    roots.push(if path.is_absolute() {
                        path
                    } else {
                        ancestor.join(path)
                    });
                }
            }
        }
        roots.sort();
        roots.dedup();
        roots
    }
}

fn read_first(roots: &[std::path::PathBuf], path: &str) -> Option<Vec<u8>> {
    meteor_asset::runtime_assets::read_legacy_path(path).or_else(|| {
        roots
            .iter()
            .find_map(|root| crate::resource_io::read(root.join(path)).ok())
    })
}

fn load_sfx_bytes(roots: &[std::path::PathBuf], name: &str) -> Option<Vec<u8>> {
    if let Some(bytes) =
        meteor_asset::runtime_assets::read_entry(&format!("sound/{}", name.to_lowercase()))
    {
        return Some(bytes);
    }

    if let Some(bytes) = read_first(roots, &format!("Patch/{name}")) {
        return Some(bytes);
    }
    let pak_bytes = read_first(roots, "waves.pak")?;
    let pak = meteor_asset::pak::PakArchive::from_bytes(&pak_bytes).ok()?;
    let bytes = pak
        .entries()
        .find(|entry| entry.name.to_lowercase().ends_with(&name.to_lowercase()))
        .and_then(|entry| pak.read_entry(&entry.name));
    bytes
}

fn sfx_audio_source(_name: &str, bytes: Vec<u8>) -> AudioSource {
    AudioSource {
        bytes: bytes.into(),
    }
}

fn load_music_handle(
    assets: &mut Assets<AudioSource>,
    roots: &[std::path::PathBuf],
    path: &str,
) -> Option<Handle<AudioSource>> {
    let bytes = read_first(roots, path)?;
    Some(assets.add(AudioSource {
        bytes: bytes.into(),
    }))
}

fn play_bgm(
    commands: &mut Commands,
    bgm: &mut BgmEntity,
    audio_assets: &mut Assets<AudioSource>,
    roots: &[std::path::PathBuf],
    path: &str,
    volume: f32,
    enabled: bool,
) {
    if let Some(entity) = bgm.0.take() {
        commands.entity(entity).despawn();
    }
    let Some(handle) = load_music_handle(audio_assets, roots, path) else {
        warn!("bgm missing: {path}");
        return;
    };
    let entity = commands
        .spawn((
            AudioPlayer::new(handle),
            PlaybackSettings {
                mode: PlaybackMode::Loop,
                volume: Volume::Linear(if enabled { volume } else { 0.0 }),
                paused: !enabled,
                ..default()
            },
        ))
        .id();
    bgm.0 = Some(entity);
    info!("bgm playing: {path}");
}

fn stop_bgm(mut commands: Commands, mut bgm: ResMut<BgmEntity>) {
    if let Some(entity) = bgm.0.take() {
        commands.entity(entity).despawn();
    }
}

fn play_menu_music(
    mut commands: Commands,
    mut bgm: ResMut<BgmEntity>,
    mut audio_assets: ResMut<Assets<AudioSource>>,
    mut cache: ResMut<SfxCache>,
    settings: Res<crate::app_state::Settings>,
) {
    cache.master_volume = settings.audio_volume_linear();
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let roots = resource_roots(&root);
    play_bgm(
        &mut commands,
        &mut bgm,
        &mut audio_assets,
        &roots,
        "music/menu.mp3",
        settings.music_volume_linear(),
        settings.music_enabled,
    );
}

fn play_stage_music(
    mut commands: Commands,
    mut bgm: ResMut<BgmEntity>,
    mut audio_assets: ResMut<Assets<AudioSource>>,
    mut cache: ResMut<SfxCache>,
    request: Res<LaunchRequest>,
    settings: Res<crate::app_state::Settings>,
) {
    cache.master_volume = settings.audio_volume_linear();
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let roots = resource_roots(&root);
    let music_index = STAGE_MUSIC
        .iter()
        .find(|(id, _)| *id == request.stage_id)
        .map(|(_, index)| *index)
        .unwrap_or(1);
    play_bgm(
        &mut commands,
        &mut bgm,
        &mut audio_assets,
        &roots,
        &format!("music/s{music_index}.mp3"),
        settings.music_volume_linear(),
        settings.music_enabled,
    );
}

pub fn play_sfx_at(
    commands: &mut Commands,
    cache: &mut SfxCache,
    audio_assets: &mut Assets<AudioSource>,
    roots: &[std::path::PathBuf],
    name: &str,
    volume: f32,
    position: Vec3,
) -> Option<Entity> {
    play_sfx_at_mode(
        commands,
        cache,
        audio_assets,
        roots,
        name,
        volume,
        position,
        PlaybackMode::Despawn,
    )
}

pub fn play_sfx_windowed_at(
    commands: &mut Commands,
    cache: &mut SfxCache,
    audio_assets: &mut Assets<AudioSource>,
    roots: &[std::path::PathBuf],
    name: &str,
    volume: f32,
    position: Vec3,
) -> Option<Entity> {
    play_sfx_at_mode(
        commands,
        cache,
        audio_assets,
        roots,
        name,
        volume,
        position,
        PlaybackMode::Once,
    )
}

#[allow(clippy::too_many_arguments)]
fn play_sfx_at_mode(
    commands: &mut Commands,
    cache: &mut SfxCache,
    audio_assets: &mut Assets<AudioSource>,
    roots: &[std::path::PathBuf],
    name: &str,
    volume: f32,
    position: Vec3,
    mode: PlaybackMode,
) -> Option<Entity> {
    let handle =
        cache
            .map
            .entry(name.to_string())
            .or_insert_with(|| match load_sfx_bytes(roots, name) {
                Some(bytes) => audio_assets.add(sfx_audio_source(name, bytes)),
                None => Handle::default(),
            });
    if !handle.is_strong() {
        return None;
    }
    Some(
        commands
            .spawn((
                AudioPlayer::new(handle.clone()),
                PlaybackSettings {
                    mode,
                    volume: Volume::Linear(cache.effective_volume(volume)),
                    speed: 1.0,
                    spatial: true,
                    spatial_scale: Some(SpatialScale::new(RODIO_SPATIAL_SCALE)),
                    ..default()
                },
                Transform::from_translation(position),
                SpatialSfx {
                    base_volume: volume,
                },
            ))
            .id(),
    )
}

pub fn play_sfx(
    commands: &mut Commands,
    cache: &mut SfxCache,
    audio_assets: &mut Assets<AudioSource>,
    roots: &[std::path::PathBuf],
    name: &str,
) {
    play_sfx_volume(commands, cache, audio_assets, roots, name, 1.0);
}

pub fn play_sfx_volume(
    commands: &mut Commands,
    cache: &mut SfxCache,
    audio_assets: &mut Assets<AudioSource>,
    roots: &[std::path::PathBuf],
    name: &str,
    volume: f32,
) {
    let handle =
        cache
            .map
            .entry(name.to_string())
            .or_insert_with(|| match load_sfx_bytes(roots, name) {
                Some(bytes) => audio_assets.add(sfx_audio_source(name, bytes)),
                None => Handle::default(),
            });
    if handle.is_strong() {
        commands.spawn((
            AudioPlayer::new(handle.clone()),
            PlaybackSettings {
                mode: PlaybackMode::Despawn,
                volume: Volume::Linear(cache.effective_volume(volume)),
                speed: 1.0,
                ..default()
            },
        ));
    }
}

pub fn play_sfx_loop_at(
    commands: &mut Commands,
    cache: &mut SfxCache,
    audio_assets: &mut Assets<AudioSource>,
    roots: &[std::path::PathBuf],
    name: &str,
    position: Vec3,
) -> Option<Entity> {
    let handle =
        cache
            .map
            .entry(name.to_string())
            .or_insert_with(|| match load_sfx_bytes(roots, name) {
                Some(bytes) => audio_assets.add(sfx_audio_source(name, bytes)),
                None => Handle::default(),
            });
    if !handle.is_strong() {
        return None;
    }
    Some(
        commands
            .spawn((
                AudioPlayer::new(handle.clone()),
                PlaybackSettings {
                    mode: PlaybackMode::Loop,
                    volume: Volume::Linear(cache.effective_volume(1.0)),
                    speed: 1.0,
                    spatial: true,
                    spatial_scale: Some(SpatialScale::new(RODIO_SPATIAL_SCALE)),
                    ..default()
                },
                Transform::from_translation(position),
                SpatialSfx { base_volume: 1.0 },
            ))
            .id(),
    )
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use bevy::audio::{Decodable, Source};

    fn origin_res() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../origin_res")
    }

    #[test]
    fn patch_only_weapon_sounds_resolve_through_production_loader() {
        let root = origin_res();
        for strategy in 9..=11 {
            for kind in ["HIT", "GRD", "FIR"] {
                for variant in 1..=3 {
                    let name = format!("W{strategy:02}{kind}0{variant}.wav");
                    let expected = std::fs::read(root.join("Patch").join(&name)).unwrap();
                    assert_eq!(
                        load_sfx_bytes(std::slice::from_ref(&root), &name),
                        Some(expected),
                        "{name}"
                    );
                }
            }
        }
    }

    fn spawn_disabled_menu_bgm_for_test(
        mut commands: Commands,
        mut bgm: ResMut<BgmEntity>,
        mut audio_assets: ResMut<Assets<AudioSource>>,
    ) {
        play_bgm(
            &mut commands,
            &mut bgm,
            &mut audio_assets,
            &[origin_res()],
            "music/menu.mp3",
            0.5,
            false,
        );
    }

    #[test]
    fn disabled_music_is_prepared_paused_so_enabling_can_resume_it() {
        let mut app = App::new();
        app.init_resource::<BgmEntity>()
            .init_resource::<Assets<AudioSource>>()
            .add_systems(Update, spawn_disabled_menu_bgm_for_test);
        app.update();
        let entity = app
            .world()
            .resource::<BgmEntity>()
            .0
            .expect("disabled BGM should still be prepared");
        let playback = app
            .world()
            .get::<PlaybackSettings>(entity)
            .expect("BGM has playback settings");
        assert!(playback.paused);
        assert_eq!(playback.volume.to_linear(), 0.0);
    }

    #[test]
    fn original_music_decoder_preserves_authored_sample_rates_and_durations() {
        let root = origin_res();
        if !root.join("music/s2.mp3").is_file() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        for (file, sample_rate, duration) in [
            ("menu.mp3", 44_100, 154.625_f32),
            ("s2.mp3", 16_000, 82.899_f32),
        ] {
            let source = AudioSource {
                bytes: std::fs::read(root.join("music").join(file))
                    .expect(file)
                    .into(),
            };
            let decoder = source.decoder();
            assert_eq!(decoder.sample_rate().get(), sample_rate, "{file} rate");
            let decoded_duration = decoder
                .total_duration()
                .expect("original mp3 reports duration")
                .as_secs_f32();
            assert!(
                (decoded_duration - duration).abs() < 0.05,
                "{file} duration {decoded_duration}"
            );
        }
    }

    #[test]
    fn original_sfx_decoder_preserves_authored_rate_and_duration() {
        let root = origin_res();
        let roots = [root];
        for (file, duration) in [
            ("MMRun.wav", 0.3475_f32),
            ("W00FIR01.wav", 0.2678_f32),
            ("Wood0003.wav", 0.7503_f32),
        ] {
            let Some(bytes) = load_sfx_bytes(&roots, file) else {
                eprintln!("origin_res missing, skipping");
                return;
            };
            let source = AudioSource {
                bytes: bytes.into(),
            };
            let decoder = source.decoder();
            assert_eq!(decoder.sample_rate().get(), 22_050, "{file} rate");
            assert_eq!(decoder.channels().get(), 1, "{file} channels");
            let decoded_duration = decoder
                .total_duration()
                .expect("original wav reports duration")
                .as_secs_f32();
            assert!(
                (decoded_duration - duration).abs() < 0.001,
                "{file} duration {decoded_duration}"
            );
        }
    }

    #[test]
    fn shipped_sfx_preparation_preserves_rate_duration_and_peak_headroom() {
        let roots = [origin_res()];
        for file in [
            "MMRun.wav",
            "W02FIR01.wav",
            "Bomb0001.wav",
            "W03HIT02.wav",
            "W04HIT01.wav",
        ] {
            let bytes = load_sfx_bytes(&roots, file).expect(file);
            let original = AudioSource {
                bytes: bytes.clone().into(),
            }
            .decoder();
            let duration = original.total_duration().unwrap();
            for rate in [44_100, 48_000] {
                let pcm =
                    super::super::audio_dsp::decode(bytes.clone().into(), rate.try_into().unwrap())
                        .unwrap();
                let converted = pcm.source();
                assert_eq!(converted.sample_rate().get(), rate);
                assert_eq!(converted.channels().get(), 1);
                assert!(
                    converted.total_duration().unwrap().abs_diff(duration)
                        < std::time::Duration::from_millis(1)
                );
                if file == "W04HIT01.wav" {
                    assert!(
                        pcm.samples.iter().any(|sample| sample.abs() > 1.0),
                        "must preserve intersample headroom"
                    );
                }
            }
        }
    }

    #[test]
    fn resource_roots_find_shared_assets_from_deep_crate_cwd() {
        let deep = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/plugins");
        let roots = resource_roots(&deep);
        for fixture in ["Meteor.res", "pmodel/p0.skc", "waves.pak"] {
            assert!(
                roots.iter().any(|root| root.join(fixture).is_file()),
                "{fixture} unavailable from {deep:?}: {roots:?}"
            );
        }
        assert!(
            roots.iter().any(|root| root.ends_with("assets")),
            "meteor_rust/assets directory or symlink should be a root"
        );
    }

    #[test]
    fn distance_gain_matches_meteor_mixer_max_distance_cutoff() {
        assert_eq!(openal_distance_gain(0.0), 1.0);
        assert_eq!(openal_distance_gain(SFX_REFERENCE_DISTANCE), 1.0);
        assert_eq!(openal_distance_gain(SFX_REFERENCE_DISTANCE * 2.0), 0.5);
        assert!(openal_distance_gain(SFX_MAX_DISTANCE - 0.001) > 0.25);
        assert_eq!(openal_distance_gain(SFX_MAX_DISTANCE), 0.0);
        assert_eq!(openal_distance_gain(SFX_MAX_DISTANCE + 0.001), 0.0);
        assert_eq!(openal_distance_gain(SFX_MAX_DISTANCE * 10.0), 0.0);
    }

    #[test]
    fn origin_res_waves_pak_resolves_hit_sfx() {
        let pak_path = origin_res().join("waves.pak");
        if !pak_path.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let pak = meteor_asset::pak::PakArchive::from_bytes(&std::fs::read(&pak_path).unwrap())
            .expect("waves.pak parses");
        let entry = pak
            .entries()
            .find(|entry| entry.name.to_lowercase().ends_with(&HIT_SFX.to_lowercase()))
            .expect("W00HIT01.wav entry exists");
        assert!(
            entry.name.starts_with("waves/"),
            "entry should live under waves/: {}",
            entry.name
        );
        let bytes = pak.read_entry(&entry.name).expect("readable");
        assert!(bytes.len() > 44, "wav file should have header");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
    }

    #[test]
    fn stage_music_mapping_covers_all_stages() {
        for stage in 1..=22 {
            let id = format!("sn{stage:02}");
            let index = STAGE_MUSIC
                .iter()
                .find(|(sid, _)| *sid == id)
                .unwrap_or_else(|| panic!("{id} missing in STAGE_MUSIC"))
                .1;
            assert!((1..=10).contains(&index), "{id} -> s{index} out of range");
            let mp3 = origin_res().join(format!("music/s{index}.mp3"));
            if !mp3.exists() {
                eprintln!("{id} -> s{index}.mp3 missing, skipping file check");
                continue;
            }
            let bytes = std::fs::read(&mp3).unwrap();
            let mp3_sync = bytes.starts_with(b"ID3") || (bytes.len() > 1 && bytes[0] == 0xFF);
            assert!(mp3_sync, "{id} -> s{index}.mp3 not an MPEG stream");
        }
    }

    #[test]
    fn origin_res_menu_mp3_present() {
        let mp3 = origin_res().join("music/menu.mp3");
        if !mp3.exists() {
            eprintln!("origin_res missing, skipping");
            return;
        }
        let bytes = std::fs::read(&mp3).unwrap();
        assert!(
            bytes.starts_with(b"ID3") || bytes[0] == 0xFF,
            "menu.mp3 not an MPEG stream"
        );
    }
}
