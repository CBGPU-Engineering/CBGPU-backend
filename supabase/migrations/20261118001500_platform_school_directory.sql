begin;
-- Platform inventory only. School-admin roles never imply platform-wide visibility.
create or replace function public.cbgpu_platform_school_directory(payload jsonb default '{}'::jsonb)
returns jsonb language plpgsql stable security definer set search_path='' as $$
declare uid uuid:=auth.uid();q text;wanted text;lim integer;off integer;items jsonb;total bigint;
begin
 if uid is null or (public.cbgpu_self_session_status()->>'valid')::boolean is distinct from true then raise exception 'FRESH_SESSION_REQUIRED' using errcode='42501';end if;
 if not exists(select 1 from public.profiles p join auth.users u on u.id=p.id where p.id=uid and p.is_admin is true and p.account_status<>'SUSPENDED' and u.email_confirmed_at is not null and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now()))then raise exception 'PLATFORM_ADMIN_REQUIRED'using errcode='42501';end if;
 if jsonb_typeof(payload) is distinct from 'object' or exists(select 1 from jsonb_object_keys(payload)k where k not in('q','status','limit','offset'))then raise exception 'INVALID_DIRECTORY_FILTER'using errcode='22023';end if;
 q:=btrim(coalesce(payload->>'q',''));wanted:=nullif(payload->>'status','');
 if length(q)>150 or(wanted is not null and wanted not in('LISTED','UNCLAIMED','APPROVED','SUSPENDED'))or coalesce(payload->>'limit','30')!~'^[0-9]{1,3}$'or coalesce(payload->>'offset','0')!~'^[0-9]{1,5}$'then raise exception 'INVALID_DIRECTORY_FILTER'using errcode='22023';end if;
 lim:=coalesce(payload->>'limit','30')::integer;off:=coalesce(payload->>'offset','0')::integer;
 if lim not between 1 and 100 or off not between 0 and 10000 then raise exception 'INVALID_DIRECTORY_FILTER'using errcode='22023';end if;
 select coalesce(jsonb_agg(cbgpu_schools.summary(s) order by s.display_name,s.id),'[]'::jsonb)into items from(select * from cbgpu_schools.schools where(wanted is null or status=wanted)and(q=''or official_name ilike '%'||q||'%'or display_name ilike '%'||q||'%'or coalesce(official_id,'')=q)order by display_name,id limit lim offset off)s;
 select count(*)into total from cbgpu_schools.schools where(wanted is null or status=wanted)and(q=''or official_name ilike '%'||q||'%'or display_name ilike '%'||q||'%'or coalesce(official_id,'')=q);
 return jsonb_build_object('items',items,'total',total);
end$$;
revoke all on function public.cbgpu_platform_school_directory(jsonb) from public,anon,service_role;
grant execute on function public.cbgpu_platform_school_directory(jsonb) to authenticated;
notify pgrst,'reload schema';
commit;
