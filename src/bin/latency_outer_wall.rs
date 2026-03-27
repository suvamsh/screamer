#[path = "../hardware.rs"]
mod hardware;

#[allow(dead_code)]
#[path = "../transcriber.rs"]
mod transcriber;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use transcriber::{AudioContextStrategy, Transcriber, TranscriberConfig};

const SAMPLE_RATE: f64 = 16_000.0;

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
    outer_ms: Vec<f64>,
    internal_ms: Vec<f64>,
    drift_ms: Vec<f64>,
}

fn main() -> Result<(), String> {
    let cli = parse_args(env::args().skip(1))?;
    let model_path = Transcriber::find_model(&cli.model)
        .ok_or_else(|| format!("Could not find model '{}'", cli.model))?;

    let config = TranscriberConfig {
        audio_ctx: AudioContextStrategy::Adaptive,
        reuse_state: true,
        no_timestamps: true,
        ..TranscriberConfig::default()
    };

    let transcriber = Transcriber::with_config(&model_path, config)?;

    println!("Screamer outer-wall latency cross-check");
    println!("  model: {}", model_path.display());
    println!("  warmup runs: {}", cli.warmup);
    println!("  measured runs: {}", cli.iterations);
    println!("  runtime: {}", transcriber.runtime_summary());
    println!();

    let mut results = Vec::new();
    for input in &cli.inputs {
        let audio = read_f32le_file(input)?;

        for _ in 0..cli.warmup {
            let _ = transcriber.transcribe(&audio)?;
        }

        let mut outer_ms = Vec::with_capacity(cli.iterations);
        let mut internal_ms = Vec::with_capacity(cli.iterations);
        let mut drift_ms = Vec::with_capacity(cli.iterations);
        let mut transcript = None;

        for _ in 0..cli.iterations {
            let outer_t0 = Instant::now();
            let output = transcriber.transcribe_profiled(&audio)?;
            let outer = outer_t0.elapsed().as_secs_f64() * 1000.0;
            let internal = output.profile.total.as_secs_f64() * 1000.0;
            let drift = outer - internal;

            transcript = Some(output.text);
            outer_ms.push(outer);
            internal_ms.push(internal);
            drift_ms.push(drift);
        }

        results.push(SampleResult {
            path: input.clone(),
            samples: audio.len(),
            transcript: transcript.unwrap_or_default(),
            outer_ms,
            internal_ms,
            drift_ms,
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
    eprintln!("Usage: cargo run --release --bin latency_outer_wall -- [options] <audio.f32>...");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --model <name>         Whisper model to use (default: base)");
    eprintln!("  --iterations <count>   Measured runs per sample (default: 15)");
    eprintln!("  --warmup <count>       Warmup runs per sample (default: 2)");
    eprintln!();
    eprintln!("Input format:");
    eprintln!("  Files must be raw f32 little-endian mono audio at 16kHz.");
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
    let outer = Stats::from_samples(&result.outer_ms);
    let internal = Stats::from_samples(&result.internal_ms);
    let drift = Stats::from_samples(&result.drift_ms);
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
        "  outer wall: min {:.3} ms | p50 {:.3} ms | p95 {:.3} ms | mean {:.3} ms",
        outer.min, outer.p50, outer.p95, outer.mean
    );
    println!(
        "  internal total: min {:.3} ms | p50 {:.3} ms | p95 {:.3} ms | mean {:.3} ms",
        internal.min, internal.p50, internal.p95, internal.mean
    );
    println!(
        "  outer-internal drift: min {:.3} ms | p50 {:.3} ms | p95 {:.3} ms | mean {:.3} ms",
        drift.min, drift.p50, drift.p95, drift.mean
    );
    println!();
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
