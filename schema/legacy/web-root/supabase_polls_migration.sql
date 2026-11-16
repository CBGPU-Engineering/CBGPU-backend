-- ============================================================
-- CBGPU 투표 시스템 v2 마이그레이션
-- Supabase SQL Editor에서 실행하세요
-- ============================================================

drop table if exists poll_votes    cascade;
drop table if exists poll_answers  cascade;
drop table if exists poll_responses cascade;
drop table if exists poll_questions cascade;
drop table if exists polls         cascade;

-- 투표 헤더
create table polls (
  id          uuid primary key default gen_random_uuid(),
  created_at  timestamptz default now(),
  title       text not null,
  description text default '',
  ends_at     timestamptz,
  is_active   boolean default true,
  show_results boolean default true,
  created_by  uuid references auth.users(id)
);

-- 질문 목록 (질문 유형: choice, checkbox, text, paragraph, scale, dropdown)
create table poll_questions (
  id          uuid primary key default gen_random_uuid(),
  poll_id     uuid references polls(id) on delete cascade not null,
  order_num   int not null default 0,
  type        text not null,
  title       text not null,
  description text default '',
  required    boolean default false,
  options     jsonb default '[]',  -- [{id,text}]
  config      jsonb default '{}'   -- {min,max,minLabel,maxLabel,showIf:{questionId,value}}
);

-- 응답 헤더 (사용자당 1개)
create table poll_responses (
  id         uuid primary key default gen_random_uuid(),
  created_at timestamptz default now(),
  poll_id    uuid references polls(id) on delete cascade not null,
  user_id    uuid references auth.users(id) not null,
  unique(poll_id, user_id)
);

-- 개별 답변
create table poll_answers (
  id          uuid primary key default gen_random_uuid(),
  response_id uuid references poll_responses(id) on delete cascade not null,
  question_id uuid references poll_questions(id) on delete cascade not null,
  value       jsonb not null default 'null'
);

-- RLS
alter table polls          enable row level security;
alter table poll_questions enable row level security;
alter table poll_responses enable row level security;
alter table poll_answers   enable row level security;

create policy "polls_read"       on polls for select using (true);
create policy "polls_admin"      on polls for all
  using (exists (select 1 from profiles where id = auth.uid() and is_admin = true));

create policy "questions_read"   on poll_questions for select using (true);
create policy "questions_admin"  on poll_questions for all
  using (exists (select 1 from profiles where id = auth.uid() and is_admin = true));

create policy "responses_read"   on poll_responses for select using (auth.uid() is not null);
create policy "responses_insert" on poll_responses for insert with check (user_id = auth.uid());

create policy "answers_read"     on poll_answers for select using (auth.uid() is not null);
create policy "answers_insert"   on poll_answers for insert
  with check (exists (select 1 from poll_responses where id = response_id and user_id = auth.uid()));
