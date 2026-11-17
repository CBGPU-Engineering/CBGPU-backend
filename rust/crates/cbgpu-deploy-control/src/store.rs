use crate::{Result, config, crypto::Cipher, fail, now, valid_id};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::path::PathBuf;
#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
    pub cipher: Cipher,
    pub data: PathBuf,
    pub free_floor: u64,
}
#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub owner: String,
    pub slug: String,
    pub config: Value,
    pub secrets: String,
    pub production: Option<String>,
    pub created_at: String,
    pub role: String,
    pub actor: String,
}
fn project(row: sqlx::postgres::PgRow, role: String, actor: &str) -> Project {
    Project {
        id: row.get("id"),
        owner: row.get("owner"),
        slug: row.get("slug"),
        config: row.get("config"),
        secrets: row.get("secrets"),
        production: row.get("production"),
        created_at: row.get("created_at"),
        role,
        actor: actor.into(),
    }
}
impl Store {
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|_| fail("Deployment schema provisioning failed", 503))
    }
    pub async fn role(
        &self,
        kind: &str,
        id: &str,
        resource_owner: &str,
        actor: &str,
    ) -> Result<String> {
        if resource_owner == actor {
            return Ok("owner".into());
        }
        let role:Option<String>=sqlx::query_scalar("SELECT role FROM deploy_store.project_members WHERE resource_type=$1 AND project_id=$2 AND user_id=$3").bind(kind).bind(id).bind(actor).fetch_optional(&self.pool).await?;
        role.ok_or_else(|| fail("Project not found", 404))
    }
    pub async fn find(&self, id: &str, actor: &str) -> Result<Project> {
        if !valid_id(id) {
            return Err(fail("Project not found", 404));
        }
        let row = sqlx::query("SELECT * FROM deploy_store.projects WHERE id=$1 AND deleted=0")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| fail("Project not found", 404))?;
        let owner: String = row.get("owner");
        let role = self.role("frontend", id, &owner, actor).await?;
        Ok(project(row, role, actor))
    }
    pub async fn list(&self, actor: &str) -> Result<Value> {
        let rows=sqlx::query("SELECT p.* FROM deploy_store.projects p WHERE deleted=0 AND(owner=$1 OR EXISTS(SELECT 1 FROM deploy_store.project_members m WHERE resource_type='frontend' AND m.project_id=p.id AND m.user_id=$1))ORDER BY created_at DESC").bind(actor).fetch_all(&self.pool).await?;
        let mut list = vec![];
        for row in rows {
            let owner: String = row.get("owner");
            let id: String = row.get("id");
            let role = self.role("frontend", &id, &owner, actor).await?;
            list.push(self.output(&project(row, role, actor)).await?)
        }
        Ok(json!({"projects":list}))
    }
    pub async fn output(&self, p: &Project) -> Result<Value> {
        let secrets = self.cipher.unseal(&p.secrets)?;
        let rows=sqlx::query("SELECT id,status,error,created_at,finished_at,result FROM deploy_store.deployments WHERE project_id=$1 ORDER BY created_at DESC LIMIT 20").bind(&p.id).fetch_all(&self.pool).await?;
        let mut history = vec![];
        for row in rows {
            let id: String = row.get("id");
            let result: Option<Value> = row.get("result");
            history.push(json!({"id":id,"status":row.get::<String,_>("status"),"error":row.get::<Option<String>,_>("error"),"created_at":row.get::<String,_>("created_at"),"finished_at":row.get::<Option<String>,_>("finished_at"),"result":result.map(|v|json!({"commit":v["commit"],"framework":v["framework"]})),"url":format!("https://{}-{}.front.cbgpu.com",p.slug,&id.chars().take(8).collect::<String>())}));
        }
        let mut out = json!({});
        for key in [
            "slug",
            "repository",
            "branch",
            "framework",
            "auto_deploy",
            "root_directory",
            "output_directory",
            "install_command",
            "build_command",
        ] {
            if let Some(value) = p.config.get(key) {
                out[key] = value.clone()
            }
        }
        for(k,v)in json!({"id":p.id,"slug":p.slug,"collaboration_role":p.role,"created_at":p.created_at,"production":p.production,"domain":format!("{}.front.cbgpu.com",p.slug),"url":format!("https://{}.front.cbgpu.com",p.slug),"environment_keys":secrets["env"].as_object().map(|v|v.keys().cloned().collect::<Vec<_>>()).unwrap_or_default(),"git_connected":!secrets["git_token"].as_str().unwrap_or("").is_empty(),"deployments":history}).as_object().unwrap(){out[k]=v.clone()}
        Ok(out)
    }
    pub async fn create(&self, actor: &str, input: &Value) -> Result<Project> {
        let (config, secrets) = config::validate(input, &json!({}), &json!({}))?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("deploy-owner:{actor}"))
            .execute(&mut *tx)
            .await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM deploy_store.projects WHERE owner=$1 AND deleted=0",
        )
        .bind(actor)
        .fetch_one(&mut *tx)
        .await?;
        if count >= 5 {
            return Err(fail("Maximum5 projects per account", 429));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let sealed = self.cipher.seal(&secrets)?;
        sqlx::query("INSERT INTO deploy_store.projects(id,owner,slug,config,secrets,created_at)VALUES($1,$2,$3,$4,$5,$6)").bind(&id).bind(actor).bind(config["slug"].as_str().unwrap()).bind(&config).bind(&sealed).bind(now()).execute(&mut*tx).await?;
        tx.commit().await?;
        self.find(&id, actor).await
    }
    pub async fn lock_role(
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
        actor: &str,
    ) -> Result<String> {
        let owner: String = sqlx::query_scalar(
            "SELECT owner FROM deploy_store.projects WHERE id=$1 AND deleted=0 FOR UPDATE",
        )
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| fail("Project not found", 404))?;
        if owner == actor {
            return Ok("owner".into());
        }
        sqlx::query_scalar::<_,String>("SELECT role FROM deploy_store.project_members WHERE resource_type='frontend' AND project_id=$1 AND user_id=$2").bind(id).bind(actor).fetch_optional(&mut**tx).await?.ok_or_else(||fail("Project not found",404))
    }
    pub async fn lock_write(
        tx: &mut Transaction<'_, Postgres>,
        p: &Project,
        owner_only: bool,
    ) -> Result<()> {
        let role = Self::lock_role(tx, &p.id, &p.actor).await?;
        if role == "viewer" || owner_only && role != "owner" {
            return Err(fail("Insufficient project permission", 403));
        }
        Ok(())
    }
    pub async fn patch(&self, p: &Project, input: &Value) -> Result<Project> {
        if p.role == "viewer" {
            return Err(fail("View-only access", 403));
        }
        let mut tx = self.pool.begin().await?;
        Self::lock_write(&mut tx, p, false).await?;
        let row =
            sqlx::query("SELECT * FROM deploy_store.projects WHERE id=$1 AND deleted=0 FOR UPDATE")
                .bind(&p.id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| fail("Project not found", 404))?;
        let old: Value = row.get("config");
        let sealed: String = row.get("secrets");
        let mut input = input.clone();
        input["slug"] = json!(p.slug);
        let (config, secrets) = config::validate(&input, &old, &self.cipher.unseal(&sealed)?)?;
        sqlx::query("UPDATE deploy_store.projects SET config=$1,secrets=$2 WHERE id=$3")
            .bind(config)
            .bind(self.cipher.seal(&secrets)?)
            .bind(&p.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.find(&p.id, &p.actor).await
    }
    pub async fn schedule(&self, p: &Project, actor: &str) -> Result<String> {
        if p.role == "viewer" {
            return Err(fail("View-only access", 403));
        }
        if actor != p.actor {
            return Err(fail("Invalid project actor", 403));
        }
        self.check_disk()?;
        let mut tx = self.pool.begin().await?;
        Self::lock_write(&mut tx, p, false).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('deploy:queue',0))")
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query(
            "SELECT config,secrets FROM deploy_store.projects WHERE id=$1 AND deleted=0 FOR UPDATE",
        )
        .bind(&p.id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| fail("Project not found", 404))?;
        let active:i64=sqlx::query_scalar("SELECT count(*)FROM deploy_store.deployments WHERE project_id=$1 AND status IN('queued','building')").bind(&p.id).fetch_one(&mut*tx).await?;
        if active > 0 {
            return Err(fail("A deployment is already in progress", 409));
        }
        let queued:i64=sqlx::query_scalar("SELECT count(*)FROM deploy_store.work_items WHERE kind='front_build'AND state='queued'").fetch_one(&mut*tx).await?;
        if queued >= 20 {
            return Err(fail("Build queue is full", 429));
        }
        let cutoff = (chrono::Utc::now() - chrono::Duration::hours(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let recent: i64 = sqlx::query_scalar(
            "SELECT count(*)FROM deploy_store.deployments WHERE project_id=$1 AND created_at>$2",
        )
        .bind(&p.id)
        .bind(cutoff)
        .fetch_one(&mut *tx)
        .await?;
        if recent >= 20 {
            return Err(fail("Maximum20 builds per project per hour", 429));
        }
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO deploy_store.deployments(id,project_id,status,created_at,requested_by)VALUES($1,$2,'queued',$3,$4)").bind(&id).bind(&p.id).bind(now()).bind(actor).execute(&mut*tx).await?;
        Self::enqueue(&mut tx,"front_build","frontend",&p.id,&p.owner,Some(&id),json!({"config":row.get::<Value,_>("config"),"secrets":row.get::<String,_>("secrets"),"requested_by":actor})).await?;
        tx.commit().await?;
        Ok(id)
    }
    pub async fn enqueue(
        tx: &mut Transaction<'_, Postgres>,
        kind: &str,
        resource: &str,
        project: &str,
        owner: &str,
        deployment: Option<&str>,
        payload: Value,
    ) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let time = now();
        sqlx::query("INSERT INTO deploy_store.work_items(id,kind,resource_type,project_id,owner,deployment_id,payload,created_at,updated_at)VALUES($1,$2,$3,$4,$5,$6,$7,$8,$8)").bind(&id).bind(kind).bind(resource).bind(project).bind(owner).bind(deployment).bind(payload).bind(time).execute(&mut**tx).await?;
        Ok(id)
    }
    pub fn check_disk(&self) -> Result<()> {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(self.data.as_os_str().as_bytes())
            .map_err(|_| fail("Invalid deployment data path", 503))?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) } != 0 {
            return Err(fail("Build storage unavailable", 503));
        }
        let stats = unsafe { stats.assume_init() };
        if (stats.f_bavail as u128) * (stats.f_frsize as u128) < self.free_floor as u128 {
            return Err(fail("Build storage is temporarily full", 503));
        }
        Ok(())
    }
    pub async fn logs(&self, p: &Project, id: &str) -> Result<Value> {
        let row = sqlx::query(
            "SELECT status,logs,error FROM deploy_store.deployments WHERE id=$1 AND project_id=$2",
        )
        .bind(id)
        .bind(&p.id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| fail("Deployment not found", 404))?;
        Ok(
            json!({"status":row.get::<String,_>("status"),"logs":row.get::<String,_>("logs"),"error":row.get::<Option<String>,_>("error")}),
        )
    }
    pub async fn rollback(&self, p: &Project, id: &str) -> Result<Project> {
        if p.role == "viewer" {
            return Err(fail("View-only access", 403));
        }
        let mut tx = self.pool.begin().await?;
        Self::lock_write(&mut tx, p, false).await?;
        let ready:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM deploy_store.deployments WHERE id=$1 AND project_id=$2 AND status='ready' AND result IS NOT NULL)").bind(id).bind(&p.id).fetch_one(&mut*tx).await?;
        if !ready {
            return Err(fail("Deployment unavailable for rollback", 400));
        }
        sqlx::query("UPDATE deploy_store.projects SET production=$1 WHERE id=$2 AND deleted=0")
            .bind(id)
            .bind(&p.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.find(&p.id, &p.actor).await
    }
    pub async fn delete(&self, p: &Project) -> Result<()> {
        if p.role != "owner" {
            return Err(fail("Only the project owner can delete it", 403));
        }
        let mut tx = self.pool.begin().await?;
        Self::lock_write(&mut tx, p, true).await?;
        sqlx::query("UPDATE deploy_store.projects SET deleted=1 WHERE id=$1")
            .bind(&p.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM deploy_store.project_members WHERE resource_type='frontend'AND project_id=$1").bind(&p.id).execute(&mut*tx).await?;
        sqlx::query("UPDATE deploy_store.work_items SET cancel_requested=true,state=CASE WHEN state='queued'THEN'cancelled'ELSE state END WHERE resource_type='frontend'AND project_id=$1 AND state IN('queued','running')").bind(&p.id).execute(&mut*tx).await?;
        sqlx::query("UPDATE deploy_store.deployments SET status='error',error='Project deleted',finished_at=$1 WHERE project_id=$2 AND status IN('queued','building')").bind(now()).bind(&p.id).execute(&mut*tx).await?;
        Self::enqueue(
            &mut tx,
            "front_cleanup",
            "frontend",
            &p.id,
            &p.owner,
            None,
            json!({"deleted":true}),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}
