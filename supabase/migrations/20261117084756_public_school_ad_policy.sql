begin;
-- Public ad eligibility is metadata only; it never changes visibility or settings.
-- String/number truthiness is not consent. Listed/suspended/missing schools deny.
create or replace function cbgpu_schools.public_ads_enabled(school text) returns boolean
language sql stable security definer set search_path='' as $$
 select exists(select 1 from cbgpu_schools.schools s where s.id=school and s.status='APPROVED' and s.configuration->'ads_enabled'='true'::jsonb)
$$;
revoke all on function cbgpu_schools.public_ads_enabled(text) from public,anon,authenticated;
grant execute on function cbgpu_schools.public_ads_enabled(text) to postgres;

-- Lists are conservative: one false/unknown item or an empty page disables ads.
create or replace function cbgpu_schools.with_public_ad_policy(value jsonb) returns jsonb
language sql immutable set search_path='' as $$
 select value||jsonb_build_object('ads_enabled',case
 when jsonb_typeof(value->'items')='array' then jsonb_array_length(value->'items')>0 and not exists(select 1 from jsonb_array_elements(value->'items')i where i->'ads_enabled' is distinct from 'true'::jsonb)
 when jsonb_typeof(value->'item')='object' then value#>'{item,ads_enabled}'='true'::jsonb
 else false end is true)
$$;
revoke all on function cbgpu_schools.with_public_ad_policy(jsonb) from public,anon,authenticated;
grant execute on function cbgpu_schools.with_public_ad_policy(jsonb) to postgres;

do $$declare definition text;updated text;begin
 definition:=pg_get_functiondef('cbgpu_schools.public_project(cbgpu_schools.public_project_profiles)'::regprocedure);
 if position('cbgpu_schools.public_ads_enabled' in definition)=0 then
  updated:=replace(definition,$a$'updated_at',p.updated_at)$a$,$b$'updated_at',p.updated_at,'ads_enabled',cbgpu_schools.public_ads_enabled(p.school_id))$b$);
  if updated=definition then raise exception 'PUBLIC_PROJECT_AD_PROJECTION_VERSION_MISMATCH';end if;execute updated;
 end if;
 alter function cbgpu_schools.public_project(cbgpu_schools.public_project_profiles) stable security definer;
 definition:=pg_get_functiondef('cbgpu_schools.public_contest(public.contests)'::regprocedure);
 if position('cbgpu_schools.public_ads_enabled' in definition)=0 then
  updated:=replace(definition,$a$'created_at',c.created_at)$a$,$b$'created_at',c.created_at,'ads_enabled',cbgpu_schools.public_ads_enabled(c.school_id))$b$);
  if updated=definition then raise exception 'PUBLIC_CONTEST_AD_PROJECTION_VERSION_MISMATCH';end if;execute updated;
 end if;
 definition:=pg_get_functiondef('cbgpu_schools.summary(cbgpu_schools.schools)'::regprocedure);
 if position('cbgpu_schools.public_ads_enabled' in definition)=0 then
  updated:=replace(definition,$a$'ads_enabled',coalesce((s.configuration->>'ads_enabled')::boolean,false)$a$,$b$'ads_enabled',cbgpu_schools.public_ads_enabled(s.id)$b$);
  if updated=definition then raise exception 'PUBLIC_SCHOOL_AD_PROJECTION_VERSION_MISMATCH';end if;execute updated;
 end if;
 definition:=pg_get_functiondef('public.cbgpu_public_content(text,jsonb)'::regprocedure);
 if position('cbgpu_schools.with_public_ad_policy' in definition)=0 then
  -- Keep every existing query/filter/DTO intact and wrap only its returned object.
  updated:=regexp_replace(definition,'return ([^;]+);end if;','return cbgpu_schools.with_public_ad_policy(\1);end if;','g');
  if updated=definition then raise exception 'PUBLIC_COLLECTION_AD_PROJECTION_VERSION_MISMATCH';end if;execute updated;
 end if;
end$$;
notify pgrst,'reload schema';
commit;
