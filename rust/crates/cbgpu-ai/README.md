# Native AI service candidate

Rust/Axum API and reservation-first scheduler with PostgreSQL persistence. This
source is a **candidate**, not evidence that production has switched. The legacy
service must remain available until data import, release validation, and an
explicit operational cutover are complete.

The API listens on `127.0.0.1:13550` by default and rejects non-loopback binds.
It checks fresh account permission before execution; owned project/history
reads do not require school GPU permission. GPU reservations and foreign GPU
processes take priority over application-owned inference workers.

## Components

- `cbgpu-ai-store`: private `ai_store` PostgreSQL schema, owner-bound operations,
  queue, conversations, uploads, projects, hashed API keys, embedding cache, and
  transactional read-only legacy SQLite import.
- `cbgpu-robotics`: protocol validation, remote robots, command queue, and separate
  read-only legacy importer. Import this after core state; neither importer
  overwrites a nonempty target.
- `cbgpu-ai-inference`: native llama.cpp FFI using the reviewed library hash and
  local GGUF files. Handles chat and prepared document summaries.
- `cbgpu-ai-models`: separately linked Candle E5 and Whisper worker. Its process
  replaces the inference launcher for embedding, search, and transcription,
  preserving the scheduler PID/process group without a Python shim.
- `cbgpu-ai-documents`: native document evidence and isolated OCR worker used by
  preparation before document summarization or attachment context assembly.

Search ranks only caller-provided, authorized candidate text. It does not read
legacy global corpus chunks using caller-supplied IDs.

## Configuration

Keep keys and database credentials in private files/environment configuration.
Do not place them in source, public assets, job input, or logs.

Required API configuration:

| Variable | Purpose |
| --- | --- |
| `CBGPU_AI_DATABASE_URL` | Dedicated runtime PostgreSQL login |
| `CBGPU_AI_STATE` | Existing private state root |
| `CBGPU_AI_KEY_FILE` | Internal API key file, at least 16 characters |
| `CBGPU_AI_BACKEND_KEY_FILE` | Local Auth service key file for fresh identity checks |
| `CBGPU_AI_BACKEND_URL` | Local Auth/PostgREST origin; default `http://127.0.0.1:54321` |
| `CBGPU_AI_DATA_ROOT` | Optional existing bulk upload/workspace root |
| `CBGPU_AI_REQUIRE_DATA_MOUNT=1` | Fail closed if configured bulk mount is absent |

`CBGPU_AI_MODE=api-only` explicitly separates the central control plane from
school execution. It ignores an inherited `CBGPU_AI_NATIVE_RUNNER`, creates no
scheduler/instance lease, performs no local NVIDIA/reservation probes, launches
no model/customer-code/OCR subprocesses, and does not extract uncached documents.
Owned cached text/evidence and ordinary history, upload metadata, cancellation
and authenticated queue admission remain available. Tasks persist as queued;
health/status reports `NO_EXECUTOR`, never a working remote worker.

`GET /health/ready` in this mode checks the PostgreSQL queue schema and reports
`control_plane_ready` separately from `execution_ready:false`. An API-only process
must not be used as evidence that submitted jobs can currently execute.

With the mode unset or empty, existing behavior remains unchanged: scheduling
attaches only when `CBGPU_AI_NATIVE_RUNNER` names an absolute native
`cbgpu-ai-inference` ELF executable; document preparation can still run locally.
`CBGPU_AI_MODE=all-in-one` makes that requirement explicit and refuses startup
without a runner. Unknown/whitespace/case-variant mode values fail startup.

Moving this API process does not implement school-node inference. Remote
executor authentication, scoped durable claims/leases, authorized attachment and
artifact transfer, node isolation, cancellation, retries and loss recovery still
require a separate reviewed implementation. Signed GPU inventory enrollment is
not a job-dispatch channel and enables no sharing. Keep existing execution at
school and do not start independent schedulers against the same queue merely
because the API is reachable.
Configure `CBGPU_AI_MODELS_RUNNER` with the absolute separately built
`cbgpu-ai-models` ELF for embedding/search/transcription. Assigned GPU execution
requires the models binary built with its `cuda` feature; a CPU-only binary
refuses assigned-GPU work rather than silently using CPU.

`CBGPU_AI_NATIVE_LIBRARY_PATH` contains existing absolute library directories,
separated by the platform path separator. It becomes `LD_LIBRARY_PATH` only in
trusted native workers. `CBGPU_FFMPEG` selects the trusted absolute native audio
decoder. `CBGPU_AI_CPU_ONLY=1` is for explicit CPU validation, not production GPU
scheduling. Customer code containers receive none of these host library,
worker, or database settings.

The scheduler invokes native workers as:

```text
cbgpu-ai-inference KIND JOB_FOLDER STATE_ROOT
```

Job folders must be UUID directories directly beneath `STATE_ROOT/jobs`.
`input.json`, `progress.json`, `partial.json`, and `result.json` retain the
existing file protocol. Partial output is published incrementally; reasoning
channels and unvalidated tool proposals are not streamed as final answer text.
Tools are proposals for the caller to execute, never arbitrary host execution.
Document jobs require preparation-generated `extracted_text`; raw PDF/image
processing stays in the isolated preparation path.

## PostgreSQL and migration

Use a separate private database. `Store::migrate()` applies the embedded core
migrations; `cbgpu_robotics::migrate()` installs the remote tables. These are
explicit provisioning operations, not API startup actions. Use an approved
migration identity for schema/role creation and a distinct runtime login that
inherits `cbgpu_ai`. The runtime login must not be a superuser and needs no
`CREATEDB` or `CREATEROLE`. Keep `ai_store` out of public PostgREST schemas.

Before production import, stop writers during the final snapshot, back up both
databases and private filesystem state, verify row-by-row importer results,
preserve IDs/key hashes and uploaded files, and retain a rollback path. No
account-type migration or school external-access policy is activated here.

## Verification

Run from the backend repository:

```bash
cargo test --manifest-path rust/Cargo.toml -p cbgpu-ai-store -p cbgpu-ai
cargo test --manifest-path rust/Cargo.toml -p cbgpu-ai-inference
cargo clippy --manifest-path rust/Cargo.toml -p cbgpu-ai-inference --all-targets -- -D warnings
```

Database tests are intentionally ignored by default and reject database names
that do not match their disposable fixture name. On an isolated PostgreSQL 17
service, create three empty databases with a synthetic bootstrap identity that
can create roles. Never supply a production URL.

```bash
CBGPU_AI_TEST_DATABASE_URL=postgresql://fixture:fixture@127.0.0.1:5432/cbgpu_ai_store_test cargo test --manifest-path rust/Cargo.toml -p cbgpu-ai-store --test postgres -- --ignored
CBGPU_AI_TEST_DATABASE_URL=postgresql://fixture:fixture@127.0.0.1:5432/cbgpu_ai_api_test cargo test --manifest-path rust/Cargo.toml -p cbgpu-ai --test http -- --ignored
CBGPU_AI_TEST_DATABASE_URL=postgresql://fixture:fixture@127.0.0.1:5432/cbgpu_ai_scheduler_test cargo test --manifest-path rust/Cargo.toml -p cbgpu-ai --test scheduler_postgres -- --ignored
```

Core store and scheduler integration tests switch to `cbgpu_ai` after bootstrap
and verify least-privilege runtime operations. Hardware/resource fixtures use
native mock executables and never reset GPUs or stop user notebooks. Actual
model smoke tests require explicit local model paths and are separate from
ordinary CI. A passing mock test does not establish GPU model readiness or
authorize a live service restart.

## Reviewed handoff stages

`cbgpu-ai-handoff` is separate candidate operational tooling. Its default
`preflight --state PATH` opens legacy SQLite read-only and reports job counts,
nonterminal work, and user-service state. It neither changes admission nor
stops jobs. Unknown legacy tables or job states require review.

The web BFF currently targets `127.0.0.1:3550`. To preserve that route, an
approved native replacement must bind that port explicitly; `13550` is only the
candidate default. Never start both schedulers against the same filesystem.

After an operator establishes a maintenance/admission window, lets all jobs
finish, stops both AI units, and runtime-masks the legacy unit, the explicitly
gated `import-drained --apply` stage can copy and byte-verify the full state and
optional bulk trees into a new private backup. It refuses nonterminal jobs,
active listeners on either AI port, nonempty native targets, missing schemas,
and insufficient backup capacity. Core and robotics imports perform their own
row equality checks; the resulting private manifest records table counts and
row fingerprints. It does not change BFF routing or activate a native unit.

The import confirmation is `IMPORT_DRAINED_PRIVATE_AI_AFTER_REVIEW`. The database
URL is accepted only through a private regular `--database-url-file`, not a
command-line credential. Root must review this source before any production
apply. Keep legacy unit files, environment files, and credentials in the
separate operational backup as well; this tool backs up data trees only.

`rollback-check` performs read-only eligibility checks while both services are
stopped. It refuses rollback if any native row or legacy database/WAL changed
since the completed import. It never drops native data, overwrites legacy
SQLite, unmasks a unit, or restarts a service. A failed partial import must stay
inactive and be investigated; it is not permission to erase either database.
If native writes exist, reconcile them before returning traffic to the legacy
service.

Admission control, approved service-unit installation, migration provisioning,
full release/credential backup, and final activation remain explicit reviewed
operator stages. This staged tool is not an unattended zero-downtime cutover.

The handoff orchestration fixture additionally uses an empty
`cbgpu_ai_handoff_test` database:

```bash
CBGPU_AI_TEST_DATABASE_URL=postgresql://fixture:fixture@127.0.0.1:5432/cbgpu_ai_handoff_test cargo test --manifest-path rust/Cargo.toml -p cbgpu-ai-handoff --lib -- --ignored
```

It uses a compiled mock service-state executable and private ephemeral listener
ports, imports all twelve legacy core/robotics tables, checks original SQLite
bytes, restores the private snapshot in a temporary directory, rejects queued
work and repeat imports, and forbids rollback after a new native row. It never
queries real systemd units or uses the production AI ports.

## Scoped authorization and remaining operational gate

`CBGPU_MULTISCHOOL_V1=1` or `CBGPU_ACCOUNT_V2=true` selects fresh school-resource
oracles; `1` and `true` are equivalent. Malformed configured flags stop startup.
The scheduler checks the owner's current GPU scope before claiming a job and
again during execution. Trusted native subprocesses receive the canonical
effective policy flag; customer-code containers receive no host policy/database
credentials. Private owned records use fresh global identity checks, separately
from GPU approval: pending affiliation and graduation preserve ownership, while
suspension, deletion or a ban deny access.

Jobs are personal owner-bound work; there is no project editor impersonation.
Creator browser-session IDs are not persisted as an additional job lease. The
trusted BFF must authenticate the real caller freshly before admission; runtime
revocation uses current identity and scoped resource permission. A browser logout
alone is not represented as cancellation of an already admitted background job.

Production switching still requires a drained admission barrier, stopped/masked
legacy writer, idle old/native listeners, a private immutable state/files backup,
reviewed PostgreSQL provisioning, core plus robotics import and equality checks.
A zero queued-job snapshot while the Python writer remains active is insufficient.
`cbgpu-ai-handoff` implements the explicit import and rollback checks; rollback
must refuse after any new native writes. Native API availability alone does not
prove model, OCR, web-search or corpus worker readiness. Validate the configured
native model executables, trusted CUDA libraries, OCR image and scoped corpus
policy connection before a separately reviewed routing change. Private notes are
not added to the shared corpus. These changes do not activate production.

Scoped execution uses the derived `ai` permission: current GPU permission **and**
the school's effective AI module policy (global enabled state, allowed-school
restriction, local configuration). Disabling AI cancels admitted AI workers and
blocks new jobs, web search and document preparation without revoking notebook
GPU access. Successful workers recheck policy before publishing completion.
Existing owned files, extracted text, settings and history remain readable with
a valid global identity. Guest requests still grant the underlying `gpu` scope;
`ai` is a derived capability rather than a new guest-request approval scope.

## Native model data preparation

`cbgpu-ai-prepare-models MANIFEST ABSOLUTE_MODELS_ROOT` is an offline plan.
Adding `--apply` explicitly downloads the reviewed revision/data allowlist with
no cached Hugging Face credentials or proxy configuration. Every redirect uses
approved HTTPS data hosts and fresh public DNS pinning. Weights verify SHA256;
small Git-backed files verify their pinned blob identity, and every final file
receives a recorded SHA256 digest. Existing changed assets and symlink targets
are refused. This prepares data only: native CUDA/llama libraries, FFmpeg and the
OCR image remain separately provisioned prerequisites. No GPU work, runtime
installation, database migration or service activation is performed.

### CBSH shared GPU 1 fallback

On the four-card CBSH host, `CBGPU_AI_SHARE_RESERVED_GPU_ONE=1` allows one
queued chat on GPU 1 when the scheduler has confirmed reservations for all
four cards. Existing CUDA processes on card 1 may continue. Shared inference
can reduce the reservation holder's throughput or available VRAM. Coordinate
this setting with the reservation-screen notice before enabling it.

Regular inference continues to use unreserved cards in parallel. GPU units
2–4 retain normal reservation priority. The fallback is opt-in, only applies
to chat, and is suspended when the reservation snapshot is unavailable.
