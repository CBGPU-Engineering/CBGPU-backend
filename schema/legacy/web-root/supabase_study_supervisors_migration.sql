-- ============================================================
-- 오늘의 자습 감독 — 외부 Google Apps Script를 대신 가져와 한 줄로 저장해 두는 테이블.
-- Supabase SQL Editor에서 실행하세요.
--
-- 흐름:  pg_cron(1시간마다) ─→ Edge Function refresh-supervisors ─→ 외부 스크립트 ─→ 이 테이블 ─→ 모든 사용자(웹/앱)
-- 사용자는 이 테이블을 읽기만 한다. 외부 스크립트를 부르는 건 스케줄러뿐이라 접속자 수와 무관하다.
-- ============================================================

create table if not exists study_supervisors (
  id          int primary key default 1,
  duty_date   date,                                   -- duties가 어느 날(KST)의 것인지
  date_label  text not null default '',               -- 스크립트가 화면에 보여주는 날짜 문구 그대로
  duties      jsonb not null default '[]'::jsonb,     -- [{ "type": "...", "teacher": "..." }]
  fetched_at  timestamptz not null default 'epoch',   -- 마지막 호출 시도 (너무 잦은 호출 방지용)
  updated_at  timestamptz,                            -- 마지막 성공
  constraint study_supervisors_singleton check (id = 1)
);

insert into study_supervisors (id) values (1) on conflict (id) do nothing;

alter table study_supervisors enable row level security;

-- 로그인한 사용자는 읽기만 가능. 쓰기 정책은 없다 — 서버(service role)만 쓴다.
drop policy if exists "study_supervisors_read" on study_supervisors;
create policy "study_supervisors_read" on study_supervisors for select to authenticated using (true);

-- ============================================================
-- 스케줄: 매시 정각에 Edge Function을 호출한다 (UTC 15:00 = KST 00:00 포함, 그래서 날이 바뀌면 곧바로 갱신).
-- 순서가 중요하다:
--   1. 터미널에서 함수를 먼저 배포한다:
--        supabase functions deploy refresh-supervisors --no-verify-jwt --project-ref fclbzjlcepoaifvcoxdw
--   2. Vault에 cbgpu_service_role_key 시크릿이 있어야 한다 (selfstudy-reminder가 이미 쓰는 것과 같은 것).
--   3. 아래를 실행한다. 서비스 키는 이 파일에 쓰지 않고 Vault에서 읽는다.
-- ============================================================
create extension if not exists pg_cron;
create extension if not exists pg_net;

select cron.unschedule(jobid) from cron.job where jobname = 'refresh-study-supervisors';
select cron.schedule(
  'refresh-study-supervisors',
  '0 * * * *',
  $$
  select net.http_post(
    url := 'https://fclbzjlcepoaifvcoxdw.supabase.co/functions/v1/refresh-supervisors',
    headers := jsonb_build_object(
      'Authorization', 'Bearer ' || (select decrypted_secret from vault.decrypted_secrets where name = 'cbgpu_service_role_key'),
      'Content-Type', 'application/json'
    ),
    body := '{}'::jsonb
  );
  $$
);

-- 잘 도는지 확인:   select * from net._http_response order by created desc limit 5;
-- 바로 한 번 실행:  위 net.http_post(...) 부분만 단독으로 실행
-- 스케줄 해제:      select cron.unschedule('refresh-study-supervisors');
