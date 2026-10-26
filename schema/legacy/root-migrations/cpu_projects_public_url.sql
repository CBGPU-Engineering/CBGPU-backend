-- Run after cpu_projects_deployment_fix.sql.
-- Public CPU projects are served through a Cloudflare tunnel that the daemon
-- creates. The daemon records the tunnel URL here and the web/app link to it.
alter table public.cpu_projects
  add column if not exists public_url text
  check (public_url is null or public_url ~ '^https://');

-- Only the daemon (service role) or an admin may set public_url. A project owner
-- must not be able to point the "Access" link of their page at another site.
create or replace function public.guard_cpu_project_deployment_fields()
returns trigger language plpgsql security invoker as $$
begin
  if (select auth.role()) = 'service_role' or public.is_admin((select auth.uid())) then
    return new;
  end if;
  if new.user_id is distinct from old.user_id
     or new.deployed_at is distinct from old.deployed_at
     or new.public_url is distinct from old.public_url
     or new.status is distinct from old.status and new.status <> 'deleted' then
    raise exception 'Only an admin can approve or mark a CPU deployment complete';
  end if;
  return new;
end;
$$;

drop policy if exists "cpu_projects_owner_insert" on public.cpu_projects;
create policy "cpu_projects_owner_insert" on public.cpu_projects for insert
  to authenticated with check (
    user_id = (select auth.uid()) and status = 'pending' and deployed_at is null
    and public_url is null
    and exists (
      select 1 from public.profiles p
      where p.id = (select auth.uid())
        and p.is_approved is distinct from false
    )
  );
