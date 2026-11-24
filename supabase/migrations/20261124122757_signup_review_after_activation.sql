begin;
-- A null approval is an available account awaiting platform review.
-- School affiliation and resource checks keep their independent verification.
create or replace function public.handle_new_user()
returns trigger language plpgsql security definer set search_path='' as $$
declare
 kind text:=coalesce(new.raw_user_meta_data->>'account_type_request','STUDENT');
 school text;
begin
 if coalesce(length(btrim(new.raw_user_meta_data->>'full_name')),0) not between 1 and 100 then
  raise exception 'FULL_NAME_REQUIRED' using errcode='23514';
 end if;
 if kind not in('STUDENT','TEACHER','EXTERNAL') then
  raise exception 'INVALID_ACCOUNT_TYPE' using errcode='23514';
 end if;
 school:=case when kind='EXTERNAL' then null else coalesce(new.raw_user_meta_data->>'school_id',case when new.raw_user_meta_data->>'account_type_request' is null then 'cbsh' end) end;
 if kind<>'EXTERNAL' and school is distinct from 'cbsh' then
  raise exception 'INVALID_SCHOOL' using errcode='23514';
 end if;
 insert into public.profiles(id,full_name,student_id,is_admin,is_approved,account_type,account_status,school_id,affiliation_verified,department,organization)
 values(new.id,new.raw_user_meta_data->>'full_name',case when kind='STUDENT' then new.raw_user_meta_data->>'student_id' end,false,null,kind,
  case when kind='EXTERNAL' then 'ACTIVE' else 'PENDING_VERIFICATION' end,school,false,left(new.raw_user_meta_data->>'department',200),left(new.raw_user_meta_data->>'organization',200))
 on conflict(id) do nothing;
 return new;
end$$;

create or replace function public.cbgpu_review_signup(p_user_id uuid,p_decision text)
returns void language plpgsql security definer set search_path='' as $$
declare
 reviewer uuid:=auth.uid();
 target public.profiles;
begin
 if reviewer is null or reviewer=p_user_id or not cbgpu_accounts.current_session_valid() or not exists(
  select 1 from public.profiles p join auth.users u on u.id=p.id
  where p.id=reviewer and p.is_admin is true and p.is_approved is distinct from false
   and p.account_status='ACTIVE' and u.deleted_at is null and (u.banned_until is null or u.banned_until<=now())
 ) then raise exception 'INDEPENDENT_PLATFORM_REVIEW_REQUIRED' using errcode='42501';end if;
 if p_decision is null or p_decision not in('APPROVE','REJECT') then
  raise exception 'INVALID_SIGNUP_DECISION' using errcode='22023';
 end if;
 select p.* into target from public.profiles p join auth.users u on u.id=p.id
 where p.id=p_user_id and p.is_admin is distinct from true
  and (p.is_approved is null or (p_decision='APPROVE' and p.is_approved is false))
  and p.account_status in('ACTIVE','PENDING_VERIFICATION','REVERIFICATION_REQUIRED')
  and u.email_confirmed_at is not null and u.deleted_at is null
  and (u.banned_until is null or u.banned_until<=now())
 for update of p;
 if not found then raise exception 'SIGNUP_NOT_AWAITING_REVIEW' using errcode='P0002';end if;
 if p_decision='APPROVE' then
  update public.profiles set is_approved=true where id=target.id;
 else
  update public.profiles set is_approved=false,account_status='SUSPENDED' where id=target.id;
 end if;
 insert into cbgpu_accounts.audit(actor_id,event,target_id,before_value,after_value)
 values(reviewer,'SIGNUP_'||p_decision,target.id,
  jsonb_build_object('account_status',target.account_status,'is_approved',target.is_approved),
  jsonb_build_object('account_status',case when p_decision='REJECT' then 'SUSPENDED' else target.account_status end,'is_approved',p_decision='APPROVE'));
end$$;

revoke all on function public.handle_new_user() from public,anon,authenticated;
revoke all on function public.cbgpu_review_signup(uuid,text) from public,anon,authenticated;
grant execute on function public.cbgpu_review_signup(uuid,text) to authenticated;
notify pgrst, 'reload schema';
commit;
