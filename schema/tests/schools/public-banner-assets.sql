-- Synthetic-only, full production-schema clone; never change operating data.
begin;
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,status,configuration,features) values('fixture-public-assets','Image fixture','image fixture','Image fixture','HIGH_SCHOOL','APPROVED','{}',array['contests']);
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)
select ('fa850000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-image-'||x||'@fixture.invalid',jsonb_build_object('full_name','Image fixture','account_type_request','EXTERNAL'),'{}','authenticated','authenticated','fixture-no-password',now(),now(),now() from generate_series(1,2)x;
insert into storage.objects(bucket_id,name,metadata) values('contest-presentation','fa850000-0000-4000-8000-000000000001/fa860000-0000-4000-8000-000000000001.png','{"mimetype":"image/png","size":24}'),('contest-presentation','fa850000-0000-4000-8000-000000000002/fa860000-0000-4000-8000-000000000002.png','{"mimetype":"image/png","size":24}');
insert into public.contests(id,host_id,title,fields,school_id,access_scope,lifecycle_status,banner_path,participation_mode,registration_closes_at,event_start_at,ends_at) values('fa870000-0000-4000-8000-000000000001','fa850000-0000-4000-8000-000000000001','Explicit image','[{"id":"fa880000-0000-4000-8000-000000000001","type":"text","label":"Fixture","description":"","required":false}]','fixture-public-assets','PUBLIC','published','fa850000-0000-4000-8000-000000000001/fa860000-0000-4000-8000-000000000001.png','individual',now()+interval '1 hour',now()+interval '1 day',now()+interval '2 days');
set local role cbgpu_public_asset_reader;
do $$declare descriptor jsonb;begin
 descriptor:=cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000001');
 if descriptor->>'bucket'<>'contest-presentation' or descriptor->>'mime'<>'image/png' or (descriptor->>'size')::int<>24 then raise exception 'PUBLIC_BANNER_DESCRIPTOR_MISSING';end if;
 begin perform id from public.contests limit 1;raise exception 'ASSET_ROLE_READS_RAW_CONTESTS';exception when insufficient_privilege then null;end;
 begin perform name from storage.objects limit 1;raise exception 'ASSET_ROLE_READS_RAW_STORAGE';exception when insufficient_privilege then null;end;
 if cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000099') is not null then raise exception 'UNKNOWN_ASSET_LEAK';end if;
end$$;
reset role;
-- Changing scope/lifecycle, school availability or global module policy denies
-- the very next resolver request, not a cached descriptor or JWT claim.
update public.contests set access_scope='SCHOOL' where id='fa870000-0000-4000-8000-000000000001';
set local role cbgpu_public_asset_reader;
do $$begin if cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000001') is not null then raise exception 'PRIVATE_SCOPE_ASSET_LEAK';end if;end$$;
reset role;
update public.contests set access_scope='PUBLIC',lifecycle_status='archived' where id='fa870000-0000-4000-8000-000000000001';
set local role cbgpu_public_asset_reader;
do $$begin if cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000001') is not null then raise exception 'ARCHIVED_ASSET_LEAK';end if;end$$;
reset role;
update public.contests set lifecycle_status='published' where id='fa870000-0000-4000-8000-000000000001';
update cbgpu_schools.schools set status='SUSPENDED' where id='fixture-public-assets';
set local role cbgpu_public_asset_reader;
do $$begin if cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000001') is not null then raise exception 'SUSPENDED_SCHOOL_ASSET_LEAK';end if;end$$;
reset role;
update cbgpu_schools.schools set status='APPROVED' where id='fixture-public-assets';
update cbgpu_schools.feature_registry set enabled=false where key='contests';
set local role cbgpu_public_asset_reader;
do $$begin if cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000001') is not null then raise exception 'DISABLED_MODULE_ASSET_LEAK';end if;end$$;
reset role;
update cbgpu_schools.feature_registry set enabled=true where key='contests';
-- A formerly valid banner cannot be changed to another owner's object.
update public.contests set banner_path='fa850000-0000-4000-8000-000000000002/fa860000-0000-4000-8000-000000000002.png' where id='fa870000-0000-4000-8000-000000000001';
set local role cbgpu_public_asset_reader;
do $$begin if cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000001') is not null then raise exception 'FOREIGN_OWNER_ASSET_LEAK';end if;end$$;
reset role;
update public.contests set banner_path='fa850000-0000-4000-8000-000000000001/fa860000-0000-4000-8000-000000000001.png' where id='fa870000-0000-4000-8000-000000000001';
update storage.objects set metadata='{"mimetype":"text/html","size":24}' where name='fa850000-0000-4000-8000-000000000001/fa860000-0000-4000-8000-000000000001.png';
set local role cbgpu_public_asset_reader;
do $$begin if cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000001') is not null then raise exception 'ACTIVE_CONTENT_ASSET_LEAK';end if;end$$;
reset role;
set local role anon;
do $$begin begin perform cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000001');raise exception 'ANON_DESCRIPTOR_LEAK';exception when insufficient_privilege then null;end;end$$;
reset role;
set local role authenticated;
do $$begin begin perform cbgpu_schools.public_contest_banner('fa870000-0000-4000-8000-000000000001');raise exception 'AUTH_DESCRIPTOR_LEAK';exception when insufficient_privilege then null;end;end$$;
reset role;
rollback;
select 'PUBLIC_BANNER_ROLE_AND_REVOCATION_TESTS_PASSED';
