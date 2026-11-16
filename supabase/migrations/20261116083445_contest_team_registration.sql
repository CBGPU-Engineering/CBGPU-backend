begin;
alter table public.contests
 add column participation_mode text not null default 'individual' check(participation_mode in ('individual','team')),
 add column team_min_size integer not null default 2,
 add column team_max_size integer not null default 5,
 add column allow_individual_entries boolean not null default false,
 add constraint contest_team_range check(team_min_size between 1 and 50 and team_max_size between team_min_size and 50);
grant update(participation_mode,team_min_size,team_max_size,allow_individual_entries) on public.contests to authenticated;
alter table public.event_registrations add column participation_kind text not null default 'individual' check(participation_kind in ('individual','team'));
create index event_registration_team_queue_idx on public.event_registrations(contest_id,participation_kind,status,user_id);
create index contests_team_deadline_idx on public.contests((coalesce(registration_closes_at,event_start_at))) where participation_mode='team' and lifecycle_status='published';
create table public.contest_teams(
 id uuid primary key default gen_random_uuid(),contest_id uuid not null references public.contests(id) on delete cascade,
 name text not null check(char_length(btrim(name)) between 1 and 80),captain_id uuid references auth.users(id) on delete set null,
 kind text not null default 'formed' check(kind in ('formed','random')),join_code uuid not null default gen_random_uuid() unique,
 formation_status text not null default 'forming' check(formation_status in ('forming','ready','needs_members')),
 created_at timestamptz not null default clock_timestamp(),unique(id,contest_id)
);
create index contest_teams_contest_idx on public.contest_teams(contest_id);
create table public.contest_team_members(
 contest_id uuid not null references public.contests(id) on delete cascade,team_id uuid not null,user_id uuid not null references auth.users(id) on delete cascade,
 registration_id uuid not null references public.event_registrations(id) on delete cascade unique,
 attendee_name text not null check(char_length(attendee_name) between 1 and 100),registration_status text not null,
 joined_at timestamptz not null default clock_timestamp(),primary key(contest_id,user_id),
 foreign key(team_id,contest_id) references public.contest_teams(id,contest_id) on delete cascade
);
create index contest_team_members_team_idx on public.contest_team_members(team_id);
alter table public.contest_teams enable row level security;
alter table public.contest_team_members enable row level security;
revoke all on public.contest_teams,public.contest_team_members from public,anon,authenticated;
grant select on public.contest_teams,public.contest_team_members to authenticated;
create schema if not exists contest_private;
revoke all on schema contest_private from public,anon;
grant usage on schema contest_private to authenticated;
-- This scoped boolean avoids recursive membership RLS. It never returns another user's data.
create function contest_private.can_read_team(p_team uuid) returns boolean language sql stable security definer set search_path='' as $$
 select auth.uid() is not null and exists(select 1 from public.contest_teams t join public.contests c on c.id=t.contest_id where t.id=p_team and(
 c.host_id=auth.uid() or exists(select 1 from public.event_hosts h where h.contest_id=c.id and h.user_id=auth.uid())
 or exists(select 1 from public.contest_team_members m where m.team_id=t.id and m.user_id=auth.uid())))
$$;
revoke all on function contest_private.can_read_team(uuid) from public,anon;
grant execute on function contest_private.can_read_team(uuid) to authenticated;
create policy contest_team_read on public.contest_teams for select to authenticated using(contest_private.can_read_team(id));
create policy contest_team_member_read on public.contest_team_members for select to authenticated using(contest_private.can_read_team(team_id));
create function contest_private.validate_team_settings() returns trigger language plpgsql security invoker set search_path='' as $$
begin
 perform pg_advisory_xact_lock(hashtextextended(new.id::text,0));
 if new.participation_mode='team' and coalesce(new.registration_closes_at,new.event_start_at) is null then raise exception '팀 대회는 참가 신청 마감 또는 행사 시작 시간을 지정해 주세요.';end if;
 if tg_op='UPDATE' then
  if new.participation_mode<>old.participation_mode and exists(select 1 from public.event_registrations r where r.contest_id=new.id and r.status not in ('cancelled','declined')) then raise exception '참가 신청자가 있으면 개인전·팀전 방식을 변경할 수 없어요.';end if;
  if new.participation_mode='team' and not new.allow_individual_entries and exists(select 1 from public.event_registrations r where r.contest_id=new.id and r.participation_kind='individual' and r.status not in ('cancelled','declined')) then raise exception '개인 신청자가 있어 개인 신청을 끌 수 없어요.';end if;
  if exists(select 1 from public.contest_team_members m where m.contest_id=new.id group by m.team_id having count(*)>new.team_max_size) then raise exception '이미 신청한 팀보다 최대 인원을 작게 설정할 수 없어요.';end if;
 end if;return new;
end;$$;
revoke all on function contest_private.validate_team_settings() from public,anon,authenticated;
create trigger validate_team_settings before insert or update of participation_mode,team_min_size,team_max_size,allow_individual_entries,registration_closes_at,event_start_at on public.contests for each row execute function contest_private.validate_team_settings();
-- A deferred invariant makes registration+membership atomic even for direct API callers.
create function contest_private.validate_registration_team() returns trigger language plpgsql security definer set search_path='' as $$
declare r public.event_registrations;c public.contests;membership_kind text;
begin
 select * into r from public.event_registrations where id=new.id;
 if not found or r.status in ('cancelled','declined') then return null;end if;
 select * into c from public.contests where id=r.contest_id;
 select t.kind into membership_kind from public.contest_team_members m join public.contest_teams t on t.id=m.team_id where m.registration_id=r.id;
 if c.participation_mode='individual' and r.participation_kind<>'individual' then raise exception '개인전은 개인 참가로 신청해 주세요.';end if;
 if c.participation_mode='team' then
  if r.participation_kind='individual' and not c.allow_individual_entries then raise exception '주최자가 개인 신청을 허용하지 않았어요.';end if;
  if r.participation_kind='team' and membership_kind is distinct from 'formed' then raise exception '팀 생성 또는 팀 초대 코드로 함께 신청해 주세요.';end if;
 end if;return null;
end;$$;
revoke all on function contest_private.validate_registration_team() from public,anon,authenticated;
create constraint trigger validate_registration_team after insert or update on public.event_registrations deferrable initially deferred for each row execute function contest_private.validate_registration_team();
create function public.register_contest_entry(p_contest_id uuid,p_attendee_name text,p_answers jsonb default '{}',p_kind text default 'individual',p_team_name text default null,p_join_code uuid default null)
returns setof public.event_registrations language plpgsql security definer set search_path='' as $$
declare actor uuid:=auth.uid();c public.contests;r public.event_registrations;t public.contest_teams;stored_kind text;
begin
 if actor is null then raise exception '로그인이 필요해요.' using errcode='42501';end if;
 perform pg_advisory_xact_lock(hashtextextended(p_contest_id::text,0));select * into c from public.contests where id=p_contest_id;
 if not found or not c.registration_open or c.lifecycle_status<>'published' or c.host_id=actor or coalesce(c.registration_closes_at,c.event_start_at)<=clock_timestamp() then raise exception '참가 신청이 마감되었어요.';end if;
 if p_kind not in ('individual','team_create','team_join') then raise exception '참가 방식을 확인해 주세요.';end if;
 if c.participation_mode='individual' and p_kind<>'individual' then raise exception '개인전은 개인 참가로 신청해 주세요.';end if;
 if c.participation_mode='team' and p_kind='individual' and not c.allow_individual_entries then raise exception '개인 신청이 허용되지 않은 대회예요.';end if;
 if p_kind='team_join' then select * into t from public.contest_teams where contest_id=c.id and join_code=p_join_code and kind='formed';if not found then raise exception '이 대회의 팀 초대 코드를 확인해 주세요.';end if;if (select count(*) from public.contest_team_members where team_id=t.id)>=c.team_max_size then raise exception '팀의 최대 인원이 찼어요.';end if;end if;
 set constraints public.validate_registration_team deferred;
 stored_kind:=case when p_kind='individual' then 'individual' else 'team' end;
 insert into public.event_registrations(contest_id,user_id,attendee_name,answers,participation_kind)
 values(c.id,actor,btrim(p_attendee_name),p_answers,stored_kind)
 on conflict(contest_id,user_id) do update set attendee_name=excluded.attendee_name,answers=excluded.answers,status='pending',participation_kind=excluded.participation_kind
 where public.event_registrations.status in ('cancelled','declined') returning * into r;
 if not found then raise exception '이미 참가 신청했어요. 기존 신청을 확인해 주세요.';end if;
 if p_kind='team_create' then insert into public.contest_teams(contest_id,name,captain_id)values(c.id,btrim(p_team_name),actor)returning * into t;end if;
 if p_kind<>'individual' then insert into public.contest_team_members(contest_id,team_id,user_id,registration_id,attendee_name,registration_status)values(c.id,t.id,actor,r.id,r.attendee_name,r.status);end if;
 set constraints public.validate_registration_team immediate;
 set constraints public.validate_registration_team deferred;
 return next r;
end;$$;
revoke all on function public.register_contest_entry(uuid,text,jsonb,text,text,uuid) from public,anon;
grant execute on function public.register_contest_entry(uuid,text,jsonb,text,text,uuid) to authenticated;
create function contest_private.sync_team_membership() returns trigger language plpgsql security definer set search_path='' as $$
begin
 if tg_op='DELETE' then delete from public.contest_team_members where registration_id=old.id;
 elsif new.status in ('cancelled','declined') then delete from public.contest_team_members where registration_id=new.id;
 else update public.contest_team_members set attendee_name=new.attendee_name,registration_status=new.status where registration_id=new.id;end if;return null;
end;$$;
revoke all on function contest_private.sync_team_membership() from public,anon,authenticated;
create trigger sync_team_membership after update of attendee_name,status or delete on public.event_registrations for each row execute function contest_private.sync_team_membership();
create function contest_private.repair_team_captain() returns trigger language plpgsql security definer set search_path='' as $$
begin
 update public.contest_teams set captain_id=(select user_id from public.contest_team_members where team_id=old.team_id order by joined_at,user_id limit 1)where id=old.team_id and captain_id=old.user_id;
 delete from public.contest_teams t where t.id=old.team_id and not exists(select 1 from public.contest_team_members m where m.team_id=t.id);return null;
end;$$;
revoke all on function contest_private.repair_team_captain() from public,anon,authenticated;
create trigger repair_team_captain after delete on public.contest_team_members for each row execute function contest_private.repair_team_captain();
-- A private Rust scheduler owns random allocation; the login is provisioned separately.
do $$begin if not exists(select 1 from pg_roles where rolname='cbgpu_contest_teams')then create role cbgpu_contest_teams nologin noinherit;end if;end$$;
grant usage on schema public to cbgpu_contest_teams;
grant select(id,participation_mode,team_min_size,team_max_size,allow_individual_entries,registration_closes_at,event_start_at,lifecycle_status) on public.contests to cbgpu_contest_teams;
grant select(id,contest_id,user_id,attendee_name,status,participation_kind) on public.event_registrations to cbgpu_contest_teams;
grant select,insert on public.contest_teams,public.contest_team_members to cbgpu_contest_teams;
grant update(formation_status) on public.contest_teams to cbgpu_contest_teams;
create policy team_allocator_contests on public.contests for select to cbgpu_contest_teams using(participation_mode='team' and lifecycle_status='published' and coalesce(registration_closes_at,event_start_at)<=clock_timestamp());
create policy team_allocator_registrations on public.event_registrations for select to cbgpu_contest_teams using(exists(select 1 from public.contests c where c.id=contest_id));
create policy team_allocator_teams_read on public.contest_teams for select to cbgpu_contest_teams using(exists(select 1 from public.contests c where c.id=contest_id));
create policy team_allocator_teams_insert on public.contest_teams for insert to cbgpu_contest_teams with check(kind='random' and captain_id is null and exists(select 1 from public.contests c where c.id=contest_id and c.allow_individual_entries));
create policy team_allocator_teams_update on public.contest_teams for update to cbgpu_contest_teams using(exists(select 1 from public.contests c where c.id=contest_id)) with check(exists(select 1 from public.contests c where c.id=contest_id));
create policy team_allocator_members_read on public.contest_team_members for select to cbgpu_contest_teams using(exists(select 1 from public.contests c where c.id=contest_id));
create policy team_allocator_members_insert on public.contest_team_members for insert to cbgpu_contest_teams with check(
 exists(select 1 from public.contest_teams t join public.event_registrations r on r.id=registration_id where t.id=team_id and t.kind='random' and t.contest_id=contest_team_members.contest_id and r.contest_id=t.contest_id and r.user_id=contest_team_members.user_id and r.status='confirmed' and r.participation_kind='individual' and r.attendee_name=contest_team_members.attendee_name and r.status=contest_team_members.registration_status));

-- One shared submission per team, retaining all historical personal rows/keys.
alter table public.contest_submissions add column team_id uuid references public.contest_teams(id) on delete set null;
create unique index contest_submissions_team_unique on public.contest_submissions(contest_id,team_id) where team_id is not null;
grant update(team_id) on public.contest_submissions to authenticated;
create function contest_private.is_confirmed_member(p_team uuid,p_user uuid) returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from public.contest_team_members m join public.event_registrations r on r.id=m.registration_id where m.team_id=p_team and m.user_id=p_user and r.user_id=p_user and r.contest_id=m.contest_id and r.status='confirmed')
$$;
revoke all on function contest_private.is_confirmed_member(uuid,uuid) from public,anon;
grant execute on function contest_private.is_confirmed_member(uuid,uuid) to authenticated;
create function contest_private.can_submit_team(p_team uuid) returns boolean language sql stable security definer set search_path='' as $$
 select auth.uid() is not null and contest_private.is_confirmed_member(p_team,auth.uid()) and exists(
 select 1 from public.contest_teams t join public.contests c on c.id=t.contest_id where t.id=p_team and c.participation_mode='team' and
 (select count(*) from public.contest_team_members m join public.event_registrations r on r.id=m.registration_id where m.team_id=t.id and r.status='confirmed') between c.team_min_size and c.team_max_size)
$$;
revoke all on function contest_private.can_submit_team(uuid) from public,anon;
grant execute on function contest_private.can_submit_team(uuid) to authenticated;
create policy contest_team_submission_read on public.contest_submissions for select to authenticated using(team_id is not null and contest_private.is_confirmed_member(team_id,auth.uid()));
create policy contest_team_submission_update on public.contest_submissions for update to authenticated using(team_id is not null and contest_private.can_submit_team(team_id)) with check(team_id is not null and contest_private.can_submit_team(team_id) and exists(select 1 from public.contests c where c.id=contest_id and c.host_id<>auth.uid() and c.is_open and c.lifecycle_status='published' and(c.ends_at is null or c.ends_at>clock_timestamp())));
create policy contest_team_file_read on storage.objects for select to authenticated using(bucket_id='contest-submissions' and exists(select 1 from public.contest_team_members mine join public.contest_team_members other on other.team_id=mine.team_id where mine.user_id=auth.uid() and other.user_id::text=split_part(name,'/',2) and mine.contest_id::text=split_part(name,'/',1) and contest_private.is_confirmed_member(mine.team_id,auth.uid()) and contest_private.is_confirmed_member(mine.team_id,other.user_id)));
create function contest_private.can_cleanup_file(object_name text) returns boolean language sql stable security definer set search_path='' as $$
 select split_part(object_name,'/',2)=auth.uid()::text and not exists(select 1 from public.contest_submissions s,jsonb_each(s.answers)a where a.value->>'path'=object_name)
$$;
revoke all on function contest_private.can_cleanup_file(text) from public,anon;
grant execute on function contest_private.can_cleanup_file(text) to authenticated;
drop policy contest_files_cleanup on storage.objects;
create policy contest_files_cleanup on storage.objects for delete to authenticated using(bucket_id='contest-submissions' and contest_private.can_cleanup_file(name));

create or replace function public.validate_contest_submission() returns trigger
language plpgsql security invoker set search_path = '' as $$
declare c public.contests; f jsonb; v jsonb; k text; actual_size bigint; actor uuid:=(select auth.uid()); uploader uuid;
begin
  if tg_op = 'UPDATE' and (new.contest_id <> old.contest_id or new.user_id <> old.user_id) then
    raise exception 'Submission ownership is immutable';
  end if;
  -- FK team deletion retains submission content and original submitter.
  if tg_op='UPDATE' and pg_trigger_depth()>1 and old.team_id is not null and new.team_id is null
    and (new.contest_id,new.user_id,new.answers,new.participant_name,new.student_id) is not distinct from (old.contest_id,old.user_id,old.answers,old.participant_name,old.student_id) then return new;end if;
  if tg_op='UPDATE' and new.team_id is distinct from old.team_id then raise exception 'Submission team is immutable';end if;
  if actor is null then raise exception '로그인이 필요해요.'; end if;
  select * into c from public.contests where id = new.contest_id;
  if not found or c.host_id = new.user_id then raise exception '참가자만 제출할 수 있어요.'; end if;
  if c.lifecycle_status<>'published' then raise exception 'CONTEST_CLOSED';end if;
  if c.participation_mode='team' then
   if new.team_id is null or not contest_private.can_submit_team(new.team_id) or not exists(select 1 from public.contest_teams t where t.id=new.team_id and t.contest_id=c.id) then raise exception '확정된 팀과 최소·최대 인원을 확인해 주세요.';end if;
   if tg_op='INSERT' and new.user_id<>actor then raise exception 'Submission submitter must be current user';end if;
  elsif new.team_id is not null or new.user_id<>actor then raise exception '개인 제출 명의를 확인해 주세요.';end if;
  if not c.is_open or (c.ends_at is not null and c.ends_at <= clock_timestamp()) then raise exception 'CONTEST_CLOSED'; end if;
  if jsonb_typeof(new.answers) <> 'object' then raise exception '제출 내용을 확인해 주세요.'; end if;
  for k in select jsonb_object_keys(new.answers) loop
    if not exists (select 1 from jsonb_array_elements(c.fields) field where field->>'id' = k) then
      raise exception '정의되지 않은 제출 항목이에요.';
    end if;
  end loop;
  for f in select value from jsonb_array_elements(c.fields) loop
    v := new.answers->(f->>'id');
    if v is null or v = 'null'::jsonb or v = '""'::jsonb then
      if (f->>'required')::boolean then raise exception '필수 제출 항목이 누락되었어요: %', f->>'label'; end if;
      continue;
    end if;
    if f->>'type' in ('text', 'link') then
      if jsonb_typeof(v) <> 'string' or char_length(btrim(v #>> '{}')) = 0 then raise exception '텍스트 또는 링크를 입력해 주세요: %', f->>'label'; end if;
      if f->>'type' = 'text' and char_length(v #>> '{}') > 10000 then raise exception '텍스트는 10,000자 이내로 입력해 주세요.'; end if;
      if f->>'type' = 'link' and (char_length(v #>> '{}') > 2000 or (v #>> '{}') !~* '^https?://[^[:space:]/?#]+[^[:space:]]*$') then
        raise exception '올바른 http 또는 https 링크를 입력해 주세요.';
      end if;
    else
      if jsonb_typeof(v) <> 'object' or coalesce(jsonb_typeof(v->'name'), '') <> 'string'
        or char_length(coalesce(v->>'name', '')) not between 1 and 255
        or split_part(coalesce(v->>'path',''),'/',1)<>c.id::text
        or split_part(coalesce(v->>'path',''),'/',3)<>(f->>'id')
        or split_part(coalesce(v->>'path',''),'/',4)=''
        or coalesce(jsonb_typeof(v->'size'), '') <> 'number'
        or coalesce(v->>'size', '') !~ '^[0-9]+$' then raise exception '파일 정보를 확인해 주세요: %', f->>'label'; end if;
      if new.team_id is null then
       if split_part(v->>'path','/',2)<>new.user_id::text then raise exception '파일 소유자를 확인해 주세요.';end if;
      elsif not contest_private.is_confirmed_member(new.team_id,split_part(v->>'path','/',2)::uuid) then raise exception '같은 팀의 확정 참가자 파일만 제출할 수 있어요.';end if;
      if (v->>'size')::numeric not between 1 and (f->>'max_size_mb')::numeric * 1048576 then raise exception '파일 용량 제한을 초과했어요: %', f->>'label'; end if;
      if f->>'extensions' <> '' and not exists (
        select 1 from unnest(string_to_array(f->>'extensions', ',')) ext
        where right(lower(v->>'name'), char_length(ext)) = ext
          and right(lower(v->>'path'), char_length(ext)) = ext
      ) then raise exception '허용되지 않은 파일 확장자예요: %', f->>'label'; end if;
      select (o.metadata->>'size')::bigint into actual_size from storage.objects o
        where o.bucket_id = 'contest-submissions' and o.name = v->>'path';
      if not found or actual_size is null or actual_size <> (v->>'size')::numeric then raise exception '업로드한 파일을 확인할 수 없어요: %', f->>'label'; end if;
    end if;
  end loop;
  new.submitted_at := clock_timestamp();
  if c.ends_at is not null and new.submitted_at >= c.ends_at then raise exception 'CONTEST_CLOSED'; end if;
  return new;
end;
$$;

notify pgrst,'reload schema';
commit;
