begin;
-- Inventory authority only. No membership/role, GPU booking, execution or sharing grant.
create or replace function cbgpu_schools.gpu_inventory_authorized(subject uuid,school text,real_session uuid)returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from cbgpu_schools.schools s join auth.users u on u.id=subject join public.profiles p on p.id=u.id join auth.sessions a on a.id=real_session and a.user_id=u.id
 where s.id=school and s.status='APPROVED'and u.email_confirmed_at is not null and u.deleted_at is null and(u.banned_until is null or u.banned_until<=clock_timestamp())and(a.not_after is null or a.not_after>clock_timestamp())and p.account_status<>'SUSPENDED'
 and(cbgpu_schools.has_role(subject,school,array['SCHOOL_OWNER','SCHOOL_ADMIN','GPU_ADMIN'])or(p.is_admin is true and p.account_status='ACTIVE'and p.is_approved is distinct from false)))
$$;
revoke all on function cbgpu_schools.gpu_inventory_authorized(uuid,text,uuid)from public,anon,authenticated,service_role;
do $upgrade$declare d text;old_role text;new_role text;begin
 d:=pg_get_functiondef('public.school_gpu_admin(text,text,jsonb)'::regprocedure);
 old_role:='cbgpu_schools.has_role(uid,p_school_id,array[''SCHOOL_OWNER'',''SCHOOL_ADMIN'',''GPU_ADMIN''])';
 new_role:='cbgpu_schools.gpu_inventory_authorized(uid,p_school_id,(auth.jwt()->>''session_id'')::uuid)';
 if position(new_role in d)=0 then
  if position(old_role in d)=0 then raise exception 'GPU_ADMIN_INVENTORY_SOURCE_MISMATCH';end if;
  d:=replace(d,old_role,new_role);execute d;
 end if;
 d:=pg_get_functiondef('public.school_gpu_enroll(text,text)'::regprocedure);
 old_role:='cbgpu_schools.has_role(enrollment.issuer_id,enrollment.school_id,array[''SCHOOL_OWNER'',''SCHOOL_ADMIN'',''GPU_ADMIN''])';
 new_role:='cbgpu_schools.gpu_inventory_authorized(enrollment.issuer_id,enrollment.school_id,enrollment.issuer_session_id)';
 if position(new_role in d)=0 then
  if position(old_role in d)=0 then raise exception 'GPU_REDEMPTION_INVENTORY_SOURCE_MISMATCH';end if;
  execute replace(d,old_role,new_role);
 end if;
end $upgrade$;
-- Existing public RPC ACLs are retained; the private helper is never a caller-supplied-UID API.
commit;
