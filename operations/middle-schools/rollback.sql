-- Definition-only rollback. Never remove catalog schools, memberships or source audit.
BEGIN;
SET LOCAL lock_timeout='3s';
DO $$BEGIN IF EXISTS(SELECT FROM public.contests WHERE participant_policy NOT IN('ALL_ELIGIBLE','HIGH_SCHOOL_STUDENTS')) THEN RAISE EXCEPTION 'ROLLBACK_NEW_COHORTS_IN_USE';END IF;END$$;
ALTER TABLE public.contests DROP CONSTRAINT contests_participant_policy_check;
ALTER TABLE public.contests ADD CONSTRAINT contests_participant_policy_check CHECK(participant_policy IN('ALL_ELIGIBLE','HIGH_SCHOOL_STUDENTS'));
create or replace function cbgpu_schools.import_official_directory(manifest jsonb) returns jsonb language plpgsql security definer set search_path='' as $$declare item jsonb;seen integer:=0;begin
 if coalesce(manifest->>'format_version'<>'1' or manifest->>'scope'<>'KOREAN_HIGH_SCHOOLS' or manifest->>'source_sha256' !~ '^[0-9a-f]{64}$' or manifest->>'source' !~ '^https://open\.neis\.go\.kr/portal/data/file/downloadBlobFileData\.do\?' or jsonb_typeof(manifest->'schools')<>'array' or jsonb_array_length(manifest->'schools')<>(manifest->>'school_count')::integer ,true) then raise exception 'INVALID_OFFICIAL_MANIFEST';end if;
 if exists(select 1 from jsonb_array_elements(manifest->'schools') x group by x->>'official_id' having count(*)>1) then raise exception 'DUPLICATE_OFFICIAL_IDENTIFIER';end if;
 for item in select * from jsonb_array_elements(manifest->'schools') loop
 if coalesce(item->>'official_id' !~ '^[A-Z0-9]{7}$' or item->>'type'<>'HIGH_SCHOOL' or item->>'status'<>'LISTED' or item->'features'<>'[]'::jsonb or item->'verified_at'<>'null'::jsonb or coalesce(length(btrim(item->>'official_name')),0) not between 2 and 150 ,true) then raise exception 'INVALID_OFFICIAL_SCHOOL';end if;
 insert into cbgpu_schools.schools(id,official_name,normalized_name,official_id,display_name,type,region,address,contact_phone,configuration,features,status,education_office_code,source_as_of,source_sha256,source_url,official_homepage_url)
 values('neis-'||(item->>'official_id'),item->>'official_name',lower(btrim(item->>'official_name')),item->>'official_id',item->>'display_name','HIGH_SCHOOL',item->>'region',item->>'address',item->>'contact_phone',cbgpu_schools.default_draft()->'configuration','{}','LISTED',item->>'education_office_code',(manifest->>'source_as_of')::date,manifest->>'source_sha256',manifest->>'source',item->>'homepage_url')
 on conflict(official_id) do update set official_name=excluded.official_name,normalized_name=excluded.normalized_name,display_name=excluded.display_name,region=excluded.region,address=excluded.address,contact_phone=excluded.contact_phone,education_office_code=excluded.education_office_code,source_as_of=excluded.source_as_of,source_sha256=excluded.source_sha256,source_url=excluded.source_url,official_homepage_url=excluded.official_homepage_url where cbgpu_schools.schools.status in('LISTED','UNCLAIMED');
 update cbgpu_schools.schools set official_homepage_url=item->>'homepage_url' where official_id=item->>'official_id' and status='APPROVED' and official_homepage_url is null;seen:=seen+1;
 end loop;
 insert into cbgpu_schools.directory_imports(source_sha256,source_as_of,school_count) values(manifest->>'source_sha256',(manifest->>'source_as_of')::date,seen) on conflict(source_sha256) do nothing;
 return jsonb_build_object('ok',true,'school_count',seen,'source_sha256',manifest->>'source_sha256');
end$$;
create or replace function cbgpu_schools.contest_participant_allowed(contest uuid,uid uuid)
returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from public.contests c where c.id=contest and
 (c.participant_policy='ALL_ELIGIBLE' or
  (c.participant_policy='HIGH_SCHOOL_STUDENTS' and exists(
    select 1 from cbgpu_schools.memberships m join cbgpu_schools.schools s on s.id=m.school_id
    where m.user_id=uid and m.membership_type='STUDENT' and s.type='HIGH_SCHOOL'
    and cbgpu_schools.active_member(uid,m.school_id)))))
$$;
DO $$DECLARE definition text;BEGIN
 definition:=pg_get_functiondef('public.cbgpu_school_life_sources(text)'::regprocedure);
 EXECUTE replace(definition,$new$'school_id',s.id,'school_type',s.type,'status'$new$,$old$'school_id',s.id,'status'$old$);
END$$;
NOTIFY pgrst,'reload schema';
COMMIT;
