begin;
-- Membership-only rollover: primary profile/runtime compatibility is a separate
-- reviewed adapter. No automatic execution, no ownership/auth/session changes.
alter table cbgpu_schools.memberships add column if not exists required_grade integer check(required_grade between 1 and 3);
alter table cbgpu_schools.memberships add column if not exists reverification_school_year integer;
create table if not exists cbgpu_schools.academic_runs(
 id uuid primary key default gen_random_uuid(),school_id text not null references cbgpu_schools.schools(id),school_year integer not null,
 actor_id uuid not null references auth.users(id),preview_fingerprint text not null,counts jsonb not null,before_memberships jsonb not null,
 completed_at timestamptz not null default now(),unique(school_id,school_year)
);
alter table cbgpu_schools.academic_runs enable row level security;
revoke all on cbgpu_schools.academic_runs from public,anon,authenticated;
create or replace function cbgpu_schools.academic_preview(sid text,target_year integer) returns jsonb language plpgsql stable security definer set search_path='' as $$
declare rows jsonb;counts jsonb;begin
 select coalesce(jsonb_agg(jsonb_build_object('membership_id',m.id,'user_id',m.user_id,'student_number',m.student_number,'grade',case when m.student_number ~ '^[1-3][1-3][0-9]{2}$' then left(m.student_number,1)::integer end,'action',case when m.student_number !~ '^[1-3][1-3][0-9]{2}$' or m.student_number is null then 'INVALID' when left(m.student_number,1)='3' then 'EXTERNAL' else 'REVERIFICATION_REQUIRED' end) order by m.id),'[]') into rows from cbgpu_schools.memberships m where m.school_id=sid and m.membership_type='STUDENT' and m.status='ACTIVE' and m.verified and(m.student_number_school_year is null or m.student_number_school_year<target_year);
 select jsonb_build_object('grade1',count(*) filter(where r->>'grade'='1'),'grade2',count(*) filter(where r->>'grade'='2'),'grade3',count(*) filter(where r->>'grade'='3'),'invalid',count(*) filter(where r->>'action'='INVALID')) into counts from jsonb_array_elements(rows)r;
 return jsonb_build_object('school_id',sid,'school_year',target_year,'supported',true,'items',rows,'counts',counts,'preview_fingerprint',encode(sha256(convert_to(jsonb_build_object('school_id',sid,'school_year',target_year,'items',rows)::text,'UTF8')),'hex'),'errors','[]'::jsonb);
end$$;
create or replace function public.cbgpu_school_academic_api(action text,payload jsonb default '{}') returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();sid text:=payload->>'school_id';yr integer;preview jsonb;m cbgpu_schools.memberships;run cbgpu_schools.academic_runs;before_rows jsonb;number text;begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if not exists(select 1 from cbgpu_schools.schools where id=sid and status='APPROVED' and id='cbsh' and configuration->>'student_id_policy'='CBSH_4_DIGIT') then raise exception 'UNSUPPORTED_ACADEMIC_POLICY' using errcode='23514';end if;
 if action='reverify' then
 yr:=(payload->>'school_year')::integer;number:=btrim(payload->>'student_number');
 select * into m from cbgpu_schools.memberships where user_id=uid and school_id=sid for update;
 if not found or m.membership_type<>'STUDENT' or m.status<>'REVERIFICATION_REQUIRED' or m.reverification_school_year is distinct from yr then raise exception 'REVERIFICATION_NOT_REQUIRED' using errcode='23514';end if;
 if number is null or number !~ '^[1-3][1-3][0-9]{2}$' or left(number,1)::integer is distinct from m.required_grade then raise exception 'INVALID_STUDENT_NUMBER' using errcode='23514';end if;
 perform pg_advisory_xact_lock(hashtextextended('academic-number:'||sid||':'||yr::text||':'||number,0));
 if exists(select 1 from cbgpu_schools.memberships where school_id=sid and user_id<>uid and student_number=number and student_number_school_year=yr and status in('ACTIVE','PENDING_VERIFICATION')) then raise exception 'DUPLICATE_STUDENT_NUMBER' using errcode='23505';end if;
 update cbgpu_schools.memberships set student_number=number,student_number_school_year=yr,status='PENDING_VERIFICATION',verified=false,verified_at=null,verified_by=null where id=m.id returning * into m;
 return jsonb_build_object('membership',to_jsonb(m),'requires_review',true);
 end if;
 if not cbgpu_schools.has_role(uid,sid,array['SCHOOL_OWNER','SCHOOL_ADMIN']) then raise exception 'SCHOOL_ADMIN_REQUIRED' using errcode='42501';end if;
 if action='history' then return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(t)-'before_memberships' order by completed_at desc),'[]') from(select * from cbgpu_schools.academic_runs where school_id=sid order by completed_at desc limit 30)t));end if;
 yr:=(payload->>'school_year')::integer;
 if yr is null or yr not between extract(year from now())::integer and extract(year from now())::integer+1 then raise exception 'INVALID_ACADEMIC_YEAR' using errcode='23514';end if;
 if action='preview' then return cbgpu_schools.academic_preview(sid,yr);end if;
 if action<>'execute' then raise exception 'UNKNOWN_ACADEMIC_ACTION' using errcode='23514';end if;
 if payload->'confirmed' is distinct from 'true'::jsonb then raise exception 'EXPLICIT_CONFIRMATION_REQUIRED' using errcode='23514';end if;
 perform pg_advisory_xact_lock(hashtextextended('school-academic:'||sid,0));
 if exists(select 1 from cbgpu_schools.academic_runs where school_id=sid and school_year=yr) then raise exception 'ACADEMIC_YEAR_ALREADY_COMPLETED' using errcode='23505';end if;
 -- Serialize all included rows; re-read fingerprint after locks before mutation.
 lock table cbgpu_schools.memberships in share row exclusive mode;
 perform 1 from cbgpu_schools.memberships where school_id=sid for update;
 preview:=cbgpu_schools.academic_preview(sid,yr);
 if payload->>'preview_fingerprint' is distinct from preview->>'preview_fingerprint' then raise exception 'ACADEMIC_PREVIEW_CHANGED' using errcode='40001';end if;
 if (preview#>>'{counts,invalid}')::integer>0 then raise exception 'INVALID_STUDENT_NUMBERS_REQUIRE_REVIEW' using errcode='23514';end if;
 select coalesce(jsonb_agg(to_jsonb(t)),'[]') into before_rows from cbgpu_schools.memberships t where t.id in(select (r->>'membership_id')::uuid from jsonb_array_elements(preview->'items')r);
 insert into cbgpu_schools.academic_runs(school_id,school_year,actor_id,preview_fingerprint,counts,before_memberships) values(sid,yr,uid,preview->>'preview_fingerprint',preview->'counts',before_rows) returning * into run;
 update cbgpu_schools.memberships set membership_type=case when left(student_number,1)='3' then 'EXTERNAL' else 'STUDENT' end,status=case when left(student_number,1)='3' then 'ACTIVE' else 'REVERIFICATION_REQUIRED' end,required_grade=case when left(student_number,1)='3' then null else left(student_number,1)::integer+1 end,reverification_school_year=yr,student_number=null,student_number_school_year=yr,verified=false,verified_at=null,verified_by=null,school_roles=array_remove(school_roles,'STUDENT') where id in(select (r->>'membership_id')::uuid from jsonb_array_elements(preview->'items')r);
 return jsonb_build_object('ok',true,'id',run.id,'school_id',sid,'school_year',yr,'counts',run.counts,'completed_at',run.completed_at);
end$$;
revoke all on function cbgpu_schools.academic_preview(text,integer) from public,anon,authenticated;
revoke all on function public.cbgpu_school_academic_api(text,jsonb) from public,anon;
grant execute on function public.cbgpu_school_academic_api(text,jsonb) to authenticated;
commit;
