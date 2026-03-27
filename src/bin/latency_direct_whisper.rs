#[path = "../hardware.rs"]
mod hardware;

use hardware::{ComputeBackendPreference, MachineProfile, RuntimeTuning};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

const SAMPLE_RATE: f64 = 16_000.0;
const AUDIO_CTX_SAMPLES_PER_UNIT: usize = 320;
const AUDIO_CTX_GRANULARITY: i32 = 64;

struct Cli {
    model: String,
    iterations: usize,
    warmup: usize,
    inputs: Vec<PathBuf>,
}

struct SampleResult {
    path: PathBuf,
    samples: usize,
    transcript: String,
    total_ms: Vec<f64>,
}

fn main() -> Result<(), String> {
    let cli = parse_args(env::args().skip(1))?;
    let model_path =
        find_model(&cli.model).ok_or_else(|| format!("Could not find model '{}'", cli.model))?;
    let machine = MachineProfile::detect();
    let tuning = machine.recommended_tuning();
    let (ctx, backend_name) = create_context(&model_path, &machine, &tuning)?;
    let mut state = ctx
        .create_state()
        .map_err(|e| format!("Failed to create whisper state: {}", e))?;

    println!("Direct whisper-rs latency cross-check");
    println!("  model: {}", model_path.display());
    println!("  warmup runs: {}", cli.warmup);
    println!("  measured runs: {}", cli.iterations);
    println!(
        "  runtime: {} | backend={} | flash_attn={} | threads={} | min_audio_ctx={}",
        machine.summary(),
        backend_name,
        yes_no(tuning.flash_attn && backend_name == "gpu"),
        tuning.n_threads,
        tuning.adaptive_audio_ctx_min
    );
    println!();

    let mut results = Vec::new();
    for input in &cli.inputs {
        let audio = read_f32le_file(input)?;

        for _ in 0..cli.warmup {
            let _ = transcribe_once(&ctx, &mut state, &tuning, &audio)?;
        }

        let mut total_ms = Vec::with_capacity(cli.iterations);
        let mut transcript = None;

        for _ in 0..cli.iterations {
            let (text, total) = transcribe_once(&ctx, &mut state, &tuning, &audio)?;
            transcript = Some(text);
            total_ms.push(total);
        }

        results.push(SampleResult {
            path: input.clone(),
            samples: audio.len(),
            transcript: transcript.unwrap_or_default(),
            total_ms,
        });
    }

    for result in &results {
        print_result(result);
    }

    Ok(())
}

fn parse_args<I>(mut args: I) -> Result<Cli, String>
where
    I: Iterator<Item = String>,
{
    let mut model = String::from("base");
    let mut iterations = 15usize;
    let mut warmup = 2usize;
    let mut inputs = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => model = args.next().ok_or("--model requires a value")?,
            "--iterations" => {
                let value = args.next().ok_or("--iterations requires a value")?;
                iterations = value
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid --iterations value: {}", value))?;
                if iterations == 0 {
                    return Err("--iterations must be greater than 0".to_string());
                }
            }
            "--warmup" => {
                let value = args.next().ok_or("--warmup requires a value")?;
                warmup = value
                    .parse::<usize>()
                    .map_err(|_| format!("Invalid --warmup value: {}", value))?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ if arg.starts_with('-') => return Err(format!("Unknown flag: {}", arg)),
            _ => inputs.push(PathBuf::from(arg)),
        }
    }

    if inputs.is_empty() {
        return Err("No input files provided".to_string());
    }

    Ok(Cli {
        model,
        iterations,
        warmup,
        inputs,
    })
}

fn print_usage() {
    eprintln!(
        "Usage: cargo run --release --bin latency_direct_whisper -- [options] <audio.f32>..."
    );
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --model <name>         Whisper model to use (default: base)");
    eprintln!("  --iterations <count>   Measured runs per sample (default: 15)");
    eprintln!("  --warmup <count>       Warmup runs per sample (default: 2)");
    eprintln!();
    eprintln!("Input format:");
    eprintln!("  Files must be raw f32 little-endian mono audio at 16kHz.");
}

fn transcribe_once(
    ctx: &WhisperContext,
    state: &mut WhisperState,
    tuning: &RuntimeTuning,
    samples: &[f32],
) -> Result<(String, f64), String> {
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(tuning.n_threads);
    params.set_language(Some("en"));
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_no_timestamps(true);
    params.set_suppress_blank(true);
    params.set_no_context(true);
    params.set_single_segment(true);
    params.set_audio_ctx(recommended_audio_ctx(
        ctx,
        tuning.adaptive_audio_ctx_min,
        samples,
    ));

    let t0 = Instant::now();
    state
        .full(params, samples)
        .map_err(|e| format!("Transcription failed: {}", e))?;

    let num_segments = state
        .full_n_segments()
        .map_err(|e| format!("Failed to get segments: {}", e))?;

    let mut text = String::with_capacity(256);
    for i in 0..num_segments {
        if let Ok(segment) = state.full_get_segment_text(i) {
            text.push_str(&segment);
        }
    }

    Ok((text.trim().to_string(), t0.elapsed().as_secs_f64() * 1000.0))
}

fn create_context(
    model_path: &Path,
    machine: &MachineProfile,
    tuning: &RuntimeTuning,
) -> Result<(WhisperContext, &'static str), String> {
    let attempts = match tuning.compute_backend {
        ComputeBackendPreference::CpuOnly => vec![(false, "cpu")],
        ComputeBackendPreference::GpuOnly => vec![(true, "gpu")],
        ComputeBackendPreference::PreferGpu => vec![(true, "gpu"), (false, "cpu")],
    };

    let model_path = model_path.to_str().ok_or("Invalid model path")?;
    let mut last_error = None;

    for (use_gpu, label) in attempts {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(use_gpu);
        params.flash_attn(use_gpu && tuning.flash_attn);
        params.gpu_device(tuning.gpu_device);

        match WhisperContext::new_with_params(model_path, params) {
            Ok(ctx) => return Ok((ctx, label)),
            Err(err) => {
                last_error = Some(format!(
                    "Failed to load whisper model with {} backend on {}: {}",
                    label,
                    machine.summary(),
                    err
                ));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "Failed to load whisper model".to_string()))
}

fn recommended_audio_ctx(
    ctx: &WhisperContext,
    adaptive_audio_ctx_min: i32,
    samples: &[f32],
) -> i32 {
    let required = samples.len().div_ceil(AUDIO_CTX_SAMPLES_PER_UNIT) as i32;
    round_up_to_multiple(required.max(adaptive_audio_ctx_min), AUDIO_CTX_GRANULARITY)
        .min(ctx.n_audio_ctx())
}

fn round_up_to_multiple(value: i32, multiple: i32) -> i32 {
    ((value + multiple - 1) / multiple) * multiple
}

fn find_model(model_name: &str) -> Option<PathBuf> {
    let candidates = [
        format!("ggml-{}.en.bin", model_name),
        format!("ggml-{}.bin", model_name),
        format!("ggml-{}-v3.bin", model_name),
    ];

    let bundle_models_dir = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("Resources").join("models"))
    });

    for filename in &candidates {
        if let Some(ref dir) = bundle_models_dir {
            let path = dir.join(filename);
            if path.exists() {
                return Some(path);
            }
        }

        let local = PathBuf::from("models").join(filename);
        if local.exists() {
            return Some(local);
        }
    }

    None
}

fn read_f32le_file(path: &Path) -> Result<Vec<f32>, String> {
    let bytes = fs::read(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    if bytes.len() % 4 != 0 {
        return Err(format!(
            "Invalid raw audio file {}: byte length {} is not divisible by 4",
            path.display(),
            bytes.len()
        ));
    }

    let mut samples = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }

    Ok(samples)
}

fn print_result(result: &SampleResult) {
    let duration_s = result.samples as f64 / SAMPLE_RATE;
    let stats = Stats::from_samples(&result.total_ms);
    let label = result
        .path
        .file_stem()
        .or_else(|| result.path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| result.path.display().to_string());

    println!("{}", label);
    println!(
        "  duration: {:.2}s ({} samples @ 16kHz)",
        duration_s, result.samples
    );
    println!("  transcript: {}", result.transcript);
    println!(
        "  direct wall: min {:.3} ms | p50 {:.3} ms | p95 {:.3} ms | mean {:.3} ms",
        stats.min, stats.p50, stats.p95, stats.mean
    );
    println!();
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

struct Stats {
    min: f64,
    p50: f64,
    p95: f64,
    mean: f64,
}

impl Stats {
    fn from_samples(values: &[f64]) -> Self {
        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let min = *sorted.first().unwrap_or(&0.0);
        let mean = if sorted.is_empty() {
            0.0
        } else {
            sorted.iter().sum::<f64>() / sorted.len() as f64
        };

        Self {
            min,
            p50: percentile(&sorted, 0.50),
            p95: percentile(&sorted, 0.95),
            mean,
        }
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }

    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}
