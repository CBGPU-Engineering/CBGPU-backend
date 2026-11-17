begin;
-- Synthetic NEIS-shaped identity avoids colliding with an already-imported real directory.
-- Actual NEIS import/HTTP evidence is separate; all fixture claims and mutations roll back.
insert into cbgpu_schools.schools(id,official_name,normalized_name,official_id,display_name,type,region,address,contact_phone,configuration,features,status,education_office_code,source_sha256,official_homepage_url)values('fixture-catalog-maritime','합성해사고등학교','합성해사고등학교','ZZ99999','합성해사고등학교','HIGH_SCHOOL','부산광역시','부산광역시 영도구 해양로 425','051-410-2000',cbgpu_schools.default_draft()->'configuration','{}','LISTED','C10',repeat('c',64),'http://www.maritime.hs.kr/');
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at) select ('fb610000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-catalog-app-'||x||'@fixture.invalid',jsonb_build_object('full_name','Catalog application fixture','account_type_request','EXTERNAL'),'{}','authenticated','authenticated','fixture-no-password',now(),now(),now() from generate_series(1,2)x;
insert into auth.sessions(id,user_id) select ('fb620000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,('fb610000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid from generate_series(1,2)x;
select set_config('request.jwt.claims','{"role":"service_role"}',true);update public.profiles set is_admin=true where id='fb610000-0000-4000-8000-000000000002';
select set_config('request.jwt.claims','{"sub":"fb610000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fb620000-0000-4000-8000-000000000001"}',true);set local role authenticated;
create temp table catalog_draft as select public.cbgpu_school_catalog_application_api('draft_create','{"catalog_school_id":"fixture-catalog-maritime"}')value;
do $$declare a jsonb:=(select value->'application'from catalog_draft);d jsonb;result jsonb;begin
 if a->>'catalog_school_id'<>'fixture-catalog-maritime'or a#>>'{data,school,official_id}'<>'ZZ99999'or a#>>'{data,school,address}'<>'부산광역시 영도구 해양로 425'or a#>>'{data,gpu,choice}'<>'CONNECT_LATER' then raise exception 'CATALOG_PREFILL_NOT_CANONICAL';end if;
 d:=a->'data';begin perform public.cbgpu_school_api('draft_save',jsonb_build_object('id',a->>'id','revision',1,'step',1,'data',jsonb_set(d,'{school,official_name}','"Spoofed school"')));raise exception 'CATALOG_SPOOF_ALLOWED';exception when check_violation then null;end;
 d:=jsonb_set(d,'{school,contact_email}','"fixture@example.invalid"');d:=jsonb_set(d,'{administrator}','{"name":"Applicant","role":"STAFF","relationship":"School operator","contact_email":"fixture@example.invalid","verification_note":"Synthetic test"}');d:=jsonb_set(d,'{features}','["community","projects","contests","school_life"]');
 result:=public.cbgpu_school_api('draft_save',jsonb_build_object('id',a->>'id','revision',1,'step',6,'data',d));result:=public.cbgpu_school_api('draft_submit',jsonb_build_object('id',a->>'id','revision',2,'confirmed',true));if result#>>'{application,school_id}'<>'fixture-catalog-maritime'or result#>>'{application,status}'<>'PENDING'then raise exception 'CATALOG_SUBMISSION_NOT_BOUND';end if;
end$$;
select set_config('request.jwt.claims','{"sub":"fb610000-0000-4000-8000-000000000002","role":"authenticated","session_id":"fb620000-0000-4000-8000-000000000002"}',true);
select public.cbgpu_school_api('platform_review',jsonb_build_object('id',(select value#>>'{application,id}'from catalog_draft),'revision',3,'decision','APPROVE','note','Synthetic explicit independent review'));
select set_config('request.jwt.claims','{"sub":"fb610000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fb620000-0000-4000-8000-000000000001"}',true);
do $$declare r jsonb;begin r:=public.cbgpu_school_life_sources('fixture-catalog-maritime');if r->>'official_id'<>'ZZ99999'or r->>'education_office_code'<>'C10'or r->>'meal_provider'<>'NEIS'or r#>>'{comcigan,school_name}'<>'합성해사고등학교'or r#>>'{comcigan,status}'<>'LOOKUP_REQUIRED'then raise exception 'CATALOG_LIFE_SOURCE_LOST';end if;
 r:=public.cbgpu_school_catalog_application_api('draft_create','{"catalog_school_id":"fixture-catalog-maritime"}');if r->>'code'<>'DUPLICATE_SCHOOL'then raise exception 'CATALOG_DUPLICATE_NOT_BLOCKED';end if;
 if public.get_is_approved(auth.uid()) then raise exception 'SCHOOL_OWNER_ACQUIRED_CBSH_GPU';end if;
end$$;
reset role;
do $$begin if(select count(*)from cbgpu_schools.schools where official_id='ZZ99999')<>1 or(select status from cbgpu_schools.schools where id='fixture-catalog-maritime')<>'APPROVED'then raise exception 'CATALOG_SCHOOL_DUPLICATED';end if;if(select account_type<>'EXTERNAL'or is_admin or is_approved from public.profiles where id='fb610000-0000-4000-8000-000000000001')then raise exception 'CATALOG_APPROVAL_GLOBAL_PRIVILEGE_CHANGE';end if;end$$;
rollback;
