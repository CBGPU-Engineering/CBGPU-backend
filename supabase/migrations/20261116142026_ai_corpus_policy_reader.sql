-- Candidate only. No data import or changes to existing account/resource policies.
BEGIN;
DO $$BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_corpus_backend')THEN CREATE ROLE cbgpu_corpus_backend NOLOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;END IF;
 IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_corpus_backend'AND(rolcanlogin OR rolinherit OR rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls))THEN RAISE EXCEPTION 'Unexpected corpus backend privileges';END IF;
 IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_corpus_policy_reader')THEN CREATE ROLE cbgpu_corpus_policy_reader NOLOGIN INHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;END IF;
 IF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_corpus_policy_reader'AND(rolcanlogin OR NOT rolinherit OR rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls))THEN RAISE EXCEPTION 'Unexpected corpus reader privileges';END IF;
 IF EXISTS(SELECT 1 FROM pg_auth_members m JOIN pg_roles r ON r.oid=m.roleid WHERE m.member=(SELECT oid FROM pg_roles WHERE rolname='cbgpu_corpus_policy_reader')AND r.rolname<>'authenticated')THEN RAISE EXCEPTION 'Unexpected corpus reader membership';END IF;
 IF EXISTS(SELECT 1 FROM pg_auth_members WHERE member=(SELECT oid FROM pg_roles WHERE rolname='cbgpu_corpus_backend'))THEN RAISE EXCEPTION 'Corpus backend must not inherit other roles';END IF;
 IF EXISTS(SELECT 1 FROM pg_class WHERE relowner IN(SELECT oid FROM pg_roles WHERE rolname IN('cbgpu_corpus_policy_reader','cbgpu_corpus_backend')))OR EXISTS(SELECT 1 FROM pg_namespace WHERE nspowner IN(SELECT oid FROM pg_roles WHERE rolname IN('cbgpu_corpus_policy_reader','cbgpu_corpus_backend')))THEN RAISE EXCEPTION 'Corpus roles must not own relations';END IF;
END$$;
DO $$BEGIN
 IF EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public'AND c.relname IN('shared_materials','preprints','preprint_versions')AND NOT c.relrowsecurity)THEN RAISE EXCEPTION 'Corpus source RLS must be enabled';END IF;
END$$;
GRANT authenticated TO cbgpu_corpus_policy_reader;
CREATE SCHEMA IF NOT EXISTS cbgpu_corpus_auth;
REVOKE ALL ON SCHEMA cbgpu_corpus_auth FROM PUBLIC,anon,authenticated;
GRANT USAGE ON SCHEMA cbgpu_corpus_auth TO cbgpu_corpus_backend,cbgpu_corpus_policy_reader;
GRANT USAGE ON SCHEMA public,auth TO cbgpu_corpus_policy_reader;
GRANT SELECT ON public.shared_materials,public.preprints,public.preprint_versions TO cbgpu_corpus_policy_reader;
CREATE OR REPLACE FUNCTION cbgpu_corpus_auth._storage_allowed()RETURNS boolean LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path='' AS $$
DECLARE allowed boolean;
BEGIN
 IF current_setting('cbgpu.corpus.account_v2',true)IS DISTINCT FROM 'on'OR to_regprocedure('public.cbgpu_service_can_use_resource(uuid,text,text)')IS NULL THEN RETURN false;END IF;
 EXECUTE 'SELECT public.cbgpu_service_can_use_resource($1,$2,$3)' INTO allowed USING auth.uid(),'school_storage','cbsh';RETURN coalesce(allowed,false);
END$$;
-- Owned files stay readable after graduation/reverification. This narrow helper
-- role has no write grants; ordinary browser policies are not replaced.
DROP POLICY IF EXISTS corpus_owned_material ON public.shared_materials;
CREATE POLICY corpus_owned_material ON public.shared_materials FOR SELECT TO cbgpu_corpus_policy_reader USING(current_setting('cbgpu.corpus.account_v2',true)='on'AND(uploader_id=auth.uid()OR(status='ready'AND cbgpu_corpus_auth._storage_allowed())));
DROP POLICY IF EXISTS corpus_published_paper ON public.preprints;
CREATE POLICY corpus_published_paper ON public.preprints FOR SELECT TO cbgpu_corpus_policy_reader USING(status='published');
DROP POLICY IF EXISTS corpus_published_version ON public.preprint_versions;
CREATE POLICY corpus_published_version ON public.preprint_versions FOR SELECT TO cbgpu_corpus_policy_reader USING(EXISTS(SELECT 1 FROM public.preprints p WHERE p.id=paper_id AND p.status='published'));
CREATE OR REPLACE FUNCTION cbgpu_corpus_auth._policy_sources(refs jsonb)RETURNS jsonb
LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' SET row_security=on SET timezone='UTC' AS $$
 SELECT coalesce(jsonb_agg(s.item),'[]'::jsonb)FROM(
 SELECT jsonb_build_object('kind','material','id',m.id,'version',m.created_at::text,'name',m.file_name,'label',m.title,'href','/materials/'||m.id,'text',m.title||E'\n'||m.file_name,'size',m.size_bytes)AS item
 FROM public.shared_materials m WHERE m.status='ready'AND m.id IN(SELECT split_part(value,':',2)::uuid FROM jsonb_array_elements_text(refs)AS ids(value)WHERE split_part(value,':',1)='material')
 UNION ALL
 SELECT jsonb_build_object('kind','paper','id',p.id,'version',p.current_version::text,'name',p.id||'.pdf','label',p.title,'href','https://arxiv.cbgpu.com/'||p.id,'text',left(p.title||E'\n'||p.abstract,200000),'size',v.pdf_size,'sha256',v.pdf_sha256)
 FROM public.preprints p JOIN public.preprint_versions v ON v.paper_id=p.id AND v.version=p.current_version WHERE p.status='published'AND p.id IN(SELECT split_part(value,':',2)::uuid FROM jsonb_array_elements_text(refs)AS ids(value)WHERE split_part(value,':',1)='paper')
 )s
$$;
GRANT CREATE ON SCHEMA cbgpu_corpus_auth TO cbgpu_corpus_policy_reader;
ALTER FUNCTION cbgpu_corpus_auth._policy_sources(jsonb)OWNER TO cbgpu_corpus_policy_reader;
REVOKE CREATE ON SCHEMA cbgpu_corpus_auth FROM cbgpu_corpus_policy_reader;
CREATE OR REPLACE FUNCTION cbgpu_corpus_auth.sources_for_owner(p_owner uuid,refs jsonb,p_account_v2 boolean DEFAULT false)RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE actor record;result jsonb;
BEGIN
 IF p_owner IS NULL OR refs IS NULL OR jsonb_typeof(refs)<>'array'OR jsonb_array_length(refs)>500 OR octet_length(refs::text)>60000 THEN RAISE EXCEPTION 'CORPUS_INVALID_REQUEST' USING ERRCODE='PT400';END IF;
 IF EXISTS(SELECT 1 FROM jsonb_array_elements_text(refs)r WHERE r !~ '^(material|paper):[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$')THEN RAISE EXCEPTION 'CORPUS_INVALID_REFERENCE' USING ERRCODE='PT400';END IF;
 SELECT u.deleted_at,u.banned_until,to_jsonb(p)->>'account_status'AS state INTO actor FROM auth.users u JOIN public.profiles p ON p.id=u.id WHERE u.id=p_owner;
 IF NOT FOUND OR actor.deleted_at IS NOT NULL OR(actor.banned_until IS NOT NULL AND actor.banned_until>clock_timestamp())OR coalesce(actor.state,'ACTIVE')='SUSPENDED' THEN RAISE EXCEPTION 'CORPUS_IDENTITY_UNAVAILABLE' USING ERRCODE='PT401';END IF;
 PERFORM set_config('cbgpu.corpus.account_v2',CASE WHEN p_account_v2 THEN 'on' ELSE 'off' END,true);
 PERFORM set_config('request.jwt.claims',jsonb_build_object('role','authenticated','sub',p_owner)::text,true);
 PERFORM set_config('request.jwt.claim.sub',p_owner::text,true);
 SELECT cbgpu_corpus_auth._policy_sources(refs)INTO result;RETURN result;
END$$;
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA cbgpu_corpus_auth FROM PUBLIC,anon,authenticated,cbgpu_corpus_backend;
GRANT EXECUTE ON FUNCTION cbgpu_corpus_auth.sources_for_owner(uuid,jsonb,boolean)TO cbgpu_corpus_backend;
GRANT EXECUTE ON FUNCTION cbgpu_corpus_auth._storage_allowed()TO cbgpu_corpus_policy_reader;
COMMIT;
