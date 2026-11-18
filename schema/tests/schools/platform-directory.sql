-- Synthetic identities and schools only; all changes roll back.
begin;
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)
 select ('fd510000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-platform-directory-'||x||'@fixture.invalid',jsonb_build_object('full_name','Directory fixture','account_type_request','EXTERNAL'),'{}','authenticated','authenticated','fixture-no-password',now(),now(),now()from generate_series(1,3)x;
insert into auth.sessions(id,user_id)select('fd520000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,('fd510000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid from generate_series(1,3)x;
select set_config('request.jwt.claims','{"role":"service_role"}',true);
update public.profiles set is_admin=true where id='fd510000-0000-4000-8000-000000000001';
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,status,configuration,features)values
 ('fixture-platform-directory-a','Directory fixture A','directory fixture a','Directory fixture A','HIGH_SCHOOL','APPROVED','{}','{}'),
 ('fixture-platform-directory-b','Directory fixture B','directory fixture b','Directory fixture B','HIGH_SCHOOL','SUSPENDED','{}','{}'),
 ('fixture-platform-directory-c','Directory fixture C','directory fixture c','Directory fixture C','HIGH_SCHOOL','LISTED','{}','{}'),
 ('fixture-platform-directory-d','Directory fixture D','directory fixture d','Directory fixture D','HIGH_SCHOOL','UNCLAIMED','{}','{}');
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,school_roles)values('fd510000-0000-4000-8000-000000000002','fixture-platform-directory-a','TEACHER','ACTIVE',true,array['SCHOOL_OWNER','SCHOOL_ADMIN']);
set local role anon;
do $$begin begin perform public.cbgpu_platform_school_directory('{}');raise exception 'ANONYMOUS_PLATFORM_DIRECTORY';exception when insufficient_privilege then null;end;end$$;
reset role;
select set_config('request.jwt.claims','{"sub":"fd510000-0000-4000-8000-000000000002","role":"authenticated","session_id":"fd520000-0000-4000-8000-000000000002"}',true);
set local role authenticated;
do $$declare r jsonb;begin
 begin perform public.cbgpu_platform_school_directory('{"q":"Directory fixture"}');raise exception 'SCHOOL_ADMIN_PLATFORM_DIRECTORY';exception when insufficient_privilege then null;end;
 r:=public.cbgpu_school_api('directory','{"q":"Directory fixture"}');if exists(select 1 from jsonb_array_elements(r->'items')x where x->>'status'='SUSPENDED')then raise exception 'PUBLIC_SUSPENDED_DIRECTORY_LEAK';end if;
end$$;
select set_config('request.jwt.claims','{"sub":"fd510000-0000-4000-8000-000000000003","role":"authenticated","session_id":"fd520000-0000-4000-8000-000000000003"}',true);
do $$begin begin perform public.cbgpu_platform_school_directory('{}');raise exception 'MEMBER_PLATFORM_DIRECTORY';exception when insufficient_privilege then null;end;end$$;
select set_config('request.jwt.claims','{"sub":"fd510000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fd520000-0000-4000-8000-000000000001"}',true);
do $$declare r jsonb;p jsonb;begin
 r:=public.cbgpu_platform_school_directory('{"q":"Directory fixture"}');if(r->>'total')::int<>4 or jsonb_array_length(r->'items')<>4 then raise exception 'PLATFORM_DIRECTORY_INCOMPLETE';end if;
 r:=public.cbgpu_platform_school_directory('{"q":"Directory fixture","status":"SUSPENDED"}');if(r->>'total')::int<>1 or r#>>'{items,0,id}'<>'fixture-platform-directory-b'then raise exception 'SUSPENDED_PLATFORM_FILTER';end if;
 if r#>'{items,0}'?'contact_email' or r#>'{items,0}'?'contact_phone' or r#>'{items,0}'?'configuration' then raise exception 'UNNECESSARY_PRIVATE_SCHOOL_FIELDS';end if;
 r:=public.cbgpu_platform_school_directory('{"q":"Directory fixture","limit":1,"offset":1}');if(r->>'total')::int<>4 or jsonb_array_length(r->'items')<>1 or r#>>'{items,0,id}'<>'fixture-platform-directory-b'then raise exception 'PLATFORM_PAGINATION';end if;
 foreach p in array array['{"status":"all"}'::jsonb,'{"limit":0}','{"limit":101}','{"offset":10001}','{"owner_id":"forged"}']loop begin perform public.cbgpu_platform_school_directory(p);raise exception 'INVALID_FILTER_ACCEPTED';exception when invalid_parameter_value then null;end;end loop;
end$$;
reset role;
select set_config('request.jwt.claims','{"role":"service_role"}',true);update public.profiles set is_admin=false where id='fd510000-0000-4000-8000-000000000001';
select set_config('request.jwt.claims','{"sub":"fd510000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fd520000-0000-4000-8000-000000000001"}',true);set local role authenticated;
do $$begin begin perform public.cbgpu_platform_school_directory('{}');raise exception 'REVOKED_PLATFORM_ADMIN_CACHED';exception when insufficient_privilege then null;end;end$$;
reset role;select set_config('request.jwt.claims','{"role":"service_role"}',true);update public.profiles set is_admin=true where id='fd510000-0000-4000-8000-000000000001';delete from auth.sessions where id='fd520000-0000-4000-8000-000000000001';
select set_config('request.jwt.claims','{"sub":"fd510000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fd520000-0000-4000-8000-000000000001"}',true);set local role authenticated;
do $$begin begin perform public.cbgpu_platform_school_directory('{}');raise exception 'REVOKED_SESSION_PLATFORM_DIRECTORY';exception when insufficient_privilege then null;end;end$$;
reset role;
do $$begin if has_function_privilege('anon','public.cbgpu_platform_school_directory(jsonb)','EXECUTE')or has_function_privilege('service_role','public.cbgpu_platform_school_directory(jsonb)','EXECUTE')then raise exception 'UNAUTHENTICATED_DIRECTORY_RPC_GRANTED';end if;end$$;
rollback;
