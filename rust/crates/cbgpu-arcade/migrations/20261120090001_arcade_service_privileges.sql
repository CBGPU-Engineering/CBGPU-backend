-- Explicit owner-reviewed installation step. Role must already exist as an internal
-- LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS role.
-- Does not grant access to public/anon/authenticated or ownership of old data.
begin;
do $$begin
 if not exists(select 1 from pg_roles where rolname='cbgpu_arcade_service' and rolcanlogin and not rolsuper and not rolcreatedb and not rolcreaterole and not rolreplication and not rolbypassrls)
 then raise exception 'cbgpu_arcade_service must preexist with restricted privileges';end if;
 if exists(select 1 from pg_auth_members am join pg_roles r on r.oid=am.member where r.rolname='cbgpu_arcade_service') then raise exception 'cbgpu_arcade_service must not inherit other roles';end if;
end$$;
grant usage on schema auth,public,cbgpu_schools to cbgpu_arcade_service;
grant select(id,deleted_at,banned_until) on auth.users to cbgpu_arcade_service;
grant select(id,account_status) on public.profiles to cbgpu_arcade_service;
grant select(user_id,school_id,status,verified,starts_at,ends_at) on cbgpu_schools.memberships to cbgpu_arcade_service;
grant select(id,display_name,official_name) on cbgpu_schools.schools to cbgpu_arcade_service;
do $$declare t text;parts text[];begin
 foreach t in array array['auth.users','public.profiles','cbgpu_schools.memberships','cbgpu_schools.schools'] loop
  parts:=string_to_array(t,'.');
  if not exists(select 1 from pg_policies where schemaname=parts[1] and tablename=parts[2] and policyname='cbgpu_arcade_service_select') then
   execute format('create policy cbgpu_arcade_service_select on %I.%I for select to cbgpu_arcade_service using(true)',parts[1],parts[2]);
  end if;
 end loop;
end$$;
alter schema cbgpu_arcade owner to cbgpu_arcade_service;
alter table cbgpu_arcade.players owner to cbgpu_arcade_service;
alter table cbgpu_arcade.sessions owner to cbgpu_arcade_service;
commit;
