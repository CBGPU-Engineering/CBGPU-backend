//! Synthetic-only PostgreSQL fixture with no network and no production URLs.
#![cfg(unix)]
use cbgpu_ai_corpus::{Corpus, Embedder, ingest::SourceFiles};
use serde_json::{Value, json};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
    time::Duration,
};
const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";
const NOTE: &str = "10000000-0000-4000-8000-000000000001";
const PRIVATE: &str = "10000000-0000-4000-8000-000000000002";
const MEMBER: &str = "10000000-0000-4000-8000-000000000003";
const MATERIAL: &str = "20000000-0000-4000-8000-000000000001";
const PAPER: &str = "30000000-0000-4000-8000-000000000001";
const JOB: &str = "40000000-0000-4000-8000-000000000001";
const OTHER_JOB: &str = "40000000-0000-4000-8000-000000000002";
struct Fixture(String);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.0])
            .output();
    }
}
impl Fixture {
    fn sql(&self, text: &str) {
        let mut c = Command::new("docker")
            .args([
                "exec",
                "-i",
                &self.0,
                "psql",
                "-h",
                "/socket",
                "-U",
                "postgres",
                "-v",
                "ON_ERROR_STOP=1",
                "-At",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        let out = c.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
struct Toy;
impl Embedder for Toy {
    fn embed(&self, texts: &[String], _: bool) -> anyhow::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|_| {
                let mut v = vec![0.; 384];
                v[0] = 1.;
                v
            })
            .collect())
    }
}
fn input() -> Value {
    let ids = [
        format!("material:{NOTE}"),
        format!("material:{PRIVATE}"),
        format!("material:{MEMBER}"),
        format!("material:{MATERIAL}"),
        format!("paper:{PAPER}"),
    ];
    json!({"owner":OTHER,"query":"needle","candidates":ids.iter().map(|id|json!({"id":id,"text":"Caller-supplied metadata only","label":"forged label","href":"https://evil.example"})).collect::<Vec<_>>(),"corpus_ids":ids})
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Docker PG17 network-none, own synthetic fixture only; no production credentials"]
async fn trusted_metadata_lazy_fulltext_and_final_rls_revocation() {
    let socket = tempfile::tempdir().unwrap();
    std::fs::set_permissions(socket.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    let db = Fixture(format!(
        "cbgpu-corpus-test-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let out = Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            &db.0,
            "--network",
            "none",
            "--memory",
            "512m",
            "--cpus",
            "1",
            "--tmpfs",
            "/var/lib/postgresql/data:rw,size=256m",
            "-v",
            &format!("{}:/socket", socket.path().display()),
            "-e",
            "POSTGRES_HOST_AUTH_METHOD=trust",
            "-e",
            "PGHOST=/socket",
            "postgres:17-alpine",
            "postgres",
            "-k",
            "/var/run/postgresql,/socket",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut ready = false;
    for _ in 0..150 {
        if Command::new("docker")
            .args([
                "exec",
                &db.0,
                "pg_isready",
                "-h",
                "/socket",
                "-U",
                "postgres",
            ])
            .output()
            .is_ok_and(|o| o.status.success())
        {
            let logs = Command::new("docker")
                .args(["logs", &db.0])
                .output()
                .unwrap();
            if String::from_utf8_lossy(&logs.stdout).contains("PostgreSQL init process complete") {
                ready = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(ready);
    db.sql("CREATE ROLE anon NOLOGIN;CREATE ROLE authenticated NOLOGIN;CREATE ROLE cbgpu_ai NOLOGIN;CREATE SCHEMA auth;CREATE TABLE auth.users(id uuid PRIMARY KEY,deleted_at timestamptz,banned_until timestamptz);CREATE TABLE public.profiles(id uuid PRIMARY KEY,is_approved boolean,account_status text);CREATE FUNCTION auth.uid()RETURNS uuid LANGUAGE sql STABLE AS $$SELECT nullif(current_setting('request.jwt.claim.sub',true),'')::uuid$$;GRANT USAGE ON SCHEMA auth TO authenticated;GRANT EXECUTE ON FUNCTION auth.uid()TO authenticated;CREATE TABLE research_notes(id uuid PRIMARY KEY,user_id uuid,title text,content text,updated_at timestamptz DEFAULT now(),is_shared boolean DEFAULT false);CREATE TABLE note_members(note uuid,member uuid);CREATE TABLE shared_materials(id uuid PRIMARY KEY,uploader_id uuid,title text,file_name text,size_bytes bigint,status text,created_at timestamptz DEFAULT now(),visibility text DEFAULT 'private');CREATE TABLE preprints(id uuid PRIMARY KEY,status text,current_version int,title text,abstract text);CREATE TABLE preprint_versions(paper_id uuid,version int,pdf_size bigint,pdf_sha256 text);ALTER TABLE research_notes ENABLE ROW LEVEL SECURITY;ALTER TABLE shared_materials ENABLE ROW LEVEL SECURITY;ALTER TABLE preprints ENABLE ROW LEVEL SECURITY;ALTER TABLE preprint_versions ENABLE ROW LEVEL SECURITY;ALTER TABLE profiles ENABLE ROW LEVEL SECURITY;GRANT SELECT ON profiles,research_notes,shared_materials,note_members TO authenticated;CREATE POLICY own_profile ON profiles FOR SELECT TO authenticated USING(id=auth.uid());CREATE POLICY notes_own ON research_notes USING(user_id=auth.uid());CREATE POLICY notes_shared ON research_notes FOR SELECT USING(is_shared=true);CREATE POLICY notes_member ON research_notes FOR SELECT USING(EXISTS(SELECT 1 FROM note_members WHERE note=id AND member=auth.uid()));CREATE POLICY materials_legacy ON shared_materials FOR SELECT TO authenticated USING((status='ready'OR uploader_id=auth.uid())AND EXISTS(SELECT 1 FROM profiles WHERE id=auth.uid()AND is_approved IS DISTINCT FROM false));CREATE POLICY synthetic_material_visibility ON shared_materials AS RESTRICTIVE FOR SELECT TO authenticated USING(visibility='public'OR uploader_id=auth.uid()OR EXISTS(SELECT 1 FROM note_members WHERE note=id AND member=auth.uid()));CREATE SCHEMA ai_store;CREATE TABLE ai_store.jobs(id text PRIMARY KEY,owner text,kind text,state text,input jsonb);");
    let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let app_sql = std::fs::read_to_string(
        repo.join("supabase/migrations/20261116142026_ai_corpus_policy_reader.sql"),
    )
    .unwrap();
    for _ in 0..2 {
        db.sql(&app_sql);
        db.sql(include_str!("../../cbgpu-ai-corpus/migrations/runtime.sql"));
    }
    let options = PgConnectOptions::new()
        .host(socket.path().to_str().unwrap())
        .username("postgres")
        .database("postgres");
    let admin = PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options.clone())
        .await
        .unwrap();
    let policy = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(|c, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE cbgpu_corpus_backend")
                    .execute(c)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .unwrap();
    db.sql(&format!("INSERT INTO auth.users(id)VALUES('{OWNER}'),('{OTHER}');INSERT INTO profiles VALUES('{OWNER}',true,'ACTIVE'),('{OTHER}',true,'ACTIVE');INSERT INTO shared_materials(id,uploader_id,title,file_name,size_bytes,status,visibility)VALUES('{NOTE}','{OWNER}','Owner title','owner.md',0,'ready','public'),('{PRIVATE}','{OTHER}','Private title','private.md',0,'ready','private'),('{MEMBER}','{OWNER}','Member title','member.md',0,'ready','private'),('{MATERIAL}','{OWNER}','Material title','fixture.md',0,'ready','private');INSERT INTO note_members VALUES('{MEMBER}','{OTHER}');INSERT INTO preprints VALUES('{PAPER}','published',1,'Public title','Public abstract');INSERT INTO preprint_versions VALUES('{PAPER}',1,20,'fixture-hash');"));
    for (id, owner) in [(JOB, OWNER), (OTHER_JOB, OTHER)] {
        sqlx::query("INSERT INTO ai_store.jobs VALUES($1,$2,'search','running',$3)")
            .bind(id)
            .bind(owner)
            .bind(input())
            .execute(&admin)
            .await
            .unwrap();
    }
    let corpus = Corpus::new(admin.clone(), policy.clone());
    let sources = corpus.authorized_sources(JOB).await.unwrap();
    assert!(sources.iter().any(|s| s.id.to_string() == NOTE));
    assert!(sources.iter().any(|s| s.id.to_string() == MATERIAL));
    assert!(!sources.iter().any(|s| s.id.to_string() == PRIVATE));
    assert!(sources.iter().any(|s| s.id.to_string() == PAPER));
    let files = tempfile::tempdir().unwrap();
    for (id, text) in [
        (NOTE, "owner needle"),
        (MEMBER, "member needle"),
        (MATERIAL, "material needle"),
    ] {
        let hex = uuid::Uuid::parse_str(id).unwrap().simple().to_string();
        std::fs::create_dir_all(files.path().join(&hex[..2])).unwrap();
        std::fs::write(files.path().join(&hex[..2]).join(&hex), text).unwrap();
        sqlx::query("UPDATE shared_materials SET size_bytes=$1 WHERE id=$2::uuid")
            .bind(text.len() as i64)
            .bind(id)
            .execute(&admin)
            .await
            .unwrap();
    }
    let paths = SourceFiles::new(
        files.path().into(),
        "http://127.0.0.1:1",
        std::path::PathBuf::from("/usr/bin/pdftotext"),
    )
    .unwrap();

    let snapshot = corpus.trusted_job(JOB).await.unwrap();
    let result = cbgpu_ai_search::search(&snapshot, &corpus, &paths, &Toy)
        .await
        .unwrap();
    let output = result.to_string();
    assert!(output.contains("Owner title") && output.contains("Material title"));
    assert!(
        !output.contains("forged label")
            && !output.contains(PRIVATE)
            && !output.contains("evil.example")
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*)FROM ai_corpus.chunks")
        .fetch_one(&admin)
        .await
        .unwrap();
    assert!(count > 0, "full text was not lazily indexed");
    // The worker never accepts changed disk/request input after ownership validation.
    db.sql(&format!(
        "UPDATE ai_store.jobs SET input=jsonb_set(input,'{{query}}','\"changed\"')WHERE id='{JOB}';"
    ));
    assert!(
        cbgpu_ai_search::search(&snapshot, &corpus, &paths, &Toy)
            .await
            .is_err()
    );
    sqlx::query("UPDATE ai_store.jobs SET input=$1 WHERE id=$2")
        .bind(input())
        .bind(JOB)
        .execute(&admin)
        .await
        .unwrap();
    struct Revoke(PgPool);
    impl Embedder for Revoke {
        fn embed(&self, texts: &[String], query: bool) -> anyhow::Result<Vec<Vec<f32>>> {
            if query {
                let pool = self.0.clone();
                std::thread::spawn(move || {
                    tokio::runtime::Runtime::new()
                        .unwrap()
                        .block_on(async move {
                            sqlx::query(
                                "UPDATE shared_materials SET visibility='private'WHERE id=$1::uuid",
                            )
                            .bind(NOTE)
                            .execute(&pool)
                            .await
                            .unwrap();
                            sqlx::query("DELETE FROM note_members WHERE member=$1::uuid")
                                .bind(OTHER)
                                .execute(&pool)
                                .await
                                .unwrap();
                        })
                })
                .join()
                .unwrap();
            }
            Toy.embed(texts, query)
        }
    }
    let other = corpus.trusted_job(OTHER_JOB).await.unwrap();
    let result = cbgpu_ai_search::search(&other, &corpus, &paths, &Revoke(admin.clone()))
        .await
        .unwrap();
    let output = result.to_string();
    assert!(
        !output.contains(NOTE) && !output.contains(MEMBER),
        "revoked metadata escaped final source filter"
    );
    db.sql(&format!(
        "UPDATE ai_store.jobs SET owner='{OTHER}'WHERE id='{JOB}';"
    ));
    assert!(
        cbgpu_ai_search::search(&snapshot, &corpus, &paths, &Toy)
            .await
            .is_err()
    );
    policy.close().await;
    admin.close().await;
}
