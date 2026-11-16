-- ============================================================
-- CBGPU 연구노트 + 스터디 매칭 마이그레이션
-- ============================================================

-- 연구 노트
create table if not exists research_notes (
  id         uuid primary key default gen_random_uuid(),
  created_at timestamptz default now(),
  updated_at timestamptz default now(),
  user_id    uuid references auth.users(id) not null,
  title      text not null default '제목 없음',
  content    text not null default '',
  
  tags       text[] default '{}',
  is_shared  boolean default false
);
alter table research_notes enable row level security;
create policy "notes_own"    on research_notes for all  using (user_id = auth.uid());
create policy "notes_shared" on research_notes for select using (is_shared = true);

-- 스터디 그룹
create table if not exists study_posts (
  id          uuid primary key default gen_random_uuid(),
  created_at  timestamptz default now(),
  user_id     uuid references auth.users(id) not null,
  subject     text not null,
  title       text not null,
  description text not null default '',
  tags        text[] default '{}',
  max_members int not null default 4,
  is_open     boolean default true
);
alter table study_posts enable row level security;
create policy "study_read"   on study_posts for select using (true);
create policy "study_write"  on study_posts for all    using (user_id = auth.uid());

-- 스터디 멤버
create table if not exists study_members (
  post_id    uuid references study_posts(id) on delete cascade,
  user_id    uuid references auth.users(id) not null,
  joined_at  timestamptz default now(),
  primary key(post_id, user_id)
);
alter table study_members enable row level security;
create policy "member_read"   on study_members for select using (true);
create policy "member_insert" on study_members for insert with check (user_id = auth.uid());
create policy "member_delete" on study_members for delete using (user_id = auth.uid());
