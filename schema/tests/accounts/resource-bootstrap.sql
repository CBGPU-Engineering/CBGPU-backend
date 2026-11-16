-- Standalone synthetic fixtures for restrictive-policy mechanics.
create role authenticated nologin;create role anon nologin;
create schema auth;create schema cbgpu_accounts;create schema storage;
create function auth.uid() returns uuid language sql stable as $$select '00000000-0000-0000-0000-000000000001'::uuid$$;
create function auth.role() returns text language sql stable as $$select current_user::text$$;
create function public.cbgpu_can_use_resource(resource text,target_school text default 'cbsh') returns boolean language sql stable as $$select current_setting('test.resource',true)=resource$$;
create table public.cpu_projects(id int primary key,user_id uuid,status text,deploy_requested_at timestamptz,name text);
create table public.reservations(id int primary key,user_id uuid,status text,start_time timestamptz,end_time timestamptz);
create table public.print_jobs(id int primary key,user_id uuid);create table public.laser_jobs(id int primary key,user_id uuid);
create table storage.objects(id int primary key,bucket_id text);
grant usage on schema public,auth,storage to authenticated;
grant select,insert,update,delete on all tables in schema public,storage to authenticated;
do $$declare t text;begin foreach t in array array['cpu_projects','reservations','print_jobs','laser_jobs'] loop execute format('alter table public.%I enable row level security',t);execute format('create policy baseline on public.%I for all to authenticated using(true) with check(true)',t);end loop;end$$;
alter table storage.objects enable row level security;create policy baseline on storage.objects for all to authenticated using(true)with check(true);
