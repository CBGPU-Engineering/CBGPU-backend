BEGIN;
SET LOCAL lock_timeout='3s';
SET LOCAL statement_timeout='30s';
INSERT INTO cbgpu_schools.feature_registry(key,globally_available,allowed_school_ids,required_permission) VALUES('clubs',true,'{}','ACTIVE_VERIFIED_MEMBERSHIP') ON CONFLICT(key) DO NOTHING;
-- One-time, tracked default for already APPROVED schools; later opt-outs survive reruns.
CREATE TABLE IF NOT EXISTS cbgpu_schools.club_feature_backfill(school_id text PRIMARY KEY REFERENCES cbgpu_schools.schools,before_features text[] NOT NULL,applied_features text[] NOT NULL,applied_revision integer NOT NULL,created_at timestamptz NOT NULL DEFAULT now());
ALTER TABLE cbgpu_schools.club_feature_backfill ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON cbgpu_schools.club_feature_backfill FROM PUBLIC,anon,authenticated,service_role;
DO $$BEGIN
 IF NOT EXISTS(SELECT FROM cbgpu_schools.migration_state WHERE key='clubs-approved-default-v1') THEN
  WITH previous AS MATERIALIZED(SELECT id,features FROM cbgpu_schools.schools WHERE status='APPROVED' AND NOT 'clubs'=ANY(features) FOR UPDATE),
  updated AS(UPDATE cbgpu_schools.schools s SET features=array_append(s.features,'clubs'),revision=revision+1 FROM previous p WHERE s.id=p.id RETURNING s.id,s.features,s.revision)
  INSERT INTO cbgpu_schools.club_feature_backfill(school_id,before_features,applied_features,applied_revision) SELECT u.id,p.features,u.features,u.revision FROM updated u JOIN previous p ON p.id=u.id;
  INSERT INTO cbgpu_schools.migration_state(key) VALUES('clubs-approved-default-v1');
 END IF;
END$$;
DO $$DECLARE definition text;updated text;BEGIN
 definition:=pg_get_functiondef('cbgpu_schools.default_draft()'::regprocedure);
 IF position('"clubs"' IN definition)=0 THEN
  updated:=replace(definition,'"features":["community","projects","contests"]','"features":["community","projects","contests","clubs"]');
  IF updated=definition THEN RAISE EXCEPTION 'CLUBS_DEFAULT_DRAFT_VERSION_MISMATCH';END IF;EXECUTE updated;
 END IF;
END$$;
CREATE TABLE IF NOT EXISTS cbgpu_schools.club_campaigns(
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),school_id text NOT NULL REFERENCES cbgpu_schools.schools,
 title text NOT NULL CHECK(length(btrim(title)) BETWEEN 1 AND 150),
 registration_starts_at timestamptz NOT NULL,registration_ends_at timestamptz NOT NULL,
 promotion_starts_at timestamptz NOT NULL,promotion_ends_at timestamptz NOT NULL,
 application_starts_at timestamptz NOT NULL,application_ends_at timestamptz NOT NULL,
 status text NOT NULL DEFAULT 'ACTIVE' CHECK(status IN('ACTIVE','PAUSED')),revision integer NOT NULL DEFAULT 1,
 created_by uuid NOT NULL REFERENCES auth.users,created_at timestamptz NOT NULL DEFAULT now(),updated_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(id,school_id),CHECK(isfinite(registration_starts_at) AND isfinite(registration_ends_at) AND isfinite(promotion_starts_at) AND isfinite(promotion_ends_at) AND isfinite(application_starts_at) AND isfinite(application_ends_at)),CHECK(registration_starts_at<registration_ends_at AND promotion_starts_at<promotion_ends_at AND application_starts_at<application_ends_at AND registration_ends_at<=promotion_ends_at AND application_starts_at>=promotion_starts_at AND application_ends_at<=promotion_ends_at));
CREATE TABLE IF NOT EXISTS cbgpu_schools.clubs(
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),school_id text NOT NULL, campaign_id uuid NOT NULL,owner_id uuid NOT NULL REFERENCES auth.users,
 title text NOT NULL CHECK(length(btrim(title)) BETWEEN 1 AND 150),description text NOT NULL CHECK(length(description)<=10000),application_prompt text NOT NULL DEFAULT '' CHECK(length(application_prompt)<=2000),
 capacity integer CHECK(capacity BETWEEN 1 AND 10000),status text NOT NULL DEFAULT 'DRAFT' CHECK(status IN('DRAFT','READY','WITHDRAWN')),
 revision integer NOT NULL DEFAULT 1,created_at timestamptz NOT NULL DEFAULT now(),updated_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY(campaign_id,school_id) REFERENCES cbgpu_schools.club_campaigns(id,school_id),UNIQUE(id,campaign_id,school_id));
CREATE TABLE IF NOT EXISTS cbgpu_schools.club_applications(
 id uuid PRIMARY KEY DEFAULT gen_random_uuid(),school_id text NOT NULL,club_id uuid NOT NULL,campaign_id uuid NOT NULL,applicant_id uuid NOT NULL REFERENCES auth.users,
 answers text NOT NULL DEFAULT '' CHECK(length(answers)<=10000),status text NOT NULL DEFAULT 'PENDING' CHECK(status IN('PENDING','APPROVED','REJECTED','WITHDRAWN')),
 revision integer NOT NULL DEFAULT 1,created_at timestamptz NOT NULL DEFAULT now(),updated_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY(club_id,campaign_id,school_id) REFERENCES cbgpu_schools.clubs(id,campaign_id,school_id),UNIQUE(applicant_id,club_id,campaign_id));
CREATE INDEX IF NOT EXISTS club_campaigns_school_time ON cbgpu_schools.club_campaigns(school_id,created_at DESC,id);
CREATE INDEX IF NOT EXISTS clubs_school_campaign ON cbgpu_schools.clubs(school_id,campaign_id,created_at DESC,id);
CREATE INDEX IF NOT EXISTS club_applications_club_status ON cbgpu_schools.club_applications(club_id,status,created_at,id);
CREATE INDEX IF NOT EXISTS club_applications_school_campaign ON cbgpu_schools.club_applications(school_id,campaign_id);
ALTER TABLE cbgpu_schools.club_campaigns ENABLE ROW LEVEL SECURITY;
ALTER TABLE cbgpu_schools.clubs ENABLE ROW LEVEL SECURITY;
ALTER TABLE cbgpu_schools.club_applications ENABLE ROW LEVEL SECURITY;
REVOKE ALL ON cbgpu_schools.club_campaigns,cbgpu_schools.clubs,cbgpu_schools.club_applications FROM PUBLIC,anon,authenticated,service_role;
CREATE OR REPLACE FUNCTION cbgpu_schools.club_student(uid uuid,sid text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT cbgpu_schools.active_member(uid,sid) AND EXISTS(SELECT FROM cbgpu_schools.memberships m WHERE m.user_id=uid AND m.school_id=sid AND m.membership_type='STUDENT')
$$;
CREATE OR REPLACE FUNCTION cbgpu_schools.club_admin(uid uuid,sid text) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT cbgpu_schools.has_role(uid,sid,ARRAY['SCHOOL_OWNER','SCHOOL_ADMIN'])
$$;
CREATE OR REPLACE FUNCTION cbgpu_schools.club_manager(c cbgpu_schools.clubs,uid uuid) RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT cbgpu_schools.club_admin(uid,c.school_id) OR(c.owner_id=uid AND cbgpu_schools.club_student(uid,c.school_id))
$$;
CREATE OR REPLACE FUNCTION cbgpu_schools.club_campaign_json(c cbgpu_schools.club_campaigns,at_time timestamptz) RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT to_jsonb(c)-'created_by'||jsonb_build_object(
 'registration_open',c.status='ACTIVE' AND at_time>=c.registration_starts_at AND at_time<c.registration_ends_at,
 'promotion_open',c.status='ACTIVE' AND at_time>=c.promotion_starts_at AND at_time<c.promotion_ends_at,
 'application_open',c.status='ACTIVE' AND at_time>=c.application_starts_at AND at_time<c.application_ends_at,
 'next_at',CASE WHEN c.status='ACTIVE' THEN(SELECT min(t) FROM unnest(ARRAY[c.registration_starts_at,c.registration_ends_at,c.promotion_starts_at,c.promotion_ends_at,c.application_starts_at,c.application_ends_at])t WHERE t>at_time) END)
$$;
CREATE OR REPLACE FUNCTION cbgpu_schools.club_json(c cbgpu_schools.clubs,uid uuid,at_time timestamptz) RETURNS jsonb LANGUAGE sql STABLE SECURITY DEFINER SET search_path='' AS $$
 SELECT to_jsonb(c)||jsonb_build_object('published',published,'can_manage',manager,
 'can_edit',manager AND(c.status='DRAFT' OR c.status='READY' AND registration_open),
 'can_submit_ready',manager AND c.status='DRAFT' AND registration_open AND cbgpu_schools.club_student(c.owner_id,c.school_id),
 'can_withdraw',manager AND c.status<>'WITHDRAWN',
 'can_apply',published AND application_open AND cbgpu_schools.club_student(uid,c.school_id) AND NOT EXISTS(SELECT FROM cbgpu_schools.club_applications a WHERE a.club_id=c.id AND a.applicant_id=uid),
 'my_application',(SELECT to_jsonb(a)-'applicant_id'-'school_id' FROM cbgpu_schools.club_applications a WHERE a.club_id=c.id AND a.applicant_id=uid))
 FROM(SELECT cbgpu_schools.club_manager(c,uid) manager,
 p.status='ACTIVE' AND at_time>=p.registration_starts_at AND at_time<p.registration_ends_at registration_open,
 p.status='ACTIVE' AND at_time>=p.application_starts_at AND at_time<p.application_ends_at application_open,
 c.status='READY' AND p.status='ACTIVE' AND at_time>=p.promotion_starts_at AND at_time<p.promotion_ends_at published
 FROM cbgpu_schools.club_campaigns p WHERE p.id=c.campaign_id AND p.school_id=c.school_id) flags
$$;
-- Expose the feature in existing school settings/draft validation, without enabling it on any school.
DO $$DECLARE definition text;updated text;BEGIN
 definition:=pg_get_functiondef('cbgpu_schools.validate_draft(jsonb)'::regprocedure);
 IF position($marker$'clubs'$marker$ IN definition)=0 THEN
 updated:=replace(definition,$old$'community','school_life','projects','contests',$old$,$new$'community','school_life','projects','contests','clubs',$new$);
 IF updated=definition THEN RAISE EXCEPTION 'CLUBS_DRAFT_HANDLER_VERSION_MISMATCH';END IF;EXECUTE updated;END IF;
END$$;
CREATE OR REPLACE FUNCTION public.cbgpu_school_clubs_api(action text,payload jsonb DEFAULT '{}') RETURNS jsonb LANGUAGE plpgsql SECURITY DEFINER SET search_path='' SET lock_timeout='2s' AS $$
DECLARE uid uuid:=auth.uid();sid text:=payload->>'school_id';admin boolean;student boolean;at_time timestamptz;
 p cbgpu_schools.club_campaigns;c cbgpu_schools.clubs;a cbgpu_schools.club_applications;candidate jsonb;allowed text[];
 lim integer:=least(100,greatest(1,coalesce((payload->>'limit')::integer,50)));off integer:=greatest(0,coalesce((payload->>'offset')::integer,0));items jsonb;total bigint;result jsonb;manager boolean;
BEGIN
 IF NOT coalesce(public.cbgpu_personal_project_identity(),false) THEN RAISE EXCEPTION 'AUTHENTICATION_REQUIRED' USING ERRCODE='42501';END IF;
 IF jsonb_typeof(payload) IS DISTINCT FROM 'object' OR octet_length(payload::text)>65536 THEN RAISE EXCEPTION 'INVALID_CLUB_REQUEST' USING ERRCODE='22023';END IF;
 allowed:=CASE action
 WHEN 'list' THEN ARRAY['school_id','campaign_id','limit','offset']
 WHEN 'campaign_create' THEN ARRAY['school_id','title','registration_starts_at','registration_ends_at','promotion_starts_at','promotion_ends_at','application_starts_at','application_ends_at','confirmed']
 WHEN 'campaign_update' THEN ARRAY['school_id','campaign_id','expected_revision','title','registration_starts_at','registration_ends_at','promotion_starts_at','promotion_ends_at','application_starts_at','application_ends_at','status','confirmed']
 WHEN 'create' THEN ARRAY['school_id','campaign_id','title','description','application_prompt','capacity']
 WHEN 'update' THEN ARRAY['school_id','club_id','expected_revision','title','description','application_prompt','capacity']
 WHEN 'ready' THEN ARRAY['school_id','club_id','expected_revision']
 WHEN 'withdraw' THEN ARRAY['school_id','club_id','expected_revision']
 WHEN 'apply' THEN ARRAY['school_id','club_id','answers']
 WHEN 'applications' THEN ARRAY['school_id','club_id','limit','offset']
 WHEN 'application_review' THEN ARRAY['school_id','club_id','application_id','expected_revision','status']
 WHEN 'application_withdraw' THEN ARRAY['school_id','club_id','application_id','expected_revision'] END;
 IF allowed IS NULL OR EXISTS(SELECT FROM jsonb_object_keys(payload)k WHERE NOT k=ANY(allowed)) THEN RAISE EXCEPTION 'INVALID_CLUB_REQUEST' USING ERRCODE='22023';END IF;
 -- Serialize permission changes against writes; no caller-supplied role or identity is trusted.
 PERFORM 1 FROM cbgpu_schools.memberships WHERE user_id=uid AND school_id=sid FOR SHARE;
 PERFORM 1 FROM cbgpu_schools.schools WHERE id=sid AND status='APPROVED' FOR SHARE;
 IF NOT FOUND OR NOT cbgpu_schools.active_member(uid,sid) THEN RAISE EXCEPTION 'ACTIVE_SCHOOL_MEMBERSHIP_REQUIRED' USING ERRCODE='42501';END IF;
 IF NOT cbgpu_schools.feature_enabled(sid,'clubs') THEN RAISE EXCEPTION 'CLUBS_DISABLED' USING ERRCODE='42501';END IF;
 admin:=cbgpu_schools.club_admin(uid,sid);student:=cbgpu_schools.club_student(uid,sid);at_time:=clock_timestamp();
 IF action='list' THEN
  IF payload->>'campaign_id' IS NOT NULL AND NOT EXISTS(SELECT FROM cbgpu_schools.club_campaigns WHERE id=(payload->>'campaign_id')::uuid AND school_id=sid) THEN RAISE EXCEPTION 'CLUB_CAMPAIGN_NOT_FOUND' USING ERRCODE='P0002';END IF;
  WITH visible AS MATERIALIZED(SELECT x.* FROM cbgpu_schools.clubs x JOIN cbgpu_schools.club_campaigns y ON y.id=x.campaign_id AND y.school_id=x.school_id
   WHERE x.school_id=sid AND(payload->>'campaign_id' IS NULL OR x.campaign_id=(payload->>'campaign_id')::uuid)
   AND(cbgpu_schools.club_manager(x,uid) OR EXISTS(SELECT FROM cbgpu_schools.club_applications own WHERE own.club_id=x.id AND own.school_id=sid AND own.applicant_id=uid) OR x.status='READY' AND(
    y.status='ACTIVE' AND at_time>=y.promotion_starts_at AND at_time<y.promotion_ends_at
    OR payload->>'campaign_id' IS NOT NULL AND at_time>=y.promotion_ends_at))),
  page AS(SELECT * FROM visible ORDER BY created_at DESC,id LIMIT lim OFFSET off)
  SELECT(SELECT coalesce(jsonb_agg(cbgpu_schools.club_json(page,uid,at_time) ORDER BY created_at DESC,id),'[]') FROM page),(SELECT count(*) FROM visible) INTO items,total;
  RETURN jsonb_build_object('school_id',sid,'server_now',at_time,'can_manage_campaigns',admin,'campaigns',(SELECT coalesce(jsonb_agg(cbgpu_schools.club_campaign_json(x,at_time)||CASE WHEN admin THEN jsonb_build_object('club_count',(SELECT count(*) FROM cbgpu_schools.clubs z WHERE z.school_id=sid AND z.campaign_id=x.id),'application_count',(SELECT count(*) FROM cbgpu_schools.club_applications z WHERE z.school_id=sid AND z.campaign_id=x.id)) ELSE '{}'::jsonb END ORDER BY created_at DESC,id),'[]') FROM cbgpu_schools.club_campaigns x WHERE school_id=sid),'items',items,'total',total);
 ELSIF action IN('campaign_create','campaign_update') THEN
  IF NOT admin THEN RAISE EXCEPTION 'SCHOOL_ADMIN_REQUIRED' USING ERRCODE='42501';END IF;
  IF payload->'confirmed' IS DISTINCT FROM 'true'::jsonb THEN RAISE EXCEPTION 'EXPLICIT_CONFIRMATION_REQUIRED' USING ERRCODE='22023';END IF;
  IF action='campaign_update' THEN
   SELECT * INTO p FROM cbgpu_schools.club_campaigns WHERE school_id=sid AND id=(payload->>'campaign_id')::uuid FOR UPDATE;
   IF NOT FOUND THEN RAISE EXCEPTION 'CLUB_CAMPAIGN_NOT_FOUND' USING ERRCODE='P0002';END IF;
   IF p.revision IS DISTINCT FROM (payload->>'expected_revision')::integer THEN RAISE EXCEPTION 'REVISION_CONFLICT' USING ERRCODE='40001';END IF;
   candidate:=to_jsonb(p)||payload;
  ELSE candidate:=jsonb_build_object('status','ACTIVE')||payload;END IF;
  IF coalesce(length(btrim(candidate->>'title')),0) NOT BETWEEN 1 AND 150
   OR candidate->>'status' IS NULL OR candidate->>'status' NOT IN('ACTIVE','PAUSED')
   OR coalesce((candidate->>'registration_starts_at')::timestamptz >= (candidate->>'registration_ends_at')::timestamptz
    OR(candidate->>'promotion_starts_at')::timestamptz >=(candidate->>'promotion_ends_at')::timestamptz
    OR(candidate->>'application_starts_at')::timestamptz >=(candidate->>'application_ends_at')::timestamptz
    OR(candidate->>'registration_ends_at')::timestamptz >(candidate->>'promotion_ends_at')::timestamptz
    OR(candidate->>'application_starts_at')::timestamptz <(candidate->>'promotion_starts_at')::timestamptz
    OR(candidate->>'application_ends_at')::timestamptz >(candidate->>'promotion_ends_at')::timestamptz,true)
  THEN RAISE EXCEPTION 'INVALID_CLUB_CAMPAIGN' USING ERRCODE='22023';END IF;
  IF action='campaign_create' THEN
   INSERT INTO cbgpu_schools.club_campaigns(school_id,title,registration_starts_at,registration_ends_at,promotion_starts_at,promotion_ends_at,application_starts_at,application_ends_at,created_by)
   VALUES(sid,btrim(candidate->>'title'),(candidate->>'registration_starts_at')::timestamptz,(candidate->>'registration_ends_at')::timestamptz,(candidate->>'promotion_starts_at')::timestamptz,(candidate->>'promotion_ends_at')::timestamptz,(candidate->>'application_starts_at')::timestamptz,(candidate->>'application_ends_at')::timestamptz,uid) RETURNING * INTO p;
  ELSE
   UPDATE cbgpu_schools.club_campaigns SET title=btrim(candidate->>'title'),registration_starts_at=(candidate->>'registration_starts_at')::timestamptz,registration_ends_at=(candidate->>'registration_ends_at')::timestamptz,promotion_starts_at=(candidate->>'promotion_starts_at')::timestamptz,promotion_ends_at=(candidate->>'promotion_ends_at')::timestamptz,application_starts_at=(candidate->>'application_starts_at')::timestamptz,application_ends_at=(candidate->>'application_ends_at')::timestamptz,status=candidate->>'status',revision=revision+1,updated_at=clock_timestamp() WHERE id=p.id RETURNING * INTO p;
  END IF;
  INSERT INTO cbgpu_schools.audit(actor_id,school_id,action,after_data) VALUES(uid,sid,'club_'||action,jsonb_build_object('campaign_id',p.id,'revision',p.revision,'status',p.status));
  RETURN jsonb_build_object('school_id',sid,'server_now',clock_timestamp(),'campaign',cbgpu_schools.club_campaign_json(p,clock_timestamp()));
 ELSIF action='create' THEN
  IF NOT student THEN RAISE EXCEPTION 'VERIFIED_STUDENT_REQUIRED' USING ERRCODE='42501';END IF;
  SELECT * INTO p FROM cbgpu_schools.club_campaigns WHERE school_id=sid AND id=(payload->>'campaign_id')::uuid FOR SHARE;
  IF NOT FOUND THEN RAISE EXCEPTION 'CLUB_CAMPAIGN_NOT_FOUND' USING ERRCODE='P0002';END IF;
  IF coalesce(length(btrim(payload->>'title')),0) NOT BETWEEN 1 AND 150 OR coalesce(length(payload->>'description'),0)>10000 OR coalesce(length(payload->>'application_prompt'),0)>2000 OR (payload->>'capacity')::integer NOT BETWEEN 1 AND 10000 THEN RAISE EXCEPTION 'INVALID_CLUB_REQUEST' USING ERRCODE='22023';END IF;
  INSERT INTO cbgpu_schools.clubs(school_id,campaign_id,owner_id,title,description,application_prompt,capacity) VALUES(sid,p.id,uid,btrim(payload->>'title'),coalesce(payload->>'description',''),coalesce(payload->>'application_prompt',''),(payload->>'capacity')::integer) RETURNING * INTO c;
 ELSE
  SELECT * INTO c FROM cbgpu_schools.clubs WHERE school_id=sid AND id=(payload->>'club_id')::uuid FOR UPDATE;
  IF NOT FOUND THEN RAISE EXCEPTION 'CLUB_NOT_FOUND' USING ERRCODE='P0002';END IF;
  SELECT * INTO p FROM cbgpu_schools.club_campaigns WHERE school_id=sid AND id=c.campaign_id FOR SHARE;
  at_time:=clock_timestamp();manager:=cbgpu_schools.club_manager(c,uid);
  IF action IN('update','ready','withdraw') THEN
   IF NOT manager THEN RAISE EXCEPTION 'CLUB_MANAGER_REQUIRED' USING ERRCODE='42501';END IF;
   IF c.revision IS DISTINCT FROM(payload->>'expected_revision')::integer THEN RAISE EXCEPTION 'REVISION_CONFLICT' USING ERRCODE='40001';END IF;
   IF c.status='WITHDRAWN' THEN RAISE EXCEPTION 'CLUB_WITHDRAWN' USING ERRCODE='40001';END IF;
   IF action='ready' OR(action='update' AND c.status='READY') THEN
    IF p.status<>'ACTIVE' OR at_time<p.registration_starts_at OR at_time>=p.registration_ends_at THEN RAISE EXCEPTION 'REGISTRATION_WINDOW_CLOSED' USING ERRCODE='40001';END IF;
    IF NOT cbgpu_schools.club_student(c.owner_id,sid) THEN RAISE EXCEPTION 'VERIFIED_STUDENT_REQUIRED' USING ERRCODE='42501';END IF;
   END IF;
   IF action='update' THEN
    candidate:=to_jsonb(c)||payload;
    IF coalesce(length(btrim(candidate->>'title')),0) NOT BETWEEN 1 AND 150 OR length(candidate->>'description')>10000 OR length(candidate->>'application_prompt')>2000 OR(candidate->>'capacity')::integer NOT BETWEEN 1 AND 10000 THEN RAISE EXCEPTION 'INVALID_CLUB_REQUEST' USING ERRCODE='22023';END IF;
    IF(candidate->>'capacity')::integer<(SELECT count(*) FROM cbgpu_schools.club_applications WHERE club_id=c.id AND status='APPROVED') THEN RAISE EXCEPTION 'CLUB_CAPACITY_FULL' USING ERRCODE='40001';END IF;
    UPDATE cbgpu_schools.clubs SET title=btrim(candidate->>'title'),description=coalesce(candidate->>'description',''),application_prompt=coalesce(candidate->>'application_prompt',''),capacity=(candidate->>'capacity')::integer,revision=revision+1,updated_at=clock_timestamp() WHERE id=c.id RETURNING * INTO c;
   ELSE UPDATE cbgpu_schools.clubs SET status=CASE WHEN action='ready' THEN 'READY' ELSE 'WITHDRAWN' END,revision=revision+1,updated_at=clock_timestamp() WHERE id=c.id RETURNING * INTO c;END IF;
  ELSIF action='apply' THEN
   IF NOT student THEN RAISE EXCEPTION 'VERIFIED_STUDENT_REQUIRED' USING ERRCODE='42501';END IF;
   IF p.status<>'ACTIVE' OR at_time<p.application_starts_at OR at_time>=p.application_ends_at THEN RAISE EXCEPTION 'APPLICATION_WINDOW_CLOSED' USING ERRCODE='40001';END IF;
   IF c.status<>'READY' OR at_time<p.promotion_starts_at OR at_time>=p.promotion_ends_at THEN RAISE EXCEPTION 'CLUB_NOT_PUBLISHED' USING ERRCODE='40001';END IF;
   IF coalesce(length(payload->>'answers'),0)>10000 THEN RAISE EXCEPTION 'INVALID_CLUB_REQUEST' USING ERRCODE='22023';END IF;
   IF EXISTS(SELECT FROM cbgpu_schools.club_applications WHERE club_id=c.id AND applicant_id=uid) THEN RAISE EXCEPTION 'APPLICATION_ALREADY_EXISTS' USING ERRCODE='23505';END IF;
   INSERT INTO cbgpu_schools.club_applications(school_id,club_id,campaign_id,applicant_id,answers) VALUES(sid,c.id,p.id,uid,coalesce(payload->>'answers','')) RETURNING * INTO a;
   RETURN jsonb_build_object('school_id',sid,'server_now',clock_timestamp(),'application',to_jsonb(a)-'applicant_id'-'school_id');
  ELSIF action='applications' THEN
   IF NOT manager THEN RAISE EXCEPTION 'CLUB_MANAGER_REQUIRED' USING ERRCODE='42501';END IF;
   SELECT count(*) INTO total FROM cbgpu_schools.club_applications WHERE club_id=c.id;
   SELECT coalesce(jsonb_agg(to_jsonb(x)-'applicant_id'||jsonb_build_object('applicant',jsonb_build_object('user_id',x.applicant_id,'full_name',left(coalesce(pr.full_name,''),100),'student_number',m.student_number)) ORDER BY x.created_at,x.id),'[]') INTO items FROM(SELECT * FROM cbgpu_schools.club_applications WHERE club_id=c.id ORDER BY created_at,id LIMIT lim OFFSET off)x LEFT JOIN public.profiles pr ON pr.id=x.applicant_id LEFT JOIN cbgpu_schools.memberships m ON m.school_id=sid AND m.user_id=x.applicant_id;
   RETURN jsonb_build_object('school_id',sid,'server_now',clock_timestamp(),'items',items,'total',total);
  ELSIF action IN('application_review','application_withdraw') THEN
   SELECT * INTO a FROM cbgpu_schools.club_applications WHERE school_id=sid AND club_id=c.id AND id=(payload->>'application_id')::uuid FOR UPDATE;
   IF NOT FOUND THEN RAISE EXCEPTION 'CLUB_APPLICATION_NOT_FOUND' USING ERRCODE='P0002';END IF;
   IF a.revision IS DISTINCT FROM(payload->>'expected_revision')::integer THEN RAISE EXCEPTION 'REVISION_CONFLICT' USING ERRCODE='40001';END IF;
   IF action='application_withdraw' THEN
    IF a.applicant_id<>uid THEN RAISE EXCEPTION 'APPLICATION_OWNER_REQUIRED' USING ERRCODE='42501';END IF;
    UPDATE cbgpu_schools.club_applications SET status='WITHDRAWN',revision=revision+1,updated_at=clock_timestamp() WHERE id=a.id RETURNING * INTO a;
   ELSE
    IF NOT manager THEN RAISE EXCEPTION 'CLUB_MANAGER_REQUIRED' USING ERRCODE='42501';END IF;
    IF a.status='WITHDRAWN' THEN RAISE EXCEPTION 'APPLICATION_WITHDRAWN' USING ERRCODE='40001';END IF;
    IF payload->>'status' IS NULL OR payload->>'status' NOT IN('APPROVED','REJECTED') THEN RAISE EXCEPTION 'INVALID_CLUB_REQUEST' USING ERRCODE='22023';END IF;
    IF payload->>'status'='APPROVED' THEN
     IF NOT cbgpu_schools.club_student(a.applicant_id,sid) THEN RAISE EXCEPTION 'VERIFIED_STUDENT_REQUIRED' USING ERRCODE='42501';END IF;
     IF a.status<>'APPROVED' AND c.capacity IS NOT NULL AND(SELECT count(*) FROM cbgpu_schools.club_applications WHERE club_id=c.id AND status='APPROVED')>=c.capacity THEN RAISE EXCEPTION 'CLUB_CAPACITY_FULL' USING ERRCODE='40001';END IF;
    END IF;
    UPDATE cbgpu_schools.club_applications SET status=payload->>'status',revision=revision+1,updated_at=clock_timestamp() WHERE id=a.id RETURNING * INTO a;
   END IF;
   INSERT INTO cbgpu_schools.audit(actor_id,school_id,action,after_data) VALUES(uid,sid,'club_'||action,jsonb_build_object('application_id',a.id,'club_id',c.id,'status',a.status,'revision',a.revision));
   RETURN jsonb_build_object('school_id',sid,'server_now',clock_timestamp(),'application',to_jsonb(a)-'applicant_id'-'school_id');
  END IF;
 END IF;
 INSERT INTO cbgpu_schools.audit(actor_id,school_id,action,after_data) VALUES(uid,sid,'club_'||action,jsonb_build_object('club_id',c.id,'campaign_id',c.campaign_id,'revision',c.revision,'status',c.status));
 RETURN jsonb_build_object('school_id',sid,'server_now',clock_timestamp(),'club',cbgpu_schools.club_json(c,uid,clock_timestamp()));
END$$;
REVOKE ALL ON FUNCTION cbgpu_schools.club_student(uuid,text),cbgpu_schools.club_admin(uuid,text),cbgpu_schools.club_manager(cbgpu_schools.clubs,uuid),cbgpu_schools.club_campaign_json(cbgpu_schools.club_campaigns,timestamptz),cbgpu_schools.club_json(cbgpu_schools.clubs,uuid,timestamptz) FROM PUBLIC,anon,authenticated,service_role;
REVOKE ALL ON FUNCTION public.cbgpu_school_clubs_api(text,jsonb) FROM PUBLIC,anon,service_role;
GRANT EXECUTE ON FUNCTION public.cbgpu_school_clubs_api(text,jsonb) TO authenticated;
NOTIFY pgrst,'reload schema';
COMMIT;
