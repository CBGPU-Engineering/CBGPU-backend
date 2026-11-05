BEGIN;
CREATE TABLE IF NOT EXISTS public.study_auto_settings (
 user_id uuid PRIMARY KEY REFERENCES public.profiles(id) ON DELETE CASCADE,
 enabled boolean NOT NULL DEFAULT false,
 starts_on date NOT NULL DEFAULT (now() AT TIME ZONE 'Asia/Seoul')::date,
 run_time time NOT NULL CHECK(run_time BETWEEN TIME '07:30' AND TIME '15:30' AND EXTRACT(second FROM run_time)=0),
 room_id text NOT NULL CHECK(length(room_id) BETWEEN 1 AND 100),
 seat_id text NOT NULL CHECK(length(seat_id) BETWEEN 1 AND 100),
 teacher_id text NOT NULL CHECK(length(teacher_id) BETWEEN 1 AND 100),
 reason text NOT NULL CHECK(length(btrim(reason)) BETWEEN 1 AND 1000),
 periods smallint[] NOT NULL CHECK(cardinality(periods) BETWEEN 1 AND 12 AND periods <@ ARRAY[1,2,3,4,5,6,7,8,9,10,11,12]::smallint[]),
 updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS study_auto_enabled_time ON public.study_auto_settings(run_time,user_id) WHERE enabled;
CREATE TABLE IF NOT EXISTS public.study_auto_runs (
 user_id uuid NOT NULL REFERENCES public.profiles(id) ON DELETE CASCADE,
 study_date date NOT NULL, period smallint NOT NULL CHECK(period BETWEEN 1 AND 12),
 status text NOT NULL CHECK(status IN ('running','applied','already_applied','no_seats','failed','uncertain','unavailable')),
 message text NOT NULL DEFAULT '', seat_name text,room_name text,
 started_at timestamptz NOT NULL DEFAULT now(), finished_at timestamptz,
 PRIMARY KEY(user_id,study_date,period)
);
CREATE INDEX IF NOT EXISTS study_auto_runs_recent ON public.study_auto_runs(user_id,study_date DESC);
CREATE TABLE IF NOT EXISTS public.study_submission_claims (
 student_id text NOT NULL, study_date date NOT NULL, period smallint NOT NULL,
 user_id uuid NOT NULL REFERENCES public.profiles(id) ON DELETE CASCADE,
 lease_token uuid NOT NULL DEFAULT gen_random_uuid(),
 state text NOT NULL CHECK(state IN ('checking','submitting','confirmed','released','rejected','uncertain')),
 updated_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(student_id,study_date,period)
);
CREATE INDEX IF NOT EXISTS study_submission_claims_owner ON public.study_submission_claims(user_id);
ALTER TABLE public.study_auto_settings ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.study_auto_runs ENABLE ROW LEVEL SECURITY;
ALTER TABLE public.study_submission_claims ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON public.study_auto_settings,public.study_auto_runs,public.study_submission_claims FROM PUBLIC,anon,authenticated;
GRANT SELECT ON public.study_auto_settings,public.study_auto_runs TO authenticated;
GRANT ALL ON public.study_auto_settings,public.study_auto_runs,public.study_submission_claims TO service_role;
DROP POLICY IF EXISTS study_auto_settings_owner ON public.study_auto_settings;
CREATE POLICY study_auto_settings_owner ON public.study_auto_settings FOR SELECT TO authenticated USING(user_id=(SELECT auth.uid()));
DROP POLICY IF EXISTS study_auto_runs_owner ON public.study_auto_runs;
CREATE POLICY study_auto_runs_owner ON public.study_auto_runs FOR SELECT TO authenticated USING(user_id=(SELECT auth.uid()));
CREATE OR REPLACE FUNCTION public.reserve_study_submission(p_student text,p_date date,p_period smallint,p_owner uuid) RETURNS uuid
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE token uuid;
BEGIN
 INSERT INTO public.study_submission_claims(student_id,study_date,period,user_id,state)
 VALUES(p_student,p_date,p_period,p_owner,'checking')
 ON CONFLICT(student_id,study_date,period) DO UPDATE SET user_id=EXCLUDED.user_id,state='checking',lease_token=gen_random_uuid(),updated_at=now()
 WHERE public.study_submission_claims.state IN ('released','rejected')
 OR (public.study_submission_claims.state='checking' AND public.study_submission_claims.updated_at<now()-interval '5 minutes')
 RETURNING lease_token INTO token;
 RETURN token;
END $$;
REVOKE ALL ON FUNCTION public.reserve_study_submission(text,date,smallint,uuid) FROM PUBLIC,anon,authenticated;
GRANT EXECUTE ON FUNCTION public.reserve_study_submission(text,date,smallint,uuid) TO service_role;
NOTIFY pgrst,'reload schema';
COMMIT;
