-- Public institutional facts for the two reported patterns, with synthetic local IDs.
INSERT INTO cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,region,address,official_id,status,source_sha256,configuration)
VALUES
('registered-drone','경북드론고등학교','경북드론고등학교','경북드론고등학교','HIGH_SCHOOL','경북 청도군','청도군 헐티로 185',NULL,'APPROVED',NULL,'{}'),
('catalog-drone','경북드론고등학교','경북드론고등학교','드론 카탈로그 별칭','HIGH_SCHOOL','경상북도','경상북도 청도군 풍각면 헐티로 185','8750731','LISTED',repeat('a',64),'{}'),
('registered-seolhwa','설화고등학교','설화고등학교','설화고등학교','HIGH_SCHOOL','충청남도 아산시','충청남도 아산시 배방읍 용연로 111',NULL,'APPROVED',NULL,'{}'),
('catalog-seolhwa','설화고등학교','설화고등학교','설화고등학교','HIGH_SCHOOL','충청남도','충청남도 아산시 배방읍 용연로','8140266','LISTED',repeat('a',64),'{}'),
('registered-id','Official identity fixture','official identity fixture','Official identity fixture','HIGH_SCHOOL','서울','서울 강남구 도산대로',' SAME-ID ','APPROVED',NULL,'{}'),
('catalog-id','Older official name','older official name','Older official name','HIGH_SCHOOL','부산','부산 중구 중앙대로','SAME-ID','LISTED',repeat('a',64),'{}'),
('registered-city','Same city fixture','same city fixture','Same city fixture','HIGH_SCHOOL','경기 수원시','경기 수원시 중앙로 1',NULL,'APPROVED',NULL,'{}'),
('catalog-city','Same city fixture','same city fixture','Same city fixture','HIGH_SCHOOL','경기도','경기도 성남시 중앙로 1','CITY-ID','LISTED',repeat('a',64),'{}'),
('registered-province','Province fixture','province fixture','Province fixture','HIGH_SCHOOL','경북 청도군','경북 청도군 중앙로 1',NULL,'APPROVED',NULL,'{}'),
('catalog-province','Province fixture','province fixture','Province fixture','HIGH_SCHOOL','경상남도','경상남도 청도군 중앙로 1','PROVINCE-ID','LISTED',repeat('a',64),'{}'),
('registered-road','Road fixture','road fixture','Road fixture','HIGH_SCHOOL','충남 아산시','충남 아산시 용연로 1',NULL,'APPROVED',NULL,'{}'),
('catalog-road','Road fixture','road fixture','Road fixture','HIGH_SCHOOL','충청남도','충청남도 아산시 중앙로 1','ROAD-ID','LISTED',repeat('a',64),'{}'),
('registered-known','Known conflict fixture','known conflict fixture','Known conflict fixture','HIGH_SCHOOL','충남 아산시','충남 아산시 용연로 1','KNOWN-A','APPROVED',NULL,'{}'),
('catalog-known','Known conflict fixture','known conflict fixture','Known conflict fixture','HIGH_SCHOOL','충청남도','충청남도 아산시 용연로 1','KNOWN-B','LISTED',repeat('a',64),'{}'),
('registered-amb-a','Ambiguous registered fixture','ambiguous registered fixture','Ambiguous registered fixture','HIGH_SCHOOL','충남 아산시','충남 아산시 용연로 1',NULL,'APPROVED',NULL,'{}'),
('registered-amb-b','Ambiguous registered fixture','ambiguous registered fixture','Ambiguous registered fixture','HIGH_SCHOOL','충남 아산시','충남 아산시 용연로 1',NULL,'APPROVED',NULL,'{}'),
('catalog-amb','Ambiguous registered fixture','ambiguous registered fixture','Ambiguous registered fixture','HIGH_SCHOOL','충청남도','충청남도 아산시 용연로 1','AMB-ID','LISTED',repeat('a',64),'{}'),
('registered-catalog-amb','Ambiguous catalog fixture','ambiguous catalog fixture','Ambiguous catalog fixture','HIGH_SCHOOL','충남 아산시','충남 아산시 용연로 1',NULL,'APPROVED',NULL,'{}'),
('catalog-amb-a','Ambiguous catalog fixture','ambiguous catalog fixture','Ambiguous catalog fixture','HIGH_SCHOOL','충청남도','충청남도 아산시 용연로 1','AMB-A','LISTED',repeat('a',64),'{}'),
('catalog-amb-b','Ambiguous catalog fixture','ambiguous catalog fixture','Ambiguous catalog fixture','HIGH_SCHOOL','충청남도','충청남도 아산시 용연로 1','AMB-B','LISTED',repeat('a',64),'{}'),
('unregistered-only','Unregistered fixture','unregistered fixture','Unregistered fixture','HIGH_SCHOOL','충청남도','충청남도 아산시 다른로 1','UNREGISTERED','LISTED',repeat('a',64),'{}');
CREATE TABLE school_rows_before AS SELECT jsonb_agg(to_jsonb(s) ORDER BY id) value FROM cbgpu_schools.schools s;
CREATE TABLE memberships_before AS SELECT coalesce(jsonb_agg(to_jsonb(m) ORDER BY id),'[]') value FROM cbgpu_schools.memberships m;
CREATE TABLE api_body_before AS SELECT regexp_replace(prosrc,'if action=''directory'' then[^\n]+','DIRECTORY_BRANCH') value FROM pg_proc WHERE oid='public.cbgpu_school_api(text,jsonb)'::regprocedure;
CREATE TABLE catalog_api_before AS SELECT regexp_replace(prosrc,'if s.status=''APPROVED'' then[^\n]+','CATALOG_DUPLICATE_GUARD') value FROM pg_proc WHERE oid='public.cbgpu_school_catalog_application_api(text,jsonb)'::regprocedure;
SELECT set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000005","session_id":"10000000-0000-4000-8000-000000000005","role":"authenticated"}',false);
SELECT public.cbgpu_school_catalog_application_api('draft_create','{"catalog_school_id":"catalog-road"}');
CREATE TABLE applications_before AS SELECT jsonb_agg(to_jsonb(a) ORDER BY id) value FROM cbgpu_schools.applications a;
