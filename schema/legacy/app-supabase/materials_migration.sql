-- Metadata lives in Supabase; file contents live on the CBGPU server.
create table if not exists public.shared_materials (
  id uuid primary key default gen_random_uuid(),
  uploader_id uuid not null references public.profiles(id) on delete cascade,
  title text not null check (char_length(btrim(title)) between 1 and 120),
  tags text[] not null default '{}',
  file_name text not null check (char_length(file_name) between 1 and 255),
  content_type text not null default 'application/octet-stream',
  size_bytes bigint not null check (size_bytes between 1 and 1073741824),
  status text not null default 'pending' check (status in ('pending', 'ready')),
  created_at timestamptz not null default now(),
  constraint shared_materials_tags_limit check (cardinality(tags) <= 8)
);

create index if not exists shared_materials_recent_idx on public.shared_materials (created_at desc);
create index if not exists shared_materials_tags_idx on public.shared_materials using gin (tags);
alter table public.shared_materials enable row level security;

drop policy if exists "students_read_ready_materials" on public.shared_materials;
create policy "students_read_ready_materials" on public.shared_materials
  for select to authenticated using (
    (status = 'ready' or uploader_id = (select auth.uid()))
    and exists (select 1 from public.profiles where id = (select auth.uid()) and is_approved is distinct from false)
  );

drop policy if exists "students_create_own_materials" on public.shared_materials;
create policy "students_create_own_materials" on public.shared_materials
  for insert to authenticated with check (
    uploader_id = (select auth.uid()) and status = 'pending'
    and exists (select 1 from public.profiles where id = (select auth.uid()) and is_approved is distinct from false)
  );

drop policy if exists "students_finish_own_materials" on public.shared_materials;
create policy "students_finish_own_materials" on public.shared_materials
  for update to authenticated using (uploader_id = (select auth.uid()) and status = 'pending')
  with check (uploader_id = (select auth.uid()) and status = 'ready');

drop policy if exists "students_delete_own_materials" on public.shared_materials;
create policy "students_delete_own_materials" on public.shared_materials
  for delete to authenticated using (uploader_id = (select auth.uid()));

-- 업로드 한도를 200MB에서 1GB로 올린다. 이미 테이블이 만들어진 DB에서는 위 create table이
-- 다시 실행돼도 제약이 바뀌지 않으므로 제약을 직접 교체한다.
alter table public.shared_materials drop constraint if exists shared_materials_size_bytes_check;
alter table public.shared_materials add constraint shared_materials_size_bytes_check
  check (size_bytes between 1 and 1073741824);
