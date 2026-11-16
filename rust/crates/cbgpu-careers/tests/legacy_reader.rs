use cbgpu_careers::import::legacy_admin_list;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
#[tokio::test]
async fn compatibility_reader_preserves_order_and_never_returns_private_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("applications.sqlite");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::query("CREATE TABLE applications(id TEXT,created INTEGER,role TEXT,name TEXT,email TEXT,affiliation TEXT,motivation TEXT,experience TEXT,portfolio TEXT,status TEXT,token_hash TEXT,notified INTEGER)").execute(&pool).await.unwrap();
    for (id, stamp) in [("first", 1_i64), ("second", 2_i64)] {
        sqlx::query("INSERT INTO applications VALUES(?,?,'frontend','Fixture','fixture@example.invalid','School','Motivation','Experience','','received','private-token-hash',1)").bind(id).bind(stamp).execute(&pool).await.unwrap();
    }
    pool.close().await;
    let before = std::fs::read(&path).unwrap();
    let rows = legacy_admin_list(&path).await.unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert_eq!(rows[0]["id"], "second");
    assert_eq!(rows[0]["created"], 2);
    assert!(rows[0].get("token_hash").is_none());
    assert!(rows[0].get("notified").is_none());
    assert!(!rows.to_string().contains("private-token-hash"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let missing = dir.path().join("missing.sqlite");
    assert!(legacy_admin_list(&missing).await.is_err());
    assert!(!missing.exists());
}
