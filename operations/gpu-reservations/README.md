# Reservation extension activation candidate

Source and disposable tests only until separately approved. No schema migration or
running workload changes are performed by the binary. The existing registry's
anon/service HTTP keys cannot access private reservation tables, so enabling this
feature requires review and explicit approval of `activation.sql`, a dedicated
database credential, and a later registry binary rollout. Do not use a general
`postgres`/`supabase_admin` runtime connection.

The candidate role starts NOLOGIN. Reruns validate existing privilege attributes
and reject unexpected role memberships; they preserve existing LOGIN, password,
expiry, connection limit and per-role default settings. All grants are idempotent
and the complete artifact is transactional. Provision its LOGIN/password privately using
the operator's existing credential procedure; keep its PostgreSQL connection URL
in a regular mode0600 file readable only by the existing registry service user.
Add only `CBGPU_GPU_RESERVATION_DATABASE_URL_FILE=/private/operator/path` to that
service's existing configuration. Preserve its complete current environment,
anon/service key paths, signing keys, feature flags and service identity. Change
only the reviewed registry ExecStart binary. Do not replace its unit with a new
default configuration, rotate node keys, replay enrollment, enable notebooks,
upgrade another controller, or apply `activation.sql` automatically at startup.
Omitting this one setting keeps extension routes unavailable (503).

The role is NOSUPERUSER/NOINHERIT with BYPASSRLS, minimal explicit SELECT columns,
only two end-time UPDATE columns, and existing permission-helper EXECUTE grants.
PostgreSQL17 MAINTAIN on the two reservation tables permits the necessary brief
explicit lock without granting arbitrary owner/status/password UPDATE; MAINTAIN
also permits table maintenance, so that residual capability needs operator review.
It receives no INSERT/DELETE, auth-session UPDATE, password SELECT/UPDATE,
notebook-command/session mutation or private profile access. Public/default grants
remain part of the existing database security boundary; the runtime role is not
a general SQL API and its connection file must remain trusted/private.

The pool has at most two connections, a 3s acquisition timeout, transaction-local
1.5s lock and 4s statement timeouts, and an 8s request transaction bound. An EXCLUSIVE
table lock fences direct PostgREST INSERT/UPDATE as well as existing notebook
launch's SELECT FOR UPDATE. It briefly serializes all writers to that booking
table; public readers continue. No schema replacement, trigger disable or RLS
policy changes are used. PostgreSQL transactions are explicitly READ COMMITTED.

GET `/v1/gpu/legacy/reservations/:id/extension` and
`/v1/schools/:school/gpu/reservations/:id/extension` return
`{reservation_id,current_end,max_end,server_now,can_extend,reason}`. Null `max_end`
means no finite legacy limit; legacy database policy has no duration cap (seven
days is a UI preset). School bookings retain their original starts_at+4h cap.
Finite membership/guest ends and pending/approved next-booking starts clamp the
preview. School `RESERVED` is the blocking state. Every preview requires fresh
owner authentication and current permission; it reserves no space.

POST the same prefix ending `/extend` with exactly `{ends_at,expected_ends_at}`.
The absolute target must increase the current end; an exact end-time CAS and all
policy/conflict/time checks repeat under the lock. A successful response contains
the same fields and `updated:true`. A lost response must be reconciled with GET;
never blindly retry with a newly read CAS. Only end_time/ends_at changes. Identity,
start time, password, booking ID, controller state and notebook jobs are untouched.
No clock or privilege decision is accepted from the browser.

The deployed legacy trigger catalog was reviewed read-only: the only application
UPDATE triggers are `guard_gpu_reservation_write` (validation/overlap) and
`account_v2_execution` (permission gate). Other UPDATE triggers enforce foreign
keys. The admin notification trigger is AFTER INSERT only: extending an existing
row does not send a new-reservation notification or re-run approval. Password
rotation is in the separate `approve_gpu_reservation` RPC, which this adapter
never calls. No new extension-notification workflow is added.

Reasons/errors: `AUTHENTICATION_REQUIRED`/`INVALID_SESSION`401,
`GPU_PERMISSION_REQUIRED` (403), owner-scoped `RESERVATION_NOT_FOUND`404,
`RESERVATION_EXPIRED`, `RESERVATION_NOT_ACTIVE`, `RESERVATION_CHANGED`,
`RESERVATION_CONFLICT`, `EXTENSION_LIMIT`, `EXECUTOR_EXTENSION_UNSUPPORTED`,
`GPU_RESOURCE_UNAVAILABLE`409,
`INVALID_BODY`/`INVALID_WINDOW`400 and
`EXTENSION_UNAVAILABLE`/`RESERVATION_BUSY`/`AUTH_UNAVAILABLE`503.
GET returns inactive/expired/limited/job reasons with can_extend:false at 200.

School reservation-only extension repeats the existing `gpu_resource_state`
READY admission check; revoked, disabled, stale or offline resources fail closed.
School bookings with **any** notebook job fail closed: the current executor has
an immutable signed deadline and hard `/usr/bin/timeout`; changing SQL alone would
not extend it. No session/job restart or tmpfs workspace loss is attempted.
Legacy extension preserves the same booking and workload. Existing streams need
the separately reviewed gateway/controller source fixes to observe a longer
fresh end before cached expiry. A last-instant extension, outage or expired JWT
may still require reconnect; no authority grace beyond verified deadlines exists.

Rollback: remove the optional connection-file setting and roll back only the
registry binary using its normal reviewed procedure. This disables further
extensions; it does not shorten already approved extensions or stop workloads.
Do not revoke a role while a service still depends on it. Any later credential/
role cleanup needs separate operator approval.

Validation: `cargo test -p cbgpu-gpu-enrollment`,
`cargo test -p cbgpu-gpu-enrollment --test reservation_extensions -- --ignored`,
and `cargo clippy -p cbgpu-gpu-enrollment --all-targets -- -D warnings`.
The explicit ignored test starts/deletes only its own disposable loopback Docker
PostgreSQL17 fixture with synthetic users and imports real versioned auth helpers
and the deployed legacy write guard. It never connects to production.
