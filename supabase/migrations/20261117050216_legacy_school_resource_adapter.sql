begin;
-- Compatibility approval means current INTERNAL CBsh affiliation, not a cached
-- primary-profile approval bit. Guest grants remain explicitly resource-scoped.
create or replace function cbgpu_schools.legacy_cbsh_access(subject uuid) returns boolean language sql stable security definer set search_path='' as $$
select exists(select 1 from cbgpu_schools.memberships m join public.profiles p on p.id=m.user_id join auth.users u on u.id=m.user_id where m.user_id=subject and m.school_id='cbsh' and m.membership_type in('STUDENT','TEACHER') and cbgpu_schools.active_member(subject,'cbsh') and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now()))
$$;
revoke all on function cbgpu_schools.legacy_cbsh_access(uuid) from public,anon,authenticated;
-- Existing compatibility predicate owner is postgres (non-superuser on Supabase).
grant usage on schema cbgpu_schools to postgres;
grant execute on function cbgpu_schools.legacy_cbsh_access(uuid) to postgres;
create or replace function public.get_is_approved(uid uuid) returns boolean language sql stable security definer set search_path='' as $$
select case when coalesce(auth.role(),current_setting('role',true),'')<>'anon' and
 (auth.role()='service_role' or session_user in('postgres','supabase_admin') or (public.cbgpu_personal_project_identity() and(uid=auth.uid() or exists(select 1 from public.profiles p where p.id=auth.uid() and p.is_admin and p.account_status<>'SUSPENDED'))))
 then cbgpu_schools.legacy_cbsh_access(uid) else false end
$$;
-- Self-only BFF adapter; never read/mirror profiles.is_approved to grant access.
create or replace function public.cbgpu_self_resource_access(target_school text default 'cbsh') returns jsonb language plpgsql stable security definer set search_path='' as $$
declare uid uuid:=auth.uid();begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 return jsonb_build_object('school_id',target_school,'legacy_cbsh_access',target_school='cbsh' and cbgpu_schools.legacy_cbsh_access(uid),'resources',(select jsonb_object_agg(r,cbgpu_accounts.can_user_use_resource(uid,r,target_school)) from unnest(array['gpu','study','school_storage','fabrication','school_compute','contests'])r));
end$$;
revoke all on function public.cbgpu_self_resource_access(text) from public,anon;
grant execute on function public.cbgpu_self_resource_access(text) to authenticated;
-- Required controller adapter: same approved reservation fields, fresh owner
-- authorization on EVERY poll. No secrets. Existing service-role table scans
-- bypass RLS and MUST be replaced by this call before feature activation.
create or replace function public.cbgpu_service_gpu_assignments() returns table(id uuid,gpu_id integer,user_id uuid,start_time timestamptz,end_time timestamptz,status text) language sql stable security definer set search_path='' as $$
select r.id,r.gpu_id,r.user_id,r.start_time,r.end_time,r.status::text from public.reservations r where r.status='approved' and r.end_time>now() and cbgpu_accounts.can_user_use_resource(r.user_id,'gpu','cbsh') order by r.gpu_id,r.start_time
$$;
revoke all on function public.cbgpu_service_gpu_assignments() from public,anon,authenticated;
grant execute on function public.cbgpu_service_gpu_assignments() to service_role;
commit;
