begin;

-- The public signup-review RPC runs as its owner, postgres. On hosted and
-- self-hosted Supabase this role is not a superuser and does not own the audit
-- table. Let it append the reviewed decision in the same transaction.
grant insert (actor_id, event, target_id, before_value, after_value)
  on cbgpu_accounts.audit to postgres;

commit;
