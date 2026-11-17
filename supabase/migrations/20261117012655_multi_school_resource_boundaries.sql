begin;
-- Legacy content stays in CBsh; original IDs/owners/members and all files survive.
alter table public.contests add column if not exists school_id text not null default 'cbsh' references cbgpu_schools.schools(id),add column if not exists access_scope text not null default 'SCHOOL',add column if not exists allowed_school_ids text[] not null default '{}';
alter table public.research_notes add column if not exists school_id text not null default 'cbsh' references cbgpu_schools.schools(id),add column if not exists access_scope text not null default 'SCHOOL',add column if not exists allowed_school_ids text[] not null default '{}';
alter table public.shared_materials add column if not exists school_id text not null default 'cbsh' references cbgpu_schools.schools(id),add column if not exists access_scope text not null default 'SCHOOL',add column if not exists allowed_school_ids text[] not null default '{}';
alter table public.cpu_projects add column if not exists school_id text not null default 'cbsh' references cbgpu_schools.schools(id);
create index if not exists contests_school on public.contests(school_id);
create index if not exists research_notes_school on public.research_notes(school_id);
create index if not exists shared_materials_school on public.shared_materials(school_id);
create or replace function cbgpu_schools.scope_read(uid uuid,school text,scope text,allowed text[]) returns boolean language sql stable security definer set search_path='' as $$select scope='PUBLIC' or(scope='SCHOOL' and cbgpu_schools.active_member(uid,school)) or(scope='MULTI_SCHOOL' and exists(select 1 from unnest(allowed||school) sid where cbgpu_schools.active_member(uid,sid)))$$;
create or replace function cbgpu_schools.scope_valid(uid uuid,school text,scope text,allowed text[]) returns boolean language sql stable security definer set search_path='' as $$select cbgpu_schools.active_member(uid,school) and scope in('PRIVATE','SCHOOL','MULTI_SCHOOL','PUBLIC') and cardinality(allowed)<=100 and array_position(allowed,null) is null and(scope='MULTI_SCHOOL' or cardinality(allowed)=0) and not exists(select 1 from unnest(allowed) sid where not exists(select 1 from cbgpu_schools.schools s where s.id=sid and s.status='APPROVED'))$$;
create or replace function cbgpu_schools.contest_read(contest uuid,uid uuid) returns boolean language sql stable security definer set search_path='' as $$select exists(select 1 from public.contests c where c.id=contest and(c.host_id=uid or cbgpu_schools.scope_read(uid,c.school_id,c.access_scope,c.allowed_school_ids) or exists(select 1 from public.event_hosts h where h.contest_id=c.id and h.user_id=uid) or exists(select 1 from public.event_registrations r where r.contest_id=c.id and r.user_id=uid)))$$;
-- Actual target comes from the immutable contest ID, never a caller school hint.
create or replace function cbgpu_schools.contest_register(contest uuid,uid uuid) returns boolean language sql stable security definer set search_path='' as $$select exists(select 1 from public.contests c join public.profiles p on p.id=uid join auth.users u on u.id=uid where c.id=contest and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now()) and exists(select 1 from cbgpu_schools.schools s where s.id=c.school_id and s.status='APPROVED' and 'contests'=any(s.features)) and(c.access_scope='PUBLIC' or(c.access_scope='MULTI_SCHOOL' and exists(select 1 from unnest(c.allowed_school_ids||c.school_id) sid where cbgpu_accounts.can_user_use_resource(uid,'contests',sid))) or(c.access_scope='SCHOOL' and cbgpu_accounts.can_user_use_resource(uid,'contests',c.school_id)) or(c.access_scope='PRIVATE' and exists(select 1 from public.event_hosts h where h.contest_id=c.id and h.user_id=uid))))$$;
create or replace function public.cbgpu_can_register_contest(p_contest uuid) returns boolean language sql stable security definer set search_path='' as $$select cbgpu_accounts.current_session_valid() and cbgpu_schools.contest_register(p_contest,auth.uid())$$;
revoke all on function public.cbgpu_can_register_contest(uuid) from public,anon;grant execute on function public.cbgpu_can_register_contest(uuid) to authenticated;
create or replace function public.cbgpu_can_create_contest() returns boolean language sql stable security definer set search_path='' as $$select cbgpu_accounts.current_session_valid() and exists(select 1 from cbgpu_schools.memberships m where m.user_id=auth.uid() and cbgpu_accounts.can_user_use_resource(auth.uid(),'contests',m.school_id))$$;
revoke all on function public.cbgpu_can_create_contest() from public,anon;grant execute on function public.cbgpu_can_create_contest() to authenticated;
create or replace function public.cbgpu_school_scope_read(school text,scope text,allowed text[]) returns boolean language sql stable security definer set search_path='' as $$select cbgpu_schools.scope_read(auth.uid(),school,scope,allowed)$$;
create or replace function public.cbgpu_school_scope_valid(school text,scope text,allowed text[]) returns boolean language sql stable security definer set search_path='' as $$select cbgpu_accounts.current_session_valid() and cbgpu_schools.scope_valid(auth.uid(),school,scope,allowed)$$;
create or replace function public.cbgpu_contest_visible(contest uuid) returns boolean language sql stable security definer set search_path='' as $$select cbgpu_schools.contest_read(contest,auth.uid())$$;
revoke all on function public.cbgpu_school_scope_read(text,text,text[]),public.cbgpu_school_scope_valid(text,text,text[]),public.cbgpu_contest_visible(uuid) from public;
grant execute on function public.cbgpu_school_scope_read(text,text,text[]),public.cbgpu_contest_visible(uuid) to anon,authenticated;
grant execute on function public.cbgpu_school_scope_valid(text,text,text[]) to authenticated;
create or replace function public.cbgpu_personal_project_identity() returns boolean language sql stable security definer set search_path='' as $$select cbgpu_accounts.current_session_valid() and exists(select 1 from public.profiles p join auth.users u on u.id=p.id where p.id=auth.uid() and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now()))$$;
revoke all on function public.cbgpu_personal_project_identity() from public,anon;grant execute on function public.cbgpu_personal_project_identity() to authenticated;
-- Restrictive policies intersect existing owner/member/host rules, never replace them.
drop policy if exists multischool_contest_read on public.contests;
create policy multischool_contest_read on public.contests as restrictive for select to anon,authenticated using(public.cbgpu_contest_visible(id));
drop policy if exists multischool_contest_insert on public.contests;
create policy multischool_contest_insert on public.contests as restrictive for insert to authenticated with check(public.cbgpu_school_scope_valid(school_id,access_scope,allowed_school_ids));
drop policy if exists multischool_material_read on public.shared_materials;
create policy multischool_material_read on public.shared_materials as restrictive for select to anon,authenticated using(uploader_id=auth.uid() or public.cbgpu_school_scope_read(school_id,access_scope,allowed_school_ids));
drop policy if exists multischool_material_insert on public.shared_materials;
create policy multischool_material_insert on public.shared_materials as restrictive for insert to authenticated with check(public.cbgpu_school_scope_valid(school_id,access_scope,allowed_school_ids) and public.cbgpu_can_use_resource('school_storage',school_id));
drop policy if exists multischool_notes_read on public.research_notes;
create policy multischool_notes_read on public.research_notes as restrictive for select to anon,authenticated using(user_id=auth.uid() or public.cbgpu_school_scope_read(school_id,access_scope,allowed_school_ids));
drop policy if exists multischool_notes_insert on public.research_notes;
create policy multischool_notes_insert on public.research_notes as restrictive for insert to authenticated with check(user_id=auth.uid() and public.cbgpu_personal_project_identity() and(not is_shared or public.cbgpu_school_scope_valid(school_id,access_scope,allowed_school_ids)));
-- Changing sharing or school requires a fresh verified source-school membership.
-- Ordinary owner edits remain valid after graduation or a membership expires.
create or replace function cbgpu_schools.guard_scope_change() returns trigger language plpgsql security definer set search_path='' as $$begin
 if auth.uid() is null then return new;end if;
 if not cbgpu_accounts.current_session_valid() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if tg_table_name='research_notes' then if new.is_shared and not old.is_shared and not cbgpu_schools.scope_valid(auth.uid(),new.school_id,new.access_scope,new.allowed_school_ids) then raise exception 'SCHOOL_SCOPE_PERMISSION_REQUIRED' using errcode='42501';end if;end if;
 if(new.school_id,new.access_scope,new.allowed_school_ids) is distinct from(old.school_id,old.access_scope,old.allowed_school_ids) and not cbgpu_schools.scope_valid(auth.uid(),new.school_id,new.access_scope,new.allowed_school_ids) then raise exception 'SCHOOL_SCOPE_PERMISSION_REQUIRED' using errcode='42501';end if;return new;end$$;
drop trigger if exists zz_multischool_contest_scope on public.contests;create trigger zz_multischool_contest_scope before update on public.contests for each row execute function cbgpu_schools.guard_scope_change();
drop trigger if exists zz_multischool_material_scope on public.shared_materials;create trigger zz_multischool_material_scope before update on public.shared_materials for each row execute function cbgpu_schools.guard_scope_change();
drop trigger if exists zz_multischool_note_scope on public.research_notes;create trigger zz_multischool_note_scope before update on public.research_notes for each row execute function cbgpu_schools.guard_scope_change();
create or replace function cbgpu_accounts.guard_contest_execution() returns trigger language plpgsql security definer set search_path='' as $$begin
 if auth.uid() is not null and not cbgpu_accounts.current_session_valid() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if tg_table_name='event_registrations' then if new.status in('cancelled','declined') then return new;end if;end if;
 if not cbgpu_schools.contest_register(new.contest_id,case when tg_table_name='event_registrations' then new.user_id else auth.uid() end) then raise exception 'CONTEST_RESOURCE_PERMISSION_REQUIRED' using errcode='42501';end if;return new;end$$;
create or replace function public.promote_event_waitlist() returns trigger language plpgsql security definer set search_path='' as $$declare next_id uuid;begin
 if old.status='confirmed' and new.status<>'confirmed' then perform pg_advisory_xact_lock(hashtextextended(new.contest_id::text,0));select r.id into next_id from public.event_registrations r where r.contest_id=new.contest_id and r.status='waitlisted' and cbgpu_schools.contest_register(r.contest_id,r.user_id) order by r.created_at,r.id limit 1 for update skip locked;if next_id is not null then update public.event_registrations set status='confirmed' where id=next_id;end if;end if;return null;end$$;
-- Votes use the same fresh contest scope as registrations; old confirmed
-- entries alone do not retain school-only voting after affiliation/grant expiry.
create or replace function cbgpu_schools.guard_award_ballot() returns trigger language plpgsql security definer set search_path='' as $$begin
 if auth.uid() is null then return new;end if;
 if new.voter_id<>auth.uid() or not public.cbgpu_personal_project_identity() or not cbgpu_schools.contest_register(new.contest_id,auth.uid()) then raise exception 'CONTEST_VOTING_PERMISSION_REQUIRED' using errcode='42501';end if;return new;end$$;
drop trigger if exists zz_multischool_ballot_scope on public.contest_award_ballots;
create trigger zz_multischool_ballot_scope before insert or update on public.contest_award_ballots for each row execute function cbgpu_schools.guard_award_ballot();
-- Preserve the existing reviewed awards business logic; require a fresh identity
-- before its SECURITY DEFINER body can read or mutate any contest state.
do $$declare definition text;updated text;begin
 if to_regprocedure('public.contest_awards_api(uuid,text,jsonb)') is not null then
 definition:=pg_get_functiondef('public.contest_awards_api(uuid,text,jsonb)'::regprocedure);
 if position('public.cbgpu_personal_project_identity()' in definition)=0 then
 updated:=replace(definition,' if actor is null then',E' if not public.cbgpu_personal_project_identity() then raise exception ''AUTHENTICATION_REQUIRED'' using errcode=''42501'';end if;\n if actor is null then');
 if updated=definition then raise exception 'CONTEST_AWARDS_VERSION_MISMATCH';end if;execute updated;
 end if;end if;
end$$;
-- Existing scoped tokens retain their same owner/session/hash/allowed IDs. Only
-- registration permission changes to each real contest's tenant; read/cancel do
-- not require a school grant and remain protected by their existing RLS reader.
do $$declare definition text;updated text;begin
 if to_regprocedure('contest_api.mint(bytea,text,text[],uuid[],integer)') is not null then
 definition:=pg_get_functiondef('contest_api.mint(bytea,text,text[],uuid[],integer)'::regprocedure);
 updated:=replace(definition,'NOT contest_api._registration_allowed(actor.approved)','EXISTS(SELECT 1 FROM unnest(contest_set) cid WHERE NOT public.cbgpu_can_register_contest(cid))');
 if updated=definition and position('public.cbgpu_can_register_contest(cid)' in definition)=0 then raise exception 'CONTEST_TOKEN_MINT_VERSION_MISMATCH';end if;execute updated;
 definition:=pg_get_functiondef('contest_api._authorize(bytea,text,uuid)'::regprocedure);
 updated:=replace(definition,'NOT contest_api._registration_allowed(approved)','NOT public.cbgpu_can_register_contest(p_contest)');
 if updated=definition and position('public.cbgpu_can_register_contest(p_contest)' in definition)=0 then raise exception 'CONTEST_TOKEN_AUTH_VERSION_MISMATCH';end if;execute updated;
 end if;
end$$;
drop policy if exists account_v2_contest_upload on storage.objects;
create policy account_v2_contest_upload on storage.objects as restrictive for insert to authenticated with check(
 bucket_id not in('contest-submissions','contest-presentation') or
 (bucket_id='contest-submissions' and public.cbgpu_can_register_contest(case when split_part(name,'/',1) ~ '^[0-9a-fA-F-]{36}$' then split_part(name,'/',1)::uuid end)) or
 (bucket_id='contest-presentation' and public.cbgpu_can_create_contest())
);
-- RLS can invoke only these narrow predicates; private contacts/state remain inaccessible.
revoke usage on schema cbgpu_schools from anon,authenticated;
-- The existing waitlist SECURITY DEFINER function is owned by the legacy
-- database role; grant only its required fresh predicate, never private tables.
grant usage on schema cbgpu_schools to postgres;
grant execute on function cbgpu_schools.contest_register(uuid,uuid) to postgres;
revoke all on all functions in schema cbgpu_schools from public,anon,authenticated;

commit;
