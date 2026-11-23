CREATE FUNCTION pg_temp.assert_true(value boolean,label text) RETURNS void LANGUAGE plpgsql AS $$BEGIN IF value IS DISTINCT FROM true THEN RAISE EXCEPTION '%',label;END IF;END$$;
CREATE FUNCTION pg_temp.as_user(n integer,action text,payload jsonb) RETURNS jsonb LANGUAGE plpgsql AS $$BEGIN PERFORM set_config('request.jwt.claims',jsonb_build_object('sub','00000000-0000-4000-8000-'||lpad(n::text,12,'0'),'session_id','10000000-0000-4000-8000-'||lpad(n::text,12,'0'))::text,true);RETURN public.cbgpu_school_clubs_api(action,payload);END$$;
INSERT INTO auth.users(id,raw_user_meta_data) SELECT ('00000000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid,'{"account_type_request":"EXTERNAL"}' FROM generate_series(6,8)n;
INSERT INTO auth.sessions(id,user_id) SELECT ('10000000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid,('00000000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid FROM generate_series(6,8)n;
INSERT INTO public.profiles(id,full_name,account_type) SELECT ('00000000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid,'Private synthetic applicant '||n,'EXTERNAL' FROM generate_series(6,8)n;
INSERT INTO cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,region,configuration,features,status) SELECT 'clubs-school-'||n,'School '||n,'school '||n,'School '||n,'MIDDLE_SCHOOL','충북','{"student_id_policy":"SCHOOL_DEFINED"}',ARRAY['clubs'],'APPROVED' FROM generate_series(1,2)n;
INSERT INTO cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,school_roles,student_number)
SELECT ('00000000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid,CASE WHEN n=8 THEN 'clubs-school-2' ELSE 'clubs-school-1' END,CASE WHEN n=4 THEN 'TEACHER' ELSE 'STUDENT' END,'ACTIVE',true,CASE WHEN n=1 THEN ARRAY['SCHOOL_OWNER','SCHOOL_ADMIN'] ELSE '{}'::text[] END,'1-1-'||n FROM unnest(ARRAY[1,4,5,6,7,8])n;
UPDATE public.profiles SET account_type='EXTERNAL' WHERE id IN('00000000-0000-4000-8000-000000000005','00000000-0000-4000-8000-000000000004');
CREATE TABLE fixture_clubs_state(key text PRIMARY KEY,value jsonb);
DO $$DECLARE body jsonb;campaign jsonb;club jsonb;app jsonb;result jsonb;BEGIN
body:=jsonb_build_object('school_id','clubs-school-1','title','Arbitrary autumn round','registration_starts_at',now()-interval '1 day','registration_ends_at',now()+interval '1 day','promotion_starts_at',now()-interval '1 hour','promotion_ends_at',now()+interval '2 days','application_starts_at',now()-interval '1 minute','application_ends_at',now()+interval '1 day','confirmed',true);
BEGIN PERFORM pg_temp.as_user(4,'campaign_create',body);RAISE EXCEPTION 'TEACHER_IMPLICIT_ADMIN';EXCEPTION WHEN insufficient_privilege THEN NULL;END;
BEGIN PERFORM pg_temp.as_user(1,'campaign_create',body||'{"school_id":"clubs-school-2"}');RAISE EXCEPTION 'OTHER_SCHOOL_ADMIN';EXCEPTION WHEN insufficient_privilege THEN NULL;END;
BEGIN PERFORM pg_temp.as_user(1,'campaign_create',body||jsonb_build_object('application_ends_at',now()+interval '3 days'));RAISE EXCEPTION 'UNREACHABLE_APPLICATION_WINDOW';EXCEPTION WHEN invalid_parameter_value THEN NULL;END;
campaign:=pg_temp.as_user(1,'campaign_create',body)->'campaign';
PERFORM pg_temp.assert_true((campaign->>'registration_open')::boolean AND(campaign->>'promotion_open')::boolean AND(campaign->>'application_open')::boolean,'three independent overlapping windows open');
-- Multiple rounds in the same year are allowed; no school-year key or destructive rollover.
PERFORM pg_temp.as_user(1,'campaign_create',body||'{"title":"Second round same year"}');
club:=pg_temp.as_user(5,'create',jsonb_build_object('school_id','clubs-school-1','campaign_id',campaign->>'id','title','Synthetic astronomy','description','Student-founded school club','application_prompt','Why join?','capacity',1))->'club';
PERFORM pg_temp.assert_true(club->>'status'='DRAFT' AND NOT(club->>'published')::boolean AND(club->>'can_submit_ready')::boolean,'private draft ready permission');
result:=pg_temp.as_user(6,'list','{"school_id":"clubs-school-1"}');
PERFORM pg_temp.assert_true(result->>'total'='0' AND position('Student-founded' IN result::text)=0,'non-owner cannot see drafts');
club:=pg_temp.as_user(5,'ready',jsonb_build_object('school_id','clubs-school-1','club_id',club->>'id','expected_revision',club->'revision'))->'club';
PERFORM pg_temp.assert_true((club->>'published')::boolean,'ready automatically published by server clock');
result:=pg_temp.as_user(6,'list','{"school_id":"clubs-school-1"}');PERFORM pg_temp.assert_true(result->>'total'='1','verified same-school publication');
BEGIN PERFORM pg_temp.as_user(8,'list','{"school_id":"clubs-school-1"}');RAISE EXCEPTION 'OTHER_SCHOOL_READ';EXCEPTION WHEN insufficient_privilege THEN NULL;END;
BEGIN PERFORM pg_temp.as_user(6,'applications',jsonb_build_object('school_id','clubs-school-1','club_id',club->>'id'));RAISE EXCEPTION 'APPLICANT_LIST_READ';EXCEPTION WHEN insufficient_privilege THEN NULL;END;
app:=pg_temp.as_user(6,'apply',jsonb_build_object('school_id','clubs-school-1','club_id',club->>'id','answers','Private synthetic motivation'))->'application';
BEGIN PERFORM pg_temp.as_user(6,'apply',jsonb_build_object('school_id','clubs-school-1','club_id',club->>'id','answers','duplicate'));RAISE EXCEPTION 'DUPLICATE_ALLOWED';EXCEPTION WHEN unique_violation THEN NULL;END;
PERFORM pg_temp.as_user(7,'apply',jsonb_build_object('school_id','clubs-school-1','club_id',club->>'id','answers','Second private applicant'));
result:=pg_temp.as_user(7,'list','{"school_id":"clubs-school-1"}');PERFORM pg_temp.assert_true(position('Private synthetic motivation' IN result::text)=0 AND position('Private synthetic applicant 6' IN result::text)=0,'no other applicant answers or identity in feed');
result:=pg_temp.as_user(5,'applications',jsonb_build_object('school_id','clubs-school-1','club_id',club->>'id'));PERFORM pg_temp.assert_true(result->>'total'='2' AND position('Private synthetic motivation' IN result::text)>0,'founder can review own club applications');
BEGIN PERFORM pg_temp.as_user(4,'applications',jsonb_build_object('school_id','clubs-school-1','club_id',club->>'id'));RAISE EXCEPTION 'TEACHER_REVIEW';EXCEPTION WHEN insufficient_privilege THEN NULL;END;
BEGIN PERFORM pg_temp.as_user(6,'update',jsonb_build_object('school_id','clubs-school-1','club_id',club->>'id','expected_revision',club->'revision','title','Hijack'));RAISE EXCEPTION 'FOREIGN_OWNER_EDIT';EXCEPTION WHEN insufficient_privilege THEN NULL;END;
BEGIN PERFORM pg_temp.as_user(5,'update',jsonb_build_object('school_id','clubs-school-1','club_id',club->>'id','expected_revision',1,'title','Stale'));RAISE EXCEPTION 'STALE_WRITE';EXCEPTION WHEN serialization_failure THEN NULL;END;
INSERT INTO fixture_clubs_state VALUES('campaign',campaign),('club',club),('application',app);
END$$;
