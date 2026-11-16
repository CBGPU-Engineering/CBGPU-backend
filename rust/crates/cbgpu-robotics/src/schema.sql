CREATE SCHEMA IF NOT EXISTS ai_store;
CREATE TABLE IF NOT EXISTS ai_store.remote_robots(
 id text PRIMARY KEY,owner text NOT NULL,name text NOT NULL,created double precision NOT NULL,
 revoked boolean NOT NULL DEFAULT false,pair_hash text,pair_until double precision,
 token_hash text,agent_session text,seen double precision NOT NULL DEFAULT 0,
 lease text,lease_until double precision NOT NULL DEFAULT 0,
 telemetry jsonb,map jsonb,image text,stop_epoch bigint NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS remote_robots_owner ON ai_store.remote_robots(owner);
CREATE UNIQUE INDEX IF NOT EXISTS remote_robot_pair_token ON ai_store.remote_robots(pair_hash) WHERE pair_hash IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS remote_robot_device_token ON ai_store.remote_robots(token_hash) WHERE token_hash IS NOT NULL;
CREATE TABLE IF NOT EXISTS ai_store.robot_commands(
 id text PRIMARY KEY,device text NOT NULL REFERENCES ai_store.remote_robots(id) ON DELETE CASCADE,
 kind text NOT NULL CHECK(kind='navigate'),state text NOT NULL CHECK(state IN('queued','dispatched','running','succeeded','failed','cancelled','expired')),
 created double precision NOT NULL,expires double precision NOT NULL,lease text,payload jsonb NOT NULL,
 error text,updated double precision NOT NULL,accepted_at double precision
);
CREATE INDEX IF NOT EXISTS robot_commands_device ON ai_store.robot_commands(device,created);
CREATE UNIQUE INDEX IF NOT EXISTS robot_one_live_goal ON ai_store.robot_commands(device) WHERE state IN('queued','dispatched','running');
CREATE TABLE IF NOT EXISTS ai_store.robot_sync_events(device text NOT NULL REFERENCES ai_store.remote_robots(id) ON DELETE CASCADE,at double precision NOT NULL);
CREATE INDEX IF NOT EXISTS robot_sync_events_window ON ai_store.robot_sync_events(device,at);
CREATE TABLE IF NOT EXISTS ai_store.robot_plans(
 id text PRIMARY KEY,owner text NOT NULL,robot text NOT NULL,capabilities jsonb NOT NULL,
 observed double precision NOT NULL,mission jsonb,error text
);
CREATE INDEX IF NOT EXISTS robot_plans_owner ON ai_store.robot_plans(owner);
-- Private schema. Only the native service DB role is granted access by the reviewed cutover.
REVOKE ALL ON SCHEMA ai_store FROM PUBLIC;
REVOKE ALL ON ai_store.remote_robots,ai_store.robot_commands,ai_store.robot_sync_events,ai_store.robot_plans FROM PUBLIC;
DO $$BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_ai') THEN
  CREATE ROLE cbgpu_ai NOLOGIN NOINHERIT NOSUPERUSER;
 END IF;
END$$;
GRANT USAGE ON SCHEMA ai_store TO cbgpu_ai;
GRANT SELECT,INSERT,UPDATE,DELETE ON ai_store.remote_robots,ai_store.robot_commands,ai_store.robot_sync_events,ai_store.robot_plans TO cbgpu_ai;
ALTER TABLE ai_store.remote_robots ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.robot_commands ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.robot_sync_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE ai_store.robot_plans ENABLE ROW LEVEL SECURITY;
DO $$DECLARE relation text;BEGIN
 FOREACH relation IN ARRAY ARRAY['remote_robots','robot_commands','robot_sync_events','robot_plans'] LOOP
  IF NOT EXISTS(SELECT 1 FROM pg_policies WHERE schemaname='ai_store' AND tablename=relation AND policyname='robotics_trusted_service') THEN
   EXECUTE format('CREATE POLICY robotics_trusted_service ON ai_store.%I FOR ALL TO cbgpu_ai USING(true) WITH CHECK(true)',relation);
  END IF;
 END LOOP;
END$$;
