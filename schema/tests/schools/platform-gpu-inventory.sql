begin;
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at)select('fd110000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'gpu-inventory-'||x||'@fixture.invalid','{"full_name":"Synthetic inventory operator","account_type_request":"EXTERNAL"}','{}','authenticated','authenticated','fixture-no-password',now(),now(),now()from generate_series(1,4)x;
insert into auth.sessions(id,user_id)select('fd120000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,('fd110000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid from generate_series(1,4)x;
select set_config('request.jwt.claims','{"role":"service_role"}',true);
update public.profiles set is_admin=true,is_approved=true,account_status='ACTIVE'where id='fd110000-0000-4000-8000-000000000001';
insert into cbgpu_schools.schools(id,official_name,normalized_name,display_name,type,configuration,features,status)values('fixture-inventory-a','Inventory A','inventory a','Inventory A','HIGH_SCHOOL','{}',array['gpu_compute'],'APPROVED'),('fixture-inventory-b','Inventory B','inventory b','Inventory B','HIGH_SCHOOL','{}',array['gpu_compute'],'APPROVED'),('fixture-inventory-listed','Inventory listed','inventory listed','Inventory listed','HIGH_SCHOOL','{}','{}','LISTED');
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,school_roles)values('fd110000-0000-4000-8000-000000000002','fixture-inventory-a','TEACHER','ACTIVE',true,array['GPU_ADMIN']),('fd110000-0000-4000-8000-000000000003','fixture-inventory-a','TEACHER','PENDING_VERIFICATION',false,array['SCHOOL_ADMIN']),('fd110000-0000-4000-8000-000000000004','fixture-inventory-a','TEACHER','ACTIVE',true,'{}');
create temp table inv_test(k text primary key,v jsonb);grant all on inv_test to authenticated;grant select on inv_test to anon;
select set_config('request.jwt.claims','{"sub":"fd110000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fd120000-0000-4000-8000-000000000001"}',true);set local role authenticated;
insert into inv_test values('platform',public.school_gpu_admin('fixture-inventory-b','mint','{"name":"Synthetic new server"}'));
do $$declare v jsonb;begin
 v:=public.school_gpu_admin('fixture-inventory-b','list');if jsonb_array_length(v->'items')<>1 or v#>>'{items,0,sharing_enabled}'<>'false'or v::text like '%gpuenr_%'then raise exception 'PLATFORM_INVENTORY_LIST_WRONG';end if;
 begin perform public.school_gpu_admin('fixture-inventory-listed','mint','{"name":"Not approved"}');raise exception 'LISTED_SCHOOL_GPU_MINT';exception when insufficient_privilege then null;end;
 begin perform public.school_gpu_resource_api('fixture-inventory-b','reserve',jsonb_build_object('resource_id',gen_random_uuid(),'starts_at',clock_timestamp()+interval'1 hour','ends_at',clock_timestamp()+interval'2 hours'));raise exception 'PLATFORM_INVENTORY_BOOKED_GPU';exception when insufficient_privilege then null;end;
 if public.cbgpu_can_use_resource('gpu','fixture-inventory-b')then raise exception 'INVENTORY_GRANTED_COMPUTE';end if;
 begin perform cbgpu_schools.gpu_inventory_authorized(auth.uid(),'fixture-inventory-b',(auth.jwt()->>'session_id')::uuid);raise exception 'PRIVATE_AUTHORITY_EXPOSED';exception when insufficient_privilege then null;end;
end$$;
-- Redemption is a one-use anonymous capability, freshly bound to the original operator session.
reset role;set local role anon;
select public.school_gpu_enroll((select v->>'token'from inv_test where k='platform'),repeat('a',64));
do $$begin begin perform public.school_gpu_enroll((select v->>'token'from inv_test where k='platform'),repeat('a',64));raise exception 'REPLAY_ENROLLMENT';exception when insufficient_privilege then null;end;end$$;
reset role;set local role authenticated;
select public.school_gpu_admin('fixture-inventory-b','revoke',jsonb_build_object('node_id',(select v->>'node_id'from inv_test where k='platform')));
insert into inv_test values('revoked_session',public.school_gpu_admin('fixture-inventory-b','mint','{"name":"Session must remain fresh"}'));
reset role;update auth.sessions set not_after=clock_timestamp()-interval'1 second'where id='fd120000-0000-4000-8000-000000000001';set local role anon;
do $$begin begin perform public.school_gpu_enroll((select v->>'token'from inv_test where k='revoked_session'),repeat('a',64));raise exception 'REVOKED_ISSUER_ENROLLED';exception when insufficient_privilege then null;end;end$$;
reset role;update auth.sessions set not_after=null where id='fd120000-0000-4000-8000-000000000001';set local role authenticated;
insert into inv_test values('lost_role',public.school_gpu_admin('fixture-inventory-b','mint','{"name":"Role must remain authorized"}'));
reset role;select set_config('request.jwt.claims','{"role":"service_role"}',true);update public.profiles set is_admin=false where id='fd110000-0000-4000-8000-000000000001';set local role anon;
do $$begin begin perform public.school_gpu_enroll((select v->>'token'from inv_test where k='lost_role'),repeat('a',64));raise exception 'FORMER_PLATFORM_ADMIN_ENROLLED';exception when insufficient_privilege then null;end;end$$;
reset role;select set_config('request.jwt.claims','{"role":"service_role"}',true);update public.profiles set is_admin=true,is_approved=false where id='fd110000-0000-4000-8000-000000000001';select set_config('request.jwt.claims','{"sub":"fd110000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fd120000-0000-4000-8000-000000000001"}',true);set local role authenticated;
do $$begin begin perform public.school_gpu_admin('fixture-inventory-b','list');raise exception 'UNAPPROVED_GLOBAL_ADMIN_INVENTORY';exception when insufficient_privilege then null;end;end$$;
-- Existing verified school's GPU admin remains supported but cannot manage another school.
select set_config('request.jwt.claims','{"sub":"fd110000-0000-4000-8000-000000000002","role":"authenticated","session_id":"fd120000-0000-4000-8000-000000000002"}',true);
insert into inv_test values('school_role',public.school_gpu_admin('fixture-inventory-a','mint','{"name":"Own-school role retained"}'));
do $$begin begin perform public.school_gpu_admin('fixture-inventory-b','list');raise exception 'FOREIGN_SCHOOL_ADMIN_INVENTORY';exception when insufficient_privilege then null;end;end$$;
select set_config('request.jwt.claims','{"sub":"fd110000-0000-4000-8000-000000000003","role":"authenticated","session_id":"fd120000-0000-4000-8000-000000000003"}',true);
do $$begin begin perform public.school_gpu_admin('fixture-inventory-a','list');raise exception 'PENDING_SCHOOL_ADMIN_INVENTORY';exception when insufficient_privilege then null;end;end$$;
select set_config('request.jwt.claims','{"sub":"fd110000-0000-4000-8000-000000000004","role":"authenticated","session_id":"fd120000-0000-4000-8000-000000000004"}',true);
do $$begin begin perform public.school_gpu_admin('fixture-inventory-a','list');raise exception 'TEACHER_IS_GPU_ADMIN';exception when insufficient_privilege then null;end;end$$;
reset role;

-- No time-based capability survives expiry, school suspension or operator ban.
update cbgpu_schools.gpu_enrollments set expires_at=clock_timestamp()-interval'1 second'where node_id=(select(v->>'node_id')::uuid from inv_test where k='school_role');set local role anon;
do $$begin begin perform public.school_gpu_enroll((select v->>'token'from inv_test where k='school_role'),repeat('a',64));raise exception 'EXPIRED_ENROLLMENT';exception when insufficient_privilege then null;end;end$$;
reset role;select set_config('request.jwt.claims','{"role":"service_role"}',true);update public.profiles set is_approved=true where id='fd110000-0000-4000-8000-000000000001';update auth.users set banned_until=clock_timestamp()+interval'1 hour'where id='fd110000-0000-4000-8000-000000000001';select set_config('request.jwt.claims','{"sub":"fd110000-0000-4000-8000-000000000001","role":"authenticated","session_id":"fd120000-0000-4000-8000-000000000001"}',true);set local role authenticated;
do $$begin begin perform public.school_gpu_admin('fixture-inventory-b','list');raise exception 'BANNED_GLOBAL_ADMIN_INVENTORY';exception when insufficient_privilege then null;end;end$$;
reset role;
rollback;
