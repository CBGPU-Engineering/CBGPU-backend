# CBGPU local AI

Four services share a private, reservation-first queue: audio transcription and SRT output, semantic search over authorized sources, an OpenAI-shaped inference API, and restartable Python GPU jobs. Native website actions use the same queue for notes, abstracts, shared documents, and deployment logs. No student input is sent to an external AI provider.

The standard-library control plane binds **127.0.0.1:3550**. Next.js verifies CBGPU identity, approval and live sessions, then forwards a per-owner request with a private internal secret. API keys are hashed and shown once. They can call inference endpoints, not enumerate private browser tasks or run Python containers. GPU workers independently check account access before execution and periodically while running. SQLite state, inputs, outputs and keys are private files outside the checkout.

## Install

Run `ai/bootstrap.sh` to prepare an isolated managed Python 3.12 environment. Run `ai/prepare_models.py` using a Python with huggingface-hub installed to download the exact model revisions in `models.json`. Public model downloads explicitly disable any cached account token. Weights are never committed. Review the upstream model licenses in that manifest.

Install source files into `~/.local/lib/cbgpu-ai/`, install `cbgpu-ai.service` as a user unit, and enable it with user systemd. The installed CI-gated delivery agent switches the unit to immutable backend releases when `ai/` changes. Runtime state is `~/.local/share/cbgpu-ai`; `backend/private/ai` points there for SQLite snapshots. The managed inference venv and weights persist across source releases. Do not upgrade them in place while a task is running.

The internal secret is created at `~/.config/cbgpu-ai-internal-key` with mode 600. It never belongs in source control, a NEXT_PUBLIC variable, a browser or a log. The frontend user must be able to read it. No new DNS record or public port is required; use `/ai` and `/api/ai` on existing CBGPU origins.

## Reservation and workload safety

The scheduler never resets, restarts, stops or mounts a user's Jupyter container. It selects one GPU with no approved reservation active or starting in the next five minutes, no GPU compute process, and under 100 MiB allocated memory. It rechecks immediately before launching, then every one-second polling cycle (database/tool calls can add delay). A new reservation, another GPU process, an unverifiable reservation state, or shutdown stops **only the scheduler's own process/container** and requeues the task. This is bounded polling-based preemption, not zero-latency hardware partitioning. All GPUs being idle in nvidia-smi does not override a reservation.

Audio segments are atomically checkpointed. Text inference restarts the request after preemption. Python jobs keep `/workspace`; users must implement checkpoint writes, reloads and preferably SIGTERM handling themselves. Interactive AI jobs can preempt a Python job after a 60-second time slice. Total CPU/GPU execution time is accumulated; time limits do not reset on reservation preemption. Cancelled/deleted work cannot be resurrected during startup.

Python jobs use a pinned PyTorch/CUDA image with one requested GPU UUID, 4 CPUs, 16 GiB RAM, 256 PIDs, a read-only image, an unprivileged user, dropped capabilities, default seccomp, no-new-privileges, no network and a bounded tmpfs. Only that job's workspace is mounted. Host credentials, other users' workspaces and the Docker socket are never mounted. No arbitrary host shell command or user-selected image is accepted. Docker recovery/removal is restricted to the application's own labelled `cbgpu-ai-*` containers.

Uploads: 200 MiB each, 2 GiB per owner, 30/day. ZIPs: 10,000 entries and 2 GiB expanded, no traversal or symlinks. Workspace size is monitored at runtime, and logs are bounded. Disk free-space guards preserve headroom for the main site. Five active tasks and five keys per account; at most one AI/GPU worker globally. Results expire after 30 days; uploaded originals expire after 14 days unless the work is active. Deleting a task stops it, then removes its input, output, checkpoint and bound upload. Inactive uploads expire separately.

## Search and document boundaries

The frontend selects private-note and shared-file candidates with the caller's Supabase bearer/RLS. Published papers use the same explicit published-only server projection as the public preprint API; public docs are static. The search worker receives only the caller-authorized candidate IDs. A separate CPU-only indexer extracts supported ready shared files and published paper PDFs into a private local corpus, with bounded file size, pages and text length; unsupported and image-only files retain metadata search. The indexer removes withdrawn sources during its next sync. Private notes are embedded only from the caller-authorized request and never enter that corpus. Candidate payloads are bounded and long text is truncated. Sources and permissions are checked again before a search result is returned. Private notes never become a public corpus. The vector cache contains only text hashes and embeddings, not a separately browsable document store.

Document summary accepts text PDFs and selected text formats, up to the first 100 PDF pages and the model's token budget. Image-only/encrypted PDFs fail explicitly; OCR is not implemented. Truncation is reported. Native editing shows proposals; it never silently overwrites or publishes a note. If input changes after inference starts, applying the old proposal is disabled.

## API

Base URL: `https://cbgpu.com/api/ai/v1`. Send `Authorization: Bearer <key>`.

- `GET /models`: `cbgpu-chat`, `cbgpu-embeddings`, `cbgpu-whisper`.
- `POST /chat/completions`: text messages, `stream: false`, max 2,048 output tokens.
- `POST /embeddings`: one to 64 text inputs; 384-dimensional multilingual E5 embeddings.
- `POST /audio/transcriptions`: multipart `file`, optional `language`; JSON `text` result.

The gateway waits up to two minutes for synchronous inference. If no GPU becomes available or a reservation preempts it, a 503 may be returned; the owner can inspect the persistent task at `/ai`. This is not a promise of continuous inference capacity. Browser features use asynchronous `/api/ai/jobs` and polling, and the UI offers cancellation, retries, deletion, logs and artifact download. Raw inference logs are visible only to the job's owner.

## Verify and operate

`python3 -m unittest discover -s ai -p 'test_*.py'` uses synthetic fixtures and no GPU or real school records. It checks ownership, keys, upload limits, ZIP/artifact escapes, queue limits, reservation/user-process preemption, unknown-state pausing and cancellation races. CI runs these tests. Real model checks must first read reservations and GPU processes, select an unreserved GPU and monitor it for preemption; never run QA on a reserved device.

Use `systemctl --user status cbgpu-ai` and `journalctl --user -u cbgpu-ai`. `/health` contains aggregate worker state only. User input and outputs do not belong in journald. Stop this service to release AI resources; do not stop the root GPU daemon. A model/dependency failure should fail the individual task, and an unknown GPU/reservation state must leave work queued/paused.

Existing installed apps can download AI output through their existing authenticated material-download bridge. Ephemeral AI download UUIDs are bound to the owner and task, expire after ten minutes, and never create a public shared-material record. The gateway preserves real material downloads and uses the private AI ticket resolver only when there is no readable ready material with that UUID.

## Private conversation UI

`/ai/chat` uses account-owned conversations stored in the same private SQLite database. History survives browser/device changes; the browser never uses localStorage as a conversation database. Each turn enqueues the same reservation-first local model job. Only the owner can list, open, rename, delete or append messages; API keys cannot access conversation endpoints. One live generation per conversation prevents conflicting turns. Up to 100 conversations per owner and 100 turns per conversation are supported. Model context is bounded to recent completed turns, and pending/cancelled turns are excluded. Completed responses are persisted into history when polled. Unread job results retain the standard 30-day retention; private chat messages remain until the conversation is deleted. Deletion removes chat rows and marks associated jobs deleted, so the scheduler stops only its own running generation and cleans the associated job files.
