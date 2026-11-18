# Deployment architecture

The implementation and measured limits are recorded in [cloud-readiness.md](./cloud-readiness.md).
The central application consists of the Next.js web/BFF, native Rust APIs,
PostgreSQL/Auth/Storage, persistent workers and private object/artifact storage.
The current host also runs compatibility services awaiting separately fenced
native handoffs. A source replacement does not switch the running process.

School GPU controllers, notebook kernels, model execution and school-owned volumes
remain at their schools. Enrollment is outbound, authenticated and private by
default. A central relocation must preserve protocol versions, reservation IDs,
job IDs and reconciliation; it must never infer sharing consent from enrollment.

Public and authenticated traffic retain the same stable domains. Host allowlists,
caller-bound session checks, tenant policies and file authorization are application
boundaries, regardless of whether ingress is Cloudflare or a cloud load balancer.

### Latest-green delivery and reviewed Node control scope

Delivery selects one newest successful **main push** CI ancestor; it does not
queue intermediate commits. A pending/failed head can use the newest verified
ancestor, but ancestry checks retain any newer already-deployed release. The
candidate is checked again immediately before switching, so a newer green
revision supersedes an obsolete prepared build. A newer failed rerun cannot
borrow an older successful run for the same commit.

GitHub compare responses reaching the 300-file limit are incomplete. Delivery
fetches the two exact commit trees and reads their complete NUL-delimited Git
diff instead. Infrastructure approval does not override independent AI/GPU data
handoffs.

An explicit private `--adopt-managed-deploy REVIEW.json` handshake, while the
watcher is paused, enables only the existing Node deployment-control adapter on
3510. The version1 review object contains `service:"deploy"`, `reviewed:true`,
`source_sha`, `runtime_sha`, the exact `unit_sha256`, the actual absolute
`data_path` ending in `/frontend-deployments/deploy.sqlite`, and two `workers`
objects `{path,sha256}` pinning `cbgpu-front-worker` and `cbgpu-compose-worker`.
Adoption verifies current source CI/main ancestry, installed unit, private-state
binding, worker hashes and the existing health revision. It changes metadata
only. It does not mark the aggregate legacy backend, AI, GPU, native portal,
Schools API or database as deployed at that revision.

Unrelated school/community schema and documentation history does not inflate
this service's managed change count. Deployment shared dependencies, native
worker/CLI changes and deployment/central-CPU schema changes remain review holds.
Unit or worker drift also holds. No unsupported unit is reverted to Node.
Actual deployment state, environment/key files and artifact references remain
in place. The actual pinned SQLite database is snapshotted, then a bounded
`BEGIN IMMEDIATE` fence checks both build queues and blocks concurrent enqueue
writes across the Type=simple control-process restart. The fence is rolled back
before health checks; no schema, permanent fence record or customer row changes
are used. Unknown/active queues hold the switch. Running customer containers and
native build workers are not restarted by this adapter.

`--plan backend` reports the service scope and independently held services.
`--rollback-managed-deploy` restores only that service's reviewed previous unit,
with the same idle/write-fence and worker checks. It never restores an old
customer database over newer writes. Aggregate `--rollback backend` is refused
once this separate scope is adopted. `--retry backend` clears its failed-candidate
hold. Installing source or a candidate binary does not enable the handshake or
activate these changes.
