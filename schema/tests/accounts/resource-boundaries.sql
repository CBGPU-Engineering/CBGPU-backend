\set ON_ERROR_STOP on
set role authenticated;
-- Outside school allocation rights, project metadata and personal data survive.
insert into cpu_projects values(1,null,'pending',null,'retained');
update cpu_projects set name='edited' where id=1;
do $$begin
 if not exists(select 1 from cpu_projects where id=1 and name='edited')then raise exception 'project continuity failed';end if;
 begin insert into reservations values(1,null,'pending',now(),now()+interval '1 hour');raise exception 'GPU bypass';exception when insufficient_privilege then null;end;
 begin insert into print_jobs values(1,null);raise exception 'print bypass';exception when insufficient_privilege then null;end;
 begin insert into laser_jobs values(1,null);raise exception 'laser bypass';exception when insufficient_privilege then null;end;
 begin update cpu_projects set deploy_requested_at=now() where id=1;raise exception 'execution bypass';exception when insufficient_privilege then null;end;
 begin insert into storage.objects values(1,'manufacturing');raise exception 'school upload bypass';exception when insufficient_privilege then null;end;
end$$;
insert into storage.objects values(2,'preprints');
select set_config('test.resource','gpu',false);
insert into reservations values(2,null,'pending',now(),now()+interval '1 hour');
select set_config('test.resource','',false);
-- Revocation prevents extensions; shortening and history remain available.
update reservations set end_time=start_time where id=2;
do $$begin begin update reservations set end_time=now()+interval '2 hours' where id=2;raise exception 'extension bypass';exception when insufficient_privilege then null;end;end$$;
select set_config('test.resource','school_compute',false);
update cpu_projects set deploy_requested_at=now(),status='approved' where id=1;
select set_config('test.resource','',false);
update cpu_projects set name='still retained' where id=1;
delete from cpu_projects where id=1;

reset role;
insert into print_jobs values(5,'00000000-0000-0000-0000-000000000001'),(6,'00000000-0000-0000-0000-000000000002');
set role authenticated;
do $$begin if (select count(*) from print_jobs)<>1 then raise exception 'equipment privacy boundary failed';end if;end$$;
