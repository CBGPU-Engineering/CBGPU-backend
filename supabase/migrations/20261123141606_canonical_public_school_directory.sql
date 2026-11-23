-- Public read projection only: preserve every school row, ID, application and permission.
BEGIN;
CREATE OR REPLACE FUNCTION cbgpu_schools.directory_province(value text) RETURNS text
LANGUAGE sql IMMUTABLE SET search_path='' AS $$
 SELECT CASE split_part(regexp_replace(btrim(coalesce(value,'')),'[[:space:]]+',' ','g'),' ',1)
 WHEN '서울' THEN '서울특별시' WHEN '서울특별시' THEN '서울특별시'
 WHEN '부산' THEN '부산광역시' WHEN '부산광역시' THEN '부산광역시'
 WHEN '대구' THEN '대구광역시' WHEN '대구광역시' THEN '대구광역시'
 WHEN '인천' THEN '인천광역시' WHEN '인천광역시' THEN '인천광역시'
 WHEN '광주' THEN '광주광역시' WHEN '광주광역시' THEN '광주광역시'
 WHEN '대전' THEN '대전광역시' WHEN '대전광역시' THEN '대전광역시'
 WHEN '울산' THEN '울산광역시' WHEN '울산광역시' THEN '울산광역시'
 WHEN '세종' THEN '세종특별자치시' WHEN '세종특별자치시' THEN '세종특별자치시'
 WHEN '경기' THEN '경기도' WHEN '경기도' THEN '경기도'
 WHEN '강원' THEN '강원특별자치도' WHEN '강원도' THEN '강원특별자치도' WHEN '강원특별자치도' THEN '강원특별자치도'
 WHEN '충북' THEN '충청북도' WHEN '충청북도' THEN '충청북도'
 WHEN '충남' THEN '충청남도' WHEN '충청남도' THEN '충청남도'
 WHEN '전북' THEN '전북특별자치도' WHEN '전라북도' THEN '전북특별자치도' WHEN '전북특별자치도' THEN '전북특별자치도'
 WHEN '전남' THEN '전라남도' WHEN '전라남도' THEN '전라남도'
 WHEN '경북' THEN '경상북도' WHEN '경상북도' THEN '경상북도'
 WHEN '경남' THEN '경상남도' WHEN '경상남도' THEN '경상남도'
 WHEN '제주' THEN '제주특별자치도' WHEN '제주도' THEN '제주특별자치도' WHEN '제주특별자치도' THEN '제주특별자치도'
 ELSE NULL END
$$;
CREATE OR REPLACE FUNCTION cbgpu_schools.directory_location(region text,address text) RETURNS text
LANGUAGE sql IMMUTABLE SET search_path='' AS $$
 WITH stripped AS(
  SELECT cbgpu_schools.directory_province(region) province,cbgpu_schools.directory_province(address) address_province,
   CASE WHEN cbgpu_schools.directory_province(region) IS NULL THEN region ELSE regexp_replace(btrim(region),'^[^[:space:]]+[[:space:]]*','') END r,
   CASE WHEN cbgpu_schools.directory_province(address) IS NULL THEN address ELSE regexp_replace(btrim(address),'^[^[:space:]]+[[:space:]]*','') END a
 ), parts AS(
  SELECT *, (regexp_match(r,'(?:^|[[:space:]])([가-힣]+(?:시|군|구))(?=[[:space:]]|$)'))[1] region_city,(regexp_match(a,'(?:^|[[:space:]])([가-힣]+(?:시|군|구))(?=[[:space:]]|$)'))[1] address_city,
   (regexp_match(a,'(?:^|[[:space:]])([가-힣0-9]+(?:대로|로|길))(?=[[:space:]]|[0-9]|$)'))[1] road FROM stripped
 )
 SELECT CASE WHEN province IS NOT NULL AND(address_province IS NULL OR province=address_province)
  AND(region_city IS NULL OR address_city IS NULL OR region_city=address_city)
  AND coalesce(address_city,region_city) IS NOT NULL AND road IS NOT NULL
 THEN coalesce(address_city,region_city)||'/'||road ELSE NULL END FROM parts
$$;
CREATE OR REPLACE FUNCTION cbgpu_schools.directory_registered_match(c cbgpu_schools.schools) RETURNS text
LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT CASE WHEN count(*)=1 THEN min(a.id) ELSE NULL END
 FROM cbgpu_schools.schools a
 WHERE c.status IN('LISTED','UNCLAIMED') AND c.source_sha256 IS NOT NULL AND a.status='APPROVED' AND a.id<>c.id
 AND(
  nullif(btrim(c.official_id),'') IS NOT NULL AND nullif(btrim(a.official_id),'')=nullif(btrim(c.official_id),'')
  OR(nullif(btrim(a.official_id),'') IS NULL OR nullif(btrim(c.official_id),'') IS NULL)
   AND lower(btrim(a.normalized_name))=lower(btrim(c.normalized_name)) AND a.type=c.type
   AND cbgpu_schools.directory_province(a.region)=cbgpu_schools.directory_province(c.region)
   AND cbgpu_schools.directory_location(a.region,a.address)=cbgpu_schools.directory_location(c.region,c.address)
   AND(SELECT count(*) FROM cbgpu_schools.schools x WHERE x.status='APPROVED' AND lower(btrim(x.normalized_name))=lower(btrim(c.normalized_name)) AND x.type=c.type AND cbgpu_schools.directory_province(x.region)=cbgpu_schools.directory_province(c.region))=1
   AND(SELECT count(*) FROM cbgpu_schools.schools x WHERE x.status IN('LISTED','UNCLAIMED','APPROVED') AND x.source_sha256 IS NOT NULL AND lower(btrim(x.normalized_name))=lower(btrim(c.normalized_name)) AND x.type=c.type AND cbgpu_schools.directory_province(x.region)=cbgpu_schools.directory_province(c.region))=1
 )
$$;
CREATE OR REPLACE FUNCTION cbgpu_schools.canonical_directory_rows() RETURNS SETOF cbgpu_schools.schools
LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT s.* FROM cbgpu_schools.schools s
 WHERE s.status IN('LISTED','UNCLAIMED','APPROVED') AND cbgpu_schools.directory_registered_match(s) IS NULL
$$;
CREATE OR REPLACE FUNCTION public.cbgpu_public_school_directory(payload jsonb DEFAULT '{}') RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path='' AS $$
DECLARE q text:=left(coalesce(payload->>'q',''),150);wanted text:=nullif(payload->>'status','');lim integer:=least(100,greatest(1,coalesce((payload->>'limit')::integer,30)));off integer:=greatest(0,coalesce((payload->>'offset')::integer,0));items jsonb;total bigint;
BEGIN
 IF jsonb_typeof(payload) IS DISTINCT FROM 'object' OR octet_length(payload::text)>4096 OR EXISTS(SELECT FROM jsonb_object_keys(payload) k WHERE k NOT IN('q','status','limit','offset')) OR(wanted IS NOT NULL AND wanted NOT IN('LISTED','UNCLAIMED','APPROVED')) THEN RAISE EXCEPTION 'INVALID_DIRECTORY_FILTER' USING ERRCODE='22023';END IF;
 -- The canonical projection precedes all caller-controlled filters and pagination.
 WITH canonical AS MATERIALIZED(SELECT * FROM cbgpu_schools.canonical_directory_rows()),filtered AS MATERIALIZED(
  SELECT * FROM canonical WHERE(wanted IS NULL OR status=wanted) AND(NOT EXISTS(SELECT FROM regexp_split_to_table(btrim(q),'[[:space:]]+') term WHERE term<>'' AND NOT(concat_ws(' ',official_name,display_name,region,cbgpu_schools.directory_province(region)) ILIKE '%'||term||'%' OR coalesce(cbgpu_schools.directory_province(term)=cbgpu_schools.directory_province(region),false))))
 ),page AS(SELECT * FROM filtered ORDER BY CASE WHEN id='cbsh' THEN 0 ELSE 1 END,display_name,id LIMIT lim OFFSET off)
 SELECT(SELECT coalesce(jsonb_agg(cbgpu_schools.summary(p)-'gpu'-'logo_path' ORDER BY CASE WHEN p.id='cbsh' THEN 0 ELSE 1 END,p.display_name,p.id),'[]') FROM page p),(SELECT count(*) FROM filtered) INTO items,total;
 RETURN jsonb_build_object('items',items,'total',total);
END$$;
REVOKE ALL ON FUNCTION cbgpu_schools.directory_province(text),cbgpu_schools.directory_location(text,text),cbgpu_schools.directory_registered_match(cbgpu_schools.schools),cbgpu_schools.canonical_directory_rows() FROM PUBLIC,anon,authenticated,service_role;
REVOKE ALL ON FUNCTION public.cbgpu_public_school_directory(jsonb) FROM PUBLIC,anon,authenticated,service_role;
GRANT EXECUTE ON FUNCTION public.cbgpu_public_school_directory(jsonb) TO anon,authenticated;
-- Guarded branch-only replacement preserves every existing identity, application,
-- membership and permission branch, including later amendments to that function.
DO $$DECLARE definition text;start_at integer;end_at integer;old_branch text;new_branch text:='if action=''directory'' then return public.cbgpu_public_school_directory(payload);end if;';BEGIN
 definition:=pg_get_functiondef('public.cbgpu_school_api(text,jsonb)'::regprocedure);
 IF position(new_branch IN definition)>0 THEN RETURN;END IF;
 start_at:=position('if action=''directory'' then return jsonb_build_object(' IN definition);
 IF start_at=0 THEN RAISE EXCEPTION 'DIRECTORY_API_VERSION_MISMATCH';END IF;
 end_at:=position('end if;' IN substr(definition,start_at));
 IF end_at=0 THEN RAISE EXCEPTION 'DIRECTORY_API_VERSION_MISMATCH';END IF;
 old_branch:=substr(definition,start_at,end_at+6);
 IF position('cbgpu_schools.summary(z)' IN old_branch)=0 OR position('count(*) from cbgpu_schools.schools' IN old_branch)=0 THEN RAISE EXCEPTION 'DIRECTORY_API_VERSION_MISMATCH';END IF;
 EXECUTE replace(definition,old_branch,new_branch);
END$$;
-- The same read match prevents a cached catalog URL from creating a new draft
-- for an already registered institution. Existing drafts and approval logic stay untouched.
DO $$DECLARE definition text;old_branch text:=$old$if s.status='APPROVED' then return jsonb_build_object('ok',false,'code','DUPLICATE_SCHOOL','existing_school_id',s.id);end if;$old$;
new_branch text:=$new$if s.status='APPROVED' or cbgpu_schools.directory_registered_match(s) is not null then return jsonb_build_object('ok',false,'code','DUPLICATE_SCHOOL','existing_school_id',coalesce(cbgpu_schools.directory_registered_match(s),s.id));end if;$new$;
BEGIN
 definition:=pg_get_functiondef('public.cbgpu_school_catalog_application_api(text,jsonb)'::regprocedure);
 IF position(new_branch IN definition)>0 THEN RETURN;END IF;
 IF position(old_branch IN definition)=0 THEN RAISE EXCEPTION 'CATALOG_DRAFT_API_VERSION_MISMATCH';END IF;
 EXECUTE replace(definition,old_branch,new_branch);
END$$;
NOTIFY pgrst,'reload schema';
COMMIT;
