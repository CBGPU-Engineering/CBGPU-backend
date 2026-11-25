use cbgpu_ai_store::{
    Limits, Store, conversations::SendRequest, hash_key, importer::import_read_only,
};
use serde_json::json;
use std::io::Read;
const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";
#[tokio::test]
#[ignore = "Requires explicit disposable CBGPU_AI_TEST_DATABASE_URL ending /cbgpu_ai_store_test"]
async fn postgres_store_preserves_import_ownership_queue_history_files_and_keys() {
    let url = std::env::var("CBGPU_AI_TEST_DATABASE_URL").unwrap();
    assert!(
        url.ends_with("/cbgpu_ai_store_test"),
        "Refuse non-test database"
    );
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("state");
    let mut store = Store::new(
        pool,
        root.clone(),
        Some(temp.path().join("bulk")),
        Limits {
            free_floor: 0,
            ..Limits::default()
        },
    )
    .unwrap();
    store.migrate().await.unwrap();
    let runtime = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET ROLE cbgpu_ai").execute(connection).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    store.pool.close().await;
    store.pool = runtime;
    let role: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(role, "cbgpu_ai");
    assert!(
        sqlx::query("SELECT * FROM pg_authid")
            .fetch_all(&store.pool)
            .await
            .is_err(),
        "runtime cannot read database credential hashes"
    );

    let source = temp.path().join("legacy.sqlite");
    let legacy = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&source)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE jobs(id TEXT PRIMARY KEY,owner TEXT,kind TEXT,title TEXT,state TEXT,created REAL,updated REAL);CREATE TABLE api_keys(id TEXT,owner TEXT,name TEXT,prefix TEXT,hash TEXT,created REAL);CREATE TABLE conversations(id TEXT,owner TEXT,title TEXT,created REAL,updated REAL);").execute(&legacy).await.unwrap();
    let oldjob = "10000000-0000-4000-8000-000000000001";
    sqlx::query("INSERT INTO jobs VALUES(?,?,'chat','Preserved','failed',1.,2.)")
        .bind(oldjob)
        .bind(OWNER)
        .execute(&legacy)
        .await
        .unwrap();
    sqlx::query("INSERT INTO api_keys VALUES('20000000-0000-4000-8000-000000000001',?,'Old key','cbgpu_ai_old',?,1.)").bind(OWNER).bind(hash_key("cbgpu_ai_legacy-secret")).execute(&legacy).await.unwrap();
    sqlx::query("INSERT INTO conversations VALUES('30000000-0000-4000-8000-000000000001',?,'Retained chat',1.,2.)").bind(OWNER).execute(&legacy).await.unwrap();
    legacy.close().await;
    let before = std::fs::read(&source).unwrap();
    let imported = import_read_only(&store, &source).await.unwrap();
    assert_eq!(imported["counts"]["jobs"], 1);
    assert_eq!(
        before,
        std::fs::read(&source).unwrap(),
        "SQLite source unchanged"
    );
    assert_eq!(
        store.key_owner("cbgpu_ai_legacy-secret").await.unwrap(),
        OWNER
    );
    assert!(
        import_read_only(&store, &source).await.is_err(),
        "repeat import refuses overwrite"
    );
    assert_eq!(
        store.job(OWNER, oldjob, false).await.unwrap()["title"],
        "Preserved"
    );
    assert!(store.job(OTHER, oldjob, false).await.is_err());
    let key = store.create_key(OWNER, "Native").await.unwrap();
    for index in 0..5 {
        store
            .create_key(OWNER, &format!("Extra {index}"))
            .await
            .unwrap();
    }
    assert_eq!(store.keys(OWNER).await.unwrap().len(), 7);
    assert_eq!(
        store.key_owner(key["key"].as_str().unwrap()).await.unwrap(),
        OWNER
    );
    let public = store.keys(OWNER).await.unwrap();
    assert!(
        public
            .iter()
            .all(|k| k.get("hash").is_none() && k.get("key").is_none())
    );
    store
        .revoke_key(OWNER, key["id"].as_str().unwrap())
        .await
        .unwrap();
    assert!(store.key_owner(key["key"].as_str().unwrap()).await.is_err());
    let upload = store
        .upload(OWNER, "../report.txt", &b"hello"[..])
        .await
        .unwrap();
    assert_eq!(upload["name"], "report.txt");
    let uploadid = upload["id"].as_str().unwrap();
    assert!(store.get_upload(OTHER, uploadid).await.is_err());
    let project = store.create_project(OWNER, "Research").await.unwrap();
    let projectid = project["id"].as_str().unwrap();
    let chat = store
        .create_conversation(OWNER, Some(projectid), false)
        .await
        .unwrap();
    let chatid = chat["id"].as_str().unwrap();
    assert!(store.conversation(OTHER, chatid).await.is_err());
    let mut request = SendRequest::text("첫 질문");
    request.attachments.push(uploadid.into());
    let sent = store.send_message(OWNER, chatid, request).await.unwrap();
    let job = sent["messages"][1]["job_id"].as_str().unwrap().to_owned();
    assert_eq!(sent["messages"][1]["state"], "queued");
    assert!(
        store.delete_upload(OWNER, uploadid).await.is_err(),
        "project association preserved"
    );
    assert!(
        store
            .send_message(OWNER, chatid, SendRequest::text("duplicate"))
            .await
            .is_err(),
        "no parallel answer in same chat"
    );
    cbgpu_ai_store::atomic_json(
        &root.join("jobs").join(&job).join("partial.json"),
        &json!({"text":"안녕"}),
    )
    .unwrap();
    assert_eq!(
        store.live_conversation(OWNER, chatid).await.unwrap()["message"]["content"],
        "안녕"
    );
    cbgpu_ai_store::atomic_json(
        &root.join("jobs").join(&job).join("result.json"),
        &json!({"choices":[{"message":{"content":"안녕하세요"},"finish_reason":"length"}]}),
    )
    .unwrap();
    sqlx::query("UPDATE ai_store.jobs SET state='succeeded' WHERE id=$1")
        .bind(&job)
        .execute(&store.pool)
        .await
        .unwrap();
    let ready = store.conversation(OWNER, chatid).await.unwrap();
    assert_eq!(ready["messages"][1]["content"], "안녕하세요");
    assert_eq!(ready["messages"][1]["truncated"], true);
    let ticket = store
        .download_ticket(OWNER, &job, "text", "")
        .await
        .unwrap();
    assert!(
        store
            .download(OTHER, ticket["id"].as_str().unwrap())
            .await
            .is_err()
    );
    match store
        .download(OWNER, ticket["id"].as_str().unwrap())
        .await
        .unwrap()
    {
        cbgpu_ai_store::Download::Bytes(bytes) => {
            assert_eq!(String::from_utf8(bytes).unwrap(), "안녕하세요")
        }
        _ => panic!(),
    };
    let workspace = root.join("jobs").join(&job).join("workspace/output");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("safe.txt"), "safe").unwrap();
    let mut descriptor = store.artifact(OWNER, &job, "safe.txt").await.unwrap();
    #[cfg(unix)]
    {
        std::fs::remove_file(workspace.join("safe.txt")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", workspace.join("safe.txt")).unwrap();
        assert!(store.artifact(OWNER, &job, "safe.txt").await.is_err());
        let mut text = String::new();
        descriptor.read_to_string(&mut text).unwrap();
        assert_eq!(
            text, "safe",
            "open file descriptor remains original despite path swap"
        );
    }
    assert!(store.artifact(OWNER, &job, "../../secret").await.is_err());
    store.delete_project(OWNER, projectid).await.unwrap();
    assert!(
        store.conversation(OWNER, chatid).await.unwrap()["project"].is_null(),
        "project deletion retains conversations"
    );
    store.delete_upload(OWNER, uploadid).await.unwrap();
    let mut tasks = vec![];
    for n in 0..7 {
        let s = store.clone();
        tasks.push(tokio::spawn(async move {
            s.enqueue(OTHER, "embed", json!({"input":"text"}), &format!("{n}"))
                .await
        }));
    }
    let mut successes = 0;
    for task in tasks {
        if task.await.unwrap().is_ok() {
            successes += 1;
        }
    }
    assert_eq!(successes, 5, "concurrent owner queue admission is atomic");
    let jobs = store.jobs(OTHER).await.unwrap();
    let first = jobs[0]["id"].as_str().unwrap();
    store.cancel_job(OTHER, first).await.unwrap();
    assert_eq!(
        store.job(OTHER, first, false).await.unwrap()["state"],
        "cancelled"
    );
    store.retry_job(OTHER, first).await.unwrap();
    store.delete_conversation(OWNER, chatid).await.unwrap();
    assert!(store.conversation(OWNER, chatid).await.is_err());
    assert_eq!(
        store.job(OWNER, &job, false).await.unwrap()["state"],
        "deleted"
    );
    let parallel = store.create_conversation(OWNER, None, false).await.unwrap();
    let parallelid = parallel["id"].as_str().unwrap().to_owned();
    let mut senders = vec![];
    for _ in 0..12 {
        let s = store.clone();
        let c = parallelid.clone();
        senders.push(tokio::spawn(async move {
            s.send_message(OWNER, &c, SendRequest::text("serialized"))
                .await
        }));
    }
    let mut accepted = 0;
    for sender in senders {
        if tokio::time::timeout(std::time::Duration::from_secs(10), sender)
            .await
            .expect("owner-lock wait must not starve connection pool")
            .unwrap()
            .is_ok()
        {
            accepted += 1;
        }
    }
    assert_eq!(
        accepted, 1,
        "one live generation per conversation under contention"
    );
    let count_before: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_store.uploads")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    let mut small = store.clone();
    small.limits.upload_bytes = 2;
    assert!(
        small
            .upload(OWNER, "too-large", &b"abcd"[..])
            .await
            .is_err()
    );
    assert!(store.upload(OWNER, "empty", &b""[..]).await.is_err());
    let count_after: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_store.uploads")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(
        count_before, count_after,
        "failed uploads never create rows"
    );
    sqlx::query("INSERT INTO ai_store.vectors(hash,vector,updated)SELECT 'old-'||n,'[1,0]'::jsonb,n::double precision FROM generate_series(1,5001)n").execute(&store.pool).await.unwrap();
    store
        .cache_vector("newest", &json!([0., 1.]))
        .await
        .unwrap();
    let cached: i64 = sqlx::query_scalar("SELECT count(*) FROM ai_store.vectors")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(cached, 5000);
    assert_eq!(
        store.cached_vector("newest").await.unwrap(),
        Some(json!([0., 1.]))
    );
    store.pool.close().await;
}
