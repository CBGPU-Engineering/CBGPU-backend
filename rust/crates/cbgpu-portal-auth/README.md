# Native portal authentication candidate

Rust authentication/session core for the existing developer and admin portals. This is **disabled by default** and does not replace the running Node portal, operations APIs, SQL editor, equipment authentication, or terminal connections.

The core preserves the existing `__Host-cbgpu-developer` cookie, eight-hour lifetime, HMAC/version derivation, AES-GCM token envelope, CSRF values, and exactly two configured developer identities. Every authenticated request checks a private PostgreSQL principal oracle against the actual Auth session, current account restrictions, platform administrator role, and current developer roster. Scoped mode does not confuse school approval with an explicit platform administrator role. No system password, MFA requirement, token refresh, password reset, or global logout is added. A definitive account/role/session revocation deletes only the affected private portal session and records it atomically; restoring the role cannot resurrect that cookie. Temporary authority/roster lookup errors deny access without deleting sessions.

The only native HTTP routes are session, login, logout, and the existing admin session aliases. Other operations explicitly return unavailable; they are not forwarded to a hidden TypeScript worker. The four-second monitoring utility fails closed within a normal five-second check cycle, but no existing terminal has been handed over to it.

## Configuration and staged handoff

`CBGPU_PORTAL_NATIVE_AUTH` defaults off. `CBGPU_ACCOUNT_V2` and `CBGPU_MULTISCHOOL_V1` accept only empty/0/false or 1/true; malformed values abort. The listener is loopback only, default `127.0.0.1:3551`.

Private regular configuration files must have mode 600 on Unix:

- `CBGPU_PORTAL_SETTINGS_FILE`: existing portal settings with the original `sessionSecret` and two `developers` entries.
- `CBGPU_PORTAL_STORE_DATABASE_URL_FILE`: dedicated private runtime database; the runtime login must be allowed to `SET ROLE cbgpu_portal_session_store`.
- `CBGPU_PORTAL_AUTH_DATABASE_URL_FILE`: private principal-reader login allowed to `SET ROLE cbgpu_portal_auth_reader`; no raw Auth table grants.
- `CBGPU_PORTAL_ANON_KEY_FILE`: public Auth API key, stored privately for consistent configuration handling.

`CBGPU_PORTAL_AUTH_ORIGIN` defaults to `http://127.0.0.1:54321/`. Operator-configured internal service DNS requires `CBGPU_PORTAL_NETWORK_MODE=private-container`. Redirects and proxy environment inheritance are disabled. No service key is sent to GoTrue.

Explicit `--bootstrap-store` creates only the dedicated runtime schema and refuses a database containing `auth.users`. Apply the separately reviewed backend migration `20261117092500_portal_principal_authority.sql` before any proposed auth handoff; startup does not apply it. `--import-snapshot PATH` reads a private SQLite snapshot without changing the source. It preserves session IDs, ciphertext and CSRF, imports into an empty target transactionally, and records a fingerprint. Repeating an identical import never resurrects a session deleted after import. Unhandled audit, job and SQL log tables are reported and retained in SQLite.

**Activation is not complete:** snapshot import is not online replication. A reviewed write fence/final snapshot and routing transition must precede native authority; otherwise Node and Rust would have divergent session stores. Existing static UI, admin aliases, WebSocket authentication/monitoring, SQLite logs and operations must be verified together before switching. Do not restart the operating controller or terminate active terminals as part of source validation.

The proxy binding helper accepts a newer caller bearer only when subject and Auth session ID match the independently authenticated portal session. Decoding is only a binding check: GoTrue/Kong must still verify the bearer signature. It never borrows another user/session token or refreshes a token family.

## Validation

```text
cargo test -p cbgpu-portal-auth
cargo clippy -p cbgpu-portal-auth --all-targets -- -D warnings
cargo test -p cbgpu-portal-auth --test postgres -- --ignored
```

The explicit PostgreSQL test owns a network-none disposable PG17 container with synthetic accounts, no host TCP port and no operating data. Contract tests include offline Node crypto vectors, CSRF/cookie scope, strict flags, scoped administrator continuity, fresh caller bearer binding, fake GoTrue transport, and unavailable-authority monitoring. The full same-image backend schema clone separately verifies private principal ACLs, revocation and original account/project preservation. No real passwords, emails, test users, notifications or production writes are required.
