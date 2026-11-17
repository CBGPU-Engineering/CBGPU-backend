-- Raw shared credentials never become public. Synthetic insert only if absent;
-- never selects any real ciphertext, and every data change rolls back.
begin;
insert into public.eduset_settings(id,access_token_enc) values(1,'synthetic-private-reader-fixture') on conflict(id) do nothing;
set local role anon;
select set_config('request.jwt.claims','{"role":"anon"}',true);
do $$begin if exists(select 1 from public.eduset_settings) then raise exception 'ANONYMOUS_SHARED_CREDENTIAL_ROW_VISIBLE';end if;end$$;
reset role;
set local role service_role;
select set_config('request.jwt.claims','{"role":"service_role"}',true);
do $$begin if not exists(select 1 from public.eduset_settings where id=1) then raise exception 'PRIVATE_OPERATOR_SHARED_CREDENTIAL_ROW_HIDDEN';end if;end$$;
reset role;
rollback;
