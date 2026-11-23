-- Import remains an offline least-privilege operation; new entries are LISTED only.
BEGIN;
SET LOCAL lock_timeout='3s';
SET LOCAL statement_timeout='30s';
CREATE TABLE IF NOT EXISTS cbgpu_schools.directory_import_scopes(
 source_sha256 text NOT NULL,scope text NOT NULL CHECK(scope IN('KOREAN_HIGH_SCHOOLS','KOREAN_MIDDLE_SCHOOLS')),
 source_as_of date NOT NULL,school_count integer NOT NULL CHECK(school_count>=0),imported_at timestamptz NOT NULL DEFAULT now(),PRIMARY KEY(source_sha256,scope));
ALTER TABLE cbgpu_schools.directory_import_scopes ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON cbgpu_schools.directory_import_scopes FROM PUBLIC,anon,authenticated,service_role;
create or replace function cbgpu_schools.import_official_directory(manifest jsonb) returns jsonb language plpgsql security definer set search_path='' as $$declare item jsonb;seen integer:=0;expected_type text;begin
 expected_type:=case manifest->>'scope' when 'KOREAN_HIGH_SCHOOLS' then 'HIGH_SCHOOL' when 'KOREAN_MIDDLE_SCHOOLS' then 'MIDDLE_SCHOOL' end;
 if expected_type is null then raise exception 'INVALID_OFFICIAL_MANIFEST';end if;
 if coalesce(manifest->>'format_version'<>'1' or manifest->>'source_sha256' !~ '^[0-9a-f]{64}$' or manifest->>'source' !~ '^https://open\.neis\.go\.kr/portal/data/file/downloadBlobFileData\.do\?' or jsonb_typeof(manifest->'schools')<>'array' or jsonb_array_length(manifest->'schools')<>(manifest->>'school_count')::integer ,true) then raise exception 'INVALID_OFFICIAL_MANIFEST';end if;
 if exists(select 1 from jsonb_array_elements(manifest->'schools') x group by x->>'official_id' having count(*)>1) then raise exception 'DUPLICATE_OFFICIAL_IDENTIFIER';end if;
 for item in select * from jsonb_array_elements(manifest->'schools') loop
 if coalesce(item->>'official_id' !~ '^[A-Z0-9]{7}$' or item->>'type'<>expected_type or item->>'status'<>'LISTED' or item->'features'<>'[]'::jsonb or item->'verified_at'<>'null'::jsonb or coalesce(length(btrim(item->>'official_name')),0) not between 2 and 150 ,true) then raise exception 'INVALID_OFFICIAL_SCHOOL';end if;
 if (expected_type='HIGH_SCHOOL' and item->>'source_kind' not in('고등학교','각종학교(고)')) or(expected_type='MIDDLE_SCHOOL' and item->>'source_kind' not in('중학교','각종학교(중)')) or nullif(item->>'source_kind','') is null then raise exception 'INVALID_OFFICIAL_SCHOOL_KIND';end if;
 if exists(select from cbgpu_schools.schools s where s.official_id=item->>'official_id' and s.type<>expected_type) then raise exception 'OFFICIAL_SCHOOL_TYPE_CONFLICT';end if;
 insert into cbgpu_schools.schools(id,official_name,normalized_name,official_id,display_name,type,region,address,contact_phone,configuration,features,status,education_office_code,source_as_of,source_sha256,source_url,official_homepage_url)
 values('neis-'||(item->>'official_id'),item->>'official_name',lower(btrim(item->>'official_name')),item->>'official_id',item->>'display_name',expected_type,item->>'region',item->>'address',item->>'contact_phone',cbgpu_schools.default_draft()->'configuration','{}','LISTED',item->>'education_office_code',(manifest->>'source_as_of')::date,manifest->>'source_sha256',manifest->>'source',item->>'homepage_url')
 on conflict(official_id) do update set official_name=excluded.official_name,normalized_name=excluded.normalized_name,display_name=excluded.display_name,region=excluded.region,address=excluded.address,contact_phone=excluded.contact_phone,education_office_code=excluded.education_office_code,source_as_of=excluded.source_as_of,source_sha256=excluded.source_sha256,source_url=excluded.source_url,official_homepage_url=excluded.official_homepage_url where cbgpu_schools.schools.status in('LISTED','UNCLAIMED');
 update cbgpu_schools.schools set official_homepage_url=item->>'homepage_url' where official_id=item->>'official_id' and status='APPROVED' and official_homepage_url is null;seen:=seen+1;
 end loop;
 insert into cbgpu_schools.directory_imports(source_sha256,source_as_of,school_count) values(manifest->>'source_sha256',(manifest->>'source_as_of')::date,seen) on conflict(source_sha256) do nothing;
 insert into cbgpu_schools.directory_import_scopes(source_sha256,scope,source_as_of,school_count) values(manifest->>'source_sha256',manifest->>'scope',(manifest->>'source_as_of')::date,seen) on conflict(source_sha256,scope) do nothing;
 return jsonb_build_object('ok',true,'school_count',seen,'source_sha256',manifest->>'source_sha256');
end$$;

-- Existing policies and rows remain unchanged; organizers opt into the new cohorts.
ALTER TABLE public.contests DROP CONSTRAINT IF EXISTS contests_participant_policy_check;
ALTER TABLE public.contests ADD CONSTRAINT contests_participant_policy_check CHECK(participant_policy IN('ALL_ELIGIBLE','HIGH_SCHOOL_STUDENTS','MIDDLE_SCHOOL_STUDENTS','SECONDARY_SCHOOL_STUDENTS','SCHOOL_TEACHERS'));
CREATE OR REPLACE FUNCTION cbgpu_schools.contest_participant_allowed(contest uuid,uid uuid)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT EXISTS(SELECT FROM public.contests c WHERE c.id=contest AND (
 c.participant_policy='ALL_ELIGIBLE' OR EXISTS(
 SELECT FROM cbgpu_schools.memberships m JOIN cbgpu_schools.schools s ON s.id=m.school_id
 WHERE m.user_id=uid AND cbgpu_schools.active_member(uid,m.school_id) AND (
 c.participant_policy='HIGH_SCHOOL_STUDENTS' AND m.membership_type='STUDENT' AND s.type='HIGH_SCHOOL'
 OR c.participant_policy='MIDDLE_SCHOOL_STUDENTS' AND m.membership_type='STUDENT' AND s.type='MIDDLE_SCHOOL'
 OR c.participant_policy='SECONDARY_SCHOOL_STUDENTS' AND m.membership_type='STUDENT' AND s.type IN('MIDDLE_SCHOOL','HIGH_SCHOOL')
 OR c.participant_policy='SCHOOL_TEACHERS' AND m.membership_type='TEACHER'))))
$$;
REVOKE ALL ON FUNCTION cbgpu_schools.import_official_directory(jsonb),cbgpu_schools.contest_participant_allowed(uuid,uuid) FROM PUBLIC,anon,authenticated,service_role;
GRANT EXECUTE ON FUNCTION cbgpu_schools.import_official_directory(jsonb) TO cbgpu_school_directory_importer;
-- Add authoritative type only; retain the deployed handler's exact authorization and source policy.
DO $$DECLARE definition text;updated text;BEGIN
 definition:=pg_get_functiondef('public.cbgpu_school_life_sources(text)'::regprocedure);
 IF position($marker$'school_id',s.id,'school_type',s.type,'status'$marker$ IN definition)=0 THEN
  updated:=replace(definition,$old$'school_id',s.id,'status'$old$,$new$'school_id',s.id,'school_type',s.type,'status'$new$);
  IF updated=definition THEN RAISE EXCEPTION 'SCHOOL_LIFE_TYPE_HANDLER_VERSION_MISMATCH';END IF;
  EXECUTE updated;
 END IF;
END$$;
NOTIFY pgrst,'reload schema';
COMMIT;
