-- ============================================================
-- 알림 종류별 켜고 끄기 + "제작 완료 → 신청자" 알림.
-- notification_migration.sql, notify_triggers_migration.sql 다음에 실행.
-- Supabase SQL Editor에서 실행하세요.
-- ============================================================

-- 사용자별 알림 설정. 전부 기본값 true(켜짐) — 기존 사용자도 이 컬럼이 추가되는
-- 순간 전부 켜진 상태로 시작한다.
alter table profiles
  add column if not exists notify_job_new           boolean not null default true, -- 담당자: 새 제작(3D프린팅/레이저) 신청
  add column if not exists notify_job_done           boolean not null default true, -- 신청자: 내 제작이 완료됨
  add column if not exists notify_selfstudy_reminder boolean not null default true, -- 학생: 자습신청 안 했다고 리마인드
  add column if not exists notify_admin_signup       boolean not null default true, -- 관리자: 새 회원가입
  add column if not exists notify_admin_gpu_request  boolean not null default true; -- 관리자: 새 GPU 예약 신청

-- ── 제작 완료(status → 'done') → 신청자에게 알림 ───────────────────────────
-- notify-job-manager와 대칭인 notify-job-requester Edge Function을 먼저 배포:
--   supabase functions deploy notify-job-requester --no-verify-jwt
-- 아래 두 함수의 YOUR_INTERNAL_TRIGGER_SECRET_HERE는 notify_triggers_migration.sql
-- 때 쓴 것과 동일한 값으로 바꿔서 실행.
create or replace function public.notify_print_job_done() returns trigger
language plpgsql security definer as $$
begin
  if new.status = 'done' and old.status is distinct from new.status then
    perform net.http_post(
      url := 'https://fclbzjlcepoaifvcoxdw.supabase.co/functions/v1/notify-job-requester',
      headers := jsonb_build_object('Content-Type', 'application/json', 'Authorization', 'Bearer YOUR_INTERNAL_TRIGGER_SECRET_HERE'),
      body := jsonb_build_object('type', 'print', 'job_id', new.id)
    );
  end if;
  return new;
end;
$$;

drop trigger if exists on_print_job_done on print_jobs;
create trigger on_print_job_done after update on print_jobs
for each row execute function public.notify_print_job_done();

create or replace function public.notify_laser_job_done() returns trigger
language plpgsql security definer as $$
begin
  if new.status = 'done' and old.status is distinct from new.status then
    perform net.http_post(
      url := 'https://fclbzjlcepoaifvcoxdw.supabase.co/functions/v1/notify-job-requester',
      headers := jsonb_build_object('Content-Type', 'application/json', 'Authorization', 'Bearer YOUR_INTERNAL_TRIGGER_SECRET_HERE'),
      body := jsonb_build_object('type', 'laser', 'job_id', new.id)
    );
  end if;
  return new;
end;
$$;

drop trigger if exists on_laser_job_done on laser_jobs;
create trigger on_laser_job_done after update on laser_jobs
for each row execute function public.notify_laser_job_done();

-- ── 자습신청 리마인더 시각을 13:00 → 13:15 KST로 변경 ─────────────────────
-- cron.schedule은 같은 job 이름(selfstudy-reminder-daily)이 이미 있으면 새로
-- 만들지 않고 그 자리에서 갱신한다 — notification_migration.sql을 먼저
-- 실행했든 안 했든 이 문만 실행하면 최종적으로 13:15 KST로 맞춰진다.
-- YOUR_SERVICE_ROLE_KEY_HERE는 notification_migration.sql 때와 동일한 값으로.
select cron.schedule(
  'selfstudy-reminder-daily',
  '15 4 * * *',
  $$
  select net.http_post(
    url := 'https://fclbzjlcepoaifvcoxdw.supabase.co/functions/v1/selfstudy-reminder',
    headers := jsonb_build_object(
      'Authorization', 'Bearer YOUR_SERVICE_ROLE_KEY_HERE',
      'Content-Type', 'application/json'
    ),
    body := '{}'::jsonb
  );
  $$
);
