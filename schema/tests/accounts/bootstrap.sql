-- Synthetic isolated PostgreSQL only; no production fixtures or personal data.
create role anon nologin;create role authenticated nologin;create role service_role nologin;
create schema auth;
create table auth.users(id uuid primary key,raw_app_meta_data jsonb default '{}',raw_user_meta_data jsonb default '{}',deleted_at timestamptz,banned_until timestamptz,email text unique);
create function auth.uid() returns uuid language sql stable as $$select nullif(current_setting('request.jwt.claim.sub',true),'')::uuid$$;
create function auth.role() returns text language sql stable as $$select nullif(current_setting('request.jwt.claim.role',true),'')$$;
create table public.profiles(id uuid primary key references auth.users(id),full_name text,student_id text,student_id_school_year integer,is_admin boolean default false,is_approved boolean);
create function public.is_admin(uid uuid) returns boolean language sql stable as $$select exists(select 1 from public.profiles where id=uid and is_admin)$$;
create function public.cbgpu_school_year(at_time timestamptz) returns integer language sql stable as $$select extract(year from at_time at time zone 'Asia/Seoul')::integer-case when extract(month from at_time at time zone 'Asia/Seoul')<3 then 1 else 0 end$$;
create function public.handle_new_user() returns trigger language plpgsql security definer as $$begin insert into public.profiles(id,full_name,student_id)values(new.id,new.raw_user_meta_data->>'full_name',new.raw_user_meta_data->>'student_id');return new;end$$;
create trigger on_auth_user_created after insert on auth.users for each row execute function public.handle_new_user();
create table auth.sessions(id uuid primary key,user_id uuid references auth.users(id),not_after timestamptz);
create function auth.test_session() returns trigger language plpgsql as $$begin insert into auth.sessions(id,user_id)values(new.id,new.id);return new;end$$;
create trigger test_session after insert on auth.users for each row execute function auth.test_session();
insert into auth.users(id,raw_user_meta_data,raw_app_meta_data,email) values
 ('00000000-0000-0000-0000-000000000001','{"full_name":"Admin","student_id":"1101"}','{}','admin@example.test'),
 ('00000000-0000-0000-0000-000000000002','{"full_name":"Grade two","student_id":"2102"}','{}','two@example.test'),
 ('00000000-0000-0000-0000-000000000003','{"full_name":"Grade three","student_id":"3103"}','{}','three@example.test'),
 ('00000000-0000-0000-0000-000000000004','{"full_name":"Teacher","student_id":"1104"}','{"cbgpu_account_type":"TEACHER"}','teacher@example.test');
update public.profiles set is_approved=true,student_id_school_year=2025;
update public.profiles set is_admin=true where id='00000000-0000-0000-0000-000000000001';
grant usage on schema auth to authenticated;grant execute on function auth.uid(),auth.role() to authenticated;
grant select,insert,update on public.profiles to authenticated;
create function public.guard_student_id_claim() returns trigger language plpgsql as $$begin return new;end$$;
create trigger guard_student_id_claim before insert or update of student_id,student_id_school_year on public.profiles for each row execute function public.guard_student_id_claim();


create function auth.jwt() returns jsonb language sql stable as $$select jsonb_build_object('session_id',coalesce(nullif(current_setting('request.jwt.claim.session_id',true),''),auth.uid()::text))$$;
create table public.reservations(id uuid primary key default gen_random_uuid(),user_id uuid references public.profiles(id),gpu_id integer,status text,start_time timestamptz,end_time timestamptz);
