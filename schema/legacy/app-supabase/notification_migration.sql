-- ============================================================
-- 푸시 알림용 디바이스 토큰 저장 (자습신청 리마인더 등에서 사용)
-- Supabase SQL Editor에서 실행하세요
-- ============================================================

-- references profiles(id) rather than auth.users(id) directly — profiles.id
-- is itself an FK to auth.users(id), and this lets PostgREST auto-embed
-- device_tokens under a profiles query (the same convention used by
-- manager_id/preferred_printer_id elsewhere; auth.users(id) FKs don't embed).
create table if not exists device_tokens (
  id          uuid primary key default gen_random_uuid(),
  user_id     uuid references profiles(id) not null,
  token       text not null unique,
  platform    text not null check (platform in ('android', 'ios', 'web')),
  created_at  timestamptz default now()
);

alter table device_tokens enable row level security;

-- 본인 토큰만 등록/삭제 가능. 발송(cron)은 서비스 롤로 하므로 RLS를 안 탄다.
create policy "device_tokens_own" on device_tokens
  for all using (auth.uid() = user_id) with check (auth.uid() = user_id);

create index if not exists device_tokens_user_id_idx on device_tokens (user_id);

-- ============================================================
-- 매일 04:00 UTC(=13:00 KST)에 selfstudy-reminder Edge Function을 호출한다.
-- 실행 전에 아래 두 가지를 먼저 해야 함:
--
-- 1. Edge Function 배포 + secrets 등록 (터미널에서):
--      supabase functions deploy selfstudy-reminder
--      supabase secrets set EDUSET_TOKEN_SECRET=<.env.local과 동일한 값>
--      supabase secrets set FIREBASE_SERVICE_ACCOUNT_JSON='<서비스 계정 JSON 전체>'
--
-- 2. 아래 SQL의 서비스 롤 키 부분을 실제 값으로 바꿔서 실행 (Project Settings
--    > API에서 확인). DB 설정에만 저장되고 클라이언트에는 노출되지 않는다.
-- ============================================================
create extension if not exists pg_cron;
create extension if not exists pg_net;

select cron.schedule(
  'selfstudy-reminder-daily',
  '0 4 * * *',
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
