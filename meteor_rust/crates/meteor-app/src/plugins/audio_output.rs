use super::{
    audio_dsp::{self, Pcm, PeakLimiter},
    audio_plugin::{openal_distance_gain, SpatialSfx},
};
use bevy::{
    audio::{AudioSink, AudioSinkPlayback, PlaybackMode, SpatialAudioSink, SpatialListener},
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use rodio::{mixer::Mixer, DeviceSinkBuilder, MixerDeviceSink, Player, SpatialPlayer};
use std::collections::HashMap;

pub(super) struct OutputPlugin;

impl Plugin for OutputPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<AudioSource>()
            .init_asset_loader::<bevy::audio::AudioLoader>()
            .init_resource::<AudioOutput>()
            .init_resource::<PreparedAudio>()
            .add_systems(
                PostUpdate,
                (prepare_audio, play_queued, update_spatial, cleanup_finished)
                    .chain()
                    .after(bevy::transform::TransformSystems::Propagate),
            );
        #[cfg(feature = "perf-diagnostics")]
        app.add_systems(PostUpdate, log_audio_voices.after(cleanup_finished));
    }
}

#[derive(Resource)]
struct AudioOutput {
    _stream: Option<MixerDeviceSink>,
    mixer: Option<Mixer>,
    rate: rodio::SampleRate,
}

impl Default for AudioOutput {
    fn default() -> Self {
        match DeviceSinkBuilder::open_default_sink() {
            Ok(mut stream) => {
                stream.log_on_drop(false);
                let rate = stream.config().sample_rate();
                let channels = stream.config().channel_count();
                let (mixer, source) = rodio::mixer::mixer(channels, rate);

                mixer.add(rodio::source::Zero::new(channels, rate));
                stream.mixer().add(PeakLimiter::new(source));
                info!("audio output: {} Hz, {} channels, {:?}, buffer {:?}; float sinc + linked peak limiter",
                    rate, channels, stream.config().sample_format(), stream.config().buffer_size());
                Self {
                    _stream: Some(stream),
                    mixer: Some(mixer),
                    rate,
                }
            }
            Err(error) => {
                warn!("audio output unavailable: {error}");
                Self {
                    _stream: None,
                    mixer: None,
                    rate: 48_000.try_into().unwrap(),
                }
            }
        }
    }
}

enum Prepared {
    Loading(Task<Result<Pcm, String>>),
    Ready(Pcm),
    Failed,
}

#[derive(Resource, Default)]
struct PreparedAudio(HashMap<AssetId<AudioSource>, Prepared>);

fn prepare_audio(
    output: Res<AudioOutput>,
    assets: Res<Assets<AudioSource>>,
    players: Query<&AudioPlayer>,
    mut prepared: ResMut<PreparedAudio>,
) {
    if output.mixer.is_none() {
        return;
    }
    prepared.0.retain(|id, _| assets.contains(*id));
    for player in &players {
        let Some(asset) = assets.get(&player.0) else {
            continue;
        };
        prepared.0.entry(player.0.id()).or_insert_with(|| {
            let bytes = asset.bytes.clone();
            let rate = output.rate;
            Prepared::Loading(
                AsyncComputeTaskPool::get().spawn(async move { audio_dsp::decode(bytes, rate) }),
            )
        });
    }
    for state in prepared.0.values_mut() {
        if let Prepared::Loading(task) = state {
            if let Some(result) = block_on(poll_once(task)) {
                *state = match result {
                    Ok(pcm) => Prepared::Ready(pcm),
                    Err(error) => {
                        warn!("audio decode failed: {error}");
                        Prepared::Failed
                    }
                };
            }
        }
    }
}

#[derive(Component)]
struct FailedAudio;

#[derive(Component)]
struct FinishedAudio;

#[derive(Component, PartialEq)]
struct SpatialControls {
    emitter: Vec3,
    left: Vec3,
    right: Vec3,
    volume: f32,
}

type ListenerQuery<'w, 's> = Query<'w, 's, (&'static GlobalTransform, &'static SpatialListener)>;

fn listener_frame(listeners: &ListenerQuery) -> (Vec3, Vec3, Vec3) {
    if let Some((transform, listener)) = listeners.iter().next() {
        (
            transform.translation(),
            transform.transform_point(listener.left_ear_offset),
            transform.transform_point(listener.right_ear_offset),
        )
    } else {
        let listener = SpatialListener::default();
        (
            Vec3::ZERO,
            listener.left_ear_offset,
            listener.right_ear_offset,
        )
    }
}

fn spatial_volume(
    spatial: Option<&SpatialSfx>,
    playback: &PlaybackSettings,
    settings: &crate::app_state::Settings,
    emitter: Vec3,
    listener: Vec3,
) -> f32 {
    spatial.map_or(playback.volume.to_linear(), |spatial| {
        spatial.base_volume.clamp(0.0, 1.0)
            * settings.audio_volume_linear()
            * openal_distance_gain(emitter.distance(listener))
    })
}

#[allow(clippy::type_complexity)]
fn play_queued(
    mut commands: Commands,
    output: Res<AudioOutput>,
    prepared: Res<PreparedAudio>,
    settings: Res<crate::app_state::Settings>,
    listeners: ListenerQuery,
    players: Query<
        (
            Entity,
            &AudioPlayer,
            &PlaybackSettings,
            Option<&GlobalTransform>,
            Option<&SpatialSfx>,
        ),
        (
            Without<AudioSink>,
            Without<SpatialAudioSink>,
            Without<FailedAudio>,
            Without<FinishedAudio>,
        ),
    >,
) {
    let Some(mixer) = &output.mixer else {
        return;
    };
    let (listener, left, right) = listener_frame(&listeners);
    for (entity, player, playback, transform, spatial) in &players {
        let pcm = match prepared.0.get(&player.0.id()) {
            Some(Prepared::Ready(pcm)) => pcm,
            Some(Prepared::Failed) => {
                commands.entity(entity).insert(FailedAudio);
                continue;
            }
            _ => continue,
        };
        let source = pcm.source().playback(
            playback.start_position,
            playback.duration,
            matches!(playback.mode, PlaybackMode::Loop),
        );
        if playback.spatial {
            let position = transform.map_or(Vec3::ZERO, GlobalTransform::translation);
            let scale = playback.spatial_scale.map_or(Vec3::ONE, |scale| scale.0);
            let player = SpatialPlayer::connect_new(
                mixer,
                (position * scale).into(),
                (left * scale).into(),
                (right * scale).into(),
            );

            player.set_volume(if playback.muted {
                0.0
            } else {
                spatial_volume(spatial, playback, &settings, position, listener)
            });
            player.set_speed(playback.speed);
            if playback.paused {
                player.pause();
            }
            player.append(source);
            let mut sink = SpatialAudioSink::new(player);
            if playback.muted {
                sink.mute();
                sink.set_volume(bevy::audio::Volume::Linear(spatial_volume(
                    spatial, playback, &settings, position, listener,
                )));
            }
            commands.entity(entity).insert((
                sink,
                SpatialControls {
                    emitter: position * scale,
                    left: left * scale,
                    right: right * scale,
                    volume: spatial_volume(spatial, playback, &settings, position, listener),
                },
            ));
        } else {
            let player = Player::connect_new(mixer);
            player.set_volume(if playback.muted {
                0.0
            } else {
                playback.volume.to_linear()
            });
            player.set_speed(playback.speed);
            if playback.paused {
                player.pause();
            }
            player.append(source);
            let mut sink = AudioSink::new(player);
            if playback.muted {
                sink.mute();
                sink.set_volume(playback.volume);
            }
            commands.entity(entity).insert(sink);
        }
    }
}

fn update_spatial(
    settings: Res<crate::app_state::Settings>,
    listeners: ListenerQuery,
    mut players: Query<(
        &GlobalTransform,
        &PlaybackSettings,
        Option<&SpatialSfx>,
        &mut SpatialAudioSink,
        &mut SpatialControls,
    )>,
) {
    let (listener, left, right) = listener_frame(&listeners);
    for (transform, playback, spatial, mut sink, mut previous) in &mut players {
        let position = transform.translation();
        let scale = playback.spatial_scale.map_or(Vec3::ONE, |scale| scale.0);
        let next = SpatialControls {
            emitter: position * scale,
            left: left * scale,
            right: right * scale,
            volume: spatial_volume(spatial, playback, &settings, position, listener),
        };
        if previous.emitter != next.emitter {
            sink.set_emitter_position(next.emitter);
        }
        if previous.left != next.left || previous.right != next.right {
            sink.set_ears_position(next.left, next.right);
        }
        if previous.volume != next.volume {
            sink.set_volume(bevy::audio::Volume::Linear(next.volume));
        }
        if *previous != next {
            *previous = next;
        }
    }
}

#[allow(clippy::type_complexity)]
fn cleanup_finished(
    mut commands: Commands,
    players: Query<
        (
            Entity,
            &PlaybackSettings,
            Option<&AudioSink>,
            Option<&SpatialAudioSink>,
            Has<FailedAudio>,
        ),
        (With<AudioPlayer>, Without<FinishedAudio>),
    >,
) {
    for (entity, settings, sink, spatial, failed) in &players {
        let finished = failed
            || sink.is_some_and(AudioSinkPlayback::empty)
            || spatial.is_some_and(AudioSinkPlayback::empty);
        if !finished {
            continue;
        }
        match settings.mode {
            PlaybackMode::Despawn => {
                commands.entity(entity).despawn();
            }
            PlaybackMode::Remove => {
                commands.entity(entity).remove::<(
                    AudioPlayer,
                    PlaybackSettings,
                    AudioSink,
                    SpatialAudioSink,
                    FailedAudio,
                    FinishedAudio,
                    SpatialControls,
                )>();
            }
            PlaybackMode::Once | PlaybackMode::Loop => {
                commands
                    .entity(entity)
                    .remove::<(AudioSink, SpatialAudioSink, SpatialControls)>()
                    .insert(FinishedAudio);
            }
        }
    }
}

#[cfg(feature = "perf-diagnostics")]
#[allow(clippy::type_complexity)]
fn log_audio_voices(
    mut last: Local<Option<std::time::Instant>>,
    output: Res<AudioOutput>,
    players: Query<(
        &PlaybackSettings,
        Option<&AudioSink>,
        Option<&SpatialAudioSink>,
        Has<FinishedAudio>,
        Has<FailedAudio>,
    )>,
) {
    let now = std::time::Instant::now();
    if last.is_some_and(|last| now.duration_since(last).as_secs() < 5) {
        return;
    }
    *last = Some(now);
    let (
        mut playing,
        mut paused,
        mut empty,
        mut retired,
        mut pending,
        mut failed,
        mut spatial_count,
        mut loops,
    ) = (0, 0, 0, 0, 0, 0, 0, 0);
    for (settings, sink, spatial, finished, failure) in &players {
        if failure {
            failed += 1;
        }
        if finished {
            retired += 1;
            continue;
        }
        let sink: Option<&dyn AudioSinkPlayback> = sink
            .map(|s| s as &dyn AudioSinkPlayback)
            .or_else(|| spatial.map(|s| s as &dyn AudioSinkPlayback));
        if let Some(sink) = sink {
            spatial_count += usize::from(spatial.is_some());
            loops += usize::from(matches!(settings.mode, PlaybackMode::Loop));
            if sink.empty() {
                empty += 1;
            } else if sink.is_paused() {
                paused += 1;
            } else {
                playing += 1;
            }
        } else if !failure {
            pending += 1;
        }
    }
    info!(target: "meteor::perf", "audio_perf_v1 rate={} playing={} paused={} empty={} retired={} pending={} failed={} spatial={} loops={}", output.rate, playing, paused, empty, retired, pending, failed, spatial_count, loops);
}

#[cfg(feature = "original-assets")]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn app_with_bus() -> (App, rodio::mixer::MixerSource, Handle<AudioSource>) {
        let rate = 48_000.try_into().unwrap();
        let (mixer, source) = rodio::mixer::mixer(2.try_into().unwrap(), rate);
        mixer.add(rodio::source::Zero::new(2.try_into().unwrap(), rate));
        let mut app = App::new();
        app.init_resource::<Assets<AudioSource>>()
            .init_resource::<crate::app_state::Settings>()
            .init_resource::<PreparedAudio>()
            .insert_resource(AudioOutput {
                _stream: None,
                mixer: Some(mixer),
                rate,
            })
            .add_systems(
                Update,
                (play_queued, update_spatial, cleanup_finished).chain(),
            );
        let handle = app
            .world_mut()
            .resource_mut::<Assets<AudioSource>>()
            .add(AudioSource {
                bytes: Arc::from([]),
            });
        app.world_mut().resource_mut::<PreparedAudio>().0.insert(
            handle.id(),
            Prepared::Ready(Pcm {
                samples: vec![0.8; 480].into(),
                channels: 1.try_into().unwrap(),
                rate,
            }),
        );
        (app, source, handle)
    }

    #[test]
    fn spatial_first_sample_already_has_distance_gain_and_follows_listener() {
        let (mut app, mut bus, handle) = app_with_bus();
        let listener = app
            .world_mut()
            .spawn((GlobalTransform::IDENTITY, SpatialListener::new(0.3)))
            .id();
        let entity = app
            .world_mut()
            .spawn((
                AudioPlayer::new(handle),
                PlaybackSettings {
                    spatial: true,
                    spatial_scale: Some(bevy::audio::SpatialScale::new(0.001)),
                    ..default()
                },
                GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 24.0)),
                SpatialSfx { base_volume: 1.0 },
            ))
            .id();
        app.update();
        assert!(
            (app.world()
                .get::<SpatialAudioSink>(entity)
                .unwrap()
                .volume()
                .to_linear()
                - 0.25)
                .abs()
                < 1e-6
        );

        assert!((bus.next().unwrap() - 0.15).abs() < 1e-5);
        *app.world_mut()
            .get_mut::<GlobalTransform>(listener)
            .unwrap() = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 12.0));
        app.update();
        assert!(
            (app.world()
                .get::<SpatialAudioSink>(entity)
                .unwrap()
                .volume()
                .to_linear()
                - 0.5)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn distant_ambient_loops_start_silent_and_recover_without_restarting() {
        for count in [4, 6] {
            let (mut app, mut bus, handle) = app_with_bus();
            let listener = app
                .world_mut()
                .spawn((GlobalTransform::IDENTITY, SpatialListener::new(0.3)))
                .id();
            let emitters: Vec<_> = (0..count)
                .map(|index| {
                    app.world_mut()
                        .spawn((
                            AudioPlayer::new(handle.clone()),
                            PlaybackSettings {
                                mode: PlaybackMode::Loop,
                                spatial: true,
                                spatial_scale: Some(bevy::audio::SpatialScale::new(0.001)),
                                ..default()
                            },
                            GlobalTransform::from_translation(Vec3::new(
                                0.0,
                                0.0,
                                48.0 + index as f32 * 96.0,
                            )),
                            SpatialSfx { base_volume: 1.0 },
                        ))
                        .id()
                })
                .collect();
            app.update();
            for &entity in &emitters {
                assert_eq!(
                    app.world()
                        .get::<SpatialAudioSink>(entity)
                        .unwrap()
                        .volume()
                        .to_linear(),
                    0.0
                );
            }
            assert_eq!(
                bus.next().unwrap(),
                0.0,
                "distant loops must not add a noise floor"
            );

            *app.world_mut()
                .get_mut::<GlobalTransform>(listener)
                .unwrap() = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 36.0));
            app.update();
            assert_eq!(
                app.world()
                    .get::<SpatialAudioSink>(emitters[0])
                    .unwrap()
                    .volume()
                    .to_linear(),
                0.5
            );
            for &entity in &emitters[1..] {
                assert_eq!(
                    app.world()
                        .get::<SpatialAudioSink>(entity)
                        .unwrap()
                        .volume()
                        .to_linear(),
                    0.0
                );
            }

            *app.world_mut()
                .get_mut::<GlobalTransform>(listener)
                .unwrap() = GlobalTransform::IDENTITY;
            app.update();
            for &entity in &emitters {
                let sink = app.world().get::<SpatialAudioSink>(entity).unwrap();
                assert_eq!(sink.volume().to_linear(), 0.0);
                assert!(
                    !sink.is_paused(),
                    "distance mutes gain without pausing the loop clock"
                );
            }
        }
    }

    #[test]
    fn playback_modes_pause_resume_and_window_owned_completion() {
        let (mut app, mut bus, handle) = app_with_bus();
        let once = app
            .world_mut()
            .spawn((AudioPlayer::new(handle.clone()), PlaybackSettings::ONCE))
            .id();
        let despawn = app
            .world_mut()
            .spawn((AudioPlayer::new(handle.clone()), PlaybackSettings::DESPAWN))
            .id();
        let looping = app
            .world_mut()
            .spawn((
                AudioPlayer::new(handle),
                PlaybackSettings {
                    paused: true,
                    ..PlaybackSettings::LOOP
                },
            ))
            .id();
        app.update();
        assert!(app.world().get::<AudioSink>(looping).unwrap().is_paused());
        for _ in 0..10_000 {
            bus.next();
        }
        app.update();
        assert!(app.world().get_entity(despawn).is_err());
        assert!(app.world().get::<AudioSink>(once).is_none());
        assert!(app.world().get::<FinishedAudio>(once).is_some());
        app.update();
        assert!(
            app.world().get::<AudioSink>(once).is_none(),
            "finished window must not replay"
        );
        assert!(app.world().get_entity(once).is_ok());
        for _ in 0..1000 {
            assert_eq!(bus.next(), Some(0.0));
        }
        app.world().get::<AudioSink>(looping).unwrap().play();
        assert!((0..1000).any(|_| bus.next().unwrap().abs() > 0.1));
        app.world_mut().despawn(looping);
        for _ in 0..1000 {
            bus.next();
        }
        assert_eq!(bus.next(), Some(0.0));
    }

    #[test]
    fn completed_spatial_voice_retires_without_replaying_and_can_be_despawned() {
        let (mut app, _, handle) = app_with_bus();
        let (mixer, mut bus) =
            rodio::mixer::mixer(2.try_into().unwrap(), 48_000.try_into().unwrap());
        app.world_mut().resource_mut::<AudioOutput>().mixer = Some(mixer);
        let entity = app
            .world_mut()
            .spawn((
                AudioPlayer::new(handle),
                PlaybackSettings {
                    spatial: true,
                    ..PlaybackSettings::ONCE
                },
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.update();
        assert!(app.world().get::<SpatialAudioSink>(entity).is_some());
        for _ in 0..4000 {
            bus.next();
        }
        for _ in 0..3 {
            app.update();
            assert!(app.world().get::<SpatialAudioSink>(entity).is_none());
            assert!(app.world().get::<FinishedAudio>(entity).is_some());

            assert!((0..1000).any(|_| bus.next().is_none()));
            assert_eq!(bus.next(), None);
        }
        app.world_mut().despawn(entity);
        app.update();
        assert!(app.world().get_entity(entity).is_err());
    }

    #[test]
    fn async_preparation_waits_for_pcm_and_releases_removed_assets() {
        let (mut app, _bus, _handle) = app_with_bus();
        app.add_plugins(MinimalPlugins);
        let original = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../origin_res/Patch/W10HIT01.wav");
        let handle = app
            .world_mut()
            .resource_mut::<Assets<AudioSource>>()
            .add(AudioSource {
                bytes: std::fs::read(original).unwrap().into(),
            });
        let entity = app
            .world_mut()
            .spawn((AudioPlayer::new(handle.clone()), PlaybackSettings::ONCE))
            .id();
        app.update();
        assert!(
            app.world().get::<AudioSink>(entity).is_none(),
            "pending PCM must not play a raw fallback"
        );
        app.add_systems(PreUpdate, prepare_audio);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.world().get::<AudioSink>(entity).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "PCM worker did not complete"
            );
            app.update();
            std::thread::yield_now();
        }
        app.world_mut().despawn(entity);
        app.world_mut()
            .resource_mut::<Assets<AudioSource>>()
            .remove(handle.id());
        app.update();
        assert!(!app
            .world()
            .resource::<PreparedAudio>()
            .0
            .contains_key(&handle.id()));
    }
}
