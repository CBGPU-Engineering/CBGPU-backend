# CBGPU Rust services

Rust + Axum + PostgreSQL replacement work proceeds component by component.
The existing website, mobile design, auth sessions, reservations, live GPU jobs,
user deployment containers and original databases remain in place during rollout.

## Implemented

- `cbgpu-core serve`: loopback Axum server with liveness, database readiness,
  bounded connection pooling, protected operational history and graceful shutdown.
- `study-auto`: native Rust replacement for the Python **timer trigger**, retaining
  the same weekday KST 07:30–15:30 window, HTTP contract and existing scheduler.
- `timetable-refresh`: native Rust replacement for the Python timetable warmer,
  preserving the existing last-good cache API and treating stale/empty data as failure.
- PostgreSQL operation claims and redacted audit records in a **separate**
  `cbgpu_runtime` database on the existing PostgreSQL cluster. The login role is
  not a superuser, does not own tables and cannot delete audit records. Its pool
  is capped at four connections for HTTP and one for each worker.

The timer trigger is not the complete school automation implementation. School
business logic, personal tokens, nearest-seat selection and durable submission
claims still run in the existing API until separately ported and verified.
No request transport automatically retries a possible school write. A lost reply
is recorded as `uncertain`, never success. The old scheduler's per-student/per-period
claims continue to protect real applications across trigger versions.

## Build and checks

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
# On this host after provisioning; creates/drops only cbgpu_runtime_test:
python3 scripts/integration.py
```

The real PostgreSQL integration test checks concurrent claims, a single outbound
call, preserved uncertain outcomes and denied audit deletion. All test HTTP
requests use isolated local stubs, not the school API. CI provisions its own
PostgreSQL test container; the test refuses a production database name.

## Local rollout

`provision-local.py` creates only new roles/database, runs SQLx migrations with a
separate schema owner, disables owner login afterward, and writes a private 600
runtime env file outside Git. Existing roles or databases cause a refusal rather
than credential replacement. The runtime role never receives schema ownership.
The migration was created with the Supabase CLI and is applied by SQLx to the
separate, non-PostgREST database.

```bash
python3 scripts/provision-local.py
python3 scripts/install-local.py
```

Installation preflights both workers without API writes, installs a content-addressed
binary, checks the new Axum readiness endpoint and preserves both existing timers.
It restarts **no public listener** and never stops a running oneshot. The next
normal timer activation uses Rust; no extra school application is triggered to
test deployment. Previous units and binary link are retained in a private rollback
manifest. Use `python3 scripts/rollback-local.py ROLLBACK_DIRECTORY` to restore the saved
units and binary link. Code rollback does not delete the PostgreSQL audit database.

The cluster currently has no published PostgreSQL port. Provisioning discovers
its private Docker IP and saves it in the private DSN. If the database container
is recreated with a different IP, update the DSN and restart only `cbgpu-rust-core`;
timer workers read configuration on each invocation. Do not expose port 5432 to
make this service reachable. Future containerized services should use the cluster's
private Docker DNS instead.

```bash
systemctl --user status cbgpu-rust-core
curl --fail http://127.0.0.1:3540/health/ready
# /internal/operations requires the separate private Rust admin key.
```

The new Axum port is private and has no public DNS route. Existing public API
contracts are not redirected into incomplete implementations.

## Remaining conversion order

| Component | Required compatibility gate before moving production |
| --- | --- |
| Deployment control API and build runner | Existing settings, owner/editor/viewer permissions, Git subfolders, secret handling, rollback and Compose persistent volumes |
| Careers API and notification outbox | Existing five applications, form token compatibility, idempotent submits, email acknowledgement and management commands |
| Developer portal API and terminal transport | Existing developer access, account revocation and interactive PTY/WebSocket sessions |
| School integration business logic | Exact user identity, expiry/refresh behavior, seat maps and no duplicate real requests |
| Materials and document services | Existing object paths, owner/collaborator access, range downloads and original uploads |
| GPU/SSH/Robotics gateways and controllers | Long-running sessions survive cutover; reservation and emergency-stop rules remain enforced |
| AI service orchestration | Existing GPU reservation priority, streaming, jobs, keys, OCR and private data permissions |
| Operational backup/delivery jobs | Backup restore verification, approved-main CI, atomic routing and automatic rollback |
| LaTeX orchestration and MCP clients | ZIP/PDF/storage compatibility; client protocol and OS packaging tests |

All owned non-frontend code remains in scope, including AI orchestration,
MCP clients, operational tooling and application API routes. Existing Python
inference runtimes are temporary compatibility components until a Rust engine
passes model, GPU and output compatibility checks. Upstream dependencies and
user-deployed workloads are not CBGPU application code. PostgreSQL remains the target for owned
persistent service state; SQLite/MongoDB are not deleted before their consumers
and recovery paths have been replaced and verified.

Public service replacement requires a stable reverse proxy with simultaneous old
and new backends. The current delivery system restarts public services; changing
a unit alone must not be described as zero downtime. Until that routing gate is
available, move isolated background workers and compare new APIs on separate
ports. Benchmark latency and failure behavior before enabling new public routes.


## Deployment conversion candidates

`cbgpu-front-worker` implements the Git clone, isolated Docker build, Next.js
runtime health check and static artifact publication contract in Rust. It accepts
configuration on stdin and emits a single trusted `CBGPU_RESULT` record. Repository
logs are prefixed so they cannot impersonate that record; secrets are redacted.
The existing API supervisor defaults to the legacy worker until explicitly enabled.

`cbgpu-gateway` is a streaming Axum/Hyper transport. It preserves tenant Host,
cookies, request bodies, SSE and HTTP upgrades, including existing WebSockets
while the listener drains. It does not implement deployment business logic.

`cbgpu-cutover` is a local operator tool, never an HTTP API. `test-route` uses
only loopback ports 18510/18511 to rehearse connection-preserving routing and
rollback. Production activation requires an exact main commit with successful
CI and a binary built with that commit. No production routing is changed by
building or testing these crates. Rollback removes only the owned routing jump
and leaves established gateway connections alive.

`conversion.json` tracks unfinished services and web API routes explicitly.
Implemented candidates must not be reported as active production replacements.


`cbgpu-careers` implements the recruitment API, existing HMAC form tokens,
PostgreSQL concurrency limits, operator commands, verified read-only SQLite import
and Resend outbox in Rust. It serves the existing public assets without redesign.
It is a candidate, not the active recruitment service. Its outbox defaults off;
`CAREERS_ENABLE_OUTBOX=1` is permitted only after the old sender is drained and
all application notification flags have been imported. Retries use the original
idempotency key and byte-identical notification payload. Each successful provider
acknowledgement commits independently.

The separate `careers` schema is never accessible to the operational runtime
role. Provision `cbgpu_careers` as a non-owner, non-superuser, NOINHERIT login and
grant schema USAGE plus table SELECT/INSERT/UPDATE/DELETE. RLS admits only that
service role. Keep the schema owner as a NOLOGIN role. Do not add this schema to
PostgREST's exposed schemas.

Candidate configuration: `CAREERS_DATABASE_URL`, `CAREERS_PUBLIC_DIR`,
`CAREERS_ROLES_PATH`, `CAREERS_FORM_KEY_PATH` (existing UTF-8 hexadecimal key),
optional `CAREERS_LISTEN` (default `127.0.0.1:3532`), `CAREERS_ORIGIN`,
`CBGPU_RELEASE_SHA`. Enabling the outbox also requires the private
`CAREERS_MAIL_ENV` file. No provider errors, applicants, keys or tokens are logged.
Operator `--list`, `--show REFERENCE [--status STATUS]`, `--delete REFERENCE`
commands require the protected database credential. `--import-sqlite SOURCE`
requires an empty PostgreSQL destination and explicit
`CAREERS_LEGACY_WRITER_QUIESCED=1`; stop and drain the legacy writer before a final
import. It verifies every imported field and rate-limit history row before commit.
Keep the old database as a rollback artifact; never delete it during import.
