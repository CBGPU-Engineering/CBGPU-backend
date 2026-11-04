\set ON_ERROR_STOP on
BEGIN;
SET LOCAL session_replication_role=replica;
-- These rows never commit and are never visible to the live controller or notification worker.
INSERT INTO auth.users(id,instance_id,aud,role,email,raw_app_meta_data,raw_user_meta_data,created_at,updated_at) VALUES
 ('ab010000-0000-4000-8000-000000000001','00000000-0000-0000-0000-000000000000','authenticated','authenticated','printer-role-test@example.invalid','{}','{}',now(),now()),
 ('ab010000-0000-4000-8000-000000000002','00000000-0000-0000-0000-000000000000','authenticated','authenticated','printer-owner-test@example.invalid','{}','{}',now(),now());
INSERT INTO profiles(id,full_name,is_admin,is_approved) VALUES ('ab010000-0000-4000-8000-000000000001','Printer test',false,true),('ab010000-0000-4000-8000-000000000002','Request test',false,true);
INSERT INTO printers(id,name,status) VALUES (2000000101,'Transaction test printer A','idle'),(2000000102,'Transaction test printer B','idle');
INSERT INTO printer_managers(printer_id,user_id) VALUES(2000000101,'ab010000-0000-4000-8000-000000000001');
INSERT INTO print_jobs(id,user_id,mode,purpose,status,preferred_printer_id) VALUES
 ('ab020000-0000-4000-8000-000000000001','ab010000-0000-4000-8000-000000000002','beginner','ROLLBACK fixture','accepted',2000000101),
 ('ab020000-0000-4000-8000-000000000002','ab010000-0000-4000-8000-000000000002','beginner','ROLLBACK fixture','accepted',2000000102),
 ('ab020000-0000-4000-8000-000000000003','ab010000-0000-4000-8000-000000000002','beginner','ROLLBACK fixture','accepted',NULL);
SET LOCAL session_replication_role=origin;
SELECT set_config('request.jwt.claims','{"sub":"ab010000-0000-4000-8000-000000000001","role":"authenticated"}',true) IS NOT NULL;
SET LOCAL ROLE authenticated;
DO $$
BEGIN
 IF (SELECT count(*) FROM print_jobs WHERE id::text LIKE 'ab020000%')<>1 THEN RAISE EXCEPTION 'A printer manager can see other printers jobs'; END IF;
 BEGIN
  PERFORM public.start_print_job('ab020000-0000-4000-8000-000000000002',2000000101,1);
  RAISE EXCEPTION 'Cross-printer start was accepted';
 EXCEPTION WHEN object_not_in_prerequisite_state OR check_violation OR insufficient_privilege THEN NULL; END;
 BEGIN
  UPDATE printers SET current_job_id='ab020000-0000-4000-8000-000000000002' WHERE id=2000000101;
  RAISE EXCEPTION 'Legacy cross-printer link was accepted';
 EXCEPTION WHEN check_violation THEN NULL; END;
 IF (SELECT status FROM printers WHERE id=2000000101)<>'idle' THEN RAISE EXCEPTION 'Rejected start changed printer'; END IF;
 PERFORM public.start_print_job('ab020000-0000-4000-8000-000000000001',2000000101,1);
 IF NOT EXISTS(SELECT 1 FROM printers p JOIN print_jobs j ON j.id=p.current_job_id WHERE p.id=2000000101 AND j.printer_id=p.id AND j.preferred_printer_id=p.id AND j.status='printing' AND p.status='printing') THEN RAISE EXCEPTION 'Matching start was not atomic'; END IF;
 BEGIN
  PERFORM public.start_print_job('ab020000-0000-4000-8000-000000000001',2000000101,1);
  RAISE EXCEPTION 'Repeated start was accepted';
 EXCEPTION WHEN object_not_in_prerequisite_state THEN NULL; END;
END $$;
RESET ROLE;
-- Add the second assignment only inside this uncommitted fixture to test the preference guard directly.
INSERT INTO printer_managers(printer_id,user_id) VALUES(2000000102,'ab010000-0000-4000-8000-000000000001');
SET LOCAL ROLE authenticated;
DO $$
BEGIN
 BEGIN
  PERFORM public.start_print_job('ab020000-0000-4000-8000-000000000002',2000000101,1);
  RAISE EXCEPTION 'Busy printer accepted another job';
 EXCEPTION WHEN object_not_in_prerequisite_state THEN NULL; END;
 BEGIN
  UPDATE print_jobs SET printer_id=2000000101 WHERE id='ab020000-0000-4000-8000-000000000002';
  RAISE EXCEPTION 'Direct mismatched job assignment was accepted';
 EXCEPTION WHEN check_violation THEN NULL; END;
 BEGIN
  PERFORM public.start_print_job('ab020000-0000-4000-8000-000000000003',2000000102,1);
  RAISE EXCEPTION 'Unspecified job was accepted';
 EXCEPTION WHEN object_not_in_prerequisite_state OR check_violation THEN NULL; END;
 IF (SELECT status FROM printers WHERE id=2000000102)<>'idle' THEN RAISE EXCEPTION 'Rejected job modified second printer'; END IF;
END $$;
RESET ROLE;
ROLLBACK;
\echo 'PASS printer-specific RLS, preference enforcement, legacy-link guard, atomic start, busy/repeated start rejection; all fixtures rolled back.'
