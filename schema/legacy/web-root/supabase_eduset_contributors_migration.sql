-- edu-set 토큰 기여자/확장 프로그램 연결
-- CBGPU 로그인 사용자는 별도 관리자 승인 없이 직접 참여할 수 있다.

alter table public.eduset_settings
  add column if not exists expires_at timestamptz,
  add column if not exists contributor_id uuid references auth.users(id);

create table if not exists public.eduset_contributors (
  user_id uuid primary key references auth.users(id) on delete cascade,
  joined_at timestamptz not null default now(),
  enabled boolean not null default true,
  last_contributed_at timestamptz
);

create table if not exists public.eduset_contributor_devices (
  id uuid primary key default gen_random_uuid(),
  user_id uuid not null references public.eduset_contributors(user_id) on delete cascade,
  key_hash text not null unique,
  name text not null default 'Chrome 확장 프로그램',
  created_at timestamptz not null default now(),
  last_used_at timestamptz,
  revoked_at timestamptz
);

alter table public.eduset_contributors enable row level security;
alter table public.eduset_contributor_devices enable row level security;

create policy "contributors_self_read" on public.eduset_contributors for select
  using (auth.uid() = user_id or public.is_admin(auth.uid()));
create policy "contributors_self_join" on public.eduset_contributors for insert
  with check (auth.uid() = user_id and enabled = true);
create policy "contributors_self_update" on public.eduset_contributors for update
  using (auth.uid() = user_id or public.is_admin(auth.uid()))
  with check (auth.uid() = user_id or public.is_admin(auth.uid()));
create policy "contributors_self_leave" on public.eduset_contributors for delete
  using (auth.uid() = user_id or public.is_admin(auth.uid()));

create policy "contributor_devices_self_read" on public.eduset_contributor_devices for select
  using (auth.uid() = user_id or public.is_admin(auth.uid()));
create policy "contributor_devices_self_insert" on public.eduset_contributor_devices for insert
  with check (auth.uid() = user_id);
create policy "contributor_devices_self_delete" on public.eduset_contributor_devices for delete
  using (auth.uid() = user_id or public.is_admin(auth.uid()));

-- 토큰 채택은 SUPABASE_SERVICE_ROLE_KEY를 가진 CBGPU 서버에서만 수행한다.
-- 클라이언트/확장 프로그램은 이 테이블들을 직접 수정할 수 없다.
create or replace function public.accept_eduset_contribution(
  p_key_hash text, p_token_enc text, p_expires_at timestamptz
) returns boolean
language plpgsql security definer set search_path = public
as $$
declare v_device_id uuid; v_user_id uuid;
begin
  select d.id, d.user_id into v_device_id, v_user_id
  from public.eduset_contributor_devices d
  join public.eduset_contributors c on c.user_id = d.user_id
  where d.key_hash = p_key_hash and d.revoked_at is null and c.enabled = true;
  if v_user_id is null then return false; end if;

  update public.eduset_contributor_devices set last_used_at = now() where id = v_device_id;
  update public.eduset_contributors set last_contributed_at = now() where user_id = v_user_id;
  insert into public.eduset_settings(id, access_token_enc, updated_at, updated_by, expires_at, contributor_id)
  values (1, p_token_enc, now(), v_user_id, p_expires_at, v_user_id)
  on conflict (id) do update set
    access_token_enc = excluded.access_token_enc, updated_at = excluded.updated_at,
    updated_by = excluded.updated_by, expires_at = excluded.expires_at,
    contributor_id = excluded.contributor_id
  where public.eduset_settings.expires_at is null or excluded.expires_at > public.eduset_settings.expires_at;
  return true;
end;
$$;
revoke all on function public.accept_eduset_contribution(text, text, timestamptz) from public, anon, authenticated;
grant execute on function public.accept_eduset_contribution(text, text, timestamptz) to service_role;
