use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const NUM_BARS: usize = 56;
const TARGET_SAMPLE_RATE: u32 = 16000;

pub struct Recorder {
    samples: Arc<Mutex<Vec<f32>>>,
    amplitudes: Arc<[std::sync::atomic::AtomicU32; NUM_BARS]>,
    amp_index: Arc<AtomicUsize>,
    stream: Mutex<Option<Stream>>,
    chunk_buffer: Arc<Mutex<Vec<f32>>>,
    device_sample_rate: Mutex<u32>,
}

impl Recorder {
    pub fn new() -> Self {
        let amplitudes: Arc<[std::sync::atomic::AtomicU32; NUM_BARS]> = Arc::new(
            std::array::from_fn(|_| std::sync::atomic::AtomicU32::new(0)),
        );

        Self {
            samples: Arc::new(Mutex::new(Vec::new())),
            amplitudes,
            amp_index: Arc::new(AtomicUsize::new(0)),
            stream: Mutex::new(None),
            chunk_buffer: Arc::new(Mutex::new(Vec::new())),
            device_sample_rate: Mutex::new(TARGET_SAMPLE_RATE),
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

        // Use the device's default config
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

        if let Ok(mut sr) = self.device_sample_rate.lock() {
            *sr = sample_rate;
        }

        // Use 1 channel at the device's native sample rate
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

        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // If stereo or multi-channel, take only the first channel
                    let mono: Vec<f32> = if channels > 1 {
                        data.chunks(channels as usize)
                            .map(|frame| frame[0])
                            .collect()
                    } else {
                        data.to_vec()
                    };

                    // Append to full recording buffer
                    if let Ok(mut s) = samples.lock() {
                        s.extend_from_slice(&mono);
                    }

                    // Accumulate into chunk buffer for RMS calculation
                    if let Ok(mut cb) = chunk_buffer.lock() {
                        cb.extend_from_slice(&mono);

                        while cb.len() >= chunk_size {
                            let chunk: Vec<f32> = cb.drain(..chunk_size).collect();
                            let rms = (chunk.iter().map(|s| s * s).sum::<f32>()
                                / chunk.len() as f32)
                                .sqrt();
                            let normalized = (rms * 10.0).min(1.0);
                            let encoded = (normalized * 1000.0) as u32;

                            let idx = amp_index.fetch_add(1, Ordering::Relaxed) % NUM_BARS;
                            amplitudes[idx].store(encoded, Ordering::Relaxed);
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
        eprintln!("[screamer] Audio stream started ({}Hz, {}ch)", sample_rate, channels);

        if let Ok(mut s) = self.stream.lock() {
            *s = Some(stream);
        }
    }

    pub fn stop(&self) -> Vec<f32> {
        // Drop the stream to stop recording
        if let Ok(mut s) = self.stream.lock() {
            *s = None;
        }

        let device_rate = self.device_sample_rate.lock().map(|r| *r).unwrap_or(TARGET_SAMPLE_RATE);

        let raw_samples = if let Ok(s) = self.samples.lock() {
            s.clone()
        } else {
            return Vec::new();
        };

        // Resample to 16kHz if needed (whisper requires 16kHz)
        if device_rate == TARGET_SAMPLE_RATE {
            raw_samples
        } else {
            resample(&raw_samples, device_rate, TARGET_SAMPLE_RATE)
        }
    }

    pub fn amplitudes(&self) -> [f32; NUM_BARS] {
        let current_idx = self.amp_index.load(Ordering::Relaxed);
        let mut result = [0.0f32; NUM_BARS];
        for i in 0..NUM_BARS {
            let encoded = self.amplitudes[i].load(Ordering::Relaxed);
            if current_idx > i {
                result[i] = encoded as f32 / 1000.0;
            }
        }
        result
    }
}

/// Simple linear interpolation resampler
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
        let frac = src_idx - idx as f64;

        let sample = if idx + 1 < input.len() {
            input[idx] as f64 * (1.0 - frac) + input[idx + 1] as f64 * frac
        } else if idx < input.len() {
            input[idx] as f64
        } else {
            0.0
        };

        output.push(sample as f32);
    }

    output
}

// SAFETY: Stream is Send but not Sync by default. We protect it with a Mutex.
unsafe impl Send for Recorder {}
unsafe impl Sync for Recorder {}
