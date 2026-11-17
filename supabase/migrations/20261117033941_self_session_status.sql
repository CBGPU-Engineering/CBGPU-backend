begin;
-- Standalone compatibility primitive: no new account/school columns are required.
-- Token subject/session are derived from verified PostgREST JWT claims only.
create or replace function public.cbgpu_self_session_status() returns jsonb language plpgsql stable security definer set search_path='' as $$
declare uid uuid:=auth.uid();sid uuid;profile jsonb;approved boolean;membership_approved boolean;begin
 if uid is null or coalesce(auth.jwt()->>'session_id','') !~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' then return jsonb_build_object('valid',false,'approved',false,'school_id',null);end if;
 sid:=(auth.jwt()->>'session_id')::uuid;
 select to_jsonb(p) into profile from auth.users u join auth.sessions s on s.user_id=u.id join public.profiles p on p.id=u.id where u.id=uid and s.id=sid and(s.not_after is null or s.not_after>now()) and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now()) and coalesce(to_jsonb(p)->>'account_status','ACTIVE')<>'SUSPENDED';
 if not found then return jsonb_build_object('valid',false,'approved',false,'school_id',null);end if;
 approved:=(profile->>'is_approved')::boolean is distinct from false;
 -- Optional new affiliation fields may narrow, never expand, legacy CBsh access.
 if profile?'account_status' then approved:=approved and profile->>'account_status'='ACTIVE';end if;
 if profile?'school_id' then approved:=approved and profile->>'school_id'='cbsh';end if;
 if profile?'affiliation_verified' then approved:=approved and profile->'affiliation_verified'='true'::jsonb;end if;
 if approved and to_regprocedure('public.cbgpu_school_scope_read(text,text,text[])') is not null then
 execute 'select public.cbgpu_school_scope_read($1,$2,$3)' into membership_approved using 'cbsh','SCHOOL','{}'::text[];approved:=approved and coalesce(membership_approved,false);
 end if;
 approved:=coalesce(approved,false);
 return jsonb_build_object('valid',true,'approved',approved,'school_id',case when approved then 'cbsh' else null end);
end$$;
revoke all on function public.cbgpu_self_session_status() from public,anon;
grant execute on function public.cbgpu_self_session_status() to authenticated;
commit;
