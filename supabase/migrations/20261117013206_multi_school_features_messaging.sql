begin;
create table if not exists cbgpu_schools.feature_requests(id uuid primary key default gen_random_uuid(),school_id text not null references cbgpu_schools.schools,requester_id uuid not null references auth.users,feature_key text not null,title text not null,description text not null,status text not null default 'PENDING' check(status in('PENDING','REVIEWING','ACCEPTED','REJECTED','DELIVERED')),created_at timestamptz not null default now());
create index if not exists feature_requests_school on cbgpu_schools.feature_requests(school_id,created_at desc);
-- Durable mail intent, not a claim that email has been sent. Existing mail worker
-- must acknowledge delivery; retry cannot alter request or enable any module.
create table if not exists cbgpu_schools.mail_outbox(id uuid primary key default gen_random_uuid(),request_id uuid not null unique references cbgpu_schools.feature_requests,recipient text not null check(recipient='info@cbgpu.com'),template text not null check(template='school-feature-request'),status text not null default 'PENDING' check(status in('PENDING','SENDING','SENT','FAILED')),attempts integer not null default 0,next_attempt_at timestamptz not null default now(),sent_at timestamptz,last_error_code text,lease_token uuid,lease_until timestamptz,first_attempt_at timestamptz);
create table if not exists cbgpu_schools.messages(id uuid primary key default gen_random_uuid(),user_id uuid not null references auth.users,scope text not null check(scope in('SCHOOL','NATIONWIDE')),school_id text references cbgpu_schools.schools,body text not null check(length(body) between 1 and 2000),created_at timestamptz not null default now(),deleted_at timestamptz,check((scope='SCHOOL')=(school_id is not null)));
create index if not exists messages_scope_time on cbgpu_schools.messages(scope,school_id,created_at desc) where deleted_at is null;
create table if not exists cbgpu_schools.message_mutes(user_id uuid references auth.users,muted_user_id uuid references auth.users,created_at timestamptz not null default now(),primary key(user_id,muted_user_id),check(user_id<>muted_user_id));
create table if not exists cbgpu_schools.message_reports(id uuid primary key default gen_random_uuid(),message_id uuid not null references cbgpu_schools.messages,reporter_id uuid not null references auth.users,reason text not null check(length(reason) between 1 and 1000),created_at timestamptz not null default now(),unique(message_id,reporter_id));
alter table cbgpu_schools.feature_requests enable row level security;alter table cbgpu_schools.mail_outbox enable row level security;alter table cbgpu_schools.messages enable row level security;alter table cbgpu_schools.message_mutes enable row level security;alter table cbgpu_schools.message_reports enable row level security;
revoke all on cbgpu_schools.feature_requests,cbgpu_schools.mail_outbox,cbgpu_schools.messages,cbgpu_schools.message_mutes,cbgpu_schools.message_reports from public,anon,authenticated;
create or replace function cbgpu_schools.community_allowed(uid uuid,scope text,school text) returns boolean language sql stable security definer set search_path='' as $$select case when scope='SCHOOL' then cbgpu_schools.active_member(uid,school) and exists(select 1 from cbgpu_schools.schools s where s.id=school and 'community'=any(s.features)) when scope='NATIONWIDE' then school is null and exists(select 1 from cbgpu_schools.memberships m join cbgpu_schools.schools s on s.id=m.school_id where m.user_id=uid and cbgpu_schools.active_member(uid,m.school_id) and 'community'=any(s.features)) else false end$$;
create or replace function public.cbgpu_school_extensions_api(action text,payload jsonb default '{}') returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();sid text:=payload->>'school_id';v_scope text:=payload->>'scope';r cbgpu_schools.feature_requests;m cbgpu_schools.messages;rid uuid;muted uuid;begin
 if uid is null or not cbgpu_accounts.current_session_valid() or not exists(select 1 from public.profiles p join auth.users u on u.id=p.id where p.id=uid and p.account_status<>'SUSPENDED' and u.deleted_at is null and(u.banned_until is null or u.banned_until<=now())) then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if action in('feature_requests','feature_request_create') then
 if not cbgpu_schools.active_member(uid,sid) then raise exception 'ACTIVE_MEMBERSHIP_REQUIRED' using errcode='42501';end if;
 if action='feature_requests' then return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(t) order by created_at desc),'[]') from cbgpu_schools.feature_requests t where t.school_id=sid and(t.requester_id=uid or cbgpu_schools.has_role(uid,sid,array['SCHOOL_ADMIN','SCHOOL_OWNER']))));end if;
 if coalesce(length(btrim(payload->>'title')),0) not between 1 and 150 or coalesce(length(btrim(payload->>'description')),0) not between 1 and 4000 or coalesce(payload->>'feature_key','') !~ '^[a-z][a-z0-9_-]{0,79}$' then raise exception 'INVALID_FEATURE_REQUEST' using errcode='23514';end if;
 perform pg_advisory_xact_lock(hashtextextended('school-request:'||uid::text,0));if(select count(*) from cbgpu_schools.feature_requests where requester_id=uid and created_at>now()-interval '1 day')>=5 then raise exception 'FEATURE_REQUEST_RATE_LIMIT' using errcode='P0429';end if;
 insert into cbgpu_schools.feature_requests(school_id,requester_id,feature_key,title,description) values(sid,uid,payload->>'feature_key',btrim(payload->>'title'),btrim(payload->>'description')) returning * into r;
 insert into cbgpu_schools.mail_outbox(request_id,recipient,template) values(r.id,'info@cbgpu.com','school-feature-request');return jsonb_build_object('request',to_jsonb(r),'notification_status','PENDING');
 elsif action in('messages','message_create') then
 if not cbgpu_schools.community_allowed(uid,v_scope,sid) then raise exception 'COMMUNITY_SCOPE_DENIED' using errcode='42501';end if;
 if action='messages' then return jsonb_build_object('items',(select coalesce(jsonb_agg(to_jsonb(z)-'deleted_at' order by z.created_at),'[]') from(select msg.* from cbgpu_schools.messages msg where msg.scope=v_scope and msg.school_id is not distinct from sid and msg.deleted_at is null and not exists(select 1 from cbgpu_schools.message_mutes x where x.user_id=uid and x.muted_user_id=msg.user_id) order by msg.created_at desc limit 100) z));end if;
 if coalesce(length(btrim(payload->>'body')),0) not between 1 and 2000 then raise exception 'INVALID_MESSAGE' using errcode='23514';end if;
 perform pg_advisory_xact_lock(hashtextextended('school-message:'||uid::text,0));if(select count(*) from cbgpu_schools.messages where user_id=uid and created_at>now()-interval '1 minute')>=10 then raise exception 'MESSAGE_RATE_LIMIT' using errcode='P0429';end if;
 insert into cbgpu_schools.messages(user_id,scope,school_id,body) values(uid,v_scope,sid,btrim(payload->>'body')) returning * into m;return jsonb_build_object('message',to_jsonb(m)-'deleted_at');
 elsif action in('message_delete','message_report') then
 select * into m from cbgpu_schools.messages where id=(payload->>'id')::uuid and deleted_at is null;if not found or not cbgpu_schools.community_allowed(uid,m.scope,m.school_id) then raise exception 'MESSAGE_NOT_FOUND' using errcode='P0002';end if;
 if action='message_delete' then
 if m.user_id<>uid and not(m.scope='SCHOOL' and cbgpu_schools.has_role(uid,m.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN','COMMUNITY_MODERATOR'])) then raise exception 'MESSAGE_DELETE_DENIED' using errcode='42501';end if;
 update cbgpu_schools.messages set deleted_at=now() where id=m.id;return jsonb_build_object('ok',true);
 end if;
 if coalesce(length(btrim(payload->>'reason')),0) not between 1 and 1000 then raise exception 'INVALID_REPORT' using errcode='23514';end if;
 insert into cbgpu_schools.message_reports(message_id,reporter_id,reason) values(m.id,uid,btrim(payload->>'reason')) on conflict(message_id,reporter_id) do nothing;return jsonb_build_object('ok',true);
 elsif action='message_mute' then
 muted:=(payload->>'muted_user_id')::uuid;if muted=uid or not exists(select 1 from auth.users where id=muted and deleted_at is null) then raise exception 'INVALID_MUTE' using errcode='23514';end if;
 if payload->'muted'='true'::jsonb then insert into cbgpu_schools.message_mutes(user_id,muted_user_id) values(uid,muted) on conflict do nothing;elsif payload->'muted'='false'::jsonb then delete from cbgpu_schools.message_mutes where user_id=uid and muted_user_id=muted;else raise exception 'INVALID_MUTE' using errcode='23514';end if;return jsonb_build_object('ok',true);
 end if;
 raise exception 'UNKNOWN_SCHOOL_ACTION' using errcode='23514';end$$;
revoke all on function cbgpu_schools.community_allowed(uuid,text,text) from public,anon,authenticated;
revoke all on function public.cbgpu_school_extensions_api(text,jsonb) from public,anon;grant execute on function public.cbgpu_school_extensions_api(text,jsonb) to authenticated;
-- Narrow worker interface. The HTTP school role cannot access these functions or rows.
do $$begin if not exists(select 1 from pg_roles where rolname='cbgpu_school_mail_worker') then create role cbgpu_school_mail_worker nologin nosuperuser nobypassrls;end if;end$$;
create or replace function cbgpu_schools.claim_mail(p_lease uuid) returns jsonb language plpgsql security definer set search_path='' as $$declare item cbgpu_schools.mail_outbox;r cbgpu_schools.feature_requests;s cbgpu_schools.schools;begin
 if p_lease is null then raise exception 'LEASE_REQUIRED';end if;
 update cbgpu_schools.mail_outbox set status='FAILED',attempts=10,last_error_code='IDEMPOTENCY_WINDOW_EXPIRED',lease_token=null,lease_until=null where status in('PENDING','FAILED','SENDING') and first_attempt_at<now()-interval '23 hours' and(lease_until is null or lease_until<now());
 select * into item from cbgpu_schools.mail_outbox o where o.attempts<10 and o.status in('PENDING','FAILED','SENDING') and o.next_attempt_at<=now() and(o.lease_until is null or o.lease_until<now()) order by o.next_attempt_at,o.id limit 1 for update skip locked;
 if not found then return null;end if;
 update cbgpu_schools.mail_outbox set status='SENDING',attempts=attempts+1,first_attempt_at=coalesce(first_attempt_at,now()),lease_token=p_lease,lease_until=now()+interval '2 minutes' where id=item.id returning * into item;
 select * into r from cbgpu_schools.feature_requests where id=item.request_id;select * into s from cbgpu_schools.schools where id=r.school_id;
 return jsonb_build_object('id',item.id,'request_id',r.id,'lease_token',p_lease,'recipient','info@cbgpu.com','template','school-feature-request','attempt',item.attempts,'school_id',s.id,'school_name',s.display_name,'feature_key',r.feature_key,'title',r.title,'description',r.description,'requester_id',r.requester_id,'created_at',r.created_at);
end$$;
create or replace function cbgpu_schools.ack_mail(p_id uuid,p_lease uuid,p_success boolean,p_error_code text default null) returns boolean language plpgsql security definer set search_path='' as $$begin
 update cbgpu_schools.mail_outbox set status=case when p_success then 'SENT' else 'FAILED' end,sent_at=case when p_success then now() end,last_error_code=case when p_success then null else left(regexp_replace(coalesce(p_error_code,'DELIVERY_FAILED'),'[^A-Z0-9_]','','g'),80) end,next_attempt_at=now()+make_interval(secs=>least(3600,60*power(2,least(attempts,6)))::integer),lease_token=null,lease_until=null where id=p_id and lease_token=p_lease and status='SENDING' and lease_until>now();return found;
end$$;
revoke all on function cbgpu_schools.claim_mail(uuid),cbgpu_schools.ack_mail(uuid,uuid,boolean,text) from public,anon,authenticated;
grant usage on schema cbgpu_schools to cbgpu_school_mail_worker;
grant execute on function cbgpu_schools.claim_mail(uuid),cbgpu_schools.ack_mail(uuid,uuid,boolean,text) to cbgpu_school_mail_worker;
commit;
