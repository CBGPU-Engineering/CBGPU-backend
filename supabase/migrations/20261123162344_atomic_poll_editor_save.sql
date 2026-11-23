begin;
-- Additive only. Existing answer/question IDs are never rebuilt by the editor.
alter table public.polls add column if not exists editor_revision bigint not null default 0;


-- Old browser tabs still write tables directly. Keep revision checks meaningful
-- for those writes and stop question cascades from erasing existing answers.
create or replace function public.cbgpu_poll_revision_trigger() returns trigger
language plpgsql security invoker set search_path='' as $$
begin
 new.editor_revision:=old.editor_revision+1;
 return new;
end $$;
drop trigger if exists cbgpu_poll_editor_revision on public.polls;
create trigger cbgpu_poll_editor_revision before update on public.polls
for each row execute function public.cbgpu_poll_revision_trigger();

create or replace function public.cbgpu_poll_question_integrity_trigger() returns trigger
language plpgsql security invoker set search_path='' as $$
begin
 if tg_op='DELETE' then
  -- A deliberate whole-poll DELETE has already removed the parent row when its
  -- FK cascade visits questions. Preserve that existing confirmed capability.
  if exists(select 1 from public.polls where id=old.poll_id)
   and exists(select 1 from public.poll_answers a where a.question_id=old.id) then
   raise exception 'POLL_ANSWERED_QUESTION_REMOVAL';
  end if;
  return old;
 end if;
 if (new.type is distinct from old.type or new.id is distinct from old.id or new.poll_id is distinct from old.poll_id)
  and exists(select 1 from public.poll_answers a where a.question_id=old.id) then
  raise exception 'POLL_ANSWERED_QUESTION_TYPE';
 end if;
 return new;
end $$;
drop trigger if exists cbgpu_poll_question_integrity on public.poll_questions;
create trigger cbgpu_poll_question_integrity before delete or update on public.poll_questions
for each row execute function public.cbgpu_poll_question_integrity_trigger();

create or replace function public.cbgpu_poll_question_revision_trigger() returns trigger
language plpgsql security invoker set search_path='' as $$
begin
 if tg_op='DELETE' then
  update public.polls set editor_revision=editor_revision+1 where id=old.poll_id;
  return old;
 end if;
 update public.polls set editor_revision=editor_revision+1 where id=new.poll_id;
 if tg_op='UPDATE' and old.poll_id<>new.poll_id then
  update public.polls set editor_revision=editor_revision+1 where id=old.poll_id;
 end if;
 return new;
end $$;
drop trigger if exists cbgpu_poll_question_revision on public.poll_questions;
create trigger cbgpu_poll_question_revision after insert or update or delete on public.poll_questions
for each row execute function public.cbgpu_poll_question_revision_trigger();
revoke all on function public.cbgpu_poll_revision_trigger(),public.cbgpu_poll_question_integrity_trigger(),public.cbgpu_poll_question_revision_trigger() from public,anon,authenticated;

create or replace function public.cbgpu_save_poll(
 p_poll_id uuid, p_expected_revision bigint, p_poll jsonb, p_questions jsonb
) returns jsonb language plpgsql security invoker set search_path = '' as $$
declare
 target uuid := p_poll_id; revision bigint; item jsonb; old_question public.poll_questions%rowtype;
 question_id uuid; client_id text; mapped jsonb := '{}'::jsonb; ids uuid[] := '{}';
 question_config jsonb; dependency text; position integer := 0;
begin
 -- The existing identity primitive checks the actual session and account state.
 if not coalesce(public.cbgpu_personal_project_identity(), false)
 or not exists(select 1 from public.profiles where id=auth.uid() and is_admin=true) then
  raise exception using errcode='42501', message='POLL_ADMIN_REQUIRED';
 end if;
 if jsonb_typeof(p_poll) is distinct from 'object' or jsonb_typeof(p_questions) is distinct from 'array'
 or jsonb_array_length(p_questions)>100 or length(btrim(coalesce(p_poll->>'title','')))=0 then
  raise exception 'POLL_INVALID_INPUT';
 end if;
 if target is null then
  insert into public.polls(title,description,ends_at,is_active,show_results,created_by)
  values(p_poll->>'title',coalesce(p_poll->>'description',''),nullif(p_poll->>'ends_at','')::timestamptz,true,coalesce((p_poll->>'show_results')::boolean,true),auth.uid())
  returning id,editor_revision into target,revision;
 else
  select editor_revision into revision from public.polls where id=target for update;
  if not found then raise exception 'POLL_NOT_FOUND'; end if;
  if p_expected_revision is null or p_expected_revision<>revision then raise exception 'POLL_EDIT_CONFLICT'; end if;
 end if;
 -- Serialize FK checks with removal/type checks. An in-flight answer completes
 -- before this lock, or must wait until the updated question exists again.
 perform id from public.poll_questions where poll_id=target order by id for update;
 for item in select value from jsonb_array_elements(p_questions) loop
  client_id:=item->>'id';
  if jsonb_typeof(item) is distinct from 'object' or client_id is null or length(client_id) not between 1 and 100
  or mapped ? client_id or coalesce(item->>'type','') not in ('choice','checkbox','text','paragraph','scale','dropdown')
  or jsonb_typeof(item->'options') is distinct from 'array' or jsonb_typeof(item->'config') is distinct from 'object'
  or item->>'title' is null then raise exception 'POLL_INVALID_INPUT'; end if;
  if client_id ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' then
   question_id:=client_id::uuid;
   select * into old_question from public.poll_questions where id=question_id;
   if not found or old_question.poll_id<>target then raise exception 'POLL_QUESTION_SCOPE'; end if;
   if old_question.type<>item->>'type' and exists(select 1 from public.poll_answers a where a.question_id=old_question.id) then
    raise exception 'POLL_ANSWERED_QUESTION_TYPE';
   end if;
  else question_id:=gen_random_uuid(); end if;
  mapped:=mapped||jsonb_build_object(client_id,question_id::text);
  ids:=array_append(ids,question_id);
 end loop;
 if exists(select 1 from public.poll_questions q join public.poll_answers a on a.question_id=q.id where q.poll_id=target and not(q.id=any(ids))) then
  raise exception 'POLL_ANSWERED_QUESTION_REMOVAL';
 end if;
 -- Validate/remap conditional question references before any question is changed.
 for item in select value from jsonb_array_elements(p_questions) loop
  question_config:=item->'config';
  if question_config ? 'showIf' then
   dependency:=question_config#>>'{showIf,questionId}';
   if dependency is null or not(mapped ? dependency) or dependency=item->>'id' then raise exception 'POLL_INVALID_CONDITION'; end if;
  end if;
 end loop;
 delete from public.poll_questions where poll_id=target and not(id=any(ids));
 for item in select value from jsonb_array_elements(p_questions) loop
  question_id:=(mapped->>(item->>'id'))::uuid;
  question_config:=item->'config';
  if question_config ? 'showIf' then
   question_config:=jsonb_set(question_config,'{showIf,questionId}',mapped->(question_config#>>'{showIf,questionId}'));
  end if;
  -- UPDATE first avoids granting an upsert authority over another poll's IDs.
  update public.poll_questions set order_num=position,type=item->>'type',title=item->>'title',description=coalesce(item->>'description',''),
   required=coalesce((item->>'required')::boolean,false),options=item->'options',config=question_config
   where id=question_id and poll_id=target;
  if not found then
   insert into public.poll_questions(id,poll_id,order_num,type,title,description,required,options,config)
   values(question_id,target,position,item->>'type',item->>'title',coalesce(item->>'description',''),coalesce((item->>'required')::boolean,false),item->'options',question_config);
  end if;
  position:=position+1;
 end loop;
 update public.polls set title=p_poll->>'title',description=coalesce(p_poll->>'description',''),ends_at=nullif(p_poll->>'ends_at','')::timestamptz,
  show_results=coalesce((p_poll->>'show_results')::boolean,true),editor_revision=editor_revision+1 where id=target returning editor_revision into revision;
 if not found then raise exception 'POLL_NOT_FOUND'; end if;
 return jsonb_build_object('id',target,'editor_revision',revision);
end $$;
revoke all on function public.cbgpu_save_poll(uuid,bigint,jsonb,jsonb) from public,anon;
grant execute on function public.cbgpu_save_poll(uuid,bigint,jsonb,jsonb) to authenticated;
notify pgrst, 'reload schema';
commit;
