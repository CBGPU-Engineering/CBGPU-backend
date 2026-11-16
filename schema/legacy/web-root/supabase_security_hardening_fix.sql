-- ============================================================
-- 긴급 수정: supabase_security_hardening_migration.sql의 RLS 정책이
-- profiles 테이블을 자기 자신 안에서 다시 조회해서(is_admin 체크) 무한
-- 재귀를 일으킵니다 ("infinite recursion detected in policy for relation
-- profiles"). 이 스크립트를 Supabase SQL Editor에서 그대로 실행하면
-- 즉시 해결됩니다. (원본 마이그레이션 파일도 같은 내용으로 갱신해둠)
-- ============================================================

-- RLS를 우회하는 함수로 is_admin/is_approved를 조회한다 — SECURITY DEFINER라
-- 함수 내부 쿼리는 정책 재귀를 타지 않는다. profiles 관련 정책은 전부 이 함수를
-- 통해서만 is_admin을 확인해야 한다.
create or replace function public.is_admin(uid uuid)
returns boolean
language sql
stable
security definer
set search_path = public
as $$
  select coalesce((select p.is_admin from public.profiles p where p.id = uid), false);
$$;

create or replace function public.get_is_approved(uid uuid)
returns boolean
language sql
stable
security definer
set search_path = public
as $$
  select (select p.is_approved from public.profiles p where p.id = uid);
$$;

-- ── profiles: 재귀 없이 is_admin()/get_is_approved() 함수로 다시 정의 ──────
drop policy if exists "profiles_update_self" on profiles;
create policy "profiles_update_self" on profiles for update
  using (auth.uid() = id)
  with check (
    auth.uid() = id
    and is_admin    is not distinct from public.is_admin(auth.uid())
    and is_approved is not distinct from public.get_is_approved(auth.uid())
  );

drop policy if exists "profiles_admin_all" on profiles;
create policy "profiles_admin_all" on profiles for all
  using (public.is_admin(auth.uid()));

drop policy if exists "profiles_insert_self" on profiles;
create policy "profiles_insert_self" on profiles for insert
  with check (auth.uid() = id and is_admin = false and is_approved is null);

-- ── cpu_projects: 동일하게 is_admin() 함수로 교체 ──────────────────────────
drop policy if exists "cpu_projects_owner_update" on cpu_projects;
create policy "cpu_projects_owner_update" on cpu_projects for update
  using (user_id = auth.uid() or public.is_admin(auth.uid()));

drop policy if exists "cpu_projects_owner_delete" on cpu_projects;
create policy "cpu_projects_owner_delete" on cpu_projects for delete
  using (user_id = auth.uid() or public.is_admin(auth.uid()));

-- ── gpus: 동일하게 is_admin() 함수로 교체 ──────────────────────────────────
drop policy if exists "gpus_admin_select" on gpus;
create policy "gpus_admin_select" on gpus for select
  using (public.is_admin(auth.uid()));

drop policy if exists "gpus_admin_write" on gpus;
create policy "gpus_admin_write" on gpus for all
  using (public.is_admin(auth.uid()));
