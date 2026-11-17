begin;
create table if not exists cbgpu_schools.affiliation_requests(
 id uuid primary key default gen_random_uuid(),membership_id uuid not null references cbgpu_schools.memberships(id),school_id text not null references cbgpu_schools.schools(id),user_id uuid not null references auth.users(id),
 requested_type text not null check(requested_type in('STUDENT','TEACHER','EXTERNAL')),student_number text,school_year integer,name text not null,contact_email text not null,department text,reason text not null,
 status text not null check(status in('PENDING','APPROVED','REJECTED','WITHDRAWN')),revision integer not null default 1 check(revision>0),membership_fingerprint text not null,
 reviewed_by uuid references auth.users(id),review_note text,created_at timestamptz not null default clock_timestamp(),updated_at timestamptz not null default clock_timestamp());
create unique index if not exists affiliation_one_pending on cbgpu_schools.affiliation_requests(membership_id)where status='PENDING';
create index if not exists affiliation_school_inbox on cbgpu_schools.affiliation_requests(school_id,status,created_at desc);
create index if not exists affiliation_own_requests on cbgpu_schools.affiliation_requests(user_id,school_id,created_at desc);
create table if not exists cbgpu_schools.affiliation_request_audit(id bigint generated always as identity primary key,request_id uuid not null,actor_id uuid not null references auth.users(id),action text not null,revision integer not null,before_membership jsonb,after_membership jsonb,created_at timestamptz not null default clock_timestamp());
alter table cbgpu_schools.affiliation_requests enable row level security;alter table cbgpu_schools.affiliation_request_audit enable row level security;
revoke all on cbgpu_schools.affiliation_requests,cbgpu_schools.affiliation_request_audit from public,anon,authenticated,service_role;
create or replace function cbgpu_schools.affiliation_number(s cbgpu_schools.schools,m cbgpu_schools.memberships,number text,yr integer)returns void language plpgsql security definer set search_path='' as $$declare expected integer;begin
 expected:=coalesce(m.reverification_school_year,greatest((select max(school_year)from cbgpu_schools.academic_runs where school_id=s.id),extract(year from now())::integer));
 if yr is distinct from expected or number is null or length(number)not between 1 and 80 or number ~ '[[:cntrl:]]'then raise exception 'INVALID_STUDENT_YEAR_OR_NUMBER'using errcode='23514';end if;
 if s.configuration->>'student_id_policy'='CBSH_4_DIGIT'and(number!~'^[1-3][1-3][0-9]{2}$'or(m.required_grade is not null and left(number,1)::integer<>m.required_grade))then raise exception 'INVALID_STUDENT_GRADE_OR_NUMBER'using errcode='23514';end if;
 if coalesce(s.configuration->>'student_id_policy','')not in('SCHOOL_DEFINED','CBSH_4_DIGIT')then raise exception 'UNSUPPORTED_STUDENT_NUMBER_POLICY'using errcode='23514';end if;
 perform pg_advisory_xact_lock(hashtextextended('school-number:'||s.id||':'||yr::text||':'||number,0));
 if exists(select 1 from cbgpu_schools.memberships x where x.school_id=s.id and x.user_id<>m.user_id and x.student_number=number and x.student_number_school_year=yr and x.status in('ACTIVE','PENDING_VERIFICATION','REVERIFICATION_REQUIRED'))then raise exception 'DUPLICATE_STUDENT_NUMBER'using errcode='23505';end if;
end$$;
create or replace function public.cbgpu_school_affiliation_api(action text,payload jsonb default '{}')returns jsonb language plpgsql security definer set search_path='' as $$
declare actor uuid:=auth.uid();sid text:=payload->>'school_id';s cbgpu_schools.schools;m cbgpu_schools.memberships;r cbgpu_schools.affiliation_requests;platform boolean;kind text;num text;yr integer;before_value jsonb;fingerprint text;decision text;begin
 if not public.cbgpu_personal_project_identity()then raise exception 'AUTHENTICATION_REQUIRED'using errcode='42501';end if;
 if jsonb_typeof(payload)<>'object'or pg_column_size(payload)>16384 then raise exception 'INVALID_AFFILIATION_REQUEST'using errcode='23514';end if;
 select coalesce(is_admin,false)into platform from public.profiles where id=actor;
 if action in('own','inbox','platform_inbox')then
  if action='platform_inbox'and not platform or action='inbox'and not(platform or cbgpu_schools.has_role(actor,sid,array['SCHOOL_OWNER','SCHOOL_ADMIN']))then raise exception 'AFFILIATION_REVIEW_REQUIRED'using errcode='42501';end if;
  if action='own'then select * into s from cbgpu_schools.schools where id=sid;select * into m from cbgpu_schools.memberships where school_id=sid and user_id=actor;end if;
  return jsonb_build_object('membership',case when action='own'and m.id is not null then cbgpu_schools.member_json(m)end,'student_id_policy',case when action='own'then s.configuration->>'student_id_policy'end,'school_year',case when action='own'then coalesce(m.reverification_school_year,greatest((select max(school_year)from cbgpu_schools.academic_runs where school_id=sid),extract(year from now())::integer))end,'items',(select coalesce(jsonb_agg(to_jsonb(x)-'membership_fingerprint'order by x.created_at desc),'[]')from(select * from cbgpu_schools.affiliation_requests where case when action='own'then user_id=actor and school_id=sid when action='inbox'then school_id=sid else true end order by created_at desc limit 100)x));
 end if;
 if action='create'then
  if exists(select 1 from jsonb_object_keys(payload)k where k not in('school_id','membership_type','student_number','school_year','name','department','reason','consent'))or payload->'consent'is distinct from'true'::jsonb then raise exception 'EXPLICIT_AFFILIATION_CONSENT_REQUIRED'using errcode='23514';end if;
  select * into s from cbgpu_schools.schools where id=sid for share;if not found or s.status<>'APPROVED'then raise exception 'APPROVED_SCHOOL_REQUIRED'using errcode='42501';end if;
  select * into m from cbgpu_schools.memberships where school_id=sid and user_id=actor for update;if not found or m.status='SUSPENDED'or(m.ends_at is not null and m.ends_at<=clock_timestamp())then raise exception 'CURRENT_MEMBERSHIP_REQUIRED'using errcode='42501';end if;
  kind:=payload->>'membership_type';num:=nullif(btrim(payload->>'student_number'),'');yr:=(payload->>'school_year')::integer;
  if kind not in('STUDENT','TEACHER','EXTERNAL')or kind is null or length(btrim(payload->>'name'))not between 1 and 100 or coalesce(length(btrim(payload->>'reason')),0)not between 1 and 2000 or coalesce(length(payload->>'department'),0)>150 then raise exception 'INVALID_AFFILIATION_FIELDS'using errcode='23514';end if;
  if kind='STUDENT'then perform cbgpu_schools.affiliation_number(s,m,num,yr);elsif num is not null or yr is not null then raise exception 'UNEXPECTED_STUDENT_FIELDS'using errcode='23514';end if;
  fingerprint:=encode(sha256(convert_to(to_jsonb(m)::text,'UTF8')),'hex');
  insert into cbgpu_schools.affiliation_requests(membership_id,school_id,user_id,requested_type,student_number,school_year,name,contact_email,department,reason,status,membership_fingerprint)
  select m.id,sid,actor,kind,case when kind='STUDENT'then num end,case when kind='STUDENT'then yr end,btrim(payload->>'name'),u.email,nullif(btrim(payload->>'department'),''),btrim(payload->>'reason'),'PENDING',fingerprint from auth.users u where u.id=actor and u.email_confirmed_at is not null returning * into r;
  if not found then raise exception 'VERIFIED_EMAIL_REQUIRED'using errcode='42501';end if;
 elsif action in('review','withdraw')then
  select * into r from cbgpu_schools.affiliation_requests where id=(payload->>'id')::uuid and(action='review'or user_id=actor)for update;if not found then raise exception 'AFFILIATION_REQUEST_NOT_FOUND'using errcode='P0002';end if;
  if sid is not null and sid<>r.school_id then raise exception 'SCHOOL_REQUEST_MISMATCH'using errcode='42501';end if;
  if r.status<>'PENDING'or r.revision is distinct from(payload->>'revision')::integer then raise exception 'AFFILIATION_REVISION_CONFLICT'using errcode='40001';end if;
  if action='withdraw'then decision:='WITHDRAWN';else
   if r.user_id=actor then raise exception 'INDEPENDENT_AFFILIATION_REVIEW_REQUIRED'using errcode='42501';end if;
   if not(platform or cbgpu_schools.has_role(actor,r.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN']))then raise exception 'AFFILIATION_REVIEW_REQUIRED'using errcode='42501';end if;
   decision:=payload->>'decision';if decision not in('APPROVE','REJECT')or decision is null or payload->'confirmed'is distinct from'true'::jsonb then raise exception 'EXPLICIT_AFFILIATION_REVIEW_REQUIRED'using errcode='23514';end if;
   if decision='APPROVE'then
    select * into s from cbgpu_schools.schools where id=r.school_id for share;select * into m from cbgpu_schools.memberships where id=r.membership_id for update;
    if s.status<>'APPROVED'or m.status='SUSPENDED'or(m.ends_at is not null and m.ends_at<=clock_timestamp())or not exists(select 1 from auth.users u join public.profiles p on p.id=u.id where u.id=r.user_id and u.deleted_at is null and u.email_confirmed_at is not null and(u.banned_until is null or u.banned_until<=clock_timestamp())and p.account_status<>'SUSPENDED')then raise exception 'AFFILIATION_SUBJECT_UNAVAILABLE'using errcode='42501';end if;
    if encode(sha256(convert_to(to_jsonb(m)::text,'UTF8')),'hex')<>r.membership_fingerprint then raise exception 'AFFILIATION_MEMBERSHIP_CHANGED'using errcode='40001';end if;
    if r.requested_type='STUDENT'then perform cbgpu_schools.affiliation_number(s,m,r.student_number,r.school_year);end if;
    before_value:=to_jsonb(m);update cbgpu_schools.memberships set membership_type=r.requested_type,status='ACTIVE',verified=true,verified_at=clock_timestamp(),verified_by=actor,student_number=r.student_number,student_number_school_year=r.school_year,department=r.department,required_grade=null,reverification_school_year=null,school_roles=array(select role from unnest(m.school_roles)role where role not in('STUDENT','TEACHER'))||case when r.requested_type in('STUDENT','TEACHER')then array[r.requested_type]else '{}'::text[]end where id=m.id returning * into m;
   end if;
   decision:=case decision when'APPROVE'then'APPROVED'else'REJECTED'end;
  end if;
  update cbgpu_schools.affiliation_requests set status=decision,reviewed_by=case when action='review'then actor end,review_note=left(payload->>'note',2000),revision=revision+1,updated_at=clock_timestamp()where id=r.id returning * into r;
 else raise exception 'UNKNOWN_AFFILIATION_ACTION'using errcode='23514';end if;
 insert into cbgpu_schools.affiliation_request_audit(request_id,actor_id,action,revision,before_membership,after_membership)values(r.id,actor,action||':'||r.status,r.revision,before_value,case when before_value is not null then to_jsonb(m)end);
 return jsonb_build_object('request',to_jsonb(r)-'membership_fingerprint','membership',case when before_value is not null then cbgpu_schools.member_json(m)end);
end$$;
revoke all on function cbgpu_schools.affiliation_number(cbgpu_schools.schools,cbgpu_schools.memberships,text,integer),public.cbgpu_school_affiliation_api(text,jsonb)from public,anon,authenticated,service_role;
grant execute on function public.cbgpu_school_affiliation_api(text,jsonb)to authenticated;
commit;
