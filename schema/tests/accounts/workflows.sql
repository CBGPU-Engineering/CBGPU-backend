\set ON_ERROR_STOP on
create function public.test_assert(ok boolean,message text) returns void language plpgsql as $$begin if ok is distinct from true then raise exception 'TEST FAILED: %',message;end if;end$$;
select public.test_assert((select count(*)=4 from profiles),'legacy count preserved');
select public.test_assert((select account_type='TEACHER' and is_admin=false from profiles where full_name='Teacher'),'trusted teacher is not administrator');
select public.test_assert((select count(*)=3 from profiles where account_type='STUDENT'),'all other legacy users remain students');
create table public.test_projects(id uuid primary key,owner_id uuid references profiles(id),body text);
create table public.test_members(project_id uuid references test_projects(id),user_id uuid references profiles(id));
insert into test_projects values('10000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000003','retained');
insert into test_members values('10000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000002');
-- Registration requests never create verified school privileges.
insert into auth.users(id,email,raw_user_meta_data) values
 ('00000000-0000-0000-0000-000000000005','external@example.test','{"full_name":"External","full_name":"Duplicate external","account_type_request":"EXTERNAL"}'),
 ('00000000-0000-0000-0000-000000000006','student@example.test','{"full_name":"New Student","full_name":"Duplicate","account_type_request":"STUDENT","school_id":"cbsh","student_id":"1206"}'),
 ('00000000-0000-0000-0000-000000000007','teacher2@example.test','{"full_name":"New Teacher","account_type_request":"TEACHER","school_id":"cbsh"}');
select test_assert((select student_id is null and account_type='EXTERNAL' from profiles where id='00000000-0000-0000-0000-000000000005'),'external requires no student number');
select test_assert((select not affiliation_verified and not is_admin and not is_approved from profiles where id='00000000-0000-0000-0000-000000000007'),'teacher request has no privileges');
do $$begin
 begin insert into auth.users(id,email,raw_user_meta_data) values(gen_random_uuid(),'bad@example.test','{"full_name":"Bad school","account_type_request":"STUDENT","school_id":"other","student_id":"1301"}');raise exception 'expected bad school rejection';exception when check_violation then null;end;
 begin insert into auth.users(id,email,raw_user_meta_data) values(gen_random_uuid(),'bad2@example.test','{"full_name":"Bad number","account_type_request":"STUDENT","school_id":"cbsh","student_id":"9999"}');raise exception 'expected bad ID rejection';exception when check_violation then null;end;
 begin insert into auth.users(id,email,raw_user_meta_data) values(gen_random_uuid(),'bad3@example.test','{"full_name":"Duplicate","account_type_request":"STUDENT","school_id":"cbsh","student_id":"1206"}');raise exception 'expected duplicate ID rejection';exception when unique_violation then null;end;
 begin insert into auth.users(id,email,raw_user_meta_data) values(gen_random_uuid(),'external@example.test','{"full_name":"Duplicate external","account_type_request":"EXTERNAL"}');raise exception 'expected duplicate email rejection';exception when unique_violation then null;end;
end$$;
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000005',false);
select test_assert(not cbgpu_can_use_resource('gpu'),'external default denied');
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000006',false);
select test_assert(not cbgpu_can_use_resource('gpu'),'pending student denied');
-- Legacy verified sponsor requests a time-limited, resource-scoped grant.
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000002',false);
select cbgpu_account_api('guest_create',jsonb_build_object('external_user_id','00000000-0000-0000-0000-000000000005','project_name','Synthetic','purpose','Research','resources',jsonb_build_array('gpu'),'starts_at',now()+interval '1 day','ends_at',now()+interval '2 days'));
-- Sponsor cannot approve their own request, even if subsequently made admin.
update profiles set is_admin=true where id='00000000-0000-0000-0000-000000000002';
do $$declare rid uuid;begin select id into rid from cbgpu_accounts.guest_requests;begin perform cbgpu_account_api('guest_update',jsonb_build_object('id',rid,'status','APPROVED'));raise exception 'expected self approval rejection';exception when insufficient_privilege then null;end;end$$;
update profiles set is_admin=false where id='00000000-0000-0000-0000-000000000002';
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000001',false);
select cbgpu_account_api('guest_update',jsonb_build_object('id',(select id from cbgpu_accounts.guest_requests),'status','APPROVED'));
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000005',false);
select test_assert(not cbgpu_can_use_resource('gpu'),'future grant denied');
update cbgpu_accounts.guest_requests set starts_at=now()-interval '1 day';
select test_assert(cbgpu_can_use_resource('gpu') and not cbgpu_can_use_resource('study'),'grant only requested resource');
update cbgpu_accounts.guest_requests set ends_at=now()-interval '1 hour';
select test_assert(not cbgpu_can_use_resource('gpu'),'expired grant denied');
update cbgpu_accounts.guest_requests set ends_at=now()+interval '1 day';
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000001',false);
select cbgpu_account_api('guest_update',jsonb_build_object('id',(select id from cbgpu_accounts.guest_requests),'status','REVOKED'));
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000005',false);
select test_assert(not cbgpu_can_use_resource('gpu'),'revocation immediate with same token');
-- Admin cannot silently run stale or unconfirmed previews.
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000001',false);
-- Pending registrations have valid IDs; teacher pending remains unchanged.
update profiles set student_id_school_year=2025 where account_type='STUDENT';
do $$declare p jsonb;begin p:=cbgpu_account_api('rollover_preview','{"school_id":"cbsh","school_year":2027}');
 perform test_assert(not (p->>'blocked')::boolean,'valid rollover preview');
 begin perform cbgpu_account_api('rollover_execute','{"school_id":"cbsh","school_year":2027,"confirmation":"wrong","preview_token":"wrong"}');raise exception 'expected confirmation rejection';exception when check_violation then null;end;
 perform cbgpu_account_api('rollover_execute',p);
end$$;
select test_assert((select account_type='STUDENT' and account_status='REVERIFICATION_REQUIRED' and student_id is null and not affiliation_verified from profiles where id='00000000-0000-0000-0000-000000000001'),'grade one reverify');
select test_assert((select account_status='REVERIFICATION_REQUIRED' and student_id is null from profiles where id='00000000-0000-0000-0000-000000000002'),'grade two reverify');
select test_assert((select account_type='EXTERNAL' and student_id is null and not affiliation_verified from profiles where id='00000000-0000-0000-0000-000000000003'),'grade three external');
select test_assert((select account_type='TEACHER' and account_status='ACTIVE' from profiles where id='00000000-0000-0000-0000-000000000004'),'teacher unchanged');
select test_assert((select account_type='EXTERNAL' and account_status='ACTIVE' from profiles where id='00000000-0000-0000-0000-000000000005'),'external unchanged');
select test_assert((select body='retained' and owner_id='00000000-0000-0000-0000-000000000003' from test_projects),'graduated ownership retained');
select test_assert((select count(*)=1 from test_members),'membership retained');
select test_assert((select count(*)=7 from profiles),'all identities retained');
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000002',false);
select test_assert(not cbgpu_can_use_resource('gpu'),'rollover invalidates old token resource privilege');
do $$begin begin perform cbgpu_account_api('reverify','{"student_id":"2202","school_year":2027}');raise exception 'expected grade rejection';exception when check_violation then null;end;end$$;
select cbgpu_account_api('reverify','{"student_id":"3202","school_year":2027}');
select test_assert((select account_status='PENDING_VERIFICATION' and student_id='3202' from profiles where id='00000000-0000-0000-0000-000000000002'),'new number awaits evidence review');
select test_assert(not cbgpu_can_use_resource('gpu'),'number alone never verifies affiliation');
-- A separate existing teacher administrator performs a documented review.
update profiles set is_admin=true where id='00000000-0000-0000-0000-000000000004';
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000004',false);
select cbgpu_account_api('account_update','{"id":"00000000-0000-0000-0000-000000000002","approve_affiliation":true}');
do $$begin begin perform cbgpu_account_api('rollover_execute','{"school_id":"cbsh","school_year":2027}');raise exception 'expected duplicate rollover rejection';exception when unique_violation then null;end;end$$;
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000002',false);
select test_assert(cbgpu_can_use_resource('gpu'),'review restores resource privilege');
-- Browser direct profile writes cannot forge protected fields, even under legacy policies.
set role authenticated;
do $$begin begin update profiles set account_type='TEACHER' where id=auth.uid();raise exception 'expected direct privilege rejection';exception when insufficient_privilege then null;end;end$$;
reset role;
select 'ACCOUNT_WORKFLOW_TESTS_PASSED';
-- Failure inside a rollover transaction must restore every changed account/audit row.
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000004',false);
create function public.test_fail_rollover() returns trigger language plpgsql as $$begin raise exception 'synthetic storage failure';end$$;
create trigger test_fail_rollover before insert on cbgpu_accounts.rollovers for each row execute function public.test_fail_rollover();
-- Reverified students and grade-one admin are assigned synthetic next-year valid IDs.
update profiles set student_id='1201',account_status='ACTIVE',affiliation_verified=true where id='00000000-0000-0000-0000-000000000001';
update profiles set student_id='1306',account_status='ACTIVE',affiliation_verified=true where id='00000000-0000-0000-0000-000000000006';
do $$declare before_profiles jsonb;before_audit bigint;p jsonb;begin
 select jsonb_agg(to_jsonb(x) order by id) into before_profiles from profiles x;
 select count(*) into before_audit from cbgpu_accounts.audit;
 p:=cbgpu_account_api('rollover_preview','{"school_id":"cbsh","school_year":2028}');
 p:=cbgpu_account_api('rollover_execute',p);
 perform test_assert(p->>'error'='ROLLOVER_FAILED','failed execution returns structured failure');
 perform test_assert((select count(*)=1 from cbgpu_accounts.rollover_failures),'failed execution history persists');
 perform test_assert(before_profiles=(select jsonb_agg(to_jsonb(x) order by id) from profiles x),'failure rolls back every account');
 perform test_assert(before_audit=(select count(*) from cbgpu_accounts.audit),'failure rolls back audit transaction');
end$$;
drop trigger test_fail_rollover on cbgpu_accounts.rollovers;
select 'ACCOUNT_TRANSACTION_ROLLBACK_PASSED';
select test_assert(not has_function_privilege('authenticated','public.cbgpu_service_can_use_resource(uuid,text,text)','EXECUTE'),'service oracle not exposed to authenticated');
select test_assert(not has_function_privilege('anon','public.cbgpu_service_can_use_resource(uuid,text,text)','EXECUTE'),'service oracle not exposed to anonymous');
select test_assert(has_function_privilege('service_role','public.cbgpu_service_can_use_resource(uuid,text,text)','EXECUTE'),'trusted worker oracle granted');

-- Native notebook grants do not inherit administrators' global SELECT visibility.
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000004',false);
select set_config('request.jwt.claim.session_id','70000000-0000-0000-0000-000000000004',false);
insert into auth.sessions values('70000000-0000-0000-0000-000000000004','00000000-0000-0000-0000-000000000004',null);
insert into public.reservations(user_id,gpu_id,status,start_time,end_time)values('00000000-0000-0000-0000-000000000002',1,'approved',now()-interval '1 hour',now()+interval '1 hour');
do $$begin begin perform public.cbgpu_notebook_grant(1);raise exception 'expected other user notebook denial';exception when insufficient_privilege then null;end;end$$;
insert into public.reservations(user_id,gpu_id,status,start_time,end_time)values('00000000-0000-0000-0000-000000000004',1,'approved',now()-interval '1 hour',now()+interval '1 hour');
select test_assert(public.cbgpu_notebook_grant(1)->>'owner'='00000000-0000-0000-0000-000000000004','notebook owner is actual caller');
update public.reservations set status='cancelled' where user_id='00000000-0000-0000-0000-000000000004';
do $$begin begin perform public.cbgpu_notebook_grant(1);raise exception 'expected booking revocation denial';exception when insufficient_privilege then null;end;end$$;
update public.reservations set status='approved' where user_id='00000000-0000-0000-0000-000000000004';
delete from auth.sessions;
do $$begin begin perform public.cbgpu_notebook_grant(1);raise exception 'expected session revocation denial';exception when insufficient_privilege then null;end;end$$;
select 'NOTEBOOK_SESSION_OWNER_GRANTS_PASSED';
