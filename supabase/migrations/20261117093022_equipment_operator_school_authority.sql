begin;
-- Assignment is a role, not affiliation. Temporary fabrication grants authorize
-- requesting equipment, never exercising a previous operator assignment.
create or replace function cbgpu_schools.equipment_manager_valid(subject uuid,kind text,printer integer default null) returns boolean language sql stable security definer set search_path='' as $$
select cbgpu_schools.legacy_cbsh_access(subject) and cbgpu_accounts.can_user_use_resource(subject,'fabrication','cbsh') and
 case kind when 'print' then exists(select 1 from public.printer_managers pm where pm.user_id=subject and(printer is null or pm.printer_id=printer))
 when 'laser' then exists(select 1 from public.profiles p where p.id=subject and p.is_laser_cut_manager) else false end
$$;
revoke all on function cbgpu_schools.equipment_manager_valid(uuid,text,integer) from public,anon,authenticated;
create or replace function public.cbgpu_equipment_operator(kind text,printer integer default null) returns boolean language sql stable security definer set search_path='' as $$
select public.cbgpu_personal_project_identity() and cbgpu_schools.equipment_manager_valid(auth.uid(),kind,printer)
$$;
-- Only use this at surfaces which already explicitly allowed platform oversight.
create or replace function public.cbgpu_equipment_platform_oversight() returns boolean language sql stable security definer set search_path='' as $$select public.cbgpu_personal_project_identity() and public.is_admin(auth.uid())$$;
revoke all on function public.cbgpu_equipment_operator(text,integer),public.cbgpu_equipment_platform_oversight() from public,anon;
grant execute on function public.cbgpu_equipment_operator(text,integer),public.cbgpu_equipment_platform_oversight() to authenticated;
create or replace function public.cbgpu_private_fabrication_reader(kind text) returns boolean language sql stable security definer set search_path='' as $$select public.cbgpu_personal_project_identity() and(public.cbgpu_can_use_resource('fabrication','cbsh') or public.cbgpu_equipment_platform_oversight() or public.cbgpu_equipment_operator(kind,null))$$;
-- Boolean-only eligibility for an authorized requester's chosen manager. This
-- is not an operator grant, profile projection, or school-wide affiliation.
create or replace function public.cbgpu_equipment_manager_eligible(subject uuid,kind text,printer integer default null) returns boolean language sql stable security definer set search_path='' as $$select public.cbgpu_personal_project_identity() and(public.cbgpu_can_use_resource('fabrication','cbsh') or public.cbgpu_equipment_platform_oversight()) and cbgpu_schools.equipment_manager_valid(subject,kind,printer)$$;
revoke all on function public.cbgpu_equipment_manager_eligible(uuid,text,integer) from public,anon;
grant execute on function public.cbgpu_equipment_manager_eligible(uuid,text,integer) to authenticated;
-- AND existing permissive ownership/assignment policies; these add no new admin
-- surface or role. In particular a guest's old manager flag never opens a queue.
drop policy if exists account_v2_equipment_read on public.print_jobs;
create policy account_v2_equipment_read on public.print_jobs as restrictive for select to authenticated using(user_id=auth.uid() or public.cbgpu_equipment_operator('print',coalesce(printer_id,preferred_printer_id)) or public.cbgpu_equipment_platform_oversight());
drop policy if exists account_v2_equipment_read on public.laser_jobs;
create policy account_v2_equipment_read on public.laser_jobs as restrictive for select to authenticated using(user_id=auth.uid() or public.cbgpu_equipment_operator('laser',null) or public.cbgpu_equipment_platform_oversight());
drop policy if exists scoped_equipment_update on public.print_jobs;
create policy scoped_equipment_update on public.print_jobs as restrictive for update to authenticated using(user_id=auth.uid() or public.cbgpu_equipment_operator('print',coalesce(printer_id,preferred_printer_id)) or public.cbgpu_equipment_platform_oversight()) with check(user_id=auth.uid() or public.cbgpu_equipment_operator('print',coalesce(printer_id,preferred_printer_id)) or public.cbgpu_equipment_platform_oversight());
drop policy if exists scoped_equipment_update on public.laser_jobs;
create policy scoped_equipment_update on public.laser_jobs as restrictive for update to authenticated using(user_id=auth.uid() or public.cbgpu_equipment_operator('laser',null)) with check(user_id=auth.uid() or public.cbgpu_equipment_operator('laser',null));
drop policy if exists scoped_printer_update on public.printers;
create policy scoped_printer_update on public.printers as restrictive for update to authenticated using(public.cbgpu_equipment_operator('print',id)) with check(public.cbgpu_equipment_operator('print',id));
-- Inventory reads remain available to authorized requesters. Writes require a
-- current internal operator, AND the original manager policies.
drop policy if exists scoped_print_material_write on public.print_materials;
create policy scoped_print_material_write on public.print_materials as restrictive for all to authenticated using(public.cbgpu_private_fabrication_reader('print')) with check(public.cbgpu_equipment_operator('print',null));
drop policy if exists scoped_laser_material_write on public.laser_materials;
create policy scoped_laser_material_write on public.laser_materials as restrictive for all to authenticated using(public.cbgpu_private_fabrication_reader('laser')) with check(public.cbgpu_equipment_operator('laser',null));
-- DELETE has no WITH CHECK. Do not let read permission become inventory removal.
drop policy if exists scoped_print_material_delete on public.print_materials;
create policy scoped_print_material_delete on public.print_materials as restrictive for delete to authenticated using(public.cbgpu_equipment_operator('print',null));
drop policy if exists scoped_laser_material_delete on public.laser_materials;
create policy scoped_laser_material_delete on public.laser_materials as restrictive for delete to authenticated using(public.cbgpu_equipment_operator('laser',null));
create or replace function cbgpu_schools.guard_equipment_job() returns trigger language plpgsql set search_path='' as $$
declare actor uuid:=auth.uid();kind text:=case tg_table_name when 'print_jobs' then 'print' else 'laser' end;old_printer integer;new_printer integer;
begin
 -- Effective invocation role, never session_user: clone tests deliberately SET
 -- ROLE authenticated while their connection owner is a bootstrap superuser.
 if current_user in('postgres','supabase_admin') or exists(select 1 from pg_catalog.pg_roles r where r.rolname=current_user and r.rolsuper) or(current_user='service_role' and auth.role()='service_role') then return new;end if;
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if kind='print' then new_printer:=coalesce((to_jsonb(new)->>'printer_id')::integer,(to_jsonb(new)->>'preferred_printer_id')::integer);end if;
 if tg_op='INSERT' then
  if new.user_id is distinct from actor or not public.cbgpu_can_use_resource('fabrication','cbsh') or new.status is distinct from 'pending' or new.completed_at is not null or new.delivered_at is not null or new.reject_reason is not null then raise exception 'EQUIPMENT_REQUEST_ONLY' using errcode='42501';end if;
  if kind='print' and((to_jsonb(new)->>'printer_id') is not null or(to_jsonb(new)->>'started_at') is not null or(to_jsonb(new)->>'estimated_end_at') is not null or(to_jsonb(new)->>'collected_at') is not null or coalesce((to_jsonb(new)->>'plates_completed')::integer,0)<>0) then raise exception 'EQUIPMENT_REQUEST_ONLY' using errcode='42501';end if;
  if new.manager_id is not null and not public.cbgpu_equipment_manager_eligible(new.manager_id,kind,new_printer) then raise exception 'INVALID_EQUIPMENT_MANAGER' using errcode='42501';end if;
  return new;
 end if;
 if new.id is distinct from old.id or new.user_id is distinct from old.user_id or new.created_at is distinct from old.created_at then raise exception 'EQUIPMENT_OWNER_IMMUTABLE' using errcode='42501';end if;
 if kind='print' then old_printer:=coalesce((to_jsonb(old)->>'printer_id')::integer,(to_jsonb(old)->>'preferred_printer_id')::integer);end if;
 if (kind='print' and public.cbgpu_equipment_platform_oversight()) or(public.cbgpu_equipment_operator(kind,old_printer) and public.cbgpu_equipment_operator(kind,new_printer)) then
  if new.manager_id is distinct from old.manager_id and new.manager_id is not null and not public.cbgpu_equipment_manager_eligible(new.manager_id,kind,new_printer) then raise exception 'INVALID_EQUIPMENT_MANAGER' using errcode='42501';end if;
  return new;
 end if;
 -- The actual requester UI supports pending cancellation only. History/export
 -- and file ownership remain separate from current school resource eligibility.
 if old.user_id=actor and(to_jsonb(new)-'status')=(to_jsonb(old)-'status') and(new.status=old.status or(old.status='pending' and new.status='canceled')) then return new;end if;
 raise exception 'EQUIPMENT_OPERATOR_REQUIRED' using errcode='42501';
end$$;
-- Trigger functions are not public RPCs; they execute as the invocation role.
revoke all on function cbgpu_schools.guard_equipment_job() from public,anon,authenticated;
drop trigger if exists zz_scoped_equipment_job on public.print_jobs;
create trigger zz_scoped_equipment_job before insert or update on public.print_jobs for each row execute function cbgpu_schools.guard_equipment_job();
drop trigger if exists zz_scoped_equipment_job on public.laser_jobs;
create trigger zz_scoped_equipment_job before insert or update on public.laser_jobs for each row execute function cbgpu_schools.guard_equipment_job();
-- Keep private file access aligned with the exact job/printer, never merely an
-- old global manager bit. Orphaned and historical own files remain accessible.
drop policy if exists scoped_equipment_file_read on storage.objects;
create policy scoped_equipment_file_read on storage.objects as restrictive for select to authenticated using(bucket_id<>'manufacturing' or(public.cbgpu_personal_project_identity() and((storage.foldername(name))[1]=auth.uid()::text or exists(select 1 from public.print_jobs j where j.file_path=name and(public.cbgpu_equipment_operator('print',coalesce(j.printer_id,j.preferred_printer_id)) or public.cbgpu_equipment_platform_oversight())) or exists(select 1 from public.laser_jobs j where j.file_path=name and(public.cbgpu_equipment_operator('laser',null) or public.cbgpu_equipment_platform_oversight())))));
-- Explicit platform oversight is constrained by job SELECT RLS and therefore
-- adds no route into the operator portal or unreferenced school uploads.
drop policy if exists scoped_equipment_admin_file_read on storage.objects;
create policy scoped_equipment_admin_file_read on storage.objects for select to authenticated using(bucket_id='manufacturing' and public.cbgpu_equipment_platform_oversight() and(exists(select 1 from public.print_jobs j where j.file_path=name) or exists(select 1 from public.laser_jobs j where j.file_path=name)));
drop policy if exists scoped_equipment_file_delete on storage.objects;
create policy scoped_equipment_file_delete on storage.objects as restrictive for delete to authenticated using(bucket_id<>'manufacturing' or public.cbgpu_personal_project_identity());
-- Tighten SECURITY DEFINER projections and aggregate queues without broadening
-- generic legacy role predicates, which other admin views may still display.
do $$declare signature text;definition text;updated text;begin
 foreach signature in array array['public.cbgpu_manufacturing_profiles(text,uuid[])','public.cbgpu_manufacturing_managers(text,integer,uuid[])','public.print_queue_length()','public.laser_queue_length()','public.printer_plate_backlog()'] loop
  definition:=pg_get_functiondef(signature::regprocedure);updated:=definition;
  updated:=replace(updated,'public.can_manage_printer(auth.uid(),j.preferred_printer_id)', 'public.cbgpu_equipment_operator(''print'',coalesce(j.printer_id,j.preferred_printer_id))');
  updated:=replace(updated,'public.can_manage_printer(auth.uid(),p_printer_id)', 'public.cbgpu_equipment_operator(''print'',p_printer_id)');
  updated:=replace(updated,'public.is_3d_manager(auth.uid())','public.cbgpu_equipment_operator(''print'',null)');
  updated:=replace(updated,'public.is_laser_manager(auth.uid())','public.cbgpu_equipment_operator(''laser'',null)');
  updated:=replace(updated,'public.is_admin(auth.uid())','public.cbgpu_equipment_platform_oversight()');
  if signature='public.cbgpu_manufacturing_managers(text,integer,uuid[])' then
   updated:=replace(updated,'where manager.printer_id=p_printer_id and p.id=any(p_user_ids) and p.account_status', 'where manager.printer_id=p_printer_id and p.id=any(p_user_ids) and cbgpu_schools.equipment_manager_valid(p.id,''print'',p_printer_id) and p.account_status');
   updated:=replace(updated,'where p.is_laser_cut_manager and p.account_status','where p.is_laser_cut_manager and cbgpu_schools.equipment_manager_valid(p.id,''laser'',null) and p.account_status');
  end if;
  if updated<>definition then execute updated;end if;
 end loop;
 definition:=pg_get_functiondef('public.start_print_job(uuid,integer,numeric)'::regprocedure);
 if position('public.cbgpu_equipment_operator' in definition)=0 then
  updated:=replace(definition,'public.is_admin(v_user) OR EXISTS(SELECT 1 FROM public.printer_managers WHERE user_id=v_user AND printer_id=p_printer_id)','public.cbgpu_equipment_platform_oversight() OR public.cbgpu_equipment_operator(''print'',p_printer_id)');
  if updated=definition then raise exception 'START_PRINT_JOB_AUTHORITY_VERSION_MISMATCH';end if;execute updated;
 end if;
end$$;
commit;
