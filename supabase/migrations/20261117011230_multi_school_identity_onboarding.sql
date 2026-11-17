begin;
-- Candidate only. Requires reviewed account-v2 migration; no production execution.
create schema if not exists cbgpu_schools;
revoke all on schema cbgpu_schools from public,anon,authenticated;
create table if not exists cbgpu_schools.schools(
 id text primary key, official_name text not null, normalized_name text not null,
 official_id text unique, display_name text not null, type text not null,
 region text not null default '', address text not null default '', contact_email text not null default '',contact_phone text not null default '',
 brand_color text not null default '#17634d',logo_path text,
 status text not null default 'APPROVED' check(status in('LISTED','UNCLAIMED','APPROVED','SUSPENDED')),
 configuration jsonb not null,features text[] not null default '{}', revision integer not null default 1,
 created_at timestamptz not null default now(),verified_at timestamptz
);
create table if not exists cbgpu_schools.feature_registry(
 key text primary key,version integer not null default 1,globally_available boolean not null default false,
 allowed_school_ids text[] not null default '{}',required_permission text not null,
 configuration_schema jsonb not null default '{"type":"object","additionalProperties":false}',enabled boolean not null default true
);
insert into cbgpu_schools.feature_registry(key,globally_available,allowed_school_ids,required_permission)
select key,key<>'school_life',case when key='school_life' then array['cbsh'] else '{}'::text[] end,'ACTIVE_VERIFIED_MEMBERSHIP' from unnest(array['community','school_life','projects','contests','gpu_compute','ai','materials','manufacturing','latex']) key on conflict(key) do nothing;
create table if not exists cbgpu_schools.feature_configuration(school_id text not null references cbgpu_schools.schools(id),feature_key text not null references cbgpu_schools.feature_registry(key),version integer not null default 1,configuration jsonb not null default '{}',primary key(school_id,feature_key));
create table if not exists cbgpu_schools.memberships(
 id uuid primary key default gen_random_uuid(),user_id uuid not null references auth.users(id),school_id text not null references cbgpu_schools.schools(id),
 membership_type text not null check(membership_type in('STUDENT','TEACHER','EXTERNAL')),
 status text not null check(status in('ACTIVE','PENDING_VERIFICATION','REVERIFICATION_REQUIRED','SUSPENDED')),
 verified boolean not null default false,school_roles text[] not null default '{}',student_number text,student_number_school_year integer,department text,
 verification_note text,verified_by uuid references public.profiles(id),verified_at timestamptz,
 starts_at timestamptz not null default now(),ends_at timestamptz,created_at timestamptz not null default now(),
 unique(user_id,school_id),check(school_roles <@ array['STUDENT','TEACHER','SCHOOL_ADMIN','SCHOOL_OWNER','COMMUNITY_MODERATOR','GPU_ADMIN']::text[])
);
create index if not exists memberships_school_status on cbgpu_schools.memberships(school_id,status,user_id);
create index if not exists memberships_student_number on cbgpu_schools.memberships(school_id,student_number_school_year,student_number) where student_number is not null;
create table if not exists cbgpu_schools.applications(
 id uuid primary key default gen_random_uuid(),applicant_id uuid not null references public.profiles(id),status text not null default 'DRAFT' check(status in('DRAFT','PENDING','APPROVED','REJECTED')),
 step integer not null default 1 check(step between 1 and 6),revision integer not null default 1,data jsonb not null,
 review_note text,reviewed_by uuid references public.profiles(id),school_id text references cbgpu_schools.schools(id),created_at timestamptz not null default now(),updated_at timestamptz not null default now()
);
create index if not exists applications_applicant on cbgpu_schools.applications(applicant_id,created_at desc);
create table if not exists cbgpu_schools.preferences(user_id uuid primary key references public.profiles(id),school_id text not null references cbgpu_schools.schools(id));
create table if not exists cbgpu_schools.audit(id bigint generated always as identity primary key,actor_id uuid,school_id text,action text not null,target_id uuid,before_data jsonb,after_data jsonb,created_at timestamptz not null default now());
create table if not exists cbgpu_schools.gpu_nodes(id uuid primary key default gen_random_uuid(),school_id text not null references cbgpu_schools.schools(id),name text not null,status text not null default 'NOT_CONFIGURED' check(status in('NOT_CONFIGURED','PENDING','OFFLINE','ONLINE','REVOKED')),sharing_enabled boolean not null default false,capabilities jsonb not null default '{}',created_at timestamptz not null default now());
alter table cbgpu_schools.feature_registry enable row level security;
alter table cbgpu_schools.feature_configuration enable row level security;
-- Never expose private contacts, applications or membership records through table grants.
alter table cbgpu_schools.schools enable row level security;
alter table cbgpu_schools.memberships enable row level security;
alter table cbgpu_schools.applications enable row level security;
alter table cbgpu_schools.preferences enable row level security;
alter table cbgpu_schools.audit enable row level security;
alter table cbgpu_schools.gpu_nodes enable row level security;
revoke all on all tables in schema cbgpu_schools from public,anon,authenticated;
create or replace function cbgpu_schools.default_draft() returns jsonb language sql immutable set search_path='' as $$select '{"school":{"official_name":"","type":"HIGH_SCHOOL","official_id":"","region":"","address":"","contact_email":"","contact_phone":""},"administrator":{"name":"","role":"STUDENT_OPERATOR","relationship":"","contact_email":"","verification_note":""},"configuration":{"display_name":"","brand_color":"#17634d","join_policy":"APPROVAL_REQUIRED","student_id_policy":"SCHOOL_DEFINED","teacher_policy":"MANUAL_REVIEW","community_policy":"MEMBERS_ONLY"},"features":["community","projects","contests"],"gpu":{"choice":"CONNECT_LATER"},"confirmed":false}'::jsonb$$;
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,region,configuration,features,verified_at)
values('cbsh','충북과학고등학교','충북과학고등학교','충북과학고등학교','HIGH_SCHOOL','충청북도', '{"join_policy":"APPROVAL_REQUIRED","student_id_policy":"CBSH_4_DIGIT","teacher_policy":"MANUAL_REVIEW","community_policy":"MEMBERS_ONLY"}',array['community','school_life','projects','contests','gpu_compute','ai','materials','manufacturing','latex'],now()) on conflict(id) do nothing;
update cbgpu_schools.schools set official_id='8000075' where id='cbsh' and official_id is null;
-- Explicit existing-CBsh ad policy; newly listed/registered schools stay off.
update cbgpu_schools.schools set configuration=jsonb_set(configuration,'{ads_enabled}','true'::jsonb) where id='cbsh' and not(configuration?'ads_enabled');
-- Snapshot existing users exactly once, including when this file is replayed later.
create table if not exists cbgpu_schools.migration_state(key text primary key, completed_at timestamptz not null default now());
revoke all on cbgpu_schools.migration_state from public,anon,authenticated;
do $$begin
if not exists(select 1 from cbgpu_schools.migration_state where key='legacy-cbsh-v1') then
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,school_roles,student_number,student_number_school_year,department,verified_at)
select u.id,'cbsh',coalesce(p.account_type,'EXTERNAL'),case when u.deleted_at is not null then 'SUSPENDED' else coalesce(p.account_status,'PENDING_VERIFICATION') end,coalesce(p.affiliation_verified,false),
 case when p.is_admin and p.account_status='ACTIVE' then array['SCHOOL_ADMIN','SCHOOL_OWNER']::text[] else '{}'::text[] end,
 p.student_id,p.student_id_school_year,p.department,p.verified_at from auth.users u left join public.profiles p on p.id=u.id on conflict(user_id,school_id) do nothing;
insert into cbgpu_schools.migration_state(key) values('legacy-cbsh-v1');
end if;end$$;
create or replace function cbgpu_schools.summary(s cbgpu_schools.schools) returns jsonb language sql stable security definer set search_path='' as $$select jsonb_build_object('id',s.id,'official_name',s.official_name,'display_name',s.display_name,'type',s.type,'region',s.region,'brand_color',s.brand_color,'logo_path',s.logo_path,'status',s.status,'features',s.features,'allowed_membership_types',jsonb_build_array('STUDENT','TEACHER','EXTERNAL'),'join_policy',s.configuration->>'join_policy','student_id_policy',s.configuration->>'student_id_policy','teacher_policy',s.configuration->>'teacher_policy','ads_enabled',coalesce((s.configuration->>'ads_enabled')::boolean,false),'gpu',jsonb_build_object('status',case when exists(select 1 from cbgpu_schools.gpu_nodes n where n.school_id=s.id and n.status='ONLINE') then 'ONLINE' when exists(select 1 from cbgpu_schools.gpu_nodes n where n.school_id=s.id) then 'OFFLINE' else 'NOT_CONFIGURED' end,'nodes',(select count(*) from cbgpu_schools.gpu_nodes n where n.school_id=s.id),'sharing_enabled',false))$$;
create or replace function cbgpu_schools.member_json(m cbgpu_schools.memberships) returns jsonb language sql stable security definer set search_path='' as $$select (to_jsonb(m)-'verification_note') ||jsonb_build_object('school',cbgpu_schools.summary(s)) from cbgpu_schools.schools s where s.id=m.school_id$$;
create or replace function cbgpu_schools.active_member(uid uuid,school text) returns boolean language sql stable security definer set search_path='' as $$select exists(select 1 from cbgpu_schools.memberships m join cbgpu_schools.schools s on s.id=m.school_id join public.profiles p on p.id=m.user_id join auth.users u on u.id=p.id where m.user_id=uid and m.school_id=school and m.status='ACTIVE' and m.verified and m.starts_at<=now() and (m.ends_at is null or m.ends_at>now()) and s.status='APPROVED' and p.account_status<>'SUSPENDED' and u.deleted_at is null and (u.banned_until is null or u.banned_until<=now()))$$;
create or replace function cbgpu_schools.has_role(uid uuid,school text,roles text[]) returns boolean language sql stable security definer set search_path='' as $$select cbgpu_schools.active_member(uid,school) and exists(select 1 from cbgpu_schools.memberships m where m.user_id=uid and m.school_id=school and m.school_roles&&roles)$$;
create or replace function cbgpu_schools.features_allowed(school text,features text[]) returns boolean language sql stable security definer set search_path='' as $$select not exists(select 1 from unnest(features) feature left join cbgpu_schools.feature_registry r on r.key=feature where r.key is null or not r.enabled or not(r.globally_available or school=any(r.allowed_school_ids)))$$;
create or replace function cbgpu_schools.validate_draft(d jsonb) returns boolean language sql immutable set search_path='' as $$select jsonb_typeof(d)='object' and length(btrim(d#>>'{school,official_name}')) between 2 and 150 and length(btrim(d#>>'{school,region}')) between 1 and 100 and length(btrim(d#>>'{school,address}')) between 1 and 500 and d#>>'{school,contact_email}' ~ '^[^ @]+@[^ @]+\.[^ @]+$' and length(btrim(d#>>'{administrator,name}')) between 1 and 100 and length(btrim(d#>>'{administrator,relationship}')) between 1 and 1000 and d#>>'{administrator,contact_email}' ~ '^[^ @]+@[^ @]+\.[^ @]+$' and d#>>'{administrator,role}' in('STUDENT_OPERATOR','TEACHER','STAFF','OTHER') and length(btrim(d#>>'{configuration,display_name}')) between 1 and 150 and d#>>'{configuration,brand_color}' ~ '^#[0-9A-Fa-f]{6}$' and d#>>'{configuration,join_policy}'='APPROVAL_REQUIRED' and d#>>'{configuration,teacher_policy}'='MANUAL_REVIEW' and d#>>'{configuration,community_policy}'='MEMBERS_ONLY' and d#>>'{configuration,student_id_policy}' in('SCHOOL_DEFINED','CBSH_4_DIGIT') and jsonb_typeof(d->'features')='array' and not exists(select 1 from jsonb_array_elements_text(d->'features') f where f not in('community','school_life','projects','contests','gpu_compute','ai','materials','manufacturing','latex')) and d#>>'{gpu,choice}' in('CONNECT_LATER','NONE','CONNECT_NOW')$$;
create or replace function cbgpu_schools.validate_configuration(cfg jsonb) returns boolean language sql immutable set search_path='' as $$select coalesce(jsonb_typeof(cfg)='object' and length(btrim(cfg->>'display_name')) between 1 and 150 and cfg->>'brand_color' ~ '^#[0-9A-Fa-f]{6}$' and cfg->>'join_policy'='APPROVAL_REQUIRED' and cfg->>'teacher_policy'='MANUAL_REVIEW' and cfg->>'community_policy'='MEMBERS_ONLY' and cfg->>'student_id_policy' in('CBSH_4_DIGIT','SCHOOL_DEFINED') and(not(cfg?'ads_enabled') or jsonb_typeof(cfg->'ads_enabled')='boolean') and not exists(select 1 from jsonb_object_keys(cfg) k where k not in('display_name','brand_color','join_policy','student_id_policy','teacher_policy','community_policy','ads_enabled')),false)$$;
-- A new school's untrusted signup intent can only request membership; it grants no roles.
create or replace function cbgpu_schools.signup_membership() returns trigger language plpgsql security definer set search_path='' as $$
declare intent jsonb:=new.raw_user_meta_data->'school_join_request';sid text;kind text;number text;
begin
 if intent is null then
 if coalesce(new.raw_user_meta_data->>'account_type_request','STUDENT') in('STUDENT','TEACHER') then sid:='cbsh';kind:=coalesce(new.raw_user_meta_data->>'account_type_request','STUDENT');number:=new.raw_user_meta_data->>'student_id';else return new;end if;
 else
 sid:=intent->>'school_id';kind:=intent->>'type';number:=nullif(btrim(intent->>'student_number'),'');
 if jsonb_typeof(intent)<>'object' or kind not in('STUDENT','TEACHER','EXTERNAL') or not exists(select 1 from cbgpu_schools.schools s where s.id=sid and s.status='APPROVED') then raise exception 'INVALID_SCHOOL_JOIN_REQUEST' using errcode='23514';end if;
 end if;
 if kind='STUDENT' and (number is null or length(number)>80 or (sid='cbsh' and number !~ '^[1-3][1-3][0-9]{2}$')) then raise exception 'INVALID_STUDENT_NUMBER' using errcode='23514';end if;
 insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,student_number,department) values(new.id,sid,kind,'PENDING_VERIFICATION',false,case when kind='STUDENT' then number end,left(intent->>'department',200)) on conflict(user_id,school_id) do nothing;return new;
end$$;
-- z-prefix runs after the existing handle_new_user profile insertion trigger.
drop trigger if exists z_cbgpu_school_signup on auth.users;
create trigger z_cbgpu_school_signup after insert on auth.users for each row execute function cbgpu_schools.signup_membership();
create or replace function public.cbgpu_school_api(action text,payload jsonb default '{}'::jsonb) returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();platform boolean:=false;sid text:=payload->>'school_id';r cbgpu_schools.applications;m cbgpu_schools.memberships;s cbgpu_schools.schools;before_value jsonb;d jsonb;kind text;decision text;roles text[];result jsonb;lim integer:=least(100,greatest(1,coalesce((payload->>'limit')::integer,30)));off integer:=greatest(0,coalesce((payload->>'offset')::integer,0));q text:=left(coalesce(payload->>'q',''),150);owner_permission boolean;
begin
 if action='feature_catalog' then return jsonb_build_object('items',(select coalesce(jsonb_agg(jsonb_build_object('key',f.key,'version',f.version,'globally_available',f.globally_available,'required_permission',f.required_permission,'configurable',f.configuration_schema)),'[]') from cbgpu_schools.feature_registry f where f.enabled and(f.globally_available or sid=any(f.allowed_school_ids))));end if;
 if action='directory' then return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.summary(z)),'[]') from(select * from cbgpu_schools.schools where status in('LISTED','UNCLAIMED','APPROVED') and (official_name ilike '%'||q||'%' or display_name ilike '%'||q||'%') order by case when id='cbsh' then 0 else 1 end,display_name limit lim offset off) z),'total',(select count(*) from cbgpu_schools.schools where status in('LISTED','UNCLAIMED','APPROVED') and(official_name ilike '%'||q||'%' or display_name ilike '%'||q||'%')));end if;
 if action='school' then select * into s from cbgpu_schools.schools where id=sid and status in('LISTED','UNCLAIMED','APPROVED');if not found then raise exception 'SCHOOL_NOT_FOUND' using errcode='P0002';end if;return jsonb_build_object('school',cbgpu_schools.summary(s));end if;
 if uid is null or not cbgpu_accounts.current_session_valid() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 select p.is_admin into platform from public.profiles p join auth.users u on u.id=p.id where p.id=uid and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now());if not found then raise exception 'ACCOUNT_UNAVAILABLE' using errcode='42501';end if;platform:=coalesce(platform,false);
 if action='memberships' then return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.member_json(t)),'[]') from cbgpu_schools.memberships t where t.user_id=uid),'current_school_id',(select p.school_id from cbgpu_schools.preferences p where p.user_id=uid and cbgpu_schools.active_member(uid,p.school_id)));end if;
 if action='select_school' then if not cbgpu_schools.active_member(uid,sid) then raise exception 'ACTIVE_MEMBERSHIP_REQUIRED' using errcode='42501';end if;insert into cbgpu_schools.preferences values(uid,sid) on conflict(user_id) do update set school_id=excluded.school_id;return jsonb_build_object('ok',true,'current_school_id',sid);end if;
 if action='join' then
 select * into s from cbgpu_schools.schools where id=sid and status='APPROVED';if not found then raise exception 'SCHOOL_NOT_FOUND' using errcode='P0002';end if;
 kind:=payload->>'membership_type';if kind not in('STUDENT','TEACHER','EXTERNAL') or kind is null then raise exception 'INVALID_MEMBERSHIP_TYPE' using errcode='23514';end if;
 if kind='STUDENT' and (coalesce(length(btrim(payload->>'student_number')),0) not between 1 and 80 or(s.configuration->>'student_id_policy'='CBSH_4_DIGIT' and payload->>'student_number' !~ '^[1-3][1-3][0-9]{2}$')) then raise exception 'INVALID_STUDENT_NUMBER' using errcode='23514';end if;
 insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,student_number,department) values(uid,sid,kind,'PENDING_VERIFICATION',case when kind='STUDENT' then btrim(payload->>'student_number') end,left(payload->>'department',200)) on conflict(user_id,school_id) do nothing;
 select * into m from cbgpu_schools.memberships where user_id=uid and school_id=sid;return jsonb_build_object('membership',cbgpu_schools.member_json(m));end if;
 if action='draft_create' then perform pg_advisory_xact_lock(hashtextextended('school-application:'||uid::text,0));if(select count(*) from cbgpu_schools.applications where applicant_id=uid and created_at>now()-interval '1 day')>=20 then raise exception 'APPLICATION_RATE_LIMIT' using errcode='P0429';end if;insert into cbgpu_schools.applications(applicant_id,data) values(uid,cbgpu_schools.default_draft()) returning * into r;return jsonb_build_object('application',to_jsonb(r)-'applicant_id'-'reviewed_by');end if;
 if action='applications' then return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(t)-'applicant_id'-'reviewed_by' order by t.created_at desc),'[]') from cbgpu_schools.applications t where t.applicant_id=uid));end if;
 if action in('draft_get','draft_save','draft_submit','platform_review') then
 select * into r from cbgpu_schools.applications where id=(payload->>'id')::uuid and (applicant_id=uid or(action='platform_review' and platform)) for update;
 if not found then raise exception 'APPLICATION_NOT_FOUND' using errcode='P0002';end if;
 if action='draft_get' then return jsonb_build_object('application',to_jsonb(r)-'applicant_id'-'reviewed_by');end if;
 if (payload->>'revision')::integer is distinct from r.revision then return jsonb_build_object('ok',false,'code','REVISION_CONFLICT','error','This application changed. Reload before saving.');end if;
 before_value:=to_jsonb(r);
 if action='draft_save' then
 if r.status not in('DRAFT','REJECTED') then raise exception 'APPLICATION_NOT_EDITABLE' using errcode='23514';end if;
 d:=payload->'data';if jsonb_typeof(d)<>'object' or pg_column_size(d)>50000 or not(d ?& array['school','administrator','configuration','features','gpu','confirmed']) or coalesce((payload->>'step')::integer,0) not between 1 and 6 then raise exception 'INVALID_DRAFT' using errcode='23514';end if;
 update cbgpu_schools.applications set data=d,step=(payload->>'step')::integer,status='DRAFT',revision=revision+1,updated_at=now() where id=r.id returning * into r;
 elsif action='draft_submit' then
 if r.status<>'DRAFT' or payload->'confirmed' is distinct from 'true'::jsonb or not coalesce(cbgpu_schools.validate_draft(r.data),false) or not cbgpu_schools.validate_configuration(r.data->'configuration') or not cbgpu_schools.features_allowed('school-'||r.id::text,array(select jsonb_array_elements_text(r.data->'features'))) then raise exception 'APPLICATION_INCOMPLETE' using errcode='23514';end if;
 perform pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended('cbgpu:school:'||lower(btrim(r.data#>>'{school,official_name}')),0));
 select * into s from cbgpu_schools.schools where (normalized_name=lower(btrim(r.data#>>'{school,official_name}')) and region=btrim(r.data#>>'{school,region}') and address=btrim(r.data#>>'{school,address}')) or (nullif(btrim(r.data#>>'{school,official_id}'),'') is not null and official_id=btrim(r.data#>>'{school,official_id}'));
 if found and s.status='APPROVED' then return jsonb_build_object('ok',false,'code','DUPLICATE_SCHOOL','error','This school already exists. Request membership instead.','existing_school_id',s.id);end if;
 if exists(select 1 from cbgpu_schools.applications a where a.id<>r.id and a.status='PENDING' and((lower(btrim(a.data#>>'{school,official_name}'))=lower(btrim(r.data#>>'{school,official_name}')) and a.data#>>'{school,region}'=r.data#>>'{school,region}' and a.data#>>'{school,address}'=r.data#>>'{school,address}') or(nullif(r.data#>>'{school,official_id}','') is not null and a.data#>>'{school,official_id}'=r.data#>>'{school,official_id}'))) then return jsonb_build_object('ok',false,'code','DUPLICATE_APPLICATION','error','This school already has an application pending review.');end if;
 update cbgpu_schools.applications set status='PENDING',school_id=s.id,step=6,revision=revision+1,updated_at=now() where id=r.id returning * into r;
 else
 if not platform or r.applicant_id=uid then raise exception 'INDEPENDENT_PLATFORM_REVIEW_REQUIRED' using errcode='42501';end if;
 decision:=payload->>'decision';if r.status<>'PENDING' or decision not in('APPROVE','REJECT') or coalesce(length(btrim(payload->>'note')),0) not between 1 and 2000 then raise exception 'INVALID_REVIEW' using errcode='23514';end if;
 if decision='APPROVE' then
 if not coalesce(cbgpu_schools.validate_draft(r.data),false) or not cbgpu_schools.validate_configuration(r.data->'configuration') or not cbgpu_schools.features_allowed('school-'||r.id::text,array(select jsonb_array_elements_text(r.data->'features'))) then raise exception 'APPLICATION_INCOMPLETE' using errcode='23514';end if;
 sid:=coalesce(r.school_id,'school-'||r.id::text);
 if r.school_id is not null then select * into s from cbgpu_schools.schools where id=r.school_id for update;if not found or s.status not in('LISTED','UNCLAIMED') then raise exception 'SCHOOL_ALREADY_CLAIMED' using errcode='23505';end if;
 update cbgpu_schools.schools set display_name=btrim(r.data#>>'{configuration,display_name}'),contact_email=r.data#>>'{school,contact_email}',contact_phone=r.data#>>'{school,contact_phone}',brand_color=r.data#>>'{configuration,brand_color}',configuration=jsonb_set(r.data->'configuration','{ads_enabled}','false'::jsonb),features=array(select jsonb_array_elements_text(r.data->'features')),status='APPROVED',verified_at=now(),revision=revision+1 where id=sid returning * into s;
 else
 insert into cbgpu_schools.schools(id,official_name,normalized_name,official_id,display_name,type,region,address,contact_email,contact_phone,brand_color,configuration,features,verified_at)
 values(sid,btrim(r.data#>>'{school,official_name}'),lower(btrim(r.data#>>'{school,official_name}')),nullif(btrim(r.data#>>'{school,official_id}'),''),btrim(r.data#>>'{configuration,display_name}'),r.data#>>'{school,type}',r.data#>>'{school,region}',r.data#>>'{school,address}',r.data#>>'{school,contact_email}',r.data#>>'{school,contact_phone}',r.data#>>'{configuration,brand_color}',jsonb_set(r.data->'configuration','{ads_enabled}','false'::jsonb),array(select jsonb_array_elements_text(r.data->'features')),now()) returning * into s;
 end if;
 insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,school_roles,verified_by,verified_at) values(r.applicant_id,sid,'EXTERNAL','ACTIVE',true,array['SCHOOL_OWNER','SCHOOL_ADMIN'],uid,now());
 end if;
 update cbgpu_schools.applications set status=case when decision='APPROVE' then 'APPROVED' else 'REJECTED' end,school_id=sid,reviewed_by=uid,review_note=btrim(payload->>'note'),revision=revision+1,updated_at=now() where id=r.id returning * into r;
 end if;
 insert into cbgpu_schools.audit(actor_id,school_id,action,target_id,before_data,after_data) values(uid,r.school_id,action,r.id,before_value,to_jsonb(r));return jsonb_build_object('application',to_jsonb(r)-'applicant_id'-'reviewed_by','school',case when s.id is not null then cbgpu_schools.summary(s) end);
 end if;
 if action='platform_applications' then if not platform then raise exception 'PLATFORM_ADMIN_REQUIRED' using errcode='42501';end if;return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(t) order by t.created_at),'[]') from cbgpu_schools.applications t where t.status=coalesce(payload->>'status','PENDING')));end if;
 if action in('configuration','configure','members','review_member') then
 if not platform and not cbgpu_schools.has_role(uid,sid,array['SCHOOL_OWNER','SCHOOL_ADMIN']) then raise exception 'SCHOOL_ADMIN_REQUIRED' using errcode='42501';end if;
 select * into s from cbgpu_schools.schools where id=sid for update;if not found then raise exception 'SCHOOL_NOT_FOUND' using errcode='P0002';end if;
 owner_permission:=platform or cbgpu_schools.has_role(uid,sid,array['SCHOOL_OWNER']);
 if action='members' then return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.member_json(t)),'[]') from cbgpu_schools.memberships t where t.school_id=sid and (payload->>'status' is null or t.status=payload->>'status') and(q='' or exists(select 1 from public.profiles p where p.id=t.user_id and p.full_name ilike '%'||q||'%'))),'total',(select count(*) from cbgpu_schools.memberships t where t.school_id=sid));end if;
 if action='configure' then
 if (payload->>'revision')::integer is distinct from s.revision then return jsonb_build_object('ok',false,'code','REVISION_CONFLICT','error','School settings changed. Reload first.');end if;
 d:=jsonb_build_object('school',jsonb_build_object('official_name',s.official_name,'region',s.region,'address',s.address,'contact_email',s.contact_email),'administrator',jsonb_build_object('name','admin','relationship','operator','contact_email',s.contact_email,'role','STAFF'),'configuration',coalesce(payload->'configuration',s.configuration||jsonb_build_object('display_name',s.display_name,'brand_color',s.brand_color)),'features',coalesce(payload->'features',to_jsonb(s.features)),'gpu',jsonb_build_object('choice','CONNECT_LATER'));
 if not cbgpu_schools.validate_configuration(d->'configuration') or (sid='cbsh' and d#>>'{configuration,student_id_policy}'<>'CBSH_4_DIGIT') or not cbgpu_schools.features_allowed(sid,array(select jsonb_array_elements_text(d->'features'))) then raise exception 'INVALID_CONFIGURATION' using errcode='23514';end if;
 update cbgpu_schools.schools set configuration=d->'configuration',display_name=d#>>'{configuration,display_name}',brand_color=d#>>'{configuration,brand_color}',features=array(select jsonb_array_elements_text(d->'features')),revision=revision+1 where id=sid returning * into s;
 elsif action='review_member' then
 select * into m from cbgpu_schools.memberships where school_id=sid and id=(payload->>'id')::uuid for update;if not found then raise exception 'MEMBERSHIP_NOT_FOUND' using errcode='P0002';end if;before_value:=to_jsonb(m);decision:=payload->>'action';
 if m.user_id=uid then raise exception 'INDEPENDENT_MEMBERSHIP_REVIEW_REQUIRED' using errcode='42501';end if;
 if decision='SET_ROLES' then roles:=array(select jsonb_array_elements_text(payload->'school_roles'));if not(roles <@ array['STUDENT','TEACHER','SCHOOL_ADMIN','SCHOOL_OWNER','COMMUNITY_MODERATOR','GPU_ADMIN']) or (not owner_permission and ('SCHOOL_OWNER'=any(roles) or 'SCHOOL_OWNER'=any(m.school_roles))) then raise exception 'ROLE_GRANT_DENIED' using errcode='42501';end if;
 if 'SCHOOL_OWNER'=any(m.school_roles) and not('SCHOOL_OWNER'=any(roles)) and (select count(*) from cbgpu_schools.memberships where school_id=sid and status='ACTIVE' and verified and 'SCHOOL_OWNER'=any(school_roles))<=1 then raise exception 'LAST_SCHOOL_OWNER_REQUIRED' using errcode='23514';end if;
 update cbgpu_schools.memberships set school_roles=roles where id=m.id returning * into m;
 elsif decision in('APPROVE','RESTORE','REJECT','REVOKE','SUSPEND') then
 if 'SCHOOL_OWNER'=any(m.school_roles) and (not owner_permission or(decision in('REJECT','REVOKE','SUSPEND') and (select count(*) from cbgpu_schools.memberships where school_id=sid and status='ACTIVE' and verified and 'SCHOOL_OWNER'=any(school_roles))<=1)) then raise exception 'LAST_SCHOOL_OWNER_REQUIRED' using errcode='42501';end if;
 update cbgpu_schools.memberships set status=case when decision in('APPROVE','RESTORE') then 'ACTIVE' when decision='SUSPEND' then 'SUSPENDED' else 'PENDING_VERIFICATION' end,verified=decision in('APPROVE','RESTORE'),verified_by=uid,verified_at=case when decision in('APPROVE','RESTORE') then now() end,verification_note=left(payload->>'verification_note',2000) where id=m.id returning * into m;
 else raise exception 'INVALID_MEMBERSHIP_ACTION' using errcode='23514';end if;
 insert into cbgpu_schools.audit(actor_id,school_id,action,target_id,before_data,after_data) values(uid,sid,decision,m.id,before_value,to_jsonb(m));return jsonb_build_object('membership',cbgpu_schools.member_json(m));
 end if;
 return jsonb_build_object('school',cbgpu_schools.summary(s),'configuration',s.configuration||jsonb_build_object('display_name',s.display_name,'brand_color',s.brand_color),'features',s.features,'revision',s.revision,'can_manage_roles',owner_permission,'available_features',(select coalesce(jsonb_agg(f.key order by f.key),'[]') from cbgpu_schools.feature_registry f where f.enabled and(f.globally_available or sid=any(f.allowed_school_ids))));
 end if;
 raise exception 'UNKNOWN_SCHOOL_ACTION' using errcode='23514';
end$$;
revoke all on all functions in schema cbgpu_schools from public,anon,authenticated;
revoke all on function public.cbgpu_school_api(text,jsonb) from public;
grant execute on function public.cbgpu_school_api(text,jsonb) to anon,authenticated;
-- Keep the existing CBsh administrator/rollover workflows synchronized. This is
-- one-way: approving another school's membership never grants global privileges.
create or replace function cbgpu_schools.sync_primary_affiliation() returns trigger language plpgsql security definer set search_path='' as $$begin
 if new.school_id='cbsh' then
 update cbgpu_schools.memberships set membership_type=new.account_type,status=new.account_status,verified=new.affiliation_verified,student_number=new.student_id,student_number_school_year=new.student_id_school_year,department=new.department,verified_at=new.verified_at where user_id=new.id and school_id='cbsh';
 end if;return new;end$$;
drop trigger if exists z_cbgpu_school_profile_sync on public.profiles;
create trigger z_cbgpu_school_profile_sync after update of account_type,account_status,school_id,affiliation_verified,student_id,student_id_school_year,department,verified_at on public.profiles for each row execute function cbgpu_schools.sync_primary_affiliation();
-- Resource permission is school-specific and is independent of project ownership.
create or replace function cbgpu_accounts.can_user_use_resource(subject_id uuid,resource text,target_school text default 'cbsh') returns boolean language sql stable security definer set search_path='' as $$
select exists(select 1 from cbgpu_schools.schools s join public.profiles p on p.id=subject_id join auth.users u on u.id=p.id where s.id=target_school and s.status='APPROVED' and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now())
 and case resource when 'study' then s.id='cbsh' and 'school_life'=any(s.features) when 'gpu' then 'gpu_compute'=any(s.features) when 'school_compute' then 'projects'=any(s.features) when 'fabrication' then 'manufacturing'=any(s.features) when 'school_storage' then 'materials'=any(s.features) when 'contests' then 'contests'=any(s.features) else false end
 -- No route/daemon exists for a second school's execution yet. Membership alone
 -- must never borrow legacy CBsh GPU/CPU/manufacturing hardware.
 and(resource not in('gpu','school_compute','fabrication') or s.id='cbsh')
 and(exists(select 1 from cbgpu_schools.memberships m where m.user_id=subject_id and m.school_id=s.id and m.membership_type in('STUDENT','TEACHER') and cbgpu_schools.active_member(subject_id,s.id))
 or exists(select 1 from cbgpu_accounts.guest_requests g where g.external_user_id=subject_id and g.school_id=s.id and g.status='APPROVED' and g.starts_at<=now() and g.ends_at>now() and resource=any(g.resources) and cbgpu_schools.active_member(g.requester_id,s.id) and exists(select 1 from cbgpu_schools.memberships sponsor where sponsor.user_id=g.requester_id and sponsor.school_id=s.id and sponsor.membership_type in('STUDENT','TEACHER')))))
$$;
revoke all on all functions in schema cbgpu_schools from public,anon,authenticated;

create or replace function cbgpu_schools.guard_student_claim() returns trigger language plpgsql security definer set search_path='' as $$begin
 if new.membership_type='STUDENT' and new.status='ACTIVE' and new.verified and new.student_number is not null and(tg_op='INSERT' or(new.student_number,new.student_number_school_year,new.status,new.verified) is distinct from(old.student_number,old.student_number_school_year,old.status,old.verified)) then
 new.student_number_school_year:=coalesce(new.student_number_school_year,extract(year from now())::integer);
 perform pg_advisory_xact_lock(hashtextextended('school-number:'||new.school_id||':'||new.student_number_school_year||':'||new.student_number,0));
 if exists(select 1 from cbgpu_schools.memberships m where m.school_id=new.school_id and m.student_number=new.student_number and m.student_number_school_year=new.student_number_school_year and m.user_id<>new.user_id and m.status='ACTIVE' and m.verified) then raise exception 'STUDENT_NUMBER_ALREADY_REGISTERED' using errcode='23505';end if;
 end if;return new;end$$;
drop trigger if exists school_student_claim on cbgpu_schools.memberships;
create trigger school_student_claim before insert or update on cbgpu_schools.memberships for each row execute function cbgpu_schools.guard_student_claim();
revoke all on function cbgpu_schools.guard_student_claim() from public,anon,authenticated;
-- A narrow offline importer can add official unclaimed directory entries only.
alter table cbgpu_schools.schools add column if not exists education_office_code text,add column if not exists source_as_of date,add column if not exists source_sha256 text,add column if not exists source_url text;
create table if not exists cbgpu_schools.directory_imports(source_sha256 text primary key,source_as_of date not null,school_count integer not null,imported_at timestamptz not null default now());
revoke all on cbgpu_schools.directory_imports from public,anon,authenticated;
do $$begin if not exists(select 1 from pg_roles where rolname='cbgpu_school_directory_importer') then create role cbgpu_school_directory_importer nologin nosuperuser nobypassrls;end if;end$$;
create or replace function cbgpu_schools.import_official_directory(manifest jsonb) returns jsonb language plpgsql security definer set search_path='' as $$declare item jsonb;seen integer:=0;begin
 if coalesce(manifest->>'format_version'<>'1' or manifest->>'scope'<>'KOREAN_HIGH_SCHOOLS' or manifest->>'source_sha256' !~ '^[0-9a-f]{64}$' or manifest->>'source' !~ '^https://open\.neis\.go\.kr/portal/data/file/downloadBlobFileData\.do\?' or jsonb_typeof(manifest->'schools')<>'array' or jsonb_array_length(manifest->'schools')<>(manifest->>'school_count')::integer ,true) then raise exception 'INVALID_OFFICIAL_MANIFEST';end if;
 if exists(select 1 from jsonb_array_elements(manifest->'schools') x group by x->>'official_id' having count(*)>1) then raise exception 'DUPLICATE_OFFICIAL_IDENTIFIER';end if;
 for item in select * from jsonb_array_elements(manifest->'schools') loop
 if coalesce(item->>'official_id' !~ '^[A-Z0-9]{7}$' or item->>'type'<>'HIGH_SCHOOL' or item->>'status'<>'LISTED' or item->'features'<>'[]'::jsonb or item->'verified_at'<>'null'::jsonb or coalesce(length(btrim(item->>'official_name')),0) not between 2 and 150 ,true) then raise exception 'INVALID_OFFICIAL_SCHOOL';end if;
 insert into cbgpu_schools.schools(id,official_name,normalized_name,official_id,display_name,type,region,address,contact_phone,configuration,features,status,education_office_code,source_as_of,source_sha256,source_url)
 values('neis-'||(item->>'official_id'),item->>'official_name',lower(btrim(item->>'official_name')),item->>'official_id',item->>'display_name','HIGH_SCHOOL',item->>'region',item->>'address',item->>'contact_phone',cbgpu_schools.default_draft()->'configuration','{}','LISTED',item->>'education_office_code',(manifest->>'source_as_of')::date,manifest->>'source_sha256',manifest->>'source')
 on conflict(official_id) do update set official_name=excluded.official_name,normalized_name=excluded.normalized_name,display_name=excluded.display_name,region=excluded.region,address=excluded.address,contact_phone=excluded.contact_phone,education_office_code=excluded.education_office_code,source_as_of=excluded.source_as_of,source_sha256=excluded.source_sha256,source_url=excluded.source_url where cbgpu_schools.schools.status in('LISTED','UNCLAIMED');seen:=seen+1;
 end loop;
 insert into cbgpu_schools.directory_imports(source_sha256,source_as_of,school_count) values(manifest->>'source_sha256',(manifest->>'source_as_of')::date,seen) on conflict(source_sha256) do nothing;
 return jsonb_build_object('ok',true,'school_count',seen,'source_sha256',manifest->>'source_sha256');
end$$;
revoke all on function cbgpu_schools.import_official_directory(jsonb) from public,anon,authenticated;
grant usage on schema cbgpu_schools to cbgpu_school_directory_importer;grant execute on function cbgpu_schools.import_official_directory(jsonb) to cbgpu_school_directory_importer;
commit;
