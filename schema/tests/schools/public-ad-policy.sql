-- Public projection metadata only; all synthetic content/settings roll back.
begin;
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,status,configuration,features) values
 ('fixture-ad-a','Synthetic public ad A','synthetic public ad a','Ad A','HIGH_SCHOOL','APPROVED','{"ads_enabled":true}',array['projects','contests']),
 ('fixture-ad-b','Synthetic public ad B','synthetic public ad b','Ad B','HIGH_SCHOOL','APPROVED','{"ads_enabled":false}',array['projects','contests']),
 ('fixture-ad-string','Synthetic public ad string','synthetic public ad string','Ad string','HIGH_SCHOOL','APPROVED','{"ads_enabled":"true"}',array['projects','contests']),
 ('fixture-ad-listed','Synthetic public ad listed','synthetic public ad listed','Ad listed','HIGH_SCHOOL','LISTED','{"ads_enabled":true}','{}'),
 ('fixture-ad-suspended','Synthetic public ad suspended','synthetic public ad suspended','Ad suspended','HIGH_SCHOOL','SUSPENDED','{"ads_enabled":true}','{}'),
 ('fixture-ad-missing','Synthetic public ad missing','synthetic public ad missing','Ad missing','HIGH_SCHOOL','APPROVED','{}',array['projects']),
 ('fixture-ad-null','Synthetic public ad null','synthetic public ad null','Ad null','HIGH_SCHOOL','APPROVED','{"ads_enabled":null}',array['projects']);
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at) values('fa910000-0000-4000-8000-000000000001','synthetic-public-ad@fixture.invalid','{"full_name":"Public ad fixture","account_type_request":"EXTERNAL"}','{}','authenticated','authenticated','fixture-no-password',now(),now(),now());
insert into cbgpu_schools.public_project_profiles(id,owner_id,school_id,slug,title,summary,body_markdown,visibility,status) values
 ('fa920000-0000-4000-8000-000000000001','fa910000-0000-4000-8000-000000000001','fixture-ad-a','fixture-ad-public-a','Synthetic ad project A','Public fixture','Public body','PUBLIC','PUBLISHED'),
 ('fa920000-0000-4000-8000-000000000002','fa910000-0000-4000-8000-000000000001','fixture-ad-b','fixture-ad-public-b','Synthetic ad project B','Public fixture','Public body','PUBLIC','PUBLISHED'),
 ('fa920000-0000-4000-8000-000000000003','fa910000-0000-4000-8000-000000000001','fixture-ad-string','fixture-ad-public-string','Synthetic ad project string','Public fixture','Public body','PUBLIC','PUBLISHED'),
 ('fa920000-0000-4000-8000-000000000004','fa910000-0000-4000-8000-000000000001','fixture-ad-a','fixture-ad-private','Synthetic ad private project','Never public','Never public body','PRIVATE','PUBLISHED');
insert into public.contests(id,host_id,title,fields,school_id,access_scope,lifecycle_status,participation_mode,registration_closes_at,event_start_at,ends_at) values
 ('fa930000-0000-4000-8000-000000000001','fa910000-0000-4000-8000-000000000001','Synthetic ad contest A','[{"id":"fa940000-0000-4000-8000-000000000001","type":"text","label":"Fixture","description":"","required":false}]','fixture-ad-a','PUBLIC','published','individual',now()+interval '1 hour',now()+interval '1 day',now()+interval '2 days'),
 ('fa930000-0000-4000-8000-000000000002','fa910000-0000-4000-8000-000000000001','Synthetic ad contest B','[{"id":"fa940000-0000-4000-8000-000000000002","type":"text","label":"Fixture","description":"","required":false}]','fixture-ad-b','PUBLIC','published','individual',now()+interval '1 hour',now()+interval '1 day',now()+interval '2 days'),
 ('fa930000-0000-4000-8000-000000000003','fa910000-0000-4000-8000-000000000001','Synthetic ad private contest','[{"id":"fa940000-0000-4000-8000-000000000003","type":"text","label":"Fixture","description":"","required":false}]','fixture-ad-a','SCHOOL','published','individual',now()+interval '1 hour',now()+interval '1 day',now()+interval '2 days');
set local role anon;
do $$declare value jsonb;sid text;begin
 foreach sid in array array['fixture-ad-a','fixture-ad-b','fixture-ad-string','fixture-ad-listed','fixture-ad-missing','fixture-ad-null'] loop
  value:=public.cbgpu_public_content('school',jsonb_build_object('id',sid));
  if jsonb_typeof(value#>'{item,ads_enabled}')<>'boolean' or (value#>'{item,ads_enabled}'='true'::jsonb) is distinct from (sid='fixture-ad-a') or(value->'ads_enabled'='true'::jsonb) is distinct from (sid='fixture-ad-a') then raise exception 'PUBLIC_SCHOOL_AD_POLICY_OR_TYPE';end if;
  if value->'item' ?|array['email','contact_email','configuration','gpu'] then raise exception 'PUBLIC_AD_METADATA_PRIVACY_REGRESSION';end if;
 end loop;
 if public.cbgpu_public_content('school','{"id":"fixture-ad-suspended"}')#>'{item}'<>'null'::jsonb then raise exception 'SUSPENDED_SCHOOL_VISIBILITY_CHANGED';end if;
 value:=public.cbgpu_public_content('project','{"id":"fa920000-0000-4000-8000-000000000001"}');
 if value->'ads_enabled'<>'true'::jsonb or value#>'{item,ads_enabled}'<>'true'::jsonb or value->'item' ?|array['owner_id','email','configuration'] then raise exception 'PUBLIC_PROJECT_AD_POLICY';end if;
 if public.cbgpu_public_content('project','{"id":"fa920000-0000-4000-8000-000000000004"}')#>'{item}'<>'null'::jsonb then raise exception 'AD_FLAG_PUBLISHED_PRIVATE_PROJECT';end if;
 value:=public.cbgpu_public_content('contest','{"id":"fa930000-0000-4000-8000-000000000001"}');if value->'ads_enabled'<>'true'::jsonb or value#>'{item,ads_enabled}'<>'true'::jsonb then raise exception 'PUBLIC_CONTEST_AD_POLICY';end if;
 if public.cbgpu_public_content('contest','{"id":"fa930000-0000-4000-8000-000000000003"}')#>'{item}'<>'null'::jsonb then raise exception 'AD_FLAG_PUBLISHED_PRIVATE_CONTEST';end if;
 value:=public.cbgpu_public_content('projects_list','{"query":"Synthetic ad project A"}');if value->'ads_enabled'<>'true'::jsonb or jsonb_array_length(value->'items')<>1 then raise exception 'SINGLE_ENABLED_COLLECTION_AD_POLICY';end if;
 value:=public.cbgpu_public_content('projects_list','{"query":"Synthetic ad project"}');if value->'ads_enabled'<>'false'::jsonb or jsonb_array_length(value->'items')<>3 then raise exception 'MIXED_COLLECTION_AD_POLICY';end if;
 value:=public.cbgpu_public_content('contests_list','{"query":"Synthetic ad contest"}');if value->'ads_enabled'<>'false'::jsonb or jsonb_array_length(value->'items')<>2 then raise exception 'MIXED_CONTEST_COLLECTION_AD_POLICY';end if;
 value:=public.cbgpu_public_content('projects_list','{"query":"no-such-synthetic-ad-xxx"}');if value->'ads_enabled'<>'false'::jsonb then raise exception 'EMPTY_COLLECTION_ENABLED_ADS';end if;
end$$;
reset role;
-- Changes take effect on the very next public read, with no stale entitlement.
update cbgpu_schools.schools set configuration='{"ads_enabled":false}' where id='fixture-ad-a';
set local role anon;
do $$begin
 if public.cbgpu_public_content('project','{"id":"fa920000-0000-4000-8000-000000000001"}')#>'{item,ads_enabled}'<>'false'::jsonb or public.cbgpu_public_content('contest','{"id":"fa930000-0000-4000-8000-000000000001"}')#>'{item,ads_enabled}'<>'false'::jsonb then raise exception 'REVOKED_SCHOOL_AD_POLICY_STALE';end if;
 if public.cbgpu_public_content('projects_list','{"query":"Synthetic ad project A"}')->'ads_enabled'<>'false'::jsonb then raise exception 'REVOKED_COLLECTION_AD_POLICY_STALE';end if;
end$$;
reset role;
do $$begin
 if exists(select 1 from cbgpu_schools.schools where id in('fixture-ad-string','fixture-ad-null','fixture-ad-missing') and cbgpu_schools.public_ads_enabled(id)) then raise exception 'COERCED_AD_POLICY';end if;
 if cbgpu_schools.public_ads_enabled(null) or cbgpu_schools.public_ads_enabled('unknown-school') or cbgpu_schools.public_ads_enabled('fixture-ad-suspended') then raise exception 'UNKNOWN_OR_SUSPENDED_AD_POLICY';end if;
 if has_function_privilege('anon','cbgpu_schools.public_ads_enabled(text)','execute') or has_function_privilege('authenticated','cbgpu_schools.with_public_ad_policy(jsonb)','execute') then raise exception 'PRIVATE_AD_POLICY_HELPER_EXPOSED';end if;
end$$;
rollback;
