begin;
-- Profiles contain legacy PRIMARY-school academic data. Sharing a second-school
-- membership never authorizes reading that entire global profile row.
create or replace function public.cbgpu_profile_visible(p_user uuid) returns boolean language sql stable security definer set search_path='' as $$select cbgpu_schools.read_identity_allowed() and(p_user=auth.uid() or exists(select 1 from public.profiles actor where actor.id=auth.uid() and actor.is_admin))$$;
revoke all on function public.cbgpu_profile_visible(uuid) from public,anon;grant execute on function public.cbgpu_profile_visible(uuid) to authenticated;
drop policy if exists multischool_profile_private on public.profiles;
create policy multischool_profile_private on public.profiles as restrictive for select to authenticated using(public.cbgpu_profile_visible(id));
-- Factory managers receive only identifiers needed for THEIR authorized jobs.
-- Existing dedicated manager assignments do not become platform administrator roles.
create or replace function public.cbgpu_manufacturing_profiles(p_kind text,p_user_ids uuid[]) returns table(id uuid,full_name text,student_id text) language plpgsql stable security definer set search_path='' as $$begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if p_user_ids is null or cardinality(p_user_ids)>500 or array_position(p_user_ids,null) is not null then raise exception 'INVALID_PROFILE_REQUEST' using errcode='23514';end if;
 if p_kind='print' then return query select distinct p.id,p.full_name,p.student_id from public.profiles p join public.print_jobs j on j.user_id=p.id where p.id=any(p_user_ids) and(j.user_id=auth.uid() or public.is_admin(auth.uid()) or public.can_manage_printer(auth.uid(),j.preferred_printer_id));
 elsif p_kind='laser' then return query select distinct p.id,p.full_name,p.student_id from public.profiles p join public.laser_jobs j on j.user_id=p.id where p.id=any(p_user_ids) and(j.user_id=auth.uid() or public.is_admin(auth.uid()) or public.is_laser_manager(auth.uid()));
 else raise exception 'INVALID_MANUFACTURING_KIND' using errcode='23514';end if;
end$$;
-- Project operators may resolve only existing operator labels, or perform an
-- exact academic-number lookup within the contest's own verified school. Never
-- return another school's primary number as a shortcut for a global uid search.
create or replace function public.cbgpu_contest_operator_profiles(p_contest uuid,p_user_ids uuid[] default null,p_student_number text default null) returns table(id uuid,full_name text,student_id text) language plpgsql stable security definer set search_path='' as $$declare c public.contests;actor uuid:=auth.uid();matches integer;begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 select * into c from public.contests where contests.id=p_contest;
 if not found or not(c.host_id=actor or exists(select 1 from public.event_hosts h where h.contest_id=c.id and h.user_id=actor and h.role='manager')) then raise exception 'CONTEST_MANAGER_REQUIRED' using errcode='42501';end if;
 if p_student_number is not null then
 if c.host_id<>actor or not cbgpu_schools.active_member(actor,c.school_id) or length(btrim(p_student_number)) not between 1 and 80 then raise exception 'SCHOOL_OPERATOR_LOOKUP_DENIED' using errcode='42501';end if;
 select count(*) into matches from cbgpu_schools.memberships m where m.school_id=c.school_id and m.student_number=btrim(p_student_number) and m.membership_type='STUDENT' and cbgpu_schools.active_member(m.user_id,c.school_id);
 if matches>1 then raise exception 'AMBIGUOUS_STUDENT_NUMBER' using errcode='23505';end if;
 return query select p.id,p.full_name,m.student_number from public.profiles p join cbgpu_schools.memberships m on m.user_id=p.id where m.school_id=c.school_id and m.student_number=btrim(p_student_number) and m.membership_type='STUDENT' and cbgpu_schools.active_member(m.user_id,c.school_id);
 else
 if p_user_ids is null or cardinality(p_user_ids)>500 or array_position(p_user_ids,null) is not null then raise exception 'INVALID_PROFILE_REQUEST' using errcode='23514';end if;
 return query select p.id,p.full_name,case when cbgpu_schools.active_member(actor,c.school_id) then (select m.student_number from cbgpu_schools.memberships m where m.user_id=p.id and m.school_id=c.school_id and m.status='ACTIVE' and m.verified) end from public.profiles p where p.id=any(p_user_ids) and(p.id=c.host_id or exists(select 1 from public.event_hosts h where h.contest_id=c.id and h.user_id=p.id));
 end if;
end$$;
revoke all on function public.cbgpu_manufacturing_profiles(text,uuid[]),public.cbgpu_contest_operator_profiles(uuid,uuid[],text) from public,anon;
grant execute on function public.cbgpu_manufacturing_profiles(text,uuid[]),public.cbgpu_contest_operator_profiles(uuid,uuid[],text) to authenticated;
-- Picker names and historical job-manager labels are separate permissions.
create or replace function public.cbgpu_manufacturing_managers(p_kind text,p_printer_id integer default null,p_user_ids uuid[] default null) returns table(id uuid,full_name text) language plpgsql stable security definer set search_path='' as $$begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if p_user_ids is not null and(cardinality(p_user_ids)>500 or array_position(p_user_ids,null) is not null) then raise exception 'INVALID_PROFILE_REQUEST';end if;
 if p_kind='print' then
 if p_printer_id is not null then
 if not(public.cbgpu_can_use_resource('fabrication','cbsh') or public.can_manage_printer(auth.uid(),p_printer_id) or public.is_admin(auth.uid())) then raise exception 'PRINTER_MANAGER_LOOKUP_DENIED' using errcode='42501';end if;
 if p_user_ids is null then raise exception 'PRINTER_PICKER_IDS_REQUIRED';end if;
 return query select distinct p.id,p.full_name from public.profiles p join public.printer_managers manager on manager.user_id=p.id join auth.users u on u.id=p.id where manager.printer_id=p_printer_id and p.id=any(p_user_ids) and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now());
 else
 if p_user_ids is null then raise exception 'JOB_MANAGER_IDS_REQUIRED';end if;
 return query select distinct p.id,p.full_name from public.profiles p join public.print_jobs j on j.manager_id=p.id where p.id=any(p_user_ids) and(j.user_id=auth.uid() or public.is_admin(auth.uid()) or public.can_manage_printer(auth.uid(),j.preferred_printer_id));
 end if;
 elsif p_kind='laser' then
 if p_printer_id is not null then raise exception 'INVALID_LASER_PRINTER_SCOPE';end if;
 if p_user_ids is null then
 if not(public.cbgpu_can_use_resource('fabrication','cbsh') or public.is_laser_manager(auth.uid()) or public.is_admin(auth.uid())) then raise exception 'LASER_MANAGER_LOOKUP_DENIED' using errcode='42501';end if;
 return query select p.id,p.full_name from public.profiles p join auth.users u on u.id=p.id where p.is_laser_cut_manager and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now());
 else
 return query select distinct p.id,p.full_name from public.profiles p join public.laser_jobs j on j.manager_id=p.id where p.id=any(p_user_ids) and(j.user_id=auth.uid() or public.is_admin(auth.uid()) or public.is_laser_manager(auth.uid()));
 end if;
 else raise exception 'INVALID_MANUFACTURING_KIND';end if;
end$$;
revoke all on function public.cbgpu_manufacturing_managers(text,integer,uuid[]) from public,anon;grant execute on function public.cbgpu_manufacturing_managers(text,integer,uuid[]) to authenticated;
drop policy if exists multischool_printer_manager_read on public.printer_managers;
create policy multischool_printer_manager_read on public.printer_managers as restrictive for select to authenticated using(public.cbgpu_personal_project_identity() and(user_id=auth.uid() or public.is_admin(auth.uid()) or public.cbgpu_can_use_resource('fabrication','cbsh')));
-- Legacy matching groups stay CBsh-only by default. Personal/global posts require
-- an explicit new visibility; no old group or membership becomes public.
alter table public.study_posts add column if not exists school_id text default 'cbsh' references cbgpu_schools.schools(id),add column if not exists access_scope text not null default 'SCHOOL',add column if not exists allowed_school_ids text[] not null default '{}';
create or replace function public.cbgpu_study_post_visible(p_post uuid) returns boolean language sql stable security definer set search_path='' as $$select cbgpu_schools.read_identity_allowed() and exists(select 1 from public.study_posts post where post.id=p_post and(post.user_id=auth.uid() or exists(select 1 from public.study_members member where member.post_id=post.id and member.user_id=auth.uid()) or cbgpu_schools.scope_read(auth.uid(),post.school_id,post.access_scope,post.allowed_school_ids)))$$;
revoke all on function public.cbgpu_study_post_visible(uuid) from public,anon;grant execute on function public.cbgpu_study_post_visible(uuid) to authenticated;
drop policy if exists multischool_study_post_read on public.study_posts;
create policy multischool_study_post_read on public.study_posts as restrictive for select to authenticated using(public.cbgpu_study_post_visible(id));
drop policy if exists multischool_study_members_read on public.study_members;
create policy multischool_study_members_read on public.study_members as restrictive for select to authenticated using(public.cbgpu_study_post_visible(post_id));
drop policy if exists multischool_study_post_create on public.study_posts;
create policy multischool_study_post_create on public.study_posts as restrictive for insert to authenticated with check(public.cbgpu_personal_project_identity() and user_id=auth.uid() and((school_id is null and access_scope in('PRIVATE','AUTHENTICATED','PUBLIC') and cardinality(allowed_school_ids)=0)or public.cbgpu_school_scope_valid(school_id,access_scope,allowed_school_ids)));
revoke select on public.study_posts,public.study_members from anon;
create or replace function cbgpu_schools.guard_study_membership() returns trigger language plpgsql security definer set search_path='' as $$declare post public.study_posts;begin
 if not public.cbgpu_personal_project_identity() or new.user_id<>auth.uid() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 perform pg_advisory_xact_lock(hashtextextended('study-group:'||new.post_id::text,0));select * into post from public.study_posts where id=new.post_id;
 if not found or not post.is_open or not(post.user_id=auth.uid() or(post.access_scope<>'PRIVATE' and cbgpu_schools.scope_read(auth.uid(),post.school_id,post.access_scope,post.allowed_school_ids))) then raise exception 'STUDY_GROUP_SCOPE_DENIED' using errcode='42501';end if;
 if(select count(*) from public.study_members where post_id=post.id)>=post.max_members then raise exception 'STUDY_GROUP_FULL';end if;return new;end$$;
drop trigger if exists multischool_study_join on public.study_members;create trigger multischool_study_join before insert on public.study_members for each row execute function cbgpu_schools.guard_study_membership();
create or replace function public.cbgpu_study_member_profiles(p_user_ids uuid[]) returns table(id uuid,full_name text) language plpgsql stable security definer set search_path='' as $$begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if p_user_ids is null or cardinality(p_user_ids)>500 or array_position(p_user_ids,null) is not null then raise exception 'INVALID_PROFILE_REQUEST';end if;
 return query select p.id,p.full_name from public.profiles p where p.id=any(p_user_ids) and(p.id=auth.uid() or exists(select 1 from public.study_posts post where post.user_id=p.id and public.cbgpu_study_post_visible(post.id)) or exists(select 1 from public.study_members member where member.user_id=p.id and public.cbgpu_study_post_visible(member.post_id)));
end$$;
revoke all on function cbgpu_schools.guard_study_membership() from public,anon,authenticated;
revoke all on function public.cbgpu_study_member_profiles(uuid[]) from public,anon;grant execute on function public.cbgpu_study_member_profiles(uuid[]) to authenticated;
create or replace function cbgpu_schools.guard_study_post_scope() returns trigger language plpgsql security definer set search_path='' as $$begin
 if auth.uid() is null and(auth.role()='service_role' or session_user in('postgres','supabase_admin')) then if tg_op='DELETE' then return old;else return new;end if;end if;
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if tg_op='DELETE' then return old;end if;
 if new.id<>old.id or new.user_id<>old.user_id or new.created_at<>old.created_at then raise exception 'STUDY_PROJECT_OWNERSHIP_IMMUTABLE';end if;
 if(new.school_id,new.access_scope,new.allowed_school_ids) is distinct from(old.school_id,old.access_scope,old.allowed_school_ids) and not((new.school_id is null and new.access_scope in('PRIVATE','AUTHENTICATED','PUBLIC') and cardinality(new.allowed_school_ids)=0)or cbgpu_schools.scope_valid(auth.uid(),new.school_id,new.access_scope,new.allowed_school_ids)) then raise exception 'STUDY_GROUP_SCOPE_DENIED' using errcode='42501';end if;return new;
end$$;
drop trigger if exists multischool_study_post_scope on public.study_posts;create trigger multischool_study_post_scope before update or delete on public.study_posts for each row execute function cbgpu_schools.guard_study_post_scope();
revoke all on function cbgpu_schools.guard_study_post_scope() from public,anon,authenticated;
commit;

