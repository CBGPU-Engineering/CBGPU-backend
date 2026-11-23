DO $$DECLARE result jsonb;seen text[]:='{}';item jsonb;off int:=0;expected int;BEGIN
 IF(SELECT value FROM school_rows_before) IS DISTINCT FROM(SELECT jsonb_agg(to_jsonb(s) ORDER BY id) FROM cbgpu_schools.schools s) THEN RAISE EXCEPTION 'SCHOOL_DATA_CHANGED';END IF;
 IF(SELECT value FROM memberships_before) IS DISTINCT FROM(SELECT coalesce(jsonb_agg(to_jsonb(m) ORDER BY id),'[]') FROM cbgpu_schools.memberships m) THEN RAISE EXCEPTION 'MEMBERSHIP_CHANGED';END IF;
 IF(SELECT value FROM applications_before) IS DISTINCT FROM(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM cbgpu_schools.applications a) THEN RAISE EXCEPTION 'EXISTING_DRAFT_CHANGED';END IF;
 IF(SELECT value FROM api_body_before) IS DISTINCT FROM(SELECT regexp_replace(prosrc,'if action=''directory'' then[^\n]+','DIRECTORY_BRANCH') FROM pg_proc WHERE oid='public.cbgpu_school_api(text,jsonb)'::regprocedure) THEN RAISE EXCEPTION 'UNRELATED_API_CHANGED';END IF;
 FOREACH item IN ARRAY ARRAY['{"q":"경북드론고등학교"}'::jsonb,'{"q":"설화고등학교"}'::jsonb] LOOP
  result:=public.cbgpu_school_api('directory',item);IF(result->>'total')::int<>1 OR result#>>'{items,0,status}'<>'APPROVED' THEN RAISE EXCEPTION 'REPORTED_DUPLICATE_REMAINS %',result;END IF;
 END LOOP;
 result:=public.cbgpu_school_api('directory','{"q":"경북드론고등학교","status":"LISTED"}');IF result->>'total'<>'0' THEN RAISE EXCEPTION 'STATUS_RESURRECTED_DUPLICATE';END IF;
 result:=public.cbgpu_school_api('directory','{"q":"경상북도 경북드론고등학교"}');IF result->>'total'<>'1' OR result#>>'{items,0,id}'<>'registered-drone' THEN RAISE EXCEPTION 'CANONICAL_PROVINCE_SEARCH_LOST_REGISTERED';END IF;
 result:=public.cbgpu_school_api('directory','{"q":"충청남도 설화고등학교"}');IF result->>'total'<>'1' OR result#>>'{items,0,id}'<>'registered-seolhwa' THEN RAISE EXCEPTION 'FULL_PROVINCE_SEARCH_LOST_REGISTERED';END IF;
 result:=public.cbgpu_school_api('directory','{"q":"경상북도 경북드론고등학교","status":"LISTED"}');IF result->>'total'<>'0' THEN RAISE EXCEPTION 'REGION_STATUS_RESURRECTED_DUPLICATE';END IF;
 result:=public.cbgpu_school_api('directory','{"q":"드론 카탈로그 별칭"}');IF result->>'total'<>'0' THEN RAISE EXCEPTION 'QUERY_RESURRECTED_DUPLICATE';END IF;
 result:=public.cbgpu_school_api('directory','{"q":"Official identity fixture"}');IF result->>'total'<>'1' THEN RAISE EXCEPTION 'OFFICIAL_ID_NOT_CANONICAL';END IF;
 IF EXISTS(SELECT FROM cbgpu_schools.canonical_directory_rows() WHERE id IN('catalog-drone','catalog-seolhwa','catalog-id')) THEN RAISE EXCEPTION 'CANONICAL_HIDDEN_SET_WRONG';END IF;
 FOREACH item IN ARRAY ARRAY['"Same city fixture"'::jsonb,'"Province fixture"'::jsonb,'"Road fixture"'::jsonb,'"Known conflict fixture"'::jsonb] LOOP
  result:=public.cbgpu_school_api('directory',jsonb_build_object('q',item#>>'{}'));IF result->>'total'<>'2' THEN RAISE EXCEPTION 'DISTINCT_SCHOOLS_COLLAPSED %',result;END IF;
 END LOOP;
 FOREACH item IN ARRAY ARRAY['"Ambiguous registered fixture"'::jsonb,'"Ambiguous catalog fixture"'::jsonb] LOOP
  result:=public.cbgpu_school_api('directory',jsonb_build_object('q',item#>>'{}'));IF result->>'total'<>'3' THEN RAISE EXCEPTION 'AMBIGUOUS_SCHOOLS_COLLAPSED';END IF;
 END LOOP;
 result:=public.cbgpu_school_api('directory','{"q":"Unregistered fixture","status":"LISTED"}');IF result->>'total'<>'1' THEN RAISE EXCEPTION 'UNREGISTERED_CATALOG_HIDDEN';END IF;
 result:=public.cbgpu_school_api('school','{"school_id":"catalog-drone"}');IF result#>>'{school,id}'<>'catalog-drone' OR result#>>'{school,status}'<>'LISTED' THEN RAISE EXCEPTION 'DIRECT_ID_CHANGED';END IF;
 IF cbgpu_schools.directory_province('강원도 춘천시')<>'강원특별자치도' OR cbgpu_schools.directory_province('전북 전주시')<>'전북특별자치도' OR cbgpu_schools.directory_province('경북 청도군')<>'경상북도' THEN RAISE EXCEPTION 'PROVINCE_ALIAS_FAILED';END IF;
 IF cbgpu_schools.directory_location('충남 아산시','충남 천안시 용연로') IS NOT NULL OR cbgpu_schools.directory_location('충남 아산시','경북 아산시 용연로') IS NOT NULL THEN RAISE EXCEPTION 'CONFLICTING_LOCATION_ACCEPTED';END IF;
 IF cbgpu_schools.directory_location('서울 구로구','서울 구로구 경인로')=cbgpu_schools.directory_location('서울 구로구','서울 구로구 중앙로') THEN RAISE EXCEPTION 'DISTRICT_SUBSTRING_MISTAKEN_FOR_ROAD';END IF;
 SELECT count(*)-3 INTO expected FROM cbgpu_schools.schools WHERE status IN('LISTED','UNCLAIMED','APPROVED');
 LOOP
  result:=public.cbgpu_school_api('directory',jsonb_build_object('limit',3,'offset',off));IF(result->>'total')::int<>expected THEN RAISE EXCEPTION 'TOTAL_BEFORE_CANONICALIZATION';END IF;
  FOR item IN SELECT value FROM jsonb_array_elements(result->'items') LOOP IF array_position(seen,item->>'id') IS NOT NULL THEN RAISE EXCEPTION 'PAGINATION_DUPLICATE';END IF;seen:=array_append(seen,item->>'id');END LOOP;
  off:=off+3;EXIT WHEN off>=expected;
 END LOOP;
 IF cardinality(seen)<>expected THEN RAISE EXCEPTION 'PAGINATION_GAPS';END IF;
END$$;
SELECT set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000005","session_id":"10000000-0000-4000-8000-000000000005","role":"authenticated"}',false);
SET ROLE authenticated;
DO $$DECLARE result jsonb;BEGIN
 result:=public.cbgpu_school_catalog_application_api('draft_create','{"catalog_school_id":"catalog-drone"}');
 IF result->>'code'<>'DUPLICATE_SCHOOL' OR result->>'existing_school_id'<>'registered-drone' THEN RAISE EXCEPTION 'CACHED_CATALOG_DRAFT_NOT_REJECTED';END IF;
END$$;
RESET ROLE;
DO $$BEGIN IF(SELECT value FROM applications_before) IS DISTINCT FROM(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM cbgpu_schools.applications a) THEN RAISE EXCEPTION 'DUPLICATE_DRAFT_WRITTEN';END IF;
 IF(SELECT value FROM catalog_api_before) IS DISTINCT FROM(SELECT regexp_replace(prosrc,'if s.status=''APPROVED'' or[^\n]+','CATALOG_DUPLICATE_GUARD') FROM pg_proc WHERE oid='public.cbgpu_school_catalog_application_api(text,jsonb)'::regprocedure) THEN RAISE EXCEPTION 'UNRELATED_CATALOG_API_CHANGED';END IF;
END$$;
SELECT set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000001","session_id":"10000000-0000-4000-8000-000000000001","role":"authenticated"}',false);
SET ROLE authenticated;
DO $$BEGIN IF public.cbgpu_school_catalog_application_api('draft_create','{"catalog_school_id":"catalog-known"}')#>>'{application,catalog_school_id}'<>'catalog-known' THEN RAISE EXCEPTION 'KNOWN_ID_CONFLICT_DRAFT_BLOCKED';END IF;END$$;
RESET ROLE;
SELECT set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000002","session_id":"10000000-0000-4000-8000-000000000002","role":"authenticated"}',false);
SET ROLE authenticated;
DO $$BEGIN IF public.cbgpu_school_catalog_application_api('draft_create','{"catalog_school_id":"catalog-city"}')#>>'{application,catalog_school_id}'<>'catalog-city' THEN RAISE EXCEPTION 'HOMONYM_DRAFT_BLOCKED';END IF;END$$;
RESET ROLE;
SELECT set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000004","session_id":"10000000-0000-4000-8000-000000000004","role":"authenticated"}',false);
SET ROLE authenticated;
DO $$BEGIN IF public.cbgpu_school_catalog_application_api('draft_create','{"catalog_school_id":"unregistered-only"}')#>>'{application,catalog_school_id}'<>'unregistered-only' THEN RAISE EXCEPTION 'UNREGISTERED_DRAFT_BLOCKED';END IF;END$$;
RESET ROLE;
SELECT set_config('request.jwt.claims','{}',false);
SET ROLE anon;
SELECT public.cbgpu_public_school_directory('{"q":"설화고등학교"}');
DO $$BEGIN
 BEGIN PERFORM cbgpu_schools.canonical_directory_rows();RAISE EXCEPTION 'PRIVATE_HELPER_EXPOSED';EXCEPTION WHEN insufficient_privilege THEN NULL;END;
 BEGIN PERFORM public.cbgpu_school_api('memberships','{}');RAISE EXCEPTION 'IDENTITY_GUARD_REMOVED';EXCEPTION WHEN insufficient_privilege THEN NULL;END;
END$$;
RESET ROLE;
