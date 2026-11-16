# Scoped contest tokens — phase-two candidate

This is separate, default-off source. It does not change the current JWT contest
service, install a public route, apply SQL, or activate automation credentials.
The new migration and execution login require separate review and approval.

Tokens delegate only participation actions for explicitly selected existing
contests. They cannot create/manage contests, impersonate a supplied owner,
read another person's entry, or access an AI/GPU API.

| Scope | Capability |
| --- | --- |
| `contests:read` | Allowlisted contest/form metadata and the owner's registration |
| `contests:register` | Existing individual/team registration RPC for allowlisted contests |
| `contests:cancel` | Cancel only the owner's registration for allowlisted contests |

Expiry defaults to 30 days and is bounded to 1–90 days. Each grant also belongs
to the real Auth session used to mint it: signing out/revoking that session,
account deletion, bans, suspension, or session expiry disables the token
immediately. Ordinary access-JWT refresh does not require replacing a token
while its parent session remains active. Maximum 20 active tokens per owner.

The API generates 256 random bits with the `cbgpu_ct_` prefix and returns the
secret **once**. PostgreSQL stores only SHA-256, not plaintext. Lists expose
metadata, scopes, allowlisted contest IDs, and expiration/revocation timestamps;
they omit hashes and parent session identifiers. Neither token values nor
request credentials are logged by this service.

## Candidate contract

All native requests require the trusted proxy's `x-cbgpu-contest-key`. This key
must never be distributed to integrations. Ambient cookies and owner headers
are rejected. Direct browser Origins are rejected; a reviewed thin public
proxy must handle external Bearer-only CORS and routing.

JWT-only lifecycle:

- `POST /v1/tokens` accepts exactly `name`, `scopes`,
  `allowed_contest_ids`, and optional `expires_in_days`.
- `GET /v1/tokens` lists only the verified account's metadata.
- `DELETE /v1/tokens/{id}` revokes only a token owned by that verified account.

```json
{
  "name": "School integration",
  "scopes": ["contests:read", "contests:register"],
  "allowed_contest_ids": ["60000000-0000-4000-8000-000000000001"],
  "expires_in_days": 30
}
```

Mint success is `{"token":"cbgpu_ct_...","record":{...},"shown_once":true}`.
A personal contest token and an AI key cannot authenticate these lifecycle
routes. Use a genuine CBGPU access JWT; no passwords or refresh tokens are
collected or stored.

PAT-only delegation uses the existing registration route shapes:

- `GET /v1/contests`, `GET /v1/contests/{id}`, and `GET .../{id}/form`
- `POST /v1/contests/{id}/registrations`
- `POST /v1/contests/register` with an additional `contest_id`
- `GET` or `DELETE /v1/contests/{id}/registrations/me`

Mutation responses include only `id`, `contest_id`, `status`, and
`participation_kind` unless the grant also has `contests:read`. A cancel-only or
register-only token cannot use its write response to read answers, QR ticket
codes, or identity/profile fields.

Registration JSON retains `attendee_name`, `answers`, `kind`, optional
`team_name`, and optional UUID `join_code`. Unknown identity/status fields are
rejected. Team invitation codes remain supplied through the existing team
workflow; this candidate does not introduce a team-management API.

## Private database boundary

The Supabase-CLI-created candidate migration is
`20261116131013_contest_scoped_automation_tokens.sql`. It creates only private
`contest_api` data/functions, a no-login, non-superuser, non-bypass execution
group, and a separate non-login policy-reader helper role. It does not alter accounts, passwords, sessions, projects, or contest
ownership. Ordinary `anon` and `authenticated` roles receive no access.

The backend group receives six narrow function grants, no raw table grants,
no membership in `authenticated`, and no direct registration-RPC grant.
The policy-reader role inherits ordinary `authenticated` lookup permissions,
owns only fixed read helpers with RLS explicitly enabled, and has no direct
contest grant beyond that inherited permission. The backend is not its member
and cannot call those helpers. Minting checks actual caller visibility for
every allowlisted ID. Detail, form, list, and registration recheck current RLS
on every use, including visibility changes after minting. Existing unlisted
contests remain directly readable under current policy; a newly hidden row is
not disclosed. Own-registration read and cancellation remain available even
when contest metadata becomes inaccessible.

Internal helpers are not executable by the backend group. Token validation checks
expiration, revocation, scope, allowlisted contest, parent session, account,
and profile freshly inside the same database operation. Session/profile share
locks and token row locks serialize revocation with delegated actions.

The owner is derived exclusively from the stored grant. The registration
wrapper calls the **same** `register_contest_entry` function; cancellation uses
an owner-bound update and the existing triggers. Question validation,
capacity/waitlist promotion, approval, duplicate/team/start/deadline rules are
preserved. No service-role key or fabricated Supabase JWT is used. When the
separately approved account-resource oracle exists, registration uses it
rather than granting blanket school access from legacy approval.

## Separate operation

Startup requires explicit `CBGPU_CONTEST_TOKENS_ENABLED=1`. Default listen is
`127.0.0.1:13545`; the current JWT service on 3545 stays unchanged.

- `CBGPU_CONTEST_TOKENS_DATABASE_URL`: dedicated write-enabled login inheriting
  only the private token execution group.
- `CBGPU_CONTEST_AUTH_DATABASE_URL`: the existing separate read-only Auth login.
- `CBGPU_CONTEST_TOKENS_INTERNAL_KEY_FILE`: private proxy-origin key file.
- `CBGPU_CONTEST_ANON_KEY_FILE`, `CBGPU_CONTEST_BACKEND_URL`: existing local Auth
  configuration used solely to verify JWT-only lifecycle requests.
- `CBGPU_CONTEST_TOKENS_LISTEN`: optional loopback candidate address.

The service refuses a shared Auth/execution login, an execution connection in
read-only mode, elevated/bypass roles, raw Auth/token/registration privileges,
or missing any of the six private function grants. Startup also verifies
the two policy helpers still belong to the isolated reader with RLS enabled
and an empty search path; a no-owner restore or privilege drift fails closed.
It never disables the JWT reader's
read-only setting. Store execution functions use their own transactions and
transaction-local claims; these claims are cleared when the transaction ends.

No credentials, environment file, production role, public proxy route, or
service unit is created by this crate. Existing Auth and JWT service source
remain unchanged. Keep all credential files private and out of Git.

## Verification

```bash
cargo test --manifest-path rust/Cargo.toml -p cbgpu-contest-tokens
cargo clippy --manifest-path rust/Cargo.toml -p cbgpu-contest-tokens --all-targets -- -D warnings
CBGPU_CONTEST_REPO="$PWD" CBGPU_CONTEST_TOKEN_TEST_DATABASE_URL=postgresql://fixture:fixture@127.0.0.1:5432/cbgpu_contest_tokens_test cargo test --manifest-path rust/Cargo.toml -p cbgpu-contest-tokens --test postgres -- --ignored
```

Only a new disposable PG17 database with the exact test name is accepted.
The fixture applies both private migrations twice, enables real RLS, uses the
EXECUTE-only role, and verifies no raw Auth/token/registration access. It tests
hash-only credentials, owner isolation, safe projections, token/session
expiry and revocation, profile changes, changed-RLS visibility and retained own-entry access, legacy and future resource policy,
question/duplicate/team/full/waitlist/start/cancel behavior, and unchanged
account IDs/password hashes and checked-in SQL source bytes. HTTP fixtures
verify JWT-only minting, one-time output, strict payloads, and identity/key
confusion rejection. No live users or registrations are used.
