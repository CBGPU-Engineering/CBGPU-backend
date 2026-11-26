begin;
-- Default avatars are decided by the database so every client (web, mobile, services)
-- shows the same picture.
--  * avatar_seed is a random, opaque, immutable per-account UUID. It is never derived
--    from a name, e-mail, student number or school, so no personal data reaches the
--    avatar provider.
--  * avatar_settings is the single place for provider, version, style, format and size.
--  * Uploaded photos (profiles.avatar_url) win; clearing one falls back to the default.

create table if not exists public.avatar_settings(
 singleton boolean primary key default true check(singleton),
 base_url text not null default 'https://api.dicebear.com'
  check(base_url ~ '^https://[a-z0-9]([a-z0-9.-]*[a-z0-9])?(:[0-9]{1,5})?(/[A-Za-z0-9._~-]+)*$'),
 version text not null default '9.x' check(version ~ '^[0-9]{1,3}\.x$'),
 style text not null default 'thumbs' check(style ~ '^[a-z][a-z0-9-]{0,39}$'),
 format text not null default 'png' check(format in('png','svg','jpg','webp','avif')),
 size integer not null default 256 check(size between 16 and 512),
 updated_at timestamptz not null default now()
);
insert into public.avatar_settings(singleton) values(true) on conflict do nothing;
alter table public.avatar_settings enable row level security;
-- No policy: only the definer function below (and service_role) can read it.
revoke all on public.avatar_settings from public,anon,authenticated;

-- ADD COLUMN with a volatile default rewrites the table and gives every existing row its
-- own value without firing row triggers, so existing users are migrated in this statement.
alter table public.profiles add column if not exists avatar_seed uuid not null default gen_random_uuid();
create unique index if not exists profiles_avatar_seed_key on public.profiles(avatar_seed);

create or replace function public.cbgpu_profiles_avatar_seed_guard() returns trigger
language plpgsql set search_path='' as $$
begin
 if tg_op='INSERT' then
  -- Client roles can never pick a seed; trusted roles (restores, migrations) may.
  if current_user in('anon','authenticated') or new.avatar_seed is null then new.avatar_seed:=gen_random_uuid(); end if;
 elsif new.avatar_seed is distinct from old.avatar_seed then
  raise exception 'AVATAR_SEED_IMMUTABLE' using errcode='42501';
 end if;
 return new;
end$$;
drop trigger if exists cbgpu_profiles_avatar_seed_guard on public.profiles;
create trigger cbgpu_profiles_avatar_seed_guard before insert or update of avatar_seed on public.profiles
 for each row execute function public.cbgpu_profiles_avatar_seed_guard();

create or replace function public.cbgpu_default_avatar_url(seed uuid) returns text
language sql stable security definer set search_path='' as $$
 select s.base_url||'/'||s.version||'/'||s.style||'/'||s.format||'?seed='||seed::text||'&size='||s.size::text
 from public.avatar_settings s where s.singleton
$$;

-- PostgREST computed columns: select=id,profile_image_url,display_avatar_url.
-- The default URL is deliberately not exposed on its own: clients render avatarUrl and never build
-- avatar-provider URLs themselves, so style/version cannot drift between clients.
create or replace function public.profile_image_url(p public.profiles) returns text
language sql stable set search_path='' as $$ select nullif(btrim(p.avatar_url),'') $$;
create or replace function public.display_avatar_url(p public.profiles) returns text
language sql stable set search_path='' as $$
 select coalesce(nullif(btrim(p.avatar_url),''),public.cbgpu_default_avatar_url(p.avatar_seed))
$$;

-- Canonical user card: {id,name,avatarUrl,profileImageUrl}. avatarUrl is always present. Runs with the caller's
-- privileges, so profile visibility (RLS, including school boundaries) is unchanged.
create or replace view public.user_cards with(security_invoker=true) as
 select p.id,p.full_name as name,
  coalesce(nullif(btrim(p.avatar_url),''),public.cbgpu_default_avatar_url(p.avatar_seed)) as "avatarUrl",
  nullif(btrim(p.avatar_url),'') as "profileImageUrl"
 from public.profiles p;

-- School-scoped screens only get names through cbgpu_study_member_profiles (its visibility rules are
-- unchanged). This wrapper asks that function who is visible and only adds the avatar fields.
do $$ begin
 if to_regprocedure('public.cbgpu_study_member_profiles(uuid[])') is not null then
  create or replace function public.cbgpu_study_member_cards(p_user_ids uuid[])
  returns table(id uuid,name text,"avatarUrl" text,"profileImageUrl" text)
  language sql stable security definer set search_path='' as $f$
   select m.id,m.full_name,
    coalesce(nullif(btrim(p.avatar_url),''),public.cbgpu_default_avatar_url(p.avatar_seed)),
    nullif(btrim(p.avatar_url),'')
   from public.cbgpu_study_member_profiles(p_user_ids) m join public.profiles p on p.id=m.id
  $f$;
  revoke all on function public.cbgpu_study_member_cards(uuid[]) from public,anon;
  grant execute on function public.cbgpu_study_member_cards(uuid[]) to authenticated,service_role;
 end if;
end $$;

revoke all on function public.cbgpu_default_avatar_url(uuid),public.profile_image_url(public.profiles),
 public.display_avatar_url(public.profiles) from public,anon;
grant execute on function public.cbgpu_default_avatar_url(uuid),public.profile_image_url(public.profiles),
 public.display_avatar_url(public.profiles) to authenticated,service_role;
revoke all on public.user_cards from public,anon;
grant select on public.user_cards to authenticated,service_role;
commit;
