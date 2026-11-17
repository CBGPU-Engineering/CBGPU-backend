begin;
-- JSON envelope is one row, immune to PostgREST max_rows truncation. It is
-- generated from one statement snapshot; a partial set must never cause reset.
create or replace function public.cbgpu_service_gpu_snapshot() returns jsonb language plpgsql stable security definer set search_path='' as $$
declare rows jsonb;n integer;begin
 select coalesce(jsonb_agg(jsonb_build_object('id',r.id,'gpu_id',r.gpu_id,'user_id',r.user_id,'start_time',r.start_time,'end_time',r.end_time,'status',r.status) order by r.id),'[]'),count(*) into rows,n from public.reservations r where r.status='approved' and r.end_time>now() and cbgpu_accounts.can_user_use_resource(r.user_id,'gpu','cbsh');
 if n>10000 then raise exception 'GPU_ASSIGNMENT_SNAPSHOT_TOO_LARGE' using errcode='54000';end if;
 return jsonb_build_object('version',1,'school_id','cbsh','complete',true,'checked_at',statement_timestamp(),'count',n,'reservations',rows);
end$$;
revoke all on function public.cbgpu_service_gpu_snapshot() from public,anon,authenticated;
grant execute on function public.cbgpu_service_gpu_snapshot() to service_role;
commit;
