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

Python-only model inference libraries and upstream services are separate
components. They are not rewritten or deleted merely to claim that all code is
Rust. Rust can own their HTTP/queue orchestration while keeping a narrow existing
inference runtime when it is necessary. PostgreSQL remains the target for owned
persistent service state; SQLite/MongoDB are not deleted before their consumers
and recovery paths have been replaced and verified.

Public service replacement requires a stable reverse proxy with simultaneous old
and new backends. The current delivery system restarts public services; changing
a unit alone must not be described as zero downtime. Until that routing gate is
available, move isolated background workers and compare new APIs on separate
ports. Benchmark latency and failure behavior before enabling new public routes.
