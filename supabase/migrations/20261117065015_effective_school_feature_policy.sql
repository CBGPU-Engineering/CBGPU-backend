begin;
-- Configuration-time validation is insufficient: global policy can change
-- after a school enables a module. Evaluate the registry on every request.
create or replace function cbgpu_schools.feature_enabled(school text,feature text) returns boolean language sql stable security definer set search_path='' as $$
select exists(select 1 from cbgpu_schools.schools s join cbgpu_schools.feature_registry f on f.key=feature where s.id=school and s.status='APPROVED' and feature=any(s.features) and f.enabled and(f.globally_available or school=any(f.allowed_school_ids)))
$$;
revoke all on function cbgpu_schools.feature_enabled(text,text) from public,anon,authenticated;
create or replace function public.cbgpu_school_feature_enabled(school text,feature text) returns boolean language sql stable security definer set search_path='' as $$select cbgpu_schools.feature_enabled(school,feature)$$;
revoke all on function public.cbgpu_school_feature_enabled(text,text) from public;
grant execute on function public.cbgpu_school_feature_enabled(text,text) to anon,authenticated;
create or replace function cbgpu_accounts.can_user_use_resource(subject_id uuid,resource text,target_school text default 'cbsh') returns boolean language sql stable security definer set search_path='' as $$
select case when resource='ai' then cbgpu_schools.feature_enabled(target_school,'ai') and cbgpu_accounts.can_user_use_resource(subject_id,'gpu',target_school)
else exists(select 1 from cbgpu_schools.schools s join public.profiles p on p.id=subject_id join auth.users u on u.id=p.id where s.id=target_school and s.status='APPROVED' and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now())
 and case resource when 'study' then s.id='cbsh' and cbgpu_schools.feature_enabled(s.id,'school_life') and cbgpu_schools.feature_enabled(s.id,'cbsh_eduset') when 'gpu' then cbgpu_schools.feature_enabled(s.id,'gpu_compute') when 'school_compute' then cbgpu_schools.feature_enabled(s.id,'projects') when 'fabrication' then cbgpu_schools.feature_enabled(s.id,'manufacturing') when 'school_storage' then cbgpu_schools.feature_enabled(s.id,'materials') when 'contests' then cbgpu_schools.feature_enabled(s.id,'contests') else false end
 and(resource not in('gpu','school_compute','fabrication') or s.id='cbsh')
 and(exists(select 1 from cbgpu_schools.memberships m where m.user_id=subject_id and m.school_id=s.id and m.membership_type in('STUDENT','TEACHER') and cbgpu_schools.active_member(subject_id,s.id))
 or exists(select 1 from cbgpu_accounts.guest_requests g where g.external_user_id=subject_id and g.school_id=s.id and g.status='APPROVED' and g.starts_at<=now() and g.ends_at>now() and resource=any(g.resources) and cbgpu_schools.active_member(g.requester_id,s.id) and exists(select 1 from cbgpu_schools.memberships sponsor where sponsor.user_id=g.requester_id and sponsor.school_id=s.id and sponsor.membership_type in('STUDENT','TEACHER'))))) end
$$;
-- Existing public/own content remains durable. School-shared material visibility
-- follows module availability; the owner keeps their private records/files.
drop policy if exists effective_material_module on public.shared_materials;
create policy effective_material_module on public.shared_materials as restrictive for select to anon,authenticated using(uploader_id=auth.uid() or public.cbgpu_school_feature_enabled(school_id,'materials'));
-- Reuse actual existing handlers, preserving their fresh identity, scope and
-- private/public ownership predicates. Refuse an unexpected source version.
do $$declare signature text;definition text;updated text;begin
 foreach signature in array array['cbgpu_schools.contest_register(uuid,uuid)','cbgpu_schools.community_allowed(uuid,text,text)','cbgpu_schools.life_capabilities(text)','public.cbgpu_school_life_sources(text)'] loop
 definition:=pg_get_functiondef(signature::regprocedure);updated:=definition;
 updated:=replace(updated,$a$'contests'=any(s.features)$a$,$b$cbgpu_schools.feature_enabled(s.id,'contests')$b$);
 updated:=replace(updated,$a$'community'=any(s.features)$a$,$b$cbgpu_schools.feature_enabled(s.id,'community')$b$);
 updated:=replace(updated,$a$'school_life'=any(s.features)$a$,$b$cbgpu_schools.feature_enabled(s.id,'school_life')$b$);
 updated:=replace(updated,$a$when scope in('PUBLIC','AUTHENTICATED') then school is null$a$,$b$when scope in('PUBLIC','AUTHENTICATED') then exists(select 1 from cbgpu_schools.feature_registry public_module where public_module.key='community' and public_module.enabled and public_module.globally_available) and school is null$b$);
 if updated=definition and position('cbgpu_schools.feature_enabled' in definition)=0 then raise exception 'EFFECTIVE_FEATURE_HANDLER_VERSION_MISMATCH';end if;
 execute updated;
 end loop;
 -- Add derived AI capability to self/context DTOs, never to guest grant scopes.
 definition:=pg_get_functiondef('cbgpu_schools.resource_context(uuid,text)'::regprocedure);
 updated:=replace(definition,$a$'school_compute','contests']$a$,$b$'school_compute','contests','ai']$b$);
 updated:=replace(updated,$a$'features',s.features$a$,$b$'features',array(select f.key from unnest(s.features) as f(key) where cbgpu_schools.feature_enabled(s.id,f.key) order by f.key)$b$);execute updated;
end$$;
create or replace function public.cbgpu_self_resource_access(target_school text default 'cbsh') returns jsonb language plpgsql stable security definer set search_path='' as $$
declare uid uuid:=auth.uid();s cbgpu_schools.schools;context jsonb;resources jsonb:='{}';reasons jsonb:='{}';r text;allowed boolean;feature text;reason text;begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 select * into s from cbgpu_schools.schools where id=target_school and status='APPROVED';context:=cbgpu_schools.resource_context(uid,target_school);
 foreach r in array array['gpu','study','school_storage','fabrication','school_compute','contests','ai'] loop
 allowed:=cbgpu_accounts.can_user_use_resource(uid,r,target_school);
 feature:=case r when 'gpu' then 'gpu_compute' when 'ai' then 'ai' when 'study' then 'school_life' when 'school_storage' then 'materials' when 'fabrication' then 'manufacturing' when 'school_compute' then 'projects' when 'contests' then 'contests' end;
 reason:=case when allowed then 'ALLOWED' when s.id is null then 'SCHOOL_UNAVAILABLE' when context is null then 'NO_CONTEXT' when target_school<>'cbsh' and r in('gpu','ai','study','fabrication','school_compute') then 'NOT_CONFIGURED' when not cbgpu_schools.feature_enabled(target_school,feature) or(r='study' and not cbgpu_schools.feature_enabled(target_school,'cbsh_eduset')) or(r='ai' and not cbgpu_schools.feature_enabled(target_school,'gpu_compute')) then 'FEATURE_DISABLED' else 'NOT_GRANTED' end;
 resources:=resources||jsonb_build_object(r,allowed);reasons:=reasons||jsonb_build_object(r,reason);
 end loop;
 return jsonb_build_object('school_id',target_school,'legacy_cbsh_access',target_school='cbsh' and cbgpu_schools.legacy_cbsh_access(uid),'resources',resources,'reasons',reasons,'context_type',context->'type','expires_at',context->'expires_at','server_time',now());
end$$;
commit;
