BEGIN;
CREATE TABLE public.preprints (
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
 paper_number bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
 identifier text GENERATED ALWAYS AS ('CBGPU-'||lpad(paper_number::text,6,'0')) STORED,
 owner_id uuid REFERENCES auth.users(id) ON DELETE SET NULL,
 title text NOT NULL CHECK(length(btrim(title)) BETWEEN 5 AND 300),
 abstract text NOT NULL CHECK(length(btrim(abstract)) BETWEEN 30 AND 12000),
 authors jsonb NOT NULL CHECK(jsonb_typeof(authors)='array' AND jsonb_array_length(authors) BETWEEN 1 AND 20),
 category text NOT NULL CHECK(category IN ('math','physics','chemistry','biology','computer-science','engineering','other')),
 license text NOT NULL DEFAULT 'all-rights-reserved' CHECK(license IN ('all-rights-reserved','cc-by-4.0')),
 status text NOT NULL DEFAULT 'published' CHECK(status IN ('draft','published','withdrawn')),
 current_version integer NOT NULL DEFAULT 1 CHECK(current_version>0),
 created_at timestamptz NOT NULL DEFAULT now(),published_at timestamptz NOT NULL DEFAULT now(),
 search_text text GENERATED ALWAYS AS (title||' '||abstract||' '||authors::text) STORED
);
CREATE TABLE public.preprint_versions (
 paper_id uuid NOT NULL REFERENCES public.preprints(id) ON DELETE CASCADE,
 version integer NOT NULL CHECK(version>0),
 pdf_path text NOT NULL UNIQUE,
 pdf_size integer NOT NULL CHECK(pdf_size BETWEEN 1 AND 20971520),
 pdf_sha256 text NOT NULL CHECK(pdf_sha256 ~ '^[a-f0-9]{64}$'),
 notes text NOT NULL DEFAULT '' CHECK(length(notes)<=4000),
 created_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(paper_id,version)
);
CREATE INDEX preprints_publication ON public.preprints(status,published_at DESC);
CREATE INDEX preprints_owner ON public.preprints(owner_id,created_at DESC);
ALTER TABLE public.preprints ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.preprint_versions ENABLE ROW LEVEL SECURITY;
-- All publication access goes through the validated server API. No direct Data
-- API writes can bypass PDF validation, ownership checks or upload quotas.
REVOKE ALL ON public.preprints,public.preprint_versions FROM anon,authenticated;
GRANT ALL ON public.preprints,public.preprint_versions TO service_role;
GRANT USAGE,SELECT ON SEQUENCE public.preprints_paper_number_seq TO service_role;
GRANT SELECT,INSERT,UPDATE,DELETE ON public.preprints,public.preprint_versions TO cbgpu_developer;
GRANT USAGE,SELECT ON SEQUENCE public.preprints_paper_number_seq TO cbgpu_developer;
INSERT INTO storage.buckets(id,name,public,file_size_limit,allowed_mime_types) VALUES('preprints','preprints',false,20971520,ARRAY['application/pdf']) ON CONFLICT(id) DO NOTHING;
CREATE POLICY preprints_api_only ON storage.objects AS RESTRICTIVE FOR ALL TO anon,authenticated USING(bucket_id<>'preprints') WITH CHECK(bucket_id<>'preprints');
CREATE OR REPLACE FUNCTION public.publish_preprint(p_id uuid,p_owner_id uuid,p_title text,p_abstract text,p_authors jsonb,p_category text,p_license text,p_pdf_path text,p_pdf_size integer,p_pdf_sha256 text,p_notes text DEFAULT '') RETURNS jsonb LANGUAGE plpgsql SECURITY INVOKER SET search_path='' AS $$
DECLARE item public.preprints;
BEGIN
 PERFORM pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended('preprint:'||p_owner_id::text,0));
 IF (SELECT count(*) FROM public.preprints WHERE owner_id=p_owner_id AND created_at>now()-interval '24 hours')>=5 THEN RAISE EXCEPTION 'PUBLICATION_DAILY_LIMIT' USING ERRCODE='23514'; END IF;
 IF p_pdf_path<>p_owner_id::text||'/'||p_id::text||'/v1.pdf' THEN RAISE EXCEPTION 'INVALID_PDF_PATH' USING ERRCODE='23514'; END IF;
 INSERT INTO public.preprints(id,owner_id,title,abstract,authors,category,license) VALUES(p_id,p_owner_id,p_title,p_abstract,p_authors,p_category,p_license) RETURNING * INTO item;
 INSERT INTO public.preprint_versions(paper_id,version,pdf_path,pdf_size,pdf_sha256,notes) VALUES(p_id,1,p_pdf_path,p_pdf_size,p_pdf_sha256,p_notes);
 RETURN to_jsonb(item)-'owner_id'-'search_text';
END $$;
REVOKE ALL ON FUNCTION public.publish_preprint(uuid,uuid,text,text,jsonb,text,text,text,integer,text,text) FROM PUBLIC,anon,authenticated;
GRANT EXECUTE ON FUNCTION public.publish_preprint(uuid,uuid,text,text,jsonb,text,text,text,integer,text,text) TO service_role;
COMMIT;
