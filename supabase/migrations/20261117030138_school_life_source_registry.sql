begin;
-- Generic school life is reusable; EduSet credentials and wakeup administration
-- remain separate CBsh-only modules, never inherited by another school.
update cbgpu_schools.feature_registry set globally_available=true,allowed_school_ids='{}' where key='school_life';
insert into cbgpu_schools.feature_registry(key,globally_available,allowed_school_ids,required_permission) values('cbsh_eduset',false,array['cbsh'],'VERIFIED_CBSH_MEMBERSHIP'),('cbsh_wakeup',false,array['cbsh'],'VERIFIED_CBSH_MEMBERSHIP') on conflict(key) do nothing;
do $$begin if not exists(select 1 from cbgpu_schools.migration_state where key='cbsh-life-modules-v1') then
 update cbgpu_schools.schools set features=array(select distinct unnest(features||array['cbsh_eduset','cbsh_wakeup'])) where id='cbsh';insert into cbgpu_schools.migration_state(key) values('cbsh-life-modules-v1');end if;end$$;
create table if not exists cbgpu_schools.life_sources(
 school_id text primary key references cbgpu_schools.schools(id),homepage_url text,news_url text,letter_url text,calendar_url text,meal_url text,
 allowed_hosts text[] not null default '{}',meal_provider text check(meal_provider in('SCHOOL_WEBSITE','NEIS')),
 comcigan_school_code integer check(comcigan_school_code>0),comcigan_school_name text,comcigan_region text,
 approved_by uuid references public.profiles(id),approved_at timestamptz,version integer not null default 1
);
alter table cbgpu_schools.life_sources enable row level security;
revoke all on cbgpu_schools.life_sources from public,anon,authenticated;
-- Known, already operating CBsh adapters. No secret or EduSet token is stored here.
insert into cbgpu_schools.life_sources(school_id,homepage_url,news_url,letter_url,calendar_url,meal_url,allowed_hosts,meal_provider,comcigan_school_name,comcigan_region,approved_at)
values('cbsh','https://school.cbe.go.kr/cbs-h/','https://school.cbe.go.kr/rss/cbs-h/M010401','https://school.cbe.go.kr/rss/cbs-h/M010403','https://school.cbe.go.kr/cbs-h/M010407/list','https://school.cbe.go.kr/cbs-h/M01050705/list',array['school.cbe.go.kr'],'SCHOOL_WEBSITE','충북과학고등학교','충청북도',now()) on conflict(school_id) do nothing;
create or replace function cbgpu_schools.valid_source_url(url text,hosts text[]) returns boolean language sql immutable set search_path='' as $$select url is null or(coalesce(length(url),0) between 8 and 2000 and url ~ '^https?://[a-z0-9][a-z0-9.-]*\.[a-z]{2,}(/[A-Za-z0-9/_?=&.%+~-]*)?$' and url !~* '[?&](key|api_key|access_token|token|secret|password|authorization)=' and split_part(split_part(url,'://',2),'/',1)=any(hosts) and split_part(split_part(url,'://',2),'/',1) !~ '(\.(local|internal|localhost|test|invalid))$')$$;
-- These are capabilities/configuration, not an assertion that an upstream is online.
create or replace function cbgpu_schools.life_capabilities(school text) returns jsonb language sql stable security definer set search_path='' as $$select coalesce((select jsonb_build_object('timetable',s.status='APPROVED' and 'school_life'=any(s.features) and length(s.official_name)>0 and length(s.region)>0,'meal',s.status='APPROVED' and 'school_life'=any(s.features) and((l.approved_at is not null and l.meal_url is not null)or(s.official_id is not null and s.education_office_code is not null)),'news',s.status='APPROVED' and 'school_life'=any(s.features) and l.approved_at is not null and(l.news_url is not null or l.letter_url is not null or l.calendar_url is not null)) from cbgpu_schools.schools s left join cbgpu_schools.life_sources l on l.school_id=s.id where s.id=school),jsonb_build_object('timetable',false,'meal',false,'news',false))$$;
create or replace function public.cbgpu_school_life_sources(p_school_id text) returns jsonb language plpgsql stable security definer set search_path='' as $$declare s cbgpu_schools.schools;l cbgpu_schools.life_sources;enabled boolean;begin
 if not public.cbgpu_personal_project_identity() or not(cbgpu_schools.active_member(auth.uid(),p_school_id) or exists(select 1 from public.profiles reviewer where reviewer.id=auth.uid() and reviewer.is_admin)) then raise exception 'ACTIVE_SCHOOL_MEMBERSHIP_REQUIRED' using errcode='42501';end if;
 select * into s from cbgpu_schools.schools where id=p_school_id and status='APPROVED';if not found then raise exception 'APPROVED_SCHOOL_REQUIRED' using errcode='P0002';end if;enabled:='school_life'=any(s.features) and exists(select 1 from cbgpu_schools.feature_registry f where f.key='school_life' and f.enabled);
 if enabled then select * into l from cbgpu_schools.life_sources where school_id=s.id and approved_at is not null;end if;
 return jsonb_build_object('school_id',s.id,'status',case when not enabled then 'DISABLED' when length(s.official_name)>0 and length(s.region)>0 then 'READY' else 'NOT_CONFIGURED' end,'official_id',s.official_id,'education_office_code',s.education_office_code,'school_name',s.official_name,'region',s.region,'homepage_url',case when enabled then coalesce(l.homepage_url,s.official_homepage_url) end,'news_url',l.news_url,'letter_url',l.letter_url,'calendar_url',l.calendar_url,'meal_url',l.meal_url,'allowed_hosts',coalesce(l.allowed_hosts,'{}'::text[]),'meal_provider',case when enabled then coalesce(l.meal_provider,case when s.official_id is not null and s.education_office_code is not null then 'NEIS' end) end,'comcigan',jsonb_build_object('status',case when not enabled then 'NOT_CONFIGURED' when l.comcigan_school_code is not null then 'READY' when length(s.official_name)>0 and length(s.region)>0 then 'LOOKUP_REQUIRED' else 'NOT_CONFIGURED' end,'school_code',l.comcigan_school_code,'school_name',coalesce(l.comcigan_school_name,s.official_name),'region',coalesce(l.comcigan_region,s.region)),'version',coalesce(l.version,0));
end$$;
-- Only explicit platform review can curate website adapters. The runtime fetcher
-- must additionally resolve/pin public DNS and validate every redirect; SQL URL
-- syntax checks do NOT establish network trust or fetch any supplied URL.
create or replace function public.cbgpu_school_life_configure(p_school_id text,p_payload jsonb) returns jsonb language plpgsql security definer set search_path='' as $$declare hosts text[];reviewer uuid:=auth.uid();before_data jsonb;begin
 if not public.cbgpu_personal_project_identity() or not exists(select 1 from public.profiles where id=reviewer and is_admin) then raise exception 'PLATFORM_ADMIN_REQUIRED' using errcode='42501';end if;
 if not exists(select 1 from cbgpu_schools.schools where id=p_school_id and status='APPROVED') then raise exception 'APPROVED_SCHOOL_REQUIRED';end if;
 if p_payload->'confirmed' is distinct from 'true'::jsonb or jsonb_typeof(p_payload->'allowed_hosts') is distinct from 'array' then raise exception 'CONFIRMED_SOURCE_REVIEW_REQUIRED';end if;
 hosts:=array(select distinct jsonb_array_elements_text(p_payload->'allowed_hosts'));
 if cardinality(hosts) not between 1 and 10 or array_position(hosts,null) is not null or exists(select 1 from unnest(hosts) h where h !~ '^[a-z0-9][a-z0-9.-]*\.[a-z]{2,}$' or h ~ '(\.(local|internal|localhost|test|invalid))$') or not cbgpu_schools.valid_source_url(p_payload->>'homepage_url',hosts) or not cbgpu_schools.valid_source_url(p_payload->>'news_url',hosts) or not cbgpu_schools.valid_source_url(p_payload->>'letter_url',hosts) or not cbgpu_schools.valid_source_url(p_payload->>'calendar_url',hosts) or not cbgpu_schools.valid_source_url(p_payload->>'meal_url',hosts) then raise exception 'INVALID_APPROVED_SOURCE_URL';end if;
 if nullif(p_payload->>'comcigan_school_code','') is not null and((p_payload->>'comcigan_school_code') !~ '^[1-9][0-9]{0,8}$' or coalesce(length(p_payload->>'comcigan_school_name'),0) not between 1 and 150 or coalesce(length(p_payload->>'comcigan_region'),0) not between 1 and 100) then raise exception 'INVALID_COMCIGAN_MAPPING';end if;
 select to_jsonb(previous) into before_data from cbgpu_schools.life_sources previous where previous.school_id=p_school_id;
 insert into cbgpu_schools.life_sources(school_id,homepage_url,news_url,letter_url,calendar_url,meal_url,allowed_hosts,meal_provider,comcigan_school_code,comcigan_school_name,comcigan_region,approved_by,approved_at)
 values(p_school_id,p_payload->>'homepage_url',p_payload->>'news_url',p_payload->>'letter_url',p_payload->>'calendar_url',p_payload->>'meal_url',hosts,p_payload->>'meal_provider',nullif(p_payload->>'comcigan_school_code','')::integer,left(p_payload->>'comcigan_school_name',150),left(p_payload->>'comcigan_region',100),reviewer,now())
 on conflict(school_id) do update set homepage_url=excluded.homepage_url,news_url=excluded.news_url,letter_url=excluded.letter_url,calendar_url=excluded.calendar_url,meal_url=excluded.meal_url,allowed_hosts=excluded.allowed_hosts,meal_provider=excluded.meal_provider,comcigan_school_code=excluded.comcigan_school_code,comcigan_school_name=excluded.comcigan_school_name,comcigan_region=excluded.comcigan_region,approved_by=reviewer,approved_at=now(),version=cbgpu_schools.life_sources.version+1;
 insert into cbgpu_schools.audit(actor_id,school_id,action,before_data,after_data) values(reviewer,p_school_id,'school_life_sources_configure',before_data,(select to_jsonb(current) from cbgpu_schools.life_sources current where current.school_id=p_school_id));
 return jsonb_build_object('ok',true);
end$$;
revoke all on function cbgpu_schools.valid_source_url(text,text[]),cbgpu_schools.life_capabilities(text) from public,anon,authenticated;
revoke all on function public.cbgpu_school_life_sources(text),public.cbgpu_school_life_configure(text,jsonb) from public,anon;
grant execute on function public.cbgpu_school_life_sources(text),public.cbgpu_school_life_configure(text,jsonb) to authenticated;
do $$declare definition text;begin
 definition:=pg_get_functiondef('cbgpu_accounts.can_user_use_resource(uuid,text,text)'::regprocedure);
 if position('cbsh_eduset' in definition)=0 then definition:=replace(definition,$needle$s.id='cbsh' and 'school_life'=any(s.features)$needle$,$replacement$s.id='cbsh' and 'school_life'=any(s.features) and 'cbsh_eduset'=any(s.features)$replacement$);execute definition;end if;
end$$;
commit;
