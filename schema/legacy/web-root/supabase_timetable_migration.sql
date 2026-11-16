-- ============================================================
-- CBGPU 시간표 마이그레이션
-- Supabase SQL Editor에서 실행하세요
-- ============================================================

create table if not exists timetable (
  id         uuid primary key default gen_random_uuid(),
  updated_at timestamptz default now(),
  grade      int not null check (grade between 1 and 3),
  class_num  int not null check (class_num between 1 and 10),
  weekday    int not null check (weekday between 0 and 4), -- 0=월 4=금
  period     int not null check (period between 1 and 9),
  subject    text not null default '',
  unique(grade, class_num, weekday, period)
);

alter table timetable enable row level security;

-- 누구나 읽기 가능
create policy "timetable_read"  on timetable for select using (true);
-- 어드민만 쓰기 가능
create policy "timetable_write" on timetable for all
  using (exists (select 1 from profiles where id = auth.uid() and is_admin = true));
