begin;
-- Collaboration metadata never grants event_hosts, participant eligibility, or school roles.
create table if not exists cbgpu_schools.contest_organizer_schools(
 id uuid primary key default gen_random_uuid(),contest_id uuid not null references public.contests(id) on delete cascade,
 school_id text not null references cbgpu_schools.schools(id),status text not null check(status in('INVITED','ACCEPTED','DECLINED','REVOKED')),
 revision integer not null default 1 check(revision>0),invited_by uuid not null references auth.users(id),reviewed_by uuid references auth.users(id),
 created_at timestamptz not null default clock_timestamp(),updated_at timestamptz not null default clock_timestamp(),unique(contest_id,school_id));
create table if not exists cbgpu_schools.contest_organizer_audit(
 id bigint generated always as identity primary key,invitation_id uuid not null,
 actor_id uuid not null references auth.users(id),action text not null,previous_status text,status text not null,revision integer not null,created_at timestamptz not null default clock_timestamp());
alter table cbgpu_schools.contest_organizer_schools enable row level security;
alter table cbgpu_schools.contest_organizer_audit enable row level security;
revoke all on cbgpu_schools.contest_organizer_schools,cbgpu_schools.contest_organizer_audit from public,anon,authenticated,service_role;
create index if not exists contest_organizer_inbox on cbgpu_schools.contest_organizer_schools(school_id,updated_at desc);
create or replace function cbgpu_schools.organizer_invitation(i cbgpu_schools.contest_organizer_schools) returns jsonb language sql stable security definer set search_path='' as $$
 select jsonb_build_object('id',i.id,'contest_id',i.contest_id,'contest_title',c.title,'school_id',i.school_id,'school_name',s.display_name,'status',i.status,'revision',i.revision,'created_at',i.created_at,'updated_at',i.updated_at)
 from public.contests c join cbgpu_schools.schools s on s.id=i.school_id where c.id=i.contest_id$$;
create or replace function public.cbgpu_contest_organizer_schools(p_action text,p_payload jsonb default '{}') returns jsonb language plpgsql security definer set search_path='' as $$
declare actor uuid:=auth.uid();c public.contests;i cbgpu_schools.contest_organizer_schools;old_status text;desired text;sid text;begin
 if actor is null or not public.cbgpu_personal_project_identity() then raise exception 'FRESH_IDENTITY_REQUIRED' using errcode='42501';end if;
 if jsonb_typeof(p_payload)<>'object' or pg_column_size(p_payload)>8192 then raise exception 'INVALID_ORGANIZER_REQUEST' using errcode='23514';end if;
 if p_action='inbox' then
  sid:=p_payload->>'school_id';if not cbgpu_schools.has_role(actor,sid,array['SCHOOL_OWNER','SCHOOL_ADMIN']) then raise exception 'SCHOOL_ADMIN_REQUIRED' using errcode='42501';end if;
  return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.organizer_invitation(x) order by x.updated_at desc),'[]') from(select * from cbgpu_schools.contest_organizer_schools where school_id=sid order by updated_at desc limit 100)x));
 end if;
 if p_action in('state','invite') then
  select * into c from public.contests where id=(p_payload->>'contest_id')::uuid for update;
  if not found or c.host_id<>actor then raise exception 'CONTEST_OWNER_REQUIRED' using errcode='42501';end if;
  if p_action='state' then return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.organizer_invitation(x) order by x.created_at),'[]') from cbgpu_schools.contest_organizer_schools x where contest_id=c.id));end if;
  sid:=p_payload->>'school_id';if sid=c.school_id or not cbgpu_schools.feature_enabled(sid,'contests') or not cbgpu_schools.feature_enabled(c.school_id,'contests') then raise exception 'APPROVED_CONTEST_SCHOOL_REQUIRED' using errcode='23514';end if;
  if (select count(*) from cbgpu_schools.contest_organizer_schools where contest_id=c.id)>=50 then raise exception 'ORGANIZER_SCHOOL_LIMIT' using errcode='23514';end if;
  select * into i from cbgpu_schools.contest_organizer_schools where contest_id=c.id and school_id=sid for update;
  if found then
   if i.revision is distinct from (p_payload->>'revision')::integer then raise exception 'ORGANIZER_REVISION_CONFLICT' using errcode='40001';end if;
   if i.status in('INVITED','ACCEPTED') then raise exception 'ORGANIZER_ALREADY_INVITED' using errcode='23514';end if;
   old_status:=i.status;update cbgpu_schools.contest_organizer_schools set status='INVITED',revision=revision+1,invited_by=actor,reviewed_by=null,updated_at=clock_timestamp() where id=i.id returning * into i;
  else
   if coalesce((p_payload->>'revision')::integer,0)<>0 then raise exception 'ORGANIZER_REVISION_CONFLICT' using errcode='40001';end if;
   insert into cbgpu_schools.contest_organizer_schools(contest_id,school_id,status,invited_by)values(c.id,sid,'INVITED',actor)returning * into i;
  end if;
 elsif p_action in('accept','decline','revoke','audit') then
  select * into i from cbgpu_schools.contest_organizer_schools where id=(p_payload->>'id')::uuid for update;
  if not found then raise exception 'INVITATION_NOT_FOUND' using errcode='P0002';end if;
  select * into c from public.contests where id=i.contest_id;
  if p_action='audit' then
   if c.host_id<>actor and not cbgpu_schools.has_role(actor,i.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN']) then raise exception 'ORGANIZER_REVIEW_REQUIRED' using errcode='42501';end if;
   return jsonb_build_object('items',(select coalesce(jsonb_agg(jsonb_build_object('action',a.action,'previous_status',a.previous_status,'status',a.status,'revision',a.revision,'created_at',a.created_at) order by a.id),'[]') from cbgpu_schools.contest_organizer_audit a where a.invitation_id=i.id));
  end if;
  if p_action='revoke' then
   if c.host_id<>actor and not cbgpu_schools.has_role(actor,i.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN']) then raise exception 'ORGANIZER_REVIEW_REQUIRED' using errcode='42501';end if;
  elsif not cbgpu_schools.has_role(actor,i.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN']) then raise exception 'TARGET_SCHOOL_ADMIN_REQUIRED' using errcode='42501';end if;
  if i.revision is distinct from (p_payload->>'revision')::integer then raise exception 'ORGANIZER_REVISION_CONFLICT' using errcode='40001';end if;
  if p_action in('accept','decline') and i.status<>'INVITED' or p_action='revoke' and i.status not in('INVITED','ACCEPTED') then raise exception 'INVALID_ORGANIZER_TRANSITION' using errcode='23514';end if;
  if p_action='accept' and(not cbgpu_schools.feature_enabled(i.school_id,'contests') or not cbgpu_schools.feature_enabled(c.school_id,'contests')) then raise exception 'APPROVED_CONTEST_SCHOOL_REQUIRED' using errcode='23514';end if;
  old_status:=i.status;desired:=case p_action when 'accept' then 'ACCEPTED' when 'decline' then 'DECLINED' else 'REVOKED' end;
  update cbgpu_schools.contest_organizer_schools set status=desired,revision=revision+1,reviewed_by=actor,updated_at=clock_timestamp() where id=i.id returning * into i;
 else raise exception 'UNKNOWN_ORGANIZER_ACTION' using errcode='23514';end if;
 insert into cbgpu_schools.contest_organizer_audit(invitation_id,actor_id,action,previous_status,status,revision)values(i.id,actor,p_action,old_status,i.status,i.revision);
 return jsonb_build_object('invitation',cbgpu_schools.organizer_invitation(i),'metadata_only',true);
end$$;
revoke all on function cbgpu_schools.organizer_invitation(cbgpu_schools.contest_organizer_schools),public.cbgpu_contest_organizer_schools(text,jsonb) from public,anon,authenticated,service_role;
grant execute on function public.cbgpu_contest_organizer_schools(text,jsonb) to authenticated;
create or replace function cbgpu_schools.accepted_organizer_school_names(contest uuid)returns text language sql stable security definer set search_path='' as $$
 select string_agg(s.display_name,', ' order by s.display_name) from cbgpu_schools.contest_organizer_schools i join cbgpu_schools.schools s on s.id=i.school_id
 where i.contest_id=contest and i.status='ACCEPTED' and cbgpu_schools.feature_enabled(s.id,'contests')$$;
revoke all on function cbgpu_schools.accepted_organizer_school_names(uuid)from public,anon,authenticated,service_role;
-- Append one bounded plain-text field to the existing curated public projection.
do $$begin if to_regprocedure('cbgpu_schools.public_contest_before_organizers(public.contests)')is null then alter function cbgpu_schools.public_contest(public.contests)rename to public_contest_before_organizers;end if;end$$;
create or replace function cbgpu_schools.public_contest(c public.contests)returns jsonb language sql stable security definer set search_path='' as $$select cbgpu_schools.public_contest_before_organizers(c)||jsonb_build_object('organizer_school_names',cbgpu_schools.accepted_organizer_school_names(c.id))$$;
revoke all on function cbgpu_schools.public_contest(public.contests),cbgpu_schools.public_contest_before_organizers(public.contests)from public,anon,authenticated,service_role;
commit;
