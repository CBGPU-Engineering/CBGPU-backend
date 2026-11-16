-- CANDIDATE ONLY: apply with account-v2 approval, never independently.
-- Add restrictive school allocation checks without replacing ownership policies.
begin;
do $$declare target text;scope text;begin
 for target,scope in select * from (values('reservations','gpu'),('print_jobs','fabrication'),('laser_jobs','fabrication')) v(t,s) loop
  if to_regclass('public.'||target) is null then raise exception 'Missing resource table: %',target;end if;
  execute format('drop policy if exists account_v2_resource_insert on public.%I',target);
  execute format('create policy account_v2_resource_insert on public.%I as restrictive for insert to authenticated with check(public.cbgpu_can_use_resource(%L))',target,scope);
 end loop;
end$$;
drop policy if exists account_v2_resource_insert on public.cpu_projects;
create policy account_v2_resource_insert on public.cpu_projects as restrictive for insert to authenticated
 with check((deploy_requested_at is null and status='pending') or public.cbgpu_can_use_resource('school_compute'));
-- Owner settings, project history/memberships, files and deletion are unchanged.
-- Prevent direct REST callers from requesting execution through legacy UPDATE RLS.
create or replace function cbgpu_accounts.guard_resource_execution() returns trigger language plpgsql set search_path='' as $$
begin
 if auth.role()='service_role' or current_user in('postgres','supabase_admin') then return new;end if;
 if tg_table_name='cpu_projects' then
  if (new.deploy_requested_at is distinct from old.deploy_requested_at or new.status='approved' and old.status is distinct from 'approved')
   and public.cbgpu_can_use_resource('school_compute') is not true then raise exception 'SCHOOL_RESOURCE_PERMISSION_REQUIRED' using errcode='42501';end if;
 elsif tg_table_name='reservations' then
  if (new.start_time is distinct from old.start_time or new.end_time>old.end_time or new.status='approved' and old.status is distinct from 'approved')
   and public.cbgpu_can_use_resource('gpu') is not true then raise exception 'SCHOOL_RESOURCE_PERMISSION_REQUIRED' using errcode='42501';end if;
 end if;
 return new;
end$$;
drop trigger if exists account_v2_execution on public.cpu_projects;
create trigger account_v2_execution before update on public.cpu_projects for each row execute function cbgpu_accounts.guard_resource_execution();
drop trigger if exists account_v2_execution on public.reservations;
create trigger account_v2_execution before update on public.reservations for each row execute function cbgpu_accounts.guard_resource_execution();
-- Former equipment managers retain their own jobs, not other pupils' school records.
drop policy if exists account_v2_equipment_read on public.print_jobs;
create policy account_v2_equipment_read on public.print_jobs as restrictive for select to authenticated using(user_id=auth.uid() or public.cbgpu_can_use_resource('fabrication'));
drop policy if exists account_v2_equipment_read on public.laser_jobs;
create policy account_v2_equipment_read on public.laser_jobs as restrictive for select to authenticated using(user_id=auth.uid() or public.cbgpu_can_use_resource('fabrication'));
-- Equipment files contain school-scoped uploads. Personal preprints, avatars and
-- project files retain their own membership/ownership policies.
drop policy if exists account_v2_school_upload on storage.objects;
create policy account_v2_school_upload on storage.objects as restrictive for insert to authenticated
 with check(bucket_id<>'manufacturing' or public.cbgpu_can_use_resource('fabrication'));
commit;
