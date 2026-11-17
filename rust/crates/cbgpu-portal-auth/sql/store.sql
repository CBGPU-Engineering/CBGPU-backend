-- Explicit bootstrap on a dedicated private runtime PostgreSQL database only.
-- Never run this against the hosted Auth schema or automatically at API startup.
create schema if not exists portal_auth;
revoke all on schema portal_auth from public;
create table if not exists portal_auth.sessions(scope text not null,token_hash text not null,csrf text not null,expires bigint not null,user_id uuid not null,email text not null,auth_version text not null,auth_session_id uuid not null,auth_tokens text not null,primary key(scope,token_hash));
create index if not exists portal_sessions_expiry on portal_auth.sessions(expires);
create table if not exists portal_auth.failures(scope text not null,key text not null,count integer not null,expires bigint not null,primary key(scope,key));
create table if not exists portal_auth.import_receipts(scope text primary key,fingerprint text not null,session_count bigint not null,failure_count bigint not null,imported_at timestamptz not null default now());
create table if not exists portal_auth.audit(id bigserial primary key,at timestamptz not null default now(),actor_id uuid,action text not null,outcome text not null);
revoke all on all tables in schema portal_auth from public;
do $$begin
 if not exists(select 1 from pg_roles where rolname='cbgpu_portal_session_store') then create role cbgpu_portal_session_store nologin noinherit nosuperuser nobypassrls;
 elsif exists(select 1 from pg_roles where rolname='cbgpu_portal_session_store' and(rolcanlogin or rolinherit or rolsuper or rolbypassrls)) then raise exception 'UNSAFE_PORTAL_STORE_ROLE';end if;
end$$;
grant usage on schema portal_auth to cbgpu_portal_session_store;
grant select,insert,update,delete on all tables in schema portal_auth to cbgpu_portal_session_store;
grant usage,select on all sequences in schema portal_auth to cbgpu_portal_session_store;
alter table portal_auth.sessions enable row level security;
alter table portal_auth.failures enable row level security;
alter table portal_auth.import_receipts enable row level security;
alter table portal_auth.audit enable row level security;
do $$declare t text;begin foreach t in array array['sessions','failures','import_receipts','audit'] loop
 if not exists(select 1 from pg_policies where schemaname='portal_auth' and tablename=t and policyname='portal_native_store') then execute format('create policy portal_native_store on portal_auth.%I to cbgpu_portal_session_store using(true) with check(true)',t);end if;
end loop;end$$;
