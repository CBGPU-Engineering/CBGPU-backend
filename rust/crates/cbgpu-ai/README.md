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

Scheduling is attached only when `CBGPU_AI_NATIVE_RUNNER` names an absolute native
`cbgpu-ai-inference` ELF executable. Omitting it starts an API-only candidate.
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
