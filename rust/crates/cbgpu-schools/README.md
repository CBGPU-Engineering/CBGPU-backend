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

## Central CPU source candidate

`CBGPU_CENTRAL_CPU_V1` is a separate strict boolean flag and defaults off. It
adds platform-admin-only `GET/PATCH /v1/admin/central-cpu-pool` and
`GET/PATCH /v1/admin/schools/:id/central-cpu`. Both require explicit confirmation
and revision checks. The global pool starts disabled with zero limits; school
approval, school ownership and an allocation alone never create host capacity.
`GET /v1/central-cpu-access?purpose=frontend|compose|latex` reads the account's
saved school and returns fresh permission reasons. Accounted lease reservations
are not measurements of physical CPU/RAM/disk utilization.

Migration `20261117154700` is an additional candidate after the operating47;
preparation does not apply it or enable execution. Existing `school_compute`,
GPU/AI predicates and old NULL execution-mode rows keep their legacy meanings.
New unfunded projects use `UNASSIGNED_V1`, retain their ID and personal metadata,
and cannot execute. The trusted runtime can explicitly attach a never-executed
project to an approved school allocation once; `CENTRAL_V1` school/owner binding
is immutable. Compose binding is checked in the database as well as in the
runtime. No browser-provided owner, mode switch or default CBsh school is trusted.

A verified internal member, or an independently approved temporary guest with
an active school-scoped `school_compute` grant and eligible sponsor, still needs
the separate allocation, enabled module/purpose and global capacity. Central
guest context is a derived `central_cpu` capability, never school membership,
administrator access or a widening of the old School-B `school_compute` oracle.

The service-only CPU RPC reserves durable hashed-nonce leases under a global
then school lock. No age/heartbeat/TTL frees capacity. Proven runtime retirement
keeps storage/project accounting (`STORAGE_RETAINED`) while releasing CPU/RAM;
redeploy credits retained storage rather than counting the same persistent bytes
twice. Only proven project-data removal can release that persistent reservation.
Unknown or unreconciled workloads remain counted.

A first manual start requires the submitting actor's fresh Auth session and
project authority. A started owned service uses its narrow lease authority and
fresh owner/allocation checks; browser logout or editor removal does not stop
it. Owner-enabled frontend Git automation uses a separate project/school-bound,
revocable hashed secret, with fixed build/runtime ceilings, stored only in the
runtime's encrypted private journal. It never mints or borrows an expired user
JWT. Revocation blocks new automation while an already-started owned site follows
its owner's resource permission. LaTeX has a separate EXEC-only wrapper limited
to its own typed purpose; no arbitrary shell or generic executor access is added.

The full-schema clone runs default-off, privacy, unfunded-draft, guest, quota,
retirement and delegation SQL contracts. The isolated two-school race rehearsal
uses a real PostgreSQL17 global cap: simultaneous individually affordable jobs
must admit only one when their combined budget exceeds that cap. This is source
and isolated-runtime evidence, not an assertion that central CPU has been enabled
on the operating host or that unrelated legacy/AI workloads are accounted here.

School/nationwide community posts preserve every former channel message and ID.
The additive threaded-replies migration leaves old `parent_id` values NULL and
never changes a body's visibility. `GET /v1/messages` returns the latest root posts in the existing chronological wire
order (new community clients may sort the feed newest first); each DTO has `parent_id` and a viewer-filtered `reply_count`.
`GET /v1/messages/:id` returns `{message}`. `GET /v1/messages/:id/replies` accepts
bounded `limit`/`offset` and returns chronological `{items,total}`.
`POST` on that replies route accepts only `body` and a required UUID
`client_message_id`; the server derives scope/school from its live root and
returns `{message,duplicate}`. Comments cannot nest. Existing delete/report/mute
endpoints also cover replies. Deleted/muted/inaccessible parents do not expose
comments, and owner cleanup remains available after affiliation loss. DM storage
and opt-in are separate. No old school or nationwide content becomes public.
