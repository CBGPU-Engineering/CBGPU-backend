-- Disposable PostgreSQL17 fixture only: no FCM network calls or operational tokens.
create role anon nologin;create role authenticated nologin;create role service_role nologin bypassrls;
create schema extensions;create extension pgcrypto with schema extensions;
create schema auth;
create table auth.users(id uuid primary key,deleted_at timestamptz,banned_until timestamptz);
create table auth.sessions(id uuid primary key,user_id uuid references auth.users(id),not_after timestamptz);
create function auth.uid()returns uuid language sql stable as $$select nullif(current_setting('request.jwt.claim.sub',true),'')::uuid$$;
create function auth.role()returns text language sql stable as $$select nullif(current_setting('request.jwt.claim.role',true),'')$$;
create function auth.jwt()returns jsonb language sql stable as $$select jsonb_build_object('session_id',nullif(current_setting('request.jwt.claim.session_id',true),''))$$;
create table profiles(id uuid primary key references auth.users(id),account_status text default 'ACTIVE');
create table device_tokens(id uuid primary key default gen_random_uuid(),token text unique not null,user_id uuid not null references profiles(id),platform text not null check(platform in('android','ios','web')));
alter table device_tokens enable row level security;create policy own on device_tokens for all to authenticated using(auth.uid()=user_id)with check(auth.uid()=user_id);
grant usage on schema auth to authenticated;grant select,insert,update,delete on device_tokens to authenticated,service_role;
insert into auth.users(id)values('00000000-0000-4000-8000-000000000001'),('00000000-0000-4000-8000-000000000002');
insert into profiles(id)select id from auth.users;insert into auth.sessions(id,user_id)select id,id from auth.users;
insert into device_tokens(token,user_id,platform)values('legacy-kept','00000000-0000-4000-8000-000000000001','android');
