-- Requires account-v2 schema. Preserve registration ownership, cancellation and history.
begin;
create or replace function cbgpu_accounts.guard_contest_execution() returns trigger
language plpgsql security definer set search_path='' as $$
begin
 -- This trigger also runs inside SECURITY DEFINER registration/team RPCs: current_user
 -- is deliberately NOT used as an authorization bypass.
 if auth.uid() is not null and not cbgpu_accounts.current_session_valid() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if tg_table_name='event_registrations' then
  if new.status in('cancelled','declined') then return new;end if;
 end if;
 if tg_table_name='event_registrations' then
  if not cbgpu_accounts.can_user_use_resource(new.user_id,'contests','cbsh') then raise exception 'CONTEST_RESOURCE_PERMISSION_REQUIRED' using errcode='42501';end if;
 else
  if not public.cbgpu_can_use_resource('contests') then raise exception 'CONTEST_RESOURCE_PERMISSION_REQUIRED' using errcode='42501';end if;
 end if;
 return new;
end$$;
revoke all on function cbgpu_accounts.guard_contest_execution() from public,anon,authenticated;
drop trigger if exists zz_account_v2_contest_execution on public.event_registrations;
create trigger zz_account_v2_contest_execution before insert or update on public.event_registrations for each row execute function cbgpu_accounts.guard_contest_execution();
drop trigger if exists zz_account_v2_contest_submission on public.contest_submissions;
create trigger zz_account_v2_contest_submission before insert or update on public.contest_submissions for each row execute function cbgpu_accounts.guard_contest_execution();
-- An expired waitlisted guest must not make another user's cancellation fail.
-- Select the oldest still-eligible candidate, retaining every ineligible historical entry.
create or replace function public.promote_event_waitlist() returns trigger
language plpgsql security definer set search_path='' as $$
declare next_id uuid;
begin
 if old.status='confirmed' and new.status<>'confirmed' then
  perform pg_advisory_xact_lock(hashtextextended(new.contest_id::text,0));
  select r.id into next_id from public.event_registrations r
   where r.contest_id=new.contest_id and r.status='waitlisted'
   and cbgpu_accounts.can_user_use_resource(r.user_id,'contests','cbsh')
   order by r.created_at,r.id limit 1 for update skip locked;
  if next_id is not null then update public.event_registrations set status='confirmed' where id=next_id;end if;
 end if;
 return null;
end$$;
revoke all on function public.promote_event_waitlist() from public,anon,authenticated;
drop policy if exists account_v2_contest_upload on storage.objects;
create policy account_v2_contest_upload on storage.objects as restrictive for insert to authenticated
 with check(bucket_id not in('contest-submissions','contest-presentation') or public.cbgpu_can_use_resource('contests'));
commit;
