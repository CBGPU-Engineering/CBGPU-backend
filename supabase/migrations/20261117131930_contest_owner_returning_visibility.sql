begin;
-- INSERT ... RETURNING evaluates SELECT policy before a STABLE lookup by ID
-- can see the newly inserted tuple. Use its validated owner column directly;
-- existing non-owner audience checks still use the fresh scoped predicate.
drop policy if exists multischool_contest_read on public.contests;
create policy multischool_contest_read on public.contests as restrictive for select to anon,authenticated
 using((host_id=auth.uid() and public.cbgpu_school_read_identity())
 or public.cbgpu_contest_visible(id));
commit;
