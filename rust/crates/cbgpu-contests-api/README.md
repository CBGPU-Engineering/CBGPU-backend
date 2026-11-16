# Native contest registration API

Rust/Axum adapter for the existing contest registration RPC and RLS policies.
This API registers participants; it does **not** create or manage contests.
It does not accept AI inference keys, owner IDs, passwords, or ambient cookies.

Public base: `https://cbgpu.com/api/ai/v1/contests`.
Native origin: `http://127.0.0.1:3545/v1/contests`.
The thin web proxy supplies the private origin key and forwards the caller's
Bearer token. It never selects an owner or performs business writes.

| Method | Public path after `/api/ai/v1/contests` | Result |
| --- | --- | --- |
| GET | empty; optional `?limit=50&offset=0` | Public, published contest list under caller RLS |
| GET | `/{contest_id}` | Contest details and registration settings |
| GET | `/{contest_id}/form` | Questions, team settings, windows, and registration endpoint |
| POST | `/{contest_id}/registrations` | Register using the existing database RPC |
| POST | `/register` | Same registration; also include `contest_id` in the body |
| GET | `/{contest_id}/registrations/me` | The authenticated account's registration or `null` |
| DELETE | `/{contest_id}/registrations/me` | Cancel only the authenticated account's entry |

All requests require `Authorization: Bearer <CBGPU access JWT>`. For an
integration endpoint variable, use
`CBGPU_CONTEST_ENDPOINT=https://cbgpu.com/api/ai/v1/contests/register`.
POSTing to the bare list path does not create a contest or a registration.

```json
{
  "contest_id": "60000000-0000-4000-8000-000000000001",
  "attendee_name": "Participant name",
  "answers": {"question-id-from-the-form": "Answer"},
  "kind": "individual"
}
```

Fetch the real form first. `kind` also supports `team_create` with `team_name`,
or `team_join` with a UUID `join_code`. Individual entry into a team contest is
allowed only when its host enabled that option. Unknown identity/status fields
are rejected. JSON bodies are bounded to 256 KiB.

Required answers, approval, capacity, waitlist, duplicate registration, team
limits, cancellation, and opening/closing times stay enforced by the existing
PostgreSQL functions/triggers. No service-role contest write or alternate
registration implementation is used. Registration responses contain
`{"registration": {...}}`; failures contain `{"error":{"code":"...","message":"..."}}`.
A retry after a lost response may receive `already_registered`; query `/me`
to reconcile rather than making a second identity.

## Authentication and operation

GoTrue verifies the actual JWT on every request. The API then calls the private
`cbgpu_contests_auth.session_status` function in a read-only transaction with
transaction-local **verified** subject/session claims. This rejects signed-out,
expired, banned, deleted, or suspended identities even if their JWT has not yet
expired. It needs no raw Auth/profile table privileges or RLS bypass.
Legacy approval is required to register; an active unapproved account can still
read or cancel its own entry. Account-v2 contest resource enforcement is used
only when its separately approved feature flag is enabled.

| Variable | Value/purpose |
| --- | --- |
| `CBGPU_CONTEST_INTERNAL_KEY_FILE` | Private proxy-origin key, at least 32 bytes |
| `CBGPU_CONTEST_AUTH_DATABASE_URL` | Dedicated private login inheriting only `cbgpu_contests_auth_reader` |
| `CBGPU_CONTEST_ANON_KEY_FILE` | Existing local public anon key |
| `CBGPU_CONTEST_BACKEND_URL` | Local GoTrue/PostgREST origin; default `http://127.0.0.1:54321` |
| `CBGPU_CONTEST_LISTEN` | Default `127.0.0.1:3545`; non-loopback binds refused |
| `CBGPU_CONTEST_ALLOWED_ORIGINS` | Comma-separated exact HTTPS origins for direct trusted native browser clients |
| `CBGPU_ACCOUNT_V2` | Keep disabled until the separate account migration is approved |

The native process requires `x-cbgpu-contest-key` from the trusted thin proxy.
Public clients must never receive this key. The public proxy uses explicit
Bearer authentication and no credentialed-cookie CORS. Server clients omit
`Origin`; direct browser clients are restricted to configured exact origins.
Verified-subject limits are 120 reads and 20 mutations per minute, with bounded
concurrency. No rate-limit identity is taken from forwarded IP/owner headers.

Example reviewed user service (replace the executable path before installation):

```ini
[Unit]
Description=CBGPU native contest registration API
After=network-online.target
[Service]
ExecStart=/absolute/reviewed/path/cbgpu-contests-api
EnvironmentFile=%h/.config/cbgpu/contest-api.env
Restart=on-failure
RestartSec=5
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
MemoryMax=256M
TasksMax=64
TimeoutStopSec=15
[Install]
WantedBy=default.target
```

Keep the environment and key files private (`0600`). The executable never
applies migrations or provisions its own database permissions. The reviewed
session-oracle migration and least-privilege login must be provisioned
separately. No production contest registration is needed for deployment checks.

Use the existing CBGPU Auth client for login/session refresh. The existing
GoTrue token endpoint supports its normal refresh flow; do not collect account
passwords in third-party integrations. Scoped personal automation tokens are a
separate future capability, not an alias for AI keys or an undocumented JWT
minting mechanism. Their design must restrict contests/actions, expiration,
revocation, and fresh account status before any delegated registration write.

## Verification

```bash
cargo test --manifest-path rust/Cargo.toml -p cbgpu-contests-api
cargo clippy --manifest-path rust/Cargo.toml -p cbgpu-contests-api --all-targets -- -D warnings
CBGPU_CONTEST_REPO="$PWD" CBGPU_CONTEST_TEST_DATABASE_URL=postgresql://fixture:fixture@127.0.0.1:5432/cbgpu_contests_api_test cargo test --manifest-path rust/Cargo.toml -p cbgpu-contests-api --test postgres -- --ignored
```

The PostgreSQL fixture requires a new disposable PG17 database with the exact
name above. It enables real RLS, applies the exact session-oracle SQL twice,
checks denied direct reads/writes and ordinary-role execution, then reuses the
existing event/team/start-window SQL contracts. HTTP fixtures inspect actual
caller-JWT RPC requests and reject identity spoofing, inference keys, stale
sessions, unsupported actions, invalid origins, and oversized bodies. No live
user or contest data is used.
