-- Fresh channel authority, owner cleanup continuity and retry-safe sends. No old message is made public.
alter table cbgpu_schools.messages add column if not exists client_message_id uuid;
create unique index if not exists school_message_client_id on cbgpu_schools.messages(user_id,client_message_id)where client_message_id is not null;
create or replace function cbgpu_schools.message_json(m cbgpu_schools.messages)returns jsonb language sql immutable set search_path='' as $$select jsonb_build_object('id',m.id,'user_id',m.user_id,'sender_label','Member '||left(replace(m.user_id::text,'-',''),8),'scope',m.scope,'school_id',m.school_id,'body',m.body,'created_at',m.created_at,'client_message_id',m.client_message_id)$$;
revoke all on function cbgpu_schools.message_json(cbgpu_schools.messages)from public,anon,authenticated,service_role;
create or replace function public.cbgpu_school_messenger_api(action text,payload jsonb default '{}')returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();sid text:=payload->>'school_id';v_scope text:=payload->>'scope';m cbgpu_schools.messages;muted uuid;cid uuid;begin
 if not public.cbgpu_personal_project_identity()then raise exception 'AUTHENTICATION_REQUIRED' using errcode='42501';end if;
 if jsonb_typeof(payload)<>'object'or octet_length(payload::text)>20000 then raise exception 'INVALID_REQUEST' using errcode='22023';end if;
 if action='message_mutes'then return jsonb_build_object('items',(select coalesce(jsonb_agg(jsonb_build_object('user_id',muted_user_id,'sender_label','Member '||left(replace(muted_user_id::text,'-',''),8))order by created_at desc),'[]')from cbgpu_schools.message_mutes where user_id=uid));end if;
 if action in('messages','message_create')then
  if exists(select 1 from jsonb_object_keys(payload)k where k not in('scope','school_id','body','client_message_id','limit','offset'))then raise exception 'INVALID_MESSAGE_FIELDS' using errcode='22023';end if;
  if not cbgpu_schools.community_allowed(uid,v_scope,sid)then raise exception 'COMMUNITY_SCOPE_DENIED' using errcode='42501';end if;
  if action='messages'then return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.message_json(z::cbgpu_schools.messages)order by z.created_at,z.id),'[]')from(select msg.* from cbgpu_schools.messages msg where msg.scope=v_scope and msg.school_id is not distinct from sid and msg.deleted_at is null and not exists(select 1 from cbgpu_schools.message_mutes x where x.user_id=uid and x.muted_user_id=msg.user_id)order by msg.created_at desc,msg.id desc limit 100)z));end if;
  if jsonb_typeof(payload->'body')is distinct from 'string'or length(btrim(payload->>'body'))not between 1 and 2000 then raise exception 'INVALID_MESSAGE' using errcode='22023';end if;
  if payload?'client_message_id'then cid:=(payload->>'client_message_id')::uuid;if cid is null then raise exception 'INVALID_CLIENT_MESSAGE_ID' using errcode='22023';end if;end if;
  perform pg_advisory_xact_lock(hashtextextended('school-message:'||uid::text,0));
  if cid is not null then select * into m from cbgpu_schools.messages where user_id=uid and client_message_id=cid;if found then if m.scope is distinct from v_scope or m.school_id is distinct from sid or m.body<>btrim(payload->>'body')or m.deleted_at is not null then raise exception 'MESSAGE_IDEMPOTENCY_CONFLICT' using errcode='23505';end if;return jsonb_build_object('message',cbgpu_schools.message_json(m),'duplicate',true);end if;end if;
  if(select count(*)from cbgpu_schools.messages where user_id=uid and created_at>clock_timestamp()-interval '1 minute')>=10 then raise exception 'MESSAGE_RATE_LIMIT' using errcode='P0429';end if;
  insert into cbgpu_schools.messages(user_id,scope,school_id,body,client_message_id)values(uid,v_scope,sid,btrim(payload->>'body'),cid)returning * into m;
  return jsonb_build_object('message',cbgpu_schools.message_json(m),'duplicate',false);
 elsif action in('message_delete','message_report')then
  if exists(select 1 from jsonb_object_keys(payload)k where k not in('id','reason'))then raise exception 'INVALID_MESSAGE_FIELDS' using errcode='22023';end if;
  select * into m from cbgpu_schools.messages where id=(payload->>'id')::uuid and deleted_at is null for update;
  if not found then raise exception 'MESSAGE_NOT_FOUND' using errcode='P0002';end if;
  if action='message_delete'and m.user_id=uid then update cbgpu_schools.messages set deleted_at=clock_timestamp()where id=m.id;return jsonb_build_object('ok',true);end if;
  if not cbgpu_schools.community_allowed(uid,m.scope,m.school_id)then raise exception 'MESSAGE_NOT_FOUND' using errcode='P0002';end if;
  if action='message_delete'then
   if not(m.scope='SCHOOL'and cbgpu_schools.has_role(uid,m.school_id,array['SCHOOL_OWNER','SCHOOL_ADMIN','COMMUNITY_MODERATOR']))then raise exception 'MESSAGE_DELETE_DENIED' using errcode='42501';end if;
   update cbgpu_schools.messages set deleted_at=clock_timestamp()where id=m.id;insert into cbgpu_schools.audit(actor_id,school_id,action,target_id)values(uid,m.school_id,'MESSAGE_MODERATED',m.id);return jsonb_build_object('ok',true);
  end if;
  if jsonb_typeof(payload->'reason')is distinct from 'string'or length(btrim(payload->>'reason'))not between 1 and 1000 then raise exception 'INVALID_REPORT' using errcode='22023';end if;
  insert into cbgpu_schools.message_reports(message_id,reporter_id,reason)values(m.id,uid,btrim(payload->>'reason'))on conflict(message_id,reporter_id)do nothing;return jsonb_build_object('ok',true);
 elsif action='message_mute'then
  if exists(select 1 from jsonb_object_keys(payload)k where k not in('muted_user_id','muted'))then raise exception 'INVALID_MUTE_FIELDS' using errcode='22023';end if;
  muted:=(payload->>'muted_user_id')::uuid;
  if muted is null or muted=uid or jsonb_typeof(payload->'muted')is distinct from 'boolean'then raise exception 'INVALID_MUTE' using errcode='22023';end if;
  if payload->'muted'='true'::jsonb then
   if not exists(select 1 from cbgpu_schools.messages visible where visible.user_id=muted and visible.deleted_at is null and cbgpu_schools.community_allowed(uid,visible.scope,visible.school_id))then raise exception 'VISIBLE_MESSAGE_AUTHOR_REQUIRED' using errcode='P0002';end if;
   insert into cbgpu_schools.message_mutes(user_id,muted_user_id)values(uid,muted)on conflict do nothing;
  else delete from cbgpu_schools.message_mutes where user_id=uid and muted_user_id=muted;end if;
  return jsonb_build_object('ok',true);
 end if;raise exception 'UNKNOWN_MESSENGER_ACTION' using errcode='22023';
end$$;
revoke all on function public.cbgpu_school_messenger_api(text,jsonb)from public,anon,service_role;grant execute on function public.cbgpu_school_messenger_api(text,jsonb)to authenticated;
-- The older public RPC must not bypass the same authority through direct REST.
do $$declare definition text;begin definition:=pg_get_functiondef('public.cbgpu_school_extensions_api(text,jsonb)'::regprocedure);if position('cbgpu_school_messenger_api' in definition)=0 then if position('begin' in definition)=0 then raise exception 'MESSENGER_COMPAT_SOURCE_MISMATCH';end if;definition:=replace(definition,'begin','begin if action in(''messages'',''message_create'',''message_delete'',''message_report'',''message_mute'',''message_mutes'')then return public.cbgpu_school_messenger_api(action,payload);end if;');execute definition;end if;end$$;
