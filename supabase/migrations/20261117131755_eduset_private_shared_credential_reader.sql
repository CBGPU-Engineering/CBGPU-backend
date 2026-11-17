-- A legacy PERMISSIVE SELECT TO PUBLIC policy predates scoped accounts.
-- Authenticated administrator restrictions do not apply to role anon. Keep
-- caller policy evaluation intact and close this separate anonymous path.
-- Private service-role workers use their existing server-only authority.
begin;
drop policy if exists fresh_eduset_settings_anon_select on public.eduset_settings;
create policy fresh_eduset_settings_anon_select on public.eduset_settings
 as restrictive for select to anon using(false);
notify pgrst, 'reload schema';
commit;
