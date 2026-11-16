\set ON_ERROR_STOP on
reset role;
select set_config('request.jwt.claim.role','authenticated',false);
select set_config('request.jwt.claim.session_id','',false);
insert into auth.users(id,email,raw_user_meta_data)values
 ('00000000-0000-0000-0000-000000000010','sponsor-new@example.test','{"full_name":"Sponsor","account_type_request":"EXTERNAL"}'),
 ('00000000-0000-0000-0000-000000000011','expired-new@example.test','{"full_name":"Expired guest","account_type_request":"EXTERNAL"}'),
 ('00000000-0000-0000-0000-000000000012','eligible-new@example.test','{"full_name":"Eligible","account_type_request":"EXTERNAL"}'),
 ('00000000-0000-0000-0000-000000000013','denied-new@example.test','{"full_name":"Denied guest","account_type_request":"EXTERNAL"}');
update profiles set account_type='STUDENT',account_status='ACTIVE',school_id='cbsh',affiliation_verified=true,is_approved=true,student_id=case when right(id::text,2)='10' then '1110' else '1112' end where id in('00000000-0000-0000-0000-000000000010','00000000-0000-0000-0000-000000000012');
create table public.event_registrations(id uuid primary key default gen_random_uuid(),contest_id uuid not null,user_id uuid not null,status text not null,created_at timestamptz default now());
create table public.contest_submissions(id uuid primary key default gen_random_uuid(),contest_id uuid,user_id uuid);
create schema storage;create table storage.objects(id uuid default gen_random_uuid(),bucket_id text);
alter table event_registrations enable row level security;alter table contest_submissions enable row level security;alter table storage.objects enable row level security;
create policy baseline on event_registrations for all to authenticated using(true)with check(true);
create policy baseline on contest_submissions for all to authenticated using(true)with check(true);
create policy baseline on storage.objects for all to authenticated using(true)with check(true);
grant usage on schema storage to authenticated;grant select,insert,update on event_registrations,contest_submissions,storage.objects to authenticated;
-- A deliberately privileged wrapper demonstrates the same execution context as the production RPC.
create function public.test_definer_register(target uuid)returns void language sql security definer as $$insert into public.event_registrations(contest_id,user_id,status)values('10000000-0000-0000-0000-000000000010',target,'confirmed')$$;
