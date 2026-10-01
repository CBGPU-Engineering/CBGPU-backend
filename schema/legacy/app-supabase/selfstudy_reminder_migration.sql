-- Run after notification_migration.sql. Safe to run again without resetting
-- an existing student's opt-in choice.
create extension if not exists pg_cron;
create extension if not exists pg_net;
alter table public.profiles
  add column if not exists notify_selfstudy_reminder boolean not null default false;
alter table public.profiles
  alter column notify_selfstudy_reminder set default false;

alter table public.profiles
  add column if not exists selfstudy_reminder_time time not null default '13:15',
  add column if not exists selfstudy_reminder_repeat_minutes integer not null default 30,
  add column if not exists selfstudy_reminder_repeat_count integer not null default 2;

do $$ begin
  if not exists (select 1 from pg_constraint where conname = 'selfstudy_reminder_time_valid') then
    alter table public.profiles add constraint selfstudy_reminder_time_valid check (
      selfstudy_reminder_time between time '08:00' and time '15:15'
      and extract(minute from selfstudy_reminder_time)::integer % 15 = 0
      and extract(second from selfstudy_reminder_time) = 0
    );
  end if;
  if not exists (select 1 from pg_constraint where conname = 'selfstudy_reminder_repeat_minutes_valid') then
    alter table public.profiles add constraint selfstudy_reminder_repeat_minutes_valid check (selfstudy_reminder_repeat_minutes in (15, 30, 45, 60));
  end if;
  if not exists (select 1 from pg_constraint where conname = 'selfstudy_reminder_repeat_count_valid') then
    alter table public.profiles add constraint selfstudy_reminder_repeat_count_valid check (selfstudy_reminder_repeat_count between 0 and 5);
  end if;
end $$;

-- The service-role Edge Function claims each attempt before sending. A unique
-- key prevents duplicate sends when a cron run is retried or overlaps.
create table if not exists public.selfstudy_reminder_attempts (
  user_id uuid not null references public.profiles(id) on delete cascade,
  study_date date not null,
  attempt integer not null check (attempt between 0 and 5),
  claimed_at timestamptz not null default now(),
  primary key (user_id, study_date, attempt)
);
alter table public.selfstudy_reminder_attempts enable row level security;
drop policy if exists "read_own_selfstudy_reminder_attempts" on public.selfstudy_reminder_attempts;
create policy "read_own_selfstudy_reminder_attempts" on public.selfstudy_reminder_attempts
  for select to authenticated using ((select auth.uid()) = user_id);

-- Opening an alert is not an acknowledgement. The user explicitly taps
-- "오늘 알림 확인" on /study to stop repeats for that date.
create table if not exists public.selfstudy_reminder_acknowledgements (
  user_id uuid not null references public.profiles(id) on delete cascade,
  study_date date not null,
  acknowledged_at timestamptz not null default now(),
  primary key (user_id, study_date)
);
alter table public.selfstudy_reminder_acknowledgements enable row level security;
drop policy if exists "read_own_selfstudy_reminder_acknowledgements" on public.selfstudy_reminder_acknowledgements;
create policy "read_own_selfstudy_reminder_acknowledgements" on public.selfstudy_reminder_acknowledgements
  for select to authenticated using ((select auth.uid()) = user_id);
drop policy if exists "acknowledge_own_selfstudy_reminder" on public.selfstudy_reminder_acknowledgements;
create policy "acknowledge_own_selfstudy_reminder" on public.selfstudy_reminder_acknowledgements
  for insert to authenticated with check ((select auth.uid()) = user_id);

-- First create a Vault secret named cbgpu_service_role_key containing the
-- project's service-role key (Dashboard > Vault). Never commit it to SQL.
-- Replace the old once-daily job. The Edge Function filters by Korean time,
-- personal settings and the actual self-study policy before doing EduSet I/O.
select cron.unschedule(jobid) from cron.job where jobname in ('selfstudy-reminder-daily', 'selfstudy-reminder-quarter-hour');
select cron.schedule(
  'selfstudy-reminder-quarter-hour',
  '*/15 * * * *',
  $$
  select net.http_post(
    url := 'https://fclbzjlcepoaifvcoxdw.supabase.co/functions/v1/selfstudy-reminder',
    headers := jsonb_build_object(
      'Authorization', 'Bearer ' || (select decrypted_secret from vault.decrypted_secrets where name = 'cbgpu_service_role_key'),
      'Content-Type', 'application/json'
    ),
    body := '{}'::jsonb
  );
  $$
);
