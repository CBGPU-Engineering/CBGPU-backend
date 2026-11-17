-- Synthetic reports only, in the original-schema network-isolated clone.
begin;
select set_config('request.jwt.claims','{"role":"service_role"}',true);
insert into auth.users(id,email,raw_user_meta_data,raw_app_meta_data,aud,role,encrypted_password,email_confirmed_at,created_at,updated_at) select ('af010000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,'synthetic-ad-report-'||x||'@fixture.invalid',jsonb_build_object('full_name','Ad report fixture','account_type_request','EXTERNAL'),'{}','authenticated','authenticated','fixture-no-password',now(),now(),now() from generate_series(1,3)x;
insert into auth.sessions(id,user_id) select ('af020000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid,('af010000-0000-4000-8000-'||lpad(x::text,12,'0'))::uuid from generate_series(1,3)x;
update public.profiles set is_admin=true where id='af010000-0000-4000-8000-000000000001';
insert into cbgpu_schools.memberships(user_id,school_id,membership_type,status,verified,school_roles) values('af010000-0000-4000-8000-000000000002','cbsh','TEACHER','ACTIVE',true,array['SCHOOL_ADMIN']);
create function pg_temp.ad_payload(k text,g text,start_at date,end_at date,unit text,money bigint) returns jsonb language sql as $$
select jsonb_build_object('kind',k,'timezone','Asia/Seoul','currency','KRW','granularity',g,'period_start',start_at,'period_end',end_at,'source_hash',encode(sha256('synthetic report fixture'::bytea),'hex'),'preview_hash',encode(sha256(convert_to(k||E'|KRW|Asia/Seoul\n'||start_at::text||'|'||end_at::text||'|'||unit||'|'||money::text||E'|~|~|~\n','UTF8')),'hex'),'confirmed',true,'rows',jsonb_build_array(jsonb_build_object('start_date',start_at,'end_date',end_at,'unit_id',unit,'revenue_micros',money,'impressions',null,'clicks',null,'requests',null)))
$$;
create temp table ad_fixture_values(k text primary key,v jsonb);grant all on ad_fixture_values to authenticated;
select set_config('request.jwt.claims','{"role":"authenticated","sub":"af010000-0000-4000-8000-000000000001","session_id":"af020000-0000-4000-8000-000000000001"}',true);
set local role authenticated;
insert into ad_fixture_values values('first',public.cbgpu_ad_revenue_api('import',pg_temp.ad_payload('ESTIMATED','DAY','2026-10-01','2026-10-01','DAN-kVpBmlPRbOvGb6oi',1250000)));
do $$declare r jsonb;begin
 r:=public.cbgpu_ad_revenue_api('import',pg_temp.ad_payload('ESTIMATED','DAY','2026-10-01','2026-10-01','DAN-kVpBmlPRbOvGb6oi',1250000));if r->>'duplicate'<>'true' or r->>'id'<>(select v->>'id' from ad_fixture_values where k='first') then raise exception 'AD_REPORT_IDEMPOTENCY_FAILED';end if;
 begin perform public.cbgpu_ad_revenue_api('import',pg_temp.ad_payload('ESTIMATED','PERIOD','2026-10-01','2026-10-31','DAN-kVpBmlPRbOvGb6oi',10000000));raise exception 'DAILY_PERIOD_DOUBLECOUNT_ALLOWED' using errcode='XX000';exception when sqlstate 'PT409' then null;end;
 r:=public.cbgpu_ad_revenue_api('analytics','{"from":"2026-10-01","to":"2026-10-31","kind":"ESTIMATED","timezone":"Asia/Seoul"}');if r#>>'{devices,0,revenue_krw}'<>'1.25' or r#>'{devices,0,impressions}'<>'null'::jsonb or r#>'{devices,0,rpuv_krw}'<>'null'::jsonb or r#>'{devices,0,fill_rate_percent}'<>'null'::jsonb or r->>'official_console_verified'<>'false' then raise exception 'AD_REVENUE_ESTIMATION_OR_DENOMINATOR_FABRICATED';end if;
end$$;
select public.cbgpu_ad_revenue_api('import',pg_temp.ad_payload('CONFIRMED','DAY','2026-10-01','2026-10-01','DAN-kVpBmlPRbOvGb6oi',80000000));
select public.cbgpu_ad_revenue_api('import',pg_temp.ad_payload('ESTIMATED','PERIOD','2026-11-01','2026-12-31','DAN-vpZNfcj3tj0jMkhV',6000000));
do $$declare r jsonb;begin
 r:=public.cbgpu_ad_revenue_api('analytics','{"from":"2026-10-01","to":"2026-10-31","kind":"CONFIRMED"}');if r#>>'{devices,0,revenue_krw}'<>'80' then raise exception 'ESTIMATED_CONFIRMED_SUMMED';end if;
 r:=public.cbgpu_ad_revenue_api('analytics','{"from":"2026-11-01","to":"2026-12-31","kind":"ESTIMATED"}');if r->'monthly'<>'[]'::jsonb or r->'daily'<>'[]'::jsonb or not(r->'warnings'?'CROSS_MONTH_PERIOD_NOT_ALLOCATED') then raise exception 'CROSS_MONTH_REVENUE_PRORATED';end if;
 r:=public.cbgpu_ad_revenue_api('analytics','{"from":"2026-11-01","to":"2026-11-30","kind":"ESTIMATED"}');if r->'devices'<>'[]'::jsonb or not(r->'warnings'?'PARTIAL_PERIOD_EXCLUDED') then raise exception 'PARTIAL_REPORT_PRORATED';end if;
end$$;
reset role;
select set_config('request.jwt.claims','{"role":"authenticated","sub":"af010000-0000-4000-8000-000000000002","session_id":"af020000-0000-4000-8000-000000000002"}',true);
set local role authenticated;
do $$begin begin perform public.cbgpu_ad_revenue_api('imports','{}');raise exception 'SCHOOL_ADMIN_BECAME_FINANCIAL_ADMIN' using errcode='XX000';exception when sqlstate 'PT403' then null;end;end$$;
reset role;
select set_config('request.jwt.claims','{"role":"authenticated","sub":"af010000-0000-4000-8000-000000000001","session_id":"af020000-0000-4000-8000-000000000001"}',true);
update auth.users set banned_until=now()+interval '1 hour' where id='af010000-0000-4000-8000-000000000001';
set local role authenticated;
do $$begin begin perform public.cbgpu_ad_revenue_api('imports','{}');raise exception 'BANNED_FINANCIAL_ADMIN_ALLOWED' using errcode='XX000';exception when sqlstate 'PT403' then null;end;end$$;
reset role;
update auth.users set banned_until=null where id='af010000-0000-4000-8000-000000000001';
delete from auth.sessions where id='af020000-0000-4000-8000-000000000001';
set local role authenticated;
do $$begin begin perform public.cbgpu_ad_revenue_api('imports','{}');raise exception 'REVOKED_FINANCIAL_ADMIN_SESSION_ALLOWED' using errcode='XX000';exception when sqlstate 'PT403' then null;end;end$$;
reset role;
do $$begin if has_function_privilege('anon','public.cbgpu_ad_revenue_api(text,jsonb)','EXECUTE') or has_function_privilege('service_role','public.cbgpu_ad_revenue_api(text,jsonb)','EXECUTE') or has_table_privilege('authenticated','cbgpu_ad_reports.facts','SELECT') or has_table_privilege('authenticated','cbgpu_ad_reports.imports','INSERT') then raise exception 'PRIVATE_FINANCIAL_SCHEMA_EXPOSED';end if;end$$;
rollback;
