begin;
create table if not exists cbgpu_schools.contest_operator_nominations(
 id uuid primary key default gen_random_uuid(),invitation_id uuid not null references cbgpu_schools.contest_organizer_schools(id) on delete cascade,
 contest_id uuid not null references public.contests(id) on delete cascade,school_id text not null references cbgpu_schools.schools(id),user_id uuid not null references auth.users(id),
 consent_session_id uuid not null,grant_created_at timestamptz,grant_row_version text,status text not null check(status in('PENDING','APPROVED','REJECTED','WITHDRAWN','REVOKED')),revision integer not null default 1,
 created_at timestamptz not null default clock_timestamp(),updated_at timestamptz not null default clock_timestamp(),unique(contest_id,user_id));
create table if not exists cbgpu_schools.contest_operator_nomination_audit(id bigint generated always as identity primary key,nomination_id uuid not null,actor_id uuid not null references auth.users(id),action text not null,status text not null,revision integer not null,created_at timestamptz not null default clock_timestamp());
alter table cbgpu_schools.contest_operator_nominations enable row level security;alter table cbgpu_schools.contest_operator_nomination_audit enable row level security;
revoke all on cbgpu_schools.contest_operator_nominations,cbgpu_schools.contest_operator_nomination_audit from public,anon,authenticated,service_role;
create or replace function cbgpu_schools.operator_nomination(n cbgpu_schools.contest_operator_nominations)returns jsonb language sql stable security definer set search_path='' as $$select jsonb_build_object('id',n.id,'invitation_id',n.invitation_id,'contest_id',n.contest_id,'school_id',n.school_id,'school_name',s.display_name,'display_name',p.full_name,'status',n.status,'revision',n.revision,'owned_grant_active',exists(select 1 from public.event_hosts owned where owned.contest_id=n.contest_id and owned.user_id=n.user_id and owned.role='manager'and owned.created_at=n.grant_created_at and owned.xmin::text=n.grant_row_version),'event_role',(select h.role from public.event_hosts h where h.contest_id=n.contest_id and h.user_id=n.user_id),'updated_at',n.updated_at)from cbgpu_schools.schools s join public.profiles p on p.id=n.user_id where s.id=n.school_id$$;
create or replace function public.cbgpu_contest_school_operators(p_action text,p_payload jsonb default '{}')returns jsonb language plpgsql security definer set search_path='' as $$
declare actor uuid:=auth.uid();n cbgpu_schools.contest_operator_nominations;i cbgpu_schools.contest_organizer_schools;c public.contests;sid uuid;desired text;granted_at timestamptz;row_version text;removed boolean:=false;begin
 if actor is null or not public.cbgpu_personal_project_identity()then raise exception 'FRESH_IDENTITY_REQUIRED'using errcode='42501';end if;
 if jsonb_typeof(p_payload)<>'object'or pg_column_size(p_payload)>8192 then raise exception 'INVALID_OPERATOR_REQUEST'using errcode='23514';end if;
 if p_action='owner_list'then
  select * into c from public.contests where id=(p_payload->>'contest_id')::uuid;if not found or c.host_id<>actor then raise exception 'CONTEST_OWNER_REQUIRED'using errcode='42501';end if;
  return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.operator_nomination(x)order by x.updated_at desc),'[]')from cbgpu_schools.contest_operator_nominations x where x.contest_id=c.id));
 elsif p_action in('self_list','self_for_contest')then
  return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.operator_nomination(x)order by x.updated_at desc),'[]')from cbgpu_schools.contest_operator_nominations x where x.user_id=actor and(case when p_action='self_list'then x.school_id=p_payload->>'school_id'else x.contest_id=(p_payload->>'contest_id')::uuid end)));
 end if;
 if p_payload->'confirmed'is distinct from 'true'::jsonb then raise exception 'EXPLICIT_OPERATOR_CONFIRMATION_REQUIRED'using errcode='23514';end if;
 if p_action='nominate'then
  select * into i from cbgpu_schools.contest_organizer_schools where id=(p_payload->>'invitation_id')::uuid for update;
  if not found or i.status<>'ACCEPTED'or not cbgpu_schools.has_role(actor,i.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN'])or not cbgpu_schools.feature_enabled(i.school_id,'contests')then raise exception 'ACCEPTED_SCHOOL_ADMIN_REQUIRED'using errcode='42501';end if;
  select * into c from public.contests where id=i.contest_id;if c.host_id=actor then raise exception 'OWNER_ALREADY_OPERATES_CONTEST'using errcode='23514';end if;
  sid:=(auth.jwt()->>'session_id')::uuid;
  select * into n from cbgpu_schools.contest_operator_nominations where contest_id=i.contest_id and user_id=actor for update;
  if found then
   if n.status='APPROVED'or n.revision is distinct from(p_payload->>'revision')::integer then raise exception 'OPERATOR_REVISION_CONFLICT'using errcode='40001';end if;
   update cbgpu_schools.contest_operator_nominations set invitation_id=i.id,school_id=i.school_id,status='PENDING',consent_session_id=sid,revision=revision+1,updated_at=clock_timestamp()where id=n.id returning * into n;
  else
   if coalesce((p_payload->>'revision')::integer,0)<>0 then raise exception 'OPERATOR_REVISION_CONFLICT'using errcode='40001';end if;
   insert into cbgpu_schools.contest_operator_nominations(invitation_id,contest_id,school_id,user_id,consent_session_id,status)values(i.id,i.contest_id,i.school_id,actor,sid,'PENDING')returning * into n;
  end if;
 elsif p_action in('approve','reject','withdraw','revoke')then
  select * into n from cbgpu_schools.contest_operator_nominations where id=(p_payload->>'id')::uuid for update;if not found then raise exception 'NOMINATION_NOT_FOUND'using errcode='P0002';end if;
  select * into c from public.contests where id=n.contest_id;
  if p_action in('approve','reject')and(c.host_id<>actor or n.user_id=actor)or p_action in('withdraw','revoke')and actor not in(c.host_id,n.user_id)then raise exception 'OPERATOR_REVIEW_REQUIRED'using errcode='42501';end if;
  if n.revision is distinct from(p_payload->>'revision')::integer then raise exception 'OPERATOR_REVISION_CONFLICT'using errcode='40001';end if;
  if p_action in('approve','reject','withdraw')and n.status<>'PENDING'or p_action='revoke'and n.status<>'APPROVED'then raise exception 'INVALID_OPERATOR_TRANSITION'using errcode='23514';end if;
  if p_action='approve'then
   if not exists(select 1 from cbgpu_schools.contest_organizer_schools link where link.id=n.invitation_id and link.status='ACCEPTED')or not cbgpu_schools.has_role(n.user_id,n.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN'])or not cbgpu_schools.feature_enabled(n.school_id,'contests')or not cbgpu_schools.feature_enabled(c.school_id,'contests')or not exists(select 1 from auth.sessions a where a.id=n.consent_session_id and a.user_id=n.user_id and(a.not_after is null or a.not_after>clock_timestamp()))then raise exception 'NOMINEE_AUTHORITY_EXPIRED'using errcode='42501';end if;
   insert into public.event_hosts(contest_id,user_id,role)values(n.contest_id,n.user_id,'manager')on conflict(contest_id,user_id)do nothing returning created_at,xmin::text into granted_at,row_version;
   if not found then raise exception 'EXISTING_OPERATOR_REVIEW_SEPARATELY'using errcode='23505';end if;
  elsif p_action='revoke'then delete from public.event_hosts where contest_id=n.contest_id and user_id=n.user_id and role='manager'and created_at=n.grant_created_at and xmin::text=n.grant_row_version;removed:=found;
  end if;
  desired:=case p_action when 'approve'then'APPROVED'when'reject'then'REJECTED'when'withdraw'then'WITHDRAWN'else'REVOKED'end;
  update cbgpu_schools.contest_operator_nominations set status=desired,grant_created_at=case when p_action='approve'then granted_at else grant_created_at end,grant_row_version=case when p_action='approve'then row_version else grant_row_version end,revision=revision+1,updated_at=clock_timestamp()where id=n.id returning * into n;
 else raise exception 'UNKNOWN_OPERATOR_ACTION'using errcode='23514';end if;
 insert into cbgpu_schools.contest_operator_nomination_audit(nomination_id,actor_id,action,status,revision)values(n.id,actor,p_action,n.status,n.revision);
 return jsonb_build_object('nomination',cbgpu_schools.operator_nomination(n),'event_role_granted',p_action='approve','operator_grant_removed',removed);
end$$;
revoke all on function cbgpu_schools.operator_nomination(cbgpu_schools.contest_operator_nominations),public.cbgpu_contest_school_operators(text,jsonb)from public,anon,authenticated,service_role;
grant execute on function public.cbgpu_contest_school_operators(text,jsonb)to authenticated;
commit;
