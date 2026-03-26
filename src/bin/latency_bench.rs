use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

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
    warmup_ms: Vec<f64>,
    run_ms: Vec<f64>,
}

fn main() -> Result<(), String> {
    let cli = parse_args(env::args().skip(1))?;
    let model_path =
        find_model(&cli.model).ok_or_else(|| format!("Could not find model '{}'", cli.model))?;

    println!("Screamer latency bench");
    println!("  model: {}", model_path.display());
    println!("  warmup runs: {}", cli.warmup);
    println!("  measured runs: {}", cli.iterations);
    println!();

    let transcriber = Transcriber::new(&model_path)?;

    let mut results = Vec::new();
    for input in &cli.inputs {
        let audio = read_f32le_file(input)?;
        let transcript = transcriber.transcribe(&audio)?;

        let mut warmup_ms = Vec::with_capacity(cli.warmup);
        for _ in 0..cli.warmup {
            let t0 = Instant::now();
            let _ = transcriber.transcribe(&audio)?;
            warmup_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        }

        let mut run_ms = Vec::with_capacity(cli.iterations);
        for _ in 0..cli.iterations {
            let t0 = Instant::now();
            let _ = transcriber.transcribe(&audio)?;
            run_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        }

        results.push(SampleResult {
            path: input.clone(),
            samples: audio.len(),
            transcript,
            warmup_ms,
            run_ms,
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
    let stats = Stats::from_samples(&result.run_ms);
    let warmup_stats = Stats::from_samples(&result.warmup_ms);
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

    if !result.warmup_ms.is_empty() {
        println!(
            "  warmup: min {:.1} ms | p50 {:.1} ms | max {:.1} ms",
            warmup_stats.min, warmup_stats.p50, warmup_stats.max
        );
    }

    println!(
        "  measured: min {:.1} ms | p50 {:.1} ms | p95 {:.1} ms | max {:.1} ms | mean {:.1} ms",
        stats.min, stats.p50, stats.p95, stats.max, stats.mean
    );
    println!();
}

struct Stats {
    min: f64,
    p50: f64,
    p95: f64,
    max: f64,
    mean: f64,
}

impl Stats {
    fn from_samples(values: &[f64]) -> Self {
        let mut sorted = values.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let min = *sorted.first().unwrap_or(&0.0);
        let max = *sorted.last().unwrap_or(&0.0);
        let mean = if sorted.is_empty() {
            0.0
        } else {
            sorted.iter().sum::<f64>() / sorted.len() as f64
        };

        Self {
            min,
            p50: percentile(&sorted, 0.50),
            p95: percentile(&sorted, 0.95),
            max,
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

struct Transcriber {
    ctx: WhisperContext,
    n_threads: i32,
}

impl Transcriber {
    fn new(model_path: &Path) -> Result<Self, String> {
        let mut params = WhisperContextParameters::default();
        params.flash_attn(true);

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or("Invalid model path")?,
            params,
        )
        .map_err(|e| format!("Failed to load whisper model: {}", e))?;

        let n_threads = std::thread::available_parallelism()
            .map(|n| n.get() as i32)
            .unwrap_or(4);

        Ok(Self { ctx, n_threads })
    }

    fn transcribe(&self, samples: &[f32]) -> Result<String, String> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.n_threads);
        params.set_language(Some("en"));
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_no_context(true);
        params.set_single_segment(true);

        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("Failed to create whisper state: {}", e))?;

        state
            .full(params, samples)
            .map_err(|e| format!("Transcription failed: {}", e))?;

        let segments = state
            .full_n_segments()
            .map_err(|e| format!("Failed to get segments: {}", e))?;

        let mut text = String::with_capacity(256);
        for i in 0..segments {
            if let Ok(segment) = state.full_get_segment_text(i) {
                text.push_str(&segment);
            }
        }

        Ok(text.trim().to_string())
    }
}

fn find_model(model_name: &str) -> Option<PathBuf> {
    let candidates = [
        format!("ggml-{}.en.bin", model_name),
        format!("ggml-{}.bin", model_name),
        format!("ggml-{}-v3.bin", model_name),
    ];

    for filename in candidates {
        let path = PathBuf::from("models").join(filename);
        if path.exists() {
            return Some(path);
        }
    }

    None
}
