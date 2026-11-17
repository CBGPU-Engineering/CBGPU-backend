begin;
create or replace function pg_temp.assert_true(value boolean,label text) returns void language plpgsql as $$begin if value is distinct from true then raise exception '%',label;end if;end$$;
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,configuration,features)
values('fixture-preference-other','Synthetic preference school','synthetic preference school','Synthetic preference school','HIGH_SCHOOL','{}',array['community']);
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)
select ('f9100000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-preference-'||x||'@fixture.invalid','{"full_name":"Synthetic preference","account_type_request":"EXTERNAL"}','{}','authenticated','authenticated','fixture-no-password',now(),(select completed_at-interval '1 day' from cbgpu_schools.migration_state where key='legacy-cbsh-v1'),now() from generate_series(1,4)x;
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,created_at)
select ('f9100000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'cbsh','STUDENT',case when x=3 then 'PENDING_VERIFICATION' else 'ACTIVE' end,x<>3,(select completed_at+case when x=4 then interval '1 second' else interval '-1 day' end from cbgpu_schools.migration_state where key='legacy-cbsh-v1') from generate_series(1,4)x;
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified)values('f9100000-0000-4000-8000-000000000002','fixture-preference-other','STUDENT','ACTIVE',true);
insert into cbgpu_schools.preferences values('f9100000-0000-4000-8000-000000000002','fixture-preference-other');
create temp table preference_members_before as select to_jsonb(m) value from cbgpu_schools.memberships m where m.user_id::text like 'f9100000%';
delete from cbgpu_schools.migration_state where key='legacy-cbsh-preference-v1';
select cbgpu_schools.seed_legacy_school_preference();
select pg_temp.assert_true((select school_id='cbsh' from cbgpu_schools.preferences where user_id='f9100000-0000-4000-8000-000000000001'),'historical verified account retains CBsh without a device fallback');
select pg_temp.assert_true((select school_id='fixture-preference-other' from cbgpu_schools.preferences where user_id='f9100000-0000-4000-8000-000000000002'),'existing deliberate preference is not overwritten');
select pg_temp.assert_true(not exists(select 1 from cbgpu_schools.preferences where user_id in('f9100000-0000-4000-8000-000000000003','f9100000-0000-4000-8000-000000000004')),'pending and post-migration membership never receive a default');
delete from cbgpu_schools.preferences where user_id='f9100000-0000-4000-8000-000000000001';
select pg_temp.assert_true(cbgpu_schools.seed_legacy_school_preference()=0,'replay never reselects a cleared preference');
select pg_temp.assert_true(not exists((select to_jsonb(m) from cbgpu_schools.memberships m where m.user_id::text like 'f9100000%') except select value from preference_members_before),'seeding changes no affiliation or permission');
rollback;
select 'LEGACY_ACCOUNT_PREFERENCE_TESTS_PASSED';
