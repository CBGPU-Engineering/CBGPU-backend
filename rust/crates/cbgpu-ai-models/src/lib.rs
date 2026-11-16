//! Local native inference. CPU-only by default; no model/network/user-data discovery.
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};
use tokenizers::{Tokenizer, TruncationParams};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
pub type Result<T> = std::result::Result<T, String>;
pub type Progress = Arc<dyn Fn(String) + Send + Sync>;
pub const DIMENSIONS: usize = 384;
pub const WHISPER_SHA256: &str = "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2";
pub const WHISPER_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";
pub fn prefixed(text: &str, query: bool) -> String {
    if text.starts_with("query: ") || text.starts_with("passage: ") {
        text.into()
    } else {
        format!("{}: {text}", if query { "query" } else { "passage" })
    }
}
pub fn cache_key(text: &str, query: bool) -> String {
    hex::encode(Sha256::digest(format!(
        "e5-small-v1:{}",
        prefixed(text, query)
    )))
}
pub fn normalize(vector: &mut [f32]) -> Result<()> {
    if vector.is_empty() || vector.iter().any(|v| !v.is_finite()) {
        return Err("Invalid embedding values".into());
    }
    let norm = vector
        .iter()
        .map(|v| (*v as f64).powi(2))
        .sum::<f64>()
        .sqrt();
    if norm <= 1e-12 {
        return Err("Embedding has no magnitude".into());
    }
    for v in vector {
        *v = (*v as f64 / norm) as f32;
    }
    Ok(())
}
pub fn masked_mean(tokens: &[Vec<f32>], mask: &[u32]) -> Result<Vec<f32>> {
    if tokens.len() != mask.len()
        || tokens.is_empty()
        || tokens.iter().any(|t| t.len() != DIMENSIONS)
    {
        return Err("Embedding mask/dimensions mismatch".into());
    }
    let count = mask.iter().filter(|m| **m != 0).count();
    if count == 0 {
        return Err("Embedding contains only padding".into());
    }
    let mut output = vec![0f32; DIMENSIONS];
    for (token, active) in tokens.iter().zip(mask) {
        if *active != 0 {
            for (o, v) in output.iter_mut().zip(token) {
                *o += *v / count as f32;
            }
        }
    }
    normalize(&mut output)?;
    Ok(output)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Execution {
    Cpu,
    AssignedGpu,
}
pub fn assignment(force_cpu: bool, visible: Option<&str>) -> Result<Execution> {
    if force_cpu {
        return Ok(Execution::Cpu);
    }
    match visible {
        None | Some("") | Some("-1") => Ok(Execution::Cpu),
        Some(value)
            if value
                .strip_prefix("GPU-")
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()) =>
        {
            Ok(Execution::AssignedGpu)
        }
        _ => Err("Native model requires one scheduler-assigned GPU UUID".into()),
    }
}
fn device(mode: Execution) -> Result<Device> {
    match mode {
        Execution::Cpu => Ok(Device::Cpu),
        Execution::AssignedGpu => {
            #[cfg(feature = "cuda")]
            {
                Device::new_cuda(0).map_err(|_| "Assigned CUDA device unavailable".into())
            }
            #[cfg(not(feature = "cuda"))]
            {
                Err("Assigned GPU requested but this binary lacks CUDA support".into())
            }
        }
    }
}
pub struct E5Model {
    model: BertModel,
    tokenizer: Tokenizer,
    pad: u32,
}
impl E5Model {
    pub fn load(directory: &Path) -> Result<Self> {
        Self::load_on(directory, Execution::Cpu)
    }
    pub fn load_on(directory: &Path, mode: Execution) -> Result<Self> {
        let selected = device(mode)?;
        let config: Config = serde_json::from_slice(
            &fs::read(directory.join("config.json")).map_err(|_| "E5 configuration unavailable")?,
        )
        .map_err(|_| "Invalid E5 configuration")?;
        if config.hidden_size != DIMENSIONS
            || config.max_position_embeddings != 512
            || config.vocab_size != 250037
        {
            return Err("Unexpected E5 model architecture".into());
        }
        let mut tokenizer = Tokenizer::from_file(directory.join("tokenizer.json"))
            .map_err(|_| "Local E5 tokenizer unavailable")?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: 512,
                ..Default::default()
            }))
            .map_err(|_| "Cannot configure E5 truncation")?;
        tokenizer.with_padding(None);
        let pad = tokenizer
            .token_to_id("<pad>")
            .ok_or("E5 padding token missing")?;
        // Installed safetensors assets are immutable: model upgrades must replace the
        // file atomically, never modify the mapped inode while an inference is active.
        let weights = unsafe {
            VarBuilder::from_mmaped_safetensors(
                &[directory.join("model.safetensors")],
                DType::F32,
                &selected,
            )
        }
        .map_err(|_| "Local E5 weights unavailable")?;
        let model = BertModel::load(weights, &config)
            .map_err(|_| "E5 weights do not match configuration")?;
        Ok(Self {
            model,
            tokenizer,
            pad,
        })
    }
    pub fn token_count(&self, texts: &[String], query: bool) -> Result<usize> {
        texts
            .iter()
            .map(|t| {
                self.tokenizer
                    .encode(prefixed(t, query), true)
                    .map(|t| t.len())
                    .map_err(|_| "E5 tokenization failed".into())
            })
            .sum()
    }
    pub fn vectors(
        &self,
        texts: &[String],
        query: bool,
        progress: &Progress,
    ) -> Result<Vec<Vec<f32>>> {
        let mut output = Vec::with_capacity(texts.len());
        for (batch_index, batch) in texts.chunks(24).enumerate() {
            let encoded = self
                .tokenizer
                .encode_batch(
                    batch.iter().map(|t| prefixed(t, query)).collect::<Vec<_>>(),
                    true,
                )
                .map_err(|_| "E5 tokenization failed")?;
            let width = encoded
                .iter()
                .map(|e| e.len())
                .max()
                .ok_or("Empty E5 batch")?;
            let mut ids = Vec::with_capacity(batch.len() * width);
            let mut masks = Vec::new();
            for encoding in encoded {
                let mut row = encoding.get_ids().to_vec();
                let mut mask = vec![1u32; row.len()];
                row.resize(width, self.pad);
                mask.resize(width, 0);
                ids.extend(row);
                masks.push(mask);
            }
            let mask = Tensor::new(masks.clone(), &self.model.device)
                .map_err(|_| "Cannot construct E5 attention mask")?;
            let ids = Tensor::from_vec(ids, (batch.len(), width), &self.model.device)
                .map_err(|_| "Cannot construct E5 token batch")?;
            let types = ids
                .zeros_like()
                .map_err(|_| "Cannot construct E5 token types")?;
            let hidden = self
                .model
                .forward(&ids, &types, Some(&mask))
                .map_err(|_| "Native E5 forward pass failed")?
                .to_vec3::<f32>()
                .map_err(|_| "Cannot extract E5 embeddings")?;
            for (tokens, mask) in hidden.iter().zip(&masks) {
                output.push(masked_mean(tokens, mask)?)
            }
            progress(format!(
                "Embedded {} passages",
                ((batch_index + 1) * 24).min(texts.len())
            ));
        }
        Ok(output)
    }
    pub async fn vectors_cached(
        &self,
        store: Option<&cbgpu_ai_store::Store>,
        texts: &[String],
        query: bool,
        progress: &Progress,
    ) -> Result<Vec<Vec<f32>>> {
        let mut vectors: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
        let mut missing = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            if let Some(store) = store
                && let Some(value) = store
                    .cached_vector(&cache_key(text, query))
                    .await
                    .map_err(|_| "Embedding cache unavailable")?
                && let Ok(vector) = serde_json::from_value::<Vec<f32>>(value)
            {
                let norm = vector
                    .iter()
                    .map(|v| (*v as f64).powi(2))
                    .sum::<f64>()
                    .sqrt();
                if vector.len() == DIMENSIONS
                    && vector.iter().all(|v| v.is_finite())
                    && (norm - 1.0).abs() < 0.001
                {
                    vectors[i] = Some(vector);
                    continue;
                }
            }
            missing.push(i);
        }
        let input: Vec<_> = missing.iter().map(|i| texts[*i].clone()).collect();
        for (index, vector) in missing
            .into_iter()
            .zip(self.vectors(&input, query, progress)?)
        {
            if let Some(store) = store {
                store
                    .cache_vector(&cache_key(&texts[index], query), &json!(vector))
                    .await
                    .map_err(|_| "Cannot persist embedding cache")?;
            }
            vectors[index] = Some(vector);
        }
        vectors
            .into_iter()
            .map(|v| v.ok_or_else(|| "Embedding result missing".into()))
            .collect()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub href: String,
    #[serde(default)]
    pub snippet: String,
}
pub fn rank(
    query: &str,
    candidates: &[Candidate],
    question: &[f32],
    vectors: &[Vec<f32>],
) -> Result<Value> {
    if question.len() != DIMENSIONS
        || vectors.len() != candidates.len()
        || question.iter().any(|v| !v.is_finite())
    {
        return Err("Search embedding shape invalid".into());
    }
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut best: BTreeMap<String, Value> = BTreeMap::new();
    for (candidate, vector) in candidates.iter().zip(vectors) {
        if vector.len() != DIMENSIONS || vector.iter().any(|v| !v.is_finite()) {
            return Err("Search candidate embedding invalid".into());
        }
        let semantic = question
            .iter()
            .zip(vector)
            .map(|(q, v)| *q as f64 * *v as f64)
            .sum::<f64>();
        let lower = candidate.text.to_lowercase();
        let lexical = words.iter().filter(|w| lower.contains(w.as_str())).count() as f64
            / words.len().max(1) as f64;
        let score = ((semantic * 0.8 + lexical * 0.2) * 100000.0).round() / 100000.0;
        let hit = json!({"id":candidate.id,"label":candidate.label,"href":candidate.href,"snippet":candidate.snippet,"score":score});
        if best
            .get(&candidate.id)
            .is_none_or(|old| old["score"].as_f64().unwrap_or(-2.0) < score)
        {
            best.insert(candidate.id.clone(), hit);
        }
    }
    let mut hits: Vec<_> = best.into_values().collect();
    hits.sort_by(|a, b| {
        b["score"]
            .as_f64()
            .unwrap()
            .total_cmp(&a["score"].as_f64().unwrap())
    });
    hits.truncate(20);
    Ok(json!({"hits":hits,"model":"cbgpu-embeddings"}))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
}
pub fn stamp(seconds: f64) -> String {
    let millis = (seconds.max(0.0) * 1000.0).round() as u64;
    format!(
        "{:02}:{:02}:{:02},{:03}",
        millis / 3600000,
        millis / 60000 % 60,
        millis / 1000 % 60,
        millis % 1000
    )
}
pub fn transcript(parts: &[Segment], language: &str, duration: f64) -> Value {
    let text = parts
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let srt = parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            format!(
                "{}\n{} --> {}\n{}",
                i + 1,
                stamp(p.start),
                stamp(p.end),
                p.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    json!({"text":text,"segments":parts,"srt":srt,"language":language,"duration":duration,"model":"cbgpu-whisper"})
}
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path.parent().ok_or("Output has no parent")?;
    fs::create_dir_all(parent).map_err(|_| "Cannot create inference output directory")?;
    let temp = parent.join(format!(".inference-{}", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)
        .map_err(|_| "Cannot create inference output")?;
    let result = (|| {
        serde_json::to_writer(&mut file, value).map_err(|_| "Cannot encode inference output")?;
        file.write_all(b"\n")
            .and_then(|_| file.sync_all())
            .map_err(|_| "Cannot sync inference output")?;
        fs::rename(&temp, path).map_err(|_| "Cannot publish inference output")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
pub fn verify_whisper_model(path: &Path) -> Result<()> {
    if fs::metadata(path)
        .map_err(|_| "Native Whisper model missing")?
        .len()
        != 3095033483
    {
        return Err("Whisper large-v3 asset size does not match pinned model".into());
    }
    let mut file = File::open(path).map_err(|_| "Cannot open Whisper model")?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|_| "Cannot verify Whisper model")?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    if hex::encode(hash.finalize()) != WHISPER_SHA256 {
        return Err("Whisper large-v3 asset hash mismatch".into());
    }
    Ok(())
}
pub fn ffmpeg_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("CBGPU_FFMPEG") {
        let path = PathBuf::from(path);
        if path.is_absolute() && path.is_file() {
            return Ok(path);
        }
        return Err("CBGPU_FFMPEG must select an absolute native executable".into());
    }
    for path in [
        PathBuf::from("/usr/bin/ffmpeg"),
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".cache/cbgpu-native-deps/ffmpeg/extracted/usr/bin/ffmpeg"),
    ] {
        if path.is_file() {
            return Ok(path);
        }
    }
    Err("Native FFmpeg decoder is unavailable".into())
}
pub fn decode_audio(ffmpeg: &Path, audio: &Path) -> Result<Vec<f32>> {
    let output = Command::new(ffmpeg)
        .args([
            "-v",
            "error",
            "-nostdin",
            "-threads",
            "2",
            "-hwaccel",
            "none",
            "-protocol_whitelist",
            "fd,pipe",
            "-fd",
            "0",
            "-i",
            "fd:",
        ])
        .args(["-vn", "-ac", "1", "-ar", "16000", "-f", "f32le", "pipe:1"])
        .stdin(Stdio::from(
            File::open(audio).map_err(|_| "Cannot open authorized audio upload")?,
        ))
        .stderr(Stdio::null())
        .output()
        .map_err(|_| "Cannot start native audio decoder")?;
    if !output.status.success() || output.stdout.len() % 4 != 0 {
        return Err("Unsupported or invalid audio input".into());
    }
    let samples: Vec<_> = output
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    if samples.is_empty()
        || samples.len() > i32::MAX as usize
        || samples.iter().any(|v| !v.is_finite())
    {
        return Err("Audio contains no valid samples".into());
    }
    Ok(samples)
}
pub fn transcribe_samples(
    model: &Path,
    samples: &[f32],
    language: Option<&str>,
    checkpoint: &Path,
    progress: Progress,
) -> Result<Value> {
    transcribe_samples_on(
        model,
        samples,
        language,
        checkpoint,
        progress,
        Execution::Cpu,
    )
}
pub fn transcribe_samples_on(
    model: &Path,
    samples: &[f32],
    language: Option<&str>,
    checkpoint: &Path,
    progress: Progress,
    mode: Execution,
) -> Result<Value> {
    if samples.is_empty() || samples.iter().any(|v| !v.is_finite()) {
        return Err("Invalid 16kHz mono PCM audio".into());
    }
    if language
        .is_some_and(|v| v.len() > 12 || !v.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-'))
    {
        return Err("Invalid transcription language".into());
    }
    let duration = samples.len() as f64 / 16000.0;
    let mut previous: Vec<Segment> = if checkpoint.exists() {
        serde_json::from_slice(
            &fs::read(checkpoint).map_err(|_| "Cannot read transcription checkpoint")?,
        )
        .map_err(|_| "Invalid transcription checkpoint")?
    } else {
        vec![]
    };
    if previous.iter().any(|p| {
        !p.start.is_finite()
            || !p.end.is_finite()
            || p.start < 0.0
            || p.end < p.start
            || p.end > duration + 1.0
    }) || previous.windows(2).any(|w| w[1].start < w[0].end)
    {
        return Err("Invalid checkpoint timestamps".into());
    }
    let offset = previous.last().map(|p| p.end).unwrap_or(0.0);
    if offset >= duration {
        return Ok(transcript(
            &previous,
            language.unwrap_or("unknown"),
            duration,
        ));
    }
    if mode == Execution::AssignedGpu && !cfg!(feature = "cuda") {
        return Err("Assigned GPU requested but Whisper binary lacks CUDA support".into());
    }
    progress("Detecting speech with local Silero VAD".into());
    // WhisperState::full bypasses whisper_full's built-in VAD wrapper. Run
    // native Silero explicitly, preserving timestamps in the original audio.
    static LOGGING: std::sync::Once = std::sync::Once::new();
    LOGGING.call_once(whisper_rs::install_logging_hooks);
    let vad = model
        .parent()
        .ok_or("Native Whisper model directory missing")?
        .join("ggml-silero-v5.1.2.bin");
    let bytes = fs::read(&vad).map_err(|_| "Native Silero VAD model missing")?;
    if hex::encode(Sha256::digest(&bytes))
        != "29940d98d42b91fbd05ce489f3ecf7c72f0a42f027e4875919a28fb4c04ea2cf"
    {
        return Err("Native VAD model hash mismatch".into());
    }
    let mut vad_context = whisper_rs::WhisperVadContext::new(
        vad.to_str().ok_or("Invalid VAD path")?,
        whisper_rs::WhisperVadContextParams::default(),
    )
    .map_err(|_| "Cannot initialize native speech detection")?;
    let mut vad_params = whisper_rs::WhisperVadParams::default();
    vad_params.set_max_speech_duration(30.0);
    let regions = vad_context
        .segments_from_samples(vad_params, samples)
        .map_err(|_| "Native speech detection failed")?;
    let ranges: Vec<_> = regions
        .into_iter()
        .filter_map(|r| {
            let start = (r.start as f64 / 100.0).max(offset);
            let end = (r.end as f64 / 100.0).min(duration);
            (end > start).then_some((start, end))
        })
        .collect();
    if ranges.is_empty() {
        atomic_json(checkpoint, &previous)?;
        return Ok(transcript(
            &previous,
            language.unwrap_or("unknown"),
            duration,
        ));
    }
    progress("Loading the native Whisper model".into());
    let mut context = WhisperContextParameters::default();
    context.use_gpu(mode == Execution::AssignedGpu);
    context.gpu_device = 0;
    let context = WhisperContext::new_with_params(model, context)
        .map_err(|_| "Cannot load native Whisper model")?;
    let mut state = context
        .create_state()
        .map_err(|_| "Cannot initialize Whisper state")?;
    let shared = Arc::new(Mutex::new(std::mem::take(&mut previous)));
    let mut detected_language = language.unwrap_or("unknown").to_owned();
    for (start, end) in ranges {
        let mut params = FullParams::new(SamplingStrategy::BeamSearch {
            beam_size: 3,
            patience: -1.0,
        });
        params.set_n_threads(4);
        params.set_language(language);
        params.set_translate(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        let updates = shared.clone();
        let path = checkpoint.to_owned();
        let progress = progress.clone();
        params.set_segment_callback_safe(move |data: whisper_rs::SegmentCallbackData| {
            let mut parts = match updates.lock() {
                Ok(parts) => parts,
                Err(_) => return,
            };
            let previous_end = parts.last().map(|p| p.end).unwrap_or(0.0);
            let segment = Segment {
                start: (start + data.start_timestamp as f64 / 100.0).max(previous_end),
                end: (start + data.end_timestamp as f64 / 100.0).min(end),
                text: data.text.trim().into(),
            };
            if segment.end <= segment.start || segment.text.is_empty() {
                return;
            }
            parts.push(segment.clone());
            let _ = atomic_json(&path, &*parts);
            progress(format!("Transcribed {} seconds", segment.end as u64));
        });
        let from = ((start * 16000.0) as usize).min(samples.len());
        let to = ((end * 16000.0) as usize).min(samples.len());
        state
            .full(params, &samples[from..to])
            .map_err(|_| "Native Whisper transcription failed")?;
        if language.is_none() {
            detected_language = whisper_rs::get_lang_str(state.full_lang_id_from_state())
                .unwrap_or("unknown")
                .into();
        }
    }
    let parts = shared
        .lock()
        .map_err(|_| "Cannot read transcription results")?;
    atomic_json(checkpoint, &*parts)?;
    Ok(transcript(&parts, &detected_language, duration))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn quiet() -> Progress {
        Arc::new(|_| {})
    }
    #[test]
    fn e5_prefixes_and_cache_identity_preserve_existing_contract() {
        assert_eq!(prefixed("학교 자료", false), "passage: 학교 자료");
        assert_eq!(prefixed("학교 자료", true), "query: 학교 자료");
        assert_eq!(prefixed("query: explicit", false), "query: explicit");
        assert_eq!(cache_key("same", false), cache_key("passage: same", true));
        assert_ne!(cache_key("same", false), cache_key("same", true));
    }
    #[test]
    fn pooling_ignores_padding_and_outputs_unit_vectors() {
        let mut token = vec![0f32; DIMENSIONS];
        token[0] = 2.;
        let padding = vec![10000.; DIMENSIONS];
        let pooled = masked_mean(&[token, padding], &[1, 0]).unwrap();
        assert_eq!(pooled[0], 1.);
        assert!(pooled[1..].iter().all(|v| *v == 0.));
        assert!(masked_mean(&[vec![0.; DIMENSIONS]], &[0]).is_err());
        assert!(normalize(&mut [f32::NAN]).is_err());
    }
    #[test]
    fn ranks_only_supplied_text_and_never_returns_private_text() {
        let mut q = vec![0f32; DIMENSIONS];
        q[0] = 1.;
        let mut other = vec![0f32; DIMENSIONS];
        other[1] = 1.;
        let candidates = vec![
            Candidate {
                id: "permitted".into(),
                text: "query match private context".into(),
                label: "Public metadata".into(),
                href: "/own".into(),
                snippet: "allowed".into(),
            },
            Candidate {
                id: "other".into(),
                text: "unrelated".into(),
                label: "".into(),
                href: "".into(),
                snippet: "".into(),
            },
        ];
        let output = rank("query", &candidates, &q, &[q.clone(), other]).unwrap();
        assert_eq!(output["hits"][0]["id"], "permitted");
        assert_eq!(output["hits"][0]["score"], 1.0);
        assert!(!output.to_string().contains("private context"));
        assert_eq!(output["hits"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn srt_timestamps_round_and_checkpoint_reuse_does_not_redecode() {
        assert_eq!(stamp(1.9999), "00:00:02,000");
        assert_eq!(stamp(3600.001), "01:00:00,001");
        let tmp = tempfile::tempdir().unwrap();
        let checkpoint = tmp.path().join("parts.json");
        let parts = vec![Segment {
            start: 0.,
            end: 1.,
            text: "Synthetic transcript".into(),
        }];
        atomic_json(&checkpoint, &parts).unwrap();
        let result = transcribe_samples(
            Path::new("/nonexistent-model"),
            &vec![0.; 16000],
            Some("en"),
            &checkpoint,
            quiet(),
        )
        .unwrap();
        assert_eq!(result["text"], "Synthetic transcript");
        assert_eq!(result["duration"], 1.);
        assert_eq!(
            result["srt"],
            "1\n00:00:00,000 --> 00:00:01,000\nSynthetic transcript"
        );
        assert!(
            transcribe_samples(
                Path::new("/missing"),
                &[f32::NAN],
                None,
                &checkpoint,
                quiet()
            )
            .is_err()
        );
    }
    #[test]
    fn execution_selection_never_probes_unassigned_devices() {
        assert_eq!(assignment(false, None).unwrap(), Execution::Cpu);
        assert_eq!(
            assignment(true, Some("GPU-00000000-0000-0000-0000-000000000001")).unwrap(),
            Execution::Cpu
        );
        assert_eq!(
            assignment(false, Some("GPU-00000000-0000-0000-0000-000000000001")).unwrap(),
            Execution::AssignedGpu
        );
        assert!(assignment(false, Some("0,1")).is_err());
        assert!(assignment(false, Some("0")).is_err());
    }
    fn wav(path: &Path) {
        let count = 16000u32;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + count * 2).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(16000u32.to_le_bytes());
        bytes.extend(32000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((count * 2).to_le_bytes());
        for i in 0..count {
            let value = ((i as f64 * 440. * std::f64::consts::TAU / 16000.).sin() * 3200.) as i16;
            bytes.extend(value.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
    }
    #[test]
    #[ignore = "requires native FFmpeg; only synthetic temp audio"]
    fn native_decoder_reads_seekable_fd_and_blocks_network_playlists() {
        let tmp = tempfile::tempdir().unwrap();
        let input = tmp.path().join("synthetic.wav");
        wav(&input);
        let samples = decode_audio(&ffmpeg_path().unwrap(), &input).unwrap();
        assert_eq!(samples.len(), 16000);
        assert!(samples.iter().all(|v| v.is_finite() && v.abs() < 0.2));
        let playlist = tmp.path().join("malicious.m3u8");
        fs::write(&playlist,"#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10,\nhttp://127.0.0.1:9/must-not-fetch.wav\n#EXT-X-ENDLIST\n").unwrap();
        assert!(decode_audio(&ffmpeg_path().unwrap(), &playlist).is_err());
    }
    #[test]
    #[ignore = "requires installed E5 assets; CPU only, synthetic public text"]
    fn local_e5_cpu_smoke() {
        let directory = std::env::var_os("CBGPU_E5_TEST_MODEL")
            .map(PathBuf::from)
            .expect("Set explicit readonly model path");
        let model = E5Model::load(&directory).unwrap();
        let texts = vec![
            "Paris is the capital of France.".into(),
            "A banana is a fruit.".into(),
        ];
        let vectors = model.vectors(&texts, false, &quiet()).unwrap();
        let query = model
            .vectors(&["What is the capital of France?".into()], true, &quiet())
            .unwrap();
        for v in &vectors {
            assert_eq!(v.len(), DIMENSIONS);
            assert!((v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() - 1.).abs() < 0.001)
        }
        let dot = |v: &[f32]| v.iter().zip(&query[0]).map(|(a, b)| a * b).sum::<f32>();
        assert!(dot(&vectors[0]) > dot(&vectors[1]));
        assert!(model.token_count(&["word ".repeat(1000)], false).unwrap() <= 512);
    }
    #[test]
    #[ignore = "requires pinned large-v3+VAD assets; CPU only and synthetic silence"]
    fn local_whisper_cpu_silence_smoke() {
        let model = std::env::var_os("CBGPU_WHISPER_TEST_MODEL")
            .map(PathBuf::from)
            .expect("Set explicit readonly model path");
        verify_whisper_model(&model).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let output = transcribe_samples(
            &model,
            &vec![0.; 16000],
            Some("en"),
            &tmp.path().join("parts.json"),
            quiet(),
        )
        .unwrap();
        assert_eq!(output["duration"], 1.);
        assert_eq!(output["model"], "cbgpu-whisper");
        assert!(
            output["segments"].as_array().unwrap().is_empty(),
            "VAD must not hallucinate on silence"
        );
    }
    #[test]
    #[ignore = "requires verified local large-v3 assets and explicit public speech WAV; CPU only"]
    fn local_whisper_cpu_public_speech_smoke() {
        let model = PathBuf::from(
            std::env::var_os("CBGPU_WHISPER_TEST_MODEL").expect("Explicit verified model path"),
        );
        let input = PathBuf::from(
            std::env::var_os("CBGPU_WHISPER_TEST_AUDIO").expect("Explicit public fixture path"),
        );
        let mut samples = decode_audio(&ffmpeg_path().unwrap(), &input).unwrap();
        samples.truncate(16000 * 5);
        let tmp = tempfile::tempdir().unwrap();
        let checkpoint = tmp.path().join("parts.json");
        let output =
            transcribe_samples(&model, &samples, Some("en"), &checkpoint, quiet()).unwrap();
        let text = output["text"].as_str().unwrap().to_ascii_lowercase();
        assert!(
            text.contains("american"),
            "Expected public JFK speech fixture: {text}"
        );
        let parts: Vec<Segment> = serde_json::from_slice(&fs::read(checkpoint).unwrap()).unwrap();
        assert!(!parts.is_empty());
        assert!(
            parts
                .iter()
                .all(|p| p.start >= 0. && p.end <= 5. && p.end > p.start)
        );
        assert!(parts.windows(2).all(|p| p[0].end <= p[1].start));
    }
}
