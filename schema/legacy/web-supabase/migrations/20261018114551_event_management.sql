begin;

alter table public.contests
  add column event_start_at timestamptz,
  add column event_end_at timestamptz,
  add column location_type text not null default 'offline' check (location_type in ('offline','online','hybrid')),
  add column location_name text not null default '' check (char_length(location_name) <= 200),
  add column address text not null default '' check (char_length(address) <= 500),
  add column visibility text not null default 'public' check (visibility in ('public','unlisted')),
  add column capacity integer check (capacity is null or capacity between 1 and 10000),
  add column waitlist_enabled boolean not null default true,
  add column approval_required boolean not null default false,
  add column registration_open boolean not null default true,
  add column registration_questions jsonb not null default '[]'::jsonb,
  add column updated_at timestamptz not null default clock_timestamp(),
  add constraint contest_event_times check (event_end_at is null or event_start_at is null or event_end_at > event_start_at),
  add constraint contest_registration_questions_array check (jsonb_typeof(registration_questions) = 'array' and jsonb_array_length(registration_questions) <= 20);

create index contests_event_start_idx on public.contests(event_start_at) where event_start_at is not null;
create index contests_visibility_idx on public.contests(visibility, event_start_at);
grant update(event_start_at,event_end_at,location_type,location_name,address,visibility,capacity,waitlist_enabled,approval_required,registration_open,registration_questions) on public.contests to authenticated;

-- Join links are deliberately kept outside the public event row. Only approved
-- guests and event managers may read these details.
create table public.event_access_details (
  contest_id uuid primary key references public.contests(id) on delete cascade,
  meeting_url text check (meeting_url is null or (char_length(meeting_url) <= 2000 and meeting_url ~* '^https?://')),
  access_note text not null default '' check (char_length(access_note) <= 2000),
  updated_at timestamptz not null default clock_timestamp()
);

create table public.event_hosts (
  contest_id uuid not null references public.contests(id) on delete cascade,
  user_id uuid not null references auth.users(id) on delete cascade,
  role text not null default 'manager' check (role in ('manager','checkin')),
  show_public boolean not null default true,
  created_at timestamptz not null default clock_timestamp(),
  primary key(contest_id,user_id)
);
create index event_hosts_user_id_idx on public.event_hosts(user_id);

create table public.event_registrations (
  id uuid primary key default gen_random_uuid(),
  contest_id uuid not null references public.contests(id) on delete cascade,
  user_id uuid not null references auth.users(id) on delete cascade,
  attendee_name text not null check (char_length(btrim(attendee_name)) between 1 and 100),
  answers jsonb not null default '{}'::jsonb check (jsonb_typeof(answers) = 'object'),
  status text not null default 'confirmed' check (status in ('confirmed','pending','waitlisted','declined','cancelled')),
  ticket_code uuid not null default gen_random_uuid() unique,
  checked_in_at timestamptz,
  created_at timestamptz not null default clock_timestamp(),
  updated_at timestamptz not null default clock_timestamp(),
  unique(contest_id,user_id)
);
create index event_registrations_contest_status_idx on public.event_registrations(contest_id,status,created_at);
create index event_registrations_user_id_idx on public.event_registrations(user_id,created_at desc);

create table public.event_sessions (
  id uuid primary key default gen_random_uuid(),
  contest_id uuid not null references public.contests(id) on delete cascade,
  title text not null check (char_length(btrim(title)) between 1 and 150),
  description text not null default '' check (char_length(description) <= 2000),
  starts_at timestamptz not null,
  ends_at timestamptz not null,
  location text not null default '' check (char_length(location) <= 300),
  order_num integer not null default 0,
  check (ends_at > starts_at)
);
create index event_sessions_contest_order_idx on public.event_sessions(contest_id,order_num,starts_at);

create table public.event_announcements (
  id uuid primary key default gen_random_uuid(),
  contest_id uuid not null references public.contests(id) on delete cascade,
  author_id uuid not null references auth.users(id) on delete cascade,
  title text not null check (char_length(btrim(title)) between 1 and 150),
  content text not null check (char_length(btrim(content)) between 1 and 10000),
  audience text not null default 'all' check (audience in ('all','confirmed','pending','waitlisted')),
  published_at timestamptz not null default clock_timestamp()
);
create index event_announcements_contest_time_idx on public.event_announcements(contest_id,published_at desc);
create index event_announcements_author_id_idx on public.event_announcements(author_id);

alter table public.event_hosts enable row level security;
alter table public.event_access_details enable row level security;
alter table public.event_registrations enable row level security;
alter table public.event_sessions enable row level security;
alter table public.event_announcements enable row level security;

revoke all on public.event_hosts,public.event_access_details,public.event_registrations,public.event_sessions,public.event_announcements from anon,authenticated;
grant select,insert,update,delete on public.event_hosts to authenticated;
grant select,insert,update,delete on public.event_access_details to authenticated;
grant select,insert,update on public.event_registrations to authenticated;
grant select,insert,update,delete on public.event_sessions,public.event_announcements to authenticated;

create policy event_hosts_read on public.event_hosts for select to authenticated using (
  user_id = (select auth.uid()) or show_public or exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
);
create policy event_hosts_owner_insert on public.event_hosts for insert to authenticated with check (
  exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
);
create policy event_hosts_owner_update on public.event_hosts for update to authenticated
  using (exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid())))
  with check (exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid())));
create policy event_hosts_owner_delete on public.event_hosts for delete to authenticated using (
  exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
);

create policy event_access_details_read on public.event_access_details for select to authenticated using (
  exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
  or exists(select 1 from public.event_hosts h where h.contest_id=event_access_details.contest_id and h.user_id=(select auth.uid()))
  or exists(select 1 from public.event_registrations r where r.contest_id=event_access_details.contest_id and r.user_id=(select auth.uid()) and r.status='confirmed')
);
create policy event_access_details_manage on public.event_access_details for all to authenticated
  using (exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
    or exists(select 1 from public.event_hosts h where h.contest_id=event_access_details.contest_id and h.user_id=(select auth.uid()) and h.role='manager'))
  with check (exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
    or exists(select 1 from public.event_hosts h where h.contest_id=event_access_details.contest_id and h.user_id=(select auth.uid()) and h.role='manager'));

create policy event_registrations_read on public.event_registrations for select to authenticated using (
  user_id=(select auth.uid()) or exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
  or exists(select 1 from public.event_hosts h where h.contest_id=event_registrations.contest_id and h.user_id=(select auth.uid()))
);
create policy event_registrations_join on public.event_registrations for insert to authenticated with check (user_id=(select auth.uid()));
create policy event_registrations_manage on public.event_registrations for update to authenticated
  using (user_id=(select auth.uid()) or exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
    or exists(select 1 from public.event_hosts h where h.contest_id=event_registrations.contest_id and h.user_id=(select auth.uid())))
  with check (user_id=(select auth.uid()) or exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
    or exists(select 1 from public.event_hosts h where h.contest_id=event_registrations.contest_id and h.user_id=(select auth.uid())));

create policy event_sessions_read on public.event_sessions for select to authenticated using (true);
create policy event_sessions_manage on public.event_sessions for all to authenticated
  using (exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
    or exists(select 1 from public.event_hosts h where h.contest_id=event_sessions.contest_id and h.user_id=(select auth.uid()) and h.role='manager'))
  with check (exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
    or exists(select 1 from public.event_hosts h where h.contest_id=event_sessions.contest_id and h.user_id=(select auth.uid()) and h.role='manager'));

create policy event_announcements_read on public.event_announcements for select to authenticated using (
  exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
  or exists(select 1 from public.event_hosts h where h.contest_id=event_announcements.contest_id and h.user_id=(select auth.uid()))
  or audience='all'
  or exists(select 1 from public.event_registrations r where r.contest_id=event_announcements.contest_id and r.user_id=(select auth.uid()) and r.status=audience)
);
create policy event_announcements_manage on public.event_announcements for all to authenticated
  using (exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
    or exists(select 1 from public.event_hosts h where h.contest_id=event_announcements.contest_id and h.user_id=(select auth.uid()) and h.role='manager'))
  with check (author_id=(select auth.uid()) and (exists(select 1 from public.contests c where c.id=contest_id and c.host_id=(select auth.uid()))
    or exists(select 1 from public.event_hosts h where h.contest_id=event_announcements.contest_id and h.user_id=(select auth.uid()) and h.role='manager')));

create function public.validate_event_registration() returns trigger
language plpgsql security definer set search_path='' as $$
declare c public.contests; confirmed_count integer; q jsonb; answer jsonb; actor uuid; actor_role text;
begin
  if tg_op='UPDATE' then
    if new.id<>old.id or new.contest_id<>old.contest_id or new.user_id<>old.user_id or new.ticket_code<>old.ticket_code or new.created_at<>old.created_at then
      raise exception 'Registration identity is immutable';
    end if;
    actor:=(select auth.uid());
    if actor=old.user_id then
      if new.attendee_name<>old.attendee_name or new.answers<>old.answers or new.checked_in_at is distinct from old.checked_in_at
        or new.status<>'cancelled' or old.status in ('declined','cancelled') then
        raise exception '참가자는 신청 취소만 할 수 있어요.';
      end if;
    elsif not exists(select 1 from public.contests where id=old.contest_id and host_id=actor) then
      select role into actor_role from public.event_hosts where contest_id=old.contest_id and user_id=actor;
      if actor_role is null then raise exception '행사 운영 권한이 없어요.'; end if;
      if actor_role='checkin' and (new.attendee_name<>old.attendee_name or new.answers<>old.answers or new.status<>old.status) then
        raise exception '체크인 담당자는 체크인 상태만 변경할 수 있어요.';
      end if;
    end if;
    new.updated_at:=clock_timestamp();
    return new;
  end if;
  if new.user_id<>(select auth.uid()) then raise exception '본인 명의로만 신청할 수 있어요.'; end if;
  -- Serialize registrations per event without requiring participants to have
  -- UPDATE permission on the contest row (SELECT FOR UPDATE is blocked by RLS).
  perform pg_advisory_xact_lock(hashtextextended(new.contest_id::text,0));
  select * into c from public.contests where id=new.contest_id;
  if not found or not c.registration_open or c.host_id=new.user_id then raise exception 'EVENT_REGISTRATION_CLOSED'; end if;
  if c.event_start_at is not null and c.event_start_at<=clock_timestamp() then raise exception 'EVENT_REGISTRATION_CLOSED'; end if;
  for q in select value from jsonb_array_elements(c.registration_questions) loop
    answer:=new.answers->(q->>'id');
    if coalesce((q->>'required')::boolean,false) and (answer is null or answer='null'::jsonb or answer='""'::jsonb or answer='[]'::jsonb) then
      raise exception '필수 신청 질문에 답해 주세요: %',q->>'label';
    end if;
  end loop;
  if c.approval_required then new.status:='pending';
  elsif c.capacity is not null then
    select count(*) into confirmed_count from public.event_registrations where contest_id=c.id and status='confirmed';
    if confirmed_count>=c.capacity then
      if c.waitlist_enabled then new.status:='waitlisted'; else raise exception 'EVENT_FULL'; end if;
    else new.status:='confirmed'; end if;
  else new.status:='confirmed'; end if;
  new.checked_in_at:=null;
  return new;
end;
$$;
revoke all on function public.validate_event_registration() from public,anon,authenticated;
create trigger validate_event_registration before insert or update on public.event_registrations
  for each row execute function public.validate_event_registration();

create function public.validate_event_settings() returns trigger
language plpgsql security invoker set search_path='' as $$
declare q jsonb; ids text[]:='{}';
begin
  if jsonb_typeof(new.registration_questions)<>'array' or jsonb_array_length(new.registration_questions)>20 then raise exception '신청 질문은 최대 20개까지 만들 수 있어요.'; end if;
  for q in select value from jsonb_array_elements(new.registration_questions) loop
    if coalesce(q->>'id','') !~ '^[0-9a-f-]{36}$' or q->>'id'=any(ids)
      or coalesce(q->>'type','') not in ('text','paragraph','choice','checkbox','terms')
      or char_length(btrim(coalesce(q->>'label',''))) not between 1 and 200
      or coalesce(jsonb_typeof(q->'required'),'')<>'boolean'
      or coalesce(jsonb_typeof(q->'options'),'array')<>'array'
      or jsonb_array_length(coalesce(q->'options','[]'::jsonb))>20 then raise exception '신청 질문 설정을 확인해 주세요.'; end if;
    ids:=array_append(ids,q->>'id');
  end loop;
  new.updated_at:=clock_timestamp();
  return new;
end;
$$;
revoke all on function public.validate_event_settings() from public,anon,authenticated;
create trigger validate_event_settings before insert or update of event_start_at,event_end_at,location_type,location_name,address,visibility,capacity,waitlist_enabled,approval_required,registration_open,registration_questions on public.contests
  for each row execute function public.validate_event_settings();

commit;
