-- 요청이 중간에 끊겨 'submitting'/'uncertain'으로 남은 기록이 영원히 재신청을 막지 않도록 10분 뒤 만료시킨다.
-- confirmed(서버에 신청이 확정된 기록)는 그대로 막는다.
CREATE OR REPLACE FUNCTION public.reserve_study_submission(p_student text,p_date date,p_period smallint,p_owner uuid) RETURNS uuid
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE token uuid;
BEGIN
 INSERT INTO public.study_submission_claims(student_id,study_date,period,user_id,state)
 VALUES(p_student,p_date,p_period,p_owner,'checking')
 ON CONFLICT(student_id,study_date,period) DO UPDATE SET user_id=EXCLUDED.user_id,state='checking',lease_token=gen_random_uuid(),updated_at=now()
 WHERE public.study_submission_claims.state IN ('released','rejected')
 OR (public.study_submission_claims.state='checking' AND public.study_submission_claims.updated_at<now()-interval '5 minutes')
 OR (public.study_submission_claims.state IN ('submitting','uncertain') AND public.study_submission_claims.updated_at<now()-interval '10 minutes')
 RETURNING lease_token INTO token;
 RETURN token;
END $$;
