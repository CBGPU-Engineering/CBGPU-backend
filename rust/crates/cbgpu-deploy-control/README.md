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

## Reverse snapshots after native writes

Reverse export writes a **new** private SQLite file plus `<file>.manifest.json`; it never overwrites the original SQLite or starts a legacy/native service. The six legacy runtime tables retain their IDs, ciphertext, project memberships, deployment history, artifact references, `requested_by`, and `access_state`. Terminal native work history is archived in the manifest and restored by reverse-bundle import, so it is not silently discarded by a legacy-only schema.

The version `cbgpu-deploy-reverse-v1` manifest is authenticated and encrypted, not digitally signed. It uses the existing `Cipher` and existing 32-byte deployment key: random 12-byte nonce, AES-256-GCM ciphertext and 16-byte tag, encoded as Base64. No new key or custom cryptographic algorithm is introduced. The authenticated payload includes schema/SQLite SHA-256 proofs and the complete native state fingerprint. Files are created with mode 0600 in a private operator-owned directory; hard-link publication refuses an existing destination. Source tables are locked for a consistent snapshot, and active queue/build states cause refusal. Bounds default to 100,000 rows and 512 MiB of serialized metadata; `--max-rows=N` and `--max-bytes=N` are explicit bounded overrides.

With the existing CLI environment configured and **both controllers/workers actually stopped and fenced**, use:

```text
cbgpu-deploy-control --export-legacy /absolute/private/new.sqlite --confirm-stopped-writers
cbgpu-deploy-control --inspect-reverse /absolute/private/new.sqlite
cbgpu-deploy-control --verify-reverse-current /absolute/private/new.sqlite --confirm-stopped-writers
```

Mutating commands and current-state verification also require `CBGPU_DEPLOY_WRITERS_FENCED=1`, a private controller lock, and closed configured native/legacy listeners. These checks do not replace an operator's verification of orphan/background writers. Keep writers fenced after verification. No command automatically switches listeners or imports a stale original snapshot.

A reviewed legacy rollback can set `CBGPU_DEPLOY_LEGACY_DATABASE` to the **new** file and `CBGPU_DEPLOY_REVERSE_FINGERPRINT` to the fingerprint returned by the fresh verification. Without the database override, the legacy default path is unchanged. An override must authenticate the manifest with the existing key, match the expected fingerprint/artifact root, and match the file hash before SQLite is opened. The original database remains available as a separate historical snapshot.

To restore a reviewed reverse bundle into a provisioned **empty** native database, use `--import-reverse-bundle /absolute/private/new.sqlite --confirm-stopped-writers` with the same fence requirements. Restoration is transactional, includes terminal work history, verifies the exact schema and full state fingerprint, and refuses an existing target or a changed/tampered bundle. Artifact paths are preserved; relocation is not performed implicitly.

On the first explicit legacy selection, the adapter authenticates the immutable reverse bundle and atomically creates a private `<file>.activation.json` receipt using the same Cipher envelope. The receipt binds the approved native fingerprint, initial file hash, canonical path, device and inode. Subsequent legacy restarts validate that receipt and SQLite header rather than rejecting the legacy service's own legitimate database writes. A copied/replaced active database cannot reuse another file's activation receipt; forged receipts and different bootstrap fingerprints are refused. This receipt is an activation boundary, not a backup of subsequent legacy writes; those writes require the normal fenced snapshot/import procedure before returning to native service.
