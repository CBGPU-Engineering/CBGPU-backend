-- ============================================================
-- Event-driven push notifications (as opposed to the daily cron in
-- notification_migration.sql). Fires a pg_net POST to a notify-* Edge
-- Function on the relevant table's AFTER INSERT — fire-and-forget, doesn't
-- block the INSERT itself since pg_net queues the request async.
-- See also notification_prefs_migration.sql — per-user on/off toggles for
-- every notification type here, plus the AFTER UPDATE (job completed)
-- counterpart to the AFTER INSERT triggers below.
--
-- Before running: deploy the two functions and set the shared secret
--   supabase functions deploy notify-job-manager --no-verify-jwt
--   supabase functions deploy notify-admin-event --no-verify-jwt
--   supabase secrets set INTERNAL_TRIGGER_SECRET=<openssl rand -hex 32>
-- then replace YOUR_INTERNAL_TRIGGER_SECRET_HERE below with that same value
-- before running this file (Supabase SQL Editor or `supabase db query`).
-- ============================================================
create extension if not exists pg_net;

-- ── 3D print job submitted → notify the assigned/eligible manager(s) ──────
create or replace function public.notify_new_print_job() returns trigger
language plpgsql security definer as $$
begin
  perform net.http_post(
    url := 'https://fclbzjlcepoaifvcoxdw.supabase.co/functions/v1/notify-job-manager',
    headers := jsonb_build_object('Content-Type', 'application/json', 'Authorization', 'Bearer YOUR_INTERNAL_TRIGGER_SECRET_HERE'),
    body := jsonb_build_object('type', 'print', 'job_id', new.id)
  );
  return new;
end;
$$;

drop trigger if exists on_print_job_created on print_jobs;
create trigger on_print_job_created after insert on print_jobs
for each row execute function public.notify_new_print_job();

-- ── Laser job submitted → notify the assigned/eligible manager(s) ─────────
create or replace function public.notify_new_laser_job() returns trigger
language plpgsql security definer as $$
begin
  perform net.http_post(
    url := 'https://fclbzjlcepoaifvcoxdw.supabase.co/functions/v1/notify-job-manager',
    headers := jsonb_build_object('Content-Type', 'application/json', 'Authorization', 'Bearer YOUR_INTERNAL_TRIGGER_SECRET_HERE'),
    body := jsonb_build_object('type', 'laser', 'job_id', new.id)
  );
  return new;
end;
$$;

drop trigger if exists on_laser_job_created on laser_jobs;
create trigger on_laser_job_created after insert on laser_jobs
for each row execute function public.notify_new_laser_job();

-- ── New account created → notify every admin ──────────────────────────────
create or replace function public.notify_new_signup() returns trigger
language plpgsql security definer as $$
begin
  perform net.http_post(
    url := 'https://fclbzjlcepoaifvcoxdw.supabase.co/functions/v1/notify-admin-event',
    headers := jsonb_build_object('Content-Type', 'application/json', 'Authorization', 'Bearer YOUR_INTERNAL_TRIGGER_SECRET_HERE'),
    body := jsonb_build_object('type', 'signup', 'full_name', new.full_name, 'student_id', new.student_id)
  );
  return new;
end;
$$;

drop trigger if exists on_profile_created_notify_admin on profiles;
create trigger on_profile_created_notify_admin after insert on profiles
for each row execute function public.notify_new_signup();

-- ── New GPU reservation request → notify every admin ──────────────────────
create or replace function public.notify_new_gpu_request() returns trigger
language plpgsql security definer as $$
begin
  if new.status = 'pending' then
    perform net.http_post(
      url := 'https://fclbzjlcepoaifvcoxdw.supabase.co/functions/v1/notify-admin-event',
      headers := jsonb_build_object('Content-Type', 'application/json', 'Authorization', 'Bearer YOUR_INTERNAL_TRIGGER_SECRET_HERE'),
      body := jsonb_build_object('type', 'gpu_request', 'gpu_id', new.gpu_id)
    );
  end if;
  return new;
end;
$$;

drop trigger if exists on_reservation_created_notify_admin on reservations;
create trigger on_reservation_created_notify_admin after insert on reservations
for each row execute function public.notify_new_gpu_request();
