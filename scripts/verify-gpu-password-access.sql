\set ON_ERROR_STOP on
BEGIN;
SET LOCAL session_replication_role=replica;
INSERT INTO public.gpus(id,name,password) VALUES(999,'Temporary policy verification','not-a-real-password');
INSERT INTO public.reservations(id,user_id,gpu_id,status,start_time,end_time) VALUES('f74ec529-ced6-4978-9437-5fc82d5617b5','b2f97db7-7a83-40b2-a017-a05d52aac9c7',999,'approved',now()+interval '1 hour',now()+interval '2 hours');
SET LOCAL request.jwt.claims='{"sub":"b2f97db7-7a83-40b2-a017-a05d52aac9c7","role":"authenticated"}';
SET LOCAL ROLE authenticated;
SELECT public.get_active_gpu_password(999) IS NULL AS future_reservation_password_denied;
RESET ROLE;
UPDATE public.reservations SET start_time=now()-interval '1 hour',end_time=now()-interval '1 minute' WHERE id='f74ec529-ced6-4978-9437-5fc82d5617b5';
SET LOCAL ROLE authenticated;
SELECT public.get_active_gpu_password(999) IS NULL AS expired_reservation_password_denied;
RESET ROLE;
UPDATE public.reservations SET start_time=now()-interval '1 minute',end_time=now()+interval '1 hour' WHERE id='f74ec529-ced6-4978-9437-5fc82d5617b5';
SET LOCAL ROLE authenticated;
SELECT public.get_active_gpu_password(999) IS NOT NULL AS active_owner_password_available;
SELECT count(*)=0 AS direct_gpu_password_access_denied FROM public.gpus;
ROLLBACK;
