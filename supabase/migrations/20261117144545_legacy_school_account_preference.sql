begin;
-- Preserve the existing CBsh experience through a one-time ACCOUNT preference,
-- never a browser fallback or a default for users registering after migration.
create or replace function cbgpu_schools.seed_legacy_school_preference()
returns integer language plpgsql security definer set search_path='' as $$
declare cutoff timestamptz;inserted integer;
begin
 if exists(select 1 from cbgpu_schools.migration_state where key='legacy-cbsh-preference-v1') then return 0;end if;
 select completed_at into cutoff from cbgpu_schools.migration_state where key='legacy-cbsh-v1';
 if cutoff is null then raise exception 'LEGACY_AFFILIATION_MIGRATION_REQUIRED';end if;
 insert into cbgpu_schools.preferences(user_id,school_id)
 select m.user_id,'cbsh' from cbgpu_schools.memberships m
 join public.profiles p on p.id=m.user_id join auth.users u on u.id=m.user_id
 where m.school_id='cbsh' and m.created_at<=cutoff and u.created_at<=cutoff
 and cbgpu_schools.active_member(m.user_id,'cbsh')
 on conflict(user_id) do nothing;
 get diagnostics inserted=row_count;
 insert into cbgpu_schools.migration_state(key) values('legacy-cbsh-preference-v1') on conflict(key)do nothing;
 return inserted;
end$$;
revoke all on function cbgpu_schools.seed_legacy_school_preference() from public,anon,authenticated,service_role;
select cbgpu_schools.seed_legacy_school_preference();
commit;
