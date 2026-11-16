-- Account management: auto-create profile row on signup (so email confirmation
-- doesn't block it), used together with the app's new forgot-password and
-- account-deletion flows (those need no schema changes — deletion runs through
-- the Supabase service role, which bypasses RLS, and password reset uses
-- Supabase Auth's built-in email flow).

-- Previously the signup page inserted the profiles row itself right after
-- calling auth.signUp(). That only works when signUp() returns an active
-- session immediately. With email confirmation turned on, signUp() returns no
-- session until the link is clicked, so the client has no authenticated
-- request to satisfy the profiles insert RLS policy and the insert silently
-- fails. A trigger on auth.users runs as the table owner (security definer),
-- so it always succeeds regardless of confirmation state.
create or replace function public.handle_new_user()
returns trigger
language plpgsql
security definer set search_path = public
as $$
begin
  insert into public.profiles (id, full_name, student_id, is_admin, is_approved)
  values (
    new.id,
    new.raw_user_meta_data->>'full_name',
    new.raw_user_meta_data->>'student_id',
    false,
    null
  )
  on conflict (id) do nothing;
  return new;
end;
$$;

drop trigger if exists on_auth_user_created on auth.users;
create trigger on_auth_user_created
  after insert on auth.users
  for each row execute function public.handle_new_user();
