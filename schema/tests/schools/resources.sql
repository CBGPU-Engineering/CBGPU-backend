-- Restore synthetic owner session removed by the preceding session-revocation test.
insert into auth.sessions(id,user_id) values('10000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000002');
create temporary table school_b as select school_id id from cbgpu_schools.applications where status='APPROVED' limit 1;
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified) select '00000000-0000-4000-8000-000000000005',id,'STUDENT','ACTIVE',true from school_b;
insert into public.contests(id,host_id,title) values('20000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000001','CBsh only');
insert into public.contests(id,host_id,title,school_id) select '20000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000002','B only',id from school_b;
insert into public.research_notes(user_id,is_shared,title) values('00000000-0000-4000-8000-000000000001',true,'CBsh shared');
insert into public.shared_materials(uploader_id,status,title) values('00000000-0000-4000-8000-000000000001','ready','CBsh material');
select set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000002","session_id":"10000000-0000-4000-8000-000000000002"}',false);
-- School owner administration does not turn EXTERNAL into a GPU-enabled student.
select public.assert_true(not cbgpu_accounts.can_user_use_resource(auth.uid(),'gpu','cbsh'),'B owner cannot borrow CBshGPU');
select public.assert_true(not cbgpu_schools.features_allowed((select id from school_b),array['school_life']),'CBsh-specific module cannot be enabled for B');
set role authenticated;
select public.assert_true((select count(*)=1 from public.contests),'contest listing isolated actual SELECT');
select public.assert_true((select count(*)=0 from public.research_notes),'shared notes isolated actual SELECT');
select public.assert_true((select count(*)=0 from public.shared_materials),'materials isolated actual SELECT');
reset role;
select set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000005","session_id":"10000000-0000-4000-8000-000000000005"}',false);
-- User5 has active student memberships in both schools. Revoke only CBsh.
update cbgpu_schools.memberships set status='SUSPENDED',verified=false where user_id=auth.uid() and school_id='cbsh';
select public.assert_true(cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000002',auth.uid()),'B student may register B');
select public.assert_true(not cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000001',auth.uid()),'B student cannot register CBsh');
select public.assert_true(not cbgpu_accounts.can_user_use_resource(auth.uid(),'gpu',(select id from school_b)),'noGPU school not configured');
-- Scoped community reads, explicit nationwide scope, owner-muted messages.
select public.cbgpu_school_extensions_api('message_create',jsonb_build_object('scope','SCHOOL','school_id',(select id from school_b),'body','School B fixture message'));
select public.cbgpu_school_extensions_api('message_create','{"scope":"NATIONWIDE","body":"Nationwide fixture message"}');
select public.assert_true(jsonb_array_length(public.cbgpu_school_extensions_api('messages',jsonb_build_object('scope','SCHOOL','school_id',(select id from school_b)))->'items')=1,'school chat visible');
do $$begin begin perform public.cbgpu_school_extensions_api('messages','{"scope":"SCHOOL","school_id":"cbsh"}');raise exception 'cross-school chat read allowed';exception when insufficient_privilege then null;end;end$$;
select public.cbgpu_school_extensions_api('feature_request_create',jsonb_build_object('school_id',(select id from school_b),'feature_key','test-module','title','Synthetic request','description','Please review, not instant deployment'));
select public.assert_true((select count(*)=1 from cbgpu_schools.mail_outbox where recipient='info@cbgpu.com' and status='PENDING'),'durable mail queued not false sent');
update cbgpu_schools.memberships set status='SUSPENDED' where user_id=auth.uid() and school_id=(select id from school_b);
do $$begin begin perform public.cbgpu_school_extensions_api('messages','{"scope":"NATIONWIDE"}');raise exception 'revoked nationwide membership allowed';exception when insufficient_privilege then null;end;end$$;
-- Permanent owner access survives loss of all school resources.
insert into public.cpu_projects(user_id,title,school_id) select auth.uid(),'Permanent project',id from school_b;
set role authenticated;
select public.assert_true((select count(*)=1 from public.cpu_projects),'owned project survives resource revocation');
insert into public.research_notes(user_id,is_shared,title) values(auth.uid(),false,'Personal after graduation');
select public.assert_true((select count(*)=1 from public.research_notes),'personal unshared notes survive resource revocation');
do $$begin begin update public.research_notes set is_shared=true where user_id=auth.uid();raise exception 'revoked school share allowed';exception when insufficient_privilege then null;end;end$$;
reset role;
select 'MULTISCHOOL_RESOURCE_TESTS_PASSED';
-- Least-privilege mail worker receives one fixed-address payload, never table access.
set role cbgpu_school_mail_worker;
do $$declare lease uuid:=gen_random_uuid();item jsonb;begin
 item:=cbgpu_schools.claim_mail(lease);perform public.assert_true(item->>'recipient'='info@cbgpu.com','mail fixed recipient');
 perform public.assert_true(not cbgpu_schools.ack_mail((item->>'id')::uuid,gen_random_uuid(),true),'foreign lease cannot acknowledge');
 perform public.assert_true(cbgpu_schools.ack_mail((item->>'id')::uuid,lease,true),'owned lease acknowledgement');
 perform public.assert_true(cbgpu_schools.claim_mail(gen_random_uuid()) is null,'delivered intent not claimed twice');
 begin perform count(*) from cbgpu_schools.mail_outbox;raise exception 'worker raw outbox readable';exception when insufficient_privilege then null;end;
end$$;
reset role;
-- SENT means provider accepted, not guaranteed inbox delivery.
update cbgpu_schools.mail_outbox set status='FAILED',attempts=1,first_attempt_at=now()-interval '24 hours',next_attempt_at=now()-interval '1 hour';
select public.assert_true(cbgpu_schools.claim_mail(gen_random_uuid()) is null,'expired provider-idempotency window not resent');
select public.assert_true((select last_error_code='IDEMPOTENCY_WINDOW_EXPIRED' from cbgpu_schools.mail_outbox),'expired mail retained for manual review');
