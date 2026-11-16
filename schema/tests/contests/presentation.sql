\set ON_ERROR_STOP on
begin;
insert into auth.users values ('00000000-0000-4000-8000-000000000001'),('00000000-0000-4000-8000-000000000002'),('00000000-0000-4000-8000-000000000003');
create function pg_temp.assert_true(ok boolean,label text) returns void language plpgsql as $$begin if ok is distinct from true then raise exception 'FAILED: %',label;end if;end$$;
create function pg_temp.expect_failure(command text) returns void language plpgsql as $$declare failed boolean:=false;begin begin execute command;exception when others then failed:=true;end;if not failed then raise exception 'Expected denial';end if;end$$;
set local role authenticated;
select set_config('request.jwt.claim.sub','00000000-0000-4000-8000-000000000001',true);
insert into storage.objects(bucket_id,name,metadata)values
 ('contest-presentation','00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000001.png','{"size":100,"mimetype":"image/png"}'),
 ('contest-presentation','00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000002.webp','{"size":100,"mimetype":"image/webp"}');
insert into public.contests(id,host_id,title,description,fields,banner_path)values('30000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000001','Presentation','# Intro ![image](https://example.invalid/00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000002.webp)','[{"id":"20000000-0000-4000-8000-000000000001","type":"text","label":"Text","description":"","required":false}]','00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000001.png');
update public.contests set description=description||E'\n**Updated**' where id='30000000-0000-4000-8000-000000000001';
select pg_temp.assert_true((select description like '%Updated%'from public.contests),'host can edit Markdown');
delete from storage.objects where bucket_id='contest-presentation';
select pg_temp.assert_true((select count(*)=2 from storage.objects where bucket_id='contest-presentation'),'banner and inline images remain protected');
select pg_temp.expect_failure($q$update public.contests set banner_path='00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000099.png'$q$);
insert into public.event_hosts(contest_id,user_id,role)values('30000000-0000-4000-8000-000000000001','00000000-0000-4000-8000-000000000003','manager');
select set_config('request.jwt.claim.sub','00000000-0000-4000-8000-000000000002',true);
update public.contests set description='forged';
select pg_temp.assert_true((select description<>'forged' from public.contests),'other users cannot replace presentation');
select pg_temp.expect_failure($q$insert into storage.objects(bucket_id,name)values('contest-presentation','00000000-0000-4000-8000-000000000001/10000000-0000-4000-8000-000000000003.png')$q$);
select set_config('request.jwt.claim.sub','00000000-0000-4000-8000-000000000003',true);
update public.contests set description='## Manager update';
select pg_temp.assert_true((select description='## Manager update'from public.contests),'delegated manager can edit presentation');
select set_config('request.jwt.claim.sub','00000000-0000-4000-8000-000000000001',true);
update public.contests set description='',banner_path=null;
delete from storage.objects where bucket_id='contest-presentation';
select pg_temp.assert_true((select count(*)=0 from storage.objects where bucket_id='contest-presentation'),'unused owned assets can be cleaned up');
rollback;
