//! Entire scheduler drives synthetic PostgreSQL plus fake GPU/runner/permissions.
use cbgpu_ai::{
    Api, Authorization,
    resources::{Card, Process, Resources, Snapshot},
    runner::{Runner, Task},
    scheduler::{Scheduler, Settings},
};
use cbgpu_ai_store::{Limits, Result, Store, fail};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};
const OWNER: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";
struct FakeResources {
    reserved: AtomicBool,
    foreign: AtomicBool,
    failed: AtomicBool,
    free: AtomicU64,
    active: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl Resources for FakeResources {
    async fn snapshot(&self) -> Result<Snapshot> {
        if self.failed.load(Ordering::SeqCst) {
            return Err(fail("Synthetic reservation outage", 503));
        }
        let running = self.active.load(Ordering::SeqCst) > 0;
        let reserved = self.reserved.load(Ordering::SeqCst);
        let foreign = self.foreign.load(Ordering::SeqCst);
        let mut processes = vec![];
        if running {
            processes.push(Process {
                uuid: "GPU-fake".into(),
                pid: 100,
            });
        }
        if foreign {
            processes.push(Process {
                uuid: "GPU-fake".into(),
                pid: 999,
            });
        }
        Ok(Snapshot {
            cards: vec![Card {
                index: 0,
                uuid: "GPU-fake".into(),
                name: "Fake GPU".into(),
                memory: if running { 2048 } else { 0 },
                utilization: 0,
                reserved,
                available: !running && !reserved && !foreign,
            }],
            processes,
        })
    }
    fn free_bytes(&self, _: &Path) -> Result<u64> {
        Ok(self.free.load(Ordering::SeqCst))
    }
}
struct FakeRunner {
    active: Arc<AtomicUsize>,
    complete: Arc<AtomicBool>,
    stopped: Arc<AtomicUsize>,
    recovered: AtomicUsize,
    launched: Mutex<Vec<String>>,
}
struct FakeTask {
    active: Arc<AtomicUsize>,
    complete: Arc<AtomicBool>,
    stopped: Arc<AtomicUsize>,
    finished: bool,
}
#[async_trait::async_trait]
impl Task for FakeTask {
    async fn poll(&mut self) -> Result<Option<i32>> {
        Ok(self.complete.load(Ordering::SeqCst).then_some(0))
    }
    async fn own_pids(&mut self) -> Result<HashSet<u32>> {
        Ok(HashSet::from([100]))
    }
    async fn stop(&mut self) -> Result<()> {
        if !self.finished {
            self.active.fetch_sub(1, Ordering::SeqCst);
            self.stopped.fetch_add(1, Ordering::SeqCst);
            self.finished = true;
        }
        Ok(())
    }
}
#[async_trait::async_trait]
impl Runner for FakeRunner {
    fn attached(&self) -> bool {
        true
    }
    async fn recover_owned(&self) -> Result<()> {
        self.recovered.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn start(&self, store: &Store, job: &Value, _: &Card) -> Result<Box<dyn Task>> {
        assert_eq!(
            self.active.fetch_add(1, Ordering::SeqCst),
            0,
            "one lease per GPU"
        );
        self.launched
            .lock()
            .unwrap()
            .push(job["owner"].as_str().unwrap().into());
        cbgpu_ai_store::atomic_json(
            &store
                .root
                .join("jobs")
                .join(job["id"].as_str().unwrap())
                .join("result.json"),
            &json!({"choices":[{"message":{"content":"Synthetic success"}}]}),
        )?;
        Ok(Box::new(FakeTask {
            active: self.active.clone(),
            complete: self.complete.clone(),
            stopped: self.stopped.clone(),
            finished: false,
        }))
    }
}
async fn wait(store: &Store, owner: &str, id: &str, state: &str) {
    for _ in 0..500 {
        if store.job(owner, id, false).await.unwrap()["state"] == state {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!(
        "Expected {state}, got {}",
        store.job(owner, id, false).await.unwrap()
    );
}
fn input() -> Value {
    json!({"input":"Synthetic embedding"})
}
#[tokio::test]
#[ignore = "Requires disposable CBGPU_AI_TEST_DATABASE_URL ending /cbgpu_ai_scheduler_test"]
async fn real_queue_claim_fairness_preemption_cancel_and_recovery() {
    let url = std::env::var("CBGPU_AI_TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/cbgpu_ai_scheduler_test"));
    let temp = tempfile::tempdir().unwrap();
    let mut store = Store::new(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(8)
            .connect(&url)
            .await
            .unwrap(),
        temp.path().to_owned(),
        None,
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

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/rest/v1/rpc/cbgpu_service_can_use_resource",
                axum::routing::post(|| async { axum::Json(json!(true)) }),
            ),
        )
        .await
        .unwrap();
    });
    let api = Arc::new(Api::new(
        store.clone(),
        b"synthetic-internal-key".to_vec(),
        Authorization {
            backend_url: format!("http://{addr}"),
            service_key: "synthetic".into(),
            account_v2: true,
            client: reqwest::Client::new(),
        },
        "no-real-ocr".into(),
    ));
    let active = Arc::new(AtomicUsize::new(0));
    let resources = Arc::new(FakeResources {
        reserved: AtomicBool::new(false),
        foreign: AtomicBool::new(false),
        failed: AtomicBool::new(false),
        free: AtomicU64::new(1000),
        active: active.clone(),
    });
    let runner = Arc::new(FakeRunner {
        active,
        complete: Arc::new(AtomicBool::new(true)),
        stopped: Arc::new(AtomicUsize::new(0)),
        recovered: AtomicUsize::new(0),
        launched: Mutex::new(vec![]),
    });
    let settings = Settings {
        workers: 4,
        admission_floor: 100,
        running_floor: 10,
        tick: std::time::Duration::from_millis(20),
    };
    let a = store.enqueue(OWNER, "embed", input(), "one").await.unwrap();
    let b = store.enqueue(OWNER, "embed", input(), "two").await.unwrap();
    let c = store
        .enqueue(OTHER, "embed", input(), "three")
        .await
        .unwrap();
    let scheduler = Scheduler::new(
        api.clone(),
        resources.clone(),
        runner.clone(),
        settings.clone(),
    );
    let task = scheduler.clone().start().await.unwrap();
    wait(&store, OWNER, a["id"].as_str().unwrap(), "succeeded").await;
    wait(&store, OWNER, b["id"].as_str().unwrap(), "succeeded").await;
    wait(&store, OTHER, c["id"].as_str().unwrap(), "succeeded").await;
    assert_eq!(
        *runner.launched.lock().unwrap(),
        vec![OWNER, OTHER, OWNER],
        "same-priority owners rotate"
    );
    assert_eq!(runner.recovered.load(Ordering::SeqCst), 1);
    runner.complete.store(false, Ordering::SeqCst);
    let reserved = store
        .enqueue(OWNER, "embed", input(), "preempt")
        .await
        .unwrap();
    let id = reserved["id"].as_str().unwrap();
    wait(&store, OWNER, id, "running").await;
    resources.reserved.store(true, Ordering::SeqCst);
    wait(&store, OWNER, id, "paused").await;
    assert!(
        store
            .root
            .join("jobs")
            .join(id)
            .join("input.json")
            .is_file(),
        "pause keeps input/checkpoint"
    );
    store.cancel_job(OWNER, id).await.unwrap();
    resources.reserved.store(false, Ordering::SeqCst);
    let foreign = store
        .enqueue(OWNER, "embed", input(), "foreign")
        .await
        .unwrap();
    let id = foreign["id"].as_str().unwrap();
    wait(&store, OWNER, id, "running").await;
    resources.foreign.store(true, Ordering::SeqCst);
    wait(&store, OWNER, id, "paused").await;
    store.cancel_job(OWNER, id).await.unwrap();
    resources.foreign.store(false, Ordering::SeqCst);
    let outage = store
        .enqueue(OWNER, "embed", input(), "outage")
        .await
        .unwrap();
    let outageid = outage["id"].as_str().unwrap();
    wait(&store, OWNER, outageid, "running").await;
    resources.failed.store(true, Ordering::SeqCst);
    wait(&store, OWNER, outageid, "paused").await;
    store.cancel_job(OWNER, outageid).await.unwrap();
    resources.failed.store(false, Ordering::SeqCst);
    let cancelled = store
        .enqueue(OWNER, "embed", input(), "cancel")
        .await
        .unwrap();
    let id = cancelled["id"].as_str().unwrap();
    wait(&store, OWNER, id, "running").await;
    store.cancel_job(OWNER, id).await.unwrap();
    for _ in 0..300 {
        if !api.running.read().await.contains(id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!api.running.read().await.contains(id));
    assert_eq!(
        store.job(OWNER, id, false).await.unwrap()["state"],
        "cancelled"
    );
    resources.free.store(1, Ordering::SeqCst);
    let disk = store
        .enqueue(OWNER, "embed", input(), "disk")
        .await
        .unwrap();
    wait(&store, OWNER, disk["id"].as_str().unwrap(), "failed").await;
    resources.free.store(1000, Ordering::SeqCst);
    scheduler.close();
    task.await.unwrap();
    let recovery = store
        .enqueue(OWNER, "embed", input(), "restart")
        .await
        .unwrap();
    let id = recovery["id"].as_str().unwrap();
    sqlx::query("UPDATE ai_store.jobs SET state='running',crashes=2 WHERE id=$1")
        .bind(id)
        .execute(&store.pool)
        .await
        .unwrap();
    let restarted = Scheduler::new(api, resources, runner.clone(), settings);
    let task = restarted.clone().start().await.unwrap();
    wait(&store, OWNER, id, "failed").await;
    assert_eq!(store.job(OWNER, id, false).await.unwrap()["crashes"], 3);
    restarted.close();
    task.await.unwrap();
    assert_eq!(runner.active.load(Ordering::SeqCst), 0);
    assert!(runner.stopped.load(Ordering::SeqCst) >= 6);
    backend.abort();
    store.pool.close().await;
}
