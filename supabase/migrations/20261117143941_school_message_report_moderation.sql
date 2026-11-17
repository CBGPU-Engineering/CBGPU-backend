begin;
-- Report content is available only through scoped, fresh-session moderation.
alter table cbgpu_schools.message_reports add column if not exists status text not null default 'PENDING' check(status in('PENDING','REVIEWING','RESOLVED','DISMISSED'));
alter table cbgpu_schools.message_reports add column if not exists revision integer not null default 1;
alter table cbgpu_schools.message_reports add column if not exists review_note text not null default '';
alter table cbgpu_schools.message_reports add column if not exists reviewed_by uuid references auth.users;
alter table cbgpu_schools.message_reports add column if not exists reviewed_at timestamptz;
alter table cbgpu_schools.direct_message_reports add column if not exists status text not null default 'PENDING' check(status in('PENDING','REVIEWING','RESOLVED','DISMISSED'));
alter table cbgpu_schools.direct_message_reports add column if not exists revision integer not null default 1;
alter table cbgpu_schools.direct_message_reports add column if not exists review_note text not null default '';
alter table cbgpu_schools.direct_message_reports add column if not exists reviewed_by uuid references auth.users;
alter table cbgpu_schools.direct_message_reports add column if not exists reviewed_at timestamptz;
create table if not exists cbgpu_schools.message_moderation_audit(id bigint generated always as identity primary key,report_kind text not null,report_id uuid not null,actor_id uuid not null references auth.users,previous_status text not null,status text not null,review_note text not null,hidden boolean not null,created_at timestamptz not null default now());
alter table cbgpu_schools.message_moderation_audit enable row level security;
revoke all on cbgpu_schools.message_moderation_audit from public,anon,authenticated,service_role;
create or replace view cbgpu_schools.moderation_reports as
select r.id,'GROUP'::text kind,r.message_id,m.scope,m.school_id,left(m.body,500)body_snippet,r.reason,r.created_at,r.status,r.revision,r.review_note,r.reviewed_at,m.deleted_at is not null hidden,r.reporter_id from cbgpu_schools.message_reports r join cbgpu_schools.messages m on m.id=r.message_id
union all select r.id,'DM',r.message_id,'DM',null,left(m.body,500),r.reason,r.created_at,r.status,r.revision,r.review_note,r.reviewed_at,m.deleted_at is not null,r.reporter_id from cbgpu_schools.direct_message_reports r join cbgpu_schools.direct_messages m on m.id=r.message_id;
revoke all on cbgpu_schools.moderation_reports from public,anon,authenticated,service_role;
create or replace function public.cbgpu_message_moderation(action text,payload jsonb default '{}')returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();platform boolean;channel text:=payload->>'scope';sid text:=payload->>'school_id';lim integer;off integer;r record;previous text;hide boolean;result jsonb;
begin
 if not public.cbgpu_personal_project_identity()then raise exception 'AUTHENTICATION_REQUIRED'using errcode='42501';end if;
 if jsonb_typeof(payload)is distinct from 'object'or octet_length(payload::text)>10000 then raise exception 'INVALID_REQUEST'using errcode='22023';end if;
 select coalesce(p.is_admin,false)into platform from public.profiles p where p.id=uid;platform:=coalesce(platform,false);
 if action='list'then
  if exists(select 1 from jsonb_object_keys(payload)k where k not in('scope','school_id','status','limit','offset'))or channel not in('SCHOOL','NATIONWIDE','DM')or channel is null then raise exception 'INVALID_REQUEST'using errcode='22023';end if;
  if(channel='SCHOOL'and(sid is null or not(platform or cbgpu_schools.has_role(uid,sid,array['SCHOOL_OWNER','SCHOOL_ADMIN','COMMUNITY_MODERATOR']))))or(channel<>'SCHOOL'and not platform)then raise exception 'MODERATION_DENIED'using errcode='42501';end if;
  if payload?'status'and payload->>'status'not in('PENDING','REVIEWING','RESOLVED','DISMISSED')then raise exception 'INVALID_STATUS'using errcode='22023';end if;
  lim:=coalesce((payload->>'limit')::integer,30);off:=coalesce((payload->>'offset')::integer,0);if lim not between 1 and 100 or off not between 0 and 10000 then raise exception 'INVALID_PAGE'using errcode='22023';end if;
  return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(x)-'reporter_id'order by x.created_at desc,x.id),'[]')from(select * from cbgpu_schools.moderation_reports z where z.scope=channel and(channel<>'SCHOOL'or z.school_id=sid)and(not(payload?'status')or z.status=payload->>'status')order by z.created_at desc,z.id limit lim offset off)x),'total',(select count(*)from cbgpu_schools.moderation_reports z where z.scope=channel and(channel<>'SCHOOL'or z.school_id=sid)and(not(payload?'status')or z.status=payload->>'status')));
 elsif action='review'then
  if exists(select 1 from jsonb_object_keys(payload)k where k not in('kind','report_id','expected_revision','status','review_note','hide','confirmed'))or payload->>'kind'not in('GROUP','DM')or payload->>'kind'is null or payload->>'status'not in('PENDING','REVIEWING','RESOLVED','DISMISSED')or payload->>'status'is null or jsonb_typeof(payload->'review_note')is distinct from 'string'or length(payload->>'review_note')>2000 then raise exception 'INVALID_REVIEW'using errcode='22023';end if;
  if payload->>'kind'='GROUP'then perform 1 from cbgpu_schools.message_reports where id=(payload->>'report_id')::uuid for update;else perform 1 from cbgpu_schools.direct_message_reports where id=(payload->>'report_id')::uuid for update;end if;
  select * into r from cbgpu_schools.moderation_reports where id=(payload->>'report_id')::uuid and kind=payload->>'kind';
  if not found then raise exception 'REPORT_NOT_FOUND'using errcode='P0002';end if;
  if(r.scope='SCHOOL'and not(platform or cbgpu_schools.has_role(uid,r.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN','COMMUNITY_MODERATOR'])))or(r.scope<>'SCHOOL'and not platform)then raise exception 'MODERATION_DENIED'using errcode='42501';end if;
  if r.reporter_id=uid then raise exception 'INDEPENDENT_REVIEW_REQUIRED'using errcode='42501';end if;
  if r.revision is distinct from(payload->>'expected_revision')::integer then raise exception 'REVISION_CONFLICT'using errcode='40001';end if;
  if(payload?'hide'and jsonb_typeof(payload->'hide')<>'boolean')or(payload?'confirmed'and jsonb_typeof(payload->'confirmed')<>'boolean')then raise exception 'INVALID_CONFIRMATION'using errcode='22023';end if;
  hide:=coalesce((payload->>'hide')::boolean,false);if hide and payload->'confirmed'is distinct from 'true'::jsonb then raise exception 'EXPLICIT_HIDE_CONFIRMATION_REQUIRED'using errcode='22023';end if;previous:=r.status;
  if r.kind='GROUP'then update cbgpu_schools.message_reports set status=payload->>'status',revision=revision+1,review_note=payload->>'review_note',reviewed_by=uid,reviewed_at=clock_timestamp()where id=r.id;if hide then update cbgpu_schools.messages set deleted_at=coalesce(deleted_at,clock_timestamp())where id=r.message_id;end if;
  else update cbgpu_schools.direct_message_reports set status=payload->>'status',revision=revision+1,review_note=payload->>'review_note',reviewed_by=uid,reviewed_at=clock_timestamp()where id=r.id;if hide then update cbgpu_schools.direct_messages set deleted_at=coalesce(deleted_at,clock_timestamp())where id=r.message_id;end if;end if;
  insert into cbgpu_schools.message_moderation_audit(report_kind,report_id,actor_id,previous_status,status,review_note,hidden)values(r.kind,r.id,uid,previous,payload->>'status',payload->>'review_note',hide);
  select to_jsonb(z)-'reporter_id'into result from cbgpu_schools.moderation_reports z where id=r.id and kind=r.kind;return jsonb_build_object('ok',true,'report',result);
 end if;raise exception 'UNKNOWN_ACTION'using errcode='22023';
end$$;
revoke all on function public.cbgpu_message_moderation(text,jsonb)from public,anon,service_role;
grant execute on function public.cbgpu_message_moderation(text,jsonb)to authenticated;
commit;
