begin;
-- Restore a valid reviewer session after the previous fixture revoked its sessions.
insert into auth.sessions(id,user_id) values('00000000-0000-0000-0000-000000000004','00000000-0000-0000-0000-000000000004') on conflict do nothing;
select set_config('request.jwt.claim.session_id','',true);
insert into auth.users(id,email,email_confirmed_at,raw_user_meta_data) values
 ('f3510000-0000-4000-8000-000000000001','signup-approve@example.test',now(),'{"full_name":"New student","student_id":"1209"}'),
 ('f3510000-0000-4000-8000-000000000002','signup-reject@example.test',now(),'{"full_name":"New external user","account_type_request":"EXTERNAL"}'),
 ('f3510000-0000-4000-8000-000000000003','signup-unconfirmed@example.test',null,'{"full_name":"No confirmed email","account_type_request":"EXTERNAL"}');
insert into test_projects values('f3510000-0000-4000-8000-000000000004','f3510000-0000-4000-8000-000000000002','still here');
select test_assert((select count(*)=3 from profiles where id::text like 'f351%' and is_approved is null and not affiliation_verified and not is_admin),'new accounts are available and pending, with no verified affiliation');
select set_config('request.jwt.claim.sub','f3510000-0000-4000-8000-000000000001',true);
set local role authenticated;
select test_assert((cbgpu_account_api('self')->'account'->>'account_status')='PENDING_VERIFICATION' and not cbgpu_can_use_resource('gpu'),'student account works while school resources require affiliation');
do $$begin
 begin perform public.cbgpu_review_signup('f3510000-0000-4000-8000-000000000002','REJECT');raise exception 'expected forbidden student decision';exception when insufficient_privilege then null;end;
end$$;
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000004',true);
do $$begin
 begin perform public.cbgpu_review_signup('00000000-0000-0000-0000-000000000004','APPROVE');raise exception 'expected forbidden self decision';exception when insufficient_privilege then null;end;
 begin perform public.cbgpu_review_signup('f3510000-0000-4000-8000-000000000003','APPROVE');raise exception 'expected unconfirmed signup denial';exception when no_data_found then null;end;
end$$;
select public.cbgpu_review_signup('f3510000-0000-4000-8000-000000000001','APPROVE');
do $$begin
 begin perform public.cbgpu_review_signup('f3510000-0000-4000-8000-000000000001','REJECT');raise exception 'expected duplicate review denial';exception when no_data_found then null;end;
end$$;
select public.cbgpu_review_signup('f3510000-0000-4000-8000-000000000002','REJECT');
select set_config('request.jwt.claim.sub','f3510000-0000-4000-8000-000000000002',true);
do $$begin
 begin perform cbgpu_account_api('guest_list');raise exception 'expected suspended account denial';exception when insufficient_privilege then null;end;
end$$;
reset role;
select test_assert((select p.is_approved is true and p.account_status='PENDING_VERIFICATION' and not p.affiliation_verified from profiles p where id='f3510000-0000-4000-8000-000000000001'),'approval only marks the review complete');
select test_assert((select p.is_approved is false and p.account_status='SUSPENDED' from profiles p where id='f3510000-0000-4000-8000-000000000002'),'rejection restricts only its target');
select test_assert(exists(select 1 from test_projects where id='f3510000-0000-4000-8000-000000000004' and body='still here'),'rejection preserves owned content');
select test_assert((select count(*)=2 from cbgpu_accounts.audit where event in('SIGNUP_APPROVE','SIGNUP_REJECT')),'only committed decisions audited');
select test_assert(not has_function_privilege('anon','public.cbgpu_review_signup(uuid,text)','execute'),'anonymous review denied');
rollback;
select 'SIGNUP_REVIEW_PASSED';
