begin;
-- Ordinary JWT claims never outlive their actual session or global suspension.
-- Trusted corpus login membership is a READ-only exception; no sessionless write
-- policy is added for that worker. Existing ownership/per-resource policies still
-- independently apply because every policy below is restrictive.
do $$declare relation text;begin
 foreach relation in array array['profiles','reservations','gpu_ssh_keys','device_tokens','print_jobs','laser_jobs','cpu_projects','research_notes','shared_materials','study_posts','study_members','event_registrations','contest_submissions','contests','event_hosts','event_access_details','event_announcements','event_sessions','contest_teams','contest_team_members','contest_award_settings','contest_award_categories','contest_award_ballots','contest_showcases'] loop
 if to_regclass('public.'||relation) is null then continue;end if;
 if relation<>'profiles' then execute format('drop policy if exists fresh_identity_read on public.%I',relation);execute format('create policy fresh_identity_read on public.%I as restrictive for select to authenticated using((select public.cbgpu_school_read_identity()))',relation);end if;
 execute format('drop policy if exists fresh_identity_insert on public.%I',relation);execute format('create policy fresh_identity_insert on public.%I as restrictive for insert to authenticated with check((select public.cbgpu_personal_project_identity()))',relation);
 execute format('drop policy if exists fresh_identity_update on public.%I',relation);execute format('create policy fresh_identity_update on public.%I as restrictive for update to authenticated using((select public.cbgpu_personal_project_identity())) with check((select public.cbgpu_personal_project_identity()))',relation);
 execute format('drop policy if exists fresh_identity_delete on public.%I',relation);execute format('create policy fresh_identity_delete on public.%I as restrictive for delete to authenticated using((select public.cbgpu_personal_project_identity()))',relation);
 end loop;
end$$;
create or replace function public.cbgpu_private_gpu_reader() returns boolean language sql stable security definer set search_path='' as $$select public.cbgpu_personal_project_identity() and(public.cbgpu_can_use_resource('gpu','cbsh') or public.is_admin(auth.uid()))$$;
create or replace function public.cbgpu_private_fabrication_reader(kind text) returns boolean language sql stable security definer set search_path='' as $$select public.cbgpu_personal_project_identity() and(public.cbgpu_can_use_resource('fabrication','cbsh') or public.is_admin(auth.uid()) or(kind='print' and public.is_3d_manager(auth.uid()))or(kind='laser' and public.is_laser_manager(auth.uid())))$$;
revoke all on function public.cbgpu_private_gpu_reader(),public.cbgpu_private_fabrication_reader(text) from public,anon;
grant execute on function public.cbgpu_private_gpu_reader(),public.cbgpu_private_fabrication_reader(text) to authenticated;
drop policy if exists private_gpu_metadata on public.gpus;create policy private_gpu_metadata on public.gpus as restrictive for select to authenticated using((select public.cbgpu_private_gpu_reader()));
drop policy if exists private_gpu_endpoints on public.gpu_endpoints;create policy private_gpu_endpoints on public.gpu_endpoints as restrictive for select to authenticated using((select public.cbgpu_private_gpu_reader()));
drop policy if exists private_printer_inventory on public.printers;create policy private_printer_inventory on public.printers as restrictive for select to authenticated using((select public.cbgpu_private_fabrication_reader('print')));
drop policy if exists private_print_inventory on public.print_materials;create policy private_print_inventory on public.print_materials as restrictive for select to authenticated using((select public.cbgpu_private_fabrication_reader('print')));
drop policy if exists private_laser_inventory on public.laser_materials;create policy private_laser_inventory on public.laser_materials as restrictive for select to authenticated using((select public.cbgpu_private_fabrication_reader('laser')));
revoke select on public.gpus,public.gpu_endpoints,public.gpu_ssh_keys,public.reservations,public.device_tokens,public.print_jobs,public.laser_jobs,public.printers,public.print_materials,public.laser_materials from anon;
-- Registrations keep their owner's historical record. That does not entitle an
-- expired guest to new/internal contest content or live meeting credentials.
create or replace function cbgpu_schools.contest_read(contest uuid,uid uuid) returns boolean language sql stable security definer set search_path='' as $$select exists(select 1 from public.contests c where c.id=contest and(c.host_id=uid or cbgpu_schools.scope_read(uid,c.school_id,c.access_scope,c.allowed_school_ids) or exists(select 1 from public.event_hosts h where h.contest_id=c.id and h.user_id=uid)))$$;
create or replace function public.cbgpu_private_event_reader(contest uuid) returns boolean language sql stable security definer set search_path='' as $$select public.cbgpu_personal_project_identity() and exists(select 1 from public.contests c where c.id=contest and(c.host_id=auth.uid() or exists(select 1 from public.event_hosts h where h.contest_id=c.id and h.user_id=auth.uid()) or cbgpu_schools.contest_register(c.id,auth.uid())))$$;
revoke all on function public.cbgpu_private_event_reader(uuid) from public,anon;grant execute on function public.cbgpu_private_event_reader(uuid) to authenticated;
drop policy if exists private_event_access_scope on public.event_access_details;create policy private_event_access_scope on public.event_access_details as restrictive for select to authenticated using(public.cbgpu_private_event_reader(contest_id));
drop policy if exists private_event_announcement_scope on public.event_announcements;create policy private_event_announcement_scope on public.event_announcements as restrictive for select to authenticated using(public.cbgpu_private_event_reader(contest_id));
drop policy if exists private_event_session_scope on public.event_sessions;create policy private_event_session_scope on public.event_sessions as restrictive for select to authenticated using(public.cbgpu_contest_visible(contest_id));
-- Own basic identity remains available to show a suspended user their account
-- status; it grants no project/resource mutation, manager role, or peer data.
create or replace function public.cbgpu_profile_visible(p_user uuid) returns boolean language sql stable security definer set search_path='' as $$select(p_user=auth.uid() and cbgpu_accounts.current_session_valid() and exists(select 1 from auth.users u where u.id=auth.uid() and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now()))) or(cbgpu_schools.read_identity_allowed() and(p_user=auth.uid() or exists(select 1 from public.profiles actor where actor.id=auth.uid() and actor.is_admin)))$$;
commit;
