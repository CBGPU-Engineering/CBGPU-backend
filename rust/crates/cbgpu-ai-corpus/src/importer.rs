//! Read-only legacy cache importer. No source file is modified, renamed or
//! deleted, and existing native rows always win. Imported text remains private
//! until the normal fresh RLS retrieval gate permits it.
use crate::{Corpus, Result, SourceKey, validate_vector};
use anyhow::ensure;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::path::Path;
pub async fn import_legacy(corpus: &Corpus, path: &Path) -> Result<(usize, usize)> {
    ensure!(
        std::fs::symlink_metadata(path)?.file_type().is_file(),
        "Legacy cache must be a regular file"
    );
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    db.execute_batch("BEGIN")?;
    let mut docs = vec![];
    let mut chunks = vec![];
    {
        let mut query =
            db.prepare("SELECT kind,id,version,state,checked FROM documents ORDER BY kind,id")?;
        let rows = query.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
            ))
        })?;
        for row in rows {
            let (kind, id, version, state, checked) = row?;
            let key = SourceKey::parse(&format!("{kind}:{id}"))?;
            ensure!(
                ["material", "paper"].contains(&kind.as_str())
                    && ["ready", "error", "unsupported"].contains(&state.as_str())
                    && checked.is_finite()
                    && !version.is_empty(),
                "Invalid legacy corpus metadata"
            );
            docs.push((key, version, state, checked));
            ensure!(
                docs.len() <= 10000,
                "Legacy corpus document budget exceeded"
            );
        }
    }
    {
        let mut query =
            db.prepare("SELECT kind,id,number,text,vector FROM chunks ORDER BY kind,id,number")?;
        let rows = query.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i32>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (kind, id, number, text, vector) = row?;
            let key = SourceKey::parse(&format!("{kind}:{id}"))?;
            ensure!(
                number >= 0
                    && text.chars().count() <= 650
                    && docs
                        .iter()
                        .any(|(k, _, state, _)| k == &key && state == "ready"),
                "Invalid legacy corpus chunk"
            );
            let vector: Value = serde_json::from_str(&vector)?;
            validate_vector(&serde_json::from_value::<Vec<f32>>(vector.clone())?)?;
            chunks.push((key, number, text, vector));
            ensure!(
                chunks.len() <= 400000,
                "Legacy corpus chunk budget exceeded"
            );
        }
    }
    let mut tx = corpus.runtime.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('corpus-legacy-import',0))")
        .execute(&mut *tx)
        .await?;
    let mut added = std::collections::BTreeSet::new();
    for (key, version, state, checked) in &docs {
        let count=sqlx::query("INSERT INTO ai_corpus.documents(kind,id,version,state,checked)VALUES($1,$2,$3,$4,$5)ON CONFLICT(kind,id)DO NOTHING").bind(&key.kind).bind(key.id).bind(version).bind(state).bind(checked).execute(&mut *tx).await?.rows_affected();
        if count > 0 {
            added.insert(key.clone());
        }
    }
    let mut imported = 0;
    for (key, number, text, vector) in &chunks {
        if !added.contains(key) {
            continue;
        }
        sqlx::query(
            "INSERT INTO ai_corpus.chunks(kind,id,number,text,vector)VALUES($1,$2,$3,$4,$5)",
        )
        .bind(&key.kind)
        .bind(key.id)
        .bind(number)
        .bind(text)
        .bind(vector)
        .execute(&mut *tx)
        .await?;
        imported += 1;
    }
    tx.commit().await?;
    Ok((added.len(), imported))
}
