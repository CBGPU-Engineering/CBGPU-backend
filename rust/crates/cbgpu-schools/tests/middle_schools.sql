CREATE FUNCTION pg_temp.assert_true(value boolean,label text) RETURNS void LANGUAGE plpgsql AS $$BEGIN IF value IS DISTINCT FROM true THEN RAISE EXCEPTION '%',label;END IF;END$$;
SELECT pg_temp.assert_true((SELECT count(*)=3358 FROM cbgpu_schools.schools WHERE type='MIDDLE_SCHOOL'),'nationwide official middle catalog complete');
SELECT pg_temp.assert_true(NOT EXISTS(SELECT FROM original_school_rows old FULL JOIN cbgpu_schools.schools s ON s.id=old.id WHERE old.id IS NOT NULL AND old.value IS DISTINCT FROM to_jsonb(s)),'existing school rows unchanged');
SELECT pg_temp.assert_true(NOT EXISTS(SELECT FROM original_membership_rows old FULL JOIN cbgpu_schools.memberships m ON m.id=old.id WHERE old.value IS DISTINCT FROM to_jsonb(m)),'memberships untouched');
SELECT pg_temp.assert_true(NOT EXISTS(SELECT FROM cbgpu_schools.schools s WHERE type='MIDDLE_SCHOOL' AND(status<>'LISTED' OR cardinality(features)>0 OR verified_at IS NOT NULL OR EXISTS(SELECT FROM cbgpu_schools.memberships m WHERE m.school_id=s.id))),'no activation features memberships or roles');
SELECT pg_temp.assert_true((SELECT count(*)=1 FROM cbgpu_schools.directory_import_scopes WHERE scope='KOREAN_MIDDLE_SCHOOLS'),'idempotent scoped source audit');
DO $$DECLARE manifest jsonb; BEGIN
SELECT jsonb_build_object('format_version',1,'scope','KOREAN_HIGH_SCHOOLS','source_sha256',repeat('a',64),'source','https://open.neis.go.kr/portal/data/file/downloadBlobFileData.do?fixture','source_as_of','2026-10-31','school_count',1,'schools',jsonb_build_array(jsonb_build_object('official_id','9999999','official_name','Synthetic high','display_name','Synthetic high','type','HIGH_SCHOOL','source_kind','고등학교','region','충북','address','합성로','contact_phone','','education_office_code','M10','status','LISTED','features','[]'::jsonb,'verified_at',null))) INTO manifest;
PERFORM cbgpu_schools.import_official_directory(manifest);
PERFORM cbgpu_schools.import_official_directory(manifest);
IF (SELECT count(*) FROM cbgpu_schools.schools WHERE official_id='9999999' AND type='HIGH_SCHOOL' AND status='LISTED')<>1 THEN RAISE EXCEPTION 'HIGH_COMPATIBILITY';END IF;
BEGIN PERFORM cbgpu_schools.import_official_directory(jsonb_set(manifest,'{scope}','"UNKNOWN"'));RAISE EXCEPTION 'ACCEPTED_UNKNOWN_SCOPE';EXCEPTION WHEN raise_exception THEN IF SQLERRM<>'INVALID_OFFICIAL_MANIFEST' THEN RAISE;END IF;END;
BEGIN PERFORM cbgpu_schools.import_official_directory(jsonb_set(manifest,'{scope}','"KOREAN_MIDDLE_SCHOOLS"'));RAISE EXCEPTION 'ACCEPTED_TYPE_SCOPE_MISMATCH';EXCEPTION WHEN raise_exception THEN IF SQLERRM<>'INVALID_OFFICIAL_SCHOOL' THEN RAISE;END IF;END;
BEGIN PERFORM cbgpu_schools.import_official_directory(jsonb_set(manifest,'{schools,0,status}','"APPROVED"'));RAISE EXCEPTION 'ACCEPTED_APPROVED_IMPORT';EXCEPTION WHEN raise_exception THEN IF SQLERRM<>'INVALID_OFFICIAL_SCHOOL' THEN RAISE;END IF;END;
BEGIN PERFORM cbgpu_schools.import_official_directory(jsonb_set(manifest,'{schools,0,features}','["gpu_compute"]'));RAISE EXCEPTION 'ACCEPTED_FEATURE_GRANT';EXCEPTION WHEN raise_exception THEN IF SQLERRM<>'INVALID_OFFICIAL_SCHOOL' THEN RAISE;END IF;END;
BEGIN PERFORM cbgpu_schools.import_official_directory(jsonb_set(manifest,'{schools,0,source_kind}','"각종학교"'));RAISE EXCEPTION 'ACCEPTED_UNSPECIFIED_KIND';EXCEPTION WHEN raise_exception THEN IF SQLERRM<>'INVALID_OFFICIAL_SCHOOL_KIND' THEN RAISE;END IF;END;
BEGIN PERFORM cbgpu_schools.import_official_directory(jsonb_set(manifest,'{schools,0,official_id}',to_jsonb((SELECT official_id FROM cbgpu_schools.schools WHERE type='MIDDLE_SCHOOL' LIMIT 1))));RAISE EXCEPTION 'ACCEPTED_TYPE_ID_CONFLICT';EXCEPTION WHEN raise_exception THEN IF SQLERRM<>'OFFICIAL_SCHOOL_TYPE_CONFLICT' THEN RAISE;END IF;END;

END$$;
DO $$DECLARE school_name text;result jsonb;started timestamptz;elapsed numeric;BEGIN
SELECT official_name INTO school_name FROM cbgpu_schools.schools WHERE type='MIDDLE_SCHOOL' ORDER BY id LIMIT 1;
started:=clock_timestamp();result:=public.cbgpu_school_api('directory',jsonb_build_object('q',school_name));elapsed:=extract(epoch FROM clock_timestamp()-started)*1000;
PERFORM pg_temp.assert_true(EXISTS(SELECT FROM jsonb_array_elements(result->'items')r WHERE r->>'type'='MIDDLE_SCHOOL' AND r->>'status'='LISTED'),'canonical public directory includes actual middle type');
RAISE WARNING 'Full middle catalog public search % ms',elapsed;
END$$;
-- Synthetic activated institution only; never activate any imported institution.
INSERT INTO cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,region,configuration,features,status) VALUES('fixture-middle','Synthetic middle','synthetic middle','Synthetic middle','MIDDLE_SCHOOL','충북','{"student_id_policy":"SCHOOL_DEFINED"}',ARRAY['community','contests'],'APPROVED');
DELETE FROM cbgpu_schools.memberships WHERE user_id IN('00000000-0000-4000-8000-000000000004','00000000-0000-4000-8000-000000000005');
INSERT INTO cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,student_number) VALUES
('00000000-0000-4000-8000-000000000005','fixture-middle','STUDENT','ACTIVE',true,'1-12-003'),
('00000000-0000-4000-8000-000000000004','fixture-middle','TEACHER','ACTIVE',true,null);
-- Multi-school signup keeps global profile EXTERNAL; verified school membership is authoritative.
UPDATE public.profiles SET account_type='EXTERNAL' WHERE id IN('00000000-0000-4000-8000-000000000004','00000000-0000-4000-8000-000000000005');
UPDATE cbgpu_schools.memberships SET verified=true,status='ACTIVE',membership_type='EXTERNAL' WHERE user_id='00000000-0000-4000-8000-000000000002';
INSERT INTO public.contests(id,host_id,title,access_scope,participant_policy)
SELECT ('20000000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid,'00000000-0000-4000-8000-000000000001',policy,'PUBLIC',policy FROM(VALUES(1,'HIGH_SCHOOL_STUDENTS'),(2,'MIDDLE_SCHOOL_STUDENTS'),(3,'SECONDARY_SCHOOL_STUDENTS'),(4,'SCHOOL_TEACHERS'),(5,'ALL_ELIGIBLE'))p(n,policy);
DO $$DECLARE student uuid:='00000000-0000-4000-8000-000000000005';teacher uuid:='00000000-0000-4000-8000-000000000004';high uuid:='00000000-0000-4000-8000-000000000001'; BEGIN
PERFORM pg_temp.assert_true(NOT cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000001',student),'middle student cannot join HIGH only');
PERFORM pg_temp.assert_true(cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000001',high),'existing HIGH allowed');
PERFORM pg_temp.assert_true(cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000002',student),'middle cohort');
PERFORM pg_temp.assert_true(NOT cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000002',teacher),'teacher not student');
PERFORM pg_temp.assert_true(NOT cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000002',high),'HIGH not middle');
PERFORM pg_temp.assert_true(cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000003',student) AND cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000003',high),'secondary cohort');
PERFORM pg_temp.assert_true(cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000004',teacher) AND NOT cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000004',student),'teacher cohort');
PERFORM pg_temp.assert_true(cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000005',student),'ALL compatibility');
PERFORM pg_temp.assert_true(NOT EXISTS(SELECT FROM public.contests c WHERE c.participant_policy<>'ALL_ELIGIBLE' AND cbgpu_schools.contest_register(c.id,'00000000-0000-4000-8000-000000000002')),'external owner cannot become student or teacher');
UPDATE public.profiles SET account_status='SUSPENDED' WHERE id=student;
PERFORM pg_temp.assert_true(NOT cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000002',student),'fresh suspended account denied');
UPDATE public.profiles SET account_status='ACTIVE' WHERE id=student;

PERFORM pg_temp.assert_true(cbgpu_schools.scope_read(null,'cbsh','PUBLIC','{}'),'public visibility remains independent');
PERFORM pg_temp.assert_true(NOT cbgpu_schools.scope_read(student,'cbsh','SCHOOL','{}'),'private school scope not expanded');
UPDATE public.contests SET access_scope='SCHOOL' WHERE id='20000000-0000-4000-8000-000000000002';
PERFORM pg_temp.assert_true(NOT cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000002',student),'cohort does not bypass school scope');
UPDATE public.contests SET access_scope='PUBLIC' WHERE id='20000000-0000-4000-8000-000000000002';
PERFORM pg_temp.assert_true(cbgpu_schools.community_allowed(student,'NATIONWIDE',null),'middle participates existing nationwide community');
PERFORM pg_temp.assert_true(cbgpu_schools.community_allowed(student,'SCHOOL','fixture-middle'),'own school community');
PERFORM pg_temp.assert_true(NOT cbgpu_schools.community_allowed(student,'SCHOOL','cbsh'),'other school community denied');
UPDATE cbgpu_schools.memberships SET verified=false WHERE user_id=student;
PERFORM pg_temp.assert_true(NOT cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000002',student),'unverified denied');
UPDATE cbgpu_schools.memberships SET verified=true,ends_at=now()-interval '1 second' WHERE user_id=student;
PERFORM pg_temp.assert_true(NOT cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000002',student),'expired denied');
UPDATE cbgpu_schools.memberships SET ends_at=null WHERE user_id=student;
UPDATE cbgpu_schools.schools SET status='SUSPENDED' WHERE id='fixture-middle';
PERFORM pg_temp.assert_true(NOT cbgpu_schools.contest_register('20000000-0000-4000-8000-000000000002',student),'suspended school denied');
END$$;
SELECT pg_temp.assert_true(NOT has_function_privilege('authenticated','cbgpu_schools.import_official_directory(jsonb)','EXECUTE'),'browser cannot import');
SELECT pg_temp.assert_true(NOT has_table_privilege('authenticated','cbgpu_schools.directory_import_scopes','SELECT'),'private source audit');
SELECT 'MIDDLE_SCHOOL_CATALOG_AND_COHORT_TESTS_PASSED';

-- Same short school-defined number is accepted into review, never auto verified.
UPDATE cbgpu_schools.schools SET status='APPROVED' WHERE id='fixture-middle';
SELECT set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000002","session_id":"10000000-0000-4000-8000-000000000002"}',false);
SELECT public.cbgpu_school_api('join','{"school_id":"fixture-middle","membership_type":"STUDENT","student_number":"1-4-08"}');
SELECT pg_temp.assert_true(EXISTS(SELECT FROM cbgpu_schools.memberships WHERE user_id='00000000-0000-4000-8000-000000000002' AND school_id='fixture-middle' AND student_number='1-4-08' AND status='PENDING_VERIFICATION' AND NOT verified),'school defined numbering remains approval required');
DO $$DECLARE selected text;result jsonb;BEGIN
SELECT id INTO selected FROM cbgpu_schools.schools WHERE type='MIDDLE_SCHOOL' AND status='LISTED' ORDER BY id LIMIT 1;
result:=public.cbgpu_school_catalog_application_api('draft_create',jsonb_build_object('catalog_school_id',selected));
PERFORM pg_temp.assert_true(result#>>'{application,data,school,type}'='MIDDLE_SCHOOL','catalog-bound draft preserves middle type');
PERFORM pg_temp.assert_true((SELECT status='LISTED' AND cardinality(features)=0 AND verified_at IS NULL FROM cbgpu_schools.schools WHERE id=selected),'draft never activates catalog');
END$$;

SELECT pg_temp.assert_true((SELECT definition=replace(pg_get_functiondef('public.cbgpu_school_life_sources(text)'::regprocedure),$new$'school_id',s.id,'school_type',s.type,'status'$new$,$old$'school_id',s.id,'status'$old$) FROM original_life_definition),'life authorization and all other fields byte-equivalent');
SELECT set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000005","session_id":"10000000-0000-4000-8000-000000000005"}',false);
SELECT pg_temp.assert_true(public.cbgpu_school_life_sources('fixture-middle')->>'school_type'='MIDDLE_SCHOOL','authorized life DTO returns authoritative middle type');
DO $$BEGIN
BEGIN PERFORM public.cbgpu_school_life_sources('cbsh');RAISE EXCEPTION 'CROSS_SCHOOL_LIFE_ALLOWED';EXCEPTION WHEN insufficient_privilege THEN NULL;END;
END$$;
SELECT set_config('request.jwt.claims','{}',false);
DO $$BEGIN BEGIN PERFORM public.cbgpu_school_life_sources('fixture-middle');RAISE EXCEPTION 'ANON_LIFE_ALLOWED';EXCEPTION WHEN insufficient_privilege THEN NULL;END;END$$;
