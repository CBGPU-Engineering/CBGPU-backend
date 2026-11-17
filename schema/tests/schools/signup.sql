-- Real Auth/user/profile triggers. Execute only in the isolated same-image clone.
begin;
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,configuration,features) values('fixture-other-school','Synthetic Other School','synthetic other school','Synthetic Other School','HIGH_SCHOOL','{"join_policy":"APPROVAL_REQUIRED","student_id_policy":"SCHOOL_DEFINED","teacher_policy":"MANUAL_REVIEW","community_policy":"MEMBERS_ONLY"}',array['projects','contests']);
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at) values
('f0000000-0000-4000-8000-000000000001','synthetic-student@fixture.invalid','{"full_name":"Synthetic Student","account_type_request":"EXTERNAL","school_join_request":{"school_id":"fixture-other-school","type":"STUDENT","student_number":"AB-2026-100"}}','{}','authenticated','authenticated','fixture-no-password',null,now(),now()),
('f0000000-0000-4000-8000-000000000002','synthetic-teacher@fixture.invalid','{"full_name":"Synthetic Teacher","account_type_request":"EXTERNAL","school_join_request":{"school_id":"cbsh","type":"TEACHER","department":"Science"}}','{}','authenticated','authenticated','fixture-no-password',null,now(),now()),
('f0000000-0000-4000-8000-000000000003','synthetic-applicant@fixture.invalid','{"full_name":"Synthetic Applicant","account_type_request":"EXTERNAL","school_setup_request":true}','{}','authenticated','authenticated','fixture-no-password',null,now(),now());
do $$begin
 if(select count(*) from public.profiles where id in('f0000000-0000-4000-8000-000000000001','f0000000-0000-4000-8000-000000000002','f0000000-0000-4000-8000-000000000003') and account_type='EXTERNAL' and not affiliation_verified and not is_admin and not is_approved)<>3 then raise exception 'GLOBAL_EXTERNAL_SIGNUP_FAILED';end if;
 if not exists(select 1 from cbgpu_schools.memberships where user_id='f0000000-0000-4000-8000-000000000001' and school_id='fixture-other-school' and membership_type='STUDENT' and student_number='AB-2026-100' and status='PENDING_VERIFICATION' and not verified and school_roles='{}') then raise exception 'PENDING_STUDENT_INTENT_FAILED';end if;
 if not exists(select 1 from cbgpu_schools.memberships where user_id='f0000000-0000-4000-8000-000000000002' and school_id='cbsh' and membership_type='TEACHER' and status='PENDING_VERIFICATION' and not verified and school_roles='{}') then raise exception 'PENDING_TEACHER_INTENT_FAILED';end if;
 if exists(select 1 from cbgpu_schools.memberships where user_id='f0000000-0000-4000-8000-000000000003') then raise exception 'APPLICANT_AUTO_AFFILIATION';end if;
 if cbgpu_accounts.can_user_use_resource('f0000000-0000-4000-8000-000000000001','gpu','cbsh') or cbgpu_accounts.can_user_use_resource('f0000000-0000-4000-8000-000000000001','contests','fixture-other-school') then raise exception 'SIGNUP_AUTO_RESOURCE_GRANT';end if;
end$$;
rollback;
