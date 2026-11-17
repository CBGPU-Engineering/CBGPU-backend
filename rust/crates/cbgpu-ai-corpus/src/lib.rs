//! Private full-text corpus. Every read rechecks current application RLS as the
//! owner of a trusted runtime job, never an owner supplied in request JSON.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "e5")]
use std::path::Path;
use uuid::Uuid;
pub mod importer;
pub mod ingest;
pub const REVISION: &str = "614241f622f53c4eeff9890bdc4f31cfecc418b3";
pub const DIMENSIONS: usize = 384;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceKey {
    pub kind: String,
    pub id: Uuid,
}
impl SourceKey {
    pub fn parse(value: &str) -> Result<Self> {
        let (kind, id) = value
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("Invalid source reference"))?;
        ensure!(
            ["material", "paper"].contains(&kind),
            "Unsupported corpus kind"
        );
        let parsed = Uuid::parse_str(id)?;
        ensure!(parsed.to_string() == id, "Noncanonical source ID");
        Ok(Self {
            kind: kind.into(),
            id: parsed,
        })
    }
    pub fn reference(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub kind: String,
    pub id: Uuid,
    pub version: String,
    pub name: String,
    pub label: String,
    pub href: String,
    pub text: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}
impl Source {
    pub fn key(&self) -> SourceKey {
        SourceKey {
            kind: self.kind.clone(),
            id: self.id,
        }
    }
    pub fn indexed_version(&self) -> String {
        format!("{}:{REVISION}", self.version)
    }
}
/// An implementation must use local E5 passage/query prefixes and unit vectors.
pub trait Embedder {
    fn embed(&self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>>;
}
pub fn validate_vector(v: &[f32]) -> Result<()> {
    ensure!(
        v.len() == DIMENSIONS && v.iter().all(|x| x.is_finite()),
        "Invalid corpus vector"
    );
    let norm = v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    ensure!(
        (0.9..=1.1).contains(&norm),
        "Corpus vectors must be normalized"
    );
    Ok(())
}
#[derive(Clone)]
pub struct Corpus {
    runtime: PgPool,
    policy: PgPool,
    account_v2: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct TrustedJob {
    pub owner: Uuid,
    pub input: Value,
    id: String,
}
impl TrustedJob {
    pub fn id(&self) -> &str {
        &self.id
    }
}
impl Corpus {
    pub fn new(runtime: PgPool, policy: PgPool) -> Self {
        Self {
            runtime,
            policy,
            account_v2: false,
        }
    }
    /// Operator-controlled and default-off; never derive this from job input.
    pub fn with_account_v2(mut self, enabled: bool) -> Self {
        self.account_v2 = enabled;
        self
    }
    // Only the scheduler/API may choose this ID. An HTTP body must never select a
    // different caller's job; no API endpoint is exposed by this crate.
    async fn job(&self, job_id: &str) -> Result<(Uuid, Value)> {
        let id = Uuid::parse_str(job_id)?;
        ensure!(id.to_string() == job_id, "Invalid trusted job ID");
        let row=sqlx::query("SELECT owner,input FROM ai_store.jobs WHERE id=$1 AND kind='search' AND state IN('queued','running')").bind(job_id).fetch_optional(&self.runtime).await?.ok_or_else(||anyhow::anyhow!("Search job is not active"))?;
        let owner: String = row.try_get("owner")?;
        Ok((Uuid::parse_str(&owner)?, row.try_get("input")?))
    }
    pub async fn trusted_job(&self, id: &str) -> Result<TrustedJob> {
        let (owner, input) = self.job(id).await?;
        Ok(TrustedJob {
            owner,
            input,
            id: id.into(),
        })
    }
    pub async fn candidate_sources(&self, snapshot: &TrustedJob) -> Result<Vec<Source>> {
        let current = self.trusted_job(&snapshot.id).await?;
        ensure!(
            &current == snapshot,
            "Trusted search input or owner changed"
        );
        let keys = candidate_keys(&snapshot.input)?;
        self.sources(snapshot.owner, &keys).await
    }
    pub async fn recheck(&self, snapshot: &TrustedJob) -> Result<Vec<Source>> {
        self.candidate_sources(snapshot).await
    }
    async fn sources(&self, owner: Uuid, keys: &[SourceKey]) -> Result<Vec<Source>> {
        ensure!(keys.len() <= 500, "Too many source references");
        let refs: Vec<_> = keys.iter().map(SourceKey::reference).collect();
        let result: Value =
            sqlx::query_scalar("SELECT cbgpu_corpus_auth.sources_for_owner($1,$2,$3)")
                .bind(owner)
                .bind(json!(refs))
                .bind(self.account_v2)
                .fetch_one(&self.policy)
                .await?;
        Ok(serde_json::from_value(result)?)
    }
    /// Caller IDs are only hints. Owner and candidate permit-set come from the
    /// trusted job row. Fresh RLS is then applied before any cached text is read.
    pub async fn authorized_sources(&self, job_id: &str) -> Result<Vec<Source>> {
        let (owner, input) = self.job(job_id).await?;
        let keys = job_keys(&input)?;
        self.sources(owner, &keys).await
    }
    async fn index_text(
        &self,
        job_id: &str,
        key: &SourceKey,
        text: &str,
        embedder: &dyn Embedder,
    ) -> Result<usize> {
        let (owner, input) = self.job(job_id).await?;
        ensure!(
            job_keys(&input)?.contains(key),
            "Source not in trusted job candidate set"
        );
        let source = self
            .sources(owner, std::slice::from_ref(key))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Source is not currently readable"))?;
        let chunks = chunk_text(text);
        ensure!(!chunks.is_empty(), "No extractable text");
        let vectors = embedder.embed(&chunks, false)?;
        ensure!(vectors.len() == chunks.len(), "Embedding count mismatch");
        for vector in &vectors {
            validate_vector(vector)?;
        }
        // Recheck after expensive extraction/embedding; revoked content is not indexed.
        let refreshed = self.job(job_id).await?;
        ensure!(
            refreshed.0 == owner && job_keys(&refreshed.1)? == job_keys(&input)?,
            "Trusted job identity or source scope changed"
        );
        let current = self
            .sources(owner, std::slice::from_ref(key))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Source access was revoked"))?;
        ensure!(
            current.version == source.version,
            "Source changed during indexing"
        );
        let mut tx = self.runtime.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(key.reference())
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO ai_corpus.documents(kind,id,version,state,checked)VALUES($1,$2,$3,'ready',$4)ON CONFLICT(kind,id)DO UPDATE SET version=excluded.version,state='ready',checked=excluded.checked").bind(&key.kind).bind(key.id).bind(source.indexed_version()).bind(now()).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM ai_corpus.chunks WHERE kind=$1 AND id=$2")
            .bind(&key.kind)
            .bind(key.id)
            .execute(&mut *tx)
            .await?;
        for (i, (text, vector)) in chunks.iter().zip(&vectors).enumerate() {
            sqlx::query(
                "INSERT INTO ai_corpus.chunks(kind,id,number,text,vector)VALUES($1,$2,$3,$4,$5)",
            )
            .bind(&key.kind)
            .bind(key.id)
            .bind(i as i32)
            .bind(text)
            .bind(json!(vector))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(chunks.len())
    }
    /// Full document bytes must come from SourceFiles (trusted root/controlled PDF
    /// endpoint), never a path or URL supplied by a customer job.
    pub async fn ingest(
        &self,
        job_id: &str,
        key: &SourceKey,
        files: &ingest::SourceFiles,
        embedder: &dyn Embedder,
    ) -> Result<usize> {
        let source = self
            .authorized_sources(job_id)
            .await?
            .into_iter()
            .find(|s| s.key() == *key)
            .ok_or_else(|| anyhow::anyhow!("Source not readable"))?;
        let text = {
            let files = files.clone();
            let source = source.clone();
            tokio::task::spawn_blocking(move || files.extract(&source)).await??
        };
        self.index_text(job_id, key, &text, embedder).await
    }
    pub async fn search(
        &self,
        job_id: &str,
        query: &str,
        embedder: &dyn Embedder,
    ) -> Result<Value> {
        ensure!(
            !query.trim().is_empty() && query.chars().count() <= 4000,
            "Invalid search query"
        );
        let (owner, input) = self.job(job_id).await?;
        let keys = job_keys(&input)?;
        let allowed = self.sources(owner, &keys).await?;
        if allowed.is_empty() {
            return Ok(json!({"hits":[],"model":"cbgpu-embeddings"}));
        }
        let q = embedder.embed(&[query.to_string()], true)?;
        ensure!(q.len() == 1, "Invalid query embedding count");
        validate_vector(&q[0])?;
        let mut collected = vec![];
        for source in &allowed {
            let rows=sqlx::query("SELECT c.number,c.text,c.vector FROM ai_corpus.chunks c JOIN ai_corpus.documents d USING(kind,id)WHERE c.kind=$1 AND c.id=$2 AND d.state='ready'AND d.version=$3 ORDER BY c.number LIMIT 400").bind(&source.kind).bind(source.id).bind(source.indexed_version()).fetch_all(&self.runtime).await?;
            for row in rows {
                let v: Value = row.try_get("vector")?;
                let vector: Vec<f32> = serde_json::from_value(v)?;
                validate_vector(&vector)?;
                collected.push((source.key(), row.try_get::<String, _>("text")?, vector));
            }
            ensure!(
                collected.len() <= 200_000,
                "Corpus candidate budget exceeded"
            );
        }
        // Final check before returning private snippets, including policy/membership
        // changes while CPU ranking/cache reads ran. Metadata also comes from this read.
        let refreshed = self.job(job_id).await?;
        ensure!(
            refreshed.0 == owner && job_keys(&refreshed.1)? == keys,
            "Trusted job identity or source scope changed"
        );
        let final_sources = self.sources(owner, &keys).await?;
        let current: BTreeMap<_, _> = final_sources.into_iter().map(|s| (s.key(), s)).collect();
        let original: BTreeMap<_, _> = allowed.into_iter().map(|s| (s.key(), s.version)).collect();
        let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
        let mut best: BTreeMap<String, Value> = BTreeMap::new();
        for (key, text, v) in collected {
            let Some(source) = current
                .get(&key)
                .filter(|s| original.get(&key) == Some(&s.version))
            else {
                continue;
            };
            let dot = q[0]
                .iter()
                .zip(v)
                .map(|(a, b)| f64::from(*a) * f64::from(b))
                .sum::<f64>();
            let lower = text.to_lowercase();
            let lexical = words.iter().filter(|w| lower.contains(w.as_str())).count() as f64
                / words.len().max(1) as f64;
            let score = ((dot * 0.8 + lexical * 0.2) * 100000.).round() / 100000.;
            let id = key.reference();
            let hit = json!({"id":id,"label":source.label,"href":source.href,"snippet":text.chars().take(350).collect::<String>(),"score":score});
            if best
                .get(&id)
                .is_none_or(|old| old["score"].as_f64().unwrap_or(-2.) < score)
            {
                best.insert(id, hit);
            }
        }
        let mut hits: Vec<_> = best.into_values().collect();
        hits.sort_by(|a, b| {
            b["score"]
                .as_f64()
                .unwrap()
                .total_cmp(&a["score"].as_f64().unwrap())
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        hits.truncate(20);
        Ok(json!({"hits":hits,"model":"cbgpu-embeddings"}))
    }
    pub async fn ready(&self, job_id: &str, key: &SourceKey) -> Result<bool> {
        let Some(source) = self
            .authorized_sources(job_id)
            .await?
            .into_iter()
            .find(|s| s.key() == *key)
        else {
            return Ok(false);
        };
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ai_corpus.documents WHERE kind=$1 AND id=$2 AND version=$3 AND state='ready')").bind(&key.kind).bind(key.id).bind(source.indexed_version()).fetch_one(&self.runtime).await?)
    }
}
fn candidate_keys(input: &Value) -> Result<Vec<SourceKey>> {
    let candidates = input["candidates"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Search candidates missing"))?;
    ensure!(candidates.len() <= 500, "Too many search candidates");
    Ok(candidates
        .iter()
        .filter_map(|c| c["id"].as_str())
        .filter_map(|id| SourceKey::parse(id).ok())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}
fn job_keys(input: &Value) -> Result<Vec<SourceKey>> {
    let candidates = input["candidates"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Search candidates missing"))?;
    ensure!(candidates.len() <= 500, "Too many search candidates");
    let candidate_ids: BTreeSet<_> = candidate_keys(input)?.into_iter().collect();
    let Some(requested) = input.get("corpus_ids") else {
        return Ok(candidate_ids.into_iter().collect());
    };
    let requested = requested
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid corpus references"))?;
    ensure!(requested.len() <= 500, "Too many corpus references");
    let mut keys = BTreeSet::new();
    for id in requested {
        let key = SourceKey::parse(
            id.as_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid source ID"))?,
        )?;
        if candidate_ids.contains(&key) {
            keys.insert(key);
        }
    }
    Ok(keys.into_iter().collect())
}
pub fn chunk_text(text: &str) -> Vec<String> {
    let chars: Vec<_> = text.chars().take(200000).collect();
    (0..chars.len())
        .step_by(500)
        .filter_map(|i| {
            let s: String = chars[i..(i + 650).min(chars.len())].iter().collect();
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            (!s.is_empty()).then_some(s)
        })
        .collect()
}
pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
#[cfg(feature = "e5")]
pub struct NativeE5(cbgpu_ai_models::E5Model);
#[cfg(feature = "e5")]
impl NativeE5 {
    pub fn load(root: &Path) -> Result<Self> {
        Ok(Self(
            cbgpu_ai_models::E5Model::load_on(root, cbgpu_ai_models::Execution::Cpu)
                .map_err(|e| anyhow::anyhow!(e))?,
        ))
    }
}
#[cfg(feature = "e5")]
impl Embedder for NativeE5 {
    fn embed(&self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>> {
        let progress: cbgpu_ai_models::Progress = std::sync::Arc::new(|_| {});
        self.0
            .vectors(texts, query, &progress)
            .map_err(|e| anyhow::anyhow!(e))
    }
}
