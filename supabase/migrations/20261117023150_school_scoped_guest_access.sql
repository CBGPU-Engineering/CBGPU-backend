begin;
-- School-relative collaboration preserves the guest's global identity and every
-- project membership. Approval never modifies profiles or grants school roles.
create or replace function public.cbgpu_school_guest_api(action text,payload jsonb default '{}') returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();sid text:=payload->>'school_id';g cbgpu_accounts.guest_requests;subject uuid;scopes text[];starts timestamptz;ends timestamptz;admin boolean;before_value jsonb;decision text;begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if not exists(select 1 from cbgpu_schools.schools where id=sid and status='APPROVED') then raise exception 'SCHOOL_NOT_FOUND' using errcode='P0002';end if;
 admin:=cbgpu_schools.has_role(uid,sid,array['SCHOOL_ADMIN','SCHOOL_OWNER']);
 if action='list' then return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(t)||jsonb_build_object('status',case when t.status='APPROVED' and t.ends_at<=now() then 'EXPIRED' else t.status end) order by t.created_at desc),'[]') from(select * from cbgpu_accounts.guest_requests where school_id=sid and(admin or requester_id=uid or external_user_id=uid) order by created_at desc limit 100)t));end if;
 if action='create' then
 if not cbgpu_schools.active_member(uid,sid) or not exists(select 1 from cbgpu_schools.memberships m where m.user_id=uid and m.school_id=sid and m.membership_type in('STUDENT','TEACHER')) then raise exception 'VERIFIED_SCHOOL_SPONSOR_REQUIRED' using errcode='42501';end if;
 if jsonb_typeof(payload->'resources') is distinct from 'array' then raise exception 'INVALID_GUEST_RESOURCES' using errcode='23514';end if;
 scopes:=array(select distinct jsonb_array_elements_text(payload->'resources'));
 if cardinality(scopes) not between 1 and 6 or array_position(scopes,null) is not null or not(scopes<@array['gpu','study','school_storage','fabrication','school_compute','contests']) or exists(select 1 from unnest(scopes) resource where not cbgpu_accounts.can_user_use_resource(uid,resource,sid)) then raise exception 'GUEST_RESOURCE_UNAVAILABLE' using errcode='42501';end if;
 select p.id into subject from public.profiles p join auth.users u on u.id=p.id where lower(u.email)=lower(btrim(payload->>'external_email')) and p.account_status<>'SUSPENDED' and u.email_confirmed_at is not null and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now());
 if subject is null or subject=uid then raise exception 'EXTERNAL_ACCOUNT_NOT_FOUND' using errcode='P0002';end if;
 if exists(select 1 from cbgpu_schools.memberships m where m.user_id=subject and m.school_id=sid and m.membership_type in('STUDENT','TEACHER') and cbgpu_schools.active_member(subject,sid)) then raise exception 'EXISTING_INTERNAL_MEMBERSHIP' using errcode='23514';end if;
 starts:=(payload->>'starts_at')::timestamptz;ends:=(payload->>'ends_at')::timestamptz;
 if starts is null or ends is null or not isfinite(starts) or not isfinite(ends) or ends<=starts or ends<=now() or coalesce(length(btrim(payload->>'project_name')),0) not between 1 and 200 or coalesce(length(btrim(payload->>'purpose')),0) not between 1 and 4000 then raise exception 'INVALID_GUEST_REQUEST' using errcode='23514';end if;
 perform pg_advisory_xact_lock(hashtextextended('school-guest:'||uid::text,0));if(select count(*) from cbgpu_accounts.guest_requests where requester_id=uid and created_at>now()-interval '1 day')>=20 then raise exception 'GUEST_REQUEST_RATE_LIMIT' using errcode='P0429';end if;
 insert into cbgpu_accounts.guest_requests(external_user_id,requester_id,school_id,project_name,purpose,resources,starts_at,ends_at) values(subject,uid,sid,btrim(payload->>'project_name'),btrim(payload->>'purpose'),scopes,starts,ends) returning * into g;
 elsif action='review' then
 if not admin then raise exception 'SCHOOL_ADMIN_REQUIRED' using errcode='42501';end if;
 select * into g from cbgpu_accounts.guest_requests where id=(payload->>'id')::uuid and school_id=sid for update;if not found then raise exception 'GUEST_REQUEST_NOT_FOUND' using errcode='P0002';end if;
 if uid in(g.requester_id,g.external_user_id) then raise exception 'INDEPENDENT_GUEST_REVIEW_REQUIRED' using errcode='42501';end if;
 before_value:=to_jsonb(g);decision:=payload->>'decision';
 if decision='APPROVE' then
 if g.status<>'PENDING' or g.ends_at<=now() then raise exception 'GUEST_REQUEST_NOT_PENDING' using errcode='23514';end if;
 if not cbgpu_schools.active_member(g.requester_id,sid) or not exists(select 1 from cbgpu_schools.memberships m where m.user_id=g.requester_id and m.school_id=sid and m.membership_type in('STUDENT','TEACHER')) or exists(select 1 from unnest(g.resources) resource where not cbgpu_accounts.can_user_use_resource(g.requester_id,resource,sid)) then raise exception 'SPONSOR_RESOURCE_PERMISSION_REQUIRED' using errcode='42501';end if;
 if not exists(select 1 from public.profiles p join auth.users u on u.id=p.id where p.id=g.external_user_id and p.account_status<>'SUSPENDED' and u.email_confirmed_at is not null and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now())) then raise exception 'EXTERNAL_ACCOUNT_UNAVAILABLE' using errcode='42501';end if;
 update cbgpu_accounts.guest_requests set status='APPROVED',reviewed_by=uid,reviewed_at=now() where id=g.id returning * into g;
 elsif decision='REJECT' then
 if g.status<>'PENDING' then raise exception 'GUEST_REQUEST_NOT_PENDING' using errcode='23514';end if;update cbgpu_accounts.guest_requests set status='REJECTED',reviewed_by=uid,reviewed_at=now() where id=g.id returning * into g;
 elsif decision='REVOKE' then
 if g.status<>'APPROVED' then raise exception 'GUEST_REQUEST_NOT_APPROVED' using errcode='23514';end if;update cbgpu_accounts.guest_requests set status='REVOKED',reviewed_by=uid,reviewed_at=now() where id=g.id returning * into g;
 else raise exception 'INVALID_GUEST_REVIEW' using errcode='23514';end if;
 else raise exception 'UNKNOWN_GUEST_ACTION' using errcode='23514';end if;
 insert into cbgpu_accounts.audit(actor_id,event,target_id,before_value,after_value) values(uid,'school_guest_'||action,g.id,before_value,to_jsonb(g));return jsonb_build_object('request',to_jsonb(g));
end$$;
revoke all on function public.cbgpu_school_guest_api(text,jsonb) from public,anon;grant execute on function public.cbgpu_school_guest_api(text,jsonb) to authenticated;
commit;
