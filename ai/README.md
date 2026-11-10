# CBGPU local AI

Four services share a private, reservation-first queue: audio transcription and SRT output, semantic search over authorized sources, an OpenAI-shaped inference API, and restartable Python GPU jobs. Native website actions use the same queue for notes, abstracts, shared documents, and deployment logs. No student input is sent to an external AI model provider. Optional web search sends the search question to DuckDuckGo and reads public source pages.

The standard-library control plane binds **127.0.0.1:3550**. Next.js verifies CBGPU identity, approval and live sessions, then forwards a per-owner request with a private internal secret. API keys are hashed and shown once. They can call inference endpoints, not enumerate private browser tasks or run Python containers. GPU workers independently check account access before execution and periodically while running. SQLite state, inputs, outputs and keys are private files outside the checkout.

## Install

Run `ai/bootstrap.sh` to prepare an isolated managed Python 3.12 environment. Run `ai/prepare_models.py` using a Python with huggingface-hub installed to download the exact model revisions in `models.json`. Public model downloads explicitly disable any cached account token. Weights are never committed. Review the upstream model licenses in that manifest.

Install source files into `~/.local/lib/cbgpu-ai/`, install `cbgpu-ai.service` as a user unit, and enable it with user systemd. The installed CI-gated delivery agent switches the unit to immutable backend releases when `ai/` changes. Runtime state is `~/.local/share/cbgpu-ai`; `backend/private/ai` points there for SQLite snapshots. The managed inference venv and weights persist across source releases. Do not upgrade them in place while a task is running.

The internal secret is created at `~/.config/cbgpu-ai-internal-key` with mode 600. It never belongs in source control, a NEXT_PUBLIC variable, a browser or a log. The frontend user must be able to read it. No new DNS record or public port is required; use `/ai` and `/api/ai` on existing CBGPU origins.

## Reservation and workload safety

The scheduler never resets, restarts, stops or mounts a user's Jupyter container. It selects one GPU with no approved reservation active or starting in the next five minutes, no GPU compute process, and under 100 MiB allocated memory. It rechecks immediately before launching, then every one-second polling cycle (database/tool calls can add delay). A new reservation, another GPU process, an unverifiable reservation state, or shutdown stops **only the scheduler's own process/container** and requeues the task. This is bounded polling-based preemption, not zero-latency hardware partitioning. All GPUs being idle in nvidia-smi does not override a reservation.

Audio segments are atomically checkpointed. Text inference restarts the request after preemption. Python jobs keep `/workspace`; users must implement checkpoint writes, reloads and preferably SIGTERM handling themselves. Interactive AI jobs can preempt a Python job after a 60-second time slice. Total CPU/GPU execution time is accumulated; time limits do not reset on reservation preemption. Cancelled/deleted work cannot be resurrected during startup.

Python jobs use a pinned PyTorch/CUDA image with one requested GPU UUID, 16 CPUs, 32 GiB RAM, 256 PIDs, a read-only image, an unprivileged user, dropped capabilities, default seccomp, no-new-privileges, no network and a bounded tmpfs. Only that job's workspace is mounted. Host credentials, other users' workspaces and the Docker socket are never mounted. No arbitrary host shell command or user-selected image is accepted. Docker recovery/removal is restricted to the application's own labelled `cbgpu-ai-*` containers.

Uploads: 200 MiB each, 2 GiB per owner, 30/day. ZIPs: 10,000 entries and 2 GiB expanded, no traversal or symlinks. Workspace size is monitored at runtime, and logs are bounded. Disk free-space guards preserve headroom for the main site. Five active tasks and five keys per account; up to four workers with one exclusive GPU lease per worker. Results expire after 30 days; uploaded originals expire after 14 days unless the work is active. Deleting a task stops it, then removes its input, output, checkpoint and bound upload. Inactive uploads expire separately.

## Search and document boundaries

The frontend selects private-note and shared-file candidates with the caller's Supabase bearer/RLS. Published papers use the same explicit published-only server projection as the public preprint API; public docs are static. The search worker receives only the caller-authorized candidate IDs. A separate CPU-only indexer extracts supported ready shared files and published paper PDFs into a private local corpus, with bounded file size, pages and text length; unsupported and image-only files retain metadata search. The indexer removes withdrawn sources during its next sync. Private notes are embedded only from the caller-authorized request and never enter that corpus. Candidate payloads are bounded and long text is truncated. Sources and permissions are checked again before a search result is returned. Private notes never become a public corpus. The vector cache contains only text hashes and embeddings, not a separately browsable document store.

Document summary accepts text PDFs and selected text formats, up to the first 100 PDF pages and the model's token budget. Image-only/encrypted PDFs fail explicitly; OCR is not implemented. Truncation is reported. Native editing shows proposals; it never silently overwrites or publishes a note. If input changes after inference starts, applying the old proposal is disabled.

## API

Base URL: `https://cbgpu.com/api/ai/v1`. Send `Authorization: Bearer <key>`.

- `GET /models`: `cbgpu-chat`, `cbgpu-coder`, `cbgpu-pro`, `cbgpu-oss`, `cbgpu-embeddings`, `cbgpu-whisper`.
- `POST /chat/completions`: text/tool messages, optional `stream: true`, `reasoning_effort: low|medium|high`; max 2,048 output tokens for Qwen or 4,096 for GPT-OSS. Real inference deltas are forwarded over SSE while the job runs; validated tool calls are sent as complete objects.
- `POST /embeddings`: one to 64 text inputs; 384-dimensional multilingual E5 embeddings.
- `POST /audio/transcriptions`: multipart `file`, optional `language`; JSON `text` result.

The gateway waits up to two minutes for synchronous inference. If no GPU becomes available or a reservation preempts it, a 503 may be returned; the owner can inspect the persistent task at `/ai`. This is not a promise of continuous inference capacity. Browser conversations use private SSE with polling fallback; other features use asynchronous `/api/ai/jobs`, and the UI offers cancellation, retries, deletion, logs and artifact download. Raw inference logs are visible only to the job's owner.

## Verify and operate

`python3 -m unittest discover -s ai -p 'test_*.py'` uses synthetic fixtures and no GPU or real school records. It checks ownership, keys, upload limits, ZIP/artifact escapes, queue limits, reservation/user-process preemption, unknown-state pausing and cancellation races. CI runs these tests. Real model checks must first read reservations and GPU processes, select an unreserved GPU and monitor it for preemption; never run QA on a reserved device.

Use `systemctl --user status cbgpu-ai` and `journalctl --user -u cbgpu-ai`. `/health` contains aggregate worker state only. User input and outputs do not belong in journald. Stop this service to release AI resources; do not stop the root GPU daemon. A model/dependency failure should fail the individual task, and an unknown GPU/reservation state must leave work queued/paused.

Existing installed apps can download AI output through their existing authenticated material-download bridge. Ephemeral AI download UUIDs are bound to the owner and task, expire after ten minutes, and never create a public shared-material record. The gateway preserves real material downloads and uses the private AI ticket resolver only when there is no readable ready material with that UUID.

## Private conversation UI

`/ai/chat` uses account-owned conversations stored in the same private SQLite database. History survives browser/device changes; the browser never uses localStorage as a conversation database. Each turn enqueues the same reservation-first local model job. Only the owner can list, open, rename, delete or append messages; API keys cannot access conversation endpoints. One live generation per conversation prevents conflicting turns. Up to 100 conversations per owner and 100 turns per conversation are supported. Model context is bounded to recent completed turns, and pending/cancelled turns are excluded. Completed responses are persisted into history when polled. Unread job results retain the standard 30-day retention; private chat messages remain until the conversation is deleted. Deletion removes chat rows and marks associated jobs deleted, so the scheduler stops only its own running generation and cleans the associated job files.

## Model catalog and coding clients

`GET /api/ai/v1/models` exposes ready state, model IDs and context/output limits.
Text models: `cbgpu-chat` (Qwen2.5 7B), `cbgpu-coder` (Qwen2.5 Coder 7B),
`cbgpu-pro` (Qwen2.5 14B). All are pinned Apache-2.0 GGUF assets, loaded into a
single available GPU per task. Each chat turn selects and persists its model.

Chat completions accept bounded OpenAI-style function definitions, assistant
`tool_calls`, tool-result messages and `tool_choice` (`auto`, `none`, `required`,
or a named function). The local model proposes a JSON function call or a final
answer; the adapter validates tool names/argument objects and returns standard
`tool_calls`. **No function is executed by the API or on the server host.**
The calling coding agent validates arguments and asks the user for permission.
This is a supported subset of the OpenAI interface, not every OpenAI endpoint.

`stream: true` provides SSE keep-alive comments while GPU work is queued/running,
then emits the completed response as a chunk and `[DONE]`. It does not simulate
incremental generation. Tool-call deltas include an index and JSON arguments;
`stream_options.include_usage` is supported. A stream has a five-minute deadline;
disconnection cancels only its own queued/generating job. Reservations always win.

[CBGPU Agent](https://github.com/CBGPU-Engineering/CBGPU-agent) wraps the pinned
MIT-licensed OpenCode engine with this provider, restricted models, no paid fallback,
private API-key storage, and approval defaults for edits/shell/external directories.
Install/use instructions are at `/ai/cli`. This is not Anthropic Claude Code.

## Models, effort and private streaming

The four text models are Qwen2.5 7B, Qwen2.5 Coder 7B, Qwen2.5 14B and GPT-OSS 20B (MXFP4). All use an 8,192-token context. GPT-OSS uses its native Harmony format and low/medium/high reasoning; only the final channel is exposed. Qwen high effort runs a draft and review pass before streaming the final answer. Model files and licenses are pinned in `models.json`; GPT-OSS loads a separate llama.cpp runtime.

`partial.json` contains only visible answer text, is atomically replaced, and remains private to the owner. It is never committed as the completed conversation until inference succeeds. Reconnecting receives the current snapshot; failed or cancelled partial answers are marked unfinished and excluded from later context. Preemption restarts generation rather than pretending to continue an old token stream.

Read-only `/v1/web/search` and `/v1/web/read` support the CLI MCP bridge. DNS/IP validation, pinned public-IP TLS connections and redirect checks block private destinations; fetched page instructions are untrusted data.

## Two-volume storage

With `CBGPU_AI_DATA_ROOT=/srv/cbgpu-data/ai` and `CBGPU_AI_REQUIRE_DATA_MOUNT=1`, new uploads and Python workspaces use the mounted Samsung SSD. Existing files and model weights remain on the original NVMe, with no bulk migration. Private queue metadata stays on NVMe. Missing data mounts fail closed; cleanup removes the matching owned bulk files. Both volumes need independent backup and free-space monitoring.

## Robotics planning

`POST /api/ai/v1/robotics/plans` creates an account-owned asynchronous planning job;
`GET /api/ai/v1/robotics/plans/{id}` returns status and a validated mission;
`DELETE` cancels planning. JWTs and AI keys use the same account approval checks.
There is no actuation endpoint or robot network access. The standalone
[CBGPU Robotics SDK](https://github.com/CBGPU-Engineering/CBGPU-robotics) supports
Windows/macOS/Linux Python and documents the portable HTTP contract for other OSes.

The API accepts bounded closed capability schemas, structured observations and
fresh timestamps, and uses a required tool call to propose at most six steps / 60
seconds. It validates every argument before issuing a mission bound to the robot,
capability hash and observation expiry. Raw model proposals never become commands.
The SDK revalidates locally, defaults to dry-run and requires commissioning, arming,
approval, fresh sensors and a cooperative driver with an independent stop/watchdog.
Only virtual hardware is included; physical integration requires the robot's drivers.

`robotics_protocol.py` is vendored Apache-2.0 code from
`CBGPU-robotics/cbgpu_robotics/protocol.py` (protocol version 1). Update and test both
copies together when changing the wire contract. Planning metadata is private SQLite,
removed with expired jobs, and never exposed through the Supabase Data API.
