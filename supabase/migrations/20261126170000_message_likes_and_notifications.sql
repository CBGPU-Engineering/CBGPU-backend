begin;
-- 1) Likes ("공감") on community messages and replies, and 2) a per-account notification inbox.
-- Must be applied by the owner of the cbgpu_schools objects (supabase_admin). Re-running is a no-op.

-- ---------------------------------------------------------------------------------------------
-- Likes. Counts are public to anyone who can read the message; who liked is never exposed, because
-- community authors are intentionally anonymous.
-- ---------------------------------------------------------------------------------------------
create table if not exists cbgpu_schools.message_likes(
 message_id uuid not null references cbgpu_schools.messages(id) on delete cascade,
 user_id uuid not null references auth.users(id) on delete cascade,
 created_at timestamptz not null default now(),
 primary key(message_id,user_id)
);
create index if not exists message_likes_user_recent on cbgpu_schools.message_likes(user_id,created_at desc);
alter table cbgpu_schools.message_likes enable row level security;
revoke all on cbgpu_schools.message_likes from public,anon,authenticated;

-- Same rule as "view this message": the root must exist, be undeleted and allowed for the caller's community
-- scope; a reply is judged through its root; a muted author hides the message. (Mirrors cbgpu_school_messenger_api.)
create or replace function cbgpu_schools.message_reactable(p_user uuid,p_message uuid) returns boolean
language plpgsql stable security definer set search_path='' as $$
declare r cbgpu_schools.messages;p cbgpu_schools.messages;
begin
 if p_user is null or p_message is null then return false; end if;
 select * into r from cbgpu_schools.messages where id=p_message and deleted_at is null;
 if not found then return false; end if;
 if r.parent_id is null then p:=r;
 else
  select * into p from cbgpu_schools.messages where id=r.parent_id and parent_id is null and deleted_at is null;
  if not found then return false; end if;
 end if;
 return cbgpu_schools.community_allowed(p_user,p.scope,p.school_id)
  and not exists(select from cbgpu_schools.message_mutes x where x.user_id=p_user and x.muted_user_id in(p.user_id,r.user_id));
end$$;

create or replace function cbgpu_schools.like_fields(p_message uuid) returns jsonb
language sql stable security definer set search_path='' as $$
 select jsonb_build_object(
  'like_count',(select count(*)::integer from cbgpu_schools.message_likes l where l.message_id=p_message),
  'liked_by_me',coalesce(exists(select from cbgpu_schools.message_likes l where l.message_id=p_message and l.user_id=auth.uid()),false))
$$;
revoke all on function cbgpu_schools.message_reactable(uuid,uuid),cbgpu_schools.like_fields(uuid) from public,anon,authenticated,service_role;

-- Every community message JSON (list, detail, replies) now carries like_count and liked_by_me. This redefines the
-- wrapper created by 20261126150000; it still delegates to message_json_core and keeps the original security mode.
create or replace function cbgpu_schools.message_json(r cbgpu_schools.messages) returns jsonb
language sql stable set search_path='' as $$
 select cbgpu_schools.message_json_core(r)||cbgpu_schools.avatar_fields(r.user_id)||cbgpu_schools.like_fields(r.id)
$$;
revoke all on function cbgpu_schools.message_json(cbgpu_schools.messages) from public,anon,authenticated,service_role;

create or replace function public.cbgpu_message_like_set(p_message_id uuid,p_liked boolean) returns jsonb
language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();n integer;
begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501'; end if;
 if p_message_id is null or p_liked is null then raise exception 'INVALID_REQUEST' using errcode='22023'; end if;
 if not cbgpu_schools.message_reactable(uid,p_message_id) then raise exception 'MESSAGE_NOT_FOUND' using errcode='P0002'; end if;
 if p_liked then
  if(select count(*) from cbgpu_schools.message_likes where user_id=uid and created_at>now()-interval '1 minute')>=60 then
   raise exception 'LIKE_RATE_LIMIT' using errcode='54000';
  end if;
  insert into cbgpu_schools.message_likes(message_id,user_id) values(p_message_id,uid) on conflict do nothing;
 else
  delete from cbgpu_schools.message_likes where message_id=p_message_id and user_id=uid;
 end if;
 select count(*)::integer into n from cbgpu_schools.message_likes where message_id=p_message_id;
 return jsonb_build_object('message_id',p_message_id,'liked',p_liked,'like_count',n);
end$$;

-- Counts for a screen that already has message ids (other lists, notifications); only readable messages are answered.
create or replace function public.cbgpu_message_likes(p_ids uuid[]) returns table(message_id uuid,like_count integer,liked_by_me boolean)
language plpgsql stable security definer set search_path='' as $$
declare uid uuid:=auth.uid();
begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501'; end if;
 if p_ids is null or cardinality(p_ids)>200 or array_position(p_ids,null) is not null then raise exception 'INVALID_REQUEST' using errcode='22023'; end if;
 return query select m.id,
   (select count(*)::integer from cbgpu_schools.message_likes l where l.message_id=m.id),
   exists(select from cbgpu_schools.message_likes l where l.message_id=m.id and l.user_id=uid)
  from cbgpu_schools.messages m where m.id=any(p_ids) and cbgpu_schools.message_reactable(uid,m.id);
end$$;
revoke all on function public.cbgpu_message_like_set(uuid,boolean),public.cbgpu_message_likes(uuid[]) from public,anon;
grant execute on function public.cbgpu_message_like_set(uuid,boolean),public.cbgpu_message_likes(uuid[]) to authenticated;

-- ---------------------------------------------------------------------------------------------
-- Notification inbox. Rows are written only by the server (cbgpu_notify, service role) at the moment a push is
-- decided, so the list is complete even if the app was closed, the account had no registered device, or the
-- push failed. A person reads and deletes only their own rows; read marks go through functions.
-- ---------------------------------------------------------------------------------------------
create table if not exists public.notifications(
 id uuid primary key default gen_random_uuid(),
 user_id uuid not null references auth.users(id) on delete cascade,
 kind text not null check(kind ~ '^[a-z0-9_.-]{1,40}$'),
 title text not null check(char_length(title) between 1 and 120),
 body text not null default '' check(char_length(body)<=700),
 route text check(route is null or(char_length(route)<=300 and(route ~ '^/([^/\\]|$)' or route ~ '^https://[a-z0-9.-]+\.cbgpu\.com/'))),
 data jsonb not null default '{}' check(jsonb_typeof(data)='object' and octet_length(data::text)<=2048),
 dedupe_key text check(dedupe_key is null or char_length(dedupe_key)<=200),
 created_at timestamptz not null default clock_timestamp(),  -- distinct even inside one transaction, so "oldest first" is well defined
 read_at timestamptz
);
create unique index if not exists notifications_dedupe on public.notifications(user_id,dedupe_key) where dedupe_key is not null;
create index if not exists notifications_user_recent on public.notifications(user_id,created_at desc);
create index if not exists notifications_user_unread on public.notifications(user_id) where read_at is null;
alter table public.notifications enable row level security;
revoke all on public.notifications from public,anon,authenticated;
grant select,delete on public.notifications to authenticated;
drop policy if exists notifications_own_read on public.notifications;
create policy notifications_own_read on public.notifications for select to authenticated
 using(user_id=auth.uid() and public.cbgpu_personal_project_identity());
drop policy if exists notifications_own_delete on public.notifications;
create policy notifications_own_delete on public.notifications for delete to authenticated
 using(user_id=auth.uid() and public.cbgpu_personal_project_identity());

create or replace function public.cbgpu_notify(p_user_id uuid,p_kind text,p_title text,p_body text default '',p_route text default null,
 p_data jsonb default '{}',p_dedupe_key text default null) returns uuid
language plpgsql security definer set search_path='' as $$
declare nid uuid;
begin
 insert into public.notifications(user_id,kind,title,body,route,data,dedupe_key)
  values(p_user_id,p_kind,p_title,coalesce(p_body,''),p_route,coalesce(p_data,'{}'::jsonb),p_dedupe_key)
  on conflict(user_id,dedupe_key) where dedupe_key is not null do nothing
  returning id into nid;
 -- Bounded history: 90 days and at most 500 rows per account.
 delete from public.notifications where user_id=p_user_id and created_at<now()-interval '90 days';
 delete from public.notifications where id in(select id from public.notifications where user_id=p_user_id order by created_at desc,id desc offset 500);
 return nid;
end$$;
revoke all on function public.cbgpu_notify(uuid,text,text,text,text,jsonb,text) from public,anon,authenticated;
grant execute on function public.cbgpu_notify(uuid,text,text,text,text,jsonb,text) to service_role;

create or replace function public.cbgpu_notifications_mark_read(p_ids uuid[] default null) returns integer
language plpgsql security definer set search_path='' as $$
declare n integer;
begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501'; end if;
 if p_ids is not null and cardinality(p_ids)>500 then raise exception 'INVALID_REQUEST' using errcode='22023'; end if;
 update public.notifications set read_at=now() where user_id=auth.uid() and read_at is null and(p_ids is null or id=any(p_ids));
 get diagnostics n=row_count;
 return n;
end$$;
create or replace function public.cbgpu_notifications_unread_count() returns integer
language plpgsql stable security definer set search_path='' as $$
begin
 if not public.cbgpu_personal_project_identity() then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501'; end if;
 return(select count(*)::integer from public.notifications where user_id=auth.uid() and read_at is null);
end$$;
revoke all on function public.cbgpu_notifications_mark_read(uuid[]),public.cbgpu_notifications_unread_count() from public,anon;
grant execute on function public.cbgpu_notifications_mark_read(uuid[]),public.cbgpu_notifications_unread_count() to authenticated;
commit;
