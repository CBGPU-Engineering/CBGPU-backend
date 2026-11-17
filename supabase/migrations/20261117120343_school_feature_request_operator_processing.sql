-- Source candidate: human processing never enables a feature or changes school permissions.
alter table cbgpu_schools.feature_requests add column if not exists revision integer not null default 1;
alter table cbgpu_schools.feature_requests add column if not exists operator_message text not null default '';
alter table cbgpu_schools.feature_requests add column if not exists updated_at timestamptz not null default now();
create table if not exists cbgpu_schools.feature_request_reviews(id uuid primary key default gen_random_uuid(),request_id uuid not null references cbgpu_schools.feature_requests,reviewer_id uuid not null references auth.users,previous_status text not null,status text not null,message text not null,revision integer not null,created_at timestamptz not null default clock_timestamp());
alter table cbgpu_schools.feature_request_reviews enable row level security;
revoke all on cbgpu_schools.feature_request_reviews from public,anon,authenticated,service_role;
create or replace function cbgpu_schools.feature_request_json(r cbgpu_schools.feature_requests) returns jsonb language sql stable security definer set search_path='' as $$select to_jsonb(r)||jsonb_build_object('school_display_name',(select display_name from cbgpu_schools.schools where id=r.school_id),'notification_status',(select status from cbgpu_schools.mail_outbox where request_id=r.id),'notification_basis','PROVIDER_ACCEPTANCE_NOT_DELIVERY')$$;
revoke all on function cbgpu_schools.feature_request_json(cbgpu_schools.feature_requests) from public,anon,authenticated,service_role;
create or replace function public.cbgpu_school_feature_operator_api(action text,payload jsonb default '{}') returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();r cbgpu_schools.feature_requests;decision text:=payload->>'decision';message text:=coalesce(payload->>'operator_message','');lim integer;off integer;begin
 if not public.cbgpu_personal_project_identity() or not exists(select 1 from public.profiles where id=uid and is_admin) then raise exception 'PLATFORM_ADMIN_REQUIRED' using errcode='42501';end if;
 if jsonb_typeof(payload)<>'object' or octet_length(payload::text)>16384 then raise exception 'INVALID_REQUEST' using errcode='22023';end if;
 if action='list' then
  if exists(select 1 from jsonb_object_keys(payload) k where k not in('school_id','status','limit','offset')) or(payload?'status' and payload->>'status' not in('PENDING','REVIEWING','ACCEPTED','REJECTED','DELIVERED')) then raise exception 'INVALID_FILTER' using errcode='22023';end if;
  lim:=least(100,greatest(1,coalesce((payload->>'limit')::int,30)));off:=greatest(0,coalesce((payload->>'offset')::int,0));
  return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.feature_request_json(t::cbgpu_schools.feature_requests) order by t.created_at desc,t.id),'[]') from(select * from cbgpu_schools.feature_requests f where(not(payload?'school_id') or f.school_id=payload->>'school_id') and(not(payload?'status') or f.status=payload->>'status') order by f.created_at desc,f.id limit lim offset off)t));
 elsif action='review' then
  if exists(select 1 from jsonb_object_keys(payload) k where k not in('id','revision','decision','operator_message')) or decision is null or decision not in('REVIEWING','ACCEPTED','REJECTED','DELIVERED') or length(message)>4000 then raise exception 'INVALID_REVIEW' using errcode='22023';end if;
  select * into r from cbgpu_schools.feature_requests where id=(payload->>'id')::uuid for update;
  if not found then raise exception 'REQUEST_NOT_FOUND' using errcode='P0002';end if;
  if (payload->>'revision')::int is distinct from r.revision then raise exception 'REVISION_CONFLICT' using errcode='40001';end if;
  if not((r.status='PENDING' and decision in('REVIEWING','ACCEPTED','REJECTED')) or(r.status='REVIEWING' and decision in('ACCEPTED','REJECTED')) or(r.status='ACCEPTED' and decision in('DELIVERED','REJECTED'))) then raise exception 'INVALID_TRANSITION' using errcode='22023';end if;
  insert into cbgpu_schools.feature_request_reviews(request_id,reviewer_id,previous_status,status,message,revision) values(r.id,uid,r.status,decision,message,r.revision+1);
  update cbgpu_schools.feature_requests set status=decision,operator_message=message,revision=revision+1,updated_at=clock_timestamp() where id=r.id returning * into r;
  return jsonb_build_object('request',cbgpu_schools.feature_request_json(r));
 end if;
 raise exception 'INVALID_ACTION' using errcode='22023';
end$$;
revoke all on function public.cbgpu_school_feature_operator_api(text,jsonb) from public,anon,service_role;
grant execute on function public.cbgpu_school_feature_operator_api(text,jsonb) to authenticated;
