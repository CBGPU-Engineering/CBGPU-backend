-- Additive native delivery capability; null preserves all existing Flutter/web behavior.
begin;
alter table public.device_tokens add column if not exists native_kind text;
do $$begin if not exists(select 1 from pg_constraint where conrelid='public.device_tokens'::regclass and conname='device_tokens_native_kind_check')then
 alter table public.device_tokens add constraint device_tokens_native_kind_check check(native_kind is null or(native_kind='android-native-v1'and platform='android'));
end if;end$$;
create schema if not exists cbgpu_push;
revoke all on schema cbgpu_push from public,anon,authenticated;
create table if not exists cbgpu_push.native_capabilities(token_hash text primary key check(token_hash ~ '^[0-9a-f]{64}$'),native_kind text not null default 'android-native-v1'check(native_kind='android-native-v1'),registered_at timestamptz not null default now());
alter table cbgpu_push.native_capabilities enable row level security;
revoke all on cbgpu_push.native_capabilities from public,anon,authenticated,service_role;
create or replace function cbgpu_push.guard_native_registration() returns trigger
language plpgsql security definer set search_path='' as $$
declare sid text:=auth.jwt()->>'session_id';actor uuid:=auth.uid();device public.device_tokens;fingerprint text;known boolean;
begin
 if tg_op='DELETE' then device:=old;else device:=new;end if;
 fingerprint:=encode(extensions.digest(device.token,'sha256'),'hex');
 select exists(select 1 from cbgpu_push.native_capabilities c where c.token_hash=fingerprint) into known;
 -- A former native installation must never be reintroduced as an OS-rendered
 -- legacy notification target, including after deletion/logout and re-insert.
 if tg_op<>'DELETE' and known and device.native_kind is distinct from 'android-native-v1' then raise exception 'NATIVE_PUSH_CAPABILITY_REQUIRED' using errcode='42501';end if;
 if device.native_kind is null and not known then return device;end if;
 if auth.role() is distinct from 'service_role' then
  if actor is null or device.user_id is distinct from actor or sid is null or sid!~'^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' then raise exception 'NATIVE_PUSH_SESSION_REQUIRED' using errcode='42501';end if;
  if not exists(select 1 from auth.sessions s join auth.users u on u.id=s.user_id join public.profiles p on p.id=u.id
  where s.id=sid::uuid and s.user_id=actor and(s.not_after is null or s.not_after>now())
  and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now())and coalesce(to_jsonb(p)->>'account_status','ACTIVE')<>'SUSPENDED')then
   raise exception 'NATIVE_PUSH_SESSION_REVOKED' using errcode='42501';end if;
 end if;
 if tg_op<>'DELETE' and device.native_kind='android-native-v1' then
  insert into cbgpu_push.native_capabilities(token_hash)values(fingerprint)on conflict(token_hash)do nothing;
 end if;
 return device;
end$$;
revoke all on function cbgpu_push.guard_native_registration() from public,anon,authenticated,service_role;
drop trigger if exists guard_native_push_registration on public.device_tokens;
create trigger guard_native_push_registration before insert or update or delete on public.device_tokens for each row execute function cbgpu_push.guard_native_registration();
comment on column public.device_tokens.native_kind is 'Delivery-format capability only. android-native-v1 requires owner-bound data-only FCM and native session filtering. NULL retains legacy notification payload.';
commit;
