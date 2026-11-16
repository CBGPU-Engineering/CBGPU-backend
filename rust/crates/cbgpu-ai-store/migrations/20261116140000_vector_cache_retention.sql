-- Preserve the original 5,000-vector disposable cache bound efficiently.
CREATE INDEX IF NOT EXISTS vectors_recent ON ai_store.vectors(updated DESC,hash);
