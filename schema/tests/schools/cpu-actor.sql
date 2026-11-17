-- Isolated trigger semantics independent of CPU business payload required fields.
begin;
create temporary table cpu_actor_probe (like public.cpu_projects including defaults);
create trigger cpu_actor_probe_guard before insert or update on cpu_actor_probe for each row execute function cbgpu_schools.guard_cpu_request_actor();
grant all on cpu_actor_probe to authenticated,service_role;
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at) values('fb100000-0000-4000-8000-000000000001','synthetic-cpu-actor@fixture.invalid','{"full_name":"CPU actor","account_type_request":"EXTERNAL"}','{}','authenticated','authenticated','fixture-no-password',now(),now(),now());
insert into auth.sessions(id,user_id) values('fb200000-0000-4000-8000-000000000001','fb100000-0000-4000-8000-000000000001');
select set_config('request.jwt.claims','{"sub":"fb100000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fb200000-0000-4000-8000-000000000001"}',true);
set local role authenticated;
-- Probe reuses actual trigger with dummy complete required project fields.
insert into cpu_actor_probe(id,user_id,user_email,name,deploy_type,host_port,deploy_requested_at,deploy_requested_by) values('fb300000-0000-4000-8000-000000000001',auth.uid(),'synthetic-cpu-actor@fixture.invalid','Synthetic CPU actor','docker',49000,now(),'fb100000-0000-4000-8000-000000000099');
do $$begin if not exists(select 1 from cpu_actor_probe where deploy_requested_by=auth.uid()) then raise exception 'CPU_ACTOR_SPOOF_NOT_REPLACED';end if;
begin update cpu_actor_probe set deploy_requested_by='fb100000-0000-4000-8000-000000000099';raise exception 'CPU_ACTOR_MUTABLE';exception when insufficient_privilege then null;end;end$$;
reset role;
select set_config('request.jwt.claims','{"role":"service_role"}',true);
set local role service_role;
do $$begin begin insert into cpu_actor_probe(id,user_id,user_email,name,deploy_type,host_port,deploy_requested_at) values('fb300000-0000-4000-8000-000000000002','fb100000-0000-4000-8000-000000000001','synthetic-cpu-actor@fixture.invalid','Unproven actor','docker',49001,now());raise exception 'SERVICE_REQUEST_WITHOUT_ACTOR';exception when check_violation then null;end;end$$;
reset role;
rollback;
