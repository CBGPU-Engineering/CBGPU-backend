begin;
create schema if not exists cbgpu_ad_reports;
revoke all on schema cbgpu_ad_reports from public,anon,authenticated,service_role;
create table if not exists cbgpu_ad_reports.imports(
 id uuid primary key default gen_random_uuid(),preview_hash text not null unique,source_hash text not null,
 kind text not null check(kind in('ESTIMATED','CONFIRMED')),currency text not null check(currency='KRW'),timezone text not null check(timezone in('Asia/Seoul','UTC')),
 period_start date not null,period_end date not null,granularity text not null check(granularity in('DAY','PERIOD')),rows_count integer not null check(rows_count between 1 and 5000),created_by uuid references auth.users(id) on delete set null,created_at timestamptz not null default clock_timestamp(),check(period_start<=period_end));
create table if not exists cbgpu_ad_reports.facts(
 import_id uuid not null references cbgpu_ad_reports.imports(id) on delete restrict,kind text not null,timezone text not null,unit_id text not null,start_date date not null,end_date date not null,
 revenue_micros bigint not null,impressions bigint check(impressions>=0),clicks bigint check(clicks>=0),requests bigint check(requests>=0),
 primary key(kind,timezone,unit_id,start_date,end_date),check(start_date<=end_date));
create index if not exists ad_report_period on cbgpu_ad_reports.facts(kind,timezone,start_date,end_date);
create table if not exists cbgpu_ad_reports.experiments(
 id uuid primary key default gen_random_uuid(),name text not null,device text not null,unit_id text not null,period_start date not null,period_end date not null,timezone text not null,
 interval_seconds integer not null check(interval_seconds in(60,120)),console_verified boolean not null default false,verified_at timestamptz,notes text not null default '',revision integer not null default 1,
 created_by uuid references auth.users(id) on delete set null,created_at timestamptz not null default clock_timestamp(),updated_at timestamptz not null default clock_timestamp());
alter table cbgpu_ad_reports.imports enable row level security;alter table cbgpu_ad_reports.facts enable row level security;alter table cbgpu_ad_reports.experiments enable row level security;
revoke all on all tables in schema cbgpu_ad_reports from public,anon,authenticated,service_role;
create or replace function cbgpu_ad_reports.admin() returns boolean language plpgsql volatile security definer set search_path='' as $$declare subject uuid:=auth.uid();sid uuid;begin
 if auth.role() is distinct from 'authenticated' then return false;end if;
 begin sid:=(auth.jwt()->>'session_id')::uuid;exception when invalid_text_representation then return false;end;
 return exists(select 1 from auth.sessions s join auth.users u on u.id=s.user_id join public.profiles p on p.id=u.id where s.id=sid and s.user_id=subject and(s.not_after is null or s.not_after>clock_timestamp()) and u.deleted_at is null and(u.banned_until is null or u.banned_until<=clock_timestamp()) and u.email_confirmed_at is not null and p.is_admin and coalesce(to_jsonb(p)->>'account_status','ACTIVE') in('ACTIVE','PENDING_VERIFICATION','REVERIFICATION_REQUIRED'));
end$$;
create or replace function cbgpu_ad_reports.device(unit text) returns text language sql immutable set search_path='' as $$select case unit when 'DAN-kVpBmlPRbOvGb6oi' then 'DESKTOP' when 'DAN-vpZNfcj3tj0jMkhV' then 'MOBILE' else 'UNKNOWN' end$$;
create or replace function cbgpu_ad_reports.decimal(value numeric) returns text language sql immutable set search_path='' as $$select case when position('.' in round(value,6)::text)>0 then rtrim(rtrim(round(value,6)::text,'0'),'.') else value::text end$$;
create or replace function cbgpu_ad_reports.rows(payload jsonb) returns table(start_date date,end_date date,unit_id text,revenue_micros bigint,impressions bigint,clicks bigint,requests bigint) language plpgsql immutable set search_path='' as $$begin
 if jsonb_typeof(payload->'rows') is distinct from 'array' or jsonb_array_length(payload->'rows') not between 1 and 5000 then raise exception 'INVALID_REPORT_ROWS' using errcode='PT400';end if;
 return query select x.start_date,x.end_date,x.unit_id,x.revenue_micros,x.impressions,x.clicks,x.requests from jsonb_to_recordset(payload->'rows') as x(start_date date,end_date date,unit_id text,revenue_micros bigint,impressions bigint,clicks bigint,requests bigint);
end$$;
create or replace function cbgpu_ad_reports.summary(data jsonb,kind text,zone text,from_date date,to_date date) returns jsonb language sql stable security definer set search_path='' as $$
with source as(select *,cbgpu_ad_reports.device(unit_id) device from jsonb_to_recordset(data) x(start_date date,end_date date,unit_id text,revenue_micros numeric,impressions numeric,clicks numeric,requests numeric)),
grouped as(select device,sum(revenue_micros) revenue,case when count(*)=count(impressions) then sum(impressions) end impressions,case when count(*)=count(clicks) then sum(clicks) end clicks,case when count(*)=count(requests) then sum(requests) end requests from source group by device),
daily as(select start_date as report_date,device,sum(revenue_micros) revenue from source where start_date=end_date group by 1,2),
monthly as(select to_char(start_date,'YYYY-MM') as month_label,device,sum(revenue_micros) revenue from source where date_trunc('month',start_date)=date_trunc('month',end_date) group by 1,2)
select jsonb_build_object('kind',kind,'currency','KRW','timezone',zone,'period_start',from_date,'period_end',to_date,'source','MANUAL_OFFICIAL_REPORT','official_console_verified',false,
'devices',coalesce((select jsonb_agg(jsonb_build_object('device',device,'revenue_krw',cbgpu_ad_reports.decimal(revenue/1000000),'impressions',impressions::text,'clicks',clicks::text,'requests',requests::text,'ecpm_krw',case when impressions>0 then cbgpu_ad_reports.decimal(revenue*1000/impressions/1000000) end,'ctr_percent',case when impressions>0 and clicks is not null then cbgpu_ad_reports.decimal(clicks*100/impressions) end,'rpuv_krw',null,'rps_krw',null,'placement_revenue_krw',null,'fill_rate_percent',null,'fill_rate_reason','REQUEST_SEMANTICS_NOT_VERIFIED','denominator_reason','NO_ALIGNED_AGGREGATE','placement_reason','SHARED_AD_UNIT_ALL_PAGES') order by device) from grouped),'[]'::jsonb),
'daily',coalesce((select jsonb_agg(jsonb_build_object('date',report_date,'device',device,'revenue_krw',cbgpu_ad_reports.decimal(revenue/1000000),'coverage_verified',false) order by report_date,device) from daily),'[]'::jsonb),
'monthly',coalesce((select jsonb_agg(jsonb_build_object('month',month_label,'device',device,'revenue_krw',cbgpu_ad_reports.decimal(revenue/1000000),'coverage_verified',false) order by month_label,device) from monthly),'[]'::jsonb),
'warnings',to_jsonb(array_remove(array[case when jsonb_array_length(data)=0 then 'NO_REPORT_DATA' end,case when exists(select 1 from source where device='UNKNOWN') then 'UNKNOWN_AD_UNITS' end,case when exists(select 1 from source where date_trunc('month',start_date)<>date_trunc('month',end_date)) then 'CROSS_MONTH_PERIOD_NOT_ALLOCATED' end],null)))
$$;
create or replace function cbgpu_ad_reports.experiment(e cbgpu_ad_reports.experiments) returns jsonb language sql stable set search_path='' as $$select to_jsonb(e)-'created_by'||jsonb_build_object('phase',case when e.console_verified then 'MANUALLY_VERIFIED' else 'PLANNED' end,'verification_source','ADMIN_ATTESTATION','provider_verified',false,'attribution_available',false,'attribution_reason','SHARED_AD_UNIT_ALL_PAGES')$$;
create or replace function public.cbgpu_ad_revenue_api(action text,payload jsonb default '{}'::jsonb) returns jsonb language plpgsql volatile security definer set search_path='' as $$
declare start_at date;end_at date;report_kind text:=coalesce(payload->>'kind','ESTIMATED');zone text:=coalesce(payload->>'timezone','Asia/Seoul');canonical text;computed_hash text;batch cbgpu_ad_reports.imports;metrics jsonb;data jsonb;conflicts integer;result jsonb;ex cbgpu_ad_reports.experiments;expected integer;begin
 if not cbgpu_ad_reports.admin() then raise exception 'FRESH_PLATFORM_ADMIN_REQUIRED' using errcode='PT403';end if;
 if jsonb_typeof(payload) is distinct from 'object' or octet_length(payload::text)>3*1024*1024 then raise exception 'INVALID_REPORT_BODY' using errcode='PT400';end if;
 if action='authorize' then return jsonb_build_object('ok',true);end if;
 if action='imports' then return jsonb_build_object('items',coalesce((select jsonb_agg(to_jsonb(i)-'created_by' order by created_at desc) from(select * from cbgpu_ad_reports.imports order by created_at desc limit 50)i),'[]'::jsonb),'limit',50);end if;
 if action='experiments' then return jsonb_build_object('items',coalesce((select jsonb_agg(cbgpu_ad_reports.experiment(e) order by updated_at desc) from(select * from cbgpu_ad_reports.experiments order by updated_at desc limit 100)e),'[]'::jsonb));end if;
 if report_kind not in('ESTIMATED','CONFIRMED') or zone not in('Asia/Seoul','UTC') then raise exception 'INVALID_REPORT_KIND_OR_TIMEZONE' using errcode='PT400';end if;
 if action='analytics' then
  if payload->>'from' is null or payload->>'to' is null then raise exception 'EXPLICIT_DATE_RANGE_REQUIRED' using errcode='PT400';end if;
  start_at:=(payload->>'from')::date;end_at:=(payload->>'to')::date;
  if start_at>end_at or end_at-start_at>3660 then raise exception 'INVALID_REPORT_RANGE' using errcode='PT400';end if;
  select coalesce(jsonb_agg(to_jsonb(f)),'[]'::jsonb) into data from cbgpu_ad_reports.facts f where f.kind=report_kind and f.timezone=zone and f.start_date>=start_at and f.end_date<=end_at;
  result:=cbgpu_ad_reports.summary(data,report_kind,zone,start_at,end_at)||jsonb_build_object('reports_count',(select count(distinct import_id) from cbgpu_ad_reports.facts f where f.kind=report_kind and f.timezone=zone and f.start_date>=start_at and f.end_date<=end_at));
  if exists(select 1 from cbgpu_ad_reports.facts f where f.kind=report_kind and f.timezone=zone and f.start_date<=end_at and f.end_date>=start_at and not(f.start_date>=start_at and f.end_date<=end_at)) then result:=jsonb_set(result,'{warnings}',(result->'warnings')||'"PARTIAL_PERIOD_EXCLUDED"'::jsonb);end if;
  return result;
 end if;
 if action in('experiment_create','experiment_update') then
  if exists(select 1 from jsonb_object_keys(payload) k where k not in('id','name','device','unit_id','period_start','period_end','timezone','interval_seconds','console_verified','notes','expected_revision')) or length(btrim(payload->>'name')) not between 1 and 120 or length(coalesce(payload->>'notes',''))>2000 or coalesce(payload->>'device','') not in('DESKTOP','MOBILE') or cbgpu_ad_reports.device(payload->>'unit_id') is distinct from payload->>'device' or(payload->>'interval_seconds')::integer not in(60,120) or jsonb_typeof(payload->'console_verified') is distinct from 'boolean' then raise exception 'INVALID_EXPERIMENT_METADATA' using errcode='PT400';end if;
  start_at:=(payload->>'period_start')::date;end_at:=(payload->>'period_end')::date;if start_at is null or end_at is null or start_at>end_at or end_at-start_at>366 then raise exception 'INVALID_EXPERIMENT_WINDOW' using errcode='PT400';end if;
  if action='experiment_create' and payload?'id' then raise exception 'SERVER_EXPERIMENT_ID_REQUIRED' using errcode='PT400';end if;
 if action='experiment_create' then insert into cbgpu_ad_reports.experiments(name,device,unit_id,period_start,period_end,timezone,interval_seconds,console_verified,verified_at,notes,created_by) values(btrim(payload->>'name'),payload->>'device',payload->>'unit_id',start_at,end_at,zone,(payload->>'interval_seconds')::integer,(payload->>'console_verified')::boolean,case when(payload->>'console_verified')::boolean then clock_timestamp() end,coalesce(payload->>'notes',''),auth.uid()) returning * into ex;
  else select * into ex from cbgpu_ad_reports.experiments where id=(payload->>'id')::uuid for update;if not found then raise exception 'EXPERIMENT_NOT_FOUND' using errcode='PT404';end if;expected:=(payload->>'expected_revision')::integer;if expected is distinct from ex.revision then raise exception 'EXPERIMENT_REVISION_CONFLICT' using errcode='PT409';end if;
  update cbgpu_ad_reports.experiments set name=btrim(payload->>'name'),device=payload->>'device',unit_id=payload->>'unit_id',period_start=start_at,period_end=end_at,timezone=zone,interval_seconds=(payload->>'interval_seconds')::integer,console_verified=(payload->>'console_verified')::boolean,verified_at=case when(payload->>'console_verified')::boolean then clock_timestamp() end,notes=coalesce(payload->>'notes',''),revision=revision+1,updated_at=clock_timestamp() where id=ex.id returning * into ex;end if;
  return jsonb_build_object('experiment',cbgpu_ad_reports.experiment(ex));
 end if;
 if action not in('preview','import') then raise exception 'UNKNOWN_REPORT_ACTION' using errcode='PT400';end if;
 if action='import' and payload->'confirmed' is distinct from 'true'::jsonb then raise exception 'EXPLICIT_IMPORT_CONFIRMATION_REQUIRED' using errcode='PT409';end if;
 if payload->>'currency' is distinct from 'KRW' or coalesce(payload->>'granularity','') not in('DAY','PERIOD') or coalesce(payload->>'source_hash','') !~ '^[a-f0-9]{64}$' or coalesce(payload->>'preview_hash','') !~ '^[a-f0-9]{64}$' then raise exception 'INVALID_REPORT_PROVENANCE' using errcode='PT400';end if;
 start_at:=(payload->>'period_start')::date;end_at:=(payload->>'period_end')::date;if start_at is null or end_at is null or start_at>end_at or end_at-start_at>3660 then raise exception 'INVALID_REPORT_PERIOD' using errcode='PT400';end if;
 if exists(select 1 from cbgpu_ad_reports.rows(payload) r where r.start_date is null or r.end_date is null or r.start_date>r.end_date or r.start_date<start_at or r.end_date>end_at or r.unit_id is null or r.unit_id !~ '^DAN-[A-Za-z0-9]{2,64}$' or r.revenue_micros is null or r.impressions<0 or r.clicks<0 or r.requests<0 or r.impressions>1000000000000 or r.clicks>1000000000000 or r.requests>1000000000000 or(payload->>'granularity'='DAY' and r.start_date<>r.end_date)or(payload->>'granularity'='PERIOD' and(r.start_date<>start_at or r.end_date<>end_at))) then raise exception 'INVALID_NORMALIZED_REPORT_ROW' using errcode='PT400';end if;
 if exists(select 1 from(select r.start_date,max(r.end_date) over(partition by r.unit_id order by r.start_date,r.end_date rows between unbounded preceding and 1 preceding) previous_end from cbgpu_ad_reports.rows(payload)r) ordered where start_date<=previous_end) then raise exception 'OVERLAPPING_REPORT_ROWS' using errcode='PT400';end if;
 select report_kind||'|KRW|'||zone||E'\n'||string_agg(to_char(r.start_date,'YYYY-MM-DD')||'|'||to_char(r.end_date,'YYYY-MM-DD')||'|'||r.unit_id||'|'||r.revenue_micros::text||'|'||coalesce(r.impressions::text,'~')||'|'||coalesce(r.clicks::text,'~')||'|'||coalesce(r.requests::text,'~'),E'\n' order by r.start_date,r.end_date,r.unit_id collate "C")||E'\n' into canonical from cbgpu_ad_reports.rows(payload)r;
 computed_hash:=encode(sha256(convert_to(canonical,'UTF8')),'hex');if computed_hash is distinct from payload->>'preview_hash' then raise exception 'REPORT_PREVIEW_CHANGED' using errcode='PT409';end if;
 select * into batch from cbgpu_ad_reports.imports where preview_hash=computed_hash;
 if action='import' and found then return jsonb_build_object('ok',true,'id',batch.id,'duplicate',true,'rows_count',batch.rows_count);end if;
 if action='import' and not pg_try_advisory_xact_lock(hashtextextended('cbgpu-ad-report:'||report_kind||':'||zone,0)) then raise exception 'REPORT_IMPORT_BUSY' using errcode='PT409';end if;
 -- Recheck after lock acquisition and before the final commit-boundary return.
 if not cbgpu_ad_reports.admin() then raise exception 'ADMIN_SESSION_REVOKED' using errcode='PT403';end if;
 -- The advisory lock serializes overlap checks; recheck duplicate after the lock.
 select * into batch from cbgpu_ad_reports.imports where preview_hash=computed_hash;
 if action='import' and found then return jsonb_build_object('ok',true,'id',batch.id,'duplicate',true,'rows_count',batch.rows_count);end if;
 select count(*)::integer into conflicts from cbgpu_ad_reports.rows(payload) r join cbgpu_ad_reports.facts f on f.kind=report_kind and f.timezone=zone and f.unit_id=r.unit_id and f.start_date<=r.end_date and f.end_date>=r.start_date;
 metrics:=cbgpu_ad_reports.summary(payload->'rows',report_kind,zone,start_at,end_at);
 if action='preview' then return metrics||jsonb_build_object('preview_hash',computed_hash,'rows_count',jsonb_array_length(payload->'rows'),'conflicts',case when batch.id is not null then 0 else conflicts end);end if;
 if conflicts>0 then raise exception 'REPORT_PERIOD_OVERLAP' using errcode='PT409';end if;
 insert into cbgpu_ad_reports.imports(preview_hash,source_hash,kind,currency,timezone,period_start,period_end,granularity,rows_count,created_by) values(computed_hash,payload->>'source_hash',report_kind,'KRW',zone,start_at,end_at,payload->>'granularity',jsonb_array_length(payload->'rows'),auth.uid()) returning * into batch;
 insert into cbgpu_ad_reports.facts(import_id,kind,timezone,unit_id,start_date,end_date,revenue_micros,impressions,clicks,requests) select batch.id,report_kind,zone,r.unit_id,r.start_date,r.end_date,r.revenue_micros,r.impressions,r.clicks,r.requests from cbgpu_ad_reports.rows(payload)r;
 if not cbgpu_ad_reports.admin() then raise exception 'ADMIN_SESSION_REVOKED' using errcode='PT403';end if;
 return jsonb_build_object('ok',true,'id',batch.id,'duplicate',false,'rows_count',batch.rows_count);
exception when invalid_text_representation or invalid_datetime_format or invalid_parameter_value or datetime_field_overflow or numeric_value_out_of_range or check_violation or not_null_violation then raise exception 'INVALID_REPORT_VALUE' using errcode='PT400';
end$$;
revoke all on all functions in schema cbgpu_ad_reports from public,anon,authenticated,service_role;
revoke all on function public.cbgpu_ad_revenue_api(text,jsonb) from public,anon,service_role;
grant execute on function public.cbgpu_ad_revenue_api(text,jsonb) to authenticated;
commit;
