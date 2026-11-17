-- Real production functions and RLS, synthetic identities; every change rolls back.
begin;
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,configuration,features) values('fixture-contribution-other','Other contribution school','other contribution school','Other','HIGH_SCHOOL','{}',array['projects']);
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)
select ('fa810000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-contributor-'||x||'@fixture.invalid',jsonb_build_object('full_name','Synthetic contributor','account_type_request','EXTERNAL'),'{}','authenticated','authenticated','fixture-no-password',now(),now(),now() from generate_series(1,10)x;
insert into auth.sessions(id,user_id) select ('fa820000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,('fa810000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid from generate_series(1,10)x;
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,school_roles)
select ('fa810000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,case when x=4 then 'fixture-contribution-other' else 'cbsh' end,case when x in(2,3) then 'TEACHER' when x=8 then 'EXTERNAL' else 'STUDENT' end,case when x=3 then 'PENDING_VERIFICATION' else 'ACTIVE' end,x<>3,'{}' from generate_series(1,10)x where x<>5;
update cbgpu_schools.memberships set ends_at=now()-interval '1 minute' where user_id='fa810000-0000-4000-8000-000000000007';
update auth.users set banned_until=now()+interval '1 hour' where id='fa810000-0000-4000-8000-000000000009';
update auth.sessions set not_after=now()-interval '1 minute' where user_id='fa810000-0000-4000-8000-000000000010';
insert into cbgpu_accounts.guest_requests(external_user_id,requester_id,school_id,project_name,purpose,resources,starts_at,ends_at,status,reviewed_by) values('fa810000-0000-4000-8000-000000000005','fa810000-0000-4000-8000-000000000001','cbsh','Synthetic study grant','No contributor role',array['study'],now()-interval '1 hour',now()+interval '1 hour','APPROVED','fa810000-0000-4000-8000-000000000002');
set local role authenticated;
do $$declare n integer;uid uuid;allowed boolean;begin
 for n in 1..10 loop
  uid:=('fa810000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid;
  perform set_config('request.jwt.claims',jsonb_build_object('sub',uid,'role','authenticated','session_id','fa820000-0000-4000-8000-'||lpad(n::text,12,'0'))::text,true);
  allowed:=public.cbgpu_eduset_contributor_access();
  if allowed is distinct from (n in(1,2,6)) then raise exception 'CONTRIBUTOR_AFFILIATION_OR_SESSION_SCOPE_MISMATCH case %',n;end if;
  if allowed then
   insert into public.eduset_contributors(user_id,enabled) values(uid,true);
   insert into public.eduset_contributor_devices(user_id,key_hash,name) values(uid,lpad(n::text,64,'a'),'Synthetic device');
   begin insert into public.eduset_contributor_devices(user_id,key_hash,name) values(case when n=2 then 'fa810000-0000-4000-8000-000000000001'::uuid else 'fa810000-0000-4000-8000-000000000002'::uuid end,lpad((n+20)::text,64,'b'),'Forged owner');raise exception 'CONTRIBUTOR_DEVICE_OWNER_SPOOF';exception when insufficient_privilege then null;end;
  else
   begin insert into public.eduset_contributors(user_id,enabled) values(uid,true);raise exception 'UNVERIFIED_CONTRIBUTOR_ENROLLED';exception when insufficient_privilege then null;end;
  end if;
 end loop;
 begin perform public.cbgpu_eduset_device_source_allowed(lpad('1',64,'a'));raise exception 'DEVICE_OWNER_POLICY_PUBLIC_ORACLE';exception when insufficient_privilege then null;end;
end$$;
reset role;
-- A historical guest device/self opt-in must not turn a study grant into contribution authority.
insert into public.eduset_contributors(user_id,enabled) select ('fa810000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid,true from unnest(array[3,4,5,7,8,9,10])n;
insert into public.eduset_contributor_devices(user_id,key_hash,name) select ('fa810000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid,lpad(n::text,64,'a'),'Historical synthetic device' from unnest(array[3,4,5,7,8,9,10])n;
set local role service_role;
do $$begin
 if not public.cbgpu_eduset_device_source_allowed(lpad('1',64,'a')) or not public.accept_eduset_contribution(lpad('1',64,'a'),'synthetic-not-real-encrypted-token',now()+interval '1 day') then raise exception 'VERIFIED_CONTRIBUTOR_DEVICE_REJECTED';end if;
 if public.cbgpu_eduset_device_source_allowed(lpad('5',64,'a')) or public.accept_eduset_contribution(lpad('5',64,'a'),'synthetic-guest-must-not-store',now()+interval '2 days') then raise exception 'STUDY_GUEST_BECAME_SHARED_TOKEN_CONTRIBUTOR';end if;
 if not public.cbgpu_eduset_device_source_allowed(lpad('10',64,'a')) then raise exception 'BACKGROUND_DEVICE_INCORRECTLY_REQUIRES_INTERACTIVE_SESSION';end if;
 if exists(select 1 from unnest(array[3,4,7,8,9])n where public.cbgpu_eduset_device_source_allowed(lpad(n::text,64,'a'))) then raise exception 'INELIGIBLE_HISTORICAL_DEVICE_STILL_AUTHORIZED';end if;
end$$;
reset role;
update cbgpu_schools.memberships set verified=false,status='REVERIFICATION_REQUIRED' where user_id='fa810000-0000-4000-8000-000000000001';
update public.eduset_contributor_devices set revoked_at=now() where user_id='fa810000-0000-4000-8000-000000000002';
update public.eduset_contributors set enabled=false where user_id='fa810000-0000-4000-8000-000000000006';
set local role service_role;
do $$declare n integer;begin foreach n in array array[1,2,6] loop if public.cbgpu_eduset_device_source_allowed(lpad(n::text,64,'a')) or public.accept_eduset_contribution(lpad(n::text,64,'a'),'synthetic-must-not-store',now()+interval '3 days') then raise exception 'REVOKED_DEVICE_OR_MEMBERSHIP_STILL_CONTRIBUTES';end if;end loop;end$$;
reset role;
do $$begin if has_function_privilege('anon','public.cbgpu_eduset_contributor_access()','execute') or has_function_privilege('authenticated','public.cbgpu_eduset_device_source_allowed(text)','execute') then raise exception 'CONTRIBUTOR_ORACLE_ACL_BROADENED';end if; if exists(select 1 from public.profiles where id::text like 'fa810000-%' and is_approved) then raise exception 'CONTRIBUTOR_PROFILE_PROMOTED';end if;end$$;
-- Fresh platform role is separate from school administration and study grants.
select set_config('request.jwt.claims','{"role":"service_role"}',true);
update public.profiles set is_admin=true where id='fa810000-0000-4000-8000-000000000001';
update cbgpu_schools.memberships set school_roles=array['SCHOOL_ADMIN'] where user_id='fa810000-0000-4000-8000-000000000004';
select set_config('request.jwt.claims','{"sub":"fa810000-0000-4000-8000-000000000004","role":"authenticated","session_id":"fa820000-0000-4000-8000-000000000004"}',true);
set local role authenticated;
do $$begin if exists(select 1 from public.eduset_settings) then raise exception 'SCHOOL_ADMIN_BECAME_PLATFORM_TOKEN_ADMIN';end if;end$$;
reset role;
select set_config('request.jwt.claims','{"sub":"fa810000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fa820000-0000-4000-8000-000000000001"}',true);
set local role authenticated;
do $$declare changed integer;begin
 if not exists(select 1 from public.eduset_settings where id=1) then raise exception 'FRESH_PLATFORM_ADMIN_SETTINGS_HIDDEN';end if;
 insert into public.eduset_settings(id,access_token_enc,updated_at,updated_by) values(1,'synthetic-only-admin-token',now(),auth.uid()) on conflict(id) do update set updated_at=excluded.updated_at;
 update public.eduset_settings set updated_at=updated_at where id=1;get diagnostics changed=row_count;if changed<>1 then raise exception 'FRESH_PLATFORM_ADMIN_WRITE_DENIED';end if;
end$$;
reset role;
update auth.sessions set not_after=now()-interval '1 minute' where id='fa820000-0000-4000-8000-000000000001';
set local role authenticated;
do $$declare changed integer;begin
 if exists(select 1 from public.eduset_settings) then raise exception 'REVOKED_ADMIN_SETTINGS_VISIBLE';end if;
 update public.eduset_settings set updated_at=updated_at where id=1;get diagnostics changed=row_count;if changed<>0 then raise exception 'REVOKED_ADMIN_UPDATED_SETTINGS';end if;
 delete from public.eduset_settings where id=1;get diagnostics changed=row_count;if changed<>0 then raise exception 'REVOKED_ADMIN_DELETED_SETTINGS';end if;
 begin insert into public.eduset_settings(id,access_token_enc,updated_at,updated_by) values(1,'synthetic-must-not-store',now(),auth.uid()) on conflict(id) do update set updated_at=excluded.updated_at;raise exception 'REVOKED_ADMIN_UPSERTED_SETTINGS';exception when insufficient_privilege then null;end;
end$$;
reset role;
rollback;
