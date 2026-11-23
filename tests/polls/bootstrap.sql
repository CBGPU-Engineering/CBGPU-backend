-- Disposable PostgreSQL only. Mirrors live poll columns, FKs, grants and RLS.
create role authenticated;create role anon;
create schema auth;create table auth.users(id uuid primary key);
create or replace function auth.uid() returns uuid language sql stable as $$select (nullif(current_setting('request.jwt.claims',true),'')::jsonb->>'sub')::uuid$$;
create or replace function auth.jwt() returns jsonb language sql stable as $$select nullif(current_setting('request.jwt.claims',true),'')::jsonb$$;
alter table auth.users add column if not exists deleted_at timestamptz;
alter table auth.users add column if not exists banned_until timestamptz;
create table if not exists auth.sessions(id uuid primary key,user_id uuid not null,not_after timestamptz);
create table public.profiles(id uuid primary key,is_admin boolean not null default false,account_status text not null default 'ACTIVE');
create schema cbgpu_accounts;
create or replace function cbgpu_accounts.current_session_valid() returns boolean language sql stable security definer set search_path='' as $$
 select exists(select 1 from auth.sessions s where s.user_id=auth.uid()
 and s.id=case when (auth.jwt()->>'session_id') ~ '^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$' then (auth.jwt()->>'session_id')::uuid end
 and(s.not_after is null or s.not_after>now()))
$$;
create or replace function public.cbgpu_personal_project_identity() returns boolean language sql stable security definer set search_path='' as $$select cbgpu_accounts.current_session_valid() and exists(select 1 from public.profiles p join auth.users u on u.id=p.id where p.id=auth.uid() and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now()))$$;
create table public.polls(id uuid primary key default gen_random_uuid(),created_at timestamptz default now(),title text not null,description text default '',ends_at timestamptz,is_active boolean default true,show_results boolean default true,created_by uuid references auth.users(id));
create table public.poll_questions(id uuid primary key default gen_random_uuid(),poll_id uuid references public.polls(id) on delete cascade not null,order_num integer not null default 0,type text not null,title text not null,description text default '',required boolean default false,options jsonb default '[]',config jsonb default '{}');
create table public.poll_responses(id uuid primary key default gen_random_uuid(),created_at timestamptz default now(),poll_id uuid references public.polls(id) on delete cascade not null,user_id uuid references auth.users(id) not null,unique(poll_id,user_id));
create table public.poll_answers(id uuid primary key default gen_random_uuid(),response_id uuid references public.poll_responses(id) on delete cascade not null,question_id uuid references public.poll_questions(id) on delete cascade not null,value jsonb not null default 'null');
alter table public.polls enable row level security;alter table public.poll_questions enable row level security;alter table public.poll_responses enable row level security;alter table public.poll_answers enable row level security;
create policy polls_read on public.polls for select using(true);
create policy polls_admin on public.polls for all using(exists(select 1 from public.profiles where id=auth.uid() and is_admin=true));
create policy questions_read on public.poll_questions for select using(true);
create policy questions_admin on public.poll_questions for all using(exists(select 1 from public.profiles where id=auth.uid() and is_admin=true));
create policy responses_read on public.poll_responses for select using(auth.uid() is not null);
create policy responses_insert on public.poll_responses for insert with check(user_id=auth.uid());
create policy answers_read on public.poll_answers for select using(auth.uid() is not null);
create policy answers_insert on public.poll_answers for insert with check(exists(select 1 from public.poll_responses where id=response_id and user_id=auth.uid()));
grant usage on schema public,auth to authenticated,anon;
grant all on public.polls,public.poll_questions,public.poll_responses,public.poll_answers to authenticated;
-- Live anon ACLs permit SELECT; policies, rather than missing grants, must deny internal poll rows.
grant select on public.polls,public.poll_questions,public.poll_responses,public.poll_answers to anon;
grant select on public.profiles to authenticated;
insert into auth.users(id) values('11111111-1111-4111-8111-111111111111'),('22222222-2222-4222-8222-222222222222');
insert into public.profiles(id,is_admin) values('11111111-1111-4111-8111-111111111111',true),('22222222-2222-4222-8222-222222222222',false);
insert into auth.sessions(id,user_id) values('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa','11111111-1111-4111-8111-111111111111'),('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb','22222222-2222-4222-8222-222222222222');
