-- Real schema/RPC regression. Everything is rolled back inside isolated clone.
begin;
update cbgpu_schools.memberships set status='SUSPENDED' where school_id='cbsh' and membership_type='STUDENT';
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)
select ('fa100000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-academic-'||x||'@fixture.invalid',jsonb_build_object('full_name','Synthetic academic '||x,'account_type_request','EXTERNAL'),'{}','authenticated','authenticated','fixture-no-password',now(),now(),now() from generate_series(1,5)x;
insert into auth.sessions(id,user_id) select ('fa200000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,('fa100000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid from generate_series(1,5)x;
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,school_roles,student_number,student_number_school_year)
select ('fa100000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'cbsh',case when x=4 then 'TEACHER' when x=5 then 'EXTERNAL' else 'STUDENT' end,'ACTIVE',true,case when x=5 then array['SCHOOL_ADMIN'] else '{}'::text[] end,case when x<=3 then x::text||'198' end,extract(year from now())::integer from generate_series(1,5)x;
insert into public.reservations(id,user_id,gpu_id,status,start_time,end_time) values('fa300000-0000-4000-8000-000000000001','fa100000-0000-4000-8000-000000000001',1,'approved',now()+interval '4000 days',now()+interval '4000 days 1 hour');
select set_config('request.jwt.claims','{"sub":"fa100000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fa200000-0000-4000-8000-000000000001"}',true);
set local role authenticated;
do $$begin if not public.get_is_approved(auth.uid()) then raise exception 'VERIFIED_MEMBER_CANNOT_USE_LEGACY_ADAPTER';end if;end$$;
reset role;
set local role service_role;
do $$begin if (public.cbgpu_service_gpu_snapshot()->>'count')::int<>jsonb_array_length(public.cbgpu_service_gpu_snapshot()->'reservations') or not exists(select 1 from jsonb_array_elements(public.cbgpu_service_gpu_snapshot()->'reservations')r where r->>'id'='fa300000-0000-4000-8000-000000000001') then raise exception 'SNAPSHOT_NOT_ATOMIC_COMPLETE';end if; if not exists(select 1 from public.cbgpu_service_gpu_assignments() where id='fa300000-0000-4000-8000-000000000001') then raise exception 'FRESH_ASSIGNMENT_NOT_VISIBLE';end if;end$$;
reset role;
create temp table academic_before as select id,account_type,account_status,student_id,student_id_school_year from public.profiles where id::text like 'fa100000%';
create temp table academic_fixture(k text,v jsonb);grant all on academic_fixture to authenticated;
-- Other-school policies fail closed even if an administrator config imitates CBsh.
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,configuration,features) values('academic-unsupported','Synthetic Other','synthetic other','Synthetic Other','HIGH_SCHOOL','{"student_id_policy":"CBSH_4_DIGIT"}',array['projects']);
select set_config('request.jwt.claims','{"sub":"fa100000-0000-4000-8000-000000000005","role":"authenticated","session_id":"fa200000-0000-4000-8000-000000000005"}',true);
set local role authenticated;
do $$begin begin perform public.cbgpu_school_academic_api('preview',jsonb_build_object('school_id','academic-unsupported','school_year',extract(year from now())::integer+1));raise exception 'UNSUPPORTED_POLICY_ALLOWED';exception when check_violation then null;end;end$$;
reset role;
update cbgpu_schools.memberships set student_number='malformed' where user_id='fa100000-0000-4000-8000-000000000001';
set local role authenticated;
do $$declare p jsonb;begin
 p:=public.cbgpu_school_academic_api('preview',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1));
 begin perform public.cbgpu_school_academic_api('execute',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1,'preview_fingerprint',p->>'preview_fingerprint','confirmed',true));raise exception 'INVALID_STUDENT_BATCH_ALLOWED';exception when check_violation then null;end;
end$$;
reset role;
do $$begin if exists(select 1 from cbgpu_schools.academic_runs where school_id='cbsh') then raise exception 'FAILED_BATCH_NOT_ROLLED_BACK';end if;end$$;
update cbgpu_schools.memberships set student_number='1198' where user_id='fa100000-0000-4000-8000-000000000001';
set local role authenticated;
insert into academic_fixture values('preview',public.cbgpu_school_academic_api('preview',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1)));
do $$declare p jsonb:=(select v from academic_fixture where k='preview');begin
 if p#>>'{counts,grade1}'<>'1' or p#>>'{counts,grade2}'<>'1' or p#>>'{counts,grade3}'<>'1' then raise exception 'ACADEMIC_COUNTS';end if;
 begin perform public.cbgpu_school_academic_api('execute',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1,'preview_fingerprint',p->>'preview_fingerprint'));raise exception 'CONFIRMATION_BYPASS';exception when check_violation then null;end;
 begin perform public.cbgpu_school_academic_api('execute',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1,'preview_fingerprint','stale','confirmed',true));raise exception 'STALE_PREVIEW_BYPASS';exception when serialization_failure then null;end;
 perform public.cbgpu_school_academic_api('execute',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1,'preview_fingerprint',p->>'preview_fingerprint','confirmed',true));
 begin perform public.cbgpu_school_academic_api('execute',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1,'preview_fingerprint',p->>'preview_fingerprint','confirmed',true));raise exception 'DUPLICATE_RUN';exception when unique_violation then null;end;
end$$;
reset role;
do $$begin
 if (select count(*) from cbgpu_schools.memberships where user_id::text like 'fa100000%' and status='REVERIFICATION_REQUIRED' and required_grade in(2,3) and student_number is null)<>2 then raise exception 'LOWER_GRADES_NOT_RESET';end if;
 if not exists(select 1 from cbgpu_schools.memberships where user_id='fa100000-0000-4000-8000-000000000003' and membership_type='EXTERNAL' and not verified and student_number is null) then raise exception 'GRADUATE_NOT_EXTERNAL';end if;
 if not exists(select 1 from cbgpu_schools.memberships where user_id='fa100000-0000-4000-8000-000000000004' and membership_type='TEACHER' and verified) then raise exception 'TEACHER_CHANGED';end if;
 if (select count(*) from public.profiles where id::text like 'fa100000%' and account_type='EXTERNAL')<>5 then raise exception 'PRIMARY_PROFILE_MUTATED';end if;
end$$;
select set_config('request.jwt.claims','{"sub":"fa100000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fa200000-0000-4000-8000-000000000001"}',true);
set local role authenticated;
do $$begin
 begin perform public.cbgpu_school_academic_api('preview',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1));raise exception 'STUDENT_ADMIN_BYPASS';exception when insufficient_privilege then null;end;
 begin perform public.cbgpu_school_academic_api('reverify',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1,'student_number','1198'));raise exception 'WRONG_GRADE_ACCEPTED';exception when check_violation then null;end;
 perform public.cbgpu_school_academic_api('reverify',jsonb_build_object('school_id','cbsh','school_year',extract(year from now())::integer+1,'student_number','2198'));
end$$;
reset role;
do $$begin if not exists(select 1 from cbgpu_schools.memberships where user_id='fa100000-0000-4000-8000-000000000001' and student_number='2198' and not verified and status='PENDING_VERIFICATION') then raise exception 'REVERIFY_MUST_REMAIN_PENDING';end if;end$$;
do $$begin if exists((select id,account_type,account_status,student_id,student_id_school_year from public.profiles where id::text like 'fa100000%') except select * from academic_before) then raise exception 'LEGACY_PROFILE_CHANGED';end if;end$$;
select set_config('request.jwt.claims','{"sub":"fa100000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fa200000-0000-4000-8000-000000000001"}',true);
set local role authenticated;
do $$begin
 if public.get_is_approved(auth.uid()) then raise exception 'ROLLOVER_BORROWED_PRIMARY_APPROVAL';end if;
 if (public.cbgpu_self_resource_access('cbsh')#>>'{resources,gpu}')::boolean then raise exception 'REVERIFY_GPU_ACCESS_RETAINED';end if;
 begin perform * from public.cbgpu_service_gpu_assignments();raise exception 'USER_CONTROLLER_ASSIGNMENTS_LEAK';exception when insufficient_privilege then null;end;
end$$;
reset role;
set local role service_role;
do $$begin if exists(select 1 from jsonb_array_elements(public.cbgpu_service_gpu_snapshot()->'reservations')r where r->>'id'='fa300000-0000-4000-8000-000000000001') then raise exception 'SNAPSHOT_RETAINED_REVOKED_ASSIGNMENT';end if; if exists(select 1 from public.cbgpu_service_gpu_assignments() where id='fa300000-0000-4000-8000-000000000001') then raise exception 'ROLLOVER_ASSIGNMENT_NOT_REVOKED';end if;end$$;
reset role;
do $$begin if not exists(select 1 from public.reservations where id='fa300000-0000-4000-8000-000000000001' and status='approved') then raise exception 'RESERVATION_HISTORY_MUTATED';end if;end$$;
update auth.sessions set not_after=now()-interval '1 second' where id='fa200000-0000-4000-8000-000000000005';
select set_config('request.jwt.claims','{"sub":"fa100000-0000-4000-8000-000000000005","role":"authenticated","session_id":"fa200000-0000-4000-8000-000000000005"}',true);
set local role authenticated;
do $$begin begin perform public.cbgpu_school_academic_api('history','{"school_id":"cbsh"}');raise exception 'REVOKED_SESSION_HISTORY_ALLOWED';exception when insufficient_privilege then null;end;end$$;
reset role;
rollback;
