-- Separate private runtime database; never replay against application/Auth schemas.
CREATE SCHEMA IF NOT EXISTS ai_store;
REVOKE ALL ON SCHEMA ai_store FROM PUBLIC;
CREATE TABLE ai_store.jobs(id text PRIMARY KEY,owner text NOT NULL,kind text NOT NULL CHECK(kind IN('chat','transcribe','embed','search','document','python')),title text NOT NULL,state text NOT NULL CHECK(state IN('queued','running','paused','succeeded','failed','cancelled','deleted')),created double precision NOT NULL,updated double precision NOT NULL,gpu integer,attempts integer NOT NULL DEFAULT 0,progress text NOT NULL DEFAULT '',error text,started double precision,finished double precision,runtime double precision NOT NULL DEFAULT 0,crashes integer NOT NULL DEFAULT 0,input jsonb NOT NULL DEFAULT '{}');
CREATE INDEX jobs_owner ON ai_store.jobs(owner,created DESC);
CREATE INDEX jobs_queue ON ai_store.jobs(state,updated,created);
CREATE TABLE ai_store.uploads(id text PRIMARY KEY,owner text NOT NULL,name text NOT NULL,size bigint NOT NULL CHECK(size>0),created double precision NOT NULL,job_id text);
CREATE INDEX uploads_owner ON ai_store.uploads(owner,created);
CREATE TABLE ai_store.ai_projects(id text PRIMARY KEY,owner text NOT NULL,title text NOT NULL,created double precision NOT NULL);
CREATE INDEX ai_projects_owner ON ai_store.ai_projects(owner,created DESC);
CREATE TABLE ai_store.ai_project_files(project text NOT NULL REFERENCES ai_store.ai_projects(id) ON DELETE CASCADE,upload text PRIMARY KEY REFERENCES ai_store.uploads(id));
CREATE TABLE ai_store.api_keys(id text PRIMARY KEY,owner text NOT NULL,name text NOT NULL,prefix text NOT NULL,hash text NOT NULL UNIQUE,created double precision NOT NULL,last_used double precision,revoked integer NOT NULL DEFAULT 0);
CREATE INDEX api_keys_owner ON ai_store.api_keys(owner,created DESC);
CREATE TABLE ai_store.downloads(id text PRIMARY KEY,owner text NOT NULL,job_id text NOT NULL,kind text NOT NULL CHECK(kind IN('artifact','text','srt')),path text,expires double precision NOT NULL);
CREATE INDEX downloads_owner ON ai_store.downloads(owner,expires);
CREATE TABLE ai_store.vectors(hash text PRIMARY KEY,vector jsonb NOT NULL,updated double precision NOT NULL);
CREATE TABLE ai_store.conversations(id text PRIMARY KEY,owner text NOT NULL,title text NOT NULL,created double precision NOT NULL,updated double precision NOT NULL,model text NOT NULL DEFAULT 'cbgpu-chat',effort text NOT NULL DEFAULT 'medium',project text REFERENCES ai_store.ai_projects(id) ON DELETE SET NULL,site_search integer NOT NULL DEFAULT 0);
CREATE INDEX conversations_owner ON ai_store.conversations(owner,updated DESC);
CREATE TABLE ai_store.chat_messages(id text PRIMARY KEY,conversation text NOT NULL REFERENCES ai_store.conversations(id) ON DELETE CASCADE,role text NOT NULL,content text NOT NULL,job_id text,created double precision NOT NULL,web_sources jsonb NOT NULL DEFAULT '[]');
CREATE INDEX chat_messages_conversation ON ai_store.chat_messages(conversation,created,id);
-- No public/anonymous/browser grants. Only the dedicated trusted API/worker role
-- may access this unexposed schema; every public Store operation checks owner.
REVOKE ALL ON ALL TABLES IN SCHEMA ai_store FROM PUBLIC;
ALTER TABLE ai_store.jobs ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.uploads ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.ai_projects ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.ai_project_files ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.api_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.downloads ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.vectors ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.conversations ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.chat_messages ENABLE ROW LEVEL SECURITY;
-- Private trusted service role (login/password provisioned separately, never public).
DO $$BEGIN IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_ai') THEN CREATE ROLE cbgpu_ai NOLOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION;END IF;END$$;
GRANT USAGE ON SCHEMA ai_store TO cbgpu_ai;
GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA ai_store TO cbgpu_ai;
DO $$DECLARE item text;BEGIN FOREACH item IN ARRAY ARRAY['jobs','uploads','ai_projects','ai_project_files','api_keys','downloads','vectors','conversations','chat_messages'] LOOP EXECUTE format('CREATE POLICY ai_trusted_service ON ai_store.%I TO cbgpu_ai USING(true) WITH CHECK(true)',item);END LOOP;END$$;
