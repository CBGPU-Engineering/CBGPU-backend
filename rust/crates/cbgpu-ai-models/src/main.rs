use cbgpu_ai_models::*;
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::Arc};
#[tokio::main(worker_threads = 2)]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1)
    }
}
async fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        println!(
            "cbgpu-ai-models embed|search|transcribe JOB_FOLDER STATE_ROOT\nCUDA builds use only one scheduler-assigned CUDA_VISIBLE_DEVICES=GPU-UUID; CBGPU_AI_FORCE_CPU=1 forces CPU."
        );
        return Ok(());
    }
    if args.len() != 3 || !["embed", "search", "transcribe"].contains(&args[0].as_str()) {
        return Err("Use KIND JOB_FOLDER STATE_ROOT".into());
    }
    let folder = PathBuf::from(&args[1]);
    let state = PathBuf::from(&args[2]);
    let input: Value = serde_json::from_slice(
        &fs::read(folder.join("input.json")).map_err(|_| "Job input unavailable")?,
    )
    .map_err(|_| "Invalid job input")?;
    let progress_folder = folder.clone();
    let progress: Progress = Arc::new(move |message| {
        let _ = atomic_json(
            &progress_folder.join("progress.json"),
            &json!({"message":message}),
        );
    });
    progress("Loading the local native model".into());
    let visible = std::env::var("CUDA_VISIBLE_DEVICES").ok();
    let mode = assignment(
        std::env::var("CBGPU_AI_FORCE_CPU").as_deref() == Ok("1")
            || std::env::var("CBGPU_AI_CPU_ONLY").as_deref() == Ok("1"),
        visible.as_deref(),
    )?;
    let store = if let Ok(database) = std::env::var("CBGPU_AI_DATABASE_URL") {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database)
            .await
            .map_err(|_| "Native model cache database unavailable")?;
        Some(
            cbgpu_ai_store::Store::new(
                pool,
                state.clone(),
                std::env::var_os("CBGPU_AI_DATA_ROOT").map(PathBuf::from),
                Default::default(),
            )
            .map_err(|_| "Native cache state directory invalid")?,
        )
    } else {
        None
    };
    let result = match args[0].as_str() {
        "embed" => {
            let texts: Vec<String> = serde_json::from_value(input["input"].clone())
                .map_err(|_| "Embedding input must be text array")?;
            let model = E5Model::load_on(&state.join("models/embed"), mode)?;
            let vectors = model
                .vectors_cached(store.as_ref(), &texts, false, &progress)
                .await?;
            let count = model.token_count(&texts, false)?;
            json!({"object":"list","model":"cbgpu-embeddings","data":vectors.into_iter().enumerate().map(|(i,v)|json!({"object":"embedding","index":i,"embedding":v})).collect::<Vec<_>>(),"usage":{"prompt_tokens":count,"total_tokens":count}})
        }
        "search" => {
            let candidates: Vec<Candidate> = serde_json::from_value(input["candidates"].clone())
                .map_err(|_| "Search candidates invalid")?;
            if candidates.is_empty() {
                json!({"hits":[]})
            } else {
                let question = input["query"].as_str().ok_or("Search query missing")?;
                let model = E5Model::load_on(&state.join("models/embed"), mode)?;
                let q = model
                    .vectors_cached(store.as_ref(), &[question.to_owned()], true, &progress)
                    .await?;
                let texts = candidates
                    .iter()
                    .map(|c| c.text.clone())
                    .collect::<Vec<_>>();
                let vectors = model
                    .vectors_cached(store.as_ref(), &texts, false, &progress)
                    .await?;
                rank(question, &candidates, &q[0], &vectors)?
            }
        }
        _ => {
            let upload = input["upload_id"]
                .as_str()
                .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                .ok_or("Invalid authorized audio upload")?;
            let source = state.join("uploads").join(upload);
            let resolved = source
                .canonicalize()
                .map_err(|_| "Audio upload unavailable")?;
            let primary = state
                .join("uploads")
                .canonicalize()
                .map_err(|_| "Upload directory unavailable")?;
            let secondary = std::env::var_os("CBGPU_AI_DATA_ROOT")
                .map(PathBuf::from)
                .map(|p| p.join("uploads"))
                .and_then(|p| p.canonicalize().ok());
            if !resolved.starts_with(&primary)
                && secondary.as_ref().is_none_or(|p| !resolved.starts_with(p))
            {
                return Err("Audio upload escapes authorized storage".into());
            }
            let model = state.join("models/transcribe-native/ggml-large-v3.bin");
            progress("Verifying pinned local Whisper weights".into());
            verify_whisper_model(&model)?;
            progress("Decoding the authorized audio upload".into());
            let samples = decode_audio(&ffmpeg_path()?, &resolved)?;
            transcribe_samples_on(
                &model,
                &samples,
                input["language"].as_str(),
                &folder.join("transcript.parts.json"),
                progress.clone(),
                mode,
            )?
        }
    };
    atomic_json(&folder.join("result.json"), &result)?;
    progress("Completed".into());
    if let Some(store) = store {
        store.pool.close().await;
    }
    Ok(())
}
