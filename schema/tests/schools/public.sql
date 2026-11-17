select set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000005","session_id":"10000000-0000-4000-8000-000000000005"}',false);
-- Global logged-in identity can curate a project without borrowing school compute.
create temporary table publication as select public.cbgpu_public_project_api('save','{"slug":"fixture-project","title":"Public title","summary":"Public summary","body_markdown":"Explicit public article","visibility":"PUBLIC","confirmed":false}')->'project' item;
select public.assert_true((public.cbgpu_public_content('projects_list')->>'total')::int=0,'unconfirmed article not public');
select public.cbgpu_public_project_api('save',jsonb_build_object('id',(select item->>'id' from publication),'slug','fixture-project','title','Public title','summary','Public summary','body_markdown','Explicit public article','visibility','PUBLIC','confirmed',true));
select public.cbgpu_school_extensions_api('message_create','{"scope":"PUBLIC","body":"Explicit public fixture post"}');
select set_config('request.jwt.claims','',false);
update public.contests set access_scope='PUBLIC' where id='20000000-0000-4000-8000-000000000002';
set role anon;
select public.assert_true((public.cbgpu_public_content('projects_list')->>'total')::int=1,'explicit published article public');
select public.assert_true(not(public.cbgpu_public_content('project','{"id":"fixture-project"}')->'item' ?| array['owner_id','source_project_id','env','environment_variables','config']),'no private deployment/owner projection');
select public.assert_true((public.cbgpu_public_content('contests_list')->>'total')::int=1,'old school contests not automatically public');
select public.assert_true(public.cbgpu_public_content('contest','{"id":"20000000-0000-4000-8000-000000000001"}')->'item'='null'::jsonb,'private contest id not disclosed');
select public.assert_true(not((public.cbgpu_public_content('messages_list')->'items'->0) ?| array['user_id','owner_id','email']),'anonymous community no author identity');
do $$begin begin perform public.cbgpu_public_project_api('save','{}');raise exception 'anonymous publication allowed';exception when insufficient_privilege then null;end;end$$;
reset role;
select 'EXPLICIT_PUBLIC_CONTENT_TESTS_PASSED';

update public.contests set lifecycle_status='archived' where id='20000000-0000-4000-8000-000000000002';
set role anon;
select public.assert_true(public.cbgpu_public_content('contest','{"id":"20000000-0000-4000-8000-000000000002"}')->'item'='null'::jsonb,'unpublished contest immediately hidden');
reset role;

select public.assert_true(position('f1000000-' in cbgpu_schools.public_markdown('![cover](https://cbgpu.com/backend/storage/v1/object/public/contest-presentation/f1000000-0000-4000-8000-000000000001/f9000000-0000-4000-8000-000000000001.png)'))=0,'public Markdown hides owner-prefixed upload identifiers');
select public.assert_true(cbgpu_schools.public_markdown('Read https://github.com/example/repository and ![image](https://example.org/public.png)')='Read https://github.com/example/repository and ![image](https://example.org/public.png)','unrelated owner-authored URLs unchanged');
