begin;
-- Serialize new RPCs with old open tabs without a destructive data cleanup or
-- an index that could fail on historical duplicate answers.
create or replace function public.cbgpu_poll_answer_insert_guard() returns trigger
language plpgsql security invoker set search_path='' as $$
declare response_poll uuid; question public.poll_questions%rowtype; lower_bound numeric; upper_bound numeric;
begin
 lock table public.polls in share mode;
 lock table public.poll_questions in share mode;
 perform pg_advisory_xact_lock(hashtextextended('cbgpu-poll-answer:'||new.response_id::text,0));
 select poll_id into response_poll from public.poll_responses where id=new.response_id;
 if not found then raise exception 'POLL_RESPONSE_QUESTION_SCOPE';end if;
 select * into question from public.poll_questions q where q.id=new.question_id and q.poll_id=response_poll;
 if not found then
  raise exception 'POLL_RESPONSE_QUESTION_SCOPE';
 end if;
 -- Locks precede reading the current type/options, including old-tab INSERTs.
 if question.required and new.value in ('null'::jsonb,'""'::jsonb,'[]'::jsonb) then raise exception 'POLL_REQUIRED_ANSWER';end if;
 if new.value<>'null'::jsonb then
  if question.type in ('text','paragraph') then
   if jsonb_typeof(new.value)<>'string' then raise exception 'POLL_INVALID_ANSWER';end if;
  elsif question.type in ('choice','dropdown') then
   if jsonb_typeof(new.value)<>'string' or (new.value<>'""'::jsonb and not exists(select 1 from jsonb_array_elements(question.options) o where o->>'id'=new.value#>>'{}')) then raise exception 'POLL_INVALID_ANSWER';end if;
  elsif question.type='checkbox' then
   if jsonb_typeof(new.value)<>'array' then raise exception 'POLL_INVALID_ANSWER';end if;
   if exists(select 1 from jsonb_array_elements(new.value) v where jsonb_typeof(v)<>'string' or not exists(select 1 from jsonb_array_elements(question.options) o where o->>'id'=v#>>'{}')) then raise exception 'POLL_INVALID_ANSWER';end if;
  elsif question.type='scale' then
   if jsonb_typeof(new.value)<>'number' then raise exception 'POLL_INVALID_ANSWER';end if;
   lower_bound:=coalesce((question.config->>'min')::numeric,1);upper_bound:=coalesce((question.config->>'max')::numeric,5);
   if (new.value#>>'{}')::numeric<>trunc((new.value#>>'{}')::numeric) or (new.value#>>'{}')::numeric not between lower_bound and upper_bound then raise exception 'POLL_INVALID_ANSWER';end if;
  else raise exception 'POLL_INVALID_ANSWER';end if;
 end if;
 if exists(select 1 from public.poll_answers a where a.response_id=new.response_id and a.question_id=new.question_id) then
  raise exception 'POLL_ANSWER_ALREADY_RECORDED';
 end if;
 return new;
end $$;
drop trigger if exists cbgpu_poll_answer_insert_integrity on public.poll_answers;
create trigger cbgpu_poll_answer_insert_integrity before insert on public.poll_answers
for each row execute function public.cbgpu_poll_answer_insert_guard();
revoke all on function public.cbgpu_poll_answer_insert_guard() from public,anon,authenticated;

create or replace function public.cbgpu_submit_poll(p_poll_id uuid,p_expected_revision bigint,p_answers jsonb)
returns jsonb language plpgsql security invoker set search_path='' as $$
declare
 actor uuid:=auth.uid(); current_poll public.polls%rowtype; question public.poll_questions%rowtype;
 saved_response uuid; answer_count integer; normalized jsonb:='{}'; answer jsonb;
 key text; dependency jsonb; visible boolean; empty_answer boolean; lower_bound numeric; upper_bound numeric; existing_response boolean:=false;
begin
 if actor is null or not coalesce(public.cbgpu_personal_project_identity(),false) then
  raise exception 'POLL_AUTH_REQUIRED' using errcode='42501';
 end if;
 if jsonb_typeof(p_answers) is distinct from 'object' or (select count(*) from jsonb_each(p_answers))>100 then raise exception 'POLL_INVALID_ANSWER';end if;
 -- Row SHARE locks would silently filter nonadmin readers through UPDATE RLS.
 -- Existing authenticated table privileges permit these brief SHARE locks;
 -- they preserve RLS and keep both old/new editors from changing questions
 -- between validation and the answer commit. No grant is expanded here.
 lock table public.polls in share mode;
 lock table public.poll_questions in share mode;
 select * into current_poll from public.polls where id=p_poll_id;
 if not found then raise exception 'POLL_NOT_FOUND';end if;
 if current_poll.is_active is distinct from true or current_poll.ends_at<=now() then raise exception 'POLL_CLOSED';end if;
 if p_expected_revision is null or current_poll.editor_revision<>p_expected_revision then raise exception 'POLL_EDIT_CONFLICT';end if;
 for key in select jsonb_object_keys(p_answers) loop
  if not exists(select 1 from public.poll_questions q where q.poll_id=p_poll_id and q.id::text=key) then raise exception 'POLL_INVALID_ANSWER';end if;
 end loop;
 for question in select * from public.poll_questions where poll_id=p_poll_id order by order_num,id loop
  visible:=true;
  if question.config ? 'showIf' then
   dependency:=p_answers->(question.config#>>'{showIf,questionId}');
   if jsonb_typeof(dependency)='array' then
    visible:=exists(select 1 from jsonb_array_elements_text(dependency) x(value) where x.value=question.config#>>'{showIf,value}');
   else visible:=coalesce(dependency#>>'{}','')=question.config#>>'{showIf,value}';end if;
  end if;
  answer:=coalesce(p_answers->question.id::text,'null'::jsonb);
  empty_answer:=answer='null'::jsonb or answer='""'::jsonb or answer='[]'::jsonb;
  if coalesce(visible,false) and question.required and empty_answer then raise exception 'POLL_REQUIRED_ANSWER';end if;
  if answer<>'null'::jsonb then
   if question.type in ('text','paragraph') then
    if jsonb_typeof(answer)<>'string' then raise exception 'POLL_INVALID_ANSWER';end if;
   elsif question.type in ('choice','dropdown') then
    if jsonb_typeof(answer)<>'string' or (answer<>'""'::jsonb and not exists(select 1 from jsonb_array_elements(question.options) o where o->>'id'=answer#>>'{}')) then raise exception 'POLL_INVALID_ANSWER';end if;
   elsif question.type='checkbox' then
    if jsonb_typeof(answer)<>'array' then raise exception 'POLL_INVALID_ANSWER';end if;
    if exists(select 1 from jsonb_array_elements(answer) v where jsonb_typeof(v)<>'string' or not exists(select 1 from jsonb_array_elements(question.options) o where o->>'id'=v#>>'{}')) then raise exception 'POLL_INVALID_ANSWER';end if;
    select coalesce(jsonb_agg(value order by value),'[]'::jsonb) into answer from (select distinct value from jsonb_array_elements(answer)) choices;
   elsif question.type='scale' then
    if jsonb_typeof(answer)<>'number' then raise exception 'POLL_INVALID_ANSWER';end if;
    lower_bound:=coalesce((question.config->>'min')::numeric,1);upper_bound:=coalesce((question.config->>'max')::numeric,5);
    if (answer#>>'{}')::numeric<>trunc((answer#>>'{}')::numeric) or (answer#>>'{}')::numeric not between lower_bound and upper_bound then raise exception 'POLL_INVALID_ANSWER';end if;
   else raise exception 'POLL_INVALID_ANSWER';end if;
  end if;
  if not coalesce(visible,false) then continue;end if;
  normalized:=normalized||jsonb_build_object(question.id::text,answer);
 end loop;
 -- The user/poll lock serializes same-user RPCs before the unique INSERT.
 perform pg_advisory_xact_lock(hashtextextended('cbgpu-poll-submit:'||actor::text||':'||p_poll_id::text,0));
 select id into saved_response from public.poll_responses where poll_id=p_poll_id and user_id=actor;
 existing_response:=saved_response is not null;
 if saved_response is null then
  insert into public.poll_responses(poll_id,user_id) values(p_poll_id,actor) on conflict(poll_id,user_id) do nothing returning id into saved_response;
  if saved_response is null then select id into saved_response from public.poll_responses where poll_id=p_poll_id and user_id=actor;existing_response:=true;end if;
 end if;
 if saved_response is null then raise exception 'POLL_RESPONSE_UNCONFIRMED';end if;
 -- Every legacy INSERT takes this same response lock before inspecting/writing
 -- answers. A legacy insert that wins is observed, never erased or overwritten.
 perform pg_advisory_xact_lock(hashtextextended('cbgpu-poll-answer:'||saved_response::text,0));
 select count(*) into answer_count from public.poll_answers a where a.response_id=saved_response;
 if answer_count>0 then
  if answer_count=(select count(*) from jsonb_each(normalized))
   and answer_count=(select count(distinct a.question_id) from public.poll_answers a where a.response_id=saved_response)
   and not exists(select 1 from public.poll_answers a where a.response_id=saved_response and normalized->a.question_id::text is distinct from a.value) then
   return jsonb_build_object('id',saved_response,'poll_id',p_poll_id,'submitted',true,'reused',true);
  end if;
  raise exception 'POLL_RESPONSE_ALREADY_RECORDED';
 end if;
 if existing_response and normalized='{}'::jsonb then return jsonb_build_object('id',saved_response,'poll_id',p_poll_id,'submitted',true,'reused',true);end if;
 insert into public.poll_answers(response_id,question_id,value)
 select saved_response,entry.key::uuid,entry.value from jsonb_each(normalized) entry;
 return jsonb_build_object('id',saved_response,'poll_id',p_poll_id,'submitted',true,'reused',false);
end $$;
revoke all on function public.cbgpu_submit_poll(uuid,bigint,jsonb) from public,anon;
grant execute on function public.cbgpu_submit_poll(uuid,bigint,jsonb) to authenticated;
notify pgrst,'reload schema';
commit;
