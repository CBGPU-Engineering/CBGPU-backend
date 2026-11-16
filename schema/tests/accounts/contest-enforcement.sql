\set ON_ERROR_STOP on
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000013',false);
set role authenticated;
do $$begin
 begin perform public.test_definer_register(auth.uid());raise exception 'SECDEF external bypass';exception when insufficient_privilege then null;end;
 begin insert into event_registrations(contest_id,user_id,status)values(gen_random_uuid(),auth.uid(),'pending');raise exception 'REST external bypass';exception when insufficient_privilege then null;end;
 begin insert into contest_submissions(contest_id,user_id)values(gen_random_uuid(),auth.uid());raise exception 'submission bypass';exception when insufficient_privilege then null;end;
 begin insert into storage.objects(bucket_id)values('contest-submissions');raise exception 'upload bypass';exception when insufficient_privilege then null;end;
end$$;
reset role;
insert into cbgpu_accounts.guest_requests(external_user_id,requester_id,school_id,project_name,purpose,resources,starts_at,ends_at,status)values('00000000-0000-0000-0000-000000000011','00000000-0000-0000-0000-000000000010','cbsh','fixture','fixture',array['contests'],now()-interval '1 hour',now()+interval '1 hour','APPROVED');
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000010',false);
set role authenticated;
select public.test_definer_register(auth.uid());
insert into event_registrations(contest_id,user_id,status)values('10000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000011','waitlisted'),('10000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000012','waitlisted');
reset role;
update cbgpu_accounts.guest_requests set ends_at=now()-interval '1 second';
create trigger promote after update on event_registrations for each row execute function public.promote_event_waitlist();
set role authenticated;
update event_registrations set status='cancelled' where user_id=auth.uid();
select test_assert((select status='confirmed' from event_registrations where user_id='00000000-0000-0000-0000-000000000012'),'eligible waitlist promoted');
select test_assert((select status='waitlisted' from event_registrations where user_id='00000000-0000-0000-0000-000000000011'),'expired guest retained without promotion');
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000011',false);
update event_registrations set status='cancelled' where user_id=auth.uid();
reset role;
-- Revocation removes API/resource permission even while a JWT remains cryptographically valid.
delete from auth.sessions where user_id='00000000-0000-0000-0000-000000000010';
select set_config('request.jwt.claim.sub','00000000-0000-0000-0000-000000000010',false);
set role authenticated;
select test_assert(not public.cbgpu_can_use_resource('contests'),'revoked session denied');
do $$begin begin update event_registrations set status='cancelled' where user_id=auth.uid();raise exception 'revoked session direct cancel bypass';exception when insufficient_privilege then null;end;end$$;
do $$begin begin perform public.cbgpu_account_api('self');raise exception 'revoked session account API bypass';exception when insufficient_privilege then null;end;end$$;
select 'ACCOUNT_CONTEST_BOUNDARIES_PASSED';
