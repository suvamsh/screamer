use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const TARGET_SAMPLE_RATE: u32 = 16000;
const WAVEFORM_WINDOW_DIVISOR: usize = 20; // ~50ms of audio at the device sample rate
const WAVEFORM_WINDOW_FLOOR: usize = 512;
const WAVEFORM_NOISE_GATE: f32 = 0.003;
const WAVEFORM_BOOST: f32 = 7.5;

pub struct Recorder {
    samples: Arc<Mutex<Vec<f32>>>,
    waveform_samples: Arc<Mutex<VecDeque<f32>>>,
    waveform_window_samples: Arc<AtomicUsize>,
    stream: Mutex<Option<Stream>>,
    device_sample_rate: AtomicU32,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::with_capacity(16000 * 10))), // pre-alloc ~10s
            waveform_samples: Arc::new(Mutex::new(VecDeque::with_capacity(4096))),
            waveform_window_samples: Arc::new(AtomicUsize::new(WAVEFORM_WINDOW_FLOOR)),
            stream: Mutex::new(None),
            device_sample_rate: AtomicU32::new(TARGET_SAMPLE_RATE),
        }
    }

    pub fn start(&self) {
        // Clear buffers
        if let Ok(mut s) = self.samples.lock() {
            s.clear();
        }
        if let Ok(mut waveform) = self.waveform_samples.lock() {
            waveform.clear();
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .expect("No input device available");

        let default_config = device
            .default_input_config()
            .expect("No default input config");

        eprintln!(
            "[screamer] Audio device: {:?}, config: {:?}",
            device.name().unwrap_or_default(),
            default_config
        );

        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels();

        self.device_sample_rate
            .store(sample_rate, Ordering::Relaxed);
        self.waveform_window_samples.store(
            ((sample_rate as usize) / WAVEFORM_WINDOW_DIVISOR).max(WAVEFORM_WINDOW_FLOOR),
            Ordering::Relaxed,
        );

        let config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let samples = self.samples.clone();
        let waveform_samples = self.waveform_samples.clone();
        let waveform_window_samples = self.waveform_window_samples.clone();
        let ch = channels as usize;

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let (Ok(mut recorded), Ok(mut waveform)) =
                        (samples.lock(), waveform_samples.lock())
                    {
                        let window_limit = waveform_window_samples.load(Ordering::Relaxed);

                        if ch > 1 {
                            for frame in data.chunks(ch) {
                                let sample = frame[0];
                                recorded.push(sample);
                                waveform.push_back(sample);
                            }
                        } else {
                            recorded.extend_from_slice(data);
                            waveform.extend(data.iter().copied());
                        }

                        while waveform.len() > window_limit {
                            waveform.pop_front();
                        }
                    }
                },
                |err| {
                    eprintln!("[screamer] Audio stream error: {}", err);
                },
                None,
            )
            .expect("Failed to build input stream");

        stream.play().expect("Failed to start audio stream");
        eprintln!(
            "[screamer] Audio stream started ({}Hz, {}ch)",
            sample_rate, channels
        );

        if let Ok(mut s) = self.stream.lock() {
            *s = Some(stream);
        }
    }

    pub fn stop(&self) -> Vec<f32> {
        // Drop the stream to stop recording
        if let Ok(mut s) = self.stream.lock() {
            *s = None;
        }

        let device_rate = self.device_sample_rate.load(Ordering::Relaxed);

        // Take ownership instead of cloning
        let raw_samples = if let Ok(mut s) = self.samples.lock() {
            std::mem::take(&mut *s)
        } else {
            return Vec::new();
        };

        if device_rate == TARGET_SAMPLE_RATE {
            raw_samples
        } else {
            resample(&raw_samples, device_rate, TARGET_SAMPLE_RATE)
        }
    }

    pub fn snapshot(&self) -> Vec<f32> {
        let device_rate = self.device_sample_rate.load(Ordering::Relaxed);

        let raw_samples = if let Ok(samples) = self.samples.lock() {
            samples.clone()
        } else {
            return Vec::new();
        };

        if device_rate == TARGET_SAMPLE_RATE {
            raw_samples
        } else {
            resample(&raw_samples, device_rate, TARGET_SAMPLE_RATE)
        }
    }

    pub fn latest_waveform(&self, bins: usize) -> Vec<f32> {
        if bins == 0 {
            return Vec::new();
        }

        let snapshot: Vec<f32> = if let Ok(waveform) = self.waveform_samples.lock() {
            waveform.iter().copied().collect()
        } else {
            return vec![0.0; bins];
        };

        if snapshot.is_empty() {
            return vec![0.0; bins];
        }

        let mut bins_out = vec![0.0; bins];
        let sample_count = snapshot.len();

        for (bin_idx, level) in bins_out.iter_mut().enumerate() {
            let start = bin_idx * sample_count / bins;
            let end = ((bin_idx + 1) * sample_count / bins)
                .max(start + 1)
                .min(sample_count);
            let slice = &snapshot[start..end];
            let rms = (slice.iter().map(|sample| sample * sample).sum::<f32>()
                / slice.len() as f32)
                .sqrt();

            let gated = ((rms - WAVEFORM_NOISE_GATE) / (1.0 - WAVEFORM_NOISE_GATE)).max(0.0);
            *level = (gated * WAVEFORM_BOOST).min(1.0).powf(0.85);
        }

        bins_out
    }
}

/// Linear interpolation resampler
fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (input.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_idx = i as f64 * ratio;
        let idx = src_idx as usize;
        let frac = (src_idx - idx as f64) as f32;

        let sample = if idx + 1 < input.len() {
            input[idx] * (1.0 - frac) + input[idx + 1] * frac
        } else if idx < input.len() {
            input[idx]
        } else {
            0.0
        };

        output.push(sample);
    }

    output
}

// SAFETY: Stream is Send but not Sync by default. We protect it with a Mutex.
unsafe impl Send for Recorder {}
unsafe impl Sync for Recorder {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::iter;

    #[test]
    fn resample_identity() {
        let input = vec![1.0, 2.0, 3.0, 4.0];
        let output = resample(&input, 16000, 16000);
        assert_eq!(input, output);
    }

    #[test]
    fn resample_empty() {
        let output = resample(&[], 48000, 16000);
        assert!(output.is_empty());
    }

    #[test]
    fn resample_downsample_3x() {
        // 48kHz -> 16kHz = 3:1 ratio
        let input: Vec<f32> = (0..48).map(|i| i as f32).collect();
        let output = resample(&input, 48000, 16000);
        assert_eq!(output.len(), 16);
        // First sample should be 0.0
        assert!((output[0] - 0.0).abs() < 0.01);
        // Second sample should interpolate around index 3.0
        assert!((output[1] - 3.0).abs() < 0.01);
    }

    #[test]
    fn resample_upsample_2x() {
        let input = vec![0.0, 1.0, 2.0, 3.0];
        let output = resample(&input, 16000, 32000);
        assert_eq!(output.len(), 8);
        // Should interpolate: 0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, ...
        assert!((output[0] - 0.0).abs() < 0.01);
        assert!((output[1] - 0.5).abs() < 0.01);
        assert!((output[2] - 1.0).abs() < 0.01);
    }

    #[test]
    fn resample_preserves_approximate_length() {
        let input: Vec<f32> = vec![0.0; 48000]; // 1 second at 48kHz
        let output = resample(&input, 48000, 16000);
        // Should be approximately 16000 samples (1 second at 16kHz)
        assert!((output.len() as i32 - 16000).abs() <= 1);
    }

    #[test]
    fn resample_interpolation_accuracy() {
        // Linear ramp: output should also be a linear ramp
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 99.0).collect();
        let output = resample(&input, 100, 50);
        for i in 1..output.len() {
            assert!(
                output[i] >= output[i - 1],
                "output should be monotonically increasing"
            );
        }
    }

    #[test]
    fn waveform_snapshot_is_flat_for_silence() {
        let recorder = Recorder::new();
        if let Ok(mut waveform) = recorder.waveform_samples.lock() {
            waveform.extend(iter::repeat_n(0.0, 800));
        }

        let bins = recorder.latest_waveform(12);
        assert!(bins.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn waveform_snapshot_tracks_recent_activity() {
        let recorder = Recorder::new();
        if let Ok(mut waveform) = recorder.waveform_samples.lock() {
            waveform.extend(iter::repeat_n(0.0, 320));
            waveform.extend(iter::repeat_n(0.18, 320));
        }

        let bins = recorder.latest_waveform(8);
        assert!(bins[..4].iter().all(|value| *value < 0.05));
        assert!(bins[4..].iter().any(|value| *value > 0.25));
    }
}
