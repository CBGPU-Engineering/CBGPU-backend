begin;
-- A temporary resource context is NOT affiliation or a school membership.
create or replace function cbgpu_schools.context_available(subject uuid,school text) returns boolean language sql stable security definer set search_path='' as $$
select cbgpu_schools.active_member(subject,school) or exists(select 1 from cbgpu_accounts.guest_requests g where g.external_user_id=subject and g.school_id=school and g.status='APPROVED' and g.starts_at<=now() and g.ends_at>now() and exists(select 1 from unnest(g.resources) r where cbgpu_accounts.can_user_use_resource(subject,r,school)))
$$;
create or replace function cbgpu_schools.resource_context(subject uuid,school text) returns jsonb language plpgsql stable security definer set search_path='' as $$
declare s cbgpu_schools.schools;m cbgpu_schools.memberships;guest_end timestamptz;resources text[];begin
 if not cbgpu_schools.context_available(subject,school) then return null;end if;
 select * into s from cbgpu_schools.schools where id=school and status='APPROVED';if not found then return null;end if;
 select * into m from cbgpu_schools.memberships where user_id=subject and school_id=school and cbgpu_schools.active_member(subject,school);
 select min(g.ends_at) into guest_end from cbgpu_accounts.guest_requests g where g.external_user_id=subject and g.school_id=school and g.status='APPROVED' and g.starts_at<=now() and g.ends_at>now() and cbgpu_schools.active_member(g.requester_id,school) and exists(select 1 from unnest(g.resources)r where cbgpu_accounts.can_user_use_resource(subject,r,school));
 resources:=array(select r from unnest(array['gpu','study','school_storage','fabrication','school_compute','contests'])r where cbgpu_accounts.can_user_use_resource(subject,r,school) order by r);
 return jsonb_build_object('school_id',school,'school',jsonb_build_object('id',s.id,'official_name',s.official_name,'display_name',s.display_name,'type',s.type,'region',s.region,'features',s.features,'ads_enabled',coalesce((s.configuration->>'ads_enabled')::boolean,false),'life_capabilities',cbgpu_schools.life_capabilities(s.id)),'type',case when m.id is not null then 'MEMBERSHIP' else 'GUEST' end,'resources',resources,'expires_at',least(m.ends_at,guest_end),'can_administer',m.id is not null and cbgpu_schools.has_role(subject,school,array['SCHOOL_OWNER','SCHOOL_ADMIN']))||case when m.id is not null then jsonb_build_object('membership_id',m.id) else '{}'::jsonb end;
end$$;
create or replace function public.cbgpu_school_resource_contexts() returns jsonb language plpgsql stable security definer set search_path='' as $$
declare uid uuid:=auth.uid();items jsonb;begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 select coalesce(jsonb_agg(t.context order by t.school_id),'[]') into items from(select candidates.school_id,cbgpu_schools.resource_context(uid,candidates.school_id) context from(select m.school_id from cbgpu_schools.memberships m where m.user_id=uid union select g.school_id from cbgpu_accounts.guest_requests g where g.external_user_id=uid and g.status='APPROVED' and g.starts_at<=now() and g.ends_at>now())candidates)t where t.context is not null;
 return jsonb_build_object('items',items,'current_school_id',(select p.school_id from cbgpu_schools.preferences p where p.user_id=uid and cbgpu_schools.context_available(uid,p.school_id)),'server_time',now());
end$$;
-- Explicit per-resource reasons never contain account/roster/internal topology.
create or replace function public.cbgpu_self_resource_access(target_school text default 'cbsh') returns jsonb language plpgsql stable security definer set search_path='' as $$
declare uid uuid:=auth.uid();s cbgpu_schools.schools;context jsonb;resources jsonb:='{}';reasons jsonb:='{}';r text;allowed boolean;feature text;reason text;begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 select * into s from cbgpu_schools.schools where id=target_school and status='APPROVED';
 context:=cbgpu_schools.resource_context(uid,target_school);
 foreach r in array array['gpu','study','school_storage','fabrication','school_compute','contests'] loop
 allowed:=cbgpu_accounts.can_user_use_resource(uid,r,target_school);
 feature:=case r when 'gpu' then 'gpu_compute' when 'study' then 'school_life' when 'school_storage' then 'materials' when 'fabrication' then 'manufacturing' when 'school_compute' then 'projects' when 'contests' then 'contests' end;
 reason:=case when allowed then 'ALLOWED' when s.id is null then 'SCHOOL_UNAVAILABLE' when context is null then 'NO_CONTEXT' when target_school<>'cbsh' and r in('gpu','study','fabrication','school_compute') then 'NOT_CONFIGURED' when not(feature=any(s.features)) or(r='study' and not('cbsh_eduset'=any(s.features))) then 'FEATURE_DISABLED' else 'NOT_GRANTED' end;
 resources:=resources||jsonb_build_object(r,allowed);reasons:=reasons||jsonb_build_object(r,reason);
 end loop;
 return jsonb_build_object('school_id',target_school,'legacy_cbsh_access',target_school='cbsh' and cbgpu_schools.legacy_cbsh_access(uid),'resources',resources,'reasons',reasons,'context_type',context->'type','expires_at',context->'expires_at','server_time',now());
end$$;
-- Preserve the existing identity/session/account guard and preference storage;
-- widen only the selection predicate, never membership verification or roles.
do $$declare definition text;updated text;begin
 definition:=pg_get_functiondef('public.cbgpu_school_api(text,jsonb)'::regprocedure);
 updated:=replace(definition,$old$if action='select_school' then if not cbgpu_schools.active_member(uid,sid) then raise exception 'ACTIVE_MEMBERSHIP_REQUIRED'$old$,$new$if action='select_school' then if not cbgpu_schools.context_available(uid,sid) then raise exception 'ACTIVE_RESOURCE_CONTEXT_REQUIRED'$new$);
 updated:=replace(updated,$old$from cbgpu_schools.preferences p where p.user_id=uid and cbgpu_schools.active_member(uid,p.school_id)$old$,$new$from cbgpu_schools.preferences p where p.user_id=uid and cbgpu_schools.context_available(uid,p.school_id)$new$);
 if updated=definition and position('ACTIVE_RESOURCE_CONTEXT_REQUIRED' in definition)=0 then raise exception 'SCHOOL_CONTEXT_API_VERSION_MISMATCH';end if;
 execute updated;
end$$;
revoke all on function cbgpu_schools.context_available(uuid,text),cbgpu_schools.resource_context(uuid,text) from public,anon,authenticated;
revoke all on function public.cbgpu_school_resource_contexts(),public.cbgpu_self_resource_access(text) from public,anon;
grant execute on function public.cbgpu_school_resource_contexts(),public.cbgpu_self_resource_access(text) to authenticated;
commit;
