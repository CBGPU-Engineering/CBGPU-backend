-- ============================================================
-- CBGPU 사이트 공지 배너 마이그레이션
-- Supabase SQL Editor에서 실행하세요.
-- is_admin()은 supabase_security_hardening_migration.sql에서 만든 함수를 재사용한다
-- (RLS 재귀를 피하려고 SECURITY DEFINER로 만들어둔 것 — 먼저 그 마이그레이션을 실행해야 함).
-- ============================================================

create table if not exists site_banner (
  id         int primary key default 1,
  message    text not null default '',
  is_active  boolean not null default false,
  updated_at timestamptz default now(),
  updated_by uuid references auth.users(id),
  constraint site_banner_singleton check (id = 1)
);

insert into site_banner (id) values (1) on conflict (id) do nothing;

alter table site_banner enable row level security;

drop policy if exists "site_banner_read" on site_banner;
create policy "site_banner_read" on site_banner for select using (true);

drop policy if exists "site_banner_admin_write" on site_banner;
create policy "site_banner_admin_write" on site_banner for all
  using (public.is_admin(auth.uid()));
