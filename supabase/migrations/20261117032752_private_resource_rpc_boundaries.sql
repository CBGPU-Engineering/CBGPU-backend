begin;
-- Keep boolean RLS predicates executable, but never let anonymous callers
-- enumerate another account's approval/manager privileges through their uid arg.
do $$declare signature text;function_oid oid;body text;definition text;begin
 foreach signature in array array['public.is_admin(uuid)','public.get_is_approved(uuid)','public.is_3d_manager(uuid)','public.is_laser_manager(uuid)','public.is_wakeup_manager(uuid)','public.can_manage_printer(uuid,integer)'] loop
 function_oid:=to_regprocedure(signature);if function_oid is null then continue;end if;
 select prosrc into body from pg_proc where oid=function_oid;
 if position('cbgpu:scoped-role-reader-v1' in body)>0 then continue;end if;
 definition:=format('CREATE OR REPLACE FUNCTION public.%I(%s) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path='''' AS %L',
 (select proname from pg_proc where oid=function_oid),pg_get_function_arguments(function_oid),
 '/* cbgpu:scoped-role-reader-v1 */ SELECT CASE WHEN coalesce(auth.role(),current_setting(''role'',true),'''')<>''anon'' AND(auth.role()=''service_role'' OR session_user IN(''postgres'',''supabase_admin'') OR public.cbgpu_personal_project_identity() OR coalesce(pg_has_role(session_user,to_regrole(''cbgpu_corpus_backend''),''MEMBER''),false)) AND(uid=auth.uid() OR auth.role()=''service_role'' OR session_user IN(''postgres'',''supabase_admin'') OR EXISTS(SELECT 1 FROM public.profiles actor WHERE actor.id=auth.uid() AND actor.is_admin)) THEN ('||btrim(body,E' \n\r\t;')||') ELSE false END');
 execute definition;
 end loop;
end$$;
create or replace function public.get_gpu_reservation_schedule(p_from timestamptz,p_to timestamptz,p_gpu_id integer default null) returns table(id uuid,gpu_id integer,start_time timestamptz,end_time timestamptz) language plpgsql stable security definer set search_path='' as $$begin
 if not public.cbgpu_personal_project_identity() or not(public.cbgpu_can_use_resource('gpu','cbsh') or public.is_admin(auth.uid())) then raise exception 'GPU_SCHEDULE_PERMISSION_REQUIRED' using errcode='42501';end if;
 return query select r.id,r.gpu_id,r.start_time,r.end_time from public.reservations r where r.status='approved' and r.start_time<p_to and r.end_time>p_from and(p_gpu_id is null or r.gpu_id=p_gpu_id) order by r.start_time;
end$$;
create or replace function public.get_gpu_booking_schedule(p_from timestamptz,p_to timestamptz,p_gpu_id integer default null) returns table(id uuid,gpu_id integer,start_time timestamptz,end_time timestamptz) language plpgsql stable security definer set search_path='' as $$begin
 if not public.cbgpu_personal_project_identity() or not(public.cbgpu_can_use_resource('gpu','cbsh') or public.is_admin(auth.uid())) then raise exception 'GPU_SCHEDULE_PERMISSION_REQUIRED' using errcode='42501';end if;
 return query select r.id,r.gpu_id,r.start_time,r.end_time from public.reservations r where r.status in('pending','approved') and r.start_time<p_to and r.end_time>p_from and(p_gpu_id is null or r.gpu_id=p_gpu_id) order by r.start_time;
end$$;
create or replace function public.print_queue_length() returns integer language plpgsql stable security definer set search_path='' as $$begin
 if not public.cbgpu_personal_project_identity() or not(public.cbgpu_can_use_resource('fabrication','cbsh') or public.is_3d_manager(auth.uid()) or public.is_admin(auth.uid())) then raise exception 'PRINT_QUEUE_PERMISSION_REQUIRED' using errcode='42501';end if;
 return(select count(*)::integer from public.print_jobs where status in('pending','accepted'));
end$$;
create or replace function public.laser_queue_length() returns integer language plpgsql stable security definer set search_path='' as $$begin
 if not public.cbgpu_personal_project_identity() or not(public.cbgpu_can_use_resource('fabrication','cbsh') or public.is_laser_manager(auth.uid()) or public.is_admin(auth.uid())) then raise exception 'LASER_QUEUE_PERMISSION_REQUIRED' using errcode='42501';end if;
 return(select count(*)::integer from public.laser_jobs where status in('pending','accepted'));
end$$;
create or replace function public.printer_plate_backlog() returns table(printer_id integer,plates bigint) language plpgsql stable security definer set search_path='' as $$begin
 if not public.cbgpu_personal_project_identity() or not(public.cbgpu_can_use_resource('fabrication','cbsh') or public.is_3d_manager(auth.uid()) or public.is_admin(auth.uid())) then raise exception 'PRINT_QUEUE_PERMISSION_REQUIRED' using errcode='42501';end if;
 return query select j.preferred_printer_id,coalesce(sum(case when j.status in('pending','accepted') then j.plate_count when j.status='printing' then greatest(j.plate_count-j.plates_completed-1,0) else 0 end),0)::bigint from public.print_jobs j where j.status in('pending','accepted','printing') and j.preferred_printer_id is not null group by j.preferred_printer_id;
end$$;
revoke all on function public.get_gpu_reservation_schedule(timestamptz,timestamptz,integer),public.get_gpu_booking_schedule(timestamptz,timestamptz,integer),public.print_queue_length(),public.laser_queue_length(),public.printer_plate_backlog() from public,anon;
grant execute on function public.get_gpu_reservation_schedule(timestamptz,timestamptz,integer),public.get_gpu_booking_schedule(timestamptz,timestamptz,integer),public.print_queue_length(),public.laser_queue_length(),public.printer_plate_backlog() to authenticated;
do $$declare definition text;updated text;begin
 if to_regprocedure('public.approve_gpu_reservation(uuid)') is not null then
 definition:=pg_get_functiondef('public.approve_gpu_reservation(uuid)'::regprocedure);
 if position('public.cbgpu_personal_project_identity()' in definition)=0 then updated:=replace(definition,'if auth.uid() is null or not public.is_admin(auth.uid()) then',E'if not public.cbgpu_personal_project_identity() then raise exception ''AUTHENTICATION_REQUIRED'' using errcode=''42501'';end if;\n  if auth.uid() is null or not public.is_admin(auth.uid()) then');if updated=definition then raise exception 'GPU_APPROVAL_VERSION_MISMATCH';end if;execute updated;end if;
 execute 'revoke all on function public.approve_gpu_reservation(uuid) from public,anon';execute 'grant execute on function public.approve_gpu_reservation(uuid) to authenticated';
 end if;
 -- This endpoint authenticates a pre-registered, revocable random device
 -- capability rather than an interactive session. Keep background refresh, but
 -- reevaluate its owner's CURRENT CBsh affiliation/resource grant every call.
 if to_regprocedure('public.accept_eduset_contribution(text,text,timestamptz)') is not null then
 definition:=pg_get_functiondef('public.accept_eduset_contribution(text,text,timestamptz)'::regprocedure);
 if position('public.cbgpu_service_can_use_resource' in definition)=0 then updated:=replace(definition,'if v_user_id is null then return false; end if;',E'if v_user_id is null then return false; end if;\n  if not public.cbgpu_service_can_use_resource(v_user_id,''study'',''cbsh'') then return false;end if;');if updated=definition then raise exception 'EDUSET_CAPABILITY_VERSION_MISMATCH';end if;execute updated;end if;
 end if;
end$$;
commit;
