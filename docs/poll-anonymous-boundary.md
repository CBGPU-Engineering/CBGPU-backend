# Internal poll anonymous boundary

`20261123175207_deny_anonymous_internal_polls.sql` adds one restrictive `ALL` policy, targeted only at the `anon` database role, to each of `polls`, `poll_questions`, `poll_responses`, and `poll_answers`. Each policy has `USING(false)` and `WITH CHECK(false)`.

The internal poll pages require sign-in. The existing anonymous SELECT table grants previously allowed poll/question rows through the legacy permissive public-read policies. A missing grant in the test bootstrap had masked that policy gap. The bootstrap now includes the actual four-table anonymous SELECT grants, and the results-visibility test expects zero visible rows after this new migration rather than an ACL exception.

This is an additional fourth migration. The three previously merged/applied poll migrations are unchanged. It does not change table privileges, existing authenticated policies, RPCs, UI behavior, or stored rows. Authenticated is not a member of anon in the verified deployment role structure; the synthetic test asserts that same separation. Service and maintenance roles keep their existing privileges and bypass behavior.

## Verification

```sh
node --test tests/polls/*.test.mts
```

Every suite creates and removes its own network-isolated PostgreSQL 17 container. No existing database, HTTP login, API key, or real account is used.

The new `anonymous-boundary.test.mts` explicitly grants anon all table privileges inside the fixture so a missing ACL cannot hide a failure. It demonstrates the pre-migration poll/question read exposure, then verifies zero visible rows on all four tables, rejected INSERTs, zero-row UPDATE/DELETE acknowledgements, and unchanged row snapshots. Empty claims and synthetic user/admin claims under the anon role remain denied. Existing policies and table ACLs are compared before/after and remain identical.

Authenticated owner/admin reads, identical owner submission retries, administrator edits, and confirmed deletion continue to work. Service-role reads remain available. Applying the fourth migration twice more does not change rows, ACLs, or earlier policies. Four restrictive policies remain installed, targeted only at anon.

The policy covers row-level SELECT and DML operations. It does not revoke table privileges or change PostgreSQL operations outside RLS, such as TRUNCATE; no such operation is introduced or executed by the application/tests.

The CLI security advisor was also attempted against its default local stack only. That stack was unavailable on `127.0.0.1:54322`; no remote advisor or existing database was contacted.

## Operator rollout after review and backup

Apply only the new additive migration after the three existing poll migrations. Implementation and tests did not apply it to an existing environment.

```sh
docker exec -i cbgpu-supabase-db psql -X -v ON_ERROR_STOP=1 -U postgres -d postgres < supabase/migrations/20261123175207_deny_anonymous_internal_polls.sql
```

The file contains its own transaction and requests PostgREST schema reload on commit. Read-only installation checks:

```sql
select tablename,policyname,permissive,roles,cmd,qual,with_check
from pg_policies
where schemaname='public' and policyname like '%anonymous_boundary'
order by tablename;
select pg_has_role('authenticated','anon','member');
```

Expected: four restrictive ALL policies with `{anon}`, false predicates, and no authenticated membership in anon. No production row query is needed for this verification.
