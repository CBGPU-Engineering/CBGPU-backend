begin;

alter table public.contests
  add column registration_closes_at timestamptz,
  add column lifecycle_status text not null default 'published' check (lifecycle_status in ('published','cancelled','archived')),
  add column cancellation_message text not null default '' check (char_length(cancellation_message) <= 2000),
  add constraint contest_registration_close_time check (registration_closes_at is null or event_start_at is null or registration_closes_at <= event_start_at);

create index contests_lifecycle_event_idx on public.contests(lifecycle_status,event_start_at) where lifecycle_status<>'archived';
grant update(registration_closes_at,lifecycle_status,cancellation_message) on public.contests to authenticated;

create policy contests_event_manager_update on public.contests for update to authenticated
  using (exists(select 1 from public.event_hosts h where h.contest_id=contests.id and h.user_id=(select auth.uid()) and h.role='manager'))
  with check (exists(select 1 from public.event_hosts h where h.contest_id=contests.id and h.user_id=(select auth.uid()) and h.role='manager'));
create policy contest_submissions_event_host_read on public.contest_submissions for select to authenticated using (
  exists(select 1 from public.event_hosts h where h.contest_id=contest_submissions.contest_id and h.user_id=(select auth.uid()))
);
create policy contest_files_event_host_read on storage.objects for select to authenticated using (
  bucket_id='contest-submissions' and exists(select 1 from public.event_hosts h
    where h.contest_id::text=split_part(name,'/',1) and h.user_id=(select auth.uid()))
);

create function public.validate_event_operations() returns trigger
language plpgsql security invoker set search_path='' as $$
begin
  if new.registration_closes_at is not null and not isfinite(new.registration_closes_at) then
    raise exception '참가 신청 마감 시간을 확인해 주세요.';
  end if;
  if new.lifecycle_status='cancelled' and btrim(new.cancellation_message)='' then
    new.cancellation_message:='호스트가 행사를 취소했어요.';
  end if;
  new.updated_at:=clock_timestamp();
  return new;
end;
$$;
revoke all on function public.validate_event_operations() from public,anon,authenticated;
create trigger validate_event_operations before insert or update of registration_closes_at,lifecycle_status,cancellation_message on public.contests
  for each row execute function public.validate_event_operations();

create or replace function public.validate_event_registration() returns trigger
language plpgsql security definer set search_path='' as $$
declare c public.contests; confirmed_count integer; q jsonb; answer jsonb; actor uuid; actor_role text; rejoining boolean:=false;
begin
  if tg_op='UPDATE' and (new.id<>old.id or new.contest_id<>old.contest_id or new.user_id<>old.user_id or new.ticket_code<>old.ticket_code or new.created_at<>old.created_at) then
    raise exception 'Registration identity is immutable';
  end if;
  if tg_op='UPDATE' and pg_trigger_depth()>1 then new.updated_at:=clock_timestamp(); return new; end if;

  actor:=(select auth.uid());
  if tg_op='UPDATE' then
    if actor=old.user_id then
      select * into c from public.contests where id=old.contest_id;
      if not found or not c.registration_open or c.lifecycle_status<>'published'
        or (c.registration_closes_at is not null and c.registration_closes_at<=clock_timestamp())
        or (c.event_start_at is not null and c.event_start_at<=clock_timestamp()) then raise exception 'EVENT_REGISTRATION_CLOSED'; end if;
      rejoining:=old.status in ('cancelled','declined') and new.status='pending';
      if new.checked_in_at is distinct from old.checked_in_at
        or (not rejoining and new.status not in (old.status,'cancelled'))
        or (new.status='cancelled' and old.status in ('declined','cancelled')) then
        raise exception '참가자는 신청 정보 수정 또는 취소만 할 수 있어요.';
      end if;
    elsif not exists(select 1 from public.contests where id=old.contest_id and host_id=actor) then
      select role into actor_role from public.event_hosts where contest_id=old.contest_id and user_id=actor;
      if actor_role is null then raise exception '행사 운영 권한이 없어요.'; end if;
      if actor_role='checkin' and (new.attendee_name<>old.attendee_name or new.answers<>old.answers or new.status<>old.status) then
        raise exception '체크인 담당자는 체크인 상태만 변경할 수 있어요.';
      end if;
    end if;
  else
    if new.user_id<>actor then raise exception '본인 명의로만 신청할 수 있어요.'; end if;
    perform pg_advisory_xact_lock(hashtextextended(new.contest_id::text,0));
    select * into c from public.contests where id=new.contest_id;
    if not found or not c.registration_open or c.lifecycle_status<>'published' or c.host_id=new.user_id
      or (c.registration_closes_at is not null and c.registration_closes_at<=clock_timestamp())
      or (c.event_start_at is not null and c.event_start_at<=clock_timestamp()) then raise exception 'EVENT_REGISTRATION_CLOSED'; end if;
  end if;

  if tg_op='INSERT' or actor=old.user_id then
    for q in select value from jsonb_array_elements(c.registration_questions) loop
      answer:=new.answers->(q->>'id');
      if coalesce((q->>'required')::boolean,false) and (answer is null or answer='null'::jsonb or answer='""'::jsonb or answer='[]'::jsonb or answer='false'::jsonb) then
        raise exception '필수 신청 질문에 답해 주세요: %',q->>'label';
      end if;
    end loop;
  end if;

  if tg_op='INSERT' or rejoining then
    perform pg_advisory_xact_lock(hashtextextended(new.contest_id::text,0));
    if c.approval_required then new.status:='pending';
    elsif c.capacity is not null then
      select count(*) into confirmed_count from public.event_registrations where contest_id=c.id and status='confirmed' and id<>new.id;
      if confirmed_count>=c.capacity then
        if c.waitlist_enabled then new.status:='waitlisted'; else raise exception 'EVENT_FULL'; end if;
      else new.status:='confirmed'; end if;
    else new.status:='confirmed'; end if;
    new.checked_in_at:=null;
  end if;
  new.updated_at:=clock_timestamp();
  return new;
end;
$$;

create function public.promote_event_waitlist() returns trigger
language plpgsql security definer set search_path='' as $$
declare next_id uuid;
begin
  if old.status='confirmed' and new.status<>'confirmed' then
    perform pg_advisory_xact_lock(hashtextextended(new.contest_id::text,0));
    select id into next_id from public.event_registrations
      where contest_id=new.contest_id and status='waitlisted' order by created_at,id limit 1 for update skip locked;
    if next_id is not null then update public.event_registrations set status='confirmed' where id=next_id; end if;
  end if;
  return null;
end;
$$;
revoke all on function public.promote_event_waitlist() from public,anon,authenticated;
create trigger promote_event_waitlist after update of status on public.event_registrations
  for each row execute function public.promote_event_waitlist();

commit;
