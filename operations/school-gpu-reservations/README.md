# School-local reservation management candidate

Source only until the operator obtains explicit production migration approval.
Apply only the reviewed `20261123064008_school_gpu_reservation_admin.sql` file;
do not run a general pending-migrations runner. No new tables, indexes, roles,
membership grants, RLS policies, table grants or runtime database credentials are
needed. It adds three private helpers plus one caller-authenticated public RPC
and grants only that RPC's EXECUTE to authenticated. Private helpers are revoked
from PUBLIC/anon/authenticated/service_role. Existing platform approval and
inventory APIs remain unchanged.

Authority is a current confirmed server session and active membership with one
of SCHOOL_OWNER/SCHOOL_ADMIN/GPU_ADMIN in the exact school, following existing
GPU management policy. A platform is_admin bit alone grants nothing here. JWT,
session and membership expiry are checked against clock_timestamp, including
after the mutation lock wait. Frontend metadata never grants school authority.

Legacy rows belong to the existing CBsh resource pool only; other schools cannot
read or change them. Registry rows join their resource/node's actual school.
History keeps actual statuses, including past/rejected/cancelled/revoked rows and
missing profiles. Returned identity is only ID/name/that school's student number;
no email, password, server key, enrollment credential or notebook token is exposed.

GET `/v1/schools/:school/gpu/admin/reservations` supports `limit` (default50,max100),
opaque `cursor`, `status` (`all,pending,approved,rejected,reserved,cancelled,revoked,
active,upcoming,ended`), `q` (max200 characters) and `gpu` (`all`, legacy number or
resource UUID). Response:

```
{school_id,items:[{id,source,status,start_time,end_time,purpose,gpu_id,gpu_label,
owner:{id,full_name,student_number},allowed_actions:[]}],next_cursor,server_now}
```

Cursor order is created_at DESC NULLS LAST, ID DESC, source DESC. It preserves
timestamp precision, reaches null-date history and avoids offset drift during
concurrent new bookings. Source distinguishes the independent UUID namespaces.

POST the same prefix plus `/:id/action` with exactly
`{source,action,expected_status,expected_end_time}`. Response is
`{school_id,reservation:<same-item-shape>,server_now}`. No automatic retries:
reconcile a lost response with fresh GET. Stale status/end is409. Pending legacy
rows with remaining time can be approved/rejected; expired/approved/rejected
rows cannot be reset, reopened or altered by this endpoint. Approval duplicates
the existing platform RPC's atomic status/password pattern without promoting
the school actor. Existing overlap triggers/exclusion remain authoritative.
Approval rejects with409 GPU_CURRENTLY_IN_USE while any other approved booking
on that GPU is active, even for a nonoverlapping future request, preserving the
active user's credentials. Rejection never rotates passwords.

Registry RESERVED bookings with remaining time support cancel/revoke only, never
approval. If a notebook job exists, its existing stop helper queues STOP_NOTEBOOK,
sets STOPPING and revokes only that job's sessions. The UI must explicitly warn
that this can stop active work and discard temporary workspace files. It does
not recreate containers, reset a GPU, change node keys or launch work. Source
tests exercise this in a disposable database only; no real booking was changed.

## Backup and read-only inventory before approval/application

Run under the operator's trusted existing database administration procedure.
The following commands are preparation examples; no file below is public.
Choose a new private backup directory; never print dumps or include them in PRs.

```sh
umask 077
backup_dir=/home/cbgpu/.local/share/cbgpu-runtime-audit/school-gpu-before-APPROVED_TIMESTAMP
mkdir -m 700 -- "$backup_dir"
docker exec cbgpu-supabase-db pg_dump -U postgres -d postgres --format=custom > "$backup_dir/database-before.dump"
docker exec cbgpu-supabase-db pg_dump -U postgres -d postgres --schema-only --no-owner > "$backup_dir/schema-before.sql"
systemctl --user cat cbgpu-gpu-registry.service > "$backup_dir/registry-unit-before.txt"
cp --preserve=mode,timestamps -- /home/cbgpu/.local/lib/cbgpu-rust/releases/gpu-extension-9eb3724508dc973664a1158f39c9996bf5762ada/cbgpu-gpu-enrollment "$backup_dir/registry-before"
```

Save the existing registry executable separately using its actual configured
path; do not guess that path or change ExecStart arguments/environment. Preserve
its existing anonymous/service/signing key paths, reservation extension pool,
feature flags and service identity. Only the reviewed registry executable needs
replacement; no controller/SSH service/GPU container restart is required or
authorized by this document.

Read-only catalog inventory (no user rows or secrets):

```sql
BEGIN READ ONLY;
SELECT table_schema,table_name,column_name,data_type FROM information_schema.columns
WHERE (table_schema='public' AND table_name IN('reservations','gpus'))
   OR (table_schema='cbgpu_schools' AND table_name IN('memberships','audit','gpu_resources','gpu_nodes','gpu_resource_reservations','gpu_notebook_jobs'));
SELECT to_regprocedure('public.school_gpu_reservation_admin(text,text,jsonb)'),
       to_regprocedure('cbgpu_schools.has_role(uuid,text,text[])'),
       to_regprocedure('cbgpu_schools.gpu_notebook_permission(uuid,text,timestamp with time zone,timestamp with time zone)'),
       to_regprocedure('cbgpu_schools.gpu_notebook_stop(uuid,text)');
ROLLBACK;
```

Before applying, confirm the migration owner has the same private schema/table
authority as existing school RPC owners. Apply only this reviewed file after
backup and explicit approval, then refresh PostgREST's schema cache using the
existing operator procedure. Never grant the frontend or registry general SQL
access. The registry uses its existing caller Bearer + anonymous key; service
credentials are not used for these routes.

## Rollback

Disable the new UI entry, restore only the previous registry binary and its
unchanged configuration. If schema rollback is approved, drop only these new
functions (no CASCADE):

```sql
BEGIN;
DROP FUNCTION IF EXISTS public.school_gpu_reservation_admin(text,text,jsonb);
DROP FUNCTION IF EXISTS cbgpu_schools.gpu_admin_booking_item(text,jsonb);
DROP FUNCTION IF EXISTS cbgpu_schools.gpu_admin_booking_rows(text);
DROP FUNCTION IF EXISTS cbgpu_schools.gpu_admin_authorized(text);
COMMIT;
```

Refresh the schema cache afterward. Do not automatically revert reservation
decisions, passwords, audit records or already requested notebook stops: those
are explicit operator actions, not migration rollback state.

Validation: full cbgpu-gpu-enrollment tests/clippy plus the explicit ignored
`school_admin_reservations` PostgreSQL17 test. CI runs that disposable fixture.
It applies the migration twice, imports real existing role/session/resource
helpers and legacy guard, tests same-school all-user history, cross-school
denial, platform-only denial, stale/revoked authority, CAS, password atomicity,
busy-GPU preservation, source-aware pagination and notebook stop semantics.
