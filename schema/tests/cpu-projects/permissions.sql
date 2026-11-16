-- Run against the linked DB with `supabase db query --linked --file ...`.
-- All test rows are rolled back, so the daemon never sees a project to deploy.
begin;
select set_config('request.jwt.claim.sub', (
  select p.id::text from public.profiles p join auth.users u on u.id=p.id
  where p.is_admin is not true and p.is_approved is true limit 1
), true);
select set_config('request.jwt.claim.role', 'authenticated', true);
set local role authenticated;
do $$
declare
  project_id uuid;
  project_port int;
  attempt text;
begin
  if auth.uid() is null then raise exception 'An approved non-admin fixture is required'; end if;
  select port into project_port from generate_series(40000,65000) port
  where not exists (select 1 from public.cpu_projects where host_port=port) limit 1;
  insert into public.cpu_projects(user_id,user_email,name,deploy_type,docker_image,host_port)
  values(auth.uid(),'rls-test@example.invalid','RLS transaction test','docker','nginx:alpine',project_port)
  returning id into project_id;
  if not exists(select 1 from public.cpu_projects where id=project_id and status='pending') then
    raise exception 'Student cannot read own pending request';
  end if;
  foreach attempt in array array['approved','rejected'] loop
    begin
      insert into public.cpu_projects(user_id,user_email,name,deploy_type,host_port,status)
      values(auth.uid(),'rls-test@example.invalid','Invalid state','docker',project_port+1,attempt);
      raise exception 'Unexpected insert permission: %',attempt;
    exception when insufficient_privilege then null;
    end;
  end loop;
  begin
    insert into public.cpu_projects(user_id,user_email,name,deploy_type,host_port)
    values(gen_random_uuid(),'rls-test@example.invalid','Other owner','docker',project_port+1);
    raise exception 'Unexpected permission to create for another user';
  exception when insufficient_privilege then null;
  end;
  begin
    update public.cpu_projects set status='approved' where id=project_id;
    raise exception 'Unexpected self-approval permission';
  exception when insufficient_privilege then null;
  end;
  begin
    update public.cpu_projects set deployed_at=now() where id=project_id;
    raise exception 'Unexpected deployment field permission';
  exception when insufficient_privilege then null;
  end;
  update public.cpu_projects set name='Editable request' where id=project_id;
  update public.cpu_projects set status='deleted' where id=project_id;
end;
$$;
reset role;
rollback;
select 'PASS: student create/read/edit/cancel; forged owner/state and self-approval denied; test rows rolled back' as result;
