-- Fresh contest approval adopts the separate account resource oracle when installed.
-- No account changes or guest grants; legacy behavior is preserved without that oracle.
BEGIN;
CREATE OR REPLACE FUNCTION cbgpu_contests_auth.session_status(p_user uuid,p_session uuid)
RETURNS TABLE(valid boolean,approved boolean)
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE allowed boolean; active boolean;
BEGIN
 -- The trusted API sets these transaction-local claims only AFTER GoTrue verifies
 -- the exact bearer token. Ordinary Data API roles cannot execute this function.
 IF p_user IS NULL OR p_session IS NULL
    OR (SELECT auth.uid()) IS DISTINCT FROM p_user
    OR coalesce((SELECT auth.jwt())->>'session_id','')<>p_session::text THEN
  RETURN QUERY SELECT false,false; RETURN;
 END IF;
 SELECT p.is_approved IS DISTINCT FROM false INTO allowed
 FROM auth.sessions s JOIN auth.users u ON u.id=s.user_id
 JOIN public.profiles p ON p.id=u.id
 WHERE s.id=p_session AND s.user_id=p_user
   AND (s.not_after IS NULL OR s.not_after>clock_timestamp())
   AND u.deleted_at IS NULL
   AND (u.banned_until IS NULL OR u.banned_until<=clock_timestamp())
   AND (to_jsonb(p)->>'account_status') IS DISTINCT FROM 'SUSPENDED';
 active:=FOUND;
 IF active AND pg_catalog.to_regprocedure('cbgpu_accounts.can_user_use_resource(uuid,text,text)') IS NOT NULL THEN
  EXECUTE 'SELECT cbgpu_accounts.can_user_use_resource($1,$2,$3)' INTO allowed USING p_user,'contests','cbsh';
 END IF;
 RETURN QUERY SELECT active,coalesce(allowed,false);
END $$;
REVOKE ALL ON FUNCTION cbgpu_contests_auth.session_status(uuid,uuid) FROM PUBLIC,anon,authenticated;
GRANT EXECUTE ON FUNCTION cbgpu_contests_auth.session_status(uuid,uuid) TO cbgpu_contests_auth_reader;
COMMIT;
