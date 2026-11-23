# Atomic poll editor

The previous editor updated the poll header, deleted all questions, then inserted replacements. Because `poll_answers.question_id` has `ON DELETE CASCADE`, an ordinary edit erased existing answers. Failed replacement inserts also left a partially saved poll.

`20261123162344_atomic_poll_editor_save.sql` adds an invoker RPC and integrity/revision triggers. It does not replace tables, change existing RLS policies, grant new table privileges, or rebuild existing question IDs. The RPC checks the existing fresh-session identity helper and the current administrator profile, saves the header and questions in one transaction, maps new temporary question IDs and conditional references to UUIDs, and rejects stale editor revisions. Answered questions cannot be removed, moved to another poll, or have their IDs/types changed. Options referenced by existing answers cannot be removed, including through legacy direct updates. Duplicate option IDs, canonical UUID aliases and cyclic/invalid choice conditions are rejected; valid forward references produced by reordering remain supported. Explicit whole-poll deletion keeps the existing RLS-controlled cascade behavior.

Old open tabs remain able to use their existing direct-write flow. Database triggers stop their question deletion from cascading through existing answers, and direct header/question writes advance the revision. An old tab may have already changed its header before its separate question deletion fails; those old multi-request operations cannot become atomic retroactively. Reloading the page uses the atomic editor. No old delete-and-insert fallback is provided.

## Validation

Run the real PostgreSQL transaction suite from the backend repository:

```sh
node --test tests/polls/atomic-save.test.mts
```

It starts and removes its own network-isolated `postgres:17-alpine` container, with synthetic users/sessions and the actual poll schema/RLS shape. It tests answer and ID preservation, conditional ID mapping, invalid/foreign question rejection, late insertion failure rollback after prior changes/deletion, fresh administrator authorization, legacy direct-write protection, stale revisions, concurrent writers, option identity preservation, conditional graphs, legacy type updates racing a pending answer, and confirmed whole-poll deletion. It never connects to an existing database.

Run the web verification from the frontend repository:

```sh
node tests/polls-editor-atomic.browser.mts
npm test
npx tsc --noEmit
```

The browser test bundles the real editor with a synthetic transport and blocks external requests. It checks one RPC for duplicate clicks, draft retention on conflicts/rejected promises, and that a failed question load cannot be saved. Phone/desktop light/dark and Android WebView screenshots are saved under `/tmp/cbgpu-polls-atomic-browser`.

## Targeted rollout (operator only)

After the normal database backup and review, apply only this additive SQL file. Do not replay legacy poll bootstrap scripts or all historical migrations.

```sh
docker exec -i cbgpu-supabase-db psql -X -v ON_ERROR_STOP=1 -U supabase_admin -d postgres < supabase/migrations/20261123162344_atomic_poll_editor_save.sql
```

The file contains its own transaction and requests the PostgREST schema reload on commit. Apply it before releasing the web editor. Deploying the web first fails closed at the missing RPC and keeps the draft, but cannot save polls until the migration is installed.

Read-only installation verification:

```sql
select column_name,data_type from information_schema.columns
where table_schema='public' and table_name='polls' and column_name='editor_revision';
select prosecdef from pg_proc
where oid='public.cbgpu_save_poll(uuid,bigint,jsonb,jsonb)'::regprocedure;
select tgname from pg_trigger
where tgrelid in ('public.polls'::regclass,'public.poll_questions'::regclass)
and not tgisinternal and tgname like 'cbgpu_poll_%';
select has_function_privilege('anon','public.cbgpu_save_poll(uuid,bigint,jsonb,jsonb)','execute');
```

Expected: bigint revision, `prosecdef=false`, three integrity/revision triggers, and anonymous execute `false`. Fresh-session helper and live schema/policy definitions were inspected before authoring; no production records were queried or changed during implementation. The CLI security advisor was attempted against its default local stack only, but that stack was unavailable on `127.0.0.1:54322`; no remote advisor or migration command was run.

To roll back the web, retain the additive database guards. Removing them would reopen the legacy answer-loss path. Existing answers previously deleted by the old editor are not recovered by this migration.
