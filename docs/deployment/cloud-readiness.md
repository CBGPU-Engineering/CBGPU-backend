# Cloud readiness — source candidate, no migration performed

The current school service stays on its existing computer. This change supplies
an offline-synthesizable AWS foundation, a tested non-root container for an
existing Rust API, and an override for the **existing Supabase Storage S3
backend**. No AWS resources, accounts, credentials or DNS records were created. No
production containers or data were moved, restarted or replaced; only isolated
local review containers were created and removed.
It does not certify that the whole system can already move to AWS.

## Current boundaries

| Class | Existing component / state | Cloud treatment |
| --- | --- | --- |
| App | Next.js website and trusted-host auth broker; immutable web releases | Keep existing design, host allowlists, HttpOnly cookie names and tenant request/response stripping. Container/CDN cutover is separate. |
| App | Rust accounts, school API, contest API and other native candidates | Package exact reviewed binaries; preserve caller JWT and fresh server/DB permission checks. A source candidate is not evidence of a live handoff. |
| Data | Supabase PostgreSQL `17.6.1.136`, Auth users/sessions, RLS, project ownership | First candidate is the **same self-managed PostgreSQL image and roles**, with compatible persistent block storage and tested backup/restore. No RDS compatibility claim. |
| Data | Live deployment/AI SQLite state plus local artifact/project directories | Unportable until the separate verified PostgreSQL import/handoff and artifact manifests succeed. Do not start two independent writers or discard SQLite. |
| Data | Supabase Storage API `v1.81.0` with current file backend | Existing S3 adapter can be configured for a private bucket; bytes, object keys, metadata, RLS, signed URLs and file access must pass a migration rehearsal first. |
| Infra | Cloudflare routes/tunnel, trusted domains, custom frontend/server tenant gateways | Keep TLS, host boundaries, cookie stripping, WebSocket filtering and revocation. No tunnel token or Docker socket belongs in a public application container. |
| External | Email provider, Firebase FCM, GitHub, school EduSet/NEIS/weather services | Preserve private credentials and exact callbacks; verify provider networking and delivery separately. No emails/pushes/school submissions were sent here. |
| School GPU | Existing root GPU controller, approved reservations, Jupyter kernels and local volumes | Remain at the school. Optional signed inventory enrollment is separate, private by default and does not execute jobs or enable sharing. |

The web source now accepts `CBGPU_ANDROID_RELEASE_PATH` for Android release
metadata, with a repository-relative `public/downloads` default. Existing host
releases keep their canonical download directory through the delivery agent's
runtime environment. Update that agent before deploying the changed web source;
an older agent does not persist the new setting. Verify the APK hash and update
manifest together. The container mounts completed downloads read-only.

The web candidate has a digest-pinned multi-stage Next.js standalone image and a
non-root distroless runtime. Its loopback-only Compose entry supplies runtime
configuration, persistent cache and read-only published downloads. A baked,
licensed Chromium bundle supports existing school connection automation without
executable `/tmp` or inherited platform credentials. Browser subprocesses receive
an allowlisted environment. No human-verification bypass is supplied.

`CBGPU_BACKEND_PROXY_URL` switches the internal Auth/REST/Storage/Realtime/
Functions upstream without rebuilding the public-domain artifact. Unknown
services, path traversal and invalid internal addresses fail closed. Trusted-host
Storage response sandbox/CSP and tenant HTTP/WebSocket credential filtering
remain required. `CBGPU_SESSION_ORACLE_MODE=rpc` verifies the caller's fresh
session with the existing self-only database RPC using the public API key; no
Docker socket or service-role key is required for that session check. Do not
activate it before installing and testing the corresponding schema. The legacy
host compatibility mode is retained until the approved migration.

An isolated same-image test has exercised two synthetic upstream configurations,
HTTP, SSE, WebSocket upgrade, release metadata and completed APK delivery with a
read-only filesystem, UID 65532 and no external network. Offline Chromium launch
also passed with non-executable temporary storage. These checks do not certify
all authenticated application journeys, browser vulnerability remediation or AWS
network connectivity. Rebuild and rescan the final source artifact before release.

The live database image was inspected read-only:

```text
supabase/postgres:17.6.1.136
sha256:f371b5f3f2ac0a05703f33d6e6134515fb2498cab708fb948a0aeb7481467c00
```

The operating role model includes `supabase_admin` with SUPERUSER while
`postgres` is not SUPERUSER; Auth/Storage and application objects have their own
owners and grants. Restoring only table data under a generic RDS owner is **not**
a compatible restoration. RDS requires a separate extension, owner, privilege,
SECURITY DEFINER, role and migration audit/rehearsal. This CDK stack intentionally
creates no RDS database or data host and makes no automated restore promise.

## What the AWS source creates on synthesis

`infra/cloud` uses pinned TypeScript AWS CDK packages. `npm run synth` is offline
with lookups/version reporting disabled; it creates local CloudFormation output.
It does not deploy it. The template contains:

- Private isolated VPC subnets and an S3 gateway endpoint, without NAT/public ingress.
- Private, encrypted, versioned S3 storage with retention and denied non-TLS access.
- An immutable-tag ECR repository and an ECS cluster.
- A **task definition only** for the real school API, disabled by default, digest-pinned,
  non-root, read-only and capability-dropped. There is no ECS Service or running task.
- A separately scoped Storage API task role for the one storage bucket. Normal
  object deletion is allowed; deleting historical object versions is not granted.
- References to an existing Secrets Manager ARN, never plaintext secret values.

Before any task is activated, private connectivity to the restored TLS backend,
ECR, logs and Secrets Manager must be configured and tested. A private subnet
without NAT/endpoints is not magically able to pull images or reach providers.
The template creates no public hostname, certificate, ALB, database or production
workload; those need a separate reviewed deployment after data compatibility.

## Existing Storage API and S3

`infra/cloud/compose.storage-aws.yml` overrides the existing pinned
`supabase/storage-api:v1.81.0` service rather than introducing a mock object store.
It keeps the real PostgREST/database/Auth configuration and changes only the
storage backend/endpoint/bucket/region. It uses HTTPS virtual-host addressing and
clears inherited static AWS keys so the SDK default credential chain can use an
ECS task role or instance profile. Do not combine the MinIO override with it.

The pinned upstream adapter constructs the real AWS SDK `S3Client` without an
explicit credentials object, allowing the standard provider chain:
[Supabase Storage v1.81.0 S3 adapter](https://github.com/supabase/storage/blob/v1.81.0/src/storage/backend/s3/adapter.ts).
The existing configuration is documented in
[Supabase self-hosted Storage configuration](https://supabase.com/docs/guides/self-hosting/storage/config).
An IAM task role still needs a real AWS integration test; local Compose parsing
cannot prove AWS permission, multipart, checksum or bucket behavior.

Before switching storage:

1. Take database and file/object backups and hash manifests; preserve the previous backend.
2. Rehearse on a restored isolated database with the same users, roles and Storage version.
3. Copy bytes with their exact tenant/bucket/object layout and compare counts/sizes/hashes.
4. Test owners, project collaborators and unauthorized users through the **Storage API**,
   including upload, download, signed links, multipart uploads, overwrite and delete.
5. Freeze/drain only the affected writer at the approved final cutover, synchronize
   the last delta and verify metadata/object consistency before reopening writes.
6. Keep a tested rollback path. Do not remove local files after a configuration-only check.

## Checks run and limits

The native school API image was built from an allowlisted source-only context
using digest-pinned Rust and distroless bases. A disposable container passed its
real `/health` probe with UID `65532:65532`, read-only root filesystem, network
`none`, no published ports, no mounts and synthetic credentials. No production
container was touched. The context exporter excludes runtime databases, `.env`,
private directories, uploads, symlinks, build output and signing/credential files.

CDK assertions check that no Service/database/instance/DNS/public-ingress resource
is synthesized, and check the retained/private/versioned bucket and disabled
non-root task. The Storage override was parsed against the actual base Compose
with a private synthetic env file; no real `.env` was loaded and no storage
container or AWS bucket was started. These are source/configuration/container
checks, **not** an AWS migration, RDS restore, cross-AZ failover, real S3 transfer,
provider-delivery or school-GPU execution test.

## Tooling audit

The locked CDK dependency audit on 2026-11-17 reports no high/critical findings and
2 moderate entries from the upstream bundled `minimatch` glob-amplification issue
([GHSA-g3hv-75wg-m249](https://github.com/advisories/GHSA-g3hv-75wg-m249)). This
module defines no glob-based assets and does not accept server/client glob input;
the tooling is not shipped in the native runtime image. A tested older CDK pin
introduced higher-severity findings and was not retained. Re-run the audit and
resolve the upstream bundled dependency before an actual cloud deployment;
configuration/container checks are not a clean security certification.

## School activation is a separate cutover

The multi-school/public-browsing and academic-year code is a staged candidate,
not evidence that the running single-school services have been migrated. Keep
its flags off until **all** actual resource consumers enforce the new authority.
An authenticated PostgreSQL policy cannot restrict a legacy worker using a
service-role direct table scan.

The production-schema clone rehearsals exercise additive migrations twice,
preserve historical Auth/session/password and business-table fingerprints, and
verify tenant isolation, original contest/team/submission RPCs, teacher/student
membership review, temporary-grant boundaries and academic rollover rollback.
No operating academic rollover or membership migration was executed by those
checks. The imported official directory contains 2,408 high schools; listing a
school never verifies an applicant or enables its GPU sharing.

Before activation, verify the exact running versions and configuration of:

- The root GPU reservation controller: use fresh scope-filtered assignments,
  including revocation during the next poll, without deleting reservation history.
  Its old service-role `reservations?status=eq.approved` scan is a hard blocker.
- Deployment admission **and workers**: retain request-actor provenance, recheck
  that actor and the owner at dequeue/poll, and authorize before reinstating a
  proxy. A browser-only check does not protect MCP/internal worker paths.
- LaTeX, material/file readers and personal-project APIs: preserve owner/member
  access independently of school approval; authorize physical compute separately.
- Native and legacy configuration: do not assume enabling the web flag activates
  backend enforcement. Treat unsupported or inconsistent enabled flags as denial.
- Timetable widgets: publish the school-bound native app only with its matching
  self-session oracle, scoped school API and Edge proxy. Do not label another
  school's legacy timetable or meals as the selected school's information.

School homepage addresses are metadata until reviewed. Website adapters must
validate the exact approved host, resolve and pin public DNS addresses, reject
redirects/private addresses, and bound duration/body size. Missing configurations
must not fall back to Chungbuk Science High School.

Perform one controlled cutover with private backups, a maintenance/rollback plan,
explicit approval for operating migrations and final live smoke tests. Do not
activate distributed GPU sharing, execute a real academic rollover, or publish
old private content as part of this readiness work.

## Native AI control-plane preparation

The native AI candidate supports `CBGPU_AI_MODE=api-only` for a relocated central
API. This explicitly ignores an inherited native-runner setting and starts no
local scheduler, GPU/reservation probe, model process, customer-code container or
uncached document/OCR extraction. PostgreSQL queue admission, cancellation,
history and already authorized cached text remain available. Jobs stay queued;
status reports `NO_EXECUTOR`. Readiness distinguishes the PostgreSQL-backed
control plane from execution availability. Unset/empty mode preserves the
existing behavior; explicit `all-in-one` requires a configured native runner.

This is not remote school inference. Signed GPU enrollment remains inventory
only, sharing remains disabled, and no job is dispatched to another school.
Scoped executor authentication, distributed job leases, authorized attachment
and artifact transport, cancellation and node-loss recovery need separate
reviewed implementation before a central API can actually schedule school
execution. Independent schedulers must not share the queue without that lease
protocol. No operating AI service, model cache, GPU workload or AWS resource was
changed to validate this source mode.
