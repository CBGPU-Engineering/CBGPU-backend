BEGIN;
CREATE TABLE IF NOT EXISTS public.eduset_student_connections (
 user_id uuid PRIMARY KEY REFERENCES auth.users(id) ON DELETE CASCADE,
 student_id text NOT NULL,
 token_enc text NOT NULL,
 expires_at timestamptz NOT NULL,
 connected_at timestamptz NOT NULL DEFAULT now()
);
ALTER TABLE public.eduset_student_connections ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON public.eduset_student_connections FROM PUBLIC, anon, authenticated;
GRANT SELECT,INSERT,UPDATE,DELETE ON public.eduset_student_connections TO service_role;
COMMENT ON TABLE public.eduset_student_connections IS 'Server-only encrypted personal EduSet sessions. Never exposed to browser database clients.';
COMMIT;
NOTIFY pgrst, 'reload schema';
