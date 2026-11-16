-- A user's public key is the only SSH credential stored by the web app.
create table if not exists public.gpu_ssh_keys (
  user_id uuid primary key references auth.users(id) on delete cascade,
  public_key text not null,
  updated_at timestamptz not null default now(),
  constraint gpu_ssh_keys_ed25519 check (
    public_key ~ '^ssh-ed25519 [A-Za-z0-9+/]+={0,2}( [^\r\n]*)?$'
    and length(public_key) between 80 and 500
  )
);

alter table public.gpu_ssh_keys enable row level security;
revoke all on public.gpu_ssh_keys from anon;
grant select, insert, update, delete on public.gpu_ssh_keys to authenticated;
grant all on public.gpu_ssh_keys to service_role;

drop policy if exists "Users read own GPU SSH key" on public.gpu_ssh_keys;
create policy "Users read own GPU SSH key" on public.gpu_ssh_keys
  for select to authenticated using ((select auth.uid()) = user_id);
drop policy if exists "Users insert own GPU SSH key" on public.gpu_ssh_keys;
create policy "Users insert own GPU SSH key" on public.gpu_ssh_keys
  for insert to authenticated with check ((select auth.uid()) = user_id);
drop policy if exists "Users update own GPU SSH key" on public.gpu_ssh_keys;
create policy "Users update own GPU SSH key" on public.gpu_ssh_keys
  for update to authenticated using ((select auth.uid()) = user_id)
  with check ((select auth.uid()) = user_id);
drop policy if exists "Users delete own GPU SSH key" on public.gpu_ssh_keys;
create policy "Users delete own GPU SSH key" on public.gpu_ssh_keys
  for delete to authenticated using ((select auth.uid()) = user_id);
