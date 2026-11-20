# Arcade Rust API

Standalone Axum/Postgres service, default `127.0.0.1:3552`; non-loopback binds are refused. No database migrations run at startup. Do not expose Postgres tables through PostgREST.

Explicitly review/apply `rust/crates/cbgpu-arcade/migrations/20261120090000_arcade.sql` to the intended database. It creates only the private `cbgpu_arcade` schema/tables, with RLS and public/anon/authenticated access revoked. The service database role needs usage/select on existing `auth.users`, `public.profiles`, `cbgpu_schools.memberships`, `cbgpu_schools.schools` and CRUD on the new private tables; use a dedicated server role. Never send database credentials to the browser.

Environment:

- `CBGPU_ARCADE_DATABASE_URL`: private PostgreSQL connection string.
- `CBGPU_ARCADE_AUTH_URL`: Supabase base URL (e.g. `http://127.0.0.1:54321`).
- `CBGPU_ARCADE_ANON_KEY`: Supabase anon key, used only alongside the caller's bearer token for `/auth/v1/user`.
- `CBGPU_ARCADE_LISTEN`: optional loopback bind; default above.

Run `cargo run --release -p cbgpu-arcade` from `rust/`. Reverse proxy `/v1/arcade/*` privately and forward Authorization. Each request revalidates token with Supabase and checks deleted/banned/profile status against the database. Personal records and nationwide ranking candidates require an active account. School ranking candidates additionally require current ACTIVE verified membership in the selected school. School board viewing requires caller membership in that exact selected school. Nationwide entries never include school membership, account UUID, name/email/student number. Participation defaults false; only the player's chosen pseudonym is used after opt-in. Ask users to choose a pseudonym rather than identifying information.

## Protocol

All routes require `Authorization: Bearer <access token>`.

- `POST /v1/arcade/sessions`: `{game,params}` -> `{id,game,seed,params,rules_version,expires_at}`. Games `2048`, `memory`, `mines`, `snake`, `flappy`, `tetris`. Memory params `{pairs:6|8|12}` default6; mines `{level:"easy"|"medium"|"hard"}` default easy; others `{}`.
- `POST /v1/arcade/sessions/{id}/submit`: `{actions,score?}` -> `{score,personal_best}`. Optional score must equal replay result. Max20,000 actions,1MiB request,2h sessions; owner-bound, row-locked atomic completion. Exact action-log retries are idempotent, changed completed logs conflict.
- `GET/PUT /v1/arcade/player`: `{display_name,participating}`. Names2–24 letters/numbers/spaces/underscore/hyphen, no profile name hydration.
- `GET /v1/arcade/leaderboard?game=2048&period=week|month|all&scope=national|school&school_id=...&pairs=6&level=easy`: `{ownBest,ownRank,entries:[{rank,displayName,score}],schoolName}`. `school_id` mandatory for school scope; its membership is verified server side. Week/month are calendar periods at Asia/Seoul boundaries; personal best respects selected period and variant. Public entries are capped100. Ties share rank.

Rules are deterministic xorshift32, empty2048 board spawning twice, RNG choosing empty row-major index then2 at90% otherwise4. Memory Fisher-Yates descending paired pool; actions indices, completed score1,000,000-pairAttempts. Mines row then column rejection sampling excludes first-click3x3; actions `{type:"reveal"|"flag",r,c}`, preserving initial flags. Only a cleared board ranks, score1,000,000-actionCount. 2048 scores derive merges and valid nonempty prefixes. Action game rules are in `src/action_games.rs`, with matching browser simulation and server minimum tick duration (2-second tolerance).

Replays reject impossible transitions and forged scores. Public seeds mean a bot can plan moves; replay validation is not proof of human play. Client timestamps are not trusted and timing is not used in puzzle scores.

## Tests

`cargo test -p cbgpu-arcade` runs eight deterministic engine tests and reports the PostgreSQL test as ignored. To run the required database boundary test explicitly, use `ARCADE_TEST_DATABASE_URL=... cargo test -p cbgpu-arcade -- --ignored`. Set `ARCADE_TEST_DATABASE_URL` to a **new disposable empty database** (owner credentials; fixture creates fake auth/profiles/schools and applies only the additive migration). Never point it at any existing deployment database. Tests cover wrong owner, forged score, duplicate/replaced submissions, current membership authorization, opt-out, pseudonym DTO, calendar periods, and banned accounts. The fixture does not clean a reused database; create a fresh database per run.

## Dedicated database role

The application login should be `NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS`, with no inherited privileged memberships. An installer can transfer ownership of **only** `cbgpu_arcade`, `cbgpu_arcade.players`, and `cbgpu_arcade.sessions` to that role after applying the migration. Ownership of these new tables supplies the intended RLS owner access without global BYPASSRLS; do not FORCE ROW LEVEL SECURITY on them unless supplying explicit app policies.

Grant USAGE on `auth`, `public`, and `cbgpu_schools`, and only these existing-table columns:

```sql
GRANT SELECT(id,deleted_at,banned_until) ON auth.users TO cbgpu_arcade_service;
GRANT SELECT(id,account_status) ON public.profiles TO cbgpu_arcade_service;
GRANT SELECT(user_id,school_id,status,verified,starts_at,ends_at)
  ON cbgpu_schools.memberships TO cbgpu_arcade_service;
GRANT SELECT(id,display_name,official_name) ON cbgpu_schools.schools TO cbgpu_arcade_service;
ALTER SCHEMA cbgpu_arcade OWNER TO cbgpu_arcade_service;
ALTER TABLE cbgpu_arcade.players OWNER TO cbgpu_arcade_service;
ALTER TABLE cbgpu_arcade.sessions OWNER TO cbgpu_arcade_service;
```

Existing-table RLS still applies: column grants alone do not override it. The installer must separately review narrowly scoped SELECT policies for this dedicated role, or supply audited read functions and adapt the queries. Do not grant ownership of old tables or global BYPASSRLS to work around this. This additive migration deliberately does not change existing data, grants, or policies.

Unauthenticated `GET /healthz` returns `{ok:true}` as process liveness only; startup requires database connectivity. Graceful shutdown handles SIGINT and Unix SIGTERM.

The separately reviewed `rust/crates/cbgpu-arcade/migrations/20261120090001_arcade_service_privileges.sql` implements those exact column grants, role-specific SELECT RLS policies, and ownership transfers, and refuses a preexisting role with elevated cluster privileges. Provision the restricted internal role before executing it. It is independently retryable. The PostgreSQL integration test enables RLS on all four old fixture tables, applies this privilege script twice, and executes API queries through `SET ROLE cbgpu_arcade_service`; forbidden broad-column reads and old-data updates are also checked.
