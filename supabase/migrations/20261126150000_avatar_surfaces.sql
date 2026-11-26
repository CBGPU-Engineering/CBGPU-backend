begin;
-- Follow-up to default_avatars: put the same avatar rule on every place the database embeds a user,
-- allow avatarUrl (and only avatarUrl) across schools, and stop profiles.avatar_url from pointing anywhere
-- except the account's own file in our avatar storage.
--
-- Must be applied by the owner of the cbgpu_schools functions (supabase_admin). Re-running is a no-op.

-- 1) avatar_url may only be NULL or this account's own file in the public avatars bucket.
--    Edit the host here if the storage URL ever moves (drop and re-add the constraint).
do $$ begin
 if not exists(select 1 from pg_constraint where conname='profiles_avatar_url_storage_chk' and conrelid='public.profiles'::regclass) then
  alter table public.profiles add constraint profiles_avatar_url_storage_chk check(
   avatar_url is null or avatar_url ~ ('^https://cbgpu\.com/backend/storage/v1/object/public/avatars/'||id::text||'/avatar\.(jpe?g|png|webp)(\?v=[0-9]{1,16})?$'));
 end if;
end $$;

-- 2) Internal helper. cbgpu_schools is not exposed through PostgREST and clients have no USAGE on it, so only
--    the SECURITY DEFINER RPCs that already build these responses can call it.
--    avatarUrl is the only value shown to other people; profileImageUrl is filled only for the account itself
--    (the app needs it just to decide whether to offer "delete photo"). An account with no profile row gets the
--    default avatar of the nil seed so avatarUrl is never null.
create or replace function cbgpu_schools.avatar_fields(p_user uuid) returns jsonb
language sql stable security definer set search_path='' as $$
 select coalesce((
  select jsonb_build_object(
   'avatarUrl',coalesce(nullif(btrim(p.avatar_url),''),public.cbgpu_default_avatar_url(p.avatar_seed)),
   'profileImageUrl',case when p.id=auth.uid() then nullif(btrim(p.avatar_url),'') end)
  from public.profiles p where p.id=p_user),
  jsonb_build_object('avatarUrl',public.cbgpu_default_avatar_url('00000000-0000-0000-0000-000000000000'::uuid),'profileImageUrl',null))
$$;
revoke all on function cbgpu_schools.avatar_fields(uuid) from public,anon,authenticated,service_role;

-- 3) Wrap the response builders instead of copying their bodies: the original is renamed to <name>_core and a
--    function with the old name and the SAME security mode returns the original JSON plus avatarUrl/profileImageUrl.
--    Existing callers resolve the name at run time, so nothing else changes. Same owner-only execute rights as before.
do $$
declare spec record;
begin
 for spec in select * from (values
   ('message_json','cbgpu_schools.messages',false),
   ('dm_message_json','cbgpu_schools.direct_messages',true),
   ('member_json','cbgpu_schools.memberships',true),
   ('operator_nomination','cbgpu_schools.contest_operator_nominations',true)) as t(fn,argtype,definer)
 loop
  if to_regprocedure('cbgpu_schools.'||spec.fn||'_core('||spec.argtype||')') is null then
   execute format('alter function cbgpu_schools.%I(%s) rename to %I',spec.fn,spec.argtype,spec.fn||'_core');
  end if;
  execute format('create or replace function cbgpu_schools.%1$I(r %2$s) returns jsonb language sql stable %3$s set search_path='''' as $f$select cbgpu_schools.%4$I(r)||cbgpu_schools.avatar_fields(r.user_id)$f$',
   spec.fn,spec.argtype,case when spec.definer then 'security definer' else '' end,spec.fn||'_core');
  execute format('revoke all on function cbgpu_schools.%I(%s) from public,anon,authenticated,service_role',spec.fn,spec.argtype);
 end loop;
 -- Admin GPU bookings carry the owner in the row JSON.
 if to_regprocedure('cbgpu_schools.gpu_admin_booking_item_core(text,jsonb)') is null then
  alter function cbgpu_schools.gpu_admin_booking_item(text,jsonb) rename to gpu_admin_booking_item_core;
 end if;
 create or replace function cbgpu_schools.gpu_admin_booking_item(school text,booking_row jsonb) returns jsonb
 language sql stable security definer set search_path='' as $f$
  select cbgpu_schools.gpu_admin_booking_item_core(school,booking_row)||cbgpu_schools.avatar_fields(nullif(booking_row->>'owner_id','')::uuid)
 $f$;
 revoke all on function cbgpu_schools.gpu_admin_booking_item(text,jsonb) from public,anon,authenticated,service_role;
end $$;

-- 4) Avatar-only lookup for screens where the viewer's school rules hide the person's name (study members,
--    mentions, rosters). Returns nothing but the id and the final avatarUrl; names, student numbers and schools
--    stay behind their existing RLS/RPC boundaries. Callers need a valid signed-in session.
create or replace function public.cbgpu_avatar_urls(p_user_ids uuid[]) returns table(id uuid,"avatarUrl" text)
language plpgsql stable security definer set search_path='' as $$
begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501'; end if;
 if p_user_ids is null or cardinality(p_user_ids)>500 or array_position(p_user_ids,null) is not null then raise exception 'INVALID_AVATAR_REQUEST'; end if;
 return query select p.id,coalesce(nullif(btrim(p.avatar_url),''),public.cbgpu_default_avatar_url(p.avatar_seed))
  from public.profiles p where p.id=any(p_user_ids);
end$$;
revoke all on function public.cbgpu_avatar_urls(uuid[]) from public,anon;
grant execute on function public.cbgpu_avatar_urls(uuid[]) to authenticated,service_role;
commit;
