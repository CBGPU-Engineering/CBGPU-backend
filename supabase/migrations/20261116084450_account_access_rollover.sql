-- CANDIDATE ONLY. Requires verified backup and explicit deployment approval.
-- Additive: IDs, Auth credentials/sessions, ownership and memberships are never rewritten.
begin;
create schema if not exists cbgpu_accounts;
revoke all on schema cbgpu_accounts from public,anon,authenticated;
alter table public.profiles
 add column if not exists account_type text,
 add column if not exists account_status text,
 add column if not exists school_id text,
 add column if not exists affiliation_verified boolean,
 add column if not exists verified_at timestamptz,
 add column if not exists department text,
 add column if not exists organization text,
 add column if not exists required_school_year integer,
 add column if not exists required_grade integer;
-- Only trusted application metadata may identify a legacy teacher. All other
-- pre-existing profiles remain school students, with original approval unchanged.
update public.profiles p set
 account_type=case when u.raw_app_meta_data->>'cbgpu_account_type'='TEACHER' then 'TEACHER' else 'STUDENT' end,
 account_status=case when p.is_approved is false then 'PENDING_VERIFICATION' else 'ACTIVE' end,
 school_id='cbsh', affiliation_verified=p.is_approved is distinct from false,
 verified_at=case when p.is_approved is distinct from false then now() end
from auth.users u where u.id=p.id and p.account_type is null;
alter table public.profiles alter column account_type set default 'STUDENT', alter column account_status set default 'PENDING_VERIFICATION', alter column affiliation_verified set default false;
do $$begin
 if not exists(select 1 from pg_constraint where conname='profiles_account_type_v2') then
 alter table public.profiles add constraint profiles_account_type_v2 check(account_type in('STUDENT','TEACHER','EXTERNAL'));
 alter table public.profiles add constraint profiles_account_status_v2 check(account_status in('ACTIVE','PENDING_VERIFICATION','REVERIFICATION_REQUIRED','SUSPENDED'));
 end if;
end$$;
create table if not exists cbgpu_accounts.guest_requests(
 id uuid primary key default gen_random_uuid(),external_user_id uuid not null references public.profiles(id),requester_id uuid not null references public.profiles(id),
 school_id text not null,project_name text not null check(length(project_name) between 1 and 200),purpose text not null check(length(purpose) between 1 and 4000),
 resources text[] not null check(cardinality(resources)>0 and resources <@ array['gpu','study','school_storage','fabrication','school_compute','contests']::text[]),
 starts_at timestamptz not null,ends_at timestamptz not null, status text not null default 'PENDING' check(status in('PENDING','APPROVED','REJECTED','REVOKED','EXPIRED')),
 reviewed_by uuid references public.profiles(id),reviewed_at timestamptz,created_at timestamptz not null default now(),check(ends_at>starts_at),check(requester_id<>external_user_id));
create index if not exists guest_requests_validity on cbgpu_accounts.guest_requests(external_user_id,school_id,ends_at) where status='APPROVED';
create table if not exists cbgpu_accounts.rollovers(
 id uuid primary key default gen_random_uuid(),school_id text not null,school_year integer not null check(school_year between 2020 and 2200),actor_id uuid not null references public.profiles(id),executed_at timestamptz not null default now(),result jsonb not null, unique(school_id,school_year));
create table if not exists cbgpu_accounts.rollover_failures(id uuid primary key default gen_random_uuid(),school_id text not null,school_year integer not null,actor_id uuid not null references public.profiles(id),executed_at timestamptz not null default now(),error text not null);
create table if not exists cbgpu_accounts.audit(
 id bigint generated always as identity primary key,actor_id uuid,event text not null,target_id uuid,before_value jsonb,after_value jsonb,created_at timestamptz not null default now());
alter table cbgpu_accounts.guest_requests enable row level security;
alter table cbgpu_accounts.rollovers enable row level security;
alter table cbgpu_accounts.audit enable row level security;
alter table cbgpu_accounts.rollover_failures enable row level security;
revoke all on all tables in schema cbgpu_accounts from public,anon,authenticated;
-- Fresh, database-backed permission checks. Never trust stale JWT role metadata.
create or replace function cbgpu_accounts.can_user_use_resource(subject_id uuid,resource text,target_school text default 'cbsh') returns boolean
language sql stable security definer set search_path='' as $$
 select exists(select 1 from public.profiles p join auth.users u on u.id=p.id where p.id=subject_id
 and u.deleted_at is null and (u.banned_until is null or u.banned_until<=now())
 and p.account_status='ACTIVE' and resource=any(array['gpu','study','school_storage','fabrication','school_compute','contests']) and (
 (p.account_type in('STUDENT','TEACHER') and p.school_id=target_school and p.affiliation_verified and p.is_approved is distinct from false)
 or (p.account_type='EXTERNAL' and exists(select 1 from cbgpu_accounts.guest_requests g join public.profiles sponsor on sponsor.id=g.requester_id
 join auth.users su on su.id=sponsor.id where g.external_user_id=p.id and g.school_id=target_school and g.status='APPROVED'
 and g.starts_at<=now() and g.ends_at>now() and resource=any(g.resources)
 and sponsor.account_status='ACTIVE' and sponsor.affiliation_verified and sponsor.is_approved is distinct from false
 and sponsor.account_type in('STUDENT','TEACHER') and sponsor.school_id=g.school_id
 and su.deleted_at is null and (su.banned_until is null or su.banned_until<=now())))))
$$;
revoke all on function cbgpu_accounts.can_user_use_resource(uuid,text,text) from public,anon,authenticated;
-- A still-signed JWT must not retain permissions after its server session is revoked.
create or replace function cbgpu_accounts.current_session_valid() returns boolean
language sql stable security definer set search_path='' as $$
 select exists(select 1 from auth.sessions s where s.user_id=auth.uid()
 and s.id=case when (auth.jwt()->>'session_id') ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' then (auth.jwt()->>'session_id')::uuid end
 and(s.not_after is null or s.not_after>now()))
$$;
revoke all on function cbgpu_accounts.current_session_valid() from public,anon,authenticated;
create or replace function public.cbgpu_can_use_resource(resource text,target_school text default 'cbsh') returns boolean
language sql stable security definer set search_path='' as $$select cbgpu_accounts.current_session_valid() and cbgpu_accounts.can_user_use_resource(auth.uid(),resource,target_school)$$;
-- Only trusted server workers can ask on behalf of an API-key/project owner.
-- Callers with an ordinary bearer cannot choose another identity.
create or replace function public.cbgpu_service_can_use_resource(user_id uuid,resource text,target_school text default 'cbsh') returns boolean
language sql stable security definer set search_path='' as $$select cbgpu_accounts.can_user_use_resource(user_id,resource,target_school)$$;
revoke all on function public.cbgpu_service_can_use_resource(uuid,text,text) from public,anon,authenticated;
grant execute on function public.cbgpu_service_can_use_resource(uuid,text,text) to service_role;
revoke all on function public.cbgpu_can_use_resource(text,text) from public,anon;
grant execute on function public.cbgpu_can_use_resource(text,text) to authenticated;
-- New fields are guarded even under legacy permissive profile UPDATE policies.
-- SECURITY INVOKER deliberately observes the caller of trusted mutation functions.
create or replace function cbgpu_accounts.guard_fields() returns trigger language plpgsql set search_path='' as $$
begin
 if current_user in('postgres','supabase_admin') or auth.role()='service_role' then return new; end if;
 if tg_op='UPDATE' and (new.student_id,new.student_id_school_year) is distinct from (old.student_id,old.student_id_school_year) then
 raise exception 'STUDENT_NUMBER_REQUIRES_VERIFIED_WORKFLOW' using errcode='42501';end if;
 if tg_op='UPDATE' and (new.account_type,new.account_status,new.school_id,new.affiliation_verified,new.verified_at,new.required_school_year,new.required_grade)
 is distinct from (old.account_type,old.account_status,old.school_id,old.affiliation_verified,old.verified_at,old.required_school_year,old.required_grade) then
 raise exception 'ACCOUNT_FIELDS_REQUIRE_VERIFIED_WORKFLOW' using errcode='42501'; end if;
 if tg_op='INSERT' then raise exception 'ACCOUNT_CREATION_REQUIRES_AUTH' using errcode='42501'; end if;
 return new;
end$$;
drop trigger if exists guard_account_fields_v2 on public.profiles;
create trigger guard_account_fields_v2 before insert or update on public.profiles for each row execute function cbgpu_accounts.guard_fields();
-- Preserve the existing signup trigger, but allow teacher/external accounts without
-- student IDs. User-editable type is a REQUEST only: all school affiliations start pending.
create or replace function public.handle_new_user() returns trigger language plpgsql security definer set search_path='' as $$
declare kind text:=coalesce(new.raw_user_meta_data->>'account_type_request','STUDENT');school text;
begin
 if coalesce(length(btrim(new.raw_user_meta_data->>'full_name')),0) not between 1 and 100 then raise exception 'FULL_NAME_REQUIRED' using errcode='23514';end if;
 if kind not in('STUDENT','TEACHER','EXTERNAL') then raise exception 'INVALID_ACCOUNT_TYPE' using errcode='23514';end if;
 school:=case when kind='EXTERNAL' then null else coalesce(new.raw_user_meta_data->>'school_id',case when new.raw_user_meta_data->>'account_type_request' is null then 'cbsh' end) end;
 if kind<>'EXTERNAL' and school is distinct from 'cbsh' then raise exception 'INVALID_SCHOOL' using errcode='23514';end if;
 insert into public.profiles(id,full_name,student_id,is_admin,is_approved,account_type,account_status,school_id,affiliation_verified,department,organization)
 values(new.id,new.raw_user_meta_data->>'full_name',case when kind='STUDENT' then new.raw_user_meta_data->>'student_id' end,false,false,kind,
 case when kind='EXTERNAL' then 'ACTIVE' else 'PENDING_VERIFICATION' end,school,false,left(new.raw_user_meta_data->>'department',200),left(new.raw_user_meta_data->>'organization',200)) on conflict(id) do nothing;
 return new;
end$$;
-- Existing number format and claim serialization remain; teacher/external have no number.
create or replace function public.guard_student_id_claim() returns trigger language plpgsql security definer set search_path='' as $$
declare normalized text:=nullif(btrim(new.student_id),''); y integer;
begin
 if normalized is null then
 if new.account_type='STUDENT' and new.account_status<>'REVERIFICATION_REQUIRED' and tg_op='INSERT' then raise exception 'STUDENT_ID_REQUIRED' using errcode='23514';end if;
 if tg_op='UPDATE' and old.student_id is not null and auth.role() is distinct from 'service_role' and session_user not in('postgres','supabase_admin') and not public.is_admin(auth.uid()) then raise exception 'STUDENT_ID_REQUIRED' using errcode='23514';end if;
 new.student_id:=null;new.student_id_school_year:=null;return new;end if;
 if normalized !~ '^[1-3][1-3][0-9]{2}$' then raise exception 'INVALID_STUDENT_ID' using errcode='23514';end if;
 if tg_op='UPDATE' and normalized is not distinct from btrim(old.student_id) then new.student_id:=normalized;new.student_id_school_year:=old.student_id_school_year;return new;end if;
 y:=coalesce(new.required_school_year,greatest(public.cbgpu_school_year(now()),(select max(r.school_year) from cbgpu_accounts.rollovers r where r.school_id=new.school_id)));
 perform pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended('cbgpu:student_id:'||y||':'||normalized,0));
 if exists(select 1 from public.profiles p where p.student_id_school_year=y and btrim(p.student_id)=normalized and p.id<>new.id) then raise exception 'STUDENT_ID_ALREADY_REGISTERED' using errcode='23505';end if;
 new.student_id:=normalized;new.student_id_school_year:=y;return new;
end$$;
create or replace function cbgpu_accounts.preview(school text,yr integer) returns jsonb language sql stable security definer set search_path='' as $$
 with targets as(select p.id,p.full_name,p.student_id,p.account_type,
 case when p.student_id ~ '^[1-3][1-3][0-9]{2}$' then left(p.student_id,1)::integer end grade,
 case when p.account_type<>'STUDENT' then 'UNCHANGED' when p.student_id ~ '^[12][1-3][0-9]{2}$' then 'REVERIFY' when p.student_id ~ '^3[1-3][0-9]{2}$' then 'EXTERNAL' else 'INVALID' end action
 from public.profiles p where p.school_id=school), aggregate as(select coalesce(jsonb_agg(to_jsonb(targets) order by id),'[]'::jsonb) items from targets)
 select jsonb_build_object('school_id',school,'school_year',yr,'confirmation','ROLLOVER '||school||' '||yr,
 'preview_token',md5(school||':'||yr||':'||items::text),'items',items,'blocked',exists(select 1 from targets where action='INVALID'),
 'counts',jsonb_build_object('grade1',(select count(*) from targets where grade=1 and account_type='STUDENT'),'grade2',(select count(*) from targets where grade=2 and account_type='STUDENT'),'grade3',(select count(*) from targets where grade=3 and account_type='STUDENT'),'invalid',(select count(*) from targets where action='INVALID'),'unchanged',(select count(*) from targets where action='UNCHANGED'))) from aggregate
$$;
revoke all on function cbgpu_accounts.preview(text,integer) from public,anon,authenticated;
create or replace function public.cbgpu_account_api(action text,payload jsonb default '{}'::jsonb) returns jsonb
language plpgsql security definer set search_path='' as $$
declare actor public.profiles; target public.profiles; uid uuid:=auth.uid();rid uuid;result jsonb;before_row jsonb;pre jsonb;school text;yr integer;status text;scopes text[];g cbgpu_accounts.guest_requests;
begin
 if uid is null or not cbgpu_accounts.current_session_valid() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 select p.* into actor from public.profiles p join auth.users u on u.id=p.id where p.id=uid and u.deleted_at is null and (u.banned_until is null or u.banned_until<=now());
 if not found then raise exception 'ACCOUNT_NOT_FOUND' using errcode='42501';end if;
 if action='self' then return jsonb_build_object('account',to_jsonb(actor),'resources',(select coalesce(jsonb_agg(r),'[]'::jsonb) from unnest(array['gpu','study','school_storage','fabrication','school_compute','contests']) r where public.cbgpu_can_use_resource(r)));end if;
 if actor.account_status='SUSPENDED' then raise exception 'ACCOUNT_SUSPENDED' using errcode='42501';end if;
 if action='reverify' then
 if actor.account_type<>'STUDENT' or actor.account_status<>'REVERIFICATION_REQUIRED' or actor.required_school_year is null then raise exception 'REVERIFICATION_NOT_REQUIRED' using errcode='23514';end if;
 if (payload->>'school_year')::integer is distinct from actor.required_school_year then raise exception 'INVALID_SCHOOL_YEAR' using errcode='23514';end if;
 if left(payload->>'student_id',1) is distinct from actor.required_grade::text then raise exception 'INVALID_REVERIFICATION_GRADE' using errcode='23514';end if;
 -- A number is not proof of identity: explicit school verification is still required.
 update public.profiles set student_id=payload->>'student_id',account_status='PENDING_VERIFICATION',affiliation_verified=false where id=uid;
 return jsonb_build_object('ok',true,'pending_verification',true);end if;
 if action='guest_create' then
 if actor.account_type not in('STUDENT','TEACHER') or not public.cbgpu_can_use_resource('gpu',actor.school_id) then raise exception 'VERIFIED_SCHOOL_SPONSOR_REQUIRED' using errcode='42501';end if;
 if nullif(payload->>'external_email','') is not null then
 select p.* into target from public.profiles p join auth.users u on u.id=p.id where lower(u.email)=lower(btrim(payload->>'external_email'));
 else select * into target from public.profiles where id=(payload->>'external_user_id')::uuid;end if;
 if target.account_type is distinct from 'EXTERNAL' or target.account_status is distinct from 'ACTIVE' then raise exception 'ACTIVE_EXTERNAL_ACCOUNT_REQUIRED' using errcode='23514';end if;
 select array_agg(v) into scopes from jsonb_array_elements_text(payload->'resources') v;
 insert into cbgpu_accounts.guest_requests(external_user_id,requester_id,school_id,project_name,purpose,resources,starts_at,ends_at)
 values(target.id,uid,actor.school_id,payload->>'project_name',payload->>'purpose',scopes,(payload->>'starts_at')::timestamptz,(payload->>'ends_at')::timestamptz) returning id into rid;
 return jsonb_build_object('ok',true,'id',rid);end if;
 if action='guest_list' then return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(x)||jsonb_build_object('status',case when x.status='APPROVED' and x.ends_at<=now() then 'EXPIRED' else x.status end) order by x.created_at desc),'[]'::jsonb) from cbgpu_accounts.guest_requests x where x.external_user_id=uid or x.requester_id=uid));end if;
 if actor.is_admin is distinct from true or actor.account_status is distinct from 'ACTIVE' then raise exception 'ADMIN_REQUIRED' using errcode='42501';end if;
 if action='accounts' then return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(x)||jsonb_build_object('email',u.email) order by x.full_name),'[]'::jsonb) from public.profiles x join auth.users u on u.id=x.id));end if;
 if action='account_update' then
 select * into target from public.profiles where id=(payload->>'id')::uuid for update;
 if not found then raise exception 'ACCOUNT_NOT_FOUND' using errcode='23514';end if;
 before_row:=to_jsonb(target);
 status:=coalesce(payload->>'account_status',target.account_status);
 if status not in('ACTIVE','PENDING_VERIFICATION','REVERIFICATION_REQUIRED','SUSPENDED') then raise exception 'INVALID_ACCOUNT_STATUS' using errcode='23514';end if;
 if payload->>'approve_affiliation'='true' then
 if target.id=uid or target.account_type='EXTERNAL' or target.school_id<>'cbsh' or (target.account_type='STUDENT' and (target.student_id is null or target.student_id !~ '^[1-3][1-3][0-9]{2}$')) then raise exception 'AFFILIATION_REVIEW_REQUIRED' using errcode='42501';end if;
 status:='ACTIVE';end if;
 update public.profiles set account_status=status,
 affiliation_verified=case when payload->>'approve_affiliation'='true' then true else affiliation_verified end,
 verified_at=case when payload->>'approve_affiliation'='true' then now() else verified_at end,
 is_approved=case when payload->>'approve_affiliation'='true' then true else is_approved end,
 required_school_year=case when payload->>'approve_affiliation'='true' then null else required_school_year end,
 required_grade=case when payload->>'approve_affiliation'='true' then null else required_grade end where id=target.id;
 insert into cbgpu_accounts.audit(actor_id,event,target_id,before_value,after_value) select uid,'ACCOUNT_REVIEW',target.id,before_row,to_jsonb(p) from public.profiles p where p.id=target.id;
 return jsonb_build_object('ok',true);end if;
 if action='guest_admin_list' then return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(x)||jsonb_build_object('status',case when x.status='APPROVED' and x.ends_at<=now() then 'EXPIRED' else x.status end) order by x.created_at desc),'[]'::jsonb) from cbgpu_accounts.guest_requests x));end if;
 if action='guest_update' then
 select * into g from cbgpu_accounts.guest_requests where id=(payload->>'id')::uuid for update;
 if not found then raise exception 'REQUEST_NOT_FOUND' using errcode='23514';end if;
 status:=payload->>'status';
 if status not in('APPROVED','REJECTED','REVOKED') or (status in('APPROVED','REJECTED') and g.status<>'PENDING') or (status='REVOKED' and g.status<>'APPROVED') then raise exception 'INVALID_TRANSITION' using errcode='23514';end if;
 if status='APPROVED' and (g.requester_id=uid or g.external_user_id=uid or g.ends_at<=now()) then raise exception 'INDEPENDENT_APPROVAL_REQUIRED' using errcode='42501';end if;
 update cbgpu_accounts.guest_requests set status=payload->>'status',reviewed_by=uid,reviewed_at=now() where id=g.id;
 insert into cbgpu_accounts.audit(actor_id,event,target_id,before_value,after_value) values(uid,'GUEST_'||status,g.external_user_id,to_jsonb(g),payload);
 return jsonb_build_object('ok',true);end if;
 if action='rollover_history' then return jsonb_build_object('items',(select coalesce(jsonb_agg(x.item order by x.executed_at desc),'[]'::jsonb) from (select r.executed_at,to_jsonb(r)||jsonb_build_object('status','SUCCEEDED') item from cbgpu_accounts.rollovers r union all select f.executed_at,to_jsonb(f)||jsonb_build_object('status','FAILED') item from cbgpu_accounts.rollover_failures f) x));end if;
 if action in('rollover_preview','rollover_execute') then
 school:=payload->>'school_id';yr:=(payload->>'school_year')::integer;
 if school is distinct from 'cbsh' or yr is null or yr not between 2020 and 2200 then raise exception 'INVALID_SCHOOL_YEAR' using errcode='23514';end if;
 perform pg_catalog.pg_advisory_xact_lock(pg_catalog.hashtextextended('cbgpu:rollover:'||school,0));
 -- Lock every target before hashing so confirmation cannot race account mutations.
 if action='rollover_execute' then
 -- Prevent phantom new registrations between confirmed preview and bulk UPDATE.
 lock table public.profiles in share row exclusive mode;
 perform 1 from public.profiles where school_id=school for update;end if;
 pre:=cbgpu_accounts.preview(school,yr);
 if action='rollover_preview' then return pre;end if;
 if exists(select 1 from cbgpu_accounts.rollovers r where r.school_id=school and r.school_year=yr) then raise exception 'ROLLOVER_ALREADY_EXECUTED' using errcode='23505';end if;
 if payload->>'confirmation' is distinct from pre->>'confirmation' or payload->>'preview_token' is distinct from pre->>'preview_token' or (pre->>'blocked')::boolean then raise exception 'ROLLOVER_CONFIRMATION_REQUIRED' using errcode='23514';end if;
 if exists(select 1 from public.profiles p where p.school_id=school and p.account_type='STUDENT' and coalesce(p.student_id_school_year,yr-1)>=yr) then raise exception 'ROLLOVER_YEAR_MUST_ADVANCE' using errcode='23514';end if;
 begin
 insert into cbgpu_accounts.audit(actor_id,event,target_id,before_value)
 select uid,'ACADEMIC_YEAR_ROLLOVER',p.id,to_jsonb(p) from public.profiles p where p.school_id=school and p.account_type='STUDENT';
 update public.profiles set account_type=case when left(student_id,1)='3' then 'EXTERNAL' else 'STUDENT' end,
 account_status=case when left(student_id,1)='3' then 'ACTIVE' else 'REVERIFICATION_REQUIRED' end,
 required_school_year=case when left(student_id,1)='3' then null else yr end,
 required_grade=case when left(student_id,1)='3' then null else left(student_id,1)::integer+1 end,
 affiliation_verified=false,verified_at=null,is_approved=false,student_id=null,student_id_school_year=null
 where school_id=school and account_type='STUDENT';
 insert into cbgpu_accounts.rollovers(school_id,school_year,actor_id,result) values(school,yr,uid,pre) returning id into rid;
 return jsonb_build_object('ok',true,'id',rid,'result',pre);
 exception when others then
 -- The inner subtransaction rolls every profile/audit mutation back before recording a failed attempt.
 insert into cbgpu_accounts.rollover_failures(school_id,school_year,actor_id,error) values(school,yr,uid,left(sqlerrm,500));
 return jsonb_build_object('ok',false,'error','ROLLOVER_FAILED','details',left(sqlerrm,500));
 end;end if;
 raise exception 'UNKNOWN_ACCOUNT_ACTION' using errcode='23514';
end$$;
revoke all on function public.cbgpu_account_api(text,jsonb) from public,anon;
grant execute on function public.cbgpu_account_api(text,jsonb) to authenticated;
-- Signup trigger entry points are never callable as general browser APIs.
revoke all on function public.handle_new_user() from public,anon,authenticated;
revoke all on function public.guard_student_id_claim() from public,anon,authenticated;

-- Candidate notebook grant: live session + school scope + exact booking owner.
-- Existing admin visibility must never imply use of another user's notebook.
create or replace function public.cbgpu_notebook_grant(p_gpu integer) returns jsonb
language plpgsql security definer set search_path='' as $$
declare subject uuid:=auth.uid();sid text:=auth.jwt()->>'session_id';until_at timestamptz;
begin
 if subject is null or p_gpu not between 1 and 4 or p_gpu is null
 or sid is null or sid !~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' then raise exception 'NOTEBOOK_SESSION_REQUIRED' using errcode='42501';end if;
 if not exists(select 1 from auth.sessions s where s.id=sid::uuid and s.user_id=subject and(s.not_after is null or s.not_after>now())) then raise exception 'NOTEBOOK_SESSION_REVOKED' using errcode='42501';end if;
 if public.cbgpu_can_use_resource('gpu') is not true then raise exception 'NOTEBOOK_RESOURCE_DENIED' using errcode='42501';end if;
 select r.end_time into until_at from public.reservations r where r.user_id=subject and r.gpu_id=p_gpu and r.status='approved' and r.start_time<=now() and r.end_time>now() order by r.end_time desc limit 1;
 if until_at is null then raise exception 'NOTEBOOK_BOOKING_REQUIRED' using errcode='42501';end if;
 return jsonb_build_object('owner',subject,'ends_at',extract(epoch from until_at));
end$$;
revoke all on function public.cbgpu_notebook_grant(integer) from public,anon;
grant execute on function public.cbgpu_notebook_grant(integer) to authenticated;
-- Pending signup deliberately writes is_approved=false. The legacy privilege
-- guard must reject granting approval, not reject explicitly withholding it.
-- Browser profile creation is still denied by guard_account_fields_v2 above.
do $$declare definition text;begin
 if to_regprocedure('public.guard_profile_privileged_columns()') is not null then
 definition:=pg_get_functiondef('public.guard_profile_privileged_columns()'::regprocedure);
 definition:=replace(definition,'new.is_approved is not null','new.is_approved is true');execute definition;
 end if;
end$$;
commit;
