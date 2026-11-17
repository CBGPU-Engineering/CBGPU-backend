# Activation review and operating record

This document records the reviewed non-GPU core activation and retains earlier
preparation evidence. It is not a migration runner or permission to replay an
already committed initial-activation bundle. Private credentials and backups do
not belong in this repository.

## Operating activation — 2026-11-18

The final 47-migration bundle below committed atomically under the user's
explicit deployment authorization. The operator wrote the review marker for
that execution; this is not a claim of a separate human approval ceremony.
Post-commit validation found 175 Auth users, 167 profiles, 175 historical CBsh
memberships, 146 account-owned legacy school selections and 2,408 directory
schools. The one trusted teacher remains classified correctly. No academic
rollover, guest approval or GPU sharing was executed.

Native Schools API (3546), public content/assets API (3547) and ad reports API (3549) are enabled
and started, together with the school-mail worker. Separate encrypted narrow
mail/asset database credentials were provisioned after schema commit. The
read-only operating HTTP probe passed 19 checks; three public-detail probes
were explicitly skipped because their public collections were empty. It checked
public DTO privacy, missing-Bearer 401 and anonymous private-row/schema denial;
it did not create an operating test account or send a provider email.

Scoped Node.js deployment control `1b70506` and Materials `2d9` are live. Their files,
keys and history were retained; materials admission is unfenced. Web `20cd21420ddb947699bba0e7022e74b4378f7283` is live with exact health SHA
verified and a normal linear GitHub main push. Its source tree matches the
preceding `e3` release. The `770207c` timetable warm cron
completed against actual school data (34 cells, five days). The `9971a9c` delivery
binary/unit is active; web state adopted `20cd` while backend state stayed
unchanged. The watcher is resumed (`paused=false`) with its timer active.

AI and the root GPU controller remain on their prior operating runtimes.
The LaTeX gateway-only transition is live (`e03` runtime/source `9db29f6`).
Its reviewed freeze proved all old gateway threads stopped, zero TCP connections
and empty receive queues before replacing only the idle gateway. Mongo retained
seven projects, nine users and identical ID/owner fingerprints; compiler PID
`3095423` was untouched. The private receipt is
`aux/gateway-only-1794929656259` (`READY`). New-school LaTeX compilation remains
denied by the explicit CBsh policy. No GPU, AI, CE, Mongo or Redis restart, kernel
reset, external GPU sharing or AWS migration is implied. Dependency advisories
and remaining runtime handoffs are separate work; this is not an all-Rust or
security-clean completion claim.

## Core schema atomic rehearsal — 2026-11-17

The newer private review pack is `core-activation-review-20261117-47` under
`~/.local/share/cbgpu-rust/`. It supersedes the older 27-migration evidence below.
The final inventory includes affiliation-change review, report moderation and
one-time historical account preferences. The rehearsal itself performed no operating activation. The later Nov18
execution is recorded above; do not rerun the initial-activation bundle.

The 47 source hashes match `school-schema-clone-20261117-096-final47` (all migrations replayed twice with their actual SQL fixtures).
The 2,408-school NEIS manifest byte-matches source commit `99dc7e0`. The bundle
SHA-256 is `a7edbbf2ce793e3513644d13aa5fa182b94071d126822db9dc9a7aafdfcf8a4f`.
The same production PostgreSQL image, with network disabled, accepted the entire
bundle as the actual login/session user `supabase_admin`, not a bootstrap login
using SET ROLE. A deliberate exception immediately before commit rolled back
all new namespaces; the successful transaction took 694 ms. A second execution
was refused. Original identity, password, session, profile and business-row
fingerprints matched. During that rehearsal, operating read-only counts remained 175 identities,
167 profiles and two contests; account/school namespaces remained absent.

The private preparation script lexes SQL strings, identifiers, dollar bodies
and comments, removing only matched outer transaction wrappers. It rejects
remaining transaction control and nontransactional commands. The resulting
single transaction uses an advisory lock, 5-second lock timeout, 120-second
statement timeout and 180-second transaction timeout. A NOWAIT write fence
protects the before/backfill/after invariant. This is a brief bounded write
pause, **not a promise of zero request latency**. Busy writers cause rollback;
DDL waits are bounded. PGRST receives its reload notification only on commit.
No academic rollover, guest approval, GPU enrollment, sharing or job execution
is run. Public catalog rows remain listed/unverified with no enabled features.

### Non-GPU core boundary and pairing window

The initial schema backfill preserves existing approved internal CBsh access;
new-school members retain their global EXTERNAL identity and primary approval
bit. School-B membership does not grant CBsh GPU, AI or server execution. The
operating root controller is neither replaced nor stopped: its service-role
reads bypass RLS, and maintenance updates retain their explicit trusted-service
path. Existing reservations and kernels are not reset. This is **non-GPU school
activation**, not completed scoped compute enforcement. GPU guest approvals,
new-school execution/sharing and runtime-safe CBsh academic rollover remain
outside this handoff.

Restrictive policies take effect immediately at SQL commit, regardless of old
web flags. In particular, broad profile joins and shared-credential reads in an
old frontend cannot be assumed compatible. Prestage and verify all binaries,
private configuration, immutable web artifacts and rollback unit snapshots
before committing SQL. Then provision narrow roles, start private APIs, pair
already-tested auxiliary server adapters and switch immediately to the matching
web artifact in one bounded operator window. Do not leave an old web release
running for an open-ended interval after commit. A failed post-commit check
requires narrow service/UI rollback plus fix-forward; reverting to old web alone
does not undo privacy policies or justify restoring a stale database dump.

### Reviewed operator sequence (historical; core47 already committed)

1. Freeze the complete final inventory, rerun actual-role injected-failure and
   success rehearsals, review all protected-row fingerprints, and create the
   final private operator-review marker. At preparation this marker was absent;
   the operator later wrote the exact bundle SHA for the authorized execution.
   Its presence is not permission to rerun an initial-only migration.
2. Use the reviewed helper with its exact final bundle SHA. It validates source
   hashes, creates a new mode-0600 dump/globals snapshot for every attempt, and
   sends the SQL through `docker exec -i cbgpu-supabase-db psql -w -U supabase_admin
   -d postgres -v ON_ERROR_STOP=1`. Do not use the less-privileged postgres role.
   Logs stay private. Check the transaction marker even after a dropped client
   connection before deciding whether anything should be retried.
3. After committed schema verification, provision the separate mail and public
   asset execution-only logins from their encrypted review plans. Their group
   roles do not exist before schema activation. The plans refuse existing login
   names; passwords are independent, not copied from other service readers.
4. Install reviewed private API units: schools 3546, accounts only when its
   paired UI needs it, public content/assets 3547, optional ad reports 3549 only
   when enabled. Verify health **and actual authenticated RPC workflows**.
   Start the mail worker only after its release --check succeeds; provider
   acceptance must not be described as inbox delivery.
5. Pair scoped auxiliary BFF/materials/LaTeX configurations with their tested
   server oracles, then activate the matching immutable web release. Test signup,
   approval, school selection, NEIS life data, feature request/operator response,
   school/national/DM messages and public banner reads. Keep unsupported compute
   policies explicit; no new-school CPU/GPU capability is implied by membership.

Before commit, any SQL failure rolls back automatically. After commit, preserve
additive schema and new writes: restore prior web/unit configurations if needed
and fix forward. **Never restore the preparation dump over a database receiving
new writes.** Existing notebook kernels, controller units and tunnel settings
are outside this operation and must remain untouched. The private backup is for
review/disaster recovery under a separate write-fenced plan, not normal rollback.

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

The private clone `school-schema-clone-20261117-058` has a `PASSED` marker,
identical before/after preservation fingerprints, owner/ACL fidelity and
migration replay twice. Its **27 frozen migration SHA-256 values** matched
current source at capture. The machine-readable review includes each value;
no database dump, fingerprints, account records or credentials belong in Git.
The review also compares the frozen inventory with the current rehearsal
candidate list: adding a candidate invalidates an older matching subset.
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
  --proof /home/cbgpu/.local/share/cbgpu-rust/school-schema-clone-20261117-058 \
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
| Root combined controller | Active `/home/cbgpu/CBGPU/Daemon/daemon.py`; independent service-role assignment reads | New RLS cannot gate its raw reads. This blocks scoped GPU handoff, GPU guest access and claims of immediate runtime revocation; it does not prohibit the separately tested non-GPU school onboarding boundary. Keep GPU execution/sharing deferred and do not run CBsh rollover against active GPU users before runtime enforcement is paired. |
| AI | Active immutable Python release `59ffb2b3ab4ac013b0cb4055e517d661848ef191`, port 3550; native unit absent/inactive | Rust source and Python retirement in the current repository do not establish a live Rust cutover. Preserve the immutable release and delivery hold. |
| Deployment control | Scoped Node.js compatibility release `1b70506` live on Nov18 | Existing deployment files/keys/history retained; this is not a completed native PostgreSQL business-control handoff. |
| Materials | Scoped release `2d9` live on Nov18; admission unfenced | Existing material bytes, keys and history preserved. This activation does not migrate bytes to S3. |
| LaTeX | Gateway-only `e03` / source `9db29f6` live on Nov18 | Idle gateway replaced after reviewed freeze/drain; seven Mongo projects and nine users retained with matching ID/owner fingerprints. Compiler unchanged; new-school compilation remains denied. |
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
- **Further material-storage and LaTeX handoffs need separate review.** The Nov18
  scoped materials release is live, but its activation does not perform an S3
  migration. The gateway-only LaTeX transition is live, while its compiler and CBsh-only
  compute policy remain unchanged. Verify the operating origin,
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


### Equipment follow-up rehearsal

Candidate `20261117093022_equipment_operator_school_authority.sql` is included
in clone 058. Staff authority requires fresh identity, active verified internal
CBsh membership, current fabrication permission and the exact printer assignment
or laser role. A fabrication guest grant permits requests and catalog access,
not staff operations; an old retained assignment does not restore staff access.
The paired portal admission change is web commit `31ca577`.

The rehearsal exercises actual legacy job policies and start-print RPC, invalid
initial operator states, owner cancellation after resource revocation, inventory
writes, cross-printer job/file isolation, role/school/session revocation and
explicit platform oversight separately from equipment-portal admission. Own
historical jobs/files and assignments are retained. No production policy changed.

Storage RLS gates new object reads/signature authorization. Already-issued signed
URLs remain capabilities until their original expiry; a policy change alone does
not revoke them. No signing-key rotation, object deletion or claim of immediate
revocation of existing URLs is part of this candidate.
