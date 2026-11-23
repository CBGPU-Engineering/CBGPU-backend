BEGIN;
CREATE OR REPLACE FUNCTION cbgpu_schools.directory_province(value text) RETURNS text
LANGUAGE sql IMMUTABLE AS $$
 SELECT CASE pg_catalog.split_part(pg_catalog.regexp_replace(pg_catalog.btrim(coalesce(value,'')),'[[:space:]]+',' ','g'),' ',1)
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

CREATE OR REPLACE FUNCTION cbgpu_schools.canonical_directory_rows() RETURNS SETOF cbgpu_schools.schools
LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 WITH keys AS MATERIALIZED(
 SELECT id,status,source_sha256,nullif(btrim(official_id),'') official_key,
 lower(btrim(normalized_name)) name_key,type,cbgpu_schools.directory_province(region) province_key
 FROM cbgpu_schools.schools WHERE status IN('LISTED','UNCLAIMED','APPROVED')),
 approved AS MATERIALIZED(SELECT * FROM keys WHERE status='APPROVED'),
 catalog AS MATERIALIZED(SELECT * FROM keys WHERE status IN('LISTED','UNCLAIMED') AND source_sha256 IS NOT NULL),
 candidates AS MATERIALIZED(
 SELECT c.id FROM catalog c JOIN approved a ON a.official_key=c.official_key
 UNION
 SELECT c.id FROM catalog c JOIN approved a ON a.name_key=c.name_key AND a.type=c.type AND a.province_key=c.province_key
 WHERE a.official_key IS NULL OR c.official_key IS NULL),
 hidden AS MATERIALIZED(SELECT c.id FROM candidates p JOIN cbgpu_schools.schools c ON c.id=p.id
 WHERE cbgpu_schools.directory_registered_match(c) IS NOT NULL)
 SELECT s.* FROM cbgpu_schools.schools s WHERE s.status IN('LISTED','UNCLAIMED','APPROVED')
 AND NOT EXISTS(SELECT FROM hidden h WHERE h.id=s.id)
$$;
CREATE OR REPLACE FUNCTION public.cbgpu_public_school_directory(payload jsonb DEFAULT '{}') RETURNS jsonb
LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path='' AS $$
DECLARE q text:=left(coalesce(payload->>'q',''),150);wanted text:=nullif(payload->>'status','');lim integer:=least(100,greatest(1,coalesce((payload->>'limit')::integer,30)));off integer:=greatest(0,coalesce((payload->>'offset')::integer,0));items jsonb;total bigint;
BEGIN
 IF jsonb_typeof(payload) IS DISTINCT FROM 'object' OR octet_length(payload::text)>4096 OR EXISTS(SELECT FROM jsonb_object_keys(payload) k WHERE k NOT IN('q','status','limit','offset')) OR(wanted IS NOT NULL AND wanted NOT IN('LISTED','UNCLAIMED','APPROVED')) THEN RAISE EXCEPTION 'INVALID_DIRECTORY_FILTER' USING ERRCODE='22023';END IF;
 -- The canonical projection precedes all caller-controlled filters and pagination.
 WITH terms AS MATERIALIZED(SELECT term,cbgpu_schools.directory_province(term) province FROM regexp_split_to_table(btrim(q),'[[:space:]]+') term WHERE term<>''), canonical AS MATERIALIZED(SELECT s.*,cbgpu_schools.directory_province(s.region) search_province FROM cbgpu_schools.canonical_directory_rows() s),filtered AS MATERIALIZED(
  SELECT * FROM canonical WHERE(wanted IS NULL OR status=wanted) AND(NOT EXISTS(SELECT FROM terms WHERE NOT(concat_ws(' ',official_name,display_name,region,search_province) ILIKE '%'||term||'%' OR coalesce(terms.province=search_province,false))))
 ),page AS(SELECT * FROM filtered ORDER BY CASE WHEN id='cbsh' THEN 0 ELSE 1 END,display_name,id LIMIT lim OFFSET off)
 SELECT(SELECT coalesce(jsonb_agg(cbgpu_schools.summary((SELECT s FROM cbgpu_schools.schools s WHERE s.id=p.id))-'gpu'-'logo_path' ORDER BY CASE WHEN p.id='cbsh' THEN 0 ELSE 1 END,p.display_name,p.id),'[]') FROM page p),(SELECT count(*) FROM filtered) INTO items,total;
 RETURN jsonb_build_object('items',items,'total',total);
END$$;
NOTIFY pgrst,'reload schema';
COMMIT;
