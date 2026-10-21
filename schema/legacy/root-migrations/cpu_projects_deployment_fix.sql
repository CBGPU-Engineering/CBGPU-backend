-- Run after create_cpu_projects.sql and supabase_security_hardening_fix.sql.
-- The daemon uses a service-role key from its server environment. Regular
-- users need explicit RLS policies for their own project requests.
alter table public.cpu_projects enable row level security;

drop policy if exists "cpu_projects_owner_select" on public.cpu_projects;
create policy "cpu_projects_owner_select" on public.cpu_projects for select
  to authenticated using (user_id = (select auth.uid()) or public.is_admin((select auth.uid())));

drop policy if exists "cpu_projects_owner_insert" on public.cpu_projects;
create policy "cpu_projects_owner_insert" on public.cpu_projects for insert
  to authenticated with check (
    user_id = (select auth.uid()) and status = 'pending' and deployed_at is null
    and exists (
      select 1 from public.profiles p
      where p.id = (select auth.uid())
        and p.is_approved is distinct from false
    )
  );

-- Existing owner-update policy allows any column; do not allow a student to
-- approve their own code or claim a deployment completed.
create or replace function public.guard_cpu_project_deployment_fields()
returns trigger language plpgsql security invoker as $$
begin
  if (select auth.role()) = 'service_role' or public.is_admin((select auth.uid())) then
    return new;
  end if;
  if new.user_id is distinct from old.user_id
     or new.deployed_at is distinct from old.deployed_at
     or new.status is distinct from old.status and new.status <> 'deleted' then
    raise exception 'Only an admin can approve or mark a CPU deployment complete';
  end if;
  return new;
end;
$$;

drop trigger if exists guard_cpu_project_deployment_fields on public.cpu_projects;
create trigger guard_cpu_project_deployment_fields
  before update on public.cpu_projects for each row
  execute function public.guard_cpu_project_deployment_fields();
