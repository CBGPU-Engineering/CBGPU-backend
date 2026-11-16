-- ============================================================
-- GPU 접속 주소 테이블 — 코드에 하드코딩돼 있던 터널/내부 주소를 DB로 옮긴다.
-- Supabase SQL Editor에서 실행하세요 (is_admin()은 supabase_security_hardening_migration.sql 필요).
--
-- 새 터널을 팔았거나 내부 IP가 바뀌었을 땐 재배포 없이 아래처럼 한 줄만 고치면 된다:
--   update gpu_endpoints set public_url = 'https://새주소.trycloudflare.com', updated_at = now() where gpu_id = 1;
--   update gpu_endpoints set internal_host = '10.138.xx.xx', updated_at = now();
-- ============================================================

create table if not exists gpu_endpoints (
  gpu_id        int primary key check (gpu_id between 1 and 4),
  public_url    text not null check (public_url ~ '^https://'),
  internal_host text not null,
  internal_port int  not null check (internal_port between 1 and 65535),
  updated_at    timestamptz not null default now()
);

insert into gpu_endpoints (gpu_id, public_url, internal_host, internal_port) values
  (1, 'https://tonight-rep-admit-cooperation.trycloudflare.com',                '10.138.23.12', 8001),
  (2, 'https://nearest-situations-elite-tools.trycloudflare.com',               '10.138.23.12', 8002),
  (3, 'https://expressions-invention-investments-consortium.trycloudflare.com', '10.138.23.12', 8003),
  (4, 'https://september-notification-pants-whatever.trycloudflare.com',        '10.138.23.12', 8004)
on conflict (gpu_id) do nothing;

alter table gpu_endpoints enable row level security;

-- 이 주소들은 원래 클라이언트 번들에 그대로 들어 있던 값이라 비로그인도 읽을 수 있다.
drop policy if exists "gpu_endpoints_read" on gpu_endpoints;
create policy "gpu_endpoints_read" on gpu_endpoints for select using (true);

drop policy if exists "gpu_endpoints_admin_write" on gpu_endpoints;
create policy "gpu_endpoints_admin_write" on gpu_endpoints for all
  using (public.is_admin(auth.uid())) with check (public.is_admin(auth.uid()));
