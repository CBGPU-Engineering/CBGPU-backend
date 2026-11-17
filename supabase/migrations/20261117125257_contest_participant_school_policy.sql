begin;
-- Preserve every existing audience and registration. This is an opt-in policy,
-- separate from whether the description is publicly readable or listed.
alter table public.contests add column if not exists participant_policy text not null default 'ALL_ELIGIBLE'
 check(participant_policy in('ALL_ELIGIBLE','HIGH_SCHOOL_STUDENTS'));

create or replace function cbgpu_schools.contest_participant_allowed(contest uuid,uid uuid)
returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from public.contests c where c.id=contest and
 (c.participant_policy='ALL_ELIGIBLE' or
  (c.participant_policy='HIGH_SCHOOL_STUDENTS' and exists(
    select 1 from cbgpu_schools.memberships m join cbgpu_schools.schools s on s.id=m.school_id
    where m.user_id=uid and m.membership_type='STUDENT' and s.type='HIGH_SCHOOL'
    and cbgpu_schools.active_member(uid,m.school_id)))))
$$;
-- Extend the existing single fresh authorization predicate. Registrations,
-- submissions, token writes, waitlists and ballots all reuse it already.
do $$declare definition text;updated text;begin
 definition:=pg_get_functiondef('cbgpu_schools.contest_register(uuid,uuid)'::regprocedure);
 if position('cbgpu_schools.contest_participant_allowed(contest,uid)' in definition)=0 then
  updated:=replace(definition,'where c.id=contest and',
   'where c.id=contest and cbgpu_schools.contest_participant_allowed(contest,uid) and');
  if updated=definition then raise exception 'CONTEST_PARTICIPANT_POLICY_VERSION_MISMATCH';end if;
  execute updated;
 end if;
end$$;
create or replace function cbgpu_schools.guard_contest_participant_policy()
returns trigger language plpgsql security definer set search_path='' as $$begin
 if auth.uid() is not null and new.participant_policy is distinct from old.participant_policy
 and not (cbgpu_accounts.current_session_valid() and cbgpu_schools.scope_valid(auth.uid(),new.school_id,new.access_scope,new.allowed_school_ids))
 then raise exception 'SCHOOL_SCOPE_PERMISSION_REQUIRED' using errcode='42501';end if;
 return new;
end$$;
drop trigger if exists zz_contest_participant_policy on public.contests;
create trigger zz_contest_participant_policy before update on public.contests for each row
 execute function cbgpu_schools.guard_contest_participant_policy();
revoke all on function cbgpu_schools.contest_participant_allowed(uuid,uuid),cbgpu_schools.guard_contest_participant_policy() from public,anon,authenticated;
commit;
