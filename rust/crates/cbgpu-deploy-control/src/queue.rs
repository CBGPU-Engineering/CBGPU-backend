//! Durable leases and atomic promotions. No in-memory queue is authoritative.
use crate::{Result, fail, now, store::Store};
use serde_json::{Value, json};
use sqlx::Row;
#[derive(Clone, Debug)]
pub struct Work {
    pub id: String,
    pub kind: String,
    pub resource: String,
    pub project: String,
    pub owner: String,
    pub deployment: Option<String>,
    pub payload: Value,
    pub lease: String,
}
impl Store {
    pub async fn claim(&self, kinds: &[String]) -> Result<Option<Work>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('deploy:claim',0))")
            .execute(&mut *tx)
            .await?;
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.work_items WHERE state='running'AND kind=ANY($1))").bind(kinds).fetch_one(&mut*tx).await?;
        if active {
            return Ok(None);
        }
        let row=sqlx::query("SELECT * FROM deploy_store.work_items WHERE state='queued'AND cancel_requested=false AND kind=ANY($1)ORDER BY created_at,id FOR UPDATE SKIP LOCKED LIMIT 1").bind(kinds).fetch_optional(&mut*tx).await?;
        let Some(row) = row else { return Ok(None) };
        let id: String = row.get("id");
        let lease = uuid::Uuid::new_v4().to_string();
        let work = Work {
            id: id.clone(),
            kind: row.get("kind"),
            resource: row.get("resource_type"),
            project: row.get("project_id"),
            owner: row.get("owner"),
            deployment: row.get("deployment_id"),
            payload: row.get("payload"),
            lease: lease.clone(),
        };
        sqlx::query("UPDATE deploy_store.work_items SET state='running',lease=$1,attempts=attempts+1,updated_at=$2 WHERE id=$3").bind(lease).bind(now()).bind(id).execute(&mut*tx).await?;
        if let Some(deployment) = &work.deployment {
            sqlx::query("UPDATE deploy_store.deployments SET status='building'WHERE id=$1 AND status='queued'").bind(deployment).execute(&mut*tx).await?;
        }
        tx.commit().await?;
        Ok(Some(work))
    }
    pub async fn continuing(&self, w: &Work) -> Result<bool> {
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.work_items WHERE id=$1 AND state='running'AND lease=$2 AND cancel_requested=false)").bind(&w.id).bind(&w.lease).fetch_one(&self.pool).await?;
        if active {
            sqlx::query("UPDATE deploy_store.work_items SET updated_at=$1 WHERE id=$2 AND lease=$3 AND state='running'").bind(now()).bind(&w.id).bind(&w.lease).execute(&self.pool).await?;
        }
        Ok(active)
    }
    pub async fn logs_update(
        &self,
        w: &Work,
        logs: &str,
        diagnostics: Option<&Value>,
        attempt: Option<&Value>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.work_items WHERE id=$1 AND lease=$2 AND state='running')").bind(&w.id).bind(&w.lease).fetch_one(&mut*tx).await?;
        if !valid {
            return Err(fail("Worker lease lost", 409));
        }
        sqlx::query("UPDATE deploy_store.work_items SET updated_at=$1 WHERE id=$2")
            .bind(now())
            .bind(&w.id)
            .execute(&mut *tx)
            .await?;
        if let Some(d) = &w.deployment {
            sqlx::query("UPDATE deploy_store.deployments SET logs=$1 WHERE id=$2")
                .bind(logs)
                .bind(d)
                .execute(&mut *tx)
                .await?;
        }
        if w.kind == "compose_build" {
            sqlx::query("UPDATE deploy_store.compose_states SET logs=$1,diagnostics=coalesce($2,diagnostics),attempt_result=coalesce($3,attempt_result),updated_at=$4 WHERE project_id=$5").bind(logs).bind(diagnostics).bind(attempt).bind(now()).bind(&w.project).execute(&mut*tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn finish_work(
        &self,
        w: &Work,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        if w.kind == "front_build" {
            sqlx::query("SELECT id FROM deploy_store.projects WHERE id=$1 FOR UPDATE")
                .bind(&w.project)
                .fetch_optional(&mut *tx)
                .await?;
        }
        let row=sqlx::query("SELECT cancel_requested FROM deploy_store.work_items WHERE id=$1 AND lease=$2 AND state='running'FOR UPDATE").bind(&w.id).bind(&w.lease).fetch_optional(&mut*tx).await?.ok_or_else(||fail("Worker lease lost",409))?;
        let cancelled: bool = row.get("cancel_requested");
        let success = error.is_none()
            && !cancelled
            && (!matches!(w.kind.as_str(), "front_build" | "compose_build") || result.is_some());
        let time = now();
        let message = error.unwrap_or(if cancelled { "Operation cancelled" } else { "" });
        if w.kind == "front_build" {
            let d = w
                .deployment
                .as_ref()
                .ok_or_else(|| fail("Deployment lease malformed", 503))?;
            let deleted: Option<i32> = sqlx::query_scalar(
                "SELECT deleted FROM deploy_store.projects WHERE id=$1 FOR UPDATE",
            )
            .bind(&w.project)
            .fetch_optional(&mut *tx)
            .await?;
            if success && deleted == Some(0) && result.is_some() {
                sqlx::query("UPDATE deploy_store.deployments SET status='ready',result=$1,error=NULL,finished_at=$2 WHERE id=$3").bind(result).bind(&time).bind(d).execute(&mut*tx).await?;
                sqlx::query("UPDATE deploy_store.projects SET production=$1 WHERE id=$2")
                    .bind(d)
                    .bind(&w.project)
                    .execute(&mut *tx)
                    .await?;
                let expired:Vec<String>=sqlx::query_scalar("SELECT id FROM deploy_store.deployments WHERE project_id=$1 AND status='ready' ORDER BY finished_at DESC,id DESC OFFSET 3").bind(&w.project).fetch_all(&mut*tx).await?;
                for old in expired {
                    Store::enqueue(
                        &mut tx,
                        "front_cleanup",
                        "frontend",
                        &w.project,
                        &w.owner,
                        None,
                        serde_json::json!({"deployment_id":old}),
                    )
                    .await?;
                }
            } else {
                sqlx::query("UPDATE deploy_store.deployments SET status='error',error=$1,finished_at=$2 WHERE id=$3").bind(if message.is_empty(){"Build failed; previous production remains active"}else{message}).bind(&time).bind(d).execute(&mut*tx).await?;
            }
        }
        if w.kind == "compose_build" {
            if success && result.is_some() {
                sqlx::query("UPDATE deploy_store.compose_states SET status='ready',result=$1,error=NULL,updated_at=$2 WHERE project_id=$3").bind(result).bind(&time).bind(&w.project).execute(&mut*tx).await?;
            } else {
                sqlx::query("UPDATE deploy_store.compose_states SET status='error',error=$1,updated_at=$2 WHERE project_id=$3").bind(if message.is_empty(){"Compose failed; containers and persistent volumes retained"}else{message}).bind(&time).bind(&w.project).execute(&mut*tx).await?;
            }
        }
        sqlx::query(
            "UPDATE deploy_store.work_items SET state=$1,error=$2,updated_at=$3 WHERE id=$4",
        )
        .bind(if cancelled {
            "cancelled"
        } else if success {
            "succeeded"
        } else {
            "failed"
        })
        .bind(error)
        .bind(&time)
        .bind(&w.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn recover_interrupted(&self) -> Result<Vec<Work>> {
        let mut tx = self.pool.begin().await?;
        let rows =
            sqlx::query("SELECT * FROM deploy_store.work_items WHERE state='running'FOR UPDATE")
                .fetch_all(&mut *tx)
                .await?;
        let mut jobs = vec![];
        for row in rows {
            jobs.push(Work {
                id: row.get("id"),
                kind: row.get("kind"),
                resource: row.get("resource_type"),
                project: row.get("project_id"),
                owner: row.get("owner"),
                deployment: row.get("deployment_id"),
                payload: row.get("payload"),
                lease: row.get::<Option<String>, _>("lease").unwrap_or_default(),
            });
        }
        sqlx::query("UPDATE deploy_store.work_items SET state='failed',error='Native controller interrupted; explicit redeploy required',updated_at=$1 WHERE state='running'").bind(now()).execute(&mut*tx).await?;
        sqlx::query("UPDATE deploy_store.deployments SET status='error',error='Build interrupted; previous production retained',finished_at=$1 WHERE status='building'").bind(now()).execute(&mut*tx).await?;
        sqlx::query("UPDATE deploy_store.compose_states SET status='interrupted',error='Deployment interrupted; volumes retained',updated_at=$1 WHERE status='building'").bind(now()).execute(&mut*tx).await?;
        tx.commit().await?;
        Ok(jobs)
    }
    pub async fn enqueue_compose(&self, p: &Value) -> Result<bool> {
        let id = p["id"]
            .as_str()
            .filter(|v| crate::valid_id(v))
            .ok_or_else(|| fail("Invalid server project", 400))?;
        let owner = p["user_id"]
            .as_str()
            .filter(|v| crate::valid_id(v))
            .ok_or_else(|| fail("Invalid server owner", 400))?;
        let requested = p["deploy_requested_at"]
            .as_str()
            .ok_or_else(|| fail("Deployment request timestamp missing", 400))?;
        let storage = p["env_vars"]["CBGPU_STORAGE_GB"]
            .as_str()
            .unwrap_or("40")
            .parse::<i64>()
            .ok()
            .filter(|v| (40..=200).contains(v))
            .ok_or_else(|| fail("Storage budget must be40–200GiB", 400))?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("compose:{id}"))
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('compose:queue',0))")
            .execute(&mut *tx)
            .await?;
        let queued:i64=sqlx::query_scalar("SELECT count(*)FROM deploy_store.work_items WHERE kind='compose_build'AND state='queued'").fetch_one(&mut *tx).await?;
        if queued >= 20 {
            return Ok(false);
        }
        let old: Option<String> = sqlx::query_scalar(
            "SELECT requested_at FROM deploy_store.compose_states WHERE project_id=$1",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        if old.as_deref() == Some(requested) {
            return Ok(false);
        }
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.work_items WHERE kind='compose_build'AND project_id=$1 AND state IN('queued','running'))").bind(id).fetch_one(&mut*tx).await?;
        if active {
            return Ok(false);
        }
        sqlx::query("INSERT INTO deploy_store.compose_states(project_id,requested_at,status,updated_at,storage_gb)VALUES($1,$2,'queued',$3,$4)ON CONFLICT(project_id)DO UPDATE SET requested_at=excluded.requested_at,status='queued',logs='',error=NULL,diagnostics=NULL,attempt_result=NULL,updated_at=excluded.updated_at,storage_gb=excluded.storage_gb").bind(id).bind(requested).bind(now()).bind(storage).execute(&mut*tx).await?;
        Self::enqueue(
            &mut tx,
            "compose_build",
            "server",
            id,
            owner,
            None,
            json!({"sealed_project":self.cipher.seal(p)?,"requested_by":p["deploy_requested_by"],"requested_at":p["deploy_requested_at"]}),
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }
}
