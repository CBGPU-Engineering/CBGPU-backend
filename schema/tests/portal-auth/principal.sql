-- Full real-schema clone only; synthetic identities and all mutations roll back.
begin;
select set_config('request.jwt.claims','{"role":"service_role"}',true);
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)
select ('fa910000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-portal-'||x||'@fixture.invalid',jsonb_build_object('full_name','Portal oracle fixture','account_type_request','EXTERNAL'),'{}','authenticated','authenticated','fixture-no-password',now(),now(),now() from generate_series(1,9)x;
insert into auth.sessions(id,user_id) select ('fa920000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,('fa910000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid from generate_series(1,9)x;
update public.profiles set is_admin=true,is_approved=true where id::text like 'fa910000-%';
update public.profiles set is_approved=false,account_status='REVERIFICATION_REQUIRED' where id='fa910000-0000-4000-8000-000000000002';
update public.profiles set is_admin=false where id='fa910000-0000-4000-8000-000000000003';
update public.profiles set account_status='SUSPENDED' where id='fa910000-0000-4000-8000-000000000004';
update auth.users set banned_until=now()+interval '1 hour' where id='fa910000-0000-4000-8000-000000000005';
update auth.users set deleted_at=now() where id='fa910000-0000-4000-8000-000000000006';
update auth.users set email_confirmed_at=null where id='fa910000-0000-4000-8000-000000000007';
update auth.sessions set not_after=now()-interval '1 minute' where user_id='fa910000-0000-4000-8000-000000000008';
delete from auth.sessions where user_id='fa910000-0000-4000-8000-000000000009';
set local role cbgpu_portal_auth_reader;
do $$declare n integer;v jsonb;begin
 for n in 1..9 loop
  v:=cbgpu_portal_auth.principal(('fa910000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid,('fa920000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid);
  if (v->>'valid')::boolean is distinct from (n in(1,2,3)) then raise exception 'PORTAL_FRESH_IDENTITY_MISMATCH case %',n;end if;
  if n in(1,2) and v->>'is_admin'<>'true' then raise exception 'EXPLICIT_PLATFORM_ADMIN_LOST';end if;
  if n=2 and v->>'legacy_approved'<>'false' then raise exception 'LEGACY_APPROVAL_NOT_SEPARATED';end if;
  if n=3 and v->>'is_admin'<>'false' then raise exception 'NONADMIN_ESCALATED';end if;
  if v ?| array['password','encrypted_password','tokens','raw_user_meta_data','school_roles','refresh_token'] then raise exception 'PORTAL_PRIVATE_USER_DATA_LEAK';end if;
 end loop;
 if cbgpu_portal_auth.principal('fa910000-0000-4000-8000-000000000001','fa920000-0000-4000-8000-000000000002')->>'valid'<>'false' then raise exception 'PORTAL_SESSION_IDENTITY_BORROW';end if;
 begin perform id from auth.users limit 1;raise exception 'PORTAL_ROLE_READS_RAW_AUTH_USERS';exception when insufficient_privilege then null;end;
 begin perform id from auth.sessions limit 1;raise exception 'PORTAL_ROLE_READS_RAW_AUTH_SESSIONS';exception when insufficient_privilege then null;end;
 begin perform id from public.profiles limit 1;raise exception 'PORTAL_ROLE_READS_RAW_PROFILES';exception when insufficient_privilege then null;end;
end$$;
reset role;
update public.profiles set is_admin=false where id='fa910000-0000-4000-8000-000000000001';
set local role cbgpu_portal_auth_reader;
do $$begin if cbgpu_portal_auth.principal('fa910000-0000-4000-8000-000000000001','fa920000-0000-4000-8000-000000000001')->>'is_admin'<>'false' then raise exception 'PORTAL_ROLE_REVOCATION_CACHED';end if;end$$;
reset role;
set local role anon;
do $$begin begin perform cbgpu_portal_auth.principal('fa910000-0000-4000-8000-000000000001','fa920000-0000-4000-8000-000000000001');raise exception 'ANON_PORTAL_AUTHORITY_LEAK';exception when insufficient_privilege then null;end;end$$;
reset role;
set local role authenticated;
do $$begin begin perform cbgpu_portal_auth.principal('fa910000-0000-4000-8000-000000000001','fa920000-0000-4000-8000-000000000001');raise exception 'BROWSER_PORTAL_AUTHORITY_LEAK';exception when insufficient_privilege then null;end;end$$;
reset role;
rollback;
select 'PORTAL_PRIVATE_PRINCIPAL_AUTHORITY_TESTS_PASSED';
