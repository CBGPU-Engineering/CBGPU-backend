#[tokio::test]
async fn both_sqlite_adapters_use_the_patched_bundled_engine() {
    let legacy = rusqlite::Connection::open_in_memory().unwrap();
    let legacy_version: String = legacy
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .unwrap();
    let native = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    let native_version: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&native)
        .await
        .unwrap();
    assert_eq!(native_version, legacy_version);
    let version: Vec<u32> = native_version
        .split('.')
        .map(|part| part.parse().unwrap())
        .collect();
    assert!(version.as_slice() >= [3, 54, 1].as_slice());
    native.close().await;
}
