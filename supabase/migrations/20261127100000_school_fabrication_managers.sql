begin;
-- Fabrication equipment belongs to a school. Printers get a school, laser managers
-- become school rows, and a school administrator manages both for their own school.
-- Requests, files and queues still run only for the school that platform policy
-- opens fabrication for (can_user_use_resource), which is 'cbsh' today.

alter table public.printers add column if not exists school_id text not null default 'cbsh' references cbgpu_schools.schools(id);
create index if not exists printers_school_idx on public.printers(school_id);

create table if not exists cbgpu_schools.laser_managers(
 school_id text not null references cbgpu_schools.schools(id) on delete cascade,
 user_id uuid not null references public.profiles(id) on delete cascade,
 created_by uuid,
 created_at timestamptz not null default now(),
 primary key(school_id,user_id));
alter table cbgpu_schools.laser_managers enable row level security;
revoke all on cbgpu_schools.laser_managers from public,anon,authenticated;
insert into cbgpu_schools.laser_managers(school_id,user_id) select 'cbsh',id from public.profiles where is_laser_cut_manager on conflict do nothing;

-- A manager only counts for the school whose equipment queue is being operated.
create or replace function cbgpu_schools.equipment_manager_valid(subject uuid,kind text,printer integer default null) returns boolean language sql stable security definer set search_path='' as $$
select cbgpu_schools.legacy_cbsh_access(subject) and cbgpu_accounts.can_user_use_resource(subject,'fabrication','cbsh') and
 case kind when 'print' then exists(select 1 from public.printer_managers pm join public.printers pr on pr.id=pm.printer_id where pm.user_id=subject and pr.school_id='cbsh' and(printer is null or pm.printer_id=printer))
 when 'laser' then exists(select 1 from cbgpu_schools.laser_managers lm where lm.user_id=subject and lm.school_id='cbsh') else false end
$$;

-- The legacy profile flag is no longer the source of truth; laser_managers is.
do $$declare d text;u text;begin
 d:=pg_get_functiondef('public.is_laser_manager(uuid)'::regprocedure);
 u:=replace(d,'(select coalesce((select p.is_laser_cut_manager from public.profiles p where p.id = uid), false))','(select exists(select 1 from cbgpu_schools.laser_managers lm where lm.user_id=uid and lm.school_id=''cbsh''))');
 if u=d then raise exception 'IS_LASER_MANAGER_VERSION_MISMATCH';end if;execute u;
 d:=pg_get_functiondef('public.cbgpu_manufacturing_managers(text,integer,uuid[])'::regprocedure);
 u:=replace(d,'where p.is_laser_cut_manager and cbgpu_schools.equipment_manager_valid','where cbgpu_schools.equipment_manager_valid');
 if u=d then raise exception 'MANUFACTURING_MANAGERS_VERSION_MISMATCH';end if;execute u;
end$$;

-- Other schools' printers and their managers are not shown to the cbsh queue.
create or replace function public.cbgpu_printer_in_fabrication_school(p_printer integer) returns boolean language sql stable security definer set search_path='' as $$
select exists(select 1 from public.printers where id=p_printer and school_id='cbsh')
$$;
revoke all on function public.cbgpu_printer_in_fabrication_school(integer) from public,anon;
grant execute on function public.cbgpu_printer_in_fabrication_school(integer) to authenticated;
drop policy if exists school_printer_read on public.printers;
create policy school_printer_read on public.printers as restrictive for select to authenticated using(school_id='cbsh');
drop policy if exists school_printer_manager_read on public.printer_managers;
create policy school_printer_manager_read on public.printer_managers as restrictive for select to authenticated using(public.cbgpu_printer_in_fabrication_school(printer_id));

-- Client-side reads of the laser role (service reads are separate and service-only).
create or replace function public.cbgpu_my_laser_manager() returns boolean language sql stable security definer set search_path='' as $$
select public.cbgpu_personal_project_identity() and exists(select 1 from cbgpu_schools.laser_managers where user_id=auth.uid() and school_id='cbsh')
$$;
revoke all on function public.cbgpu_my_laser_manager() from public,anon;
grant execute on function public.cbgpu_my_laser_manager() to authenticated;
create or replace function public.cbgpu_laser_manager_ids() returns uuid[] language sql stable security definer set search_path='' as $$
select coalesce(array_agg(user_id),'{}') from cbgpu_schools.laser_managers where school_id='cbsh'
$$;
revoke all on function public.cbgpu_laser_manager_ids() from public,anon,authenticated;
grant execute on function public.cbgpu_laser_manager_ids() to service_role;
create or replace function public.cbgpu_is_laser_manager(p_user uuid) returns boolean language sql stable security definer set search_path='' as $$
select exists(select 1 from cbgpu_schools.laser_managers where user_id=p_user and school_id='cbsh')
$$;
revoke all on function public.cbgpu_is_laser_manager(uuid) from public,anon,authenticated;
grant execute on function public.cbgpu_is_laser_manager(uuid) to service_role;

-- School administration of printers and managers.
create or replace function public.cbgpu_school_fabrication(p_action text,p_payload jsonb default '{}'::jsonb) returns jsonb language plpgsql security definer set search_path='' as $$
declare actor uuid:=auth.uid();sid text;pid integer;uid uuid;nm text;kind text;r jsonb;q text;
begin
 if actor is null or not public.cbgpu_personal_project_identity() then raise exception 'FRESH_IDENTITY_REQUIRED' using errcode='42501';end if;
 if jsonb_typeof(p_payload)<>'object' or pg_column_size(p_payload)>4096 then raise exception 'INVALID_FABRICATION_REQUEST' using errcode='23514';end if;
 sid:=p_payload->>'school_id';
 if sid is null or not exists(select 1 from cbgpu_schools.schools where id=sid) then raise exception 'SCHOOL_NOT_FOUND' using errcode='P0002';end if;
 if not(cbgpu_schools.has_role(actor,sid,array['SCHOOL_OWNER','SCHOOL_ADMIN']) or(sid='cbsh' and public.cbgpu_equipment_platform_oversight())) then raise exception 'SCHOOL_ADMIN_REQUIRED' using errcode='42501';end if;
 if not cbgpu_schools.feature_enabled(sid,'manufacturing') then raise exception 'FABRICATION_NOT_ENABLED' using errcode='23514';end if;
 perform pg_advisory_xact_lock(hashtext('fabrication:'||sid));

 if p_action='overview' then
  return jsonb_build_object('school_id',sid,
   'printers',(select coalesce(jsonb_agg(jsonb_build_object('id',pr.id,'name',pr.name,'managers',(select coalesce(jsonb_agg(jsonb_build_object('user_id',pm.user_id,'full_name',p.full_name,'student_number',m.student_number) order by p.full_name),'[]') from public.printer_managers pm join public.profiles p on p.id=pm.user_id left join cbgpu_schools.memberships m on m.user_id=pm.user_id and m.school_id=sid where pm.printer_id=pr.id)) order by pr.id),'[]') from public.printers pr where pr.school_id=sid),
   'laser_managers',(select coalesce(jsonb_agg(jsonb_build_object('user_id',lm.user_id,'full_name',p.full_name,'student_number',m.student_number) order by p.full_name),'[]') from cbgpu_schools.laser_managers lm join public.profiles p on p.id=lm.user_id left join cbgpu_schools.memberships m on m.user_id=lm.user_id and m.school_id=sid where lm.school_id=sid));
 elsif p_action='candidates' then
  q:=left(coalesce(trim(p_payload->>'q'),''),40);
  return jsonb_build_object('items',(select coalesce(jsonb_agg(jsonb_build_object('user_id',x.user_id,'full_name',x.full_name,'student_number',x.student_number)),'[]') from(select m.user_id,p.full_name,m.student_number from cbgpu_schools.memberships m join public.profiles p on p.id=m.user_id where m.school_id=sid and cbgpu_schools.active_member(m.user_id,sid) and m.membership_type in('STUDENT','TEACHER') and(q='' or p.full_name ilike '%'||replace(replace(q,'%','\%'),'_','\_')||'%' or m.student_number=q) order by p.full_name limit 20)x));
 elsif p_action='printer_add' then
  nm:=trim(coalesce(p_payload->>'name',''));
  if char_length(nm) not between 1 and 40 then raise exception 'INVALID_PRINTER_NAME' using errcode='23514';end if;
  if(select count(*) from public.printers where school_id=sid)>=20 then raise exception 'PRINTER_LIMIT' using errcode='23514';end if;
  perform pg_advisory_xact_lock(hashtext('fabrication:printer-id'));
  insert into public.printers(id,name,school_id) values((select coalesce(max(id),0)+1 from public.printers),nm,sid) returning id into pid;
 elsif p_action in('printer_rename','printer_remove','manager_add','manager_remove') then
  kind:=coalesce(p_payload->>'kind','print');
  if kind not in('print','laser') then raise exception 'INVALID_FABRICATION_REQUEST' using errcode='23514';end if;
  if p_action in('printer_rename','printer_remove') or kind='print' then
   begin pid:=(p_payload->>'printer_id')::integer;exception when others then raise exception 'INVALID_FABRICATION_REQUEST' using errcode='23514';end;
   if not exists(select 1 from public.printers where id=pid and school_id=sid) then raise exception 'PRINTER_NOT_FOUND' using errcode='P0002';end if;
  end if;
  if p_action='printer_rename' then
   nm:=trim(coalesce(p_payload->>'name',''));
   if char_length(nm) not between 1 and 40 then raise exception 'INVALID_PRINTER_NAME' using errcode='23514';end if;
   update public.printers set name=nm where id=pid;
  elsif p_action='printer_remove' then
   if exists(select 1 from public.print_jobs where printer_id=pid or preferred_printer_id=pid) then raise exception 'PRINTER_IN_USE' using errcode='23514';end if;
   delete from public.printers where id=pid;
  else
   begin uid:=(p_payload->>'user_id')::uuid;exception when others then raise exception 'INVALID_FABRICATION_REQUEST' using errcode='23514';end;
   if p_action='manager_add' then
    if not cbgpu_schools.active_member(uid,sid) or not exists(select 1 from cbgpu_schools.memberships where user_id=uid and school_id=sid and membership_type in('STUDENT','TEACHER')) then raise exception 'ACTIVE_SCHOOL_MEMBER_REQUIRED' using errcode='23514';end if;
    if kind='print' then insert into public.printer_managers(printer_id,user_id) values(pid,uid) on conflict do nothing;
    else insert into cbgpu_schools.laser_managers(school_id,user_id,created_by) values(sid,uid,actor) on conflict do nothing;end if;
   else
    if kind='print' then delete from public.printer_managers where printer_id=pid and user_id=uid;
    else delete from cbgpu_schools.laser_managers where school_id=sid and user_id=uid;end if;
   end if;
  end if;
 else raise exception 'UNKNOWN_FABRICATION_ACTION' using errcode='23514';end if;
 insert into cbgpu_schools.audit(actor_id,school_id,action,target_id,after_data) values(actor,sid,'fabrication.'||p_action,uid,p_payload-'school_id');
 return jsonb_build_object('ok',true,'printer_id',pid);
end$$;
revoke all on function public.cbgpu_school_fabrication(text,jsonb) from public,anon;
grant execute on function public.cbgpu_school_fabrication(text,jsonb) to authenticated;
commit;
