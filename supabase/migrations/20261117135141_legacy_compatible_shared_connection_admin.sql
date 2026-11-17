begin;
-- Standalone-compatible companion to the anonymous-denial hotfix. This uses
-- only existing identity/session tables and does not require account-v2 DDL.
create or replace function public.cbgpu_shared_connection_admin()
returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from public.profiles p join auth.users u on u.id=p.id
 join auth.sessions s on s.user_id=p.id
 where p.id=auth.uid() and p.is_admin
 and coalesce(to_jsonb(p)->>'account_status','ACTIVE')<>'SUSPENDED'
 and u.deleted_at is null and (u.banned_until is null or u.banned_until<=now())
 and s.id::text=auth.jwt()->>'session_id' and (s.not_after is null or s.not_after>now()))
$$;
revoke all on function public.cbgpu_shared_connection_admin() from public,anon,authenticated;
grant execute on function public.cbgpu_shared_connection_admin() to authenticated;
drop policy if exists shared_connection_admin_read on public.eduset_settings;
create policy shared_connection_admin_read on public.eduset_settings as restrictive for select to authenticated
 using(public.cbgpu_shared_connection_admin());
drop policy if exists shared_connection_admin_insert on public.eduset_settings;
create policy shared_connection_admin_insert on public.eduset_settings as restrictive for insert to authenticated
 with check(public.cbgpu_shared_connection_admin());
drop policy if exists shared_connection_admin_update on public.eduset_settings;
create policy shared_connection_admin_update on public.eduset_settings as restrictive for update to authenticated
 using(public.cbgpu_shared_connection_admin()) with check(public.cbgpu_shared_connection_admin());
drop policy if exists shared_connection_admin_delete on public.eduset_settings;
create policy shared_connection_admin_delete on public.eduset_settings as restrictive for delete to authenticated
 using(public.cbgpu_shared_connection_admin());
notify pgrst,'reload schema';
commit;
