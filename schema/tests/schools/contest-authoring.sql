begin;
create or replace function pg_temp.assert_true(value boolean,label text) returns void language plpgsql as $$begin if value is distinct from true then raise exception '%',label;end if;end$$;
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,configuration,features)
values('fixture-contest-author','Synthetic author school','synthetic author school','Synthetic author school','HIGH_SCHOOL','{}',array['contests']);
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)
values('f7100000-0000-4000-8000-000000000001','synthetic-contest-author@fixture.invalid','{"full_name":"Synthetic organizer","account_type_request":"EXTERNAL"}','{}','authenticated','authenticated','fixture-no-password',now(),now(),now());
insert into auth.sessions(id,user_id) values('f7200000-0000-4000-8000-000000000001','f7100000-0000-4000-8000-000000000001');
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,school_roles)
values('f7100000-0000-4000-8000-000000000001','fixture-contest-author','EXTERNAL','ACTIVE',true,array['SCHOOL_OWNER','SCHOOL_ADMIN']);
select set_config('request.jwt.claims','{"sub":"f7100000-0000-4000-8000-000000000001","session_id":"f7200000-0000-4000-8000-000000000001","role":"authenticated"}',true);
select pg_temp.assert_true(public.cbgpu_can_create_school_contest('fixture-contest-author'),'external school owner may organize');
select pg_temp.assert_true(public.cbgpu_can_create_contest(),'presentation authoring allowed');
select pg_temp.assert_true(not public.cbgpu_can_create_school_contest('cbsh'),'other-school authoring denied');
select pg_temp.assert_true(not cbgpu_accounts.can_user_use_resource(auth.uid(),'gpu','cbsh'),'school owner cannot borrow legacy GPU');
set local role authenticated;
insert into public.contests(id,host_id,title,fields,school_id,access_scope)
values('f7300000-0000-4000-8000-000000000001',auth.uid(),'Synthetic owner event','[{"id":"f7400000-0000-4000-8000-000000000001","type":"text","label":"Fixture answer","description":"","required":false}]','fixture-contest-author','SCHOOL');
reset role;
update cbgpu_schools.schools set features='{}' where id='fixture-contest-author';
select pg_temp.assert_true(not public.cbgpu_can_create_school_contest('fixture-contest-author'),'disabled feature cannot be used for authoring');
update cbgpu_schools.schools set features=array['contests'] where id='fixture-contest-author';
update cbgpu_schools.memberships set status='SUSPENDED' where school_id='fixture-contest-author';
select pg_temp.assert_true(not public.cbgpu_can_create_contest(),'old session loses new-event permission after membership revoked');
select pg_temp.assert_true((select account_type='EXTERNAL' and not is_admin and not is_approved from public.profiles where id=auth.uid()),'authoring does not promote identity');
rollback;
select 'SCOPED_CONTEST_AUTHORING_TESTS_PASSED';
