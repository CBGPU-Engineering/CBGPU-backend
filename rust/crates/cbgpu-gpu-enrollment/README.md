# Optional school GPU inventory

Source candidate, disabled by default. This module enrolls a school's own NVIDIA
machines and accepts signed outbound inventory. It does not run jobs, install
NVIDIA drivers, modify existing GPU controllers, restart notebooks, enable sharing
or grant compute permissions. It is not a distributed inference implementation.

## Security and storage

A fresh school `SCHOOL_OWNER`, `SCHOOL_ADMIN` or `GPU_ADMIN` can mint a 10-minute,
one-use enrollment token. A platform administrator without one of those school
roles has no implicit permission. Only a SHA-256 hash is stored. Redemption checks
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
