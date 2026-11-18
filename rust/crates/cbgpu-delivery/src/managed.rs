//! Narrow reviewed Node deployment-control scope. Unsupported native/AI/GPU units stay separate.
use crate::{Result, safe_relative, valid_sha};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPin {
    pub path: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeployBaseline {
    pub version: u32,
    pub service: String,
    pub source_sha: String,
    pub runtime_sha: String,
    pub unit_sha256: String,
    pub data_path: String,
    pub workers: Vec<WorkerPin>,
    pub reviewed: bool,
}
pub fn hash_valid(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl DeployBaseline {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.service != "deploy"
            || !self.reviewed
            || !valid_sha(&self.source_sha)
            || !valid_sha(&self.runtime_sha)
            || !hash_valid(&self.unit_sha256)
            || !std::path::Path::new(&self.data_path).is_absolute()
            || !self
                .data_path
                .ends_with("/frontend-deployments/deploy.sqlite")
            || self.workers.len() != 2
        {
            return Err("Invalid reviewed deployment service baseline".into());
        }
        for name in ["cbgpu-front-worker", "cbgpu-compose-worker"] {
            if self
                .workers
                .iter()
                .filter(|w| {
                    std::path::Path::new(&w.path)
                        .file_name()
                        .is_some_and(|s| s == name)
                        && std::path::Path::new(&w.path).is_absolute()
                        && hash_valid(&w.sha256)
                })
                .count()
                != 1
            {
                return Err("Exact reviewed frontend/Compose worker pins required".into());
            }
        }
        Ok(())
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum DeployPlan {
    Unchanged,
    Update(Vec<String>),
    Hold(&'static str),
}
pub fn plan(paths: &[String]) -> Result<DeployPlan> {
    if paths.iter().any(|p| !safe_relative(p)) {
        return Err("Unsafe changed path".into());
    }
    let managed: Vec<_> = paths
        .iter()
        .filter(|p| p.starts_with("deploy/") && !p.ends_with(".test.mts") && !p.ends_with(".md"))
        .cloned()
        .collect();
    // Independent schema/AI history never triggers a service restart. If the
    // service's code does change, ANY migration requires explicit DB review;
    // do not infer authorization compatibility from a filename.
    if managed.is_empty() {
        return Ok(DeployPlan::Unchanged);
    }
    if paths.iter().any(|p| p.starts_with("supabase/migrations/")) {
        return Ok(DeployPlan::Hold(
            "Managed deploy code and database changes require an explicit reviewed baseline",
        ));
    }
    if paths.iter().any(|p| {
        ["package.json", "package-lock.json", "tooling/contracts.mts"].contains(&p.as_str())
            || p.starts_with("rust/compat/")
            || p.starts_with("rust/crates/cbgpu-front-worker/")
            || p.starts_with("rust/crates/cbgpu-compose-worker/")
            || p.starts_with("rust/crates/cbgpu-domains/")
            || p.starts_with("rust/crates/cbgpu-core/")
            || p.starts_with("deploy/migrations/")
            || p.starts_with("schema/deploy")
    }) {
        return Ok(DeployPlan::Hold(
            "Reviewed deploy dependency/worker/schema baseline required",
        ));
    }
    if managed.len() >= 300 {
        return Ok(DeployPlan::Hold(
            "Managed deployment-control change set needs review",
        ));
    }
    if managed.is_empty() {
        Ok(DeployPlan::Unchanged)
    } else {
        Ok(DeployPlan::Update(managed))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_history_never_counts_as_managed_code() {
        let mut p: Vec<_> = (0..900)
            .map(|n| format!("supabase/migrations/{n}_school.sql"))
            .collect();
        p.extend([
            "ai/server.py".into(),
            "gpu/server.mts".into(),
            "docs/guide.md".into(),
        ]);
        assert_eq!(plan(&p).unwrap(), DeployPlan::Unchanged);
        p.push("deploy/server.mts".into());
        assert!(matches!(plan(&p).unwrap(), DeployPlan::Hold(_)));
        let mut docs: Vec<_> = (0..900).map(|n| format!("docs/{n}.md")).collect();
        docs.extend(["ai/server.py".into(), "deploy/server.mts".into()]);
        assert_eq!(
            plan(&docs).unwrap(),
            DeployPlan::Update(vec!["deploy/server.mts".into()])
        );
    }
    #[test]
    fn real_shared_dependency_changes_are_held() {
        for p in [
            "package-lock.json",
            "rust/compat/worker-command.mts",
            "rust/crates/cbgpu-compose-worker/src/main.rs",
            "deploy/migrations/new.sql",
        ] {
            assert!(matches!(
                plan(&["deploy/server.mts".into(), p.into()]).unwrap(),
                DeployPlan::Hold(_)
            ));
        }
        assert!(plan(&["../unsafe".into()]).is_err());
    }
}
