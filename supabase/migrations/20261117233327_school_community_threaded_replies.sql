begin;
-- Existing channel messages remain posts with the same IDs, bodies and visibility.
-- DM storage and its consent/authority paths are deliberately independent.
alter table cbgpu_schools.messages add column if not exists parent_id uuid references cbgpu_schools.messages(id);
create index if not exists school_message_replies on cbgpu_schools.messages(parent_id,created_at,id)where parent_id is not null and deleted_at is null;
create or replace function cbgpu_schools.message_json(m cbgpu_schools.messages)returns jsonb language sql stable set search_path='' as $$select jsonb_build_object('id',m.id,'user_id',m.user_id,'sender_label','Member '||left(replace(m.user_id::text,'-',''),8),'scope',m.scope,'school_id',m.school_id,'body',m.body,'created_at',m.created_at,'client_message_id',m.client_message_id,'parent_id',m.parent_id,'reply_count',case when m.parent_id is null then(select count(*)from cbgpu_schools.messages r where r.parent_id=m.id and r.deleted_at is null and not exists(select from cbgpu_schools.message_mutes x where x.user_id=auth.uid()and x.muted_user_id=r.user_id))else 0 end)$$;
revoke all on function cbgpu_schools.message_json(cbgpu_schools.messages)from public,anon,authenticated,service_role;
-- Freeze the prior handler privately, preserving DM-aware mute integration and owner cleanup.
do $upgrade$declare d text;begin
 if to_regprocedure('cbgpu_schools.messenger_before_threads(text,jsonb)')is null then
  d:=pg_get_functiondef('public.cbgpu_school_messenger_api(text,jsonb)'::regprocedure);
  if position('msg.deleted_at is null'in d)=0 then raise exception 'MESSENGER_THREAD_SOURCE_MISMATCH';end if;
  d:=replace(d,'FUNCTION public.cbgpu_school_messenger_api','FUNCTION cbgpu_schools.messenger_before_threads');
  d:=replace(d,'msg.deleted_at is null','msg.deleted_at is null and msg.parent_id is null');
  d:=replace(d,'m.scope is distinct from v_scope','m.parent_id is not null or m.scope is distinct from v_scope');
  d:=replace(d,'order by z.created_at,z.id','order by z.created_at desc,z.id desc');
  execute d;
 end if;
end $upgrade$;
revoke all on function cbgpu_schools.messenger_before_threads(text,jsonb)from public,anon,authenticated,service_role;
create or replace function public.cbgpu_school_messenger_api(action text,payload jsonb default '{}')returns jsonb language plpgsql security definer set search_path='' as $$
declare uid uuid:=auth.uid();p cbgpu_schools.messages;r cbgpu_schools.messages;cid uuid;lim integer;off integer;begin
 if not public.cbgpu_personal_project_identity()then raise exception 'AUTHENTICATION_REQUIRED'using errcode='42501';end if;
 if jsonb_typeof(payload)is distinct from'object'or octet_length(payload::text)>20000 then raise exception 'INVALID_REQUEST'using errcode='22023';end if;
 if action not in('message','replies','reply_create')then
  -- Cleanup of one's own retained data remains possible after affiliation loss.
  if action in('message_delete','message_report')then
   select * into r from cbgpu_schools.messages where id=(payload->>'id')::uuid;
   if r.parent_id is not null and not(action='message_delete'and r.user_id=uid)then
    select * into p from cbgpu_schools.messages where id=r.parent_id and parent_id is null and deleted_at is null;
    if not found or not cbgpu_schools.community_allowed(uid,p.scope,p.school_id)or exists(select from cbgpu_schools.message_mutes x where x.user_id=uid and x.muted_user_id in(p.user_id,r.user_id))then raise exception 'MESSAGE_NOT_FOUND'using errcode='P0002';end if;
   end if;
  end if;
  return cbgpu_schools.messenger_before_threads(action,payload);
 end if;
 if exists(select from jsonb_object_keys(payload)k where k not in('id','body','client_message_id','limit','offset'))then raise exception 'INVALID_MESSAGE_FIELDS'using errcode='22023';end if;
 -- Lock the root against concurrent deletion; scope is read from it, never accepted from callers.
 select * into p from cbgpu_schools.messages where id=(payload->>'id')::uuid and parent_id is null and deleted_at is null for share;
 if not found or not cbgpu_schools.community_allowed(uid,p.scope,p.school_id)or exists(select from cbgpu_schools.message_mutes x where x.user_id=uid and x.muted_user_id=p.user_id)then raise exception 'MESSAGE_NOT_FOUND'using errcode='P0002';end if;
 if action='message'then return jsonb_build_object('message',cbgpu_schools.message_json(p));end if;
 if action='replies'then
  if(payload?'limit'and(payload->>'limit')!~'^[0-9]{1,3}$')or(payload?'offset'and(payload->>'offset')!~'^[0-9]{1,6}$')then raise exception 'INVALID_PAGINATION'using errcode='22023';end if;
  lim:=coalesce((payload->>'limit')::integer,50);off:=coalesce((payload->>'offset')::integer,0);
  if lim not between 1 and 100 or off not between 0 and 100000 then raise exception 'INVALID_PAGINATION'using errcode='22023';end if;
  return jsonb_build_object('items',(select coalesce(jsonb_agg(cbgpu_schools.message_json(z)order by z.created_at,z.id),'[]')from(select m.* from cbgpu_schools.messages m where m.parent_id=p.id and m.deleted_at is null and not exists(select from cbgpu_schools.message_mutes x where x.user_id=uid and x.muted_user_id=m.user_id)order by m.created_at,m.id limit lim offset off)z),'total',(select count(*)from cbgpu_schools.messages m where m.parent_id=p.id and m.deleted_at is null and not exists(select from cbgpu_schools.message_mutes x where x.user_id=uid and x.muted_user_id=m.user_id)));
 end if;
 if jsonb_typeof(payload->'body')is distinct from'string'or length(btrim(payload->>'body'))not between 1 and 2000 or jsonb_typeof(payload->'client_message_id')is distinct from'string'then raise exception 'INVALID_REPLY'using errcode='22023';end if;
 cid:=(payload->>'client_message_id')::uuid;if cid is null then raise exception 'INVALID_CLIENT_MESSAGE_ID'using errcode='22023';end if;
 perform pg_advisory_xact_lock(hashtextextended('school-message:'||uid::text,0));
 select * into r from cbgpu_schools.messages where user_id=uid and client_message_id=cid;
 if found then
  if r.parent_id is distinct from p.id or r.body<>btrim(payload->>'body')or r.deleted_at is not null then raise exception 'MESSAGE_IDEMPOTENCY_CONFLICT'using errcode='23505';end if;
  return jsonb_build_object('message',cbgpu_schools.message_json(r),'duplicate',true);
 end if;
 if(select count(*)from cbgpu_schools.messages where user_id=uid and created_at>clock_timestamp()-interval'1 minute')+(select count(*)from cbgpu_schools.direct_messages where user_id=uid and created_at>clock_timestamp()-interval'1 minute')>=10 then raise exception 'MESSAGE_RATE_LIMIT'using errcode='P0429';end if;
 insert into cbgpu_schools.messages(user_id,scope,school_id,body,client_message_id,parent_id)values(uid,p.scope,p.school_id,btrim(payload->>'body'),cid,p.id)returning * into r;
 return jsonb_build_object('message',cbgpu_schools.message_json(r),'duplicate',false);
end$$;
revoke all on function public.cbgpu_school_messenger_api(text,jsonb)from public,anon,service_role;grant execute on function public.cbgpu_school_messenger_api(text,jsonb)to authenticated;
-- Anonymous discovery is root-only as well: neither live nor orphaned public
-- replies are standalone posts. No anonymous message detail endpoint is added.
do $public_roots$declare d text;old_pred text:='where scope=''PUBLIC'' and deleted_at is null';new_pred text:='where scope=''PUBLIC'' and parent_id is null and deleted_at is null';begin
 d:=pg_get_functiondef('public.cbgpu_public_content(text,jsonb)'::regprocedure);
 if position(new_pred in d)=0 then
  if(length(d)-length(replace(d,old_pred,'')))/length(old_pred)<>2 then raise exception 'PUBLIC_COMMUNITY_THREAD_SOURCE_MISMATCH';end if;
  execute replace(d,old_pred,new_pred);
 end if;
end $public_roots$;
commit;
