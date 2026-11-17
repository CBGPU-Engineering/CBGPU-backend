//! Standalone corpus/model process: never linked into the llama.cpp process.
use anyhow::{Result, ensure};
use cbgpu_ai_corpus::{Corpus, Embedder, Source, SourceKey, TrustedJob, ingest::SourceFiles};
use cbgpu_ai_models::{Candidate, rank};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub fn owned_job(folder: &Path, state: &Path) -> Result<String> {
    let root = state.canonicalize()?;
    let folder = folder.canonicalize()?;
    ensure!(
        folder.parent() == Some(root.join("jobs").as_path()),
        "Invalid owned job folder"
    );
    let id = folder
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid job identity"))?;
    ensure!(
        uuid::Uuid::parse_str(id)?.to_string() == id,
        "Invalid job identity"
    );
    Ok(id.into())
}

/// Metadata/caller-text ranks are retained; full text is read only through Corpus.
pub async fn search(
    job: &TrustedJob,
    corpus: &Corpus,
    files: &SourceFiles,
    model: &dyn Embedder,
) -> Result<Value> {
    let input = &job.input;
    let query = input["query"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Query required"))?;
    ensure!(
        !query.trim().is_empty() && query.chars().count() <= 4000,
        "Invalid search query"
    );
    let supplied: Vec<Candidate> = serde_json::from_value(input["candidates"].clone())?;
    let current = corpus.candidate_sources(job).await?;
    let candidates = safe_candidates(&supplied, &current);
    ensure!(candidates.len() <= 500, "Too many search candidates");
    // Always establish trusted active job ownership before model/file work.
    let sources = corpus.authorized_sources(job.id()).await?;
    let metadata = if candidates.is_empty() {
        json!({"hits":[]})
    } else {
        let question = model.embed(&[query.into()], true)?;
        ensure!(question.len() == 1, "Invalid query embedding count");
        let texts = candidates
            .iter()
            .map(|c| c.text.clone())
            .collect::<Vec<_>>();
        rank(
            query,
            &candidates,
            &question[0],
            &model.embed(&texts, false)?,
        )
        .map_err(|_| anyhow::anyhow!("Search ranking unavailable"))?
    };
    for source in sources {
        let key = source.key();
        if !corpus.ready(job.id(), &key).await? {
            // Unsupported/corrupt documents retain their caller-visible metadata.
            // Do not publish parser errors/paths or stale cached text.
            let _ = corpus.ingest(job.id(), &key, files, model).await;
        }
    }
    // Corpus rechecks current job owner, source RLS/version and bans after ranking.
    let authorized = corpus.search(job.id(), query, model).await?;
    let fresh = corpus.recheck(job).await?;
    let fresh_by_id: BTreeMap<_, _> = fresh.iter().map(|s| (s.key().reference(), s)).collect();
    let initial: BTreeMap<_, _> = current.iter().map(|s| (s.key().reference(), s)).collect();
    let mut result = merge(&metadata, &authorized);
    if let Some(hits) = result["hits"].as_array_mut() {
        hits.retain_mut(|hit| {
            let Some(id) = hit["id"].as_str().map(str::to_owned) else {
                return false;
            };
            if id.starts_with("doc:") || id.starts_with("note:") {
                return true;
            }
            let Some(source) = fresh_by_id.get(&id) else {
                return false;
            };
            if !initial
                .get(&id)
                .is_some_and(|old| old.version == source.version && old.text == source.text)
            {
                return false;
            }
            hit["label"] = json!(source.label);
            hit["href"] = json!(source.href);
            true
        });
    }
    Ok(result)
}

fn safe_candidates(supplied: &[Candidate], sources: &[Source]) -> Vec<Candidate> {
    let visible: BTreeMap<_, _> = sources.iter().map(|s| (s.key().reference(), s)).collect();
    supplied
        .iter()
        .filter_map(|candidate| {
            if SourceKey::parse(&candidate.id).is_ok() {
                let source = visible.get(&candidate.id)?;
                return Some(Candidate {
                    id: candidate.id.clone(),
                    text: source.text.chars().take(8000).collect(),
                    label: source.label.clone(),
                    href: source.href.clone(),
                    snippet: source.text.chars().take(250).collect(),
                });
            }
            // Notes stay only in their caller-authorized private job snapshot; never
            // grant them global corpus IDs, database indexing or filesystem reads.
            if let Some(id) = candidate.id.strip_prefix("note:") {
                if uuid::Uuid::parse_str(id)
                    .ok()
                    .is_some_and(|parsed| parsed.to_string() == id)
                {
                    return Some(Candidate {
                        id: candidate.id.clone(),
                        text: candidate.text.chars().take(8000).collect(),
                        label: candidate.label.clone(),
                        href: format!("/notes?note={id}"),
                        snippet: candidate.snippet.chars().take(250).collect(),
                    });
                }
                return None;
            }
            // Documentation is already-public text supplied by the trusted BFF snapshot.
            // Rebuild the link so request text cannot create javascript/foreign targets.
            let slug = candidate.id.strip_prefix("doc:")?;
            if slug.is_empty()
                || slug.len() > 200
                || slug.contains("..")
                || !slug
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_ /".contains(&b))
                || slug.contains(' ')
            {
                return None;
            }
            Some(Candidate {
                id: candidate.id.clone(),
                text: candidate.text.chars().take(8000).collect(),
                label: candidate.label.clone(),
                href: format!("https://docs.cbgpu.com/{slug}"),
                snippet: candidate.snippet.chars().take(250).collect(),
            })
        })
        .collect()
}
pub fn merge(metadata: &Value, authorized: &Value) -> Value {
    let mut best = BTreeMap::<String, Value>::new();
    for source in [metadata, authorized] {
        for hit in source["hits"].as_array().into_iter().flatten() {
            let Some(id) = hit["id"].as_str() else {
                continue;
            };
            let Some(score) = hit["score"].as_f64().filter(|n| n.is_finite()) else {
                continue;
            };
            if best
                .get(id)
                .is_none_or(|old| old["score"].as_f64().unwrap_or(-2.) < score)
            {
                best.insert(id.into(), hit.clone());
            }
        }
    }
    // Source metadata comes from current policy reads, even when the supplied
    // text rank beats its full-text score; never retain a forged source label.
    for source in authorized["hits"].as_array().into_iter().flatten() {
        if let Some(hit) = source["id"].as_str().and_then(|id| best.get_mut(id)) {
            hit["label"] = source["label"].clone();
            hit["href"] = source["href"].clone();
        }
    }
    let mut hits = best.into_values().collect::<Vec<_>>();
    hits.sort_by(|a, b| {
        b["score"]
            .as_f64()
            .unwrap()
            .total_cmp(&a["score"].as_f64().unwrap())
            .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
    });
    hits.truncate(20);
    json!({"hits":hits,"model":"cbgpu-embeddings"})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corpus_metadata_overrides_spoofed_labels_and_ranking_is_bounded() {
        let metadata = json!({"hits":[{"id":"material:one","label":"forged","href":"https://attacker.invalid","snippet":"own supplied text","score":0.9}]});
        let corpus = json!({"hits":[{"id":"material:one","label":"Current source","href":"/materials","snippet":"authorized file text","score":0.8}]});
        let result = merge(&metadata, &corpus);
        assert_eq!(result["hits"][0]["label"], "Current source");
        assert_eq!(result["hits"][0]["href"], "/materials");
        assert_eq!(result["hits"][0]["score"], 0.9);
        let many = json!({"hits":(0..40).map(|i|json!({"id":i.to_string(),"score":i as f64})).collect::<Vec<_>>()});
        assert_eq!(
            merge(&many, &json!({"hits":[]}))["hits"]
                .as_array()
                .unwrap()
                .len(),
            20
        );
    }
    #[test]
    fn scheduler_folder_cannot_select_another_state_or_noncanonical_identity() {
        let root = tempfile::tempdir().unwrap();
        let id = "60000000-0000-4000-8000-000000000001";
        let folder = root.path().join("jobs").join(id);
        std::fs::create_dir_all(&folder).unwrap();
        assert_eq!(owned_job(&folder, root.path()).unwrap(), id);
        assert!(owned_job(root.path(), root.path()).is_err());
    }
}
