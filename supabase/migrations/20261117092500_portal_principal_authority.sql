begin;
-- Narrow server-only authority. Browser JWT/roles never receive direct grants.
create schema if not exists cbgpu_portal_auth;
revoke all on schema cbgpu_portal_auth from public,anon,authenticated;
do $$begin
 if not exists(select 1 from pg_roles where rolname='cbgpu_portal_auth_reader') then create role cbgpu_portal_auth_reader nologin noinherit nosuperuser nobypassrls;
 elsif exists(select 1 from pg_roles where rolname='cbgpu_portal_auth_reader' and(rolcanlogin or rolinherit or rolsuper or rolbypassrls)) then raise exception 'UNSAFE_PORTAL_AUTHORITY_ROLE';end if;
 execute format('grant connect on database %I to cbgpu_portal_auth_reader',current_database());
end$$;
grant usage on schema cbgpu_portal_auth to cbgpu_portal_auth_reader;
create or replace function cbgpu_portal_auth.principal(subject_id uuid,auth_session_id uuid)
returns jsonb language sql stable security definer set search_path='' as $$
select coalesce((select jsonb_build_object('valid',true,'id',p.id,'email',u.email,'name',coalesce(nullif(p.full_name,''),u.email),'is_admin',p.is_admin is true,'legacy_approved',p.is_approved is true)
 from public.profiles p join auth.users u on u.id=p.id join auth.sessions s on s.user_id=u.id
 where p.id=subject_id and s.id=auth_session_id and(s.not_after is null or s.not_after>clock_timestamp())
 and u.deleted_at is null and u.email_confirmed_at is not null and u.email is not null
 and(u.banned_until is null or u.banned_until<=clock_timestamp())
 and p.account_status in('ACTIVE','PENDING_VERIFICATION','REVERIFICATION_REQUIRED') limit 1),jsonb_build_object('valid',false))
$$;
revoke all on function cbgpu_portal_auth.principal(uuid,uuid) from public,anon,authenticated,service_role;
grant execute on function cbgpu_portal_auth.principal(uuid,uuid) to cbgpu_portal_auth_reader;
commit;
