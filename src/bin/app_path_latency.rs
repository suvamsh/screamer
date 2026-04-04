#[path = "../bench_support.rs"]
mod bench_support;

use bench_support::{read_f32le_file, sample_label, Stats};
#[path = "../paster.rs"]
mod paster;
use screamer_core::audio;
use screamer_whisper::Transcriber;
use std::env;
use std::path::PathBuf;
use std::time::Instant;

struct Cli {
    model: String,
    iterations: usize,
    warmup: usize,
    device_rate: u32,
    dispatch_paste: bool,
    inputs: Vec<PathBuf>,
}

struct SampleResult {
    path: PathBuf,
    source_samples: usize,
    output_samples: usize,
    transcript: String,
    total_ms: Vec<f64>,
    stop_ms: Vec<f64>,
    transcribe_ms: Vec<f64>,
    state_ms: Vec<f64>,
    infer_ms: Vec<f64>,
    extract_ms: Vec<f64>,
    paste_ms: Vec<f64>,
}

fn main() -> Result<(), String> {
    let cli = parse_args(env::args().skip(1))?;
    let model_path = Transcriber::find_model(&cli.model)
        .ok_or_else(|| format!("Could not find model '{}'", cli.model))?;
    let transcriber = Transcriber::new(&model_path)?;

    println!("Screamer app-path latency bench");
    println!("  model: {}", model_path.display());
    println!("  warmup runs: {}", cli.warmup);
    println!("  measured runs: {}", cli.iterations);
    println!("  device sample rate: {} Hz", cli.device_rate);
    println!("  dispatch paste: {}", yes_no(cli.dispatch_paste));
    println!("  runtime: {}", transcriber.runtime_summary());
    println!();

    let mut results = Vec::new();
    for input in &cli.inputs {
        let device_audio = read_f32le_file(input)?;
        let warmup_audio = audio::resample_to_target(&device_audio, cli.device_rate);

        for _ in 0..cli.warmup {
            let output = transcriber.transcribe_profiled(&warmup_audio)?;
            if cli.dispatch_paste && !output.text.is_empty() {
                paster::paste(&output.text)?;
            }
        }

        let mut total_ms = Vec::with_capacity(cli.iterations);
        let mut stop_ms = Vec::with_capacity(cli.iterations);
        let mut transcribe_ms = Vec::with_capacity(cli.iterations);
        let mut state_ms = Vec::with_capacity(cli.iterations);
        let mut infer_ms = Vec::with_capacity(cli.iterations);
        let mut extract_ms = Vec::with_capacity(cli.iterations);
        let mut paste_ms = Vec::with_capacity(cli.iterations);
        let mut transcript = None;
        let mut output_samples = 0usize;

        for _ in 0..cli.iterations {
            let total_t0 = Instant::now();

            let stop_t0 = Instant::now();
            let samples = audio::resample_to_target(&device_audio, cli.device_rate);
            let stop = stop_t0.elapsed().as_secs_f64() * 1000.0;
            output_samples = samples.len();

            let output = transcriber.transcribe_profiled(&samples)?;
            let transcribe = output.profile.total.as_secs_f64() * 1000.0;
            let state = output.profile.state_acquire.as_secs_f64() * 1000.0;
            let infer = output.profile.inference.as_secs_f64() * 1000.0;
            let extract = output.profile.extract.as_secs_f64() * 1000.0;

            let paste_t0 = Instant::now();
            if cli.dispatch_paste && !output.text.is_empty() {
                paster::paste(&output.text)?;
            }
            let paste = paste_t0.elapsed().as_secs_f64() * 1000.0;

            transcript = Some(output.text);
            total_ms.push(total_t0.elapsed().as_secs_f64() * 1000.0);
            stop_ms.push(stop);
            transcribe_ms.push(transcribe);
            state_ms.push(state);
            infer_ms.push(infer);
            extract_ms.push(extract);
            paste_ms.push(paste);
        }

        results.push(SampleResult {
            path: input.clone(),
            source_samples: device_audio.len(),
            output_samples,
            transcript: transcript.unwrap_or_default(),
            total_ms,
            stop_ms,
            transcribe_ms,
            state_ms,
            infer_ms,
            extract_ms,
            paste_ms,
        });
    }

    for result in &results {
        print_result(result, cli.device_rate);
    }

    print_overall_summary(&results);

    Ok(())
}

fn parse_args<I>(mut args: I) -> Result<Cli, String>
where
    I: Iterator<Item = String>,
{
    let mut model = String::from("base");
    let mut iterations = 10usize;
    let mut warmup = 2usize;
    let mut device_rate = 48_000u32;
    let mut dispatch_paste = false;
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
            "--device-rate" => {
                let value = args.next().ok_or("--device-rate requires a value")?;
                device_rate = value
                    .parse::<u32>()
                    .map_err(|_| format!("Invalid --device-rate value: {}", value))?;
                if device_rate == 0 {
                    return Err("--device-rate must be greater than 0".to_string());
                }
            }
            "--dispatch-paste" => dispatch_paste = true,
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
        device_rate,
        dispatch_paste,
        inputs,
    })
}

fn print_usage() {
    eprintln!("Usage: cargo run --release --bin app_path_latency -- [options] <audio.f32>...");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --model <name>          Whisper model to use (default: base)");
    eprintln!("  --iterations <count>    Measured runs per sample (default: 10)");
    eprintln!("  --warmup <count>        Warmup runs per sample (default: 2)");
    eprintln!("  --device-rate <hz>      Input sample rate before stop/resample (default: 48000)");
    eprintln!("  --dispatch-paste        Run the real clipboard+Cmd+V paste dispatch");
    eprintln!();
    eprintln!("Input format:");
    eprintln!("  Files must be raw f32 little-endian mono audio at the declared device rate.");
}

fn print_result(result: &SampleResult, device_rate: u32) {
    let source_duration_s = result.source_samples as f64 / device_rate as f64;
    let output_duration_s = result.output_samples as f64 / f64::from(audio::TARGET_SAMPLE_RATE);
    let total = Stats::from_samples(&result.total_ms);
    let stop = Stats::from_samples(&result.stop_ms);
    let transcribe = Stats::from_samples(&result.transcribe_ms);
    let state = Stats::from_samples(&result.state_ms);
    let infer = Stats::from_samples(&result.infer_ms);
    let extract = Stats::from_samples(&result.extract_ms);
    let paste = Stats::from_samples(&result.paste_ms);
    let label = sample_label(&result.path);

    println!("{}", label);
    println!(
        "  source duration: {:.2}s ({} samples @ {}Hz)",
        source_duration_s, result.source_samples, device_rate
    );
    println!(
        "  post-stop duration: {:.2}s ({} samples @ 16kHz)",
        output_duration_s, result.output_samples
    );
    println!("  transcript: {}", result.transcript);
    println!(
        "  app-path total: min {:.3} ms | p50 {:.3} ms | p95 {:.3} ms | mean {:.3} ms",
        total.min, total.p50, total.p95, total.mean
    );
    println!(
        "  stage mean: stop {:.3} ms | transcribe {:.3} ms | paste {:.3} ms",
        stop.mean, transcribe.mean, paste.mean
    );
    println!(
        "  transcribe detail mean: state {:.3} ms | infer {:.3} ms | extract {:.3} ms",
        state.mean, infer.mean, extract.mean
    );
    println!();
}

fn print_overall_summary(results: &[SampleResult]) {
    if results.is_empty() {
        return;
    }

    let total_ms: Vec<f64> = results
        .iter()
        .flat_map(|result| result.total_ms.iter().copied())
        .collect();
    let stop_ms: Vec<f64> = results
        .iter()
        .flat_map(|result| result.stop_ms.iter().copied())
        .collect();
    let transcribe_ms: Vec<f64> = results
        .iter()
        .flat_map(|result| result.transcribe_ms.iter().copied())
        .collect();
    let paste_ms: Vec<f64> = results
        .iter()
        .flat_map(|result| result.paste_ms.iter().copied())
        .collect();

    let total = Stats::from_samples(&total_ms);
    let stop = Stats::from_samples(&stop_ms);
    let transcribe = Stats::from_samples(&transcribe_ms);
    let paste = Stats::from_samples(&paste_ms);

    println!("overall");
    println!(
        "  average app-path latency: {:.3} ms across {} phrases x {} measured runs",
        total.mean,
        results.len(),
        total_ms.len()
    );
    println!(
        "  aggregate total: min {:.3} ms | p50 {:.3} ms | p95 {:.3} ms | mean {:.3} ms",
        total.min, total.p50, total.p95, total.mean
    );
    println!(
        "  aggregate stage mean: stop {:.3} ms | transcribe {:.3} ms | paste {:.3} ms",
        stop.mean, transcribe.mean, paste.mean
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
