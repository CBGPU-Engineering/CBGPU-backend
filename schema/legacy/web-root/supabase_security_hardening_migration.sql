-- ============================================================
-- CBGPU 보안 강화 마이그레이션
-- Supabase SQL Editor에서 실행하세요.
--
-- ⚠️ 실행 전에: Database → Tables → (profiles / cpu_projects / gpus) →
-- Policies 탭에서 기존에 만들어둔 정책 이름을 먼저 확인하세요. 이 파일은
-- 코드에서 실제로 쓰는 이름들만 추측해서 "if exists"로 지우지만, 다른 이름의
-- 기존 정책이 남아있으면 이 마이그레이션과 충돌하거나 무력화될 수 있습니다.
-- 특히 profiles/cpu_projects/gpus에 "select/update using (true)" 같은
-- 전체 허용 정책이 이미 있다면 반드시 지우고 아래 정책으로 교체하세요.
-- ============================================================

-- ── 1. profiles: 본인 프로필은 수정 가능하되, is_admin/is_approved는
--    본인이 스스로 바꿀 수 없게 한다. 클라이언트(anon key)에서 직접
--    supabase.from('profiles').update({is_admin:true})를 호출해도
--    이 체크에 걸려서 거부된다. ────────────────────────────────────
alter table profiles enable row level security;

-- profiles를 보호하는 정책 안에서 profiles를 다시 select하면(설령 서브쿼리라도)
-- Postgres가 "infinite recursion detected in policy for relation profiles" 에러를
-- 낸다. 그래서 is_admin/is_approved 조회는 SECURITY DEFINER 함수로 감싸서 RLS를
-- 우회한다 — 이 함수들은 profiles/cpu_projects/gpus 정책 전체에서 재사용한다.
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

-- is_admin/is_approved는 null일 수 있으므로 "=" 대신 "is not distinct from"으로 비교한다
-- (null = null은 SQL에서 unknown이라 "="만 쓰면 null 값을 가진 본인 행도 update가 막힌다).
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

-- 회원가입 직후 본인 행 삽입은 허용하되, is_admin은 항상 false, is_approved는 항상 null로
-- 강제한다. null/true는 정상 이용 가능, 관리자가 학번/이름이 이상하다고 판단해 false로
-- 내리면 계정은 살아있지만 예약/서버 배포 같은 핵심 기능만 막힌다 — ApprovalNotice 참고.
drop policy if exists "profiles_insert_self" on profiles;
create policy "profiles_insert_self" on profiles for insert
  with check (auth.uid() = id and is_admin = false and is_approved is null);

-- ── 2. cpu_projects: 소유자 또는 관리자만 수정/삭제 가능. UI에서만
--    막던 것을 실제 DB 레벨에서도 강제한다. ──────────────────────────
alter table cpu_projects enable row level security;

drop policy if exists "cpu_projects_owner_update" on cpu_projects;
create policy "cpu_projects_owner_update" on cpu_projects for update
  using (user_id = auth.uid() or public.is_admin(auth.uid()));

drop policy if exists "cpu_projects_owner_delete" on cpu_projects;
create policy "cpu_projects_owner_delete" on cpu_projects for delete
  using (user_id = auth.uid() or public.is_admin(auth.uid()));

-- ── 3. gpus: password 컬럼이 있는 테이블이므로 직접 SELECT는 관리자만
--    가능하게 막고, 일반 사용자는 자신이 승인받은 예약의 GPU 비밀번호만
--    돌려주는 함수를 통해서만 조회한다. (프론트엔드는 이미 이 함수를
--    쓰도록 수정됨: get_active_gpu_password) ─────────────────────────
alter table gpus enable row level security;

drop policy if exists "gpus_select_all" on gpus;
drop policy if exists "gpus_admin_select" on gpus;
create policy "gpus_admin_select" on gpus for select
  using (public.is_admin(auth.uid()));

drop policy if exists "gpus_admin_write" on gpus;
create policy "gpus_admin_write" on gpus for all
  using (public.is_admin(auth.uid()));

create or replace function get_active_gpu_password(p_gpu_id int)
returns text
language plpgsql
security definer
set search_path = public
as $$
declare
  v_password text;
begin
  if not exists (
    select 1 from reservations r
    where r.gpu_id = p_gpu_id
      and r.user_id = auth.uid()
      and r.status = 'approved'
  ) then
    return null;
  end if;

  select password into v_password from gpus where id = p_gpu_id;
  return v_password;
end;
$$;

grant execute on function get_active_gpu_password(int) to authenticated;
