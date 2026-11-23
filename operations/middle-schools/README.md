# Reviewed middle-school catalog and opt-in contest cohorts

This is an additive catalog/eligibility release. It does not approve schools, enable features, assign roles, configure GPUs, or change existing contest policies/registrations. Imported schools cannot be joined until the existing school approval process completes. Existing SCHOOL/community and contest access scopes remain unchanged.

## Official source and validation

`schema/data/neis-middle-schools-2026-10-31.json` was generated from the latest dated CSV offered by the official NEIS school-basic-information file index on 2026-11-23. Source URL, dataset/file identifiers and raw CSV SHA256 are embedded. The source is licensed without usage restriction by the NEIS dataset page.

There are 3,358 schools with official identifiers: 3,325 `중학교`, 33 `각종학교(중)`, across 17 source region values. The 17 source entries without official IDs are explicitly recorded in `excluded`; no invented IDs or name-only merges. Official IDs are deduplicated before output. Unclassified `각종학교`, elementary and high schools are not silently imported as middle schools.

Regenerate without database access:

```sh
cargo run --manifest-path rust/Cargo.toml -p cbgpu-schools --bin school-directory -- --output NEW_MANIFEST.json --school-type MIDDLE_SCHOOL
```

The default remains `HIGH_SCHOOL`, preserving existing automation. Only `HIGH_SCHOOL` and `MIDDLE_SCHOOL` arguments are accepted; each manifest scope must agree with every row's type/source kind. SQL importer rejects official-ID type conflicts. The new private, RLS-protected `directory_import_scopes` records both scopes when their raw CSV hash is identical; existing `directory_imports` is preserved.

## API compatibility and permissions

Existing directory, summary, catalog-bound draft, signup/join, membership and role APIs already carry generic school types. Middle schools use `SCHOOL_DEFINED` student numbers and the same pending/manual affiliation review. No CBsh student-number format is imposed. The existing academic rollover API explicitly supports only CBsh's reviewed `CBSH_4_DIGIT` policy; middle schools do not gain rollover or EduSet/wakeup access.

`cbgpu_school_life_sources(text)` gains only `school_type`, sourced from the authorized school row. A guarded definition replacement preserves all authorization/source configuration. The caller can choose the official middle-school timetable dataset from this field; this release does not auto-enable life features or change website/Comcigan adapters.

Contests retain independent visibility and access scopes. `participant_policy` accepts:

- `ALL_ELIGIBLE`: unchanged existing authorization.
- `HIGH_SCHOOL_STUDENTS`: unchanged verified active high-school student requirement.
- `MIDDLE_SCHOOL_STUDENTS`: verified active middle-school students.
- `SECONDARY_SCHOOL_STUDENTS`: verified active middle/high-school students.
- `SCHOOL_TEACHERS`: verified active school teachers.

All cohorts remain additional conditions on existing registration/resource/scope authorization, not grants. EXTERNAL school owners do not become students/teachers. Existing contests are untouched; no registration deletion, migration or automatic broadening. Changing a cohort can reject later writes by an existing registrant; the owner must explicitly choose it. Existing fresh-session/host RLS/scope triggers remain in force.

The native contest HTTP list/detail projection adds four already-public, RLS-filtered fields (`school_id`, `access_scope`, `allowed_school_ids`, `participant_policy`); query filters/auth stay unchanged. The optional binary rollout is limited to the existing contest HTTP service, never contest automation/controller services.

## Operator sequence (requires parent review before execution)

1. Private full database backup, plus exact definitions of `import_official_directory(jsonb)`, `contest_participant_allowed(uuid,uuid)`, `cbgpu_school_life_sources(text)` and the existing contest participant check constraint. Save catalog row count and status/type totals, membership/role totals, existing imports and effective function privileges. Do not log account records or credentials.
2. Apply only `20261123144307_middle_school_catalog_and_participant_policies.sql`; it has bounded lock/statement timeouts. No new database login, password, service credential, table grant or production account change is needed. Existing offline importer role is reused.
3. Render the single placeholder in `import.sql.template` with the reviewed JSON manifest as a SQL literal (double every single quote), save the result privately, and review its hash. The disposable fixture executes this exact template twice. Import a delta of the reviewed middle manifest, filtering out **all** existing official IDs before calling `cbgpu_schools.import_official_directory`. Capture full before-row snapshots inside the transaction and assert every old school/membership row is unchanged, and every addition is LISTED, unverified and featureless. Use the existing offline importer role via the operator's controlled database connection. Never call a browser-facing import endpoint. Keep the source manifest immutable.
4. Verify public search/canonical totals, representative middle results/type, unchanged high-school IDs, and no added memberships/features/roles. Authorized life-source checks use only isolated fixtures; no production account impersonation or synthetic production memberships.
5. If replacing the contest HTTP executable, preserve its existing unit/environment/keys and account/school flags. Back up the previous executable, change only the reviewed executable path, then check `/healthz`. No GPU/SSH/controller restart.

`rollback.sql` restores function definitions and the old constraint only if no contest uses a new cohort; otherwise it aborts the whole transaction rather than widening/narrowing live policies. Imported school rows and private source audit remain intact. Never delete catalog rows or drafts as a rollback.

## Verification

`cargo test -p cbgpu-schools`; `cargo test -p cbgpu-schools --test middle_schools -- --ignored`; `cargo test -p cbgpu-contests-api`; clippy both crates with all targets and warnings denied. The Docker PostgreSQL fixture imports the full official middle manifest twice under the actual importer role, verifies old rows/permissions unchanged, known type/scope/source-kind validation, real catalog-bound draft and school-defined join behavior, cohort isolation and expired/revoked affiliations, community school boundaries, and exact life-handler authorization preservation. No production writes are part of these tests.
