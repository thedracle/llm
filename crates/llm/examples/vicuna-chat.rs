use llm_base::{
    feed_prompt_callback, InferenceFeedback, InferenceRequest, InferenceResponse, InferenceStats,
    LoadProgress,
};
use rustyline::error::ReadlineError;
use spinoff::{spinners::Dots2, Spinner};
use std::{convert::Infallible, env::args, io::Write, path::Path, time::Instant};

fn main() {
    let raw_args: Vec<String> = args().collect();
    let args = match &raw_args.len() {
        3 => (raw_args[1].as_str(), raw_args[2].as_str()),
        _ => {
            panic!("Usage: cargo run --release --example vicuna-chat <model type> <path to model>")
        }
    };

    let model_type = args.0;
    let model_path = Path::new(args.1);

    let architecture = model_type.parse().unwrap_or_else(|e| panic!("{e}"));

    let sp = Some(Spinner::new(Dots2, "Loading model...", None));

    let now = Instant::now();
    let prev_load_time = now;

    let model = llm::load_dynamic(
        architecture,
        model_path,
        Default::default(),
        load_progress_callback(sp, now, prev_load_time),
    )
    .unwrap_or_else(|err| panic!("Failed to load {model_type} model from {model_path:?}: {err}"));

    let mut session = model.start_session(Default::default());

    let character_name = "### Assistant";
    let user_name = "### Human";
    let persona = "A chat between a human and an assistant.";
    let history = format!(
        "{character_name}: Hello - How may I help you today?\n\
         {user_name}: What is the capital or France?\n\
         {character_name}:  Paris is the capital of France."
    );

    session
        .feed_prompt(
            model.as_ref(),
            &Default::default(),
            format!("{persona}\n{history}").as_str(),
            &mut Default::default(),
            feed_prompt_callback(prompt_callback),
        )
        .expect("Failed to ingest initial prompt.");

    let mut rl = rustyline::DefaultEditor::new().expect("Failed to create input reader");

    let mut rng = rand::thread_rng();
    let mut res = InferenceStats::default();
    let mut buf = String::new();

    loop {
        println!();
        let readline = rl.readline(format!("{user_name}: ").as_str());
        print!("{character_name}:");
        match readline {
            Ok(line) => {
                let stats = session
                    .infer(
                        model.as_ref(),
                        &mut rng,
                        &InferenceRequest {
                            prompt: format!("{user_name}: {line}\n{character_name}:").as_str(),
                            ..Default::default()
                        },
                        &mut Default::default(),
                        inference_callback(String::from(user_name), &mut buf),
                    )
                    .unwrap_or_else(|e| panic!("{e}"));

                res.feed_prompt_duration = res
                    .feed_prompt_duration
                    .saturating_add(stats.feed_prompt_duration);
                res.prompt_tokens += stats.prompt_tokens;
                res.predict_duration = res.predict_duration.saturating_add(stats.predict_duration);
                res.predict_tokens += stats.predict_tokens;
            }
            Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => {
                break;
            }
            Err(err) => {
                println!("{err}");
            }
        }
    }

    println!("\n\nInference stats:\n{res}");
}

fn load_progress_callback(
    mut sp: Option<Spinner>,
    now: Instant,
    mut prev_load_time: Instant,
) -> impl FnMut(LoadProgress) {
    move |progress| match progress {
        LoadProgress::HyperparametersLoaded => {
            if let Some(sp) = sp.as_mut() {
                sp.update_text("Loaded hyperparameters")
            };
        }
        LoadProgress::ContextSize { bytes } => log::debug!(
            "ggml ctx size = {}",
            bytesize::to_string(bytes as u64, false)
        ),
        LoadProgress::TensorLoaded {
            current_tensor,
            tensor_count,
            ..
        } => {
            if prev_load_time.elapsed().as_millis() > 500 {
                // We don't want to re-render this on every message, as that causes the
                // spinner to constantly reset and not look like it's spinning (and
                // it's obviously wasteful).
                if let Some(sp) = sp.as_mut() {
                    sp.update_text(format!(
                        "Loaded tensor {}/{}",
                        current_tensor + 1,
                        tensor_count
                    ));
                };
                prev_load_time = std::time::Instant::now();
            }
        }
        LoadProgress::Loaded {
            file_size,
            tensor_count,
        } => {
            if let Some(sp) = sp.take() {
                sp.success(&format!(
                    "Loaded {tensor_count} tensors ({}) after {}ms",
                    bytesize::to_string(file_size, false),
                    now.elapsed().as_millis()
                ));
            };
        }
    }
}

fn prompt_callback(resp: InferenceResponse) -> Result<InferenceFeedback, Infallible> {
    match resp {
        InferenceResponse::PromptToken(t) | InferenceResponse::InferredToken(t) => print_token(t),
        _ => Ok(InferenceFeedback::Continue),
    }
}

#[allow(clippy::needless_lifetimes)]
fn inference_callback<'a>(
    stop_sequence: String,
    buf: &'a mut String,
) -> impl FnMut(InferenceResponse) -> Result<InferenceFeedback, Infallible> + 'a {
    move |resp| match resp {
        InferenceResponse::InferredToken(t) => {
            let mut reverse_buf = buf.clone();
            reverse_buf.push_str(t.as_str());
            if stop_sequence.as_str().eq(reverse_buf.as_str()) {
                buf.clear();
                return Ok(InferenceFeedback::Halt);
            } else if stop_sequence.as_str().starts_with(reverse_buf.as_str()) {
                buf.push_str(t.as_str());
                return Ok(InferenceFeedback::Continue);
            }

            if buf.is_empty() {
                print_token(t)
            } else {
                print_token(reverse_buf)
            }
        }
        InferenceResponse::EotToken => Ok(InferenceFeedback::Halt),
        _ => Ok(InferenceFeedback::Continue),
    }
}

fn print_token(t: String) -> Result<InferenceFeedback, Infallible> {
    print!("{t}");
    std::io::stdout().flush().unwrap();

    Ok(InferenceFeedback::Continue)
}
