//! Disposable database only: never point this test at operational state.
use cbgpu_deploy_control::{crypto::Cipher, store::Store};
use serde_json::json;
#[tokio::test]
async fn ownership_encrypted_queue_failure_recovery_and_project_continuity() {
    let Ok(url) = std::env::var("CBGPU_DEPLOY_CONTROL_TEST_DATABASE_URL") else {
        return;
    };
    assert!(url.ends_with("/cbgpu_deploy_control_test"));
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let store = Store {
        pool: pool.clone(),
        cipher: Cipher::new(&[7; 32]).unwrap(),
        data: temp.path().into(),
        free_floor: 0,
    };
    store.migrate().await.unwrap();
    store.migrate().await.unwrap();
    sqlx::query("SET ROLE cbgpu_deploy_control")
        .execute(&pool)
        .await
        .unwrap();
    let p=store.create("owner",&json!({"slug":"test-owner","repository":"https://github.com/example/repo","env":{"SECRET":"never-public"}})).await.unwrap();
    assert!(store.find(&p.id, "other").await.is_err());
    let output = store.output(&p).await.unwrap();
    assert!(!output.to_string().contains("never-public"));
    assert_eq!(
        store.cipher.unseal(&p.secrets).unwrap()["env"]["SECRET"],
        "never-public"
    );
    let first = store.schedule(&p, "owner").await.unwrap();
    assert!(store.schedule(&p, "owner").await.is_err());
    let work = store.claim(&["front_build".into()]).await.unwrap().unwrap();
    assert_eq!(work.deployment.as_deref(), Some(first.as_str()));
    assert!(!work.payload.to_string().contains("never-public"));
    store
        .finish_work(&work, Some(&json!({"kind":"static","commit":"abc"})), None)
        .await
        .unwrap();
    let p = store.find(&p.id, "owner").await.unwrap();
    assert_eq!(p.production.as_deref(), Some(first.as_str()));
    store.schedule(&p, "owner").await.unwrap();
    let failed = store.claim(&["front_build".into()]).await.unwrap().unwrap();
    store
        .finish_work(&failed, None, Some("synthetic failed build"))
        .await
        .unwrap();
    assert_eq!(
        store.find(&p.id, "owner").await.unwrap().production,
        p.production
    );
    store.schedule(&p, "owner").await.unwrap();
    store.claim(&["front_build".into()]).await.unwrap().unwrap();
    assert_eq!(store.recover_interrupted().await.unwrap().len(), 1);
    assert_eq!(
        store.find(&p.id, "owner").await.unwrap().production,
        p.production
    );
    store.delete(&p).await.unwrap();
    assert!(store.find(&p.id, "owner").await.is_err());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM deploy_store.deployments WHERE project_id=$1")
            .bind(&p.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 3);
    // A synthetic legacy snapshot is imported byte-for-byte without rewriting ciphertext or IDs.
    for table in [
        "work_items",
        "deployments",
        "custom_domains",
        "project_members",
        "server_domains",
        "compose_states",
        "projects",
    ] {
        sqlx::query(&format!("DELETE FROM deploy_store.{table}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    let source = temp.path().join("legacy.sqlite");
    let sqlite = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&source)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TABLE projects(id TEXT PRIMARY KEY,owner TEXT,slug TEXT,config TEXT,secrets TEXT,production TEXT,created_at TEXT,deleted INTEGER)").execute(&sqlite).await.unwrap();
    sqlx::query("CREATE TABLE deployments(id TEXT PRIMARY KEY,project_id TEXT,status TEXT,logs TEXT,result TEXT,error TEXT,created_at TEXT,finished_at TEXT,requested_by TEXT)").execute(&sqlite).await.unwrap();
    let cipher = store
        .cipher
        .seal(&json!({"git_token":"legacy-private","env":{"TOKEN":"preserved"}}))
        .unwrap();
    sqlx::query("INSERT INTO projects VALUES('legacy-project','legacy-owner','legacy-slug',?,?,'legacy-release','2026-01-01T00:00:00Z',0)").bind(json!({"repository":"https://github.com/example/repo"}).to_string()).bind(&cipher).execute(&sqlite).await.unwrap();
    sqlx::query("INSERT INTO deployments VALUES('legacy-release','legacy-project','ready','retained logs',?,NULL,'2026-01-01T00:00:00Z','2026-01-01T00:00:01Z','legacy-owner')").bind(json!({"kind":"static","commit":"unchanged"}).to_string()).execute(&sqlite).await.unwrap();
    sqlx::query("CREATE TABLE compose_states(project_id TEXT PRIMARY KEY,status TEXT,updated_at TEXT,access_state TEXT)").execute(&sqlite).await.unwrap();
    sqlx::query(
        "INSERT INTO compose_states VALUES('legacy-server','ready','2026-01-01T00:00:00Z','DENY')",
    )
    .execute(&sqlite)
    .await
    .unwrap();
    sqlite.close().await;
    let original = std::fs::read(&source).unwrap();
    let inspection = cbgpu_deploy_control::importer::inspect_read_only(&source)
        .await
        .unwrap();
    assert_eq!(inspection["quiescent"], true);
    assert_eq!(inspection["counts"]["projects"], 1);
    cbgpu_deploy_control::importer::import_read_only(&store, &source)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&source).unwrap(), original);
    let actor: Option<String> = sqlx::query_scalar(
        "SELECT requested_by FROM deploy_store.deployments WHERE id='legacy-release'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actor.as_deref(), Some("legacy-owner"));
    let access: Option<String> = sqlx::query_scalar(
        "SELECT access_state FROM deploy_store.compose_states WHERE project_id='legacy-server'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(access.as_deref(), Some("DENY"));
    let preserved: String =
        sqlx::query_scalar("SELECT secrets FROM deploy_store.projects WHERE id='legacy-project'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(preserved, cipher);
    assert!(
        cbgpu_deploy_control::importer::import_read_only(&store, &source)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&source).unwrap(), original);
    let imported_fingerprint = cbgpu_deploy_control::importer::target_fingerprint(&store)
        .await
        .unwrap();
    // Exercise the actual CLI's source-fence and reverse-state refusal paths.
    let key_file = temp.path().join("fixture-key");
    std::fs::write(&key_file, [7u8; 32]).unwrap();
    let unfenced = candidate_command(
        &url,
        temp.path(),
        &key_file,
        &["--import-snapshot", source.to_str().unwrap()],
        &[],
    )
    .await;
    assert!(!unfenced.status.success());
    assert!(
        String::from_utf8_lossy(&unfenced.stderr).contains("writer fence confirmation required")
    );
    let busy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let refused = candidate_command(
        &url,
        temp.path(),
        &key_file,
        &["--import-snapshot", source.to_str().unwrap()],
        &[
            ("CBGPU_DEPLOY_WRITERS_FENCED", "1".into()),
            (
                "CBGPU_DEPLOY_LEGACY_LISTEN",
                busy.local_addr().unwrap().to_string(),
            ),
        ],
    )
    .await;
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("Legacy listener still active"));
    drop(busy);
    std::fs::write(
        temp.path().join(".native-import-receipt.json"),
        serde_json::to_vec(&json!({"fingerprint":imported_fingerprint})).unwrap(),
    )
    .unwrap();
    let unchanged = candidate_command(
        &url,
        temp.path(),
        &key_file,
        &["--rollback-check"],
        &[("CBGPU_DEPLOY_WRITERS_FENCED", "1".into())],
    )
    .await;
    assert!(
        unchanged.status.success(),
        "{}",
        String::from_utf8_lossy(&unchanged.stderr)
    );
    let active_source = temp.path().join("active.sqlite");
    std::fs::copy(&source, &active_source).unwrap();
    let active = sqlx::SqlitePool::connect_with(
        sqlx::sqlite::SqliteConnectOptions::new().filename(&active_source),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE deployments SET status='queued'")
        .execute(&active)
        .await
        .unwrap();
    active.close().await;
    assert_eq!(
        cbgpu_deploy_control::importer::inspect_read_only(&active_source)
            .await
            .unwrap()["quiescent"],
        false
    );
    let drain = cbgpu_deploy_control::importer::import_read_only(&store, &active_source)
        .await
        .unwrap_err();
    assert!(drain.message.contains("Drain and stop"));
    assert_eq!(
        cbgpu_deploy_control::importer::target_fingerprint(&store)
            .await
            .unwrap(),
        imported_fingerprint
    );
    // The private HTTP boundary must not treat a forged resource-owner header as authority.
    let api = std::sync::Arc::new(cbgpu_deploy_control::api::Api {
        store: store.clone(),
        authority: std::sync::Arc::new(FakeAuthority),
        workers_enabled: Default::default(),
    });
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let owner = "10000000-0000-4000-8000-000000000001";
    let own = store
        .create(
            owner,
            &json!({"slug":"http-contract","repository":"https://github.com/example/repo"}),
        )
        .await
        .unwrap();
    assert_ne!(
        imported_fingerprint,
        cbgpu_deploy_control::importer::target_fingerprint(&store)
            .await
            .unwrap()
    );
    let stale = candidate_command(
        &url,
        temp.path(),
        &key_file,
        &["--rollback-check"],
        &[("CBGPU_DEPLOY_WRITERS_FENCED", "1".into())],
    )
    .await;
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("Native writes detected"));
    let app = cbgpu_deploy_control::api::router(api.clone());
    let denied = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    let req = Request::builder()
        .uri(format!("/projects/{}", own.id))
        .header("x-cbgpu-deploy-key", "07".repeat(32))
        .header("x-cbgpu-owner", "20000000-0000-4000-8000-000000000002")
        .header("x-cbgpu-resource-owner", owner)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.clone().oneshot(req).await.unwrap().status(), 404);
    let req = Request::builder()
        .uri(format!("/projects/{}", own.id))
        .header("x-cbgpu-deploy-key", "07".repeat(32))
        .header("x-cbgpu-owner", owner)
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.oneshot(req).await.unwrap().status(), 200);
    // Compile a genuine native fixture; never launch Git, a Docker daemon or customer code.
    let deployment = store.schedule(&own, owner).await.unwrap();
    let release = temp.path().join(&deployment);
    std::fs::create_dir_all(release.join("public")).unwrap();
    std::fs::write(release.join("public/index.html"), "native fixture").unwrap();
    let result = json!({"kind":"static","directory":release,"public":release.join("public"),"spa":true,"commit":"fixture"});
    let source = temp.path().join("worker.rs");
    let executable = temp.path().join("native-worker");
    let output = format!("CBGPU_RESULT {}", result);
    std::fs::write(&source,format!("use std::io::Read;fn main(){{let mut input=String::new();std::io::stdin().read_to_string(&mut input).unwrap();assert!(input.contains({:?}));assert!(std::env::var(\"SERVICE_ROLE_KEY\").is_err());eprintln!(\"CBGPU_RESULT forged-stderr\");println!(\"{{}}\",{:?});}}",deployment,output)).unwrap();
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| {
        "/home/cbgpu/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc".into()
    });
    assert!(
        std::process::Command::new(&rustc)
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .status()
            .unwrap()
            .success()
    );
    let compose_id = "30000000-0000-4000-8000-000000000003";
    let compose_base = temp.path().join("compose");
    std::fs::create_dir(&compose_base).unwrap();
    let managed = compose_base
        .join(compose_id)
        .join("release-fixture/compose-managed.json");
    std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    std::fs::write(&managed, "{}").unwrap();
    let persistent = compose_base.join(compose_id).join("persistent-data");
    std::fs::write(&persistent, "must survive failed deployments").unwrap();
    let attempt = json!({"namespace":format!("cbgpu-compose-{compose_id}"),"file":managed,"services":["rest","db"],"storage_gb":40});
    let compose_source = temp.path().join("compose.rs");
    let compose_exe = temp.path().join("compose-worker");
    let attempt_line = format!("CBGPU_ATTEMPT {attempt}");
    let diagnostic_line =
        "CBGPU_DIAGNOSTICS {\"failed_services\":[\"rest\"],\"output\":\"fixture-private-token\"}";
    std::fs::write(&compose_source,format!("use std::io::Read;fn main(){{let mut input=String::new();std::io::stdin().read_to_string(&mut input).unwrap();assert!(input.contains(\"fixture-private-token\"));assert!(std::env::var(\"SERVICE_ROLE_KEY\").is_err());println!(\"{{}}\",{:?});println!(\"{{}}\",{:?});println!(\"fixture-private-token unhealthy rest\");std::process::exit(1);}}",attempt_line,diagnostic_line)).unwrap();
    assert!(
        std::process::Command::new(&rustc)
            .arg(&compose_source)
            .arg("-o")
            .arg(&compose_exe)
            .status()
            .unwrap()
            .success()
    );
    let native = cbgpu_deploy_control::worker::Native {
        front: executable.clone(),
        compose: compose_exe,
        domains: executable,
        backend_root: temp.path().into(),
        compose_base: compose_base.clone(),
        home: temp.path().into(),
        tool_dir: Some(temp.path().join("tools")),
        docker_socket: temp.path().join("unused.sock"),
        log_proxies: Default::default(),
        last_measure: Default::default(),
    };
    native.validate().unwrap();
    std::fs::create_dir(temp.path().join("tools")).unwrap();
    let git_source = temp.path().join("git.rs");
    std::fs::write(&git_source,"fn main(){assert!(std::env::var(\"SERVICE_ROLE_KEY\").is_err());assert_eq!(std::env::var(\"GIT_TERMINAL_PROMPT\").unwrap(),\"0\");assert_eq!(std::env::var(\"GIT_CONFIG_VALUE_1\").unwrap(),\"false\");assert!(std::env::args().any(|a|a==\"refs/heads/main\"));println!(\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa refs/heads/main\");}").unwrap();
    assert!(
        std::process::Command::new(&rustc)
            .arg(&git_source)
            .arg("-o")
            .arg(temp.path().join("tools/git"))
            .status()
            .unwrap()
            .success()
    );

    let controller = cbgpu_deploy_control::worker::Controller {
        api: api.clone(),
        executor: std::sync::Arc::new(native),
    };
    assert!(controller.step(&["front_build".into()]).await.unwrap());
    assert_eq!(
        store
            .find(&own.id, owner)
            .await
            .unwrap()
            .production
            .as_deref(),
        Some(deployment.as_str())
    );
    assert_eq!(controller.auto_poll().await.unwrap(), 1);
    assert_eq!(controller.auto_poll().await.unwrap(), 0); // One durable active build, never duplicates.
    let polled = store.claim(&["front_build".into()]).await.unwrap().unwrap();
    store
        .finish_work(&polled, None, Some("Synthetic auto-deploy failure"))
        .await
        .unwrap();
    let compose_project = json!({"id":compose_id,"user_id":owner,"status":"approved","deploy_requested_at":"2026-11-17T00:00:00Z","env_vars":{"CBGPU_DEPLOY_ENGINE":"compose","CBGPU_STORAGE_GB":"40","PRIVATE_TOKEN":"fixture-private-token"}});
    assert!(store.enqueue_compose(&compose_project).await.unwrap());
    assert!(!store.enqueue_compose(&compose_project).await.unwrap());
    assert!(controller.step(&["compose_build".into()]).await.unwrap());
    let state: serde_json::Value = sqlx::query_scalar(
        "SELECT to_jsonb(s)FROM deploy_store.compose_states s WHERE project_id=$1",
    )
    .bind(compose_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state["status"], "error");
    assert_eq!(state["diagnostics"]["failed_services"][0], "rest");
    assert_eq!(state["attempt_result"], attempt);
    assert!(!state.to_string().contains("fixture-private-token"));
    assert!(persistent.exists());
    let mut retry = compose_project.clone();
    retry["deploy_requested_at"] = json!("2026-11-17T00:01:00Z");
    assert!(store.enqueue_compose(&retry).await.unwrap());
    assert!(controller.step(&["compose_build".into()]).await.unwrap());
    assert!(persistent.exists());
    let public = cbgpu_deploy_control::public::router(cbgpu_deploy_control::public::Public {
        api: api.clone(),
        listen: "127.0.0.1:13581".parse().unwrap(),
    });
    let response = public
        .clone()
        .oneshot(
            Request::builder()
                .uri("/client/route")
                .header("host", "http-contract.front.cbgpu.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 1000)
            .await
            .unwrap(),
        "native fixture"
    );
    let response = public
        .oneshot(
            Request::builder()
                .uri("/projects")
                .header("host", "deploy.internal")
                .header("x-cbgpu-deploy-key", "07".repeat(32))
                .header("x-cbgpu-owner", owner)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    // A real HTTP/upgrade socket exercises the same public router and cookie boundary.
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream.local_addr().unwrap().port();
    let daemon = tokio::spawn(async move {
        loop {
            let (mut socket, _) = upstream.accept().await.unwrap();
            tokio::spawn(async move {
                let header = read_headers(&mut socket).await;
                assert!(!header.contains("sb-cbgpu-auth-token"));
                assert!(!header.contains("cbgpu-shared-session-state"));
                assert!(header.contains("sb-tenant-auth-token=keep"));
                assert!(header.contains("Bearer unrelated-tenant"));
                if header.to_ascii_lowercase().contains("upgrade: websocket") {
                    socket.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: websocket\r\nSet-Cookie: sb-cbgpu-auth-token=poison; Domain=.cbgpu.com\r\nSet-Cookie: sb-tenant-auth-token=valid; Path=/\r\n\r\n").await.unwrap();
                    let mut ping = [0; 4];
                    socket.read_exact(&mut ping).await.unwrap();
                    assert_eq!(&ping, b"ping");
                    socket.write_all(b"pong").await.unwrap();
                } else {
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nSet-Cookie: other=bad; Domain=.cbgpu.com\r\nSet-Cookie: tenant=good; Path=/\r\nConnection: close\r\n\r\nlive").await.unwrap();
                }
            });
        }
    });
    sqlx::query("UPDATE deploy_store.deployments SET result=$1 WHERE id=$2")
        .bind(json!({"kind":"next","port":upstream_port}))
        .bind(&deployment)
        .execute(&pool)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = cbgpu_deploy_control::public::combined(api.clone(), address);
    let gateway = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let response=reqwest::Client::new().get(format!("http://{address}/echo")).header("host","http-contract.front.cbgpu.com").header("authorization","Bearer unrelated-tenant").header("cookie","sb-cbgpu-auth-token=secret; cbgpu-shared-session-state=active; sb-tenant-auth-token=keep").send().await.unwrap();
    assert_eq!(response.status(), 200);
    let cookies: Vec<_> = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|h| h.to_str().unwrap().to_owned())
        .collect();
    assert_eq!(cookies, vec!["tenant=good; Path=/"]);
    assert_eq!(response.text().await.unwrap(), "live");
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(b"GET /socket HTTP/1.1\r\nHost: http-contract.front.cbgpu.com\r\nConnection: upgrade\r\nUpgrade: websocket\r\nAuthorization: Bearer unrelated-tenant\r\nCookie: sb-cbgpu-auth-token=secret; cbgpu-shared-session-state=active; sb-tenant-auth-token=keep\r\n\r\n").await.unwrap();
    let header = read_headers(&mut socket).await;
    assert!(header.starts_with("HTTP/1.1 101"));
    assert!(!header.contains("poison"));
    assert!(header.contains("sb-tenant-auth-token=valid"));
    socket.write_all(b"ping").await.unwrap();
    let mut pong = [0; 4];
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        socket.read_exact(&mut pong),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&pong, b"pong");
    // Scoped compose requests retain the verified editor at queue/claim time.
    let editor = "10000000-0000-4000-8000-000000000099";
    for case in 0..4 {
        sqlx::query("DELETE FROM deploy_store.project_members WHERE resource_type='server'AND project_id=$1").bind(compose_id).execute(&pool).await.unwrap();
        if case != 0 {
            sqlx::query("INSERT INTO deploy_store.project_members(resource_type,project_id,user_id,role,email,name,created_at)VALUES('server',$1,$2,'editor','','','fixture')").bind(compose_id).bind(editor).execute(&pool).await.unwrap();
        }
        let mut request = compose_project.clone();
        request["deploy_requested_at"] = json!(format!("2027-01-0{}T00:00:00Z", case + 1));
        if case != 2 {
            request["deploy_requested_by"] = json!(editor);
        }
        let mut current = request.clone();
        if case == 3 {
            current["deploy_requested_by"] = json!(owner);
        }
        assert!(store.enqueue_compose(&request).await.unwrap());
        let payload:serde_json::Value=sqlx::query_scalar("SELECT payload FROM deploy_store.work_items WHERE project_id=$1 AND state='queued'AND kind='compose_build'").bind(compose_id).fetch_one(&pool).await.unwrap();
        assert_eq!(payload["requested_by"], request["deploy_requested_by"]);
        let strict = std::sync::Arc::new(cbgpu_deploy_control::api::Api {
            store: store.clone(),
            workers_enabled: std::sync::atomic::AtomicBool::new(true),
            authority: std::sync::Arc::new(ScopedComposeAuthority {
                project: current,
                denied_actor: if case == 1 { Some(editor.into()) } else { None },
            }),
        });
        let guarded = cbgpu_deploy_control::worker::Controller {
            api: strict,
            executor: controller.executor.clone(),
        };
        assert!(guarded.step(&["compose_build".into()]).await.unwrap());
        let result: serde_json::Value = sqlx::query_scalar(
            "SELECT to_jsonb(s) FROM deploy_store.compose_states s WHERE project_id=$1",
        )
        .bind(compose_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(result["status"], "error");
        assert!(
            result["diagnostics"].is_null(),
            "revoked/missing/changed actor must never launch customer worker"
        );
        assert!(result["attempt_result"].is_null());
    }
    gateway.abort();
    daemon.abort();
    pool.close().await;
}

struct FakeAuthority;
#[async_trait::async_trait]
impl cbgpu_deploy_control::authority::Authority for FakeAuthority {
    async fn identity(&self, _user: &str) -> cbgpu_deploy_control::Result<()> {
        Ok(())
    }
    async fn server_project(&self, id: &str) -> cbgpu_deploy_control::Result<serde_json::Value> {
        Ok(json!({"id":id,"status":"approved","user_id":"10000000-0000-4000-8000-000000000001"}))
    }
    async fn mark_deployed(
        &self,
        _id: &str,
        _time: Option<&str>,
    ) -> cbgpu_deploy_control::Result<()> {
        Ok(())
    }
    async fn can_execute(&self, _user: &str) -> cbgpu_deploy_control::Result<bool> {
        Ok(true)
    }
    async fn server_owner(&self, _id: &str) -> cbgpu_deploy_control::Result<String> {
        Ok("10000000-0000-4000-8000-000000000001".into())
    }
    async fn txt_ownership(
        &self,
        _host: &str,
        _expected: &str,
    ) -> cbgpu_deploy_control::Result<bool> {
        Ok(false)
    }
}

async fn read_headers(socket: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    loop {
        let mut next = [0u8; 1];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            socket.read_exact(&mut next),
        )
        .await
        .unwrap()
        .unwrap();
        bytes.push(next[0]);
        assert!(bytes.len() < 32768);
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(bytes).unwrap()
}

struct ScopedComposeAuthority {
    project: serde_json::Value,
    denied_actor: Option<String>,
}
#[async_trait::async_trait]
impl cbgpu_deploy_control::authority::Authority for ScopedComposeAuthority {
    fn requires_request_actor(&self) -> bool {
        true
    }
    async fn identity(&self, _: &str) -> cbgpu_deploy_control::Result<()> {
        Ok(())
    }
    async fn can_execute(&self, user: &str) -> cbgpu_deploy_control::Result<bool> {
        Ok(self.denied_actor.as_deref() != Some(user))
    }
    async fn server_owner(&self, _: &str) -> cbgpu_deploy_control::Result<String> {
        Ok(self.project["user_id"].as_str().unwrap().into())
    }
    async fn server_project(&self, _: &str) -> cbgpu_deploy_control::Result<serde_json::Value> {
        Ok(self.project.clone())
    }
    async fn mark_deployed(&self, _: &str, _: Option<&str>) -> cbgpu_deploy_control::Result<()> {
        Ok(())
    }
    async fn txt_ownership(&self, _: &str, _: &str) -> cbgpu_deploy_control::Result<bool> {
        Ok(false)
    }
}

async fn candidate_command(
    url: &str,
    data: &std::path::Path,
    key: &std::path::Path,
    args: &[&str],
    extra: &[(&str, String)],
) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_cbgpu-deploy-control"));
    command
        .kill_on_drop(true)
        .env_clear()
        .env("CBGPU_DEPLOY_CONTROL_ENABLED", "1")
        .env("CBGPU_DEPLOY_DATABASE_URL", url)
        .env("CBGPU_DEPLOY_DATA", data)
        .env("CBGPU_DEPLOY_KEY_FILE", key)
        .env("CBGPU_DEPLOY_LISTEN", "127.0.0.1:0")
        .args(args);
    for (key, value) in extra {
        command.env(key, value);
    }
    tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .unwrap()
        .unwrap()
}
