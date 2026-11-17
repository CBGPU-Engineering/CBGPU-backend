-- Bind an onboarding draft to official directory identity without granting affiliation or resources.
alter table cbgpu_schools.applications add column if not exists catalog_school_id text references cbgpu_schools.schools(id);
create or replace function cbgpu_schools.seal_catalog_application()returns trigger language plpgsql security definer set search_path='' as $$declare s cbgpu_schools.schools;begin
 if TG_OP='UPDATE' and old.catalog_school_id is not null and new.catalog_school_id is distinct from old.catalog_school_id then raise exception 'CATALOG_BINDING_IMMUTABLE' using errcode='23514';end if;
 if new.catalog_school_id is null and nullif(btrim(new.data#>>'{school,official_id}'),'')is not null then select id into new.catalog_school_id from cbgpu_schools.schools where official_id=btrim(new.data#>>'{school,official_id}') and source_sha256 is not null;end if;
 if new.catalog_school_id is not null then
  select * into s from cbgpu_schools.schools where id=new.catalog_school_id;
  if not found or s.source_sha256 is null then raise exception 'CATALOG_SCHOOL_NOT_FOUND' using errcode='P0002';end if;
  if new.data#>>'{school,official_name}' is distinct from s.official_name or new.data#>>'{school,type}' is distinct from s.type or new.data#>>'{school,official_id}' is distinct from s.official_id or new.data#>>'{school,region}' is distinct from s.region or new.data#>>'{school,address}' is distinct from s.address then raise exception 'CATALOG_IDENTITY_MISMATCH' using errcode='23514';end if;
  if new.school_id is not null and new.school_id<>s.id then raise exception 'CATALOG_TARGET_MISMATCH' using errcode='23514';end if;
 end if;return new;
end$$;
revoke all on function cbgpu_schools.seal_catalog_application()from public,anon,authenticated,service_role;
drop trigger if exists seal_catalog_application on cbgpu_schools.applications;
create trigger seal_catalog_application before insert or update on cbgpu_schools.applications for each row execute function cbgpu_schools.seal_catalog_application();
create or replace function public.cbgpu_school_catalog_application_api(action text,payload jsonb default '{}')returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();s cbgpu_schools.schools;r cbgpu_schools.applications;result jsonb;d jsonb;begin
 if action is distinct from 'draft_create' then raise exception 'INVALID_ACTION' using errcode='22023';end if;
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if jsonb_typeof(payload)<>'object'or exists(select 1 from jsonb_object_keys(payload)k where k not in('defaults','catalog_school_id'))then raise exception 'INVALID_REQUEST' using errcode='22023';end if;
 if not(payload?'catalog_school_id')then return public.cbgpu_school_api('draft_create','{}');end if;
 if jsonb_typeof(payload->'catalog_school_id')<>'string'or length(payload->>'catalog_school_id')not between 1 and 100 then raise exception 'INVALID_CATALOG_SCHOOL' using errcode='22023';end if;
 select * into s from cbgpu_schools.schools where id=payload->>'catalog_school_id' and source_sha256 is not null for share;
 if not found then raise exception 'CATALOG_SCHOOL_NOT_FOUND' using errcode='P0002';end if;
 if s.status='APPROVED' then return jsonb_build_object('ok',false,'code','DUPLICATE_SCHOOL','existing_school_id',s.id);end if;
 if s.status not in('LISTED','UNCLAIMED')then raise exception 'CATALOG_SCHOOL_UNAVAILABLE' using errcode='42501';end if;
 result:=public.cbgpu_school_api('draft_create','{}');
 select * into r from cbgpu_schools.applications where id=(result#>>'{application,id}')::uuid and applicant_id=uid for update;
 d:=jsonb_set(r.data,'{school}',jsonb_build_object('official_name',s.official_name,'type',s.type,'official_id',s.official_id,'region',s.region,'address',s.address,'contact_email','','contact_phone',s.contact_phone));
 d:=jsonb_set(d,'{configuration,display_name}',to_jsonb(s.display_name));
 update cbgpu_schools.applications set catalog_school_id=s.id,data=d where id=r.id returning * into r;
 return jsonb_build_object('application',to_jsonb(r)-'applicant_id'-'reviewed_by');
end$$;
revoke all on function public.cbgpu_school_catalog_application_api(text,jsonb)from public,anon,service_role;
grant execute on function public.cbgpu_school_catalog_application_api(text,jsonb)to authenticated;
