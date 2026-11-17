use anyhow::{Result, ensure};
use cbgpu_ai_corpus::{Corpus, Embedder, ingest::SourceFiles};
use cbgpu_ai_models::{E5Model, Progress, assignment, atomic_json};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
struct Model {
    model: E5Model,
    progress: Progress,
}
impl Embedder for Model {
    fn embed(&self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>> {
        self.model
            .vectors(texts, query, &self.progress)
            .map_err(|_| anyhow::anyhow!("Native embedding unavailable"))
    }
}
#[tokio::main(worker_threads = 2)]
async fn main() {
    if run().await.is_err() {
        eprintln!(
            "Native authorized search failed; check private runtime/policy configuration or source availability."
        );
        std::process::exit(1)
    }
}
async fn run() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["--help"] {
        println!(
            "cbgpu-ai-search search JOB_FOLDER STATE_ROOT; trusted active scheduler job required. Explicit private runtime/policy DSNs and pinned offline model/OCR are required; never applies schemas or imports data."
        );
        return Ok(());
    }
    ensure!(
        args.len() == 3 && args[0] == "search",
        "Use search JOB_FOLDER STATE_ROOT"
    );
    let folder = PathBuf::from(&args[1]);
    let state = PathBuf::from(&args[2]);
    let job = cbgpu_ai_search::owned_job(&folder, &state)?;
    let runtime = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&std::env::var("CBGPU_AI_DATABASE_URL")?)
        .await?;
    let policy = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only=on")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&std::env::var("CBGPU_AI_CORPUS_POLICY_DATABASE_URL")?)
        .await?;
    let corpus = Corpus::new(runtime, policy)
        .with_account_v2(cbgpu_ai_search::authorization_from_environment()?);
    // Validate active owner before loading model weights or opening source files.
    let snapshot = corpus.trusted_job(&job).await?;
    corpus.candidate_sources(&snapshot).await?;
    let files = SourceFiles::sandboxed(
        PathBuf::from(std::env::var("CBGPU_AI_MATERIALS_ROOT")?),
        &std::env::var("CBGPU_AI_PAPER_ORIGIN")?,
        &std::env::var("CBGPU_AI_CORPUS_OCR_IMAGE")?,
    )?;
    let progress_folder = folder.clone();
    let progress: Progress = Arc::new(move |message| {
        let _ = atomic_json(
            &progress_folder.join("progress.json"),
            &json!({"message":message}),
        );
    });
    let visible = std::env::var("CUDA_VISIBLE_DEVICES").ok();
    let mode = assignment(
        std::env::var("CBGPU_AI_CPU_ONLY").as_deref() == Ok("1"),
        visible.as_deref(),
    )
    .map_err(|_| anyhow::anyhow!("Scheduler GPU assignment required"))?;
    let model = Model {
        model: E5Model::load_on(&state.join("models/embed"), mode)
            .map_err(|_| anyhow::anyhow!("Pinned E5 unavailable"))?,
        progress,
    };
    let result = cbgpu_ai_search::search(&snapshot, &corpus, &files, &model).await?;
    corpus.recheck(&snapshot).await?;
    atomic_json(&folder.join("result.json"), &result)
        .map_err(|_| anyhow::anyhow!("Cannot persist result"))?;
    Ok(())
}
