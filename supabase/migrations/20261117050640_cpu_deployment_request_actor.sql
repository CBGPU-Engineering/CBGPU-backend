begin;
-- New requests retain their verified initiator independently of project owner.
-- Historical rows stay NULL; V1 workers must refuse missing actor proof.
alter table public.cpu_projects add column if not exists deploy_requested_by uuid references auth.users(id) on delete set null;
create or replace function cbgpu_schools.guard_cpu_request_actor() returns trigger language plpgsql security definer set search_path='' as $$
declare requested boolean;begin
 if tg_op='INSERT' then requested:=new.deploy_requested_at is not null;
 else requested:=new.deploy_requested_at is not null and new.deploy_requested_at is distinct from old.deploy_requested_at;
 end if;
 if requested then
 if auth.uid() is not null and auth.role()='authenticated' then
 if not public.cbgpu_personal_project_identity() then raise exception 'FRESH_DEPLOYMENT_ACTOR_REQUIRED' using errcode='42501';end if;
 -- Ignore any supplied foreign UUID; identity comes from validated JWT context.
 new.deploy_requested_by:=auth.uid();
 elsif auth.role()='service_role' then
 if new.deploy_requested_by is null then raise exception 'VERIFIED_DEPLOYMENT_ACTOR_REQUIRED' using errcode='23514';end if;
 else
 -- Maintenance restoration is explicit; ordinary PostgREST anon cannot request.
 if session_user not in('postgres','supabase_admin','cbgpu_clone_bootstrap') then raise exception 'DEPLOYMENT_ACTOR_REQUIRED' using errcode='42501';end if;
 end if;
 elsif tg_op='INSERT' then
 if new.deploy_requested_by is not null then raise exception 'DEPLOYMENT_ACTOR_WITHOUT_REQUEST' using errcode='23514';end if;
 elsif new.deploy_requested_by is distinct from old.deploy_requested_by then
 raise exception 'DEPLOYMENT_ACTOR_IMMUTABLE' using errcode='42501';
 end if;
 return new;
end$$;
revoke all on function cbgpu_schools.guard_cpu_request_actor() from public,anon,authenticated;
drop trigger if exists cpu_deployment_actor on public.cpu_projects;
create trigger cpu_deployment_actor before insert or update on public.cpu_projects for each row execute function cbgpu_schools.guard_cpu_request_actor();
commit;
