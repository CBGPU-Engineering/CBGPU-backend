# Canonical public school search

Apply only the reviewed `20261123141606_canonical_public_school_directory.sql`
after the operator's private database backup and saved definitions of
`public.cbgpu_school_api(text,jsonb)` and
`public.cbgpu_school_catalog_application_api(text,jsonb)`. No Rust binary,
configuration, credential, service or controller change is required.

The migration adds four private read helpers and one public read projection.
It replaces only the existing public school API's directory branch and the
catalog draft-create duplicate guard. Both replacements reject an unexpected
function version and are idempotent. Existing function ownership, permissions,
authentication and unrelated branches are retained. There are no table updates,
school merges/deletions, ID redirects, membership changes or draft rewrites.

A LISTED/UNCLAIMED NEIS row is suppressed from public search only when exactly
one APPROVED record is identified by its trimmed official ID, or by all of:

- An exact normalized official name and school type.
- The same recognized province, including common abbreviations and renamed
  Gangwon/Jeonbuk province names.
- Matching municipality and road evidence, without conflicting region/address
  province or municipality information.
- One approved record and one NEIS candidate in that name/type/province group,
  with at least one missing official ID. Different known IDs never use fallback.

Unknown or conflicting locations, homonyms, different roads/provinces/types and
ambiguous matches remain separate. The original approved ID and stored metadata
are returned. Raw admin inventory and direct-ID reads retain every original row.
This is a conservative public read projection, not a universal identity merge.

Canonicalization precedes search, status filtering, counting and pagination.
Search includes canonical province names/aliases and supports multiple terms,
so a registered row with region `경북 청도군` is still found by `경상북도`.
Status=LISTED or a hidden catalog display alias cannot resurrect the duplicate.
Unregistered catalogs remain searchable and retain their onboarding path.

A cached catalog URL uses the same strict read match to return the existing
`DUPLICATE_SCHOOL`/`existing_school_id` result before any draft insert/update.
It retains the original authentication guard and does not alter existing drafts
or the general approval workflow. Homonyms, official-ID conflicts and unmatched
catalogs continue through the original draft-create path.

## Operator verification

Backups contain private definitions/data and must remain mode0600 outside the
repository. The approved application is one atomic SQL file, not a generic
pending-migration push. With the existing database administration procedure:

```sh
docker exec -i cbgpu-supabase-db psql -U postgres -d postgres -v ON_ERROR_STOP=1 < supabase/migrations/20261123141606_canonical_public_school_directory.sql
```

Verify public name/province searches, registered IDs/statuses, totals and paging;
also verify a LISTED filter does not bring back a registered school's catalog
duplicate. No real application creation or account/role operation is needed for
read verification. The migration refreshes PostgREST's schema cache itself.

For an approved rollback, execute only `operations/school-search/rollback.sql`.
It restores the two original guarded branches and drops only the new read
functions, without CASCADE or data changes. Rollback and reapplication are tested
in a disposable PostgreSQL17 fixture; do not restore an old whole database or
overwrite newer unrelated function changes for this read-only feature.

Validation: `cargo test -p cbgpu-schools`, clippy with warnings denied, and
`cargo test -p cbgpu-schools --test school_directory_canonical -- --ignored`.
CI explicitly runs the disposable test. It covers both reported public-school
patterns, aliases and region queries, homonyms/location/known-ID conflicts,
ambiguity, counts and pagination, unchanged rows/IDs/memberships/old drafts,
preserved unrelated function bodies, cached-link draft prevention, unchanged
unregistered draft behavior, anonymous read/private-helper boundaries, repeated
migration, repeated rollback and reapplication. No production writes occur in
the test.
