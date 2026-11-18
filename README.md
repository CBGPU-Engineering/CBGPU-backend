# CBGPU Backend

[![CI](https://github.com/CBGPU-Engineering/CBGPU-backend/actions/workflows/ci.yml/badge.svg)](https://github.com/CBGPU-Engineering/CBGPU-backend/actions/workflows/ci.yml)
[![Supabase](https://img.shields.io/badge/Supabase-Self--hosted-3FCF8E?logo=supabase)](https://supabase.com)
[![Docker](https://img.shields.io/badge/Docker-Services-2496ED?logo=docker)](https://www.docker.com)

Self-hosted Supabase, database schemas, Edge Functions, email templates, the
operator console, frontend/server deployment workers and public resource gateways.
These services share backend private state and are maintained together.
The optional AGPL LaTeX adapter is a pinned nested submodule at `latex/`.

## Development checks

```bash
git clone --recurse-submodules https://github.com/CBGPU-Engineering/CBGPU-backend.git
cd CBGPU-backend
cd developer && npm ci && cd ..
node scripts/verify-mail-queue.mts
```

The queue check uses an in-memory database and fake transport. It sends no mail.
Other integration scripts can require the full platform and local services.
For running web-facing integrations and existing installation scripts, use
[CBGPU-platform](https://github.com/CBGPU-Engineering/CBGPU-platform), which places this component at `backend/`.
Do not copy production secrets or run destructive reset scripts on a live host.

## Ownership

- `docker-compose*.yml`, `volumes/`: Supabase services and runtime code/templates.
- `developer/`: developer/admin/equipment gateways and interactive console.
- `deploy/`: Git-based builds, Compose diagnostics and project domains.
- `operations/`, `gpu/`, `materials/`: operational and resource gateway integration.
- `supabase/migrations/`, `sql/`: current feature schema changes.
- `schema/legacy/`: historical SQL collections extracted from
  the former web/app/root directories, retained for provenance and review.

See [schema notes](schema/README.md), [operations](OPERATIONS.md),
[console documentation](developer/README.md). New hosts need generated secrets,
reviewed schema initialization and external integration setup; `.env.example`
is not a complete production restore.

## License

Original code is [Apache-2.0](LICENSE). Docker images and vendored libraries keep
their upstream licenses. The `latex/` submodule is separately AGPL-3.0.
See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Team recruitment

[`careers/`](./careers/) runs the independent CBGPU recruitment site, private
application reception, notification outbox and Slack / Jira / GitHub onboarding
guide at `careers.cbgpu.com`. See its README for operations and retention.

## Production delivery

[`operations/delivery/`](./operations/delivery/) documents the CI-gated host pull
agent, immutable releases, health checks, rollback, deployment history and runtime
backup boundaries. Push application changes to the extracted component repository,
not the retired monorepo source folders.


## TypeScript service sources

Maintained Node.js services, browser integrations, tests and operational tools
use TypeScript (`.ts`, `.mts`, `.cts`). Use Node.js 22.18 or newer. Run `npm ci`
at the repository root and `npm ci --prefix developer`, then `npm run check`,
`npm test` and `npm run build:browser`. Browser JavaScript and the pinned xterm
vendor files are generated, ignored build artifacts. Service code continues to
migrate to Rust; Python and other legacy implementations remain until their
verified handoff, as recorded in `rust/conversion.json`.

The delivery agent treats root tooling changes as an explicit infrastructure
rollout, so publishing this source transition cannot silently restart the
public deployment ingress. Existing sessions and deployments must be drained
or routed through simultaneous old/new instances before activating it.

Original component history was extracted from platform commit
`f41d879f6b0a10192fa36235e370372a78ec7331`. Complete history and retired files
remain available in Git; duplicate history maps and archived executable copies
are not maintained in the current source tree.

## School GPU notebooks

New school nodes can report inventory, accept school-scoped reservations and run
an explicitly enabled Jupyter executor. Inventory enrollment alone never grants
compute access or adds a node to the shared AI pool. The existing four-GPU
controller is independent of this executor.

The initial executor supports Ubuntu x86_64 with an existing NVIDIA driver,
Docker access and NVIDIA Container Toolkit. Prepare the approved image before
redeeming the short-lived enrollment file. The default notebook allocation is
2 CPU cores and 8 GiB RAM; 16 GiB host RAM is recommended. Runtime checks reject
unsupported GPU passthrough, insufficient host capacity, foreign GPU processes,
expired leases and another executor's owned container.

Administrators download the enrollment and private notebook policy files from
school administration. The installer uses private files rather than credentials
in shell arguments:

```bash
chmod 600 /absolute/enrollment.json /absolute/notebook-policy.json
./cbgpu-node-linux-x86_64 setup --install-user-service --notebooks /absolute/notebook-policy.json
```

The installer asks for the enrollment path and identity location. Notebook mode
is opt-in, uses a separate owned user service, and never installs a driver,
changes Docker group membership or stops an existing school controller. Existing
inventory service upgrades require an explicit operator action. Check user
service lingering if the node must remain online after logout.

Notebook containers have no external network or published host ports. A
read-only, verified static node executable provides a fixed local stdio tunnel;
no CBGPU account token, node signing key or Docker socket enters the container.
Files are temporary and must be downloaded before the reservation ends.

The central `cbgpu-notebook-relay` verifies live reservation and session authority
for HTTP and periodically for WebSockets. A one-use POST bootstrap installs a
host-only job cookie on `notebooks.cbgpu.com`. Untrusted notebook HTML must never
be served on a CBGPU login origin. The web deployment needs its explicit
WebSocket dispatcher; an external Next HTTP rewrite alone is insufficient.

Notebook SQL is additive and initially disabled. Apply it only after backup and
clone validation, then configure the reviewed image digest and persistent
signing key. GPU sharing remains disabled. Physical GPU passthrough must pass
on each enrolled server before its resources become reservable. CPU transport
fixtures do not establish physical GPU compatibility.
