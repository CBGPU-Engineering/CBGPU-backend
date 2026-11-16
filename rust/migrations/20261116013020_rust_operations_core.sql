-- A separate cbgpu_runtime database. Not a PostgREST-exposed application schema.
CREATE SCHEMA IF NOT EXISTS operations;
REVOKE ALL ON SCHEMA operations FROM PUBLIC;
CREATE TABLE operations.runs (
    id uuid PRIMARY KEY,
    operation text NOT NULL CHECK (operation IN ('study-auto', 'timetable-refresh')),
    slot bigint NOT NULL,
    status text NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'uncertain')),
    started_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    result jsonb NOT NULL DEFAULT '{}',
    UNIQUE (operation, slot)
);
CREATE INDEX runs_recent ON operations.runs (started_at DESC);
ALTER TABLE operations.runs ENABLE ROW LEVEL SECURITY;
-- The migration owner manages schema; the runtime is a separate non-owner role.
CREATE POLICY runtime_runs ON operations.runs TO cbgpu_runtime USING (true) WITH CHECK (true);
GRANT USAGE ON SCHEMA operations TO cbgpu_runtime;
GRANT SELECT, INSERT, UPDATE ON operations.runs TO cbgpu_runtime;
