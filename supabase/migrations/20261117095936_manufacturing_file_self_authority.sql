begin;
-- A policy reader inherits authenticated RLS only, never table ownership or
-- service-role bypass. The outer RPC returns a boolean, not an object key/URL.
do $$begin
 if not exists(select 1 from pg_catalog.pg_roles where rolname='cbgpu_equipment_file_reader') then create role cbgpu_equipment_file_reader nologin inherit nosuperuser nocreatedb nocreaterole noreplication nobypassrls;end if;
 if exists(select 1 from pg_catalog.pg_roles where rolname='cbgpu_equipment_file_reader' and(rolcanlogin or rolsuper or rolbypassrls or rolcreaterole or rolcreatedb or rolreplication)) then raise exception 'UNSAFE_EQUIPMENT_FILE_READER_ROLE';end if;
end$$;
grant authenticated to cbgpu_equipment_file_reader;
grant usage on schema storage,cbgpu_schools to cbgpu_equipment_file_reader;
grant select(bucket_id,name) on storage.objects to cbgpu_equipment_file_reader;
create or replace function cbgpu_schools.manufacturing_file_visible(object_name text) returns boolean language sql stable security definer set search_path='' set row_security=on as $$
select exists(select 1 from storage.objects o where o.bucket_id='manufacturing' and o.name=object_name)
$$;
alter function cbgpu_schools.manufacturing_file_visible(text) owner to cbgpu_equipment_file_reader;
revoke all on function cbgpu_schools.manufacturing_file_visible(text) from public,anon,authenticated,service_role;
-- Freeze the actual shared policy, not a second handwritten authorization rule.
-- A partial restore/removal/replacement of 93022 must fail closed per request.
create or replace function cbgpu_schools.equipment_file_policy_fingerprint() returns jsonb language sql stable security definer set search_path='' as $$
select jsonb_object_agg(k,encode(sha256(convert_to(definition,'UTF8')),'hex')) from(values
 ('file_policy',(select pg_get_expr(p.polqual,p.polrelid) from pg_catalog.pg_policy p where p.polrelid='storage.objects'::regclass and p.polname='scoped_equipment_file_read' and not p.polpermissive and p.polcmd='r' and 'authenticated'::regrole=any(p.polroles))),
 ('operator',pg_get_functiondef('public.cbgpu_equipment_operator(text,integer)'::regprocedure)),
 ('manager',pg_get_functiondef('cbgpu_schools.equipment_manager_valid(uuid,text,integer)'::regprocedure)),
 ('oversight',pg_get_functiondef('public.cbgpu_equipment_platform_oversight()'::regprocedure))) source(k,definition)
$$;
revoke all on function cbgpu_schools.equipment_file_policy_fingerprint() from public,anon,authenticated,service_role;
create table if not exists cbgpu_schools.equipment_file_policy_contract(version integer primary key check(version>0),fingerprint jsonb not null check(jsonb_typeof(fingerprint)='object'));
alter table cbgpu_schools.equipment_file_policy_contract enable row level security;
revoke all on cbgpu_schools.equipment_file_policy_contract from public,anon,authenticated,service_role;
do $$declare expected jsonb:=cbgpu_schools.equipment_file_policy_fingerprint();begin
 if exists(select 1 from jsonb_each(expected) item where item.value='null'::jsonb) then raise exception 'EQUIPMENT_FILE_POLICY_DEPENDENCY_MISSING';end if;
 insert into cbgpu_schools.equipment_file_policy_contract(version,fingerprint) values(1,expected) on conflict(version) do nothing;
 if(select fingerprint from cbgpu_schools.equipment_file_policy_contract where version=1) is distinct from expected then raise exception 'EQUIPMENT_FILE_POLICY_CONTRACT_CHANGED';end if;
end$$;
create or replace function public.cbgpu_manufacturing_file_access(p_object_name text) returns boolean language plpgsql stable security definer set search_path='' as $$
declare reader oid;begin
 if auth.role() is distinct from 'authenticated' or not public.cbgpu_personal_project_identity() then return false;end if;
 -- Exact stored keys only: no URL decoding, wildcard lookup or path rewriting.
 if p_object_name is null or octet_length(p_object_name) not between 1 and 2048 or p_object_name ~ '[[:cntrl:]]' or position(chr(92) in p_object_name)>0 or exists(select 1 from unnest(string_to_array(p_object_name,'/')) segment where segment in('','.','..')) then return false;end if;
 if not exists(select 1 from storage.buckets b where b.id='manufacturing' and b.public is false) then return false;end if;
 if(select fingerprint from cbgpu_schools.equipment_file_policy_contract where version=1) is distinct from cbgpu_schools.equipment_file_policy_fingerprint() then return false;end if;
 select r.oid into reader from pg_catalog.pg_roles r where r.rolname='cbgpu_equipment_file_reader' and not(r.rolcanlogin or r.rolsuper or r.rolbypassrls or r.rolcreaterole or r.rolcreatedb or r.rolreplication);
 if reader is null then return false;end if;
 -- A restore with --no-owner must not turn the nested reader into a superuser
 -- SECURITY DEFINER that silently bypasses the 93022 job/file policies.
 if not exists(select 1 from pg_catalog.pg_proc p where p.oid='cbgpu_schools.manufacturing_file_visible(text)'::regprocedure and p.proowner=reader and p.prosecdef and 'row_security=on'=any(p.proconfig) and exists(select 1 from unnest(p.proconfig) setting where setting in('search_path=""','search_path='))) then return false;end if;
 if not exists(select 1 from pg_catalog.pg_class c where c.oid='storage.objects'::regclass and c.relrowsecurity and c.relowner<>reader and not pg_catalog.pg_has_role(reader,c.relowner,'MEMBER')) then return false;end if;
 if exists(select 1 from pg_catalog.pg_auth_members m where m.member=reader and m.roleid<>'authenticated'::regrole) then return false;end if;
 return cbgpu_schools.manufacturing_file_visible(p_object_name);
end$$;
revoke all on function public.cbgpu_manufacturing_file_access(text) from public,anon,service_role;
grant execute on function public.cbgpu_manufacturing_file_access(text) to authenticated;
commit;
