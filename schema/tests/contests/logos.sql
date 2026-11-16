\set ON_ERROR_STOP on
begin;
insert into auth.users values ('00000000-0000-4000-8000-000000000001'),('00000000-0000-4000-8000-000000000002');
create or replace function pg_temp.assert_true(ok boolean, description text) returns void language plpgsql as $$
begin if ok is distinct from true then raise exception 'FAILED: %', description; end if; end $$;
create or replace function pg_temp.expect_failure(command text, description text) returns void language plpgsql as $$
declare failed boolean := false;
begin
  begin execute command; exception when others then failed := true; end;
  if not failed then raise exception 'FAILED (unexpected success): %', description; end if;
end $$;
set local role authenticated;
select set_config('request.jwt.claim.sub','00000000-0000-4000-8000-000000000001',true);
insert into storage.objects(bucket_id,name,metadata) values ('contest-logos','00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000001.png','{"size":100,"mimetype":"image/png"}');
insert into public.contests(id,host_id,title,fields,logo_path) values (
 '30000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000001','로고 대회',
 '[{"id":"20000000-0000-4000-8000-000000000001","type":"text","label":"소개","description":"","required":false}]',
 '00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000001.png');
delete from storage.objects where bucket_id='contest-logos';
select pg_temp.assert_true((select count(*)=1 from storage.objects where bucket_id='contest-logos'),'referenced logo cannot be deleted');
select pg_temp.expect_failure($q$update public.contests set logo_path='00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000099.png'$q$,'reject nonexistent logo');
select pg_temp.expect_failure($q$insert into storage.objects(bucket_id,name,metadata) values ('contest-logos','00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000002.svg','{"size":100,"mimetype":"image/svg+xml"}')$q$,'reject SVG upload');
insert into storage.objects(bucket_id,name,metadata) values ('contest-logos','00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000003.png','{"size":2097153,"mimetype":"image/png"}');
select pg_temp.expect_failure($q$update public.contests set logo_path='00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000003.png'$q$,'reject oversized logo metadata');
select set_config('request.jwt.claim.sub','00000000-0000-4000-8000-000000000002',true);
insert into storage.objects(bucket_id,name,metadata) values ('contest-logos','00000000-0000-4000-8000-000000000002/10000000-0000-4000-8000-000000000004.webp','{"size":100,"mimetype":"image/webp"}');
update public.contests set logo_path=null;
select pg_temp.assert_true((select logo_path is not null from public.contests),'non-host cannot remove logo');
select pg_temp.expect_failure($q$insert into storage.objects(bucket_id,name,metadata) values ('contest-logos','00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000005.png','{"size":100,"mimetype":"image/png"}')$q$,'cannot upload into another host folder');
select set_config('request.jwt.claim.sub','00000000-0000-4000-8000-000000000001',true);
select pg_temp.expect_failure($q$update public.contests set logo_path='00000000-0000-4000-8000-000000000002/10000000-0000-4000-8000-000000000004.webp'$q$,'cannot use another host logo');
update public.contests set logo_path=null;
delete from storage.objects where name='00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000001.png';
select pg_temp.assert_true((select count(*)=0 from storage.objects where name='00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000001.png'),'removed logo can be cleaned up');
rollback;
\echo 'PASS: logo creation/removal, host ownership, referenced asset protection, MIME/size validation'
