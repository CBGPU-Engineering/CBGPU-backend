# Atomic poll response submission

Apply `20261123164337_atomic_poll_response_submit.sql` after the atomic editor migration and before the matching web release. Apply only the reviewed additive files after the normal backup; do not replay legacy poll bootstrap scripts. This migration installs `cbgpu_submit_poll` and one answer-insert integrity trigger, then notifies PostgREST to reload its schema.

The invoker RPC uses the existing live-session identity helper and existing global poll RLS. It requires an open, unexpired poll and the current editor revision. It validates current question IDs, conditional visibility, visible required answers, option IDs, checkbox values, and integer scale bounds. A new response header and all visible answers commit in one transaction. No response or answer is deleted by this flow.

The existing `(poll_id,user_id)` uniqueness remains authoritative. An identical recorded answer set returns its existing response ID. A different nonempty answer set is preserved and the request is rejected. Only the current user's existing empty header may receive answers. Zero-question polls and polls whose questions are all hidden can legitimately retain empty headers; no bulk reclassification, deletion, or backfill is performed.

The web keeps existing response-header classification. Its explicit **내 응답 확인** action inspects the current user's response. An open poll with no recorded answers can be opened for explicit submission; this does not claim the previous header is incomplete. Results-hidden polls are not exposed by the new inspection action. A failed or lost submission response keeps the draft, and returning to the list refreshes the response record before inspection. No mutation is retried automatically.

## Concurrency and legacy clients

Ordinary nonadmin `SELECT ... FOR SHARE` is filtered through the existing admin-only UPDATE RLS. The RPC instead uses existing authenticated table privileges to take brief SHARE locks on `polls`, then `poll_questions`. This preserves RLS without adding policies or table grants. These locks delay global poll/question writes during validation and commit; there are no network/provider calls inside the transaction and the input is limited to 100 question keys.

Both the RPC and old-tab answer INSERTs take the same transaction advisory lock for a response. The legacy insert trigger also locks poll/question tables before reading the current type/options, rejects cross-poll question references, and prevents another answer for an already recorded `(response_id,question_id)`. It validates old-tab values against the current question, including an editor change that committed while the old request waited. Historical duplicates are left untouched; no index or cleanup is applied to existing rows.

Legacy header/answer requests remain separate. A failed old-tab answer insert can still leave its empty header; the new explicit submission path can reuse that owned header. Existing nonempty partial/duplicate legacy answer sets are not overwritten or automatically repaired. The mixed legacy/editor lock order can produce a database deadlock cancellation during a precisely overlapping old direct write; that transaction fails and rolls back rather than claiming success, and the web retains its draft for explicit status inspection.

## Verification

```sh
node --test tests/polls/atomic-response.test.mts
```

The suite creates and removes its own network-isolated PostgreSQL 17 container. It uses synthetic users with the existing authenticated/anonymous RLS shapes, including a genuine nonadmin submitter. It covers validation and rollback after a late insert failure; original-header retention; own/foreign header separation; identical retry and conflicting answers; legitimate empty responses; suspension/session expiry; concurrent RPC writers; legacy-answer-first and RPC-first races; editor-first text→scale and choice/checkbox option-removal races; and answer-first editor rejection.

From the web repository:

```sh
node tests/polls-response-atomic.browser.mts
node tests/polls-editor-atomic.browser.mts
npm test
npx tsc --noEmit
```

The actual-component browser fixture covers a committed submission with a lost reply, draft preservation and explicit receipt inspection without another mutation, owned-empty-header submission, and blocked `sessionStorage` with readable results and an optional visible summary. Phone/desktop light/dark and Android WebView screenshots are under `/tmp/cbgpu-polls-response-browser`. No production data or real AI requests are used.

Read-only installation checks:

```sql
select prosecdef from pg_proc where oid='public.cbgpu_submit_poll(uuid,bigint,jsonb)'::regprocedure;
select has_function_privilege('anon','public.cbgpu_submit_poll(uuid,bigint,jsonb)','execute');
select tgname from pg_trigger where tgrelid='public.poll_answers'::regclass and not tgisinternal;
```

Expected: invoker (`false`), anonymous execution denied (`false`), and `cbgpu_poll_answer_insert_integrity` installed. Keep database integrity guards if rolling back the web. Dropping them would reopen the old answer-shape and duplicate-insert races.
