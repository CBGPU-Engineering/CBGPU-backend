begin;
-- Material grants are separate from school affiliation. Do not broaden the
-- generic scope reader: notes, rosters and school life remain member-only.
create or replace function public.cbgpu_material_scope_read(school text,scope text,allowed text[]) returns boolean language sql stable security definer set search_path='' as $$
select public.cbgpu_school_scope_read(school,scope,allowed)
 or(scope='SCHOOL' and public.cbgpu_school_read_identity() and public.cbgpu_can_use_resource('school_storage',school))
$$;
revoke all on function public.cbgpu_material_scope_read(text,text,text[]) from public;
grant execute on function public.cbgpu_material_scope_read(text,text,text[]) to anon,authenticated;
drop policy if exists multischool_material_scoped_read on public.shared_materials;
create policy multischool_material_scoped_read on public.shared_materials for select to authenticated using((uploader_id=auth.uid() and public.cbgpu_school_read_identity()) or(status='ready' and public.cbgpu_material_scope_read(school_id,access_scope,allowed_school_ids)));
drop policy if exists multischool_material_read on public.shared_materials;
create policy multischool_material_read on public.shared_materials as restrictive for select to anon,authenticated using((uploader_id=auth.uid() and public.cbgpu_school_read_identity()) or public.cbgpu_material_scope_read(school_id,access_scope,allowed_school_ids));
-- Guest creation permits own pending school-shared records only. It grants
-- neither publication nor multi-school/private scope selection authority.
create or replace function public.cbgpu_material_scope_valid(school text,scope text,allowed text[]) returns boolean language sql stable security definer set search_path='' as $$
select public.cbgpu_school_scope_valid(school,scope,allowed)
 or(scope='SCHOOL' and cardinality(allowed)=0 and public.cbgpu_school_read_identity() and public.cbgpu_can_use_resource('school_storage',school))
$$;
revoke all on function public.cbgpu_material_scope_valid(text,text,text[]) from public;
grant execute on function public.cbgpu_material_scope_valid(text,text,text[]) to authenticated;
drop policy if exists multischool_material_insert on public.shared_materials;
create policy multischool_material_insert on public.shared_materials as restrictive for insert to authenticated with check(public.cbgpu_material_scope_valid(school_id,access_scope,allowed_school_ids) and public.cbgpu_can_use_resource('school_storage',school_id));
commit;
