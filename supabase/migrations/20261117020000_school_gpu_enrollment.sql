-- Candidate only. Inventory enrollment never grants compute access or sharing.
begin;
alter table cbgpu_schools.gpu_nodes add column if not exists public_key text;
alter table cbgpu_schools.gpu_nodes add column if not exists credential_version bigint not null default 1;
alter table cbgpu_schools.gpu_nodes add column if not exists last_sequence bigint not null default 0;
alter table cbgpu_schools.gpu_nodes add column if not exists last_seen_at timestamptz;
alter table cbgpu_schools.gpu_nodes add column if not exists last_reported_at timestamptz;
alter table cbgpu_schools.gpu_nodes add column if not exists enrolled_by uuid references auth.users(id);
alter table cbgpu_schools.gpu_nodes add column if not exists revoked_at timestamptz;
create or replace function cbgpu_schools.summary(s cbgpu_schools.schools) returns jsonb language sql stable security definer set search_path='' as $$select jsonb_build_object('id',s.id,'official_name',s.official_name,'display_name',s.display_name,'type',s.type,'region',s.region,'brand_color',s.brand_color,'logo_path',s.logo_path,'status',s.status,'features',s.features,'ads_enabled',coalesce((s.configuration->>'ads_enabled')::boolean,false),'allowed_membership_types',jsonb_build_array('STUDENT','TEACHER','EXTERNAL'),'join_policy',s.configuration->>'join_policy','student_id_policy',s.configuration->>'student_id_policy','teacher_policy',s.configuration->>'teacher_policy','gpu',jsonb_build_object('status',case when exists(select 1 from cbgpu_schools.gpu_nodes n where n.school_id=s.id and s.status='APPROVED' and n.status='ONLINE' and n.last_seen_at>=now()-interval '120 seconds') then 'ONLINE' when exists(select 1 from cbgpu_schools.gpu_nodes n where n.school_id=s.id) then 'OFFLINE' else 'NOT_CONFIGURED' end,'nodes',(select count(*) from cbgpu_schools.gpu_nodes n where n.school_id=s.id),'sharing_enabled',false))$$;
create table if not exists cbgpu_schools.gpu_enrollments(
 id uuid primary key default gen_random_uuid(),node_id uuid not null references cbgpu_schools.gpu_nodes(id),school_id text not null references cbgpu_schools.schools(id),
 token_hash bytea not null unique,issuer_id uuid not null references auth.users(id),issuer_session_id uuid not null,expires_at timestamptz not null,used_at timestamptz,created_at timestamptz not null default now()
);
alter table cbgpu_schools.gpu_enrollments enable row level security;
revoke all on cbgpu_schools.gpu_enrollments from public,anon,authenticated,service_role;
-- Default schema/table grants must not expose keys, tokens or report-writing capability.
revoke all on cbgpu_schools.gpu_nodes from public,anon,authenticated,service_role;
create or replace function public.school_gpu_admin(p_school_id text,p_action text,p_payload jsonb default '{}') returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();token text;nid uuid;node cbgpu_schools.gpu_nodes;name text;
begin
 if uid is null or not cbgpu_accounts.current_session_valid() or not cbgpu_schools.has_role(uid,p_school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN','GPU_ADMIN']) then raise exception 'GPU_ADMIN_REQUIRED' using errcode='42501';end if;
 if jsonb_typeof(p_payload)<>'object' then raise exception 'INVALID_REQUEST';end if;
 if p_action='mint' then
  if exists(select 1 from jsonb_object_keys(p_payload) k where k<>'name') then raise exception 'INVALID_REQUEST';end if;
  name:=trim(p_payload->>'name');if name is null or length(name) not between 1 and 100 or name~'[[:cntrl:]]' then raise exception 'INVALID_NAME';end if;
  -- Limit pending tokens per school and serialize minting against concurrent requests.
  perform 1 from cbgpu_schools.schools where id=p_school_id for update;
  if (select count(*) from cbgpu_schools.gpu_enrollments e where e.school_id=p_school_id and e.used_at is null and e.expires_at>now())>=20 then raise exception 'TOO_MANY_PENDING_ENROLLMENTS';end if;
  token:='gpuenr_'||encode(extensions.gen_random_bytes(32),'hex');
  insert into cbgpu_schools.gpu_nodes(school_id,name,status,sharing_enabled,enrolled_by) values(p_school_id,name,'PENDING',false,uid) returning id into nid;
  insert into cbgpu_schools.gpu_enrollments(node_id,school_id,token_hash,issuer_id,issuer_session_id,expires_at) values(nid,p_school_id,pg_catalog.sha256(convert_to(token,'UTF8')),uid,(auth.jwt()->>'session_id')::uuid,now()+interval '10 minutes');
  insert into cbgpu_schools.audit(actor_id,school_id,action,target_id) values(uid,p_school_id,'GPU_ENROLLMENT_MINTED',nid);
  return jsonb_build_object('node_id',nid,'school_id',p_school_id,'token',token,'expires_at',now()+interval '10 minutes','sharing_enabled',false);
 elsif p_action='list' then
  if p_payload<>'{}'::jsonb then raise exception 'INVALID_REQUEST';end if;
  return jsonb_build_object('items',(select coalesce(jsonb_agg(jsonb_build_object('id',n.id,'name',n.name,'school_id',n.school_id,'status',case when n.status='ONLINE' and n.last_seen_at<now()-interval '120 seconds' then 'OFFLINE' else n.status end,'sharing_enabled',n.sharing_enabled,'credential_version',n.credential_version,'last_seen_at',n.last_seen_at,'health',n.capabilities,'created_at',n.created_at) order by n.created_at desc),'[]') from cbgpu_schools.gpu_nodes n where n.school_id=p_school_id));
 elsif p_action='revoke' then
  if exists(select 1 from jsonb_object_keys(p_payload) k where k<>'node_id') then raise exception 'INVALID_REQUEST';end if;
  select * into node from cbgpu_schools.gpu_nodes where id=(p_payload->>'node_id')::uuid and school_id=p_school_id for update;
  if not found then raise exception 'NODE_NOT_FOUND' using errcode='42501';end if;
  if node.status<>'REVOKED' then
   update cbgpu_schools.gpu_nodes set status='REVOKED',sharing_enabled=false,credential_version=credential_version+1,revoked_at=now() where id=node.id;
   update cbgpu_schools.gpu_enrollments set expires_at=least(expires_at,now()) where node_id=node.id and used_at is null;
   insert into cbgpu_schools.audit(actor_id,school_id,action,target_id) values(uid,p_school_id,'GPU_NODE_REVOKED',node.id);
  end if;
  return jsonb_build_object('ok',true);
 end if;
 raise exception 'INVALID_ACTION';
end$$;
create or replace function public.school_gpu_enroll(p_token text,p_public_key text) returns jsonb language plpgsql security definer set search_path='' as $$
declare enrollment cbgpu_schools.gpu_enrollments;node cbgpu_schools.gpu_nodes;
begin
 if p_token !~ '^gpuenr_[a-f0-9]{64}$' or p_public_key !~ '^[a-f0-9]{64}$' or p_token is null or p_public_key is null then raise exception 'INVALID_ENROLLMENT' using errcode='42501';end if;
 select * into enrollment from cbgpu_schools.gpu_enrollments where token_hash=pg_catalog.sha256(convert_to(p_token,'UTF8')) for update;
 if not found or enrollment.used_at is not null or enrollment.expires_at<=clock_timestamp() or not exists(select 1 from auth.sessions a where a.id=enrollment.issuer_session_id and a.user_id=enrollment.issuer_id and(a.not_after is null or a.not_after>clock_timestamp())) or not cbgpu_schools.has_role(enrollment.issuer_id,enrollment.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN','GPU_ADMIN']) then raise exception 'INVALID_ENROLLMENT' using errcode='42501';end if;
 select * into node from cbgpu_schools.gpu_nodes where id=enrollment.node_id for update;
 if node.status<>'PENDING' or node.public_key is not null or node.sharing_enabled then raise exception 'INVALID_ENROLLMENT' using errcode='42501';end if;
 update cbgpu_schools.gpu_enrollments set used_at=clock_timestamp() where id=enrollment.id;
 update cbgpu_schools.gpu_nodes set public_key=p_public_key,status='OFFLINE',sharing_enabled=false where id=node.id;
 insert into cbgpu_schools.audit(actor_id,school_id,action,target_id) values(enrollment.issuer_id,enrollment.school_id,'GPU_NODE_ENROLLED',node.id);
 return jsonb_build_object('node_id',node.id,'school_id',node.school_id,'credential_version',node.credential_version,'sharing_enabled',false,'execution_enabled',false);
end$$;
create or replace function public.school_gpu_node_identity(p_node_id uuid) returns jsonb language plpgsql security definer set search_path='' as $$
declare node cbgpu_schools.gpu_nodes;
begin
 if auth.role() is distinct from 'service_role' then raise exception 'SERVICE_REQUIRED' using errcode='42501';end if;
 select n.* into node from cbgpu_schools.gpu_nodes n join cbgpu_schools.schools s on s.id=n.school_id and s.status='APPROVED' where n.id=p_node_id and n.status in('ONLINE','OFFLINE') and n.public_key is not null;
 if not found then raise exception 'NODE_UNAVAILABLE' using errcode='42501';end if;
 return jsonb_build_object('public_key',node.public_key,'credential_version',node.credential_version);
end$$;
create or replace function public.school_gpu_store_report(p_node_id uuid,p_version bigint,p_sequence bigint,p_reported_at bigint,p_health jsonb) returns jsonb language plpgsql security definer set search_path='' as $$
declare node cbgpu_schools.gpu_nodes;ts timestamptz;g jsonb;
begin
 if auth.role() is distinct from 'service_role' then raise exception 'SERVICE_REQUIRED' using errcode='42501';end if;
 if p_reported_at is null or p_reported_at<0 or p_reported_at>4000000000 then raise exception 'INVALID_REPORT';end if;
 ts:=to_timestamp(p_reported_at);
 if abs(extract(epoch from clock_timestamp()-ts))>90 or p_sequence is null or p_sequence<1 or p_version is null or p_version<1 or p_health is null or jsonb_typeof(p_health)<>'object' or octet_length(p_health::text)>12000 then raise exception 'INVALID_REPORT';end if;
 if p_health->>'agent_version' is distinct from '0.1.0' or p_health->>'os' is distinct from 'ubuntu' or jsonb_typeof(p_health->'gpus') is distinct from 'array' or jsonb_array_length(p_health->'gpus') not between 1 and 16 or exists(select 1 from jsonb_object_keys(p_health) k where k not in('agent_version','os','gpus')) then raise exception 'INVALID_REPORT';end if;
 for g in select value from jsonb_array_elements(p_health->'gpus') loop
  if jsonb_typeof(g) is distinct from 'object' or not(g ?& array['index','name','memory_total_mib','memory_used_mib','utilization_percent']) or exists(select 1 from jsonb_object_keys(g) k where k not in('index','name','memory_total_mib','memory_used_mib','utilization_percent')) or exists(select 1 from jsonb_each(g) kv where kv.value='null'::jsonb) or g->>'name'~'[[:cntrl:]]' or length(g->>'name') not between 1 and 120 or (g->>'index')::integer not between 0 and 255 or (g->>'utilization_percent')::integer not between 0 and 100 or (g->>'memory_total_mib')::bigint not between 1 and 16777216 or (g->>'memory_used_mib')::bigint not between 0 and (g->>'memory_total_mib')::bigint then raise exception 'INVALID_REPORT';end if;
 end loop;
 if exists(select 1 from jsonb_array_elements(p_health->'gpus') as readings(item) group by (readings.item->>'index')::int having count(*)>1) then raise exception 'INVALID_REPORT';end if;
 select n.* into node from cbgpu_schools.gpu_nodes n join cbgpu_schools.schools s on s.id=n.school_id and s.status='APPROVED' where n.id=p_node_id for update of n;
 if not found or node.status not in('ONLINE','OFFLINE') or node.public_key is null or node.credential_version<>p_version or p_sequence<=node.last_sequence then raise exception 'NODE_REVOKED_OR_STALE' using errcode='42501';end if;
 update cbgpu_schools.gpu_nodes set status='ONLINE',sharing_enabled=false,last_seen_at=clock_timestamp(),last_reported_at=ts,last_sequence=p_sequence,capabilities=p_health where id=p_node_id;
 return jsonb_build_object('ok',true,'execution_enabled',false,'sharing_enabled',false,'stale_after_seconds',120);
end$$;
revoke all on function public.school_gpu_admin(text,text,jsonb),public.school_gpu_enroll(text,text),public.school_gpu_node_identity(uuid),public.school_gpu_store_report(uuid,bigint,bigint,bigint,jsonb) from public,anon,authenticated,service_role;
grant execute on function public.school_gpu_admin(text,text,jsonb) to authenticated;
grant execute on function public.school_gpu_enroll(text,text) to anon,authenticated;
grant execute on function public.school_gpu_node_identity(uuid),public.school_gpu_store_report(uuid,bigint,bigint,bigint,jsonb) to service_role;
commit;
