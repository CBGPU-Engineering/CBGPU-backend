-- Separate private runtime DB only. Never auto-run by the worker.
BEGIN;
CREATE SCHEMA IF NOT EXISTS ai_corpus;
REVOKE ALL ON SCHEMA ai_corpus FROM PUBLIC;
CREATE TABLE IF NOT EXISTS ai_corpus.documents(kind text NOT NULL CHECK(kind IN('material','paper')),id uuid NOT NULL,version text NOT NULL,state text NOT NULL CHECK(state IN('ready','error','unsupported')),checked double precision NOT NULL,PRIMARY KEY(kind,id));
CREATE TABLE IF NOT EXISTS ai_corpus.chunks(kind text NOT NULL,id uuid NOT NULL,number integer NOT NULL CHECK(number>=0),text text NOT NULL CHECK(length(text)<=650),vector jsonb NOT NULL CHECK(jsonb_typeof(vector)='array'AND jsonb_array_length(vector)=384),PRIMARY KEY(kind,id,number),FOREIGN KEY(kind,id)REFERENCES ai_corpus.documents(kind,id)ON DELETE CASCADE);
ALTER TABLE ai_corpus.documents ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_corpus.chunks ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON ALL TABLES IN SCHEMA ai_corpus FROM PUBLIC;
GRANT USAGE ON SCHEMA ai_corpus TO cbgpu_ai;
GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA ai_corpus TO cbgpu_ai;
DROP POLICY IF EXISTS trusted_corpus ON ai_corpus.documents;
CREATE POLICY trusted_corpus ON ai_corpus.documents TO cbgpu_ai USING(true)WITH CHECK(true);
DROP POLICY IF EXISTS trusted_corpus ON ai_corpus.chunks;
CREATE POLICY trusted_corpus ON ai_corpus.chunks TO cbgpu_ai USING(true)WITH CHECK(true);
COMMIT;
