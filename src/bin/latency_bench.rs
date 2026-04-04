#[path = "../bench_support.rs"]
mod bench_support;

use bench_support::{read_f32le_file, sample_label, Stats};
use screamer_whisper::{AudioContextStrategy, Transcriber, TranscriberConfig};
use std::env;
use std::path::PathBuf;

const SAMPLE_RATE: f64 = 16_000.0;

struct Cli {
    model: String,
    iterations: usize,
    warmup: usize,
    threads: Option<i32>,
    reuse_state: bool,
    no_timestamps: bool,
    audio_ctx: AudioContextStrategy,
    inputs: Vec<PathBuf>,
}

struct SampleResult {
    path: PathBuf,
    samples: usize,
    transcript: String,
    warmup_total_ms: Vec<f64>,
    run_total_ms: Vec<f64>,
    run_state_ms: Vec<f64>,
    run_inference_ms: Vec<f64>,
    run_extract_ms: Vec<f64>,
}

fn main() -> Result<(), String> {
    let cli = parse_args(env::args().skip(1))?;
    let model_path = Transcriber::find_model(&cli.model)
        .ok_or_else(|| format!("Could not find model '{}'", cli.model))?;

    let mut config = TranscriberConfig {
        reuse_state: cli.reuse_state,
        no_timestamps: cli.no_timestamps,
        audio_ctx: cli.audio_ctx,
        ..TranscriberConfig::default()
    };
    if let Some(threads) = cli.threads {
        config.n_threads = threads;
    }

    println!("Screamer latency bench");
    println!("  model: {}", model_path.display());
    println!("  warmup runs: {}", cli.warmup);
    println!("  measured runs: {}", cli.iterations);
    println!("  threads: {}", config.n_threads);
    println!("  reuse state: {}", yes_no(config.reuse_state));
    println!("  generate timestamps: {}", yes_no(!config.no_timestamps));
    println!("  audio ctx: {}", audio_ctx_label(config.audio_ctx));

    let transcriber = Transcriber::with_config(&model_path, config)?;
    println!("  runtime: {}", transcriber.runtime_summary());
    println!();

    let mut results = Vec::new();
    for input in &cli.inputs {
        let audio = read_f32le_file(input)?;
        let transcript = transcriber.transcribe(&audio)?;

        let mut warmup_total_ms = Vec::with_capacity(cli.warmup);
        for _ in 0..cli.warmup {
            let run = transcriber.transcribe_profiled(&audio)?;
            warmup_total_ms.push(duration_ms(run.profile.total));
        }

        let mut run_total_ms = Vec::with_capacity(cli.iterations);
        let mut run_state_ms = Vec::with_capacity(cli.iterations);
        let mut run_inference_ms = Vec::with_capacity(cli.iterations);
        let mut run_extract_ms = Vec::with_capacity(cli.iterations);

        for _ in 0..cli.iterations {
            let run = transcriber.transcribe_profiled(&audio)?;
            run_total_ms.push(duration_ms(run.profile.total));
            run_state_ms.push(duration_ms(run.profile.state_acquire));
            run_inference_ms.push(duration_ms(run.profile.inference));
            run_extract_ms.push(duration_ms(run.profile.extract));
        }

        results.push(SampleResult {
            path: input.clone(),
            samples: audio.len(),
            transcript,
            warmup_total_ms,
            run_total_ms,
            run_state_ms,
            run_inference_ms,
            run_extract_ms,
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
    let mut threads = None;
    let mut reuse_state = true;
    let mut no_timestamps = true;
    let mut audio_ctx = AudioContextStrategy::Adaptive;
    let mut inputs = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => {
                model = args.next().ok_or("--model requires a value")?;
            }
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
            "--threads" => {
                let value = args.next().ok_or("--threads requires a value")?;
                let parsed = value
                    .parse::<i32>()
                    .map_err(|_| format!("Invalid --threads value: {}", value))?;
                if parsed <= 0 {
                    return Err("--threads must be greater than 0".to_string());
                }
                threads = Some(parsed);
            }
            "--fresh-state" => {
                reuse_state = false;
            }
            "--timestamps" => {
                no_timestamps = false;
            }
            "--full-audio-ctx" => {
                audio_ctx = AudioContextStrategy::ModelDefault;
            }
            "--audio-ctx" => {
                let value = args.next().ok_or("--audio-ctx requires a value")?;
                let parsed = value
                    .parse::<i32>()
                    .map_err(|_| format!("Invalid --audio-ctx value: {}", value))?;
                if parsed <= 0 {
                    return Err("--audio-ctx must be greater than 0".to_string());
                }
                audio_ctx = AudioContextStrategy::Fixed(parsed);
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            _ if arg.starts_with('-') => {
                return Err(format!("Unknown flag: {}", arg));
            }
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
        threads,
        reuse_state,
        no_timestamps,
        audio_ctx,
        inputs,
    })
}

fn print_usage() {
    eprintln!("Usage: cargo run --release --bin latency_bench -- [options] <audio.f32>...");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --model <name>         Whisper model to use (default: base)");
    eprintln!("  --iterations <count>   Measured runs per sample (default: 15)");
    eprintln!("  --warmup <count>       Warmup runs per sample (default: 2)");
    eprintln!("  --threads <count>      Decoder threads to use");
    eprintln!("  --fresh-state          Recreate Whisper state every run");
    eprintln!("  --timestamps           Generate timestamps during decode");
    eprintln!("  --full-audio-ctx       Use the model's default audio context");
    eprintln!("  --audio-ctx <count>    Override whisper audio context");
    eprintln!();
    eprintln!("Input format:");
    eprintln!("  Files must be raw f32 little-endian mono audio at 16kHz.");
}

fn print_result(result: &SampleResult) {
    let duration_s = result.samples as f64 / SAMPLE_RATE;
    let stats = Stats::from_samples(&result.run_total_ms);
    let warmup_stats = Stats::from_samples(&result.warmup_total_ms);
    let state_stats = Stats::from_samples(&result.run_state_ms);
    let inference_stats = Stats::from_samples(&result.run_inference_ms);
    let extract_stats = Stats::from_samples(&result.run_extract_ms);
    let label = sample_label(&result.path);

    println!("{}", label);
    println!(
        "  duration: {:.2}s ({} samples @ 16kHz)",
        duration_s, result.samples
    );
    println!("  transcript: {}", result.transcript);

    if !result.warmup_total_ms.is_empty() {
        println!(
            "  warmup total: min {:.1} ms | p50 {:.1} ms | max {:.1} ms",
            warmup_stats.min,
            warmup_stats.p50,
            max_sample(&result.warmup_total_ms)
        );
    }

    println!(
        "  measured total: min {:.1} ms | p50 {:.1} ms | p95 {:.1} ms | max {:.1} ms | mean {:.1} ms",
        stats.min,
        stats.p50,
        stats.p95,
        max_sample(&result.run_total_ms),
        stats.mean
    );
    println!(
        "  stage mean: state {:.1} ms | infer {:.1} ms | extract {:.1} ms",
        state_stats.mean, inference_stats.mean, extract_stats.mean
    );
    println!();
}

fn audio_ctx_label(audio_ctx: AudioContextStrategy) -> String {
    match audio_ctx {
        AudioContextStrategy::Adaptive => "adaptive".to_string(),
        AudioContextStrategy::Fixed(value) => value.to_string(),
        AudioContextStrategy::ModelDefault => "default".to_string(),
    }
}

fn duration_ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn max_sample(values: &[f64]) -> f64 {
    values.iter().copied().reduce(f64::max).unwrap_or(0.0)
}
