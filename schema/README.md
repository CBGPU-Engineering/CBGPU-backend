# Database source ownership

Database migrations are maintained in CBGPU-backend. Current feature migrations
remain in `supabase/migrations` and incremental SQL in `sql`; initialization SQL
is in `volumes/db`. `legacy/root-migrations`, `legacy/web-supabase` and
`legacy/app-supabase` preserve the original collections from the monorepo.

Legacy collections can overlap. They are provenance archives, not a new ordered
bootstrap sequence. Do not blindly execute every SQL file or redeploy archived
Edge Functions. Current Edge Function source is under `volumes/functions`.
Initialize and validate a new database in isolation, with RLS, grants, Storage
and service configuration, before connecting real users. Runtime dumps, password
hashes and keys are not included here.

The remaining web-root SQL was moved byte-for-byte from CBGPU-web commit
978551ded0af0ffb46015ed98bbb67dddd9733e9 into `legacy/web-root`. These are
historical incremental scripts, not a new bootstrap order. Frontend SQL test
fixtures now live under `tests/contests` and `tests/cpu-projects`; they only belong
in an isolated test database. No SQL was applied to production during this move.
