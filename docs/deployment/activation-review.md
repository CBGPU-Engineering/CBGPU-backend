# Activation review — evidence only

This pack prepares review of the account/multi-school and native-service
candidates. It is **not** an approval, migration runner, service handoff, or
claim that production already uses the candidates. It changes no users,
projects, school memberships, notebook kernels, routes, containers or services.

## Candidate inventory and tested schema

The review command below emits current repository HEADs, dirty-state booleans,
the SHA-256 of every source migration, and the exact frozen clone inventory.
It does not print credentials, environment files, database rows, command-line
secrets, or private configuration. Optional host inspection reads only fixed
systemd service metadata, recognized executable/source filenames and digests,
and the fixed materials container's image/PID metadata. Unknown evidence stays
`UNVERIFIED`; a stopped unit never becomes proof that no orphan writer exists.

Preparation baseline on 2026-11-17 (subsequent commits require a fresh manifest):

| Component | Source baseline |
| --- | --- |
| Backend before this review pack | `67adeee2f9b4f5d94d02e62752d99af19cbcb426` |
| Web integration worktree | `49f04703e412587be9898950c387a85fcd794943` (dirty at capture; not an immutable release) |
| GPU controller source | `637163e3e9620c5c2f73a47cba52d38a8a0e0a16` |
| LaTeX source | `e890ac67fea6f7ccdebcfbfc85f592429cc58dc7` |
| Materials source | `6bfefb4dfce2e9380f95a51efd19c72ebfca9750` |

The private clone `school-schema-clone-20261117-051` has a `PASSED` marker,
identical before/after preservation fingerprints, owner/ACL fidelity and
migration replay twice. Its **26 frozen migration SHA-256 values** matched
current source at capture. The machine-readable review includes each value;
no database dump, fingerprints, account records or credentials belong in Git.
The marker is local evidence, not a signed authorization. Neither it nor a clean
repository proves that operating schema or service flags have changed.

## Run the concrete read-only review

This command is valid on the current review computer. It reads source and the
existing private proof; it creates no operator lock, state directory, routing
rule or service. Cargo can build into its ordinary build cache. The temporary
web path is the actual integration worktree; if it is retired, use a verified
published checkout and regenerate the manifest rather than assuming equivalence.

```bash
cd /home/cbgpu/CBGPU-Engineering/CBGPU-backend-rust
/home/cbgpu/.cargo/bin/cargo run --offline --manifest-path rust/Cargo.toml -p cbgpu-cutover -- review \
  --repo /home/cbgpu/CBGPU-Engineering/CBGPU-backend-rust \
  --proof /home/cbgpu/.local/share/cbgpu-rust/school-schema-clone-20261117-051 \
  --web-repo /tmp/cbgpu-web-external-contest-api \
  --gpu-repo /home/cbgpu/CBGPU-Engineering/CBGPU-gpu-daemon \
  --latex-repo /home/cbgpu/CBGPU-Engineering/CBGPU-latex \
  --materials-repo /home/cbgpu/CBGPU-Engineering/CBGPU-materials \
  --inspect-host
```

Omit `--inspect-host` for source/proof-only review. To save stdout, use a new
private file with `umask 077`; do not place the report in a public asset directory.
The result deliberately retains `ready_for_activation:false`,
`automatic_activation_supported:false` and `orphan_writer_absence:UNVERIFIED`.
Any dirty source, source/proof mismatch, missing evidence or unreviewed runtime
owner must be resolved before proposing an operating change.

## Operating/source differences observed read-only

| Operating writer | Observation | Consequence |
| --- | --- | --- |
| Root combined controller | Active `/home/cbgpu/CBGPU/Daemon/daemon.py`; independent service-role assignment reads | New RLS alone cannot gate its raw reads. Do not flip school/account flags while this bypass and direct notebook/SSH paths remain. |
| AI | Active immutable Python release `59ffb2b3ab4ac013b0cb4055e517d661848ef191`, port 3550; native unit absent/inactive | Rust source and Python retirement in the current repository do not establish a live Rust cutover. Preserve the immutable release and delivery hold. |
| Deployment control | Active immutable `deploy/server.mts` compatibility release | Native workers/proxy are not evidence that all control/business API writes have moved to PostgreSQL. |
| Materials | Running Rust container `cbgpu/materials-rust:candidate`, image ID `sha256:406a45e5d79cf5496e9856bdf45922913b6fdfb41835ff0d1c54d28143805307`, loopback 8421 | Image/source parity and scoped-mode/schema activation remain unverified. Do not infer that recent guest-upload code is active. |
| GPU HTTP gateway | Active native user service with compatibility upstream | An authenticated HTTP ingress does not replace root controller ownership, fence every direct Jupyter/SSH route, or prove scoped runtime flags are enabled. |

Fresh AI preflight observed 1,407 jobs, 25 conversations, 104 messages, 20 hashed
API keys, 3 uploads and 3 robots. Nonterminal jobs were zero at that instant, but
`barrier_ready:false` because the legacy writer remained active. These aggregate
counts are a snapshot, not permission to import, and must be refreshed after
admission has actually been fenced.

## Ordered checks before any operating proposal

1. Freeze clean, immutable candidate revisions and rerun the review manifest.
   Verify the exact image/binary digests against those sources. A new source
   migration needs a new full-schema clone proof; do not reuse a stale marker.
2. Review applied operating migration metadata using the existing trusted DB
   owner workflow. The manifest intentionally leaves this `UNVERIFIED`. Never
   substitute `supabase db push`, a guessed role, or a remote-project connection
   for an audited local owner/ACL migration plan.
3. Refresh the existing AI read-only preflight:

   ```bash
   /home/cbgpu/.cargo/bin/cargo run --offline --manifest-path rust/Cargo.toml -p cbgpu-ai-handoff -- preflight \
     --state /home/cbgpu/.local/share/cbgpu-ai
   ```

   It reads state and service metadata; it does not fence admission, stop a
   writer, import data or activate a unit. `active_jobs:0` with an active writer
   is insufficient. Verify all API/CLI/cron writers, not just one process name.
4. Inspect the controller tool's available safe plan, without `--apply`:

   ```bash
   /home/cbgpu/.cargo/bin/cargo run --offline \
     --manifest-path /home/cbgpu/CBGPU-Engineering/CBGPU-gpu-daemon/Cargo.toml \
     --bin cbgpu-native-install -- preflight
   ```

   This is an offline plan, **not** the scoped RPC/host inspection. Actual
   `preflight --inspect` requires the existing private config/key paths and
   reviewed tunnel/stream/CPU-drain evidence. Those paths must be supplied by
   the owner from the private staged receipt, never guessed or copied to Git.
5. Check actual fresh school oracles, same-school/cross-school/guest/expired
   authorization, compatibility adapters and selected-school uploads in an
   isolated rehearsal. Preserve owned history/files when resource permission
   is lost. No bulk account transition, rollover or external GPU sharing is
   implied by schema compatibility.
6. Produce a separate, explicit operating proposal with backup, admission fence,
   zero-writer proof, privilege owner, changed units/routes/flags, validation
   points and a reverse path. This review command executes none of it.

## Remaining handoff blockers and safe rollback boundaries

- **GPU scoped approval is intentionally unsupported.** `stage --scoped` can
  prepare mode-bound inactive units after a fresh scoped-RPC check, but
  `approve --scoped` and legacy approval of scoped receipts explicitly refuse.
  There is no reviewed single command that safely hands the combined root
  daemon to scoped GPU/CPU/SSH controllers while preserving existing work.
  Root privilege was unavailable through noninteractive sudo during preparation.
  An operator ownership protocol, CPU request-actor proof, direct-ingress fence
  and stream/kernel-preservation rehearsal remain required. Do not tell an
  operator to run a nonexistent scoped-approval command.
- **AI import requires a real barrier.** The existing `import-drained` tool
  requires explicit apply/confirmation, already provisioned target schema,
  stopped native/legacy units and a runtime-masked legacy unit. It backs up and
  imports verified core/robotics state; it does not establish those fences or
  activate services. `rollback-check` only checks eligibility. Never run an
  import merely because a read-only snapshot has zero queued jobs.
- **A central API is not a remote executor.** `CBGPU_AI_MODE=api-only` queues
  durably and reports `NO_EXECUTOR` without local GPU/OCR/model probes. Remote
  authenticated school execution, distributed leases and artifact transport
  remain unimplemented; signed GPU enrollment enables inventory only.
- **Materials and LaTeX need paired releases.** Verify the operating origin,
  current writer, PostgreSQL state/roles, image source and shared-file access
  before changing flags. Materials migration requires an explicit validated
  inventory and genuine writer fence; it preserves source objects. LaTeX
  maintenance defaults to a plan, but its gateway/compiler commands are actual
  services. Never run them as a review probe or reset CE/Mongo project data.
- **Schema rollback is not whole-database restoration.** Existing user IDs,
  passwords, sessions, ownership and memberships must survive. Additive schema
  normally remains while a compatible previous release is restored. Once new
  native writes occur, an old snapshot is not a safe rollback target. Stop new
  admission, drain/reconcile, prove no new writes and use reviewed reverse import
  or forward repair. Keep backups private and prove restoration in isolation.
- **Routing rollback is not this review.** Existing `activate-deploy`,
  `test-route` and `rollback-deploy` commands mutate container/network state.
  None is an account/multi-school activation command; none is invoked here.

The next concrete user-facing action can be running the read-only review command
above. An approval request must identify the separately reviewed operating
change; this pack does not request or assume that approval.
