begin;
-- Keep legacy direct reads/writes aligned with the same live-session identity
-- already required by the new invoker RPCs. Service/maintenance roles retain
-- their existing permissions; no table privileges or permissive policies change.
drop policy if exists cbgpu_poll_live_identity on public.polls;
create policy cbgpu_poll_live_identity on public.polls as restrictive for all to authenticated
using(coalesce(public.cbgpu_personal_project_identity(),false))
with check(coalesce(public.cbgpu_personal_project_identity(),false));
drop policy if exists cbgpu_poll_question_live_identity on public.poll_questions;
create policy cbgpu_poll_question_live_identity on public.poll_questions as restrictive for all to authenticated
using(coalesce(public.cbgpu_personal_project_identity(),false))
with check(coalesce(public.cbgpu_personal_project_identity(),false));
drop policy if exists cbgpu_poll_response_live_identity on public.poll_responses;
create policy cbgpu_poll_response_live_identity on public.poll_responses as restrictive for all to authenticated
using(coalesce(public.cbgpu_personal_project_identity(),false))
with check(coalesce(public.cbgpu_personal_project_identity(),false));
drop policy if exists cbgpu_poll_answer_live_identity on public.poll_answers;
create policy cbgpu_poll_answer_live_identity on public.poll_answers as restrictive for all to authenticated
using(coalesce(public.cbgpu_personal_project_identity(),false))
with check(coalesce(public.cbgpu_personal_project_identity(),false));

-- Hidden participation metadata follows the same owner/admin boundary. Public
-- result counts retain the existing authenticated visibility.
drop policy if exists cbgpu_poll_response_result_visibility on public.poll_responses;
create policy cbgpu_poll_response_result_visibility on public.poll_responses
as restrictive for select to authenticated
using (
 user_id=auth.uid()
 or exists(select 1 from public.profiles p where p.id=auth.uid() and p.is_admin=true)
 or exists(select 1 from public.polls p where p.id=poll_responses.poll_id and p.show_results is true)
);

-- Preserve existing policies and table grants, adding a restrictive boundary.
-- Owners can inspect their own receipt even when aggregate results are hidden.
drop policy if exists cbgpu_poll_answer_result_visibility on public.poll_answers;
create policy cbgpu_poll_answer_result_visibility on public.poll_answers
as restrictive for select to authenticated
using (
 coalesce(public.cbgpu_personal_project_identity(),false)
 and (
  exists(select 1 from public.profiles p where p.id=auth.uid() and p.is_admin=true)
  or exists(
   select 1 from public.poll_responses r join public.polls p on p.id=r.poll_id
   where r.id=poll_answers.response_id and (r.user_id=auth.uid() or p.show_results is true)
  )
 )
);
notify pgrst,'reload schema';
commit;
