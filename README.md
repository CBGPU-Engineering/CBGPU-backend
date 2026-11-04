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
node scripts/verify-mail-queue.mjs
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
- `schema/legacy/`: historical schema/Edge Function collections extracted from
  the former web/app/root directories, retained for provenance and review.

See [schema notes](schema/README.md), [operations](OPERATIONS.md),
[installation background](docs/OPERATIONS-LEGACY.md) and
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
