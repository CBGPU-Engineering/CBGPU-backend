-- Run in the Supabase SQL Editor. Existing manager assignments are preserved.
-- profiles_update_self protects is_admin and is_approved, but previously let
-- users assign is_wakeup_manager (and is_laser_cut_manager) to themselves.
-- A trigger protects these columns even if another permissive RLS policy exists.

begin;

create or replace function public.guard_profile_privileged_columns()
returns trigger
language plpgsql
security definer
set search_path = ''
as $$
begin
  -- Trusted server calls and direct SQL maintenance retain their existing
  -- privileges. Browser requests use the authenticator session role.
  if auth.role() = 'service_role' or session_user in ('postgres', 'supabase_admin') then
    return new;
  end if;

  if tg_op = 'INSERT' then
    if not public.is_admin(auth.uid()) and (
      coalesce(new.is_admin, false)
      or new.is_approved is not null
      or coalesce(new.is_wakeup_manager, false)
      or coalesce(new.is_laser_cut_manager, false)
    ) then
      raise exception 'Only an administrator may assign profile privileges'
        using errcode = '42501';
    end if;
  elsif not public.is_admin(auth.uid()) and (
    new.is_admin is distinct from old.is_admin
    or new.is_approved is distinct from old.is_approved
    or new.is_wakeup_manager is distinct from old.is_wakeup_manager
    or new.is_laser_cut_manager is distinct from old.is_laser_cut_manager
  ) then
    raise exception 'Only an administrator may change profile privileges'
      using errcode = '42501';
  end if;

  return new;
end;
$$;

drop trigger if exists guard_profile_privileged_columns on public.profiles;
create trigger guard_profile_privileged_columns
before insert or update on public.profiles
for each row execute function public.guard_profile_privileged_columns();

commit;

-- Review existing is_wakeup_manager=true assignments separately. This fix
-- prevents new self-assignments but cannot determine which old ones were
-- intentionally granted by an administrator.
