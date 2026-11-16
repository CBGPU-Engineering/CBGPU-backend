-- Run in the Supabase SQL Editor after supabase_account_management_migration.sql.
-- Existing duplicate student IDs are preserved for manual review. New claims
-- for the same student ID in the same school year are rejected atomically.

begin;

create or replace function public.cbgpu_school_year(at_time timestamptz)
returns integer
language sql
stable
set search_path = ''
as $$
  select extract(year from at_time at time zone 'Asia/Seoul')::integer
       - case when extract(month from at_time at time zone 'Asia/Seoul') < 3
              then 1 else 0 end;
$$;

alter table public.profiles
  add column if not exists student_id_school_year integer;

-- The existing accounts predate this column. Their signup year is the best
-- available historical value; no current account or assignment is removed.
update public.profiles as p
set student_id_school_year = public.cbgpu_school_year(u.created_at)
from auth.users as u
where p.id = u.id
  and nullif(btrim(p.student_id), '') is not null
  and p.student_id_school_year is null;

create index if not exists profiles_student_id_school_year_lookup
on public.profiles (student_id_school_year, (btrim(student_id)))
where student_id is not null;

create or replace function public.guard_student_id_claim()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
declare
  normalized_id text;
  claim_year integer;
begin
  normalized_id := nullif(btrim(new.student_id), '');

  if normalized_id is null then
    if tg_op = 'INSERT' then
      raise exception 'STUDENT_ID_REQUIRED' using errcode = '23514';
    end if;
    -- Account deletion anonymizes its profile. Ordinary users cannot clear
    -- the ID to open a slot for another account; admins and service role can.
    if old.student_id is not null
       and auth.role() is distinct from 'service_role'
       and session_user not in ('postgres', 'supabase_admin')
       and not public.is_admin(auth.uid()) then
      raise exception 'STUDENT_ID_REQUIRED' using errcode = '23514';
    end if;
    new.student_id := null;
    new.student_id_school_year := null;
    return new;
  end if;

  if normalized_id !~ '^[1-3][1-3][0-9]{2}$' then
    raise exception 'INVALID_STUDENT_ID' using errcode = '23514';
  end if;

  if tg_op = 'UPDATE' and normalized_id is not distinct from btrim(old.student_id) then
    new.student_id := normalized_id;
    -- The year belongs to the existing claim, not to caller-supplied data.
    new.student_id_school_year := old.student_id_school_year;
    return new;
  end if;

  claim_year := public.cbgpu_school_year(now());
  -- Serialize competing signup/profile updates before checking existing
  -- claims. This works while old duplicate rows prevent a unique index.
  perform pg_catalog.pg_advisory_xact_lock(
    pg_catalog.hashtextextended('cbgpu:student_id:' || claim_year || ':' || normalized_id, 0)
  );
  if exists (
    select 1 from public.profiles as p
    where p.student_id_school_year = claim_year
      and btrim(p.student_id) = normalized_id
      and p.id <> new.id
  ) then
    raise exception 'STUDENT_ID_ALREADY_REGISTERED' using errcode = '23505';
  end if;

  new.student_id := normalized_id;
  new.student_id_school_year := claim_year;
  return new;
end;
$$;

drop trigger if exists guard_student_id_claim on public.profiles;
create trigger guard_student_id_claim
before insert or update of student_id, student_id_school_year on public.profiles
for each row execute function public.guard_student_id_claim();

commit;

-- After manually resolving the 2026 duplicate accounts, add a UNIQUE index
-- on (student_id_school_year, btrim(student_id)) for database-level defense
-- in depth. Do not add it now: 14 existing duplicate pairs would reject it.
