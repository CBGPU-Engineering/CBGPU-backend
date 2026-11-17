begin;
-- School management permission is distinct from resource consumption. An
-- approved external school owner may organize an event without becoming a
-- student, entering a student-only event, or acquiring GPU access.
create or replace function public.cbgpu_can_create_school_contest(school text)
returns boolean language sql stable security definer set search_path='' as $$
 select cbgpu_accounts.current_session_valid() and cbgpu_schools.feature_enabled(school,'contests')
 and (cbgpu_accounts.can_user_use_resource(auth.uid(),'contests',school)
 or cbgpu_schools.has_role(auth.uid(),school,array['SCHOOL_OWNER','SCHOOL_ADMIN']))
$$;
revoke all on function public.cbgpu_can_create_school_contest(text) from public,anon;
grant execute on function public.cbgpu_can_create_school_contest(text) to authenticated;
create or replace function public.cbgpu_can_create_contest()
returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from cbgpu_schools.memberships m where m.user_id=auth.uid()
 and public.cbgpu_can_create_school_contest(m.school_id))
$$;
revoke all on function public.cbgpu_can_create_contest() from public,anon;
grant execute on function public.cbgpu_can_create_contest() to authenticated;
drop policy if exists multischool_contest_insert on public.contests;
create policy multischool_contest_insert on public.contests as restrictive for insert to authenticated
 with check(public.cbgpu_school_scope_valid(school_id,access_scope,allowed_school_ids)
 and public.cbgpu_can_create_school_contest(school_id));
commit;
