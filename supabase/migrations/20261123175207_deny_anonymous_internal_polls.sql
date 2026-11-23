begin;
-- These internal poll pages require sign-in. Keep existing table grants and
-- authenticated/service policies intact; deny only the anon database role.
drop policy if exists cbgpu_poll_anonymous_boundary on public.polls;
create policy cbgpu_poll_anonymous_boundary on public.polls as restrictive for all to anon
using(false) with check(false);
drop policy if exists cbgpu_poll_question_anonymous_boundary on public.poll_questions;
create policy cbgpu_poll_question_anonymous_boundary on public.poll_questions as restrictive for all to anon
using(false) with check(false);
drop policy if exists cbgpu_poll_response_anonymous_boundary on public.poll_responses;
create policy cbgpu_poll_response_anonymous_boundary on public.poll_responses as restrictive for all to anon
using(false) with check(false);
drop policy if exists cbgpu_poll_answer_anonymous_boundary on public.poll_answers;
create policy cbgpu_poll_answer_anonymous_boundary on public.poll_answers as restrictive for all to anon
using(false) with check(false);
notify pgrst,'reload schema';
commit;
