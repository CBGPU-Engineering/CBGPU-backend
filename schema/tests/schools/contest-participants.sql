begin;
create or replace function pg_temp.assert_true(value boolean,label text) returns void language plpgsql as $$begin if value is distinct from true then raise exception '%',label;end if;end$$;
select pg_temp.assert_true(not exists(select 1 from public.contests where participant_policy<>'ALL_ELIGIBLE'),'existing contests retain original eligibility');
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)
select ('f6100000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-participant-'||x||'@fixture.invalid',jsonb_build_object('full_name','Synthetic participant','account_type_request','EXTERNAL'),'{}','authenticated','authenticated','fixture-no-password',now(),now(),now() from generate_series(1,3)x;
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified) values('f6100000-0000-4000-8000-000000000003','cbsh','STUDENT','ACTIVE',true);
insert into public.contests(id,host_id,title,fields,access_scope,participant_policy)
values('20000000-0000-4000-8000-000000000091','f6100000-0000-4000-8000-000000000001','National high school fixture','[{"id":"f6200000-0000-4000-8000-000000000001","type":"text","label":"Fixture answer","description":"","required":false}]','PUBLIC','HIGH_SCHOOL_STUDENTS');
-- One synthetic member's verification is restored only within this rollback.
update cbgpu_schools.memberships set membership_type='STUDENT',verified=true,status='ACTIVE',starts_at=now()-interval '1 day',ends_at=null where user_id='f6100000-0000-4000-8000-000000000003' and school_id='cbsh';
select pg_temp.assert_true(cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000091','f6100000-0000-4000-8000-000000000003'),'verified high school student eligible');
select pg_temp.assert_true(not cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000091','f6100000-0000-4000-8000-000000000002'),'school owner external not student');
update cbgpu_schools.memberships set membership_type='TEACHER' where user_id='f6100000-0000-4000-8000-000000000003' and school_id='cbsh';
select pg_temp.assert_true(not cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000091','f6100000-0000-4000-8000-000000000003'),'teacher not student');
update cbgpu_schools.memberships set membership_type='STUDENT',verified=false where user_id='f6100000-0000-4000-8000-000000000003' and school_id='cbsh';
select pg_temp.assert_true(not cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000091','f6100000-0000-4000-8000-000000000003'),'unverified student denied');
update cbgpu_schools.memberships set verified=true,ends_at=now()-interval '1 second' where user_id='f6100000-0000-4000-8000-000000000003' and school_id='cbsh';
select pg_temp.assert_true(not cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000091','f6100000-0000-4000-8000-000000000003'),'expired affiliation denied');
select pg_temp.assert_true(cbgpu_schools.scope_read(null,'cbsh','PUBLIC','{}'),'public description remains browsable');
rollback;
select 'CONTEST_PARTICIPANT_POLICY_TESTS_PASSED';
