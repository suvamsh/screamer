use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::{LlamaModelParams, LlamaSplitMode};
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::mtmd::{MtmdBitmap, MtmdContext, MtmdContextParams, MtmdInputText};
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::TokenToStringError;
use screamer_models::find_vision_model;
use std::io::{self, Read};
use std::num::NonZeroU32;

const DEFAULT_CONTEXT_TOKENS: u32 = 8_192;
const DEFAULT_BATCH_TOKENS: usize = 512;
const DEFAULT_MAX_TOKENS: usize = 512;
const MEDIA_MARKER: &str = "<__media__>";

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse(std::env::args().skip(1))?;

    let mut prompt = String::new();
    io::stdin()
        .read_to_string(&mut prompt)
        .map_err(|err| format!("Failed to read prompt from stdin: {err}"))?;
    if prompt.trim().is_empty() {
        return Err("Vision helper received an empty prompt.".to_string());
    }

    let content = generate_vision_response(prompt.trim(), &args.image_path, args.max_tokens)?;
    print!("{content}");
    Ok(())
}

struct Args {
    image_path: String,
    max_tokens: usize,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut image_path = None;
        let mut max_tokens = DEFAULT_MAX_TOKENS;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--image" => {
                    image_path = Some(
                        args.next()
                            .ok_or_else(|| "Missing value for --image".to_string())?,
                    );
                }
                "--max-tokens" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "Missing value for --max-tokens".to_string())?;
                    max_tokens = value
                        .parse()
                        .map_err(|err| format!("Invalid --max-tokens value `{value}`: {err}"))?;
                }
                other => return Err(format!("Unknown argument: {other}")),
            }
        }

        Ok(Self {
            image_path: image_path.ok_or("Missing required --image argument")?,
            max_tokens,
        })
    }
}

fn recommended_thread_count() -> i32 {
    let available = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(4);

    available.saturating_sub(1).clamp(2, 8) as i32
}

fn generate_vision_response(
    prompt: &str,
    image_path: &str,
    max_tokens: usize,
) -> Result<String, String> {
    let (model_path, mmproj_path) = find_vision_model()
        .ok_or("Vision model not found. Expected models/summary/gemma-3-4b-it-q4.gguf and mmproj-gemma-3-4b-it-f16.gguf.")?;

    let mut backend =
        LlamaBackend::init().map_err(|err| format!("Failed to init llama backend: {err}"))?;
    backend.void_logs();

    let gpu_layers = if backend.supports_gpu_offload() {
        u32::MAX
    } else {
        0
    };
    let model_params = LlamaModelParams::default()
        .with_n_gpu_layers(gpu_layers)
        .with_split_mode(LlamaSplitMode::None)
        .with_main_gpu(0)
        .with_use_mmap(backend.supports_mmap())
        .with_use_mlock(false);

    let model =
        LlamaModel::load_from_file(&backend, &model_path, &model_params).map_err(|err| {
            format!(
                "Failed to load vision model at {}: {err}",
                model_path.display()
            )
        })?;

    // Initialize multimodal context with the mmproj file
    let threads = recommended_thread_count();
    let mtmd_params = MtmdContextParams {
        use_gpu: backend.supports_gpu_offload(),
        print_timings: false,
        n_threads: threads,
        ..MtmdContextParams::default()
    };

    let mmproj_str = mmproj_path
        .to_str()
        .ok_or("mmproj path contains invalid UTF-8")?;
    let mtmd_ctx = MtmdContext::init_from_file(mmproj_str, &model, &mtmd_params)
        .map_err(|err| format!("Failed to init multimodal context: {err}"))?;

    if !mtmd_ctx.support_vision() {
        return Err("Model does not support vision input.".to_string());
    }

    // Load the screenshot image
    let bitmap = MtmdBitmap::from_file(&mtmd_ctx, image_path)
        .map_err(|err| format!("Failed to load image {image_path}: {err}"))?;

    // Build the prompt with media marker
    // Format: image first, then the user's question
    let full_prompt = format!("{MEDIA_MARKER}\n{prompt}");

    let input_text = MtmdInputText {
        text: full_prompt,
        add_special: true,
        parse_special: true,
    };

    // Tokenize with image
    let chunks = mtmd_ctx
        .tokenize(input_text, &[&bitmap])
        .map_err(|err| format!("Failed to tokenize multimodal input: {err}"))?;

    // Create inference context
    let context_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(DEFAULT_CONTEXT_TOKENS))
        .with_n_batch(DEFAULT_BATCH_TOKENS as u32)
        .with_n_ubatch(DEFAULT_BATCH_TOKENS as u32)
        .with_n_threads(threads)
        .with_n_threads_batch(threads)
        .with_offload_kqv(true);

    let mut context = model
        .new_context(&backend, context_params)
        .map_err(|err| format!("Failed to create inference context: {err}"))?;

    // Evaluate all chunks (text + image embeddings)
    let n_past = chunks
        .eval_chunks(
            &mtmd_ctx,
            &context,
            0,
            0,
            DEFAULT_BATCH_TOKENS as i32,
            true,
        )
        .map_err(|err| format!("Failed to evaluate multimodal chunks: {err}"))?;

    // Generate response tokens
    let mut generated = String::new();
    let mut position = n_past;
    let mut token_batch = llama_cpp_2::llama_batch::LlamaBatch::new(1, 1);

    for _ in 0..max_tokens {
        let next_token = context.token_data_array().sample_token_greedy();
        if model.is_eog_token(next_token) || next_token == model.token_eos() {
            break;
        }

        generated.push_str(&decode_token_piece(&model, next_token)?);

        token_batch.clear();
        token_batch
            .add(next_token, position, &[0], true)
            .map_err(|err| format!("Failed to append generated token to batch: {err}"))?;
        context
            .decode(&mut token_batch)
            .map_err(|err| format!("Failed to decode generated token: {err}"))?;
        position += 1;
    }

    Ok(generated.trim().to_string())
}

fn decode_token_piece(model: &LlamaModel, token: LlamaToken) -> Result<String, String> {
    let mut buffer_size = 8usize;
    loop {
        match model.token_to_piece_bytes(token, buffer_size, false, None) {
            Ok(bytes) => return Ok(String::from_utf8_lossy(&bytes).to_string()),
            Err(TokenToStringError::InsufficientBufferSpace(needed)) => {
                buffer_size = usize::try_from((-needed).max(8)).unwrap_or(32);
            }
            Err(err) => return Err(format!("Failed to decode generated token: {err}")),
        }
    }
}
