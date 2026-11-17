# Native school directory and tenant onboarding

Source candidate; `CBGPU_MULTISCHOOL_V1` defaults off. The private Axum service
binds `127.0.0.1:3546` and forwards verified caller Bearer credentials to narrow
PostgreSQL functions. It never accepts a caller-supplied owner or uses a service
role key. Configure `CBGPU_SCHOOLS_ANON_KEY` and, optionally,
`CBGPU_SCHOOLS_BACKEND_URL` (local Auth/PostgREST origin).

The candidate implements approved-school selection, pending memberships, resumable
six-step applications, independent platform approval, scoped administration,
feature requests, school/nationwide messaging, and explicit project publication.
Existing user IDs, passwords, sessions, project ownership and files are preserved.
The directory importer normalizes the official NEIS high-school dataset; listing
is not approval and grants no administrator, feature or compute access.

Run `cargo test -p cbgpu-schools` for native HTTP and importer/template tests.
`cargo test -p cbgpu-schools --test postgres -- --ignored` creates and removes an
isolated PostgreSQL17 fixture with synthetic users; it never sends email.
`schema-dry-run --clone-and-verify SOURCE_CONTAINER NEW_PRIVATE_DIRECTORY REPO`
takes a read-only private backup and tests the migrations twice in a same-image,
network-isolated disposable clone. It verifies immutable records, real Auth signup
triggers, all historical affiliations, and the official directory import.

`school-mail` is separately disabled unless `CBGPU_SCHOOL_MAIL_ENABLED=1`.
It requires a dedicated EXEC-only database login and the existing Resend settings.
Outbox `SENT` means provider accepted, not guaranteed inbox delivery. Leased retries
stop before the provider's idempotency window expires. No provider call is made by
the tests.

Before activation, review all candidate migrations, provision narrowly scoped
service credentials, and finish legacy resource consumers. In particular, a scoped
CBsh membership approval does not yet update the primary legacy profile approval
used by the old GPU daemon. Other-school compute/fabrication execution is denied
until actual tenant-aware runtime support is installed. Existing historical student
number duplicates are preserved and audited; new verified claims are serialized
and checked. No academic rollover or resource grant is executed by this crate.

School-mail runtime credentials may be supplied with
`CBGPU_SCHOOL_MAIL_DATABASE_URL_FILE` and `RESEND_API_KEY_FILE` instead of plaintext
environment values. Files must be regular, private files; choose exactly one
source per secret. This supports user-scoped systemd encrypted credentials with
`LoadCredentialEncrypted`. The sender should be `CBGPU <info@cbgpu.com>`; the
recipient is fixed to `info@cbgpu.com`. Notifications link to the protected
platform request inbox.

`school-mail --check` validates private configuration and the actual EXEC-only
PostgreSQL role without claiming a job or contacting Resend. Permission checks
resolve table OIDs through system catalogs, so they require no access to the
`auth` schema. The worker rejects inherited authenticated/service roles and raw
auth/profile/outbox/membership/request table or column privileges. SIGTERM drains
the current bounded request and prevents a subsequent claim.

Verification on 2026-11-17 included the isolated real PostgreSQL/GoTrue/PostgREST
school HTTP stage: an actual feature request produced a durable outbox intent,
a dedicated EXEC-only login claimed it, a loopback provider stub accepted two
retries with the same idempotency key, and the real acknowledgement removed it
from the pending queue. This is **not** verification of actual provider acceptance
or inbox delivery. No provider was contacted. The ignored stage-only mail test
requires a private synthetic loopback DSN file and a pending synthetic intent;
it is separate from ordinary CI tests.
