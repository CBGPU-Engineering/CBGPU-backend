-- Source-only notebook execution. Explicit operator enable + digest required; legacy GPU1..4 unchanged.
BEGIN;
CREATE TABLE IF NOT EXISTS cbgpu_schools.gpu_notebook_policy(singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),enabled boolean NOT NULL DEFAULT false,image_digest text CHECK(image_digest~'^[a-zA-Z0-9./:_-]+@sha256:[0-9a-f]{64}$'),updated_at timestamptz NOT NULL DEFAULT now());
INSERT INTO cbgpu_schools.gpu_notebook_policy(singleton)VALUES(true)ON CONFLICT DO NOTHING;
CREATE TABLE IF NOT EXISTS cbgpu_schools.gpu_executor_capabilities(node_id uuid PRIMARY KEY REFERENCES cbgpu_schools.gpu_nodes,credential_version bigint NOT NULL,last_sequence bigint NOT NULL DEFAULT 0,protocol integer NOT NULL DEFAULT 0,image_digest text,hardware_uuids text[] NOT NULL DEFAULT '{}',ready boolean NOT NULL DEFAULT false,relay_seen_at timestamptz,checked_at timestamptz NOT NULL DEFAULT now(),expires_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE IF NOT EXISTS cbgpu_schools.gpu_notebook_jobs(id uuid PRIMARY KEY DEFAULT gen_random_uuid(),reservation_id uuid NOT NULL UNIQUE REFERENCES cbgpu_schools.gpu_resource_reservations,resource_id uuid NOT NULL REFERENCES cbgpu_schools.gpu_resources,node_id uuid NOT NULL REFERENCES cbgpu_schools.gpu_nodes,school_id text NOT NULL REFERENCES cbgpu_schools.schools,owner_id uuid NOT NULL REFERENCES auth.users,requested_session_id uuid NOT NULL,idempotency_key uuid NOT NULL,relay_session_id uuid NOT NULL DEFAULT gen_random_uuid(),status text NOT NULL DEFAULT 'QUEUED' CHECK(status IN('QUEUED','STARTING','RUNNING','STOPPING','STOPPED','FAILED')),ends_at timestamptz NOT NULL,image_digest text NOT NULL,error_code text,created_at timestamptz NOT NULL DEFAULT now(),updated_at timestamptz NOT NULL DEFAULT now());
CREATE UNIQUE INDEX IF NOT EXISTS gpu_notebook_resource_single_execution ON cbgpu_schools.gpu_notebook_jobs(resource_id)WHERE status NOT IN('STOPPED','FAILED');
CREATE TABLE IF NOT EXISTS cbgpu_schools.gpu_notebook_commands(id uuid PRIMARY KEY DEFAULT gen_random_uuid(),job_id uuid NOT NULL REFERENCES cbgpu_schools.gpu_notebook_jobs,kind text NOT NULL CHECK(kind IN('START_NOTEBOOK','STOP_NOTEBOOK')),state text NOT NULL DEFAULT 'PENDING' CHECK(state IN('PENDING','LEASED','COMPLETED','CANCELLED')),lease_epoch bigint NOT NULL DEFAULT 0,lease_token_hash bytea,lease_expires_at timestamptz,receipt_hash bytea,created_at timestamptz NOT NULL DEFAULT now(),UNIQUE(job_id,kind));
CREATE TABLE IF NOT EXISTS cbgpu_schools.gpu_notebook_sessions(token_hash bytea PRIMARY KEY,job_id uuid NOT NULL REFERENCES cbgpu_schools.gpu_notebook_jobs,user_id uuid NOT NULL REFERENCES auth.users,auth_session_id uuid NOT NULL,expires_at timestamptz NOT NULL,created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE IF NOT EXISTS cbgpu_schools.gpu_notebook_audit(id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,job_id uuid,actor_id uuid,event text NOT NULL,detail jsonb NOT NULL DEFAULT '{}',created_at timestamptz NOT NULL DEFAULT now());
DO $$DECLARE t text;BEGIN FOREACH t IN ARRAY ARRAY['gpu_notebook_policy','gpu_executor_capabilities','gpu_notebook_jobs','gpu_notebook_commands','gpu_notebook_sessions','gpu_notebook_audit']LOOP EXECUTE format('ALTER TABLE cbgpu_schools.%I ENABLE ROW LEVEL SECURITY',t);EXECUTE format('REVOKE ALL ON cbgpu_schools.%I FROM PUBLIC,anon,authenticated,service_role',t);END LOOP;END$$;
CREATE OR REPLACE FUNCTION cbgpu_schools.gpu_notebook_permission(subject uuid,school text,from_time timestamptz,to_time timestamptz)RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
SELECT from_time<to_time AND cbgpu_schools.feature_enabled(school,'gpu_compute') AND EXISTS(SELECT FROM cbgpu_schools.schools s WHERE s.id=school AND s.status='APPROVED') AND EXISTS(SELECT FROM auth.users u JOIN public.profiles p ON p.id=u.id WHERE u.id=subject AND u.email_confirmed_at IS NOT NULL AND u.deleted_at IS NULL AND(u.banned_until IS NULL OR u.banned_until<=clock_timestamp())AND p.account_status IN('ACTIVE','PENDING_VERIFICATION','REVERIFICATION_REQUIRED'))AND(
EXISTS(SELECT FROM cbgpu_schools.memberships m WHERE m.user_id=subject AND m.school_id=school AND m.membership_type IN('STUDENT','TEACHER')AND cbgpu_schools.active_member(subject,school)AND m.starts_at<=from_time AND(m.ends_at IS NULL OR m.ends_at>=to_time))OR(
NOT EXISTS(SELECT FROM cbgpu_schools.memberships m WHERE m.user_id=subject AND m.school_id=school AND(m.status='SUSPENDED' OR(m.membership_type IN('STUDENT','TEACHER')AND NOT cbgpu_schools.active_member(subject,school))))AND EXISTS(SELECT FROM cbgpu_accounts.guest_requests g WHERE g.external_user_id=subject AND g.school_id=school AND g.status='APPROVED'AND'gpu'=ANY(g.resources)AND g.starts_at<=from_time AND g.ends_at>=to_time AND cbgpu_schools.active_member(g.requester_id,school)AND EXISTS(SELECT FROM cbgpu_schools.memberships sponsor WHERE sponsor.user_id=g.requester_id AND sponsor.school_id=school AND sponsor.membership_type IN('STUDENT','TEACHER')))))
$$;
-- Literal scope stays the existing gpu resource; no new global permission.
CREATE OR REPLACE FUNCTION cbgpu_schools.gpu_notebook_job_allowed(j cbgpu_schools.gpu_notebook_jobs)RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
SELECT EXISTS(SELECT FROM cbgpu_schools.gpu_resource_reservations b JOIN cbgpu_schools.gpu_resources r ON r.id=b.resource_id JOIN cbgpu_schools.gpu_nodes n ON n.id=r.node_id JOIN cbgpu_schools.gpu_executor_capabilities c ON c.node_id=n.id CROSS JOIN cbgpu_schools.gpu_notebook_policy p WHERE b.id=j.reservation_id AND b.owner_id=j.owner_id AND r.id=j.resource_id AND n.id=j.node_id AND n.school_id=j.school_id AND b.status='RESERVED'AND b.starts_at<=clock_timestamp()AND b.ends_at>clock_timestamp()AND p.enabled AND p.image_digest=j.image_digest AND n.status='ONLINE'AND n.credential_version=c.credential_version AND n.last_seen_at>clock_timestamp()-interval'120 seconds'AND r.last_seen_at>clock_timestamp()-interval'120 seconds'AND r.reported_sequence=n.last_sequence AND c.relay_seen_at>clock_timestamp()-interval'60 seconds'AND c.ready AND c.protocol=3 AND c.image_digest=p.image_digest AND c.expires_at>clock_timestamp()AND r.hardware_uuid=ANY(c.hardware_uuids)AND cbgpu_schools.gpu_notebook_permission(j.owner_id,j.school_id,b.starts_at,b.ends_at)AND(j.status<>'RUNNING'OR EXISTS(SELECT FROM cbgpu_schools.gpu_notebook_commands cmd WHERE cmd.job_id=j.id AND cmd.kind='START_NOTEBOOK'AND cmd.state='COMPLETED'AND cmd.lease_expires_at>clock_timestamp())))
$$;
CREATE OR REPLACE FUNCTION cbgpu_schools.gpu_notebook_stop(jid uuid,why text)RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$BEGIN
 UPDATE cbgpu_schools.gpu_notebook_commands SET state='CANCELLED'WHERE job_id=jid AND kind='START_NOTEBOOK'AND state='PENDING';
 UPDATE cbgpu_schools.gpu_notebook_jobs SET status='STOPPING',updated_at=clock_timestamp()WHERE id=jid AND status NOT IN('STOPPED','FAILED');
 INSERT INTO cbgpu_schools.gpu_notebook_commands(job_id,kind)VALUES(jid,'STOP_NOTEBOOK')ON CONFLICT DO NOTHING;
 DELETE FROM cbgpu_schools.gpu_notebook_sessions WHERE job_id=jid;
 INSERT INTO cbgpu_schools.gpu_notebook_audit(job_id,event,detail)VALUES(jid,'STOP_REQUESTED',jsonb_build_object('reason',why));
END$$;
CREATE OR REPLACE FUNCTION public.school_gpu_notebook_api(p_school_id text,p_action text,p_payload jsonb DEFAULT '{}')RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE uid uuid:=auth.uid();b cbgpu_schools.gpu_resource_reservations;r cbgpu_schools.gpu_resources;n cbgpu_schools.gpu_nodes;j cbgpu_schools.gpu_notebook_jobs;pol cbgpu_schools.gpu_notebook_policy;tok text;k uuid;
BEGIN
 IF NOT public.cbgpu_personal_project_identity()THEN RAISE EXCEPTION'AUTHENTICATION_REQUIRED'USING ERRCODE='42501';END IF;
 IF jsonb_typeof(p_payload)<>'object'OR octet_length(p_payload::text)>4096 OR EXISTS(SELECT FROM jsonb_object_keys(p_payload)x WHERE x NOT IN('reservation_id','client_request_id','temporary_storage_consent'))THEN RAISE EXCEPTION'INVALID_REQUEST'USING ERRCODE='22023';END IF;
 SELECT b0.* INTO b FROM cbgpu_schools.gpu_resource_reservations b0 JOIN cbgpu_schools.gpu_resources r0 ON r0.id=b0.resource_id JOIN cbgpu_schools.gpu_nodes n0 ON n0.id=r0.node_id WHERE b0.id=(p_payload->>'reservation_id')::uuid AND b0.owner_id=uid AND n0.school_id=p_school_id FOR UPDATE OF b0;
 IF NOT FOUND THEN RAISE EXCEPTION'RESERVATION_NOT_FOUND'USING ERRCODE='P0002';END IF;
 SELECT * INTO j FROM cbgpu_schools.gpu_notebook_jobs WHERE reservation_id=b.id FOR UPDATE;
 IF p_action='stop'THEN IF j.id IS NOT NULL THEN PERFORM cbgpu_schools.gpu_notebook_stop(j.id,'OWNER_REQUEST');SELECT * INTO j FROM cbgpu_schools.gpu_notebook_jobs WHERE id=j.id;END IF;
 ELSIF p_action='launch'THEN
  IF p_payload->'temporary_storage_consent'IS DISTINCT FROM'true'::jsonb THEN RAISE EXCEPTION'TEMPORARY_STORAGE_CONSENT_REQUIRED'USING ERRCODE='22023';END IF;k:=(p_payload->>'client_request_id')::uuid;IF k IS NULL THEN RAISE EXCEPTION'IDEMPOTENCY_REQUIRED';END IF;
  IF j.id IS NULL THEN
   SELECT * INTO r FROM cbgpu_schools.gpu_resources WHERE id=b.resource_id;SELECT * INTO n FROM cbgpu_schools.gpu_nodes WHERE id=r.node_id;SELECT * INTO pol FROM cbgpu_schools.gpu_notebook_policy;
   IF NOT pol.enabled OR pol.image_digest IS NULL OR b.status<>'RESERVED'OR b.starts_at>clock_timestamp()OR b.ends_at<=clock_timestamp()OR NOT cbgpu_schools.gpu_notebook_permission(uid,p_school_id,b.starts_at,b.ends_at)THEN RAISE EXCEPTION'NOTEBOOK_ADMISSION_DENIED'USING ERRCODE='42501';END IF;
   INSERT INTO cbgpu_schools.gpu_notebook_jobs(reservation_id,resource_id,node_id,school_id,owner_id,requested_session_id,idempotency_key,ends_at,image_digest)VALUES(b.id,r.id,n.id,p_school_id,uid,(auth.jwt()->>'session_id')::uuid,k,b.ends_at,pol.image_digest)RETURNING*INTO j;
   IF NOT cbgpu_schools.gpu_notebook_job_allowed(j)THEN RAISE EXCEPTION'EXECUTOR_UNAVAILABLE'USING ERRCODE='55000';END IF;
   INSERT INTO cbgpu_schools.gpu_notebook_commands(job_id,kind)VALUES(j.id,'START_NOTEBOOK');INSERT INTO cbgpu_schools.gpu_notebook_audit(job_id,actor_id,event)VALUES(j.id,uid,'LAUNCH_REQUESTED');
  ELSIF j.idempotency_key<>k THEN RAISE EXCEPTION'RESERVATION_ALREADY_LAUNCHED'USING ERRCODE='23505';END IF;
 ELSIF p_action='session'THEN
  IF j.id IS NULL OR j.status<>'RUNNING'OR NOT cbgpu_schools.gpu_notebook_job_allowed(j)THEN RAISE EXCEPTION'NOTEBOOK_UNAVAILABLE'USING ERRCODE='42501';END IF;
  tok:=encode(extensions.gen_random_bytes(32),'hex');INSERT INTO cbgpu_schools.gpu_notebook_sessions(token_hash,job_id,user_id,auth_session_id,expires_at)VALUES(pg_catalog.sha256(convert_to(tok,'UTF8')),j.id,uid,(auth.jwt()->>'session_id')::uuid,least(j.ends_at,clock_timestamp()+interval'4 hours'));
  RETURN jsonb_build_object('cookie_token',tok,'url','/notebooks/'||j.id::text||'/lab','job_id',j.id,'expires_at',least(j.ends_at,clock_timestamp()+interval'4 hours'));
 ELSIF p_action<>'status'THEN RAISE EXCEPTION'INVALID_ACTION'USING ERRCODE='22023';END IF;
 RETURN jsonb_build_object('job',CASE WHEN j.id IS NULL THEN NULL ELSE jsonb_build_object('id',j.id,'reservation_id',j.reservation_id,'status',j.status,'ends_at',j.ends_at,'error_code',j.error_code)END,'notebook_url',NULL);
END$$;
CREATE OR REPLACE FUNCTION public.school_gpu_notebook_session(p_token text,p_job_id uuid)RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path='' AS $$
DECLARE s cbgpu_schools.gpu_notebook_sessions;j cbgpu_schools.gpu_notebook_jobs;BEGIN
 IF auth.role()IS DISTINCT FROM'service_role'THEN RAISE EXCEPTION'SERVICE_REQUIRED'USING ERRCODE='42501';END IF;
 IF p_token!~'^[0-9a-f]{64}$'THEN RETURN jsonb_build_object('valid',false);END IF;
 SELECT * INTO s FROM cbgpu_schools.gpu_notebook_sessions WHERE job_id=p_job_id AND token_hash=pg_catalog.sha256(convert_to(p_token,'UTF8'))AND expires_at>clock_timestamp();IF NOT FOUND THEN RETURN jsonb_build_object('valid',false);END IF;
 SELECT * INTO j FROM cbgpu_schools.gpu_notebook_jobs WHERE id=s.job_id;
 IF j.status<>'RUNNING'OR NOT cbgpu_schools.gpu_notebook_job_allowed(j)OR NOT EXISTS(SELECT FROM auth.sessions a JOIN auth.users u ON u.id=a.user_id JOIN public.profiles p ON p.id=u.id WHERE a.id=s.auth_session_id AND a.user_id=s.user_id AND(a.not_after IS NULL OR a.not_after>clock_timestamp())AND u.deleted_at IS NULL AND u.email_confirmed_at IS NOT NULL AND(u.banned_until IS NULL OR u.banned_until<=clock_timestamp())AND p.account_status<>'SUSPENDED')THEN RETURN jsonb_build_object('valid',false);END IF;
 RETURN jsonb_build_object('valid',true,'job_id',j.id,'node_id',j.node_id,'relay_session_id',j.relay_session_id,'credential_version',(select credential_version from cbgpu_schools.gpu_nodes where id=j.node_id),'expires_at',least(s.expires_at,j.ends_at));
END$$;
REVOKE ALL ON FUNCTION cbgpu_schools.gpu_notebook_permission(uuid,text,timestamptz,timestamptz),cbgpu_schools.gpu_notebook_job_allowed(cbgpu_schools.gpu_notebook_jobs),cbgpu_schools.gpu_notebook_stop(uuid,text)FROM PUBLIC,anon,authenticated,service_role;
REVOKE ALL ON FUNCTION public.school_gpu_notebook_api(text,text,jsonb)FROM PUBLIC,anon,service_role;GRANT EXECUTE ON FUNCTION public.school_gpu_notebook_api(text,text,jsonb)TO authenticated;
REVOKE ALL ON FUNCTION public.school_gpu_notebook_session(text,uuid)FROM PUBLIC,anon,authenticated;GRANT EXECUTE ON FUNCTION public.school_gpu_notebook_session(text,uuid)TO service_role;

CREATE OR REPLACE FUNCTION cbgpu_schools.gpu_notebook_spec(j cbgpu_schools.gpu_notebook_jobs)RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path=''AS $$SELECT jsonb_build_object('job_id',j.id,'school_id',j.school_id,'node_id',j.node_id,'resource_id',j.resource_id,'hardware_uuid',r.hardware_uuid,'reservation_id',j.reservation_id,'owner_id',j.owner_id,'not_before',extract(epoch FROM b.starts_at)::bigint,'deadline',extract(epoch FROM j.ends_at)::bigint,'image_digest',j.image_digest,'limits',jsonb_build_object('cpu_millis',2000,'memory_bytes',8589934592,'pids',256,'tmpfs_bytes',1073741824),'relay_session_id',j.relay_session_id)FROM cbgpu_schools.gpu_resources r JOIN cbgpu_schools.gpu_resource_reservations b ON b.resource_id=r.id WHERE b.id=j.reservation_id$$;
CREATE OR REPLACE FUNCTION public.school_gpu_notebook_device(p_node_id uuid,p_version bigint,p_sequence bigint,p_issued_at bigint,p_action text,p_payload jsonb)RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=''AS $$
DECLARE n cbgpu_schools.gpu_nodes;c cbgpu_schools.gpu_notebook_commands;j cbgpu_schools.gpu_notebook_jobs;cap cbgpu_schools.gpu_executor_capabilities;pol cbgpu_schools.gpu_notebook_policy;tok text;result_hash bytea;item text;st text;
BEGIN
 IF auth.role()IS DISTINCT FROM'service_role'THEN RAISE EXCEPTION'SERVICE_REQUIRED'USING ERRCODE='42501';END IF;
 IF p_sequence<1 OR abs(extract(epoch FROM clock_timestamp())-p_issued_at)>90 OR jsonb_typeof(p_payload)<>'object'OR octet_length(p_payload::text)>24000 THEN RAISE EXCEPTION'INVALID_REQUEST';END IF;
 SELECT * INTO n FROM cbgpu_schools.gpu_nodes WHERE id=p_node_id AND credential_version=p_version AND status<>'REVOKED'FOR UPDATE;
 IF NOT FOUND OR NOT EXISTS(SELECT FROM cbgpu_schools.schools WHERE id=n.school_id AND status='APPROVED')THEN RAISE EXCEPTION'NODE_UNAVAILABLE'USING ERRCODE='42501';END IF;
 INSERT INTO cbgpu_schools.gpu_executor_capabilities(node_id,credential_version)VALUES(n.id,p_version)ON CONFLICT DO NOTHING;
 SELECT * INTO cap FROM cbgpu_schools.gpu_executor_capabilities WHERE node_id=n.id FOR UPDATE;
 IF cap.credential_version<>p_version THEN UPDATE cbgpu_schools.gpu_executor_capabilities SET credential_version=p_version,last_sequence=0,ready=false WHERE node_id=n.id;cap.last_sequence:=0;END IF;
 IF p_sequence<=cap.last_sequence THEN RAISE EXCEPTION'SEQUENCE_REPLAY'USING ERRCODE='42501';END IF;
 UPDATE cbgpu_schools.gpu_executor_capabilities SET last_sequence=p_sequence WHERE node_id=n.id;
 SELECT * INTO pol FROM cbgpu_schools.gpu_notebook_policy;
 IF p_action='capabilities'THEN
  IF(p_payload->>'executor_protocol')::int<>3 OR jsonb_typeof(p_payload->'hardware_uuids')<>'array'OR jsonb_array_length(p_payload->'hardware_uuids')NOT BETWEEN 1 AND 16 THEN RAISE EXCEPTION'INVALID_CAPABILITIES';END IF;
  FOR item IN SELECT jsonb_array_elements_text(p_payload->'hardware_uuids')LOOP IF item!~'^GPU-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'OR NOT EXISTS(SELECT FROM cbgpu_schools.gpu_resources r WHERE r.node_id=n.id AND r.hardware_uuid=item AND r.credential_version=p_version)THEN RAISE EXCEPTION'UNREPORTED_HARDWARE';END IF;END LOOP;
  UPDATE cbgpu_schools.gpu_executor_capabilities SET protocol=3,image_digest=p_payload->>'image_digest',hardware_uuids=ARRAY(SELECT jsonb_array_elements_text(p_payload->'hardware_uuids')),ready=pol.enabled AND pol.image_digest=p_payload->>'image_digest'AND p_payload->'docker_ready'='true'::jsonb AND p_payload->'toolkit_ready'='true'::jsonb,checked_at=clock_timestamp(),expires_at=clock_timestamp()+interval'90 seconds'WHERE node_id=n.id;
  RETURN jsonb_build_object('command',NULL);
 END IF;
 -- Fresh denial schedules stop; never infers a stopped container from lease/heartbeat expiry.
 FOR j IN SELECT * FROM cbgpu_schools.gpu_notebook_jobs WHERE node_id=n.id AND status IN('QUEUED','STARTING','RUNNING')LOOP IF NOT cbgpu_schools.gpu_notebook_job_allowed(j)THEN PERFORM cbgpu_schools.gpu_notebook_stop(j.id,'AUTHORITY_OR_BOOKING_ENDED');END IF;END LOOP;
 IF p_action='claim'THEN
  SELECT c0.* INTO c FROM cbgpu_schools.gpu_notebook_commands c0 JOIN cbgpu_schools.gpu_notebook_jobs j0 ON j0.id=c0.job_id WHERE j0.node_id=n.id AND(c0.state='PENDING'OR(c0.state='LEASED'AND c0.lease_expires_at<=clock_timestamp()))ORDER BY CASE c0.kind WHEN'STOP_NOTEBOOK'THEN 0 ELSE 1 END,c0.created_at FOR UPDATE OF c0 SKIP LOCKED LIMIT 1;
  IF NOT FOUND THEN RETURN jsonb_build_object('command',NULL);END IF;
  SELECT * INTO j FROM cbgpu_schools.gpu_notebook_jobs WHERE id=c.job_id FOR UPDATE;
  IF c.kind='START_NOTEBOOK'AND(NOT cbgpu_schools.gpu_notebook_job_allowed(j)OR NOT EXISTS(SELECT FROM auth.sessions a JOIN auth.users u ON u.id=a.user_id WHERE a.id=j.requested_session_id AND a.user_id=j.owner_id AND(a.not_after IS NULL OR a.not_after>clock_timestamp())AND u.deleted_at IS NULL))THEN PERFORM cbgpu_schools.gpu_notebook_stop(j.id,'START_AUTHORITY_LOST');RETURN jsonb_build_object('command',NULL);END IF;
  tok:=encode(extensions.gen_random_bytes(32),'hex');UPDATE cbgpu_schools.gpu_notebook_commands SET state='LEASED',lease_epoch=lease_epoch+1,lease_token_hash=pg_catalog.sha256(convert_to(tok,'UTF8')),lease_expires_at=clock_timestamp()+interval'45 seconds'WHERE id=c.id RETURNING*INTO c;
  IF c.kind='START_NOTEBOOK'THEN UPDATE cbgpu_schools.gpu_notebook_jobs SET status='STARTING',updated_at=clock_timestamp()WHERE id=j.id;END IF;
 ELSIF p_action IN('renew','result')THEN
  SELECT c0.* INTO c FROM cbgpu_schools.gpu_notebook_commands c0 JOIN cbgpu_schools.gpu_notebook_jobs j0 ON j0.id=c0.job_id WHERE c0.id=(p_payload->>'command_id')::uuid AND j0.node_id=n.id FOR UPDATE OF c0;
  IF NOT FOUND OR c.lease_epoch<>(p_payload->>'lease_epoch')::bigint OR c.lease_token_hash IS DISTINCT FROM pg_catalog.sha256(convert_to(p_payload->>'lease_token','UTF8'))THEN RAISE EXCEPTION'INVALID_LEASE'USING ERRCODE='42501';END IF;
  SELECT * INTO j FROM cbgpu_schools.gpu_notebook_jobs WHERE id=c.job_id FOR UPDATE;
  IF p_action='result'THEN
   result_hash:=pg_catalog.sha256(convert_to((p_payload->'result')::text,'UTF8'));
   IF c.state IN('COMPLETED','PENDING')AND c.receipt_hash IS NOT NULL THEN IF c.receipt_hash=result_hash THEN RETURN jsonb_build_object('command',NULL);ELSE RAISE EXCEPTION'RESULT_CONFLICT'USING ERRCODE='23505';END IF;END IF;
  END IF;
  IF(c.state<>'LEASED'AND NOT(p_action='renew'AND c.state='COMPLETED'AND c.kind='START_NOTEBOOK'AND j.status='RUNNING'))OR c.lease_expires_at<=clock_timestamp()THEN RAISE EXCEPTION'LEASE_EXPIRED'USING ERRCODE='42501';END IF;
  IF p_action='renew'THEN
   IF c.kind='START_NOTEBOOK'AND NOT cbgpu_schools.gpu_notebook_job_allowed(j)THEN PERFORM cbgpu_schools.gpu_notebook_stop(j.id,'REVOKED');RETURN jsonb_build_object('command',NULL);END IF;
   tok:=p_payload->>'lease_token';UPDATE cbgpu_schools.gpu_notebook_commands SET lease_expires_at=clock_timestamp()+interval'45 seconds'WHERE id=c.id RETURNING*INTO c;
  ELSE
   IF p_payload->'result'->>'job_id'<>j.id::text OR p_payload->'result'->>'generation'<>j.id::text THEN RAISE EXCEPTION'INVALID_RESULT_BINDING';END IF;
   st:=p_payload->'result'->>'state';IF st IS NULL OR(c.kind='START_NOTEBOOK'AND st NOT IN('RUNNING','FAILED'))OR(c.kind='STOP_NOTEBOOK'AND st NOT IN('STOPPED','FAILED'))THEN RAISE EXCEPTION'INVALID_RESULT_STATE';END IF;
   IF(p_payload->'result'->>'error_code')IS NOT NULL AND(p_payload->'result'->>'error_code')!~'^[A-Z0-9_]{1,64}$'THEN RAISE EXCEPTION'INVALID_ERROR_CODE';END IF;
   UPDATE cbgpu_schools.gpu_notebook_commands SET state=CASE WHEN c.kind='STOP_NOTEBOOK'AND st='FAILED'THEN'PENDING'ELSE'COMPLETED'END,receipt_hash=result_hash WHERE id=c.id;
   UPDATE cbgpu_schools.gpu_notebook_jobs SET status=CASE WHEN st='RUNNING'AND status='STOPPING'THEN'STOPPING'WHEN st='FAILED'THEN'STOPPING'ELSE st END,error_code=p_payload->'result'->>'error_code',updated_at=clock_timestamp()WHERE id=j.id;
   INSERT INTO cbgpu_schools.gpu_notebook_audit(job_id,event,detail)VALUES(j.id,'NODE_RESULT',jsonb_build_object('kind',c.kind,'state',st,'epoch',c.lease_epoch));
   IF st='FAILED'AND c.kind='START_NOTEBOOK'THEN PERFORM cbgpu_schools.gpu_notebook_stop(j.id,'START_FAILED_STOP_REQUIRED');END IF;
   IF st='RUNNING'AND NOT cbgpu_schools.gpu_notebook_job_allowed(j)THEN PERFORM cbgpu_schools.gpu_notebook_stop(j.id,'POST_START_RECHECK');END IF;
   IF st='STOPPED'THEN DELETE FROM cbgpu_schools.gpu_notebook_sessions WHERE job_id=j.id;END IF;
   RETURN jsonb_build_object('command',NULL);
  END IF;
 ELSE RAISE EXCEPTION'INVALID_ACTION';END IF;
 RETURN jsonb_build_object('command',jsonb_build_object('command_id',c.id,'lease_epoch',c.lease_epoch,'lease_token',tok,'expires_at',extract(epoch FROM c.lease_expires_at)::bigint,'kind',c.kind,'payload',cbgpu_schools.gpu_notebook_spec(j)));
END$$;
REVOKE ALL ON FUNCTION cbgpu_schools.gpu_notebook_spec(cbgpu_schools.gpu_notebook_jobs)FROM PUBLIC,anon,authenticated,service_role;
REVOKE ALL ON FUNCTION public.school_gpu_notebook_device(uuid,bigint,bigint,bigint,text,jsonb)FROM PUBLIC,anon,authenticated;GRANT EXECUTE ON FUNCTION public.school_gpu_notebook_device(uuid,bigint,bigint,bigint,text,jsonb)TO service_role;

CREATE OR REPLACE FUNCTION cbgpu_schools.gpu_resource_state(r cbgpu_schools.gpu_resources)RETURNS text LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$SELECT CASE WHEN n.status='REVOKED'OR n.credential_version<>r.credential_version THEN'REVOKED'WHEN s.status<>'APPROVED'THEN'DISABLED'WHEN n.status<>'ONLINE'OR n.last_seen_at<clock_timestamp()-interval'120 seconds'OR r.last_seen_at<clock_timestamp()-interval'120 seconds'OR r.reported_sequence<>n.last_sequence THEN'OFFLINE'WHEN NOT EXISTS(SELECT FROM cbgpu_schools.gpu_executor_capabilities c CROSS JOIN cbgpu_schools.gpu_notebook_policy p WHERE c.node_id=n.id AND c.credential_version=n.credential_version AND c.protocol=3 AND c.ready AND c.relay_seen_at>clock_timestamp()-interval'60 seconds'AND c.expires_at>clock_timestamp()AND p.enabled AND p.image_digest=c.image_digest AND r.hardware_uuid=ANY(c.hardware_uuids))THEN'NONRUNNABLE'ELSE'READY'END FROM cbgpu_schools.gpu_nodes n JOIN cbgpu_schools.schools s ON s.id=n.school_id WHERE n.id=r.node_id$$;
DO $patch$DECLARE d text;BEGIN d:=pg_get_functiondef('public.school_gpu_resource_api(text,text,jsonb)'::regprocedure);IF position('''execution_ready'''in d)>0 THEN RETURN;END IF;IF position('r.execution_enabled and r.executor_protocol=1'in d)=0 THEN RAISE EXCEPTION'RESOURCE_API_SOURCE_DRIFT';END IF;
d:=replace(d,$q$'execution_enabled',r.execution_enabled and r.executor_protocol=1$q$,$q$'execution_ready',cbgpu_schools.gpu_resource_state(r)='READY','transport_ready',(select enabled from cbgpu_schools.gpu_notebook_policy),'execution_enabled',cbgpu_schools.gpu_resource_state(r)='READY'$q$);
d:=replace(d,$q$'execution_transport_available',false$q$,$q$'execution_transport_available',(select enabled from cbgpu_schools.gpu_notebook_policy)$q$);EXECUTE d;END$patch$;
CREATE OR REPLACE FUNCTION public.school_gpu_notebook_trust()RETURNS jsonb LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path=''AS $$BEGIN IF auth.role()IS DISTINCT FROM'service_role'THEN RAISE EXCEPTION'SERVICE_REQUIRED'USING ERRCODE='42501';END IF;RETURN(SELECT jsonb_build_object('enabled',enabled,'image_digest',image_digest)FROM cbgpu_schools.gpu_notebook_policy);END$$;
REVOKE ALL ON FUNCTION public.school_gpu_notebook_trust()FROM PUBLIC,anon,authenticated;GRANT EXECUTE ON FUNCTION public.school_gpu_notebook_trust()TO service_role;
CREATE OR REPLACE FUNCTION cbgpu_schools.node_gpu_permission(subject uuid,school text)RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path=''AS $$SELECT cbgpu_schools.gpu_notebook_permission(subject,school,clock_timestamp(),clock_timestamp()+interval'1 microsecond')$$;
CREATE OR REPLACE FUNCTION public.school_gpu_notebook_relay(p_node_id uuid,p_version bigint,p_connected boolean)RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=''AS $$BEGIN
 IF auth.role()IS DISTINCT FROM'service_role'THEN RAISE EXCEPTION'SERVICE_REQUIRED'USING ERRCODE='42501';END IF;
 IF p_connected IS NULL OR NOT EXISTS(SELECT FROM cbgpu_schools.gpu_nodes n JOIN cbgpu_schools.schools s ON s.id=n.school_id WHERE n.id=p_node_id AND n.credential_version=p_version AND n.status<>'REVOKED'AND s.status='APPROVED')THEN RAISE EXCEPTION'NODE_UNAVAILABLE'USING ERRCODE='42501';END IF;
 INSERT INTO cbgpu_schools.gpu_executor_capabilities(node_id,credential_version,relay_seen_at)VALUES(p_node_id,p_version,CASE WHEN p_connected THEN clock_timestamp()END)ON CONFLICT(node_id)DO UPDATE SET relay_seen_at=CASE WHEN p_connected THEN clock_timestamp()END WHERE cbgpu_schools.gpu_executor_capabilities.credential_version=p_version;
 RETURN jsonb_build_object('ok',FOUND);END$$;
REVOKE ALL ON FUNCTION public.school_gpu_notebook_relay(uuid,bigint,boolean)FROM PUBLIC,anon,authenticated;GRANT EXECUTE ON FUNCTION public.school_gpu_notebook_relay(uuid,bigint,boolean)TO service_role;
CREATE OR REPLACE FUNCTION public.school_gpu_notebook_owner_session(p_job_id uuid)RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path=''AS $$
DECLARE j cbgpu_schools.gpu_notebook_jobs;BEGIN
 IF NOT public.cbgpu_personal_project_identity()THEN RAISE EXCEPTION'AUTHENTICATION_REQUIRED'USING ERRCODE='42501';END IF;
 SELECT * INTO j FROM cbgpu_schools.gpu_notebook_jobs WHERE id=p_job_id AND owner_id=auth.uid();
 IF NOT FOUND THEN RAISE EXCEPTION'NOTEBOOK_NOT_FOUND'USING ERRCODE='P0002';END IF;
 RETURN public.school_gpu_notebook_api(j.school_id,'session',jsonb_build_object('reservation_id',j.reservation_id));END$$;
REVOKE ALL ON FUNCTION public.school_gpu_notebook_owner_session(uuid)FROM PUBLIC,anon,service_role;GRANT EXECUTE ON FUNCTION public.school_gpu_notebook_owner_session(uuid)TO authenticated;
COMMIT;
