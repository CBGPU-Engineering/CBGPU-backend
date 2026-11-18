# Optional school GPU inventory

Source candidate, disabled by default. This module enrolls a school's own NVIDIA
machines and accepts signed outbound inventory. It does not run jobs, install
NVIDIA drivers, modify existing GPU controllers, restart notebooks, enable sharing
or grant compute permissions. It is not a distributed inference implementation.

## Security and storage

A fresh school `SCHOOL_OWNER`, `SCHOOL_ADMIN` or `GPU_ADMIN` can mint a 10-minute,
one-use enrollment token. The additive platform-inventory authority migration also permits a fresh,
confirmed, approved ACTIVE platform administrator to mint/list/revoke inventory
for an APPROVED school. This does not create membership, assign school roles,
permit GPU reservations/execution, or enable sharing. Pending/suspended/banned
accounts and LISTED schools remain denied. Redemption rechecks the original
issuer session and current inventory authority. Only a SHA-256 hash is stored. Redemption checks
the issuer's current session, current school role and approved-school state again.
The node generates its Ed25519 key locally and saves it before token consumption;
the server stores only the public key. Signed typed reports bind node ID,
credential version, monotonic sequence, timestamp and bounded GPU inventory.
PostgreSQL checks replay/version/revocation again atomically after verification.
A report older/newer than 90 seconds is rejected; inventory becomes OFFLINE after
120 seconds without an accepted report. Revocation increments the key version.

All node/enrollment tables stay private with no browser/table grants. The service
uses narrow service-only identity/report RPCs. TLS uses system certificate
validation without redirect or insecure overrides. Ed25519 proof is node-key
possession, **not** mTLS certification or proof that telemetry is physically true.
The service never receives an enrollment private key or exposes a command channel.

## Candidate API

- `POST /v1/schools/{id}/gpu/enrollments` with authenticated JWT and `{ "name": "Lab GPU" }`.
- `GET /v1/schools/{id}/gpu/nodes` for the same authorized school roles.
- `POST /v1/schools/{id}/gpu/nodes/{uuid}/revoke` with `{}`.
- `POST /v1/gpu/enroll` with one-use token/public key.
- `POST /v1/gpu/report` with signed typed inventory.

The registry defaults to loopback `127.0.0.1:3548` and
`CBGPU_GPU_REGISTRY_ENABLED=false`; no service or route is installed by this source.
Activation needs the prior school/account migrations, additive migration
`20261117020000_school_gpu_enrollment.sql`, protected TLS routing/rate limiting,
and the additive platform inventory authority when platform management is used,
and private mode-600 `CBGPU_GPU_REGISTRY_ANON_KEY_FILE` and
`CBGPU_GPU_REGISTRY_SERVICE_KEY_FILE`. The optional backend URL defaults to
`http://127.0.0.1:54321`. Never put the service credential on a school node.

## Node support

| Environment | Current scope |
| --- | --- |
| Ubuntu Linux with working `/usr/bin/nvidia-smi` | Foreground inventory reporter candidate |
| Other Linux distributions, Windows, macOS | Node initialization rejected; no installer claim |
| CPU-only, AMD, Jetson without `nvidia-smi` | Not supported by this NVIDIA collector |
| Existing root GPU daemon/notebooks | Left untouched; no replacement, restart or control |

Build `node-agent` from this crate. Save the **complete mint response JSON** in a
mode-600 file; tokens are never command-line arguments. Use a private directory:

```text
node-agent init https://registry.example /private/enrollment.json /private/node.json
node-agent report /private/node.json
node-agent run /private/node.json
```

`run` reports every 30 seconds while healthy, with an exclusive local identity
lock. A failed registry operation exits safely; deployment supervisors/retry UX
are not installed by this candidate. Keep system time synchronized. If an
initialization response is lost, the local key/expected node identity is already
saved; `report` can confirm successful consumption. If consumption did not
succeed, revoke the pending node and mint a replacement rather than overwriting
private keys. No physical school node has been activated or certified here.

Tests include Ed25519 tamper/wrong-key rejection, HTTP proof-before-store,
feature-disabled behavior and disposable network-isolated PostgreSQL enrollment,
replay, expiry, issuer-session revocation and authorization checks. No real school
tokens, notifications, equipment or live tables are used.

## Node-bound inventory and reservation candidate

The additive `20261117121721_school_node_gpu_resources_reservations.sql` candidate
normalizes signed GPU hardware UUIDs under the enrolled node foreign key. School
identity never comes from a report or reservation body. Existing CBsh GPU 1–4
and legacy reservation tables are unchanged. Reports without hardware UUIDs
remain inventory-only; they cannot create reservable resources. The Ubuntu
inventory reporter reads UUIDs through the existing NVIDIA driver query only.

New caller-JWT routes under `/v1/schools/:school/gpu`:

- GET `resources`: scoped hardware summaries and explicit state/reason, without
  host addresses, hardware UUIDs, usernames, credentials, or other owners.
- POST `resources/:resource/reservations`: `{starts_at,ends_at}`; caller identity
  and resource target are server-derived.
- GET `reservations`: the caller's latest 100 records, retained after resource
  permission loss.
- DELETE `reservations/:reservation`: own cancellation with fresh identity,
  independent of a lapsed resource grant.

Admission requires current target-school internal verified membership or a
currently valid, sponsored GPU guest grant, plus the effective `gpu_compute`
module policy. Administrative/teacher titles alone grant no compute access.
Guest approval and membership validity must cover the entire interval. Safe
fixed limits are two outstanding reservations per caller, at most four hours
per reservation and a seven-day booking horizon. A PostgreSQL exclusion
constraint prevents overlap even across concurrent writers.

All newly discovered hardware defaults to `NONRUNNABLE`, with
`EXECUTOR_NOT_CONFIGURED`. Zero utilization is not availability. Node freshness,
credential version, exact report sequence and school status also gate readiness.
The private future executor protocol/enable fields have no public enabling API;
this slice does not establish that handshake. Responses explicitly report
`execution_transport_available:false`. No command leases, execution, job runner,
notebook transport, cross-school sharing or automatic controller handoff are
implemented by this slice. Do not enable compute based on inventory alone.

Validation includes caller/path-bound HTTP tests, actual isolated PostgreSQL
concurrent enrollment and reservation exclusion races, and the complete schema
clone fixture `schema/tests/schools/gpu-resources.sql` (cross-school denial,
nonrunnable inventory, overlap, revoked affiliation, guest interval bounds and
history/cancellation after revocation). No test runs a real GPU workload.

## Outbound signed diagnostics candidate

`CBGPU_GPU_DIAGNOSTICS_ENABLED` defaults off. Enabling this source candidate also
requires `CBGPU_GPU_COMMAND_SIGNING_KEY_FILE`, a private mode-600 regular file
containing a persistent 32-byte Ed25519 seed as hex. The central process never
generates a new signing identity at startup. Authorized diagnostic list/create
responses expose only its public key for deliberate node trust configuration.
No signing seed, lease token/hash, requester session or audit internals appear
in school administrator DTOs.

School OWNER/ADMIN/GPU_ADMIN roles with a fresh real session can GET or POST
`/v1/schools/:school/gpu/nodes/:node/diagnostics`; POST accepts only
`{idempotency_key: UUID}`. This queues the fixed read-only `diagnostic_v1` action,
not an arbitrary command. Metadata explicitly says `DIAGNOSTICS_ONLY` and
`execution_ready:false`. Each node allows at most 20 requests/hour; commands
expire in ten minutes and permit at most three claims. Processing never enables
GPU resources, creates a container or publishes a notebook.

Nodes initiate POST `/v1/gpu/commands/claim`, `/renew`, and `/result`. Protocol 2
uses domain-separated Ed25519 signatures over the typed request, HTTP method,
exact path and SHA-256 of the typed payload. School scope comes only from the
previously enrolled node. Fresh credential version, independent monotonic
command sequence and a bounded timestamp reject replay. Legacy inventory
signatures and their separate sequence remain compatible.

Claims use row locks and SKIP LOCKED; a lease contains an epoch, one-time random
secret stored only as a SHA-256 hash, and a 60-second expiry bounded by the
command deadline. Renewals and results require that exact node/epoch/token and
current requester session/role. Node or requester revocation blocks further
processing. Exact result retries after a lost acknowledgement return the same
successful receipt without rerunning the diagnostic; changed results or expired
credentials fail closed. The private append-only-by-API audit records creation,
claims, renewals, expiry/revocation and accepted results without secrets.

The central signed response also binds node, credential version, request
sequence, method/path and expiry. The node must pin the central public key and
verify every envelope before work. Successful results contain only bounded GPU
UUID/index/name/memory/utilization fields. Failures contain only one of
`NVIDIA_UNAVAILABLE`, `INVALID_INVENTORY`, or `DIAGNOSTIC_TIMEOUT`; raw stdout,
stderr, paths, credentials, scripts and arbitrary shell arguments are rejected.

## Notebook execution candidate (SQL52)

`CBGPU_GPU_NOTEBOOKS_ENABLED=1` and a private persistent
`CBGPU_GPU_NOTEBOOK_SIGNING_KEY_FILE` enable the protocol-v3 HTTP surface. Both
are separate from inventory and diagnostic settings. SQL52 defaults its private
execution policy to disabled with no approved image digest. Enabling HTTP alone
therefore does not admit a job. A reviewed immutable image digest, genuine signed
node Docker/NVIDIA preflight, current hardware inventory, school feature and
fresh membership/explicit guest authorization are independently required.

A reservation owner explicitly acknowledges temporary storage and launches one
idempotent job per reservation. Running jobs retain a 45-second signed lease,
renewed every 10 seconds; the node must stop its own managed generation when the
lease or reservation deadline expires. STOPPING remains occupied until the node
confirms removal. A failed stop never makes the GPU available. Jobs never target
legacy GPU1–4 or accept shell commands, mounts, images or ports from a user.

The same-origin gateway exchanges a fresh owner session for an opaque HttpOnly
cookie. The service-only `school_gpu_notebook_session(token,job_id)` checks the
exact stored Auth session, job, booking, node version and fresh authority on each
HTTP request and WebSocket heartbeat. No JWT is stored in a job, URL or cookie
grant record. This is a source candidate until the paired node executor and relay
are exercised together; registry ONLINE alone still proves no notebook service.
