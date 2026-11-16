-- A separate schema in cbgpu_runtime, never the public/auth application database.
-- Provision the NOINHERIT cbgpu_careers login separately; no passwords in SQL.
CREATE SCHEMA careers;
REVOKE ALL ON SCHEMA careers FROM PUBLIC;
CREATE TABLE careers.applications (
    id text PRIMARY KEY,
    token_hash text UNIQUE NOT NULL CHECK (length(token_hash)=64),
    created bigint NOT NULL,
    role text NOT NULL,
    name text NOT NULL,
    email text NOT NULL,
    affiliation text NOT NULL,
    motivation text NOT NULL,
    experience text NOT NULL,
    portfolio text NOT NULL,
    consent_version text NOT NULL,
    status text NOT NULL DEFAULT 'received' CHECK (status IN ('received','contacted','joined','closed')),
    notified integer NOT NULL DEFAULT 0 CHECK (notified IN (0,1))
);
CREATE TABLE careers.requests (
    ip text NOT NULL CHECK (length(ip)=64),
    created bigint NOT NULL
);
CREATE INDEX request_ip ON careers.requests(ip,created);
CREATE INDEX applications_created ON careers.applications(created DESC);
CREATE INDEX pending_notifications ON careers.applications(created) WHERE notified=0;
ALTER TABLE careers.applications ENABLE ROW LEVEL SECURITY;
ALTER TABLE careers.requests ENABLE ROW LEVEL SECURITY;
CREATE POLICY careers_service ON careers.applications FOR ALL
    USING (current_user='cbgpu_careers') WITH CHECK (current_user='cbgpu_careers');
CREATE POLICY careers_service ON careers.requests FOR ALL
    USING (current_user='cbgpu_careers') WITH CHECK (current_user='cbgpu_careers');
-- Other runtime services never inherit applicant access. Role provisioning can
-- precede or follow this migration; grants belong only to this service role.
DO $$ BEGIN
    IF EXISTS (SELECT FROM pg_roles WHERE rolname='cbgpu_careers') THEN
        GRANT USAGE ON SCHEMA careers TO cbgpu_careers;
        GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA careers TO cbgpu_careers;
    END IF;
END $$;
