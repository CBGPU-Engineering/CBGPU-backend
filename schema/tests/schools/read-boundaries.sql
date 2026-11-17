-- Real login identities, not SET ROLE under a superuser session, exercise the
-- only trusted sessionless exception. Entire fixture is rolled back.
begin;
create role cbgpu_school_fixture_auth login noinherit nosuperuser nobypassrls;
grant authenticated to cbgpu_school_fixture_auth;
create role cbgpu_school_fixture_corpus login noinherit nosuperuser nobypassrls;
grant cbgpu_corpus_backend to cbgpu_school_fixture_corpus;
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,region,configuration,features) values('fixture-read-school','Synthetic Read School','synthetic read school','Synthetic Read School','HIGH_SCHOOL','Synthetic region','{"join_policy":"APPROVAL_REQUIRED","student_id_policy":"SCHOOL_DEFINED","teacher_policy":"MANUAL_REVIEW","community_policy":"MEMBERS_ONLY"}',array['materials','projects','contests','community']);
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)
select ('f6000000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-read-'||x||'@fixture.invalid',jsonb_build_object('full_name','Synthetic read '||x,'account_type_request','EXTERNAL','school_join_request',jsonb_build_object('school_id','fixture-read-school','type','STUDENT','student_number','AB-'||x)),'{}','authenticated','authenticated','fixture-no-password',now(),now(),now() from generate_series(1,2)x;
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at) values('f6000000-0000-4000-8000-000000000003','synthetic-read-3@fixture.invalid','{"full_name":"Synthetic other school","account_type_request":"EXTERNAL"}','{}','authenticated','authenticated','fixture-no-password',now(),now(),now());
insert into auth.sessions(id,user_id) select ('f6100000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,('f6000000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid from generate_series(1,3)x;
update cbgpu_schools.memberships set status='ACTIVE',verified=true where school_id='fixture-read-school';
update cbgpu_schools.memberships set school_roles=array['SCHOOL_OWNER','SCHOOL_ADMIN'] where school_id='fixture-read-school' and user_id='f6000000-0000-4000-8000-000000000001';
set session authorization cbgpu_school_fixture_auth;
set role authenticated;
select set_config('request.jwt.claims','{"role":"authenticated","sub":"f6000000-0000-4000-8000-000000000001","session_id":"f6100000-0000-4000-8000-000000000001"}',true);
do $$begin
 if exists(select 1 from public.profiles where id in('f6000000-0000-4000-8000-000000000002','f6000000-0000-4000-8000-000000000003')) then raise exception 'SCHOOL_ADMIN_RAW_PEER_PROFILE_LEAK';end if;
 if not exists(select 1 from public.profiles where id=auth.uid()) then raise exception 'OWN_PROFILE_NOT_VISIBLE';end if;
 if public.cbgpu_self_session_status()->>'approved'<>'false' then raise exception 'SCHOOL_B_BORROWED_LEGACY_CBSH';end if;
end$$;
insert into public.shared_materials(id,uploader_id,title,file_name,size_bytes,status,school_id,access_scope) values('f6200000-0000-4000-8000-000000000001',auth.uid(),'Synthetic owned material','fixture.txt',7,'pending','fixture-read-school','SCHOOL');
update public.shared_materials set status='ready' where id='f6200000-0000-4000-8000-000000000001';
insert into public.study_posts(id,user_id,subject,title,school_id,access_scope) values('f6400000-0000-4000-8000-000000000001',auth.uid(),'Physics','Synthetic private group','fixture-read-school','PRIVATE');
select set_config('request.jwt.claims','{"role":"authenticated","sub":"f6000000-0000-4000-8000-000000000002","session_id":"f6100000-0000-4000-8000-000000000002"}',true);
do $$begin
 if not exists(select 1 from public.shared_materials where id='f6200000-0000-4000-8000-000000000001') then raise exception 'VERIFIED_NEW_SCHOOL_MATERIAL_READ_BROKEN';end if;
 if exists(select 1 from public.study_posts where id='f6400000-0000-4000-8000-000000000001') then raise exception 'PRIVATE_GROUP_VISIBLE_TO_SCHOOL_PEER';end if;
 if exists(select 1 from public.cbgpu_study_member_profiles(array['f6000000-0000-4000-8000-000000000001'::uuid])) then raise exception 'INVISIBLE_POST_PROFILE_LABEL_LEAK';end if;
 begin insert into public.study_members(post_id,user_id) values('f6400000-0000-4000-8000-000000000001',auth.uid());raise exception 'PRIVATE_GROUP_SELF_JOIN_ALLOWED';exception when insufficient_privilege then null;end;
end$$;
reset role;
reset session authorization;
delete from auth.sessions where id='f6100000-0000-4000-8000-000000000002';
set session authorization cbgpu_school_fixture_auth;
set role authenticated;
select set_config('request.jwt.claims','{"role":"authenticated","sub":"f6000000-0000-4000-8000-000000000002","session_id":"f6100000-0000-4000-8000-000000000002","cbgpu_corpus_backend":true}',true);
select set_config('cbgpu.corpus.account_v2','on',true);
do $$begin
 if exists(select 1 from public.shared_materials where id='f6200000-0000-4000-8000-000000000001') or exists(select 1 from public.profiles where id=auth.uid()) then raise exception 'REVOKED_JWT_OR_FAKE_CORPUS_CLAIMS_READ';end if;
 if public.cbgpu_self_session_status()->>'valid'<>'false' then raise exception 'REVOKED_SELF_SESSION_ACCEPTED';end if;
end$$;
reset role;
reset session authorization;
-- A previously authorized native job retains only its owner's existing content,
-- not an expired school entitlement. No user session is fabricated by the oracle.
delete from auth.sessions where id='f6100000-0000-4000-8000-000000000001';
update cbgpu_schools.memberships set status='SUSPENDED',verified=false where user_id='f6000000-0000-4000-8000-000000000001';
set session authorization cbgpu_school_fixture_corpus;
set role cbgpu_corpus_backend;
do $$declare result jsonb;begin
 result:=cbgpu_corpus_auth.sources_for_owner('f6000000-0000-4000-8000-000000000001',jsonb_build_array('material:f6200000-0000-4000-8000-000000000001'),true);
 if jsonb_array_length(result)<>1 then raise exception 'TRUSTED_JOB_OWN_MATERIAL_AFTER_EXPIRY_BROKEN';end if;
 begin perform count(*) from public.shared_materials;raise exception 'CORPUS_BACKEND_RAW_TABLE_READ_ALLOWED';exception when insufficient_privilege then null;end;
end$$;
reset role;
reset session authorization;
rollback;
