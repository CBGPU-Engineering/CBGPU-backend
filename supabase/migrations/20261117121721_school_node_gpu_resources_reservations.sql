-- Candidate only: normalized inventory and reservations, NOT an executor or notebook transport.
create extension if not exists btree_gist with schema extensions;
create table if not exists cbgpu_schools.gpu_resources(
 id uuid primary key default gen_random_uuid(),node_id uuid not null references cbgpu_schools.gpu_nodes,hardware_uuid text not null check(hardware_uuid~'^GPU-[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$'),gpu_index integer not null,name text not null,memory_total_mib bigint not null,last_seen_at timestamptz not null,reported_sequence bigint not null,credential_version bigint not null,
 executor_protocol integer not null default 0 check(executor_protocol in(0,1)),execution_enabled boolean not null default false,unique(node_id,hardware_uuid));
create table if not exists cbgpu_schools.gpu_resource_reservations(
 id uuid primary key default gen_random_uuid(),resource_id uuid not null references cbgpu_schools.gpu_resources,owner_id uuid not null references auth.users,starts_at timestamptz not null,ends_at timestamptz not null,status text not null default 'RESERVED' check(status in('RESERVED','CANCELLED','REVOKED')),created_at timestamptz not null default now(),check(starts_at<ends_at),exclude using gist(resource_id with =,tstzrange(starts_at,ends_at,'[)') with &&) where(status='RESERVED'));
alter table cbgpu_schools.gpu_resources enable row level security;alter table cbgpu_schools.gpu_resource_reservations enable row level security;
revoke all on cbgpu_schools.gpu_resources,cbgpu_schools.gpu_resource_reservations from public,anon,authenticated,service_role;
create or replace function cbgpu_schools.gpu_resource_state(r cbgpu_schools.gpu_resources)returns text language sql stable security definer set search_path='' as $$select case when n.status='REVOKED' or n.credential_version<>r.credential_version then 'REVOKED' when s.status<>'APPROVED' then 'DISABLED' when n.status<>'ONLINE' or n.last_seen_at<clock_timestamp()-interval '120 seconds' or r.last_seen_at<clock_timestamp()-interval '120 seconds' or r.reported_sequence<>n.last_sequence then 'OFFLINE' when not r.execution_enabled or r.executor_protocol<>1 then 'NONRUNNABLE' else 'READY' end from cbgpu_schools.gpu_nodes n join cbgpu_schools.schools s on s.id=n.school_id where n.id=r.node_id$$;
create or replace function public.school_gpu_store_resource_report(p_node_id uuid,p_version bigint,p_sequence bigint,p_reported_at bigint,p_health jsonb)returns jsonb language plpgsql security definer set search_path='' as $$
declare stripped jsonb;g jsonb;n cbgpu_schools.gpu_nodes;begin
 if auth.role() is distinct from 'service_role' then raise exception 'SERVICE_REQUIRED' using errcode='42501';end if;
 if jsonb_typeof(p_health->'gpus') is distinct from 'array' or jsonb_array_length(p_health->'gpus') not between 1 and 16 then raise exception 'INVALID_REPORT' using errcode='22023';end if;
 for g in select value from jsonb_array_elements(p_health->'gpus')loop if g?'hardware_uuid' and(g->>'hardware_uuid' is null or g->>'hardware_uuid'!~'^GPU-[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$')then raise exception 'INVALID_HARDWARE_UUID' using errcode='22023';end if;end loop;
 if exists(select 1 from jsonb_array_elements(p_health->'gpus') readings(item) where item?'hardware_uuid' group by lower(item->>'hardware_uuid')having count(*)>1)then raise exception 'DUPLICATE_HARDWARE_UUID' using errcode='22023';end if;
 stripped:=jsonb_set(p_health,'{gpus}',(select jsonb_agg(value-'hardware_uuid') from jsonb_array_elements(p_health->'gpus')));
 perform public.school_gpu_store_report(p_node_id,p_version,p_sequence,p_reported_at,stripped);
 select * into n from cbgpu_schools.gpu_nodes where id=p_node_id;
 for g in select value from jsonb_array_elements(p_health->'gpus') where value?'hardware_uuid' loop
  insert into cbgpu_schools.gpu_resources(node_id,hardware_uuid,gpu_index,name,memory_total_mib,last_seen_at,reported_sequence,credential_version)values(n.id,'GPU-'||lower(substr(g->>'hardware_uuid',5)),(g->>'index')::int,g->>'name',(g->>'memory_total_mib')::bigint,clock_timestamp(),p_sequence,p_version)
  on conflict(node_id,hardware_uuid)do update set gpu_index=excluded.gpu_index,name=excluded.name,memory_total_mib=excluded.memory_total_mib,last_seen_at=excluded.last_seen_at,reported_sequence=excluded.reported_sequence,credential_version=excluded.credential_version;
 end loop;
 return jsonb_build_object('ok',true,'execution_enabled',false,'sharing_enabled',false,'resource_discovery',true,'stale_after_seconds',120);
end$$;
create or replace function cbgpu_schools.node_gpu_permission(subject uuid,school text)returns boolean language sql stable security definer set search_path='' as $$
select cbgpu_schools.feature_enabled(school,'gpu_compute') and exists(select 1 from public.profiles p join auth.users u on u.id=p.id where p.id=subject and p.account_status in('ACTIVE','PENDING_VERIFICATION','REVERIFICATION_REQUIRED') and u.email_confirmed_at is not null and u.deleted_at is null and(u.banned_until is null or u.banned_until<=clock_timestamp())) and(
 exists(select 1 from cbgpu_schools.memberships m where m.user_id=subject and m.school_id=school and m.membership_type in('STUDENT','TEACHER') and cbgpu_schools.active_member(subject,school)) or
 exists(select 1 from cbgpu_accounts.guest_requests g where g.external_user_id=subject and g.school_id=school and g.status='APPROVED' and g.starts_at<=clock_timestamp() and g.ends_at>clock_timestamp() and 'gpu'=any(g.resources) and cbgpu_schools.active_member(g.requester_id,school) and exists(select 1 from cbgpu_schools.memberships sponsor where sponsor.user_id=g.requester_id and sponsor.school_id=school and sponsor.membership_type in('STUDENT','TEACHER'))))
$$;
revoke all on function cbgpu_schools.node_gpu_permission(uuid,text) from public,anon,authenticated,service_role;
create or replace function public.school_gpu_resource_api(p_school_id text,p_action text,p_payload jsonb default '{}')returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();resource_row cbgpu_schools.gpu_resources;booking cbgpu_schools.gpu_resource_reservations;start_time timestamptz;end_time timestamptz;begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if jsonb_typeof(p_payload)<>'object' or octet_length(p_payload::text)>4096 then raise exception 'INVALID_REQUEST' using errcode='22023';end if;
 if p_action='own' then return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(x) order by x.created_at desc),'[]')from(select b.* from cbgpu_schools.gpu_resource_reservations b join cbgpu_schools.gpu_resources r on r.id=b.resource_id join cbgpu_schools.gpu_nodes n on n.id=r.node_id where b.owner_id=uid and n.school_id=p_school_id order by b.created_at desc limit 100)x));end if;
 if p_action='cancel' then
  if exists(select 1 from jsonb_object_keys(p_payload)k where k<>'reservation_id')then raise exception 'INVALID_REQUEST' using errcode='22023';end if;
  update cbgpu_schools.gpu_resource_reservations b set status='CANCELLED' where b.id=(p_payload->>'reservation_id')::uuid and b.owner_id=uid and b.status='RESERVED' and exists(select 1 from cbgpu_schools.gpu_resources r join cbgpu_schools.gpu_nodes n on n.id=r.node_id where r.id=b.resource_id and n.school_id=p_school_id)returning b.* into booking;
  if not found then raise exception 'RESERVATION_NOT_FOUND' using errcode='P0002';end if;return jsonb_build_object('reservation',to_jsonb(booking));
 end if;
 if not cbgpu_schools.node_gpu_permission(uid,p_school_id) then raise exception 'GPU_PERMISSION_REQUIRED' using errcode='42501';end if;
 if p_action='list' then
  if p_payload<>'{}'::jsonb then raise exception 'INVALID_REQUEST' using errcode='22023';end if;
  return jsonb_build_object('items',(select coalesce(jsonb_agg(jsonb_build_object('id',r.id,'node_id',r.node_id,'school_id',n.school_id,'name',r.name,'memory_total_mib',r.memory_total_mib,'state',cbgpu_schools.gpu_resource_state(r),'reason',case cbgpu_schools.gpu_resource_state(r) when 'NONRUNNABLE' then 'EXECUTOR_NOT_CONFIGURED' when 'OFFLINE' then 'SIGNED_REPORT_STALE_OR_MISSING' when 'REVOKED' then 'NODE_CREDENTIAL_REVOKED' when 'DISABLED' then 'SCHOOL_NOT_APPROVED' else 'RESERVATION_ONLY_NO_EXECUTION_TRANSPORT' end,'execution_enabled',r.execution_enabled and r.executor_protocol=1,'sharing_enabled',false) order by n.created_at,r.gpu_index),'[]')from cbgpu_schools.gpu_resources r join cbgpu_schools.gpu_nodes n on n.id=r.node_id where n.school_id=p_school_id),'execution_transport_available',false);
 elsif p_action='reserve' then
  if exists(select 1 from jsonb_object_keys(p_payload)k where k not in('resource_id','starts_at','ends_at'))then raise exception 'INVALID_REQUEST' using errcode='22023';end if;
  perform pg_advisory_xact_lock(hashtextextended('school-gpu-owner:'||uid::text,0));
  select r0.* into resource_row from cbgpu_schools.gpu_resources r0 join cbgpu_schools.gpu_nodes n on n.id=r0.node_id where r0.id=(p_payload->>'resource_id')::uuid and n.school_id=p_school_id for update of r0;
  if not found then raise exception 'RESOURCE_NOT_FOUND' using errcode='P0002';end if;
  if cbgpu_schools.gpu_resource_state(resource_row) is distinct from 'READY' then raise exception 'GPU_NONRUNNABLE' using errcode='55000';end if;
  start_time:=(p_payload->>'starts_at')::timestamptz;end_time:=(p_payload->>'ends_at')::timestamptz;
  if start_time is null or end_time is null or start_time<clock_timestamp() or start_time>clock_timestamp()+interval '7 days' or end_time<=start_time or end_time>start_time+interval '4 hours' then raise exception 'INVALID_RESERVATION_WINDOW' using errcode='22023';end if;
  if(select count(*)from cbgpu_schools.gpu_resource_reservations where owner_id=uid and status='RESERVED' and ends_at>clock_timestamp())>=2 then raise exception 'GPU_RESERVATION_QUOTA' using errcode='54000';end if;
  -- Permission must cover the entire future interval; checked again at future execution.
  if not exists(select 1 from cbgpu_schools.memberships m where m.user_id=uid and m.school_id=p_school_id and m.membership_type in('STUDENT','TEACHER') and cbgpu_schools.active_member(uid,p_school_id) and m.starts_at<=start_time and(m.ends_at is null or m.ends_at>=end_time)) and not exists(select 1 from cbgpu_accounts.guest_requests g where g.external_user_id=uid and g.school_id=p_school_id and g.status='APPROVED' and g.starts_at<=start_time and g.ends_at>=end_time and 'gpu'=any(g.resources))then raise exception 'GPU_GRANT_WINDOW_REQUIRED' using errcode='42501';end if;
  if not cbgpu_schools.node_gpu_permission(uid,p_school_id) then raise exception 'GPU_PERMISSION_REQUIRED' using errcode='42501';end if;
  insert into cbgpu_schools.gpu_resource_reservations(resource_id,owner_id,starts_at,ends_at)values(resource_row.id,uid,start_time,end_time)returning * into booking;return jsonb_build_object('reservation',to_jsonb(booking),'execution_transport_available',false);
 end if;raise exception 'INVALID_ACTION' using errcode='22023';
end$$;
revoke all on function cbgpu_schools.gpu_resource_state(cbgpu_schools.gpu_resources) from public,anon,authenticated,service_role;
revoke all on function public.school_gpu_store_resource_report(uuid,bigint,bigint,bigint,jsonb) from public,anon,authenticated;grant execute on function public.school_gpu_store_resource_report(uuid,bigint,bigint,bigint,jsonb) to service_role;
revoke all on function public.school_gpu_resource_api(text,text,jsonb) from public,anon,service_role;grant execute on function public.school_gpu_resource_api(text,text,jsonb) to authenticated;
