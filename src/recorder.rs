use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const NUM_BARS: usize = 56;
const TARGET_SAMPLE_RATE: u32 = 16000;

pub struct Recorder {
    samples: Arc<Mutex<Vec<f32>>>,
    amplitudes: Arc<[AtomicU32; NUM_BARS]>,
    amp_index: Arc<AtomicUsize>,
    stream: Mutex<Option<Stream>>,
    chunk_buffer: Arc<Mutex<Vec<f32>>>,
    device_sample_rate: AtomicU32,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            samples: Arc::new(Mutex::new(Vec::with_capacity(16000 * 10))), // pre-alloc ~10s
            amplitudes: Arc::new(std::array::from_fn(|_| AtomicU32::new(0))),
            amp_index: Arc::new(AtomicUsize::new(0)),
            stream: Mutex::new(None),
            chunk_buffer: Arc::new(Mutex::new(Vec::with_capacity(4800))),
            device_sample_rate: AtomicU32::new(TARGET_SAMPLE_RATE),
        }
    }

    pub fn start(&self) {
        // Clear buffers
        if let Ok(mut s) = self.samples.lock() {
            s.clear();
        }
        if let Ok(mut cb) = self.chunk_buffer.lock() {
            cb.clear();
        }
        for amp in self.amplitudes.iter() {
            amp.store(0, Ordering::Relaxed);
        }
        self.amp_index.store(0, Ordering::Relaxed);

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

        self.device_sample_rate.store(sample_rate, Ordering::Relaxed);

        let config = cpal::StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let chunk_size = (sample_rate as usize) / 10; // 100ms chunks
        let samples = self.samples.clone();
        let amplitudes = self.amplitudes.clone();
        let amp_index = self.amp_index.clone();
        let chunk_buffer = self.chunk_buffer.clone();
        let ch = channels as usize;

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // Append mono samples directly — no intermediate Vec allocation
                    if let Ok(mut s) = samples.lock() {
                        if ch > 1 {
                            s.extend(data.chunks(ch).map(|frame| frame[0]));
                        } else {
                            s.extend_from_slice(data);
                        }
                    }

                    // Accumulate for RMS calculation
                    if let Ok(mut cb) = chunk_buffer.lock() {
                        if ch > 1 {
                            cb.extend(data.chunks(ch).map(|frame| frame[0]));
                        } else {
                            cb.extend_from_slice(data);
                        }

                        while cb.len() >= chunk_size {
                            // Compute RMS in-place — no collect()
                            let rms = (cb[..chunk_size]
                                .iter()
                                .map(|s| s * s)
                                .sum::<f32>()
                                / chunk_size as f32)
                                .sqrt();
                            // Store raw RMS (no boost) — let the overlay handle display scaling
                            let encoded = (rms.min(1.0) * 10000.0) as u32;

                            let idx = amp_index.fetch_add(1, Ordering::Relaxed) % NUM_BARS;
                            amplitudes[idx].store(encoded, Ordering::Relaxed);

                            cb.drain(..chunk_size);
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

    /// Returns the most recent raw RMS amplitude value (0.0–1.0).
    pub fn latest_amplitude(&self) -> f32 {
        let idx = self.amp_index.load(Ordering::Relaxed);
        if idx == 0 {
            return 0.0;
        }
        self.amplitudes[(idx - 1) % NUM_BARS].load(Ordering::Relaxed) as f32 / 10000.0
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
            assert!(output[i] >= output[i - 1], "output should be monotonically increasing");
        }
    }
}
