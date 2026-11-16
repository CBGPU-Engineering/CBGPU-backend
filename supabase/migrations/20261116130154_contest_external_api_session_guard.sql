-- Private read-only session oracle for the native external contest API.
-- No account, reservation, project or registration data is modified.
BEGIN;
DO $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_contests_auth_reader') THEN
  CREATE ROLE cbgpu_contests_auth_reader NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
 ELSIF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_contests_auth_reader' AND
   (rolcanlogin OR rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls)) THEN
  RAISE EXCEPTION 'Unexpected privileges on contest authentication group';
 END IF;
END $$;
CREATE SCHEMA IF NOT EXISTS cbgpu_contests_auth;
REVOKE ALL ON SCHEMA cbgpu_contests_auth FROM PUBLIC,anon,authenticated;
GRANT USAGE ON SCHEMA cbgpu_contests_auth TO cbgpu_contests_auth_reader;

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
 RETURN QUERY SELECT active,coalesce(allowed,false);
END $$;
REVOKE ALL ON FUNCTION cbgpu_contests_auth.session_status(uuid,uuid) FROM PUBLIC,anon,authenticated;
GRANT EXECUTE ON FUNCTION cbgpu_contests_auth.session_status(uuid,uuid) TO cbgpu_contests_auth_reader;
COMMENT ON FUNCTION cbgpu_contests_auth.session_status(uuid,uuid) IS
 'Private session validity and legacy approval only; never grants contest or school-resource write permission.';
COMMIT;
