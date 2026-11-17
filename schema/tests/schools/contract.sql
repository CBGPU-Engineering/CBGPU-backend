-- Synthetic schools/users only. All assertions execute real SQL API/RLS.
create function public.assert_true(value boolean,label text) returns void language plpgsql as $$begin if value is distinct from true then raise exception 'ASSERTION: %',label;end if;end$$;
select public.assert_true((select count(*)=5 from cbgpu_schools.memberships),'legacy all identities preserved');
select public.assert_true((select membership_type='EXTERNAL' and not verified from cbgpu_schools.memberships where user_id='00000000-0000-4000-8000-000000000002'),'external not promoted');
select public.assert_true((select status='SUSPENDED' from cbgpu_schools.memberships where user_id='00000000-0000-4000-8000-000000000003'),'suspension preserved');
select public.assert_true((select membership_type='TEACHER' and school_roles='{}' from cbgpu_schools.memberships where user_id='00000000-0000-4000-8000-000000000004'),'teacher not admin');
select set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000002","session_id":"10000000-0000-4000-8000-000000000002"}',false);
create temporary table app as select (public.cbgpu_school_api('draft_create')->'application'->>'id')::uuid id;
do $$declare a uuid:=(select id from app);d jsonb:=cbgpu_schools.default_draft();r jsonb;begin
 d:=jsonb_set(d,'{school}', '{"official_name":"School B","type":"HIGH_SCHOOL","official_id":"B","region":"Test","address":"Synthetic street","contact_email":"test@example.invalid","contact_phone":""}');
 d:=jsonb_set(d,'{administrator}', '{"name":"Applicant","role":"STUDENT_OPERATOR","relationship":"Student operator","contact_email":"test@example.invalid","verification_note":"Fixture only"}');d:=jsonb_set(d,'{configuration,display_name}','"School B"');
 r:=public.cbgpu_school_api('draft_save',jsonb_build_object('id',a,'revision',1,'step',6,'data',d));perform public.assert_true((r#>>'{application,revision}')::int=2,'draft revision');
 r:=public.cbgpu_school_api('draft_save',jsonb_build_object('id',a,'revision',1,'step',6,'data',d));perform public.assert_true(r->>'code'='REVISION_CONFLICT','stale draft rejected');
 r:=public.cbgpu_school_api('draft_submit',jsonb_build_object('id',a,'revision',2,'confirmed',true));perform public.assert_true(r#>>'{application,status}'='PENDING','no implicit approval');
 perform public.assert_true(not exists(select 1 from cbgpu_schools.memberships where user_id=auth.uid() and 'SCHOOL_ADMIN'=any(school_roles)),'applicant no school admin');
 begin perform public.cbgpu_school_api('platform_review',jsonb_build_object('id',a,'revision',3,'decision','APPROVE','note','self'));raise exception 'self review unexpectedly allowed';exception when insufficient_privilege then null;end;
end$$;
select set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000001","session_id":"10000000-0000-4000-8000-000000000001"}',false);
select public.cbgpu_school_api('platform_review',jsonb_build_object('id',(select id from app),'revision',3,'decision','APPROVE','note','Reviewed synthetic school'));
select public.assert_true((select count(*)=0 from cbgpu_schools.gpu_nodes),'optional GPU absent');
select public.assert_true((select is_admin=false from public.profiles where id='00000000-0000-4000-8000-000000000002'),'school owner not platform admin');
-- School B owner cannot manage school A or platform applications.
select set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000002","session_id":"10000000-0000-4000-8000-000000000002"}',false);
do $$declare sid text:='school-'||(select id::text from app);r jsonb;begin
 r:=public.cbgpu_school_api('configuration',jsonb_build_object('school_id',sid));perform public.assert_true(r->>'can_manage_roles'='true','approved school owner');
 begin perform public.cbgpu_school_api('configuration','{"school_id":"cbsh"}');raise exception 'cross school unexpectedly allowed';exception when insufficient_privilege then null;end;
 begin perform public.cbgpu_school_api('platform_applications');raise exception 'platform unexpectedly allowed';exception when insufficient_privilege then null;end;
 perform public.assert_true(not(cbgpu_schools.summary((select s from cbgpu_schools.schools s where s.id=sid)) ? 'contact_email'),'public directory no contacts');
end$$;
-- Fresh session is checked every request, including own drafts.
delete from auth.sessions where user_id='00000000-0000-4000-8000-000000000002';
do $$begin begin perform public.cbgpu_school_api('applications');raise exception 'revoked session unexpectedly allowed';exception when insufficient_privilege then null;end;end$$;
-- Raw private state cannot be reached by anon/authenticated.
set role authenticated;
do $$begin begin perform count(*) from cbgpu_schools.applications;raise exception 'private table readable';exception when insufficient_privilege then null;end;end$$;
reset role;
