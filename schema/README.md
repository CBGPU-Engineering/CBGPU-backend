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
