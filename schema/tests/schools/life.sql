begin;
insert into auth.sessions(id,user_id) values('10000000-0000-4000-8000-000000000002','00000000-0000-4000-8000-000000000002');
select set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000001","session_id":"10000000-0000-4000-8000-000000000001"}',true);
select public.assert_true(public.cbgpu_school_life_sources('cbsh')->>'calendar_url'='https://school.cbe.go.kr/cbs-h/M010407/list','CBsh actual monthly HTML calendar source');
select public.assert_true(public.cbgpu_school_life_sources('cbsh')#>>'{comcigan,school_name}'='충북과학고등학교','CBsh canonical timetable school');
select set_config('request.jwt.claims','{"sub":"00000000-0000-4000-8000-000000000002","session_id":"10000000-0000-4000-8000-000000000002"}',true);
do $$declare school text:=(select school_id from cbgpu_schools.applications where status='APPROVED' limit 1);cfg jsonb;r jsonb;begin
 cfg:=public.cbgpu_school_api('configuration',jsonb_build_object('school_id',school));
 perform public.cbgpu_school_api('configure',jsonb_build_object('school_id',school,'revision',cfg->'revision','features',jsonb_build_array('community','projects','contests','school_life')));
 r:=public.cbgpu_school_life_sources(school);perform public.assert_true(r#>>'{comcigan,school_name}'='School B','other school never falls back to CBsh');perform public.assert_true(r#>>'{comcigan,status}'='LOOKUP_REQUIRED','canonical provider lookup state');perform public.assert_true(r->'calendar_url'='null'::jsonb,'never invent calendar path');
 begin perform public.cbgpu_school_life_configure(school,'{"confirmed":true,"allowed_hosts":["school.cbe.go.kr"],"calendar_url":"https://school.cbe.go.kr/cbs-h/M010407/list"}');raise exception 'school owner approved arbitrary source';exception when insufficient_privilege then null;end;
 cfg:=public.cbgpu_school_api('configuration',jsonb_build_object('school_id',school));perform public.cbgpu_school_api('configure',jsonb_build_object('school_id',school,'revision',cfg->'revision','features',jsonb_build_array('community','projects','contests')));
 r:=public.cbgpu_school_life_sources(school);perform public.assert_true(r->>'status'='DISABLED' and r#>>'{comcigan,status}'='NOT_CONFIGURED','disabled module cannot fetch timetable');
end$$;
select public.assert_true(not cbgpu_schools.valid_source_url('https://127.0.0.1/private',array['127.0.0.1']),'literal IP source rejected');
select public.assert_true(not cbgpu_schools.valid_source_url('https://user:pass@school.cbe.go.kr/private',array['school.cbe.go.kr']),'credentials in source rejected');
select public.assert_true(not cbgpu_schools.valid_source_url('https://school.internal/private',array['school.internal']),'internal source rejected');
rollback;
