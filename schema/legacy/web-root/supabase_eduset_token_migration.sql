-- ============================================================
-- CBGPU 자습신청(eduset) 공유 토큰 마이그레이션
-- Supabase SQL Editor에서 실행하세요
-- ============================================================

create table if not exists eduset_settings (
  id                int primary key default 1,
  access_token_enc  text,         -- 암호화된 값만 저장 (평문 저장 안 함)
  updated_at        timestamptz default now(),
  updated_by        uuid references auth.users(id),
  constraint eduset_settings_singleton check (id = 1)
);

alter table eduset_settings enable row level security;

-- 누구나 읽기 가능 (어차피 암호화된 값이라 EDUSET_TOKEN_SECRET 없이는 무용함)
create policy "eduset_settings_read" on eduset_settings for select using (true);
-- 어드민만 쓰기 가능
create policy "eduset_settings_write" on eduset_settings for all
  using (exists (select 1 from profiles where id = auth.uid() and is_admin = true));
