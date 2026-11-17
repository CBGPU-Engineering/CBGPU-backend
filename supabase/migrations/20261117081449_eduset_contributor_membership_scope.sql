begin;
-- Device contribution is an INTERNAL verified CBsh capability, not a guest study grant.
-- enabled is a user-controlled opt-in and is never proof of affiliation or a role.
create or replace function public.cbgpu_eduset_contributor_access() returns boolean
language sql stable security definer set search_path='' as $$
 select public.cbgpu_personal_project_identity() and cbgpu_schools.legacy_cbsh_access(auth.uid())
$$;
revoke all on function public.cbgpu_eduset_contributor_access() from public,anon;
grant execute on function public.cbgpu_eduset_contributor_access() to authenticated;

-- A random device credential is intentionally usable without an interactive session,
-- but its owner's CURRENT verified affiliation is checked before every contribution.
create or replace function public.cbgpu_eduset_device_source_allowed(p_key_hash text) returns boolean
language sql stable security definer set search_path='' as $$
 select coalesce(p_key_hash ~ '^[0-9a-f]{64}$',false) and exists(
  select 1 from public.eduset_contributor_devices d
  join public.eduset_contributors c on c.user_id=d.user_id
  where d.key_hash=p_key_hash and d.revoked_at is null and c.enabled
  and cbgpu_schools.legacy_cbsh_access(d.user_id)
 )
$$;
revoke all on function public.cbgpu_eduset_device_source_allowed(text) from public,anon,authenticated;
grant execute on function public.cbgpu_eduset_device_source_allowed(text) to service_role;

drop policy if exists scoped_contributor_enrollment on public.eduset_contributors;
create policy scoped_contributor_enrollment on public.eduset_contributors as restrictive
for insert to authenticated with check(user_id=auth.uid() and public.cbgpu_eduset_contributor_access());
drop policy if exists scoped_contributor_device_enrollment on public.eduset_contributor_devices;
create policy scoped_contributor_device_enrollment on public.eduset_contributor_devices as restrictive
for insert to authenticated with check(user_id=auth.uid() and public.cbgpu_eduset_contributor_access());

do $$declare definition text;updated text;begin
 if to_regprocedure('public.accept_eduset_contribution(text,text,timestamptz)') is null then raise exception 'EDUSET_CONTRIBUTION_FUNCTION_MISSING';end if;
 definition:=pg_get_functiondef('public.accept_eduset_contribution(text,text,timestamptz)'::regprocedure);
 if position('public.cbgpu_eduset_device_source_allowed(p_key_hash)' in definition)=0 then
  updated:=replace(definition,'if v_user_id is null then return false; end if;',E'if v_user_id is null then return false; end if;\n  if not public.cbgpu_eduset_device_source_allowed(p_key_hash) then return false;end if;');
  if updated=definition then raise exception 'EDUSET_CONTRIBUTION_VERSION_MISMATCH';end if;
  execute updated;
 end if;
end$$;
revoke all on function public.accept_eduset_contribution(text,text,timestamptz) from public,anon,authenticated;
grant execute on function public.accept_eduset_contribution(text,text,timestamptz) to service_role;
-- Direct REST administration is still platform-admin-only, now with a live session.
-- Service-role contribution RPCs retain their separate fresh device-owner predicate.
drop policy if exists fresh_eduset_settings_select on public.eduset_settings;
create policy fresh_eduset_settings_select on public.eduset_settings as restrictive for select to authenticated using(public.cbgpu_personal_project_identity() and public.is_admin(auth.uid()));
drop policy if exists fresh_eduset_settings_insert on public.eduset_settings;
create policy fresh_eduset_settings_insert on public.eduset_settings as restrictive for insert to authenticated with check(public.cbgpu_personal_project_identity() and public.is_admin(auth.uid()));
drop policy if exists fresh_eduset_settings_update on public.eduset_settings;
create policy fresh_eduset_settings_update on public.eduset_settings as restrictive for update to authenticated using(public.cbgpu_personal_project_identity() and public.is_admin(auth.uid())) with check(public.cbgpu_personal_project_identity() and public.is_admin(auth.uid()));
drop policy if exists fresh_eduset_settings_delete on public.eduset_settings;
create policy fresh_eduset_settings_delete on public.eduset_settings as restrictive for delete to authenticated using(public.cbgpu_personal_project_identity() and public.is_admin(auth.uid()));
notify pgrst,'reload schema';
commit;
