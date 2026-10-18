begin;

create table public.contests (
  id uuid primary key default gen_random_uuid(),
  host_id uuid not null references auth.users(id) on delete cascade,
  title text not null check (char_length(btrim(title)) between 1 and 120),
  description text not null default '' check (char_length(description) <= 10000),
  ends_at timestamptz,
  is_open boolean not null default true,
  fields jsonb not null,
  created_at timestamptz not null default clock_timestamp()
);
create index contests_host_id_idx on public.contests(host_id);
create index contests_created_at_idx on public.contests(created_at desc);

create table public.contest_submissions (
  contest_id uuid not null references public.contests(id) on delete cascade,
  user_id uuid not null references auth.users(id) on delete cascade,
  participant_name text not null check (char_length(btrim(participant_name)) between 1 and 100),
  student_id text not null default '' check (char_length(student_id) <= 30),
  answers jsonb not null check (jsonb_typeof(answers) = 'object'),
  submitted_at timestamptz not null default clock_timestamp(),
  primary key (contest_id, user_id)
);
create index contest_submissions_user_id_idx on public.contest_submissions(user_id);
create index contest_submissions_recent_idx on public.contest_submissions(contest_id, submitted_at desc);

alter table public.contests enable row level security;
alter table public.contest_submissions enable row level security;
revoke all on public.contests, public.contest_submissions from anon, authenticated;
grant select, insert on public.contests to authenticated;
grant update(is_open, ends_at) on public.contests to authenticated;
grant select, insert on public.contest_submissions to authenticated;
-- PostgREST upsert includes the conflict keys in its UPDATE; the trigger below makes them immutable.
grant update(contest_id, user_id, participant_name, student_id, answers) on public.contest_submissions to authenticated;

create policy contests_read on public.contests for select to authenticated using (true);
create policy contests_create on public.contests for insert to authenticated
  with check (host_id = (select auth.uid()));
create policy contests_host_update on public.contests for update to authenticated
  using (host_id = (select auth.uid())) with check (host_id = (select auth.uid()));
create policy contest_submissions_read on public.contest_submissions for select to authenticated
  using (user_id = (select auth.uid()) or exists (
    select 1 from public.contests c where c.id = contest_id and c.host_id = (select auth.uid())
  ));
create policy contest_submissions_insert on public.contest_submissions for insert to authenticated
  with check (user_id = (select auth.uid()) and exists (
    select 1 from public.contests c where c.id = contest_id and c.host_id <> (select auth.uid())
      and c.is_open and (c.ends_at is null or c.ends_at > clock_timestamp())
  ));
create policy contest_submissions_update on public.contest_submissions for update to authenticated
  using (user_id = (select auth.uid()))
  with check (user_id = (select auth.uid()) and exists (
    select 1 from public.contests c where c.id = contest_id and c.host_id <> (select auth.uid())
      and c.is_open and (c.ends_at is null or c.ends_at > clock_timestamp())
  ));

-- Invoker functions never bypass row permissions; trigger entry points are not RPC APIs.
create function public.validate_contest() returns trigger
language plpgsql security invoker set search_path = '' as $$
declare f jsonb; seen text[] := '{}';
begin
  if tg_op = 'UPDATE' then
    if new.id <> old.id or new.host_id <> old.host_id or new.fields <> old.fields
      or new.title <> old.title or new.description <> old.description or new.created_at <> old.created_at then
      raise exception 'Contest fields and ownership are immutable';
    end if;
  end if;
  if tg_op = 'INSERT' or new.ends_at is distinct from old.ends_at then
    if new.ends_at is not null and (not isfinite(new.ends_at) or new.ends_at <= clock_timestamp()) then
      raise exception '마감 시간은 현재 서버 시각 이후로 지정해 주세요.';
    end if;
  end if;
  if tg_op = 'UPDATE' then return new; end if;
  if jsonb_typeof(new.fields) <> 'array' or jsonb_array_length(new.fields) not between 1 and 30 then
    raise exception '제출 항목은 1~30개로 구성해 주세요.';
  end if;
  for f in select value from jsonb_array_elements(new.fields) loop
    if jsonb_typeof(f) <> 'object'
      or coalesce(f->>'id', '') !~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
      or f->>'id' = any(seen)
      or coalesce(f->>'type', '') not in ('file', 'link', 'text')
      or coalesce(jsonb_typeof(f->'label'), '') <> 'string'
      or char_length(btrim(coalesce(f->>'label', ''))) not between 1 and 200
      or coalesce(jsonb_typeof(f->'description'), '') <> 'string'
      or char_length(f->>'description') > 1000
      or coalesce(jsonb_typeof(f->'required'), '') <> 'boolean' then
      raise exception '제출 항목 설정을 확인해 주세요.';
    end if;
    seen := array_append(seen, f->>'id');
    if f->>'type' = 'file' then
      if coalesce(jsonb_typeof(f->'extensions'), '') <> 'string'
        or char_length(f->>'extensions') > 200
        or ((f->>'extensions') <> '' and (f->>'extensions') !~ '^\.[a-z0-9]+(\.[a-z0-9]+)*(,\.[a-z0-9]+(\.[a-z0-9]+)*)*$')
        or coalesce(f->>'max_size_mb', '') !~ '^[0-9]+$' then
        raise exception '파일 확장자와 용량 제한을 확인해 주세요.';
      end if;
      if (f->>'max_size_mb')::numeric not between 1 and 50 then raise exception '파일 용량은 1~50MB로 지정해 주세요.'; end if;
    end if;
  end loop;
  return new;
end;
$$;
revoke all on function public.validate_contest() from public, anon, authenticated;
create trigger validate_contest before insert or update on public.contests
  for each row execute function public.validate_contest();

insert into storage.buckets (id, name, public, file_size_limit)
values ('contest-submissions', 'contest-submissions', false, 52428800);

create policy contest_files_upload on storage.objects for insert to authenticated with check (
  bucket_id = 'contest-submissions'
  and split_part(name, '/', 2) = (select auth.uid())::text
  and exists (select 1 from public.contests c where c.id::text = split_part(name, '/', 1)
    and c.host_id <> (select auth.uid()) and c.is_open and (c.ends_at is null or c.ends_at > clock_timestamp())
    and exists (select 1 from jsonb_array_elements(c.fields) f
      where f->>'id' = split_part(name, '/', 3) and f->>'type' = 'file'
      and ((f->>'extensions') = '' or exists (
        select 1 from unnest(string_to_array(f->>'extensions', ',')) ext
        where right(lower(name), char_length(ext)) = ext
      ))
    )
  )
);
create policy contest_files_read on storage.objects for select to authenticated using (
  bucket_id = 'contest-submissions' and (
    split_part(name, '/', 2) = (select auth.uid())::text
    or exists (select 1 from public.contests c where c.id::text = split_part(name, '/', 1) and c.host_id = (select auth.uid()))
  )
);
-- No overwrite permission: a file already submitted cannot be changed after the deadline.
-- Only unreferenced uploads can be cleaned up by their uploader.
create policy contest_files_cleanup on storage.objects for delete to authenticated using (
  bucket_id = 'contest-submissions' and split_part(name, '/', 2) = (select auth.uid())::text
  and not exists (select 1 from public.contest_submissions s, jsonb_each(s.answers) a
    where s.user_id = (select auth.uid()) and a.value->>'path' = name)
);

create function public.validate_contest_submission() returns trigger
language plpgsql security invoker set search_path = '' as $$
declare c public.contests; f jsonb; v jsonb; k text; actual_size bigint;
begin
  if tg_op = 'UPDATE' and (new.contest_id <> old.contest_id or new.user_id <> old.user_id) then
    raise exception 'Submission ownership is immutable';
  end if;
  if (select auth.uid()) is null or new.user_id <> (select auth.uid()) then raise exception '로그인이 필요해요.'; end if;
  select * into c from public.contests where id = new.contest_id;
  if not found or c.host_id = new.user_id then raise exception '참가자만 제출할 수 있어요.'; end if;
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
        or coalesce(v->>'path', '') not like c.id::text || '/' || new.user_id::text || '/' || (f->>'id') || '/%'
        or coalesce(jsonb_typeof(v->'size'), '') <> 'number'
        or coalesce(v->>'size', '') !~ '^[0-9]+$' then raise exception '파일 정보를 확인해 주세요: %', f->>'label'; end if;
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
revoke all on function public.validate_contest_submission() from public, anon, authenticated;
create trigger validate_contest_submission before insert or update on public.contest_submissions
  for each row execute function public.validate_contest_submission();

create function public.contest_server_time() returns timestamptz
language sql volatile security invoker set search_path = '' as $$ select clock_timestamp(); $$;
revoke all on function public.contest_server_time() from public, anon;
grant execute on function public.contest_server_time() to authenticated;

commit;
