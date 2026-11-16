-- Candidate only. Separate explicit approval is required before applying this schema or enabling PAT routes.
BEGIN;
DO $$BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_contest_tokens_backend')THEN
  CREATE ROLE cbgpu_contest_tokens_backend NOLOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
 ELSIF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_contest_tokens_backend' AND(rolcanlogin OR rolinherit OR rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls))THEN RAISE EXCEPTION 'Unexpected contest token backend privileges';END IF;
 IF EXISTS(SELECT 1 FROM pg_auth_members WHERE member=(SELECT oid FROM pg_roles WHERE rolname='cbgpu_contest_tokens_backend'))THEN RAISE EXCEPTION 'Contest token backend must not inherit other roles';END IF;
 IF EXISTS(SELECT 1 FROM pg_namespace WHERE nspowner=(SELECT oid FROM pg_roles WHERE rolname='cbgpu_contest_tokens_backend'))OR EXISTS(SELECT 1 FROM pg_class WHERE relowner=(SELECT oid FROM pg_roles WHERE rolname='cbgpu_contest_tokens_backend'))THEN RAISE EXCEPTION 'Contest token backend must not own schemas or tables';END IF;
END$$;
-- This non-login helper role reads contests under the real authenticated RLS policies.
-- The API execution role is deliberately NOT a member of it.
DO $$BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_contest_policy_reader')THEN
  CREATE ROLE cbgpu_contest_policy_reader NOLOGIN INHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
 ELSIF EXISTS(SELECT 1 FROM pg_roles WHERE rolname='cbgpu_contest_policy_reader'AND(rolcanlogin OR NOT rolinherit OR rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls))THEN RAISE EXCEPTION 'Unexpected contest policy reader privileges';END IF;
 IF EXISTS(SELECT 1 FROM pg_auth_members m JOIN pg_roles r ON r.oid=m.roleid WHERE m.member=(SELECT oid FROM pg_roles WHERE rolname='cbgpu_contest_policy_reader')AND r.rolname<>'authenticated')THEN RAISE EXCEPTION 'Unexpected contest policy reader membership';END IF;
 IF EXISTS(SELECT 1 FROM pg_class WHERE relowner=(SELECT oid FROM pg_roles WHERE rolname='cbgpu_contest_policy_reader'))OR EXISTS(SELECT 1 FROM pg_namespace WHERE nspowner=(SELECT oid FROM pg_roles WHERE rolname='cbgpu_contest_policy_reader'))THEN RAISE EXCEPTION 'Policy reader must not own relations or schemas';END IF;
END$$;
GRANT authenticated TO cbgpu_contest_policy_reader;
CREATE SCHEMA IF NOT EXISTS contest_api;
REVOKE ALL ON SCHEMA contest_api FROM PUBLIC,anon,authenticated;
GRANT USAGE ON SCHEMA contest_api TO cbgpu_contest_tokens_backend,cbgpu_contest_policy_reader;
-- Contest/Auth lookup privileges come only from authenticated membership, so future ACL revocation also fails closed.
CREATE TABLE IF NOT EXISTS contest_api.tokens(
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),owner uuid NOT NULL,parent_session uuid NOT NULL,
 name text NOT NULL CHECK(char_length(btrim(name))BETWEEN 1 AND 80),token_sha256 bytea NOT NULL UNIQUE CHECK(octet_length(token_sha256)=32),
 scopes text[] NOT NULL CHECK(cardinality(scopes)BETWEEN 1 AND 3 AND scopes<@ARRAY['contests:read','contests:register','contests:cancel']::text[] AND array_position(scopes,NULL)IS NULL),
 allowed_contests uuid[] NOT NULL CHECK(cardinality(allowed_contests)BETWEEN 1 AND 100 AND array_position(allowed_contests,NULL)IS NULL),
 created_at timestamptz NOT NULL DEFAULT clock_timestamp(),expires_at timestamptz NOT NULL,
 revoked_at timestamptz,last_used_at timestamptz,
 CHECK(isfinite(expires_at)AND expires_at>created_at AND expires_at<=created_at+interval'2160 hours')
);
CREATE INDEX IF NOT EXISTS contest_api_tokens_owner ON contest_api.tokens(owner,created_at DESC);
ALTER TABLE contest_api.tokens ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON ALL TABLES IN SCHEMA contest_api FROM PUBLIC,anon,authenticated,cbgpu_contest_tokens_backend;

CREATE OR REPLACE FUNCTION contest_api._metadata(t contest_api.tokens)RETURNS jsonb LANGUAGE sql IMMUTABLE SET search_path='' AS $$
 SELECT jsonb_build_object('id',t.id,'name',t.name,'scopes',t.scopes,'allowed_contest_ids',t.allowed_contests,'created_at',t.created_at,'expires_at',t.expires_at,'revoked_at',t.revoked_at,'last_used_at',t.last_used_at)
$$;
CREATE OR REPLACE FUNCTION contest_api._registration(r public.event_registrations)RETURNS jsonb LANGUAGE sql IMMUTABLE SET search_path='' AS $$
 SELECT jsonb_build_object('id',r.id,'contest_id',r.contest_id,'user_id',r.user_id,'attendee_name',r.attendee_name,'answers',r.answers,'status',r.status,'ticket_code',r.ticket_code,'checked_in_at',r.checked_in_at,'created_at',r.created_at,'updated_at',r.updated_at,'participation_kind',r.participation_kind)
$$;
CREATE OR REPLACE FUNCTION contest_api._receipt(r public.event_registrations)RETURNS jsonb LANGUAGE sql IMMUTABLE SET search_path='' AS $$
 SELECT jsonb_build_object('id',r.id,'contest_id',r.contest_id,'status',r.status,'participation_kind',r.participation_kind)
$$;
CREATE OR REPLACE FUNCTION contest_api._policy_contest(p_id uuid)RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' SET row_security=on AS $$
 SELECT jsonb_build_object('id',c.id,'title',c.title,'description',c.description,'created_at',c.created_at,'fields',c.fields,'is_open',c.is_open,'ends_at',c.ends_at,'starts_at',c.starts_at,'event_start_at',c.event_start_at,'event_end_at',c.event_end_at,'location_type',c.location_type,'location_name',c.location_name,'address',c.address,'visibility',c.visibility,'capacity',c.capacity,'waitlist_enabled',c.waitlist_enabled,'approval_required',c.approval_required,'registration_open',c.registration_open,'registration_questions',c.registration_questions,'registration_opens_at',c.registration_opens_at,'registration_closes_at',c.registration_closes_at,'lifecycle_status',c.lifecycle_status,'cancellation_message',c.cancellation_message,'participation_mode',c.participation_mode,'team_min_size',c.team_min_size,'team_max_size',c.team_max_size,'allow_individual_entries',c.allow_individual_entries,'banner_path',c.banner_path)
 FROM public.contests c WHERE c.id=p_id
$$;
CREATE OR REPLACE FUNCTION contest_api._policy_visible(p_id uuid)RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' SET row_security=on AS $$
 SELECT EXISTS(SELECT 1 FROM public.contests c WHERE c.id=p_id)
$$;
GRANT CREATE ON SCHEMA contest_api TO cbgpu_contest_policy_reader;
ALTER FUNCTION contest_api._policy_contest(uuid)OWNER TO cbgpu_contest_policy_reader;
ALTER FUNCTION contest_api._policy_visible(uuid)OWNER TO cbgpu_contest_policy_reader;
REVOKE CREATE ON SCHEMA contest_api FROM cbgpu_contest_policy_reader;
CREATE OR REPLACE FUNCTION contest_api._contest(p_id uuid)RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$SELECT contest_api._policy_contest(p_id)$$;
-- No helper is executable by the backend role; it receives only the six narrow outer functions below.
CREATE OR REPLACE FUNCTION contest_api._jwt_actor()RETURNS TABLE(owner uuid,session_id uuid,approved boolean)
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE actor uuid:=auth.uid();sid uuid;state record;
BEGIN
 BEGIN sid:=(auth.jwt()->>'session_id')::uuid;EXCEPTION WHEN OTHERS THEN RAISE EXCEPTION 'TOKEN_AUTH_REQUIRED' USING ERRCODE='PT401';END;
 SELECT * INTO state FROM cbgpu_contests_auth.session_status(actor,sid);
 IF actor IS NULL OR state.valid IS DISTINCT FROM true THEN RAISE EXCEPTION 'TOKEN_AUTH_REQUIRED' USING ERRCODE='PT401';END IF;
 RETURN QUERY SELECT actor,sid,state.approved;
END$$;
CREATE OR REPLACE FUNCTION contest_api._registration_allowed(legacy_approved boolean)RETURNS boolean
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE allowed boolean;
BEGIN
 IF to_regprocedure('public.cbgpu_can_use_resource(text,text)')IS NOT NULL THEN
  EXECUTE 'SELECT public.cbgpu_can_use_resource($1,$2)' INTO allowed USING 'contests','cbsh';RETURN coalesce(allowed,false);
 END IF;
 RETURN coalesce(legacy_approved,false);
END$$;
CREATE OR REPLACE FUNCTION contest_api.mint(p_sha256 bytea,p_name text,p_scopes text[],p_contests uuid[],p_days integer DEFAULT 30)RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE actor record;t contest_api.tokens;scope_set text[];contest_set uuid[];created timestamptz:=clock_timestamp();
BEGIN
 SELECT * INTO actor FROM contest_api._jwt_actor();
 IF octet_length(p_sha256)IS DISTINCT FROM 32 OR p_name IS NULL OR char_length(btrim(p_name))NOT BETWEEN 1 AND 80 OR p_days IS NULL OR p_days NOT BETWEEN 1 AND 90 OR p_scopes IS NULL OR p_contests IS NULL OR cardinality(p_scopes)NOT BETWEEN 1 AND 3 OR cardinality(p_contests)NOT BETWEEN 1 AND 100 OR array_position(p_scopes,NULL)IS NOT NULL OR array_position(p_contests,NULL)IS NOT NULL THEN RAISE EXCEPTION 'INVALID_TOKEN_REQUEST' USING ERRCODE='PT400';END IF;
 scope_set:=ARRAY(SELECT DISTINCT s FROM unnest(p_scopes)s ORDER BY s);contest_set:=ARRAY(SELECT DISTINCT c FROM unnest(p_contests)c ORDER BY c);
 IF cardinality(scope_set)NOT BETWEEN 1 AND 3 OR NOT(scope_set<@ARRAY['contests:read','contests:register','contests:cancel']::text[]) OR cardinality(contest_set)NOT BETWEEN 1 AND 100 THEN RAISE EXCEPTION 'INVALID_TOKEN_SCOPE' USING ERRCODE='PT400';END IF;
 IF 'contests:register'=ANY(scope_set) AND NOT contest_api._registration_allowed(actor.approved)THEN RAISE EXCEPTION 'CONTEST_REGISTRATION_PERMISSION_REQUIRED' USING ERRCODE='PT403';END IF;
 IF(SELECT count(*)FROM unnest(contest_set)c WHERE contest_api._policy_visible(c))<>cardinality(contest_set)THEN RAISE EXCEPTION 'CONTEST_NOT_FOUND' USING ERRCODE='PT404';END IF;
 PERFORM pg_advisory_xact_lock(hashtextextended('contest-token-owner:'||actor.owner::text,0));
 IF(SELECT count(*)FROM contest_api.tokens existing_token JOIN auth.sessions s ON s.id=existing_token.parent_session AND s.user_id=existing_token.owner WHERE existing_token.owner=actor.owner AND existing_token.revoked_at IS NULL AND existing_token.expires_at>created AND(s.not_after IS NULL OR s.not_after>created))>=20 THEN RAISE EXCEPTION 'TOKEN_QUOTA_REACHED' USING ERRCODE='PT429';END IF;
 INSERT INTO contest_api.tokens(owner,parent_session,name,token_sha256,scopes,allowed_contests,created_at,expires_at)VALUES(actor.owner,actor.session_id,btrim(p_name),p_sha256,scope_set,contest_set,created,created+make_interval(days=>p_days))RETURNING * INTO t;
 RETURN contest_api._metadata(t);
END$$;
CREATE OR REPLACE FUNCTION contest_api.list_owned()RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE actor record;items jsonb;
BEGIN SELECT * INTO actor FROM contest_api._jwt_actor();SELECT coalesce(jsonb_agg(contest_api._metadata(t)ORDER BY t.created_at DESC),'[]'::jsonb)INTO items FROM(SELECT * FROM contest_api.tokens WHERE owner=actor.owner ORDER BY created_at DESC LIMIT 100)t;RETURN jsonb_build_object('tokens',items);END$$;
CREATE OR REPLACE FUNCTION contest_api.revoke_owned(p_id uuid)RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE actor record;
BEGIN SELECT * INTO actor FROM contest_api._jwt_actor();UPDATE contest_api.tokens SET revoked_at=coalesce(revoked_at,clock_timestamp())WHERE id=p_id AND owner=actor.owner;RETURN FOUND;END$$;
CREATE OR REPLACE FUNCTION contest_api._authorize(p_sha256 bytea,p_scope text,p_contest uuid DEFAULT NULL)RETURNS contest_api.tokens
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE t contest_api.tokens;approved boolean;
BEGIN
 IF octet_length(p_sha256)IS DISTINCT FROM 32 THEN RAISE EXCEPTION 'INVALID_CONTEST_TOKEN' USING ERRCODE='PT401';END IF;
 SELECT * INTO t FROM contest_api.tokens WHERE token_sha256=p_sha256 FOR UPDATE;
 IF NOT FOUND OR t.revoked_at IS NOT NULL OR t.expires_at<=clock_timestamp()THEN RAISE EXCEPTION 'INVALID_CONTEST_TOKEN' USING ERRCODE='PT401';END IF;
 IF NOT(p_scope=ANY(t.scopes))OR(p_contest IS NOT NULL AND NOT(p_contest=ANY(t.allowed_contests)))THEN RAISE EXCEPTION 'CONTEST_TOKEN_SCOPE_DENIED' USING ERRCODE='PT403';END IF;
 -- Share locks serialize session deletion, bans, and affiliation revocation with this operation.
 SELECT p.is_approved IS DISTINCT FROM false INTO approved FROM auth.sessions s JOIN auth.users u ON u.id=s.user_id JOIN public.profiles p ON p.id=u.id WHERE s.id=t.parent_session AND s.user_id=t.owner AND(s.not_after IS NULL OR s.not_after>clock_timestamp())AND u.deleted_at IS NULL AND(u.banned_until IS NULL OR u.banned_until<=clock_timestamp())AND(to_jsonb(p)->>'account_status')IS DISTINCT FROM 'SUSPENDED' FOR SHARE OF s,u,p;
 IF NOT FOUND THEN RAISE EXCEPTION 'CONTEST_TOKEN_SESSION_REVOKED' USING ERRCODE='PT401';END IF;
 PERFORM set_config('request.jwt.claims',jsonb_build_object('role','authenticated','sub',t.owner,'session_id',t.parent_session)::text,true);
 PERFORM set_config('request.jwt.claim.sub',t.owner::text,true);
 IF p_scope='contests:register'AND NOT contest_api._registration_allowed(approved)THEN RAISE EXCEPTION 'CONTEST_REGISTRATION_PERMISSION_REQUIRED' USING ERRCODE='PT403';END IF;
 UPDATE contest_api.tokens SET last_used_at=clock_timestamp()WHERE id=t.id;
 RETURN t;
END$$;
CREATE OR REPLACE FUNCTION contest_api.read_allowed(p_sha256 bytea,p_id uuid DEFAULT NULL,p_own boolean DEFAULT false)RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE t contest_api.tokens;item jsonb;r public.event_registrations;
BEGIN
 t:=contest_api._authorize(p_sha256,'contests:read',p_id);
 IF p_id IS NULL THEN
  IF p_own THEN RAISE EXCEPTION 'CONTEST_ID_REQUIRED' USING ERRCODE='PT400';END IF;
  SELECT coalesce(jsonb_agg(visible.item ORDER BY visible.created_at DESC),'[]'::jsonb)INTO item FROM(SELECT contest_api._contest(c.id)AS item,c.created_at FROM public.contests c WHERE c.id=ANY(t.allowed_contests)AND c.visibility='public'AND c.lifecycle_status='published')visible WHERE visible.item IS NOT NULL;RETURN jsonb_build_object('contests',item);
 END IF;
 IF p_own THEN SELECT * INTO r FROM public.event_registrations WHERE contest_id=p_id AND user_id=t.owner;RETURN jsonb_build_object('registration',CASE WHEN FOUND THEN contest_api._registration(r)ELSE NULL END);END IF;
 item:=contest_api._contest(p_id);IF item IS NULL THEN RAISE EXCEPTION 'CONTEST_NOT_FOUND' USING ERRCODE='PT404';END IF;RETURN jsonb_build_object('contest',item);
END$$;
CREATE OR REPLACE FUNCTION contest_api.register_allowed(p_sha256 bytea,p_id uuid,p_name text,p_answers jsonb DEFAULT'{}',p_kind text DEFAULT'individual',p_team_name text DEFAULT NULL,p_join_code uuid DEFAULT NULL)RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE t contest_api.tokens;r public.event_registrations;
BEGIN
 PERFORM pg_advisory_xact_lock(hashtextextended(p_id::text,0));
 t:=contest_api._authorize(p_sha256,'contests:register',p_id);
 IF NOT contest_api._policy_visible(p_id)THEN RAISE EXCEPTION 'CONTEST_NOT_FOUND' USING ERRCODE='PT404';END IF;
 SELECT * INTO r FROM public.register_contest_entry(p_id,p_name,p_answers,p_kind,p_team_name,p_join_code);
 IF NOT FOUND OR r.user_id<>t.owner THEN RAISE EXCEPTION 'REGISTRATION_IDENTITY_MISMATCH' USING ERRCODE='42501';END IF;
 RETURN jsonb_build_object('registration',CASE WHEN 'contests:read'=ANY(t.scopes)THEN contest_api._registration(r)ELSE contest_api._receipt(r)END);
END$$;
CREATE OR REPLACE FUNCTION contest_api.cancel_allowed(p_sha256 bytea,p_id uuid)RETURNS jsonb
LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE t contest_api.tokens;r public.event_registrations;
BEGIN
 PERFORM pg_advisory_xact_lock(hashtextextended(p_id::text,0));
 t:=contest_api._authorize(p_sha256,'contests:cancel',p_id);
 SELECT * INTO r FROM public.event_registrations WHERE contest_id=p_id AND user_id=t.owner FOR UPDATE;
 IF NOT FOUND THEN RAISE EXCEPTION 'REGISTRATION_NOT_FOUND' USING ERRCODE='PT404';END IF;
 IF r.status<>'cancelled'THEN UPDATE public.event_registrations SET status='cancelled'WHERE id=r.id AND user_id=t.owner RETURNING * INTO r;END IF;
 RETURN jsonb_build_object('registration',CASE WHEN 'contests:read'=ANY(t.scopes)THEN contest_api._registration(r)ELSE contest_api._receipt(r)END);
END$$;
REVOKE ALL ON ALL FUNCTIONS IN SCHEMA contest_api FROM PUBLIC,anon,authenticated,cbgpu_contest_tokens_backend;
GRANT EXECUTE ON FUNCTION contest_api.mint(bytea,text,text[],uuid[],integer),contest_api.list_owned(),contest_api.revoke_owned(uuid),contest_api.read_allowed(bytea,uuid,boolean),contest_api.register_allowed(bytea,uuid,text,jsonb,text,text,uuid),contest_api.cancel_allowed(bytea,uuid)TO cbgpu_contest_tokens_backend;
COMMENT ON SCHEMA contest_api IS 'Private scoped automation grants. No public/PostgREST exposure. Never stores plaintext bearer credentials or mints Auth JWTs.';
COMMIT;
