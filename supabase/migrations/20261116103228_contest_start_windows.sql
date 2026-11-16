-- Additive windows: NULL retains the existing immediate-open behavior.
begin;
alter table public.contests add column if not exists registration_opens_at timestamptz;
alter table public.contests add column if not exists starts_at timestamptz;
do $$begin
 if not exists(select 1 from pg_constraint where conrelid='public.contests'::regclass and conname='contest_registration_start_window') then
  alter table public.contests add constraint contest_registration_start_window check(registration_opens_at is null or (isfinite(registration_opens_at) and (registration_closes_at is null or registration_opens_at<registration_closes_at) and (event_start_at is null or registration_opens_at<event_start_at)));
 end if;
 if not exists(select 1 from pg_constraint where conrelid='public.contests'::regclass and conname='contest_submission_start_window') then
  alter table public.contests add constraint contest_submission_start_window check(starts_at is null or (isfinite(starts_at) and (ends_at is null or starts_at<ends_at)));
 end if;
end$$;
grant update(registration_opens_at,starts_at) on public.contests to authenticated;
-- SECURITY DEFINER reads the actual contest independently of caller RLS. This
-- trigger only denies early writes; all existing authorization stays in force.
create or replace function public.check_event_registration_start() returns trigger
language plpgsql security definer set search_path='' as $$
declare opens timestamptz;new_entry boolean;existing_status text;
begin
 if tg_op='INSERT' then
  -- INSERT ... ON CONFLICT also runs INSERT triggers for existing requests.
  -- Permit an existing active request to reach its unchanged UPDATE checks.
  select r.status into existing_status from public.event_registrations r where r.contest_id=new.contest_id and r.user_id=new.user_id;
  new_entry:=not found or existing_status in('cancelled','declined');
 else new_entry:=old.status in('cancelled','declined') and new.status not in('cancelled','declined');end if;
 if not new_entry then return new;end if;
 select c.registration_opens_at into opens from public.contests c where c.id=new.contest_id;
 if opens is not null and clock_timestamp()<opens then raise exception 'EVENT_REGISTRATION_NOT_STARTED' using errcode='23514';end if;
 return new;
end$$;
revoke all on function public.check_event_registration_start() from public,anon,authenticated;
drop trigger if exists aa_event_registration_start on public.event_registrations;
create trigger aa_event_registration_start before insert or update on public.event_registrations for each row execute function public.check_event_registration_start();
create or replace function public.check_contest_submission_start() returns trigger
language plpgsql security definer set search_path='' as $$
declare opens timestamptz;
begin
 -- Match the existing team's FK-preservation exception without modifying any
 -- ownership, answers, validation, approval or membership logic.
 if tg_op='UPDATE' and pg_trigger_depth()>1 and old.team_id is not null and new.team_id is null
  and (new.contest_id,new.user_id,new.answers,new.participant_name,new.student_id) is not distinct from (old.contest_id,old.user_id,old.answers,old.participant_name,old.student_id) then return new;end if;
 select c.starts_at into opens from public.contests c where c.id=new.contest_id;
 if opens is not null and clock_timestamp()<opens then raise exception 'CONTEST_NOT_STARTED' using errcode='23514';end if;
 return new;
end$$;
revoke all on function public.check_contest_submission_start() from public,anon,authenticated;
drop trigger if exists aa_contest_submission_start on public.contest_submissions;
create trigger aa_contest_submission_start before insert or update on public.contest_submissions for each row execute function public.check_contest_submission_start();
notify pgrst, 'reload schema';
commit;
