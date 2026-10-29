use rodio::{ChannelCount, SampleRate, Source};
use std::{collections::VecDeque, io::Cursor, sync::Arc, time::Duration};

#[derive(Clone, Debug)]
pub(super) struct Pcm {
    pub samples: Arc<[f32]>,
    pub channels: ChannelCount,
    pub rate: SampleRate,
}

impl Pcm {
    pub fn source(&self) -> PcmSource {
        PcmSource {
            pcm: self.clone(),
            cursor: 0,
            start: 0,
            end: self.samples.len(),
            looping: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PcmSource {
    pcm: Pcm,
    cursor: usize,
    start: usize,
    end: usize,
    looping: bool,
}

impl PcmSource {
    pub fn playback(
        mut self,
        start: Option<Duration>,
        duration: Option<Duration>,
        looping: bool,
    ) -> Self {
        let samples_for = |duration: Duration| {
            ((duration.as_secs_f64() * self.pcm.rate.get() as f64) as usize)
                .saturating_mul(self.pcm.channels.get() as usize)
        };
        self.start = start.map_or(0, samples_for).min(self.end);
        self.end = duration.map_or(self.end, |duration| {
            self.start
                .saturating_add(samples_for(duration))
                .min(self.end)
        });
        self.cursor = self.start;
        self.looping = looping && self.start < self.end;
        self
    }
}

impl Iterator for PcmSource {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.cursor == self.end {
            if !self.looping {
                return None;
            }
            self.cursor = self.start;
        }
        let sample = *self.pcm.samples.get(self.cursor)?;
        self.cursor += 1;
        Some(sample)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.looping {
            return (usize::MAX, None);
        }
        let remaining = self.end - self.cursor;
        (remaining, Some(remaining))
    }
}

impl Source for PcmSource {
    fn current_span_len(&self) -> Option<usize> {
        if self.looping {
            let frames = (self.pcm.rate.get() as usize / 200).max(1);
            Some(frames * self.pcm.channels.get() as usize)
        } else {
            Some(self.end - self.cursor)
        }
    }
    fn channels(&self) -> ChannelCount {
        self.pcm.channels
    }
    fn sample_rate(&self) -> SampleRate {
        self.pcm.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        if self.looping {
            return None;
        }
        Some(Duration::from_secs_f64(
            (self.end - self.start) as f64
                / self.pcm.channels.get() as f64
                / self.pcm.rate.get() as f64,
        ))
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        let frames = (position.as_secs_f64() * self.pcm.rate.get() as f64) as usize;
        self.cursor = self
            .start
            .saturating_add(frames.saturating_mul(self.pcm.channels.get() as usize))
            .min(self.end);
        Ok(())
    }
}

pub(super) fn decode(bytes: Arc<[u8]>, target_rate: SampleRate) -> Result<Pcm, String> {
    let decoder = rodio::Decoder::builder()
        .with_byte_len(bytes.len() as u64)
        .with_data(Cursor::new(bytes))
        .build()
        .map_err(|error| error.to_string())?;
    let channels = decoder.channels();
    let rate = decoder.sample_rate();
    let samples: Vec<f32> = decoder.collect();
    let samples = resample(
        &samples,
        channels.get() as usize,
        rate.get(),
        target_rate.get(),
    )?;
    Ok(Pcm {
        samples: samples.into(),
        channels,
        rate: target_rate,
    })
}

pub(super) fn resample(
    samples: &[f32],
    channels: usize,
    from: u32,
    to: u32,
) -> Result<Vec<f32>, String> {
    if channels == 0
        || from == 0
        || to == 0
        || samples.is_empty()
        || !samples.len().is_multiple_of(channels)
    {
        return Err("invalid PCM shape/rate".into());
    }
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err("non-finite PCM sample".into());
    }
    if from == to {
        return Ok(samples.to_vec());
    }
    const PHASES: usize = 1024;
    const TAPS: usize = 64;
    let cutoff = if to < from {
        to as f64 / from as f64 * 0.94
    } else {
        1.0
    };
    let kernels: Vec<[f32; TAPS]> = (0..PHASES)
        .map(|phase| {
            let fraction = phase as f64 / PHASES as f64;
            let mut kernel = [0.0; TAPS];
            let mut total = 0.0;
            for (tap, weight) in kernel.iter_mut().enumerate() {
                let distance = tap as f64 - (TAPS / 2 - 1) as f64 - fraction;
                let angle = std::f64::consts::PI * distance * cutoff;
                let sinc = if angle.abs() < 1e-12 {
                    1.0
                } else {
                    angle.sin() / angle
                };
                let window_angle = std::f64::consts::PI * distance / (TAPS / 2) as f64;
                let window = if distance.abs() < (TAPS / 2) as f64 {
                    0.42 + 0.5 * window_angle.cos() + 0.08 * (2.0 * window_angle).cos()
                } else {
                    0.0
                };
                *weight = (sinc * cutoff * window) as f32;
                total += *weight;
            }
            for weight in &mut kernel {
                *weight /= total;
            }
            kernel
        })
        .collect();
    let frames = samples.len() / channels;
    let out_frames = ((frames as u64 * to as u64 + from as u64 / 2) / from as u64) as usize;
    let mut output = vec![0.0; out_frames * channels];
    for frame in 0..out_frames {
        let numerator = frame as u64 * from as u64;
        let mut base = (numerator / to as u64) as isize;
        let mut phase =
            (((numerator % to as u64) * PHASES as u64 + to as u64 / 2) / to as u64) as usize;
        if phase == PHASES {
            base += 1;
            phase = 0;
        }
        let mut weight_sum = 0.0;
        for (tap, weight) in kernels[phase].iter().enumerate() {
            let index = base + tap as isize - (TAPS / 2 - 1) as isize;
            if index < 0 || index >= frames as isize {
                continue;
            }
            for channel in 0..channels {
                output[frame * channels + channel] +=
                    samples[index as usize * channels + channel] * weight;
            }
            weight_sum += weight;
        }
        for channel in 0..channels {
            output[frame * channels + channel] /= weight_sum;
        }
    }
    Ok(output)
}

pub(super) struct PeakLimiter<S> {
    input: S,
    channels: usize,
    delay: Vec<f32>,
    cursor: usize,
    channel: usize,
    frame: u64,
    peaks: VecDeque<(u64, f32)>,
    gain: f32,
    attack: f32,
    release: f32,
    output: Vec<f32>,
    ended: bool,
    tail: usize,
}

pub(super) const OUTPUT_CEILING: f32 = 0.95;

impl<S: Source<Item = f32>> PeakLimiter<S> {
    pub fn new(input: S) -> Self {
        let rate = input.sample_rate().get() as f32;
        let channels = input.channels().get() as usize;
        let frames = (rate * 0.005).ceil() as usize;
        Self {
            input,
            channels,
            delay: vec![0.0; frames.max(1) * channels],
            cursor: 0,
            channel: 0,
            frame: 0,
            peaks: VecDeque::with_capacity(frames + 1),
            gain: 1.0,
            attack: (-1.0 / (0.0005 * rate)).exp(),
            release: (-1.0 / (0.08 * rate)).exp(),
            output: vec![0.0; channels],
            ended: false,
            tail: frames.max(1),
        }
    }
}

impl<S: Source<Item = f32>> Iterator for PeakLimiter<S> {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.channel == 0 {
            if self.ended && self.tail == 0 {
                return None;
            }
            let mut peak = 0.0f32;
            let mut output_peak = 0.0f32;
            for channel in 0..self.channels {
                let sample = match self.input.next() {
                    Some(sample) if sample.is_finite() => sample,
                    Some(_) => 0.0,
                    None => {
                        self.ended = true;
                        0.0
                    }
                };
                self.output[channel] = self.delay[self.cursor + channel];
                self.delay[self.cursor + channel] = sample;
                peak = peak.max(sample.abs());
                output_peak = output_peak.max(self.output[channel].abs());
            }
            if self.ended {
                self.tail -= 1;
            }
            let lookahead = (self.delay.len() / self.channels) as u64;
            while self
                .peaks
                .front()
                .is_some_and(|(index, _)| *index + lookahead < self.frame)
            {
                self.peaks.pop_front();
            }
            while self.peaks.back().is_some_and(|(_, value)| *value <= peak) {
                self.peaks.pop_back();
            }
            self.peaks.push_back((self.frame, peak));
            let window_peak = self.peaks.front().map_or(0.0, |(_, value)| *value);
            let target = (OUTPUT_CEILING / window_peak.max(OUTPUT_CEILING)).min(1.0);
            let coefficient = if target < self.gain {
                self.attack
            } else {
                self.release
            };
            self.gain = target + coefficient * (self.gain - target);

            self.gain = self
                .gain
                .min(OUTPUT_CEILING / output_peak.max(OUTPUT_CEILING));
            for sample in &mut self.output {
                *sample *= self.gain;
            }
            self.cursor = (self.cursor + self.channels) % self.delay.len();
            self.frame += 1;
        }
        let sample = self.output[self.channel];
        self.channel = (self.channel + 1) % self.channels;
        Some(sample)
    }
}

impl<S: Source<Item = f32>> Source for PeakLimiter<S> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        self.input.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.input.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(samples: Vec<f32>, channels: u16, rate: u32) -> Pcm {
        Pcm {
            samples: samples.into(),
            channels: channels.try_into().unwrap(),
            rate: rate.try_into().unwrap(),
        }
    }

    #[test]
    fn sinc_preserves_intersample_peaks_until_after_volume() {
        let input = vec![0.0, 1.0, 1.0, -1.0, -1.0, 0.0];
        let output = resample(&input, 1, 22050, 48000).unwrap();
        assert!(output.iter().any(|x| x.abs() > 1.05));
        assert!(output.iter().all(|x| (x * 0.5).abs() < 1.0));
    }

    #[test]
    fn loop_and_trim_share_pcm_and_keep_frame_boundaries() {
        let pcm = pcm(vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3], 2, 1000);
        let mut source = pcm.source().playback(
            Some(Duration::from_millis(1)),
            Some(Duration::from_millis(2)),
            true,
        );
        assert!(Arc::ptr_eq(&pcm.samples, &source.pcm.samples));
        assert_eq!(
            source.by_ref().take(8).collect::<Vec<_>>(),
            vec![0.2, -0.2, 0.3, -0.3, 0.2, -0.2, 0.3, -0.3]
        );
        source.try_seek(Duration::ZERO).unwrap();
        assert_eq!(source.next(), Some(0.2));
        assert_eq!(
            pcm.source()
                .playback(None, Some(Duration::ZERO), true)
                .next(),
            None
        );
    }

    #[test]
    fn rodio_queue_preserves_loop_samples_across_spans_and_seek() {
        use rodio::{source::UniformSourceIterator, Player};
        for channels in [1, 2] {
            for rate in [44_100, 48_000] {
                let samples: Vec<f32> = (0..14 * channels).map(|i| i as f32 / 32.0).collect();
                let pcm = pcm(samples.clone(), channels as u16, rate);
                let (player, queue) = Player::new();
                player.append(pcm.source().playback(None, None, true));
                let mut output = UniformSourceIterator::new(queue, pcm.channels, pcm.rate);
                for i in 0..rate as usize * channels {
                    assert_eq!(output.next(), Some(samples[i % samples.len()]));
                }
                drop(player);
                assert!((0..2000).any(|_| output.next().is_none()));
                let mut looped = pcm.source().playback(None, None, true);
                looped.try_seek(Duration::from_secs(10)).unwrap();
                assert!(looped.current_span_len().unwrap() > 0);
                assert_eq!(looped.next(), Some(samples[0]));
                let mut finite = pcm.source();
                finite.try_seek(Duration::from_secs(10)).unwrap();
                assert_eq!(finite.current_span_len(), Some(0));
                assert_eq!(finite.next(), None);
            }
        }
    }

    #[test]
    fn rodio_loop_speed_changes_and_pause_remain_responsive() {
        use rodio::{source::UniformSourceIterator, Player};
        let rate = 48_000;
        let pcm = pcm(
            (0..rate)
                .map(|i| (i as f32 * std::f32::consts::TAU * 400.0 / rate as f32).sin())
                .collect(),
            1,
            rate,
        );
        let (player, queue) = Player::new();
        player.append(pcm.source().playback(None, None, true));
        let mut output = UniformSourceIterator::new(queue, pcm.channels, pcm.rate);
        for speed in [1.0, 2.0, 0.5, 1.0] {
            player.set_speed(speed);

            for _ in 0..2400 {
                output.next();
            }
            let samples: Vec<f32> = output.by_ref().take(4800).collect();
            let crossings = samples
                .windows(2)
                .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
                .count();
            assert!(
                (crossings as f32 - 40.0 * speed).abs() <= 2.0,
                "speed={speed}, crossings={crossings}"
            );
        }
        player.pause();
        for _ in 0..2400 {
            output.next();
        }
        assert!(output.by_ref().take(1000).all(|x| x == 0.0));
        player.play();
        assert!(output.by_ref().take(2400).any(|x| x.abs() > 0.5));
    }

    #[test]
    fn resampling_preserves_stereo_duration_and_filters_downsample_aliases() {
        for (from, to) in [(16000, 48000), (48000, 44100), (96000, 48000)] {
            let input: Vec<_> = (0..from)
                .flat_map(|i| {
                    let x = (i as f32 * std::f32::consts::TAU * 1000.0 / from as f32).sin() * 0.4;
                    [x, -x * 0.5]
                })
                .collect();
            let output = resample(&input, 2, from, to).unwrap();
            assert_eq!(output.len(), to as usize * 2);
            for frame in output.chunks_exact(2) {
                assert!((frame[0] * 0.5 + frame[1]).abs() < 1e-6);
            }
            let rms = (output[200..output.len() - 200]
                .chunks_exact(2)
                .map(|v| v[0] * v[0])
                .sum::<f32>()
                / ((output.len() - 400) / 2) as f32)
                .sqrt();
            assert!((rms - 0.4 / 2.0f32.sqrt()).abs() < 0.003);
        }
        let input: Vec<_> = (0..9600)
            .map(|i| (i as f32 * std::f32::consts::TAU * 30000.0 / 96000.0).sin())
            .collect();
        let output = resample(&input, 1, 96000, 48000).unwrap();
        let rms = (output[100..4700].iter().map(|v| v * v).sum::<f32>() / 4600.0).sqrt();
        assert!(rms < 0.002, "aliased RMS {rms}");
    }

    #[test]
    fn limiter_preserves_quiet_audio_and_drains_the_last_frame() {
        let input = vec![0.2, -0.1, 0.3, -0.2];
        let output: Vec<_> = PeakLimiter::new(pcm(input.clone(), 2, 48000).source()).collect();
        assert_eq!(output.len(), input.len() + 480);
        assert!(output[..480].iter().all(|x| *x == 0.0));
        assert_eq!(&output[480..], input);
    }

    #[test]
    fn limiter_bounds_overlapping_transients_without_changing_stereo_ratio() {
        let input: Vec<_> = (0..12000)
            .flat_map(|i| {
                let amplitude = if (2000..4000).contains(&i) { 8.0 } else { 0.2 };
                let sample = amplitude * (i as f32 * 0.17).sin();
                [sample, sample * -0.25]
            })
            .collect();
        let output: Vec<_> = PeakLimiter::new(pcm(input, 2, 48000).source()).collect();
        assert!(output
            .iter()
            .all(|x| x.is_finite() && x.abs() <= OUTPUT_CEILING + 1e-6));
        for pair in output.chunks_exact(2) {
            assert!((pair[0] * 0.25 + pair[1]).abs() < 1e-6);
        }
    }
}
