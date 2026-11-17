# Native deployment control candidate

This source replaces the private deployment business API with Axum and PostgreSQL. It preserves project IDs, encrypted environment variables, membership, production selection, history, custom-domain proof records and Compose state. It is **not activated** and does not modify the running Node controller.

The API requires the existing 32-byte deployment encryption key, its hexadecimal internal API credential and a trusted BFF-provided owner identity. Fresh backend account and server ownership checks occur before access; customer-supplied resource-owner headers do not confer ownership. User secrets are encrypted in durable queued work and excluded from project responses.

`CBGPU_DEPLOY_CONTROL_ENABLED=1` explicitly starts the loopback listener (default `127.0.0.1:13580`). Required configuration: `CBGPU_DEPLOY_DATABASE_URL`, `CBGPU_DEPLOY_DATA`, `CBGPU_DEPLOY_KEY_FILE`, and `CBGPU_DEPLOY_BACKEND_KEY_FILE`. `CBGPU_DEPLOY_BACKEND_URL` defaults to the local backend. Schema creation and read-only SQLite snapshot import are explicit library operations, never automatic startup actions.

`CBGPU_DEPLOY_WORKERS_ENABLED=1` additionally requires trusted absolute native ELF front, Compose and domain worker paths, backend root and Compose base. Native workers run in separate process groups with bounded/redacted log collection, fresh execution permission checks, durable leases and failure preservation of previous frontend production or Compose containers/data volumes. A per-project read-only Unix Docker proxy filters container namespaces and streams logs without exposing daemon write endpoints. Native controllers acquire an exclusive private lock; a reviewed cutover must also fence the legacy Node writer.

Tests use no operational Auth, deployment database, Docker daemon, GPU or Cloudflare mutations. Unit tests include the exact Node AES-GCM envelope, config validation, forged worker markers, secret redaction and a synthetic Unix Docker server. The PostgreSQL integration test requires a disposable database ending in `/cbgpu_deploy_control_test` via `CBGPU_DEPLOY_CONTROL_TEST_DATABASE_URL`; it provisions its schema, uses the narrow service role and verifies ownership, encrypted queue snapshots, promotion, failed-build continuity, interrupted-work recovery, retained history and read-only SQLite import with unchanged source bytes/ciphertext plus overwrite refusal.

`CBGPU_DEPLOY_PUBLIC_LISTEN=127.0.0.1:13581` optionally attaches a separate public tenant origin. Production/preview/custom-domain resolution reads PostgreSQL; approved public server targets use fresh backend state. Static releases use bounded streaming and openat/O_NOFOLLOW paths; HTTP and upgraded streams reuse the native gateway and filter core cookies in both directions. The public listener never routes to private control APIs.

Actual disposable PostgreSQL tests now compile a genuine ELF worker, promote its static output, serve an SPA route, reject private control access on the public listener, and test real HTTP plus upgraded socket traffic with tenant cookie/Bearer preservation and core/parent Set-Cookie rejection.

Before activation: finish broader collaboration/domain cases and Compose compiled-worker fixtures; validate Git auto-poll and wildcard-domain provisioning parity, native worker availability, and rollback against a private drained snapshot. Retention queues cleanup for old frontend releases while preserving three ready versions and current production. No live migration, import, domain provisioning or deployment has been performed by this candidate.


## Reviewed source boundary (2026-11-17)

This candidate is part of the Rust workspace, not the operating deployment
service. Its own tenant proxy (not the general application gateway) rechecks
canonical project/owner/port/deployment and resource access for HTTP and WebSocket
streams. Source checks include 13 library tests, all-target clippy, and a real
disposable PostgreSQL contract covering read-only SQLite import, ciphertext and
IDs, request-actor provenance, sticky access state, active-queue refusal, target
import locking, writer-listener fencing and stale rollback rejection. Those tests
do not certify the private operating snapshot or installed worker artifacts.

A closed legacy listener and an operator fence acknowledgement do not prove that
all orphan/background writers are fenced. Verify those writers explicitly before
handoff. After native writes, the old SQLite snapshot cannot safely be resumed:
the rollback checker rejects it. There is no automatic reverse SQLite exporter;
use fix-forward or a separately reviewed reverse-state transfer. Never overwrite
newer native state with the old snapshot. No production service, schema, queue,
artifact directory or container is changed by workspace integration.
