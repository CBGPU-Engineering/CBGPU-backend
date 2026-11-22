-- REVIEW CANDIDATE ONLY. Requires explicit production DB approval; never run by startup.
-- PostgreSQL >=17. No table/function/policy changes, migration history, data changes,
-- existing-account changes or controller activation. Provision LOGIN/credential separately.
BEGIN;
DO $$BEGIN IF current_setting('server_version_num')::int<170000 THEN RAISE EXCEPTION 'PostgreSQL 17 MAINTAIN privilege required';END IF;END$$;
DO $$DECLARE r record;BEGIN
 SELECT * INTO r FROM pg_roles WHERE rolname='cbgpu_gpu_reservations';
 IF NOT FOUND THEN
  CREATE ROLE cbgpu_gpu_reservations NOLOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION BYPASSRLS;
 ELSIF r.rolsuper OR r.rolinherit OR r.rolcreatedb OR r.rolcreaterole OR r.rolreplication OR NOT r.rolbypassrls
  OR EXISTS(SELECT FROM pg_auth_members m WHERE m.member=r.oid) THEN
  RAISE EXCEPTION 'Existing reservation role differs from reviewed privilege boundary';
 END IF;
 -- Existing LOGIN, password and connection settings are deliberately untouched.
END$$;
GRANT USAGE ON SCHEMA public,auth,cbgpu_schools,cbgpu_accounts TO cbgpu_gpu_reservations;
GRANT SELECT(id,user_id,not_after) ON auth.sessions TO cbgpu_gpu_reservations;
GRANT SELECT(id,user_id,gpu_id,status,start_time,end_time) ON public.reservations TO cbgpu_gpu_reservations;
GRANT SELECT(id,resource_id,owner_id,starts_at,ends_at,status) ON cbgpu_schools.gpu_resource_reservations TO cbgpu_gpu_reservations;
-- Existing readiness helper accepts the entire non-secret inventory row.
GRANT SELECT ON cbgpu_schools.gpu_resources TO cbgpu_gpu_reservations;
GRANT SELECT(id,school_id) ON cbgpu_schools.gpu_nodes TO cbgpu_gpu_reservations;
GRANT SELECT(reservation_id) ON cbgpu_schools.gpu_notebook_jobs TO cbgpu_gpu_reservations;
GRANT SELECT(user_id,school_id,membership_type,starts_at,ends_at) ON cbgpu_schools.memberships TO cbgpu_gpu_reservations;
GRANT SELECT(external_user_id,requester_id,school_id,status,resources,starts_at,ends_at) ON cbgpu_accounts.guest_requests TO cbgpu_gpu_reservations;
-- MAINTAIN permits explicit relation locks without granting UPDATE to password/owner/etc.
-- It also permits maintenance commands on these two tables; this is an explicit review tradeoff.
GRANT MAINTAIN ON public.reservations,cbgpu_schools.gpu_resource_reservations TO cbgpu_gpu_reservations;
GRANT UPDATE(end_time) ON public.reservations TO cbgpu_gpu_reservations;
GRANT UPDATE(ends_at) ON cbgpu_schools.gpu_resource_reservations TO cbgpu_gpu_reservations;
GRANT EXECUTE ON FUNCTION public.cbgpu_personal_project_identity(),public.cbgpu_can_use_resource(text,text),cbgpu_schools.node_gpu_permission(uuid,text),cbgpu_schools.gpu_notebook_permission(uuid,text,timestamptz,timestamptz),cbgpu_schools.active_member(uuid,text),cbgpu_schools.gpu_resource_state(cbgpu_schools.gpu_resources) TO cbgpu_gpu_reservations;
COMMIT;
